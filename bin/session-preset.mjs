#!/usr/bin/env node
/**
 * Смена пресета существующей сессии — офлайн-инструмент (спека 006, задача #064).
 *
 * ## Зачем
 *
 * В DSH нельзя сменить пресет уже стартовавшей сессии: реестр пресетов жёстко
 * бросает `agent-preset/locked` (`turnBoundary.openTurnStartSeq !== null` или
 * `lastTurn > 0`), это не UI-ограничение. После потери определений пресетов
 * (issue #1) или «слетания» роли остаётся только архив сессии — на парке из сотен
 * проектов это неприемлемо.
 *
 * ## Как
 *
 * Лог сессии append-only: `session.v4.jsonl.zstd` — конкатенация независимых
 * zstd-кадров, первый кадр — заголовок (`{type:"session", version:4, …}`), далее
 * один кадр на батч событий (сейчас — одно событие на строку). Записанные кадры
 * никогда не переписываются. Текущий пресет — проекция `agentPreset`: она
 * инициализируется из заголовка и продвигается событием
 * `{"type":"agent-preset/selected","seq":N,"time":ms,"data":{"agentPreset":id}}`.
 *
 * Значит законная офлайн-смена — досыпать в закрытый лог ровно один новый
 * zstd-кадр с таким событием (`seq = lastSeq + 1`): при следующем старте DSH
 * соберёт сессию по новой проекции. Правило «model-visible ⟺ logged» соблюдено —
 * решение записано в лог. Оригинальные кадры не трогаются.
 *
 * ## Ограничения (осознанные)
 *
 * - **Нужен рестарт DSH.** Живая смена невозможна: реестр DSH её запрещает.
 * - **Только между ходами.** Если в логе открытый ход (`turn/start` без
 *   `turn/end`), инструмент отказывается без `--force`: менять роль посреди
 *   рассуждения — осознанное решение, а не случайность.
 * - **При закрытом DSH.** Живой процесс держит `flock` на `<сессия>/session.lock`
 *   — если блокировка занята (или есть живой `SingletonLock`), инструмент
 *   отказывается: одновременная запись в один лог не поддерживается.
 * - **Битый/оборванный хвост.** Если файл не v4, не декодируется или в хвосте
 *   torn-кадр, инструмент отказывается: дописывать за оборванным кадром нельзя,
 *   а переписывать оригинальные кадры — против правил формата. Такой хвост
 *   усечёт сам DSH при следующей записи — повторите после его старта.
 * - **Карта ролей.** Сторож заполняет `.kanban/.watcher-roles.json` по живым
 *   событиям выбора пресета, а офлайн-событие при рестарте не эмитится. Поэтому
 *   инструмент сам находит доску (ближайший родитель cwd сессии с `.kanban/`) и
 *   обновляет карту: роль из `--role` или по имени пресета для четырёх ролей
 *   (`dispatcher/worker/reviewer/verifier`); `--no-roles` — не трогать. Пресет
 *   вне ролей (например `standard`) снимает запись этой сессии из карты.
 *
 * ## Использование
 *
 * См. `bin/session-preset.sh --help`; логика CLI — в `main()` ниже, оболочка
 * `bin/session-preset.sh` только передаёт аргументы в `bin/node.sh`.
 *
 * Коды возврата: 0 — ок; 1 — отказ или ошибка (DSH запущен, неизвестный пресет,
 * битый лог, открытый ход, не удалось записать, сессия не найдена); 2 — ошибка
 * аргументов (неизвестный флаг, нет цели, нет `--to`, нет терминала без `--yes`).
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import zlib from "node:zlib";

/** Магия zstd-кадра (little-endian). */
const ZSTD_MAGIC = 0xfd2fb528;

/** Имя текущего поколения лога: `session.v4.jsonl.zstd`. */
export const LOG_FILE = "session.v4.jsonl.zstd";

/** Версия формата лога, который умеет читать инструмент. */
export const LOG_VERSION = 4;

/** Роли доски: id пресета совпадает с именем роли (допускаем префикс `preset-`). */
export const ROLES = ["dispatcher", "worker", "reviewer", "verifier"];

/** Пресеты, которые считаются известными, пока нет `presets/role-presets.json`. */
const FALLBACK_PRESETS = ["standard", ...ROLES];

/** Каталог бэкапов логов внутри DSH_HOME. */
export const BACKUP_DIR = "session-backups";

/** Событие выбора пресета — единственное, что досыпает инструмент. */
const PRESET_EVENT = "agent-preset/selected";

/** Параметры zstd для наших кадров: как у DSH — контрольная сумма кадра. */
const ZSTD_OPTIONS = { params: { [zlib.constants.ZSTD_c_checksumFlag]: 1 } };

// ── чтение лога ──────────────────────────────────────────────────────────────

/**
 * Разбить файл лога на отдельные zstd-кадры, не распаковывая их.
 *
 * Формат кадра: магия (4 байта), заголовок кадра (1–14 байт), блоки (по 3 байта
 * заголовок + полезная нагрузка), после последнего блока — контрольная сумма,
 * если она включена. Размер кадра вычислим без декодирования данных, поэтому
 * чтение дешёвое.
 *
 * @param buf - содержимое лога.
 * @returns `{ frames, torn }`: кадры по порядку и длина оборванного хвоста
 *   (`0` — файл кончается ровно на границе кадра).
 */
export function splitFrames(buf) {
  const frames = [];
  let pos = 0;
  while (pos + 4 <= buf.length) {
    if (buf.readUInt32LE(pos) !== ZSTD_MAGIC) return { frames, torn: buf.length - pos };
    const start = pos;
    pos += 4;
    if (pos >= buf.length) return { frames, torn: buf.length - start };
    const descriptor = buf[pos];
    pos += 1;
    const fcsFlag = descriptor >> 6;
    const singleSegment = (descriptor >> 5) & 1;
    const hasChecksum = (descriptor >> 2) & 1;
    const dictFlag = descriptor & 3;
    // Window descriptor есть только у многосегментного кадра; словарь и размер
    // содержимого — по флагам заголовка.
    if (!singleSegment) pos += 1;
    pos += [0, 1, 2, 4][dictFlag];
    pos += fcsFlag === 0 ? (singleSegment ? 1 : 0) : [0, 2, 4, 8][fcsFlag];
    let last = 0;
    while (!last) {
      if (pos + 3 > buf.length) return { frames, torn: buf.length - start };
      const header = buf[pos] | (buf[pos + 1] << 8) | (buf[pos + 2] << 16);
      pos += 3;
      last = header & 1;
      const type = (header >> 1) & 3;
      const size = header >>> 3;
      // RLE-блок хранит один байт, raw и compressed — `size` байт.
      pos += type === 1 ? 1 : size;
      if (pos > buf.length) return { frames, torn: buf.length - start };
    }
    if (hasChecksum) pos += 4;
    if (pos > buf.length) return { frames, torn: buf.length - start };
    frames.push(buf.subarray(start, pos));
  }
  return { frames, torn: buf.length - pos };
}

/**
 * Декодировать лог: заголовок сессии и события по порядку.
 *
 * @param buf - содержимое лога.
 * @returns `{ header, events, frames, torn, problems }`: `problems` — то, из-за
 *   чего лог нельзя править (не распаковался кадр, строка не JSON, нет кадров).
 */
export function decodeLog(buf) {
  const { frames, torn } = splitFrames(buf);
  const problems = [];
  const lines = [];
  if (frames.length === 0) problems.push("в файле нет ни одного zstd-кадра");
  frames.forEach((frame, index) => {
    try {
      const text = zlib.zstdDecompressSync(frame).toString("utf8");
      for (const line of text.split("\n")) {
        if (line.trim() !== "") lines.push(line);
      }
    } catch (error) {
      problems.push(`кадр №${index + 1} не распакован: ${error?.message ?? error}`);
    }
  });
  let header = null;
  const events = [];
  lines.forEach((line, index) => {
    let value;
    try {
      value = JSON.parse(line);
    } catch (error) {
      problems.push(`строка №${index + 1} не JSON: ${error?.message ?? error}`);
      return;
    }
    if (index === 0 && value && value.type === "session") {
      header = value;
      return;
    }
    if (index === 0) {
      problems.push("первая строка лога — не заголовок сессии");
      return;
    }
    if (!value || typeof value !== "object" || typeof value.type !== "string") {
      problems.push(`строка №${index + 1} — не событие`);
      return;
    }
    events.push(value);
  });
  if (header === null) problems.push("заголовок сессии не найден");
  return { header, events, frames, torn, problems };
}

/**
 * Прочитать только заголовок лога (первый zstd-кадр): для фильтра `--cwd` нужен
 * лишь `cwd`, а полный разбор сотен логов — это гигабайты.
 */
export function readHeader(file) {
  const size = fs.statSync(file).size;
  const chunkSize = Math.min(size, 1 << 16);
  const chunk = Buffer.alloc(chunkSize);
  const fd = fs.openSync(file, "r");
  try {
    fs.readSync(fd, chunk, 0, chunkSize, 0);
  } finally {
    fs.closeSync(fd);
  }
  const { frames } = splitFrames(chunk);
  if (frames.length === 0) return null;
  const text = zlib.zstdDecompressSync(frames[0]).toString("utf8");
  const line = text.split("\n").find((item) => item.trim() !== "");
  if (!line) return null;
  const value = JSON.parse(line);
  return value && value.type === "session" ? value : null;
}

/** Прочитать и декодировать лог из файла. */
export function readLog(file) {
  return decodeLog(fs.readFileSync(file));
}

/** Номер последнего события: `seq` монотонен, поэтому берём максимум. */
export function lastSeqOf(events) {
  let last = -1;
  for (const event of events) {
    const seq = Number(event?.seq);
    if (Number.isInteger(seq) && seq > last) last = seq;
  }
  return last;
}

/** Текущий пресет: последнее событие выбора пресета либо заголовок. */
export function presetOf(header, events) {
  let preset = typeof header?.agentPreset === "string" ? header.agentPreset : "";
  for (const event of events) {
    if (event?.type !== PRESET_EVENT) continue;
    const value = event?.data?.agentPreset;
    if (typeof value === "string" && value !== "") preset = value;
  }
  return preset;
}

/**
 * Сводка по логу без разбора всех событий: для `--list` сессий бывают сотни, а
 * полный `JSON.parse` каждой строки — секунды на гигабайтах логов. Заголовок
 * разбирается как JSON (он один), дальше нужны только: последний `seq` (последняя
 * строка), последний `turn/*` (открыт ли ход) и последний `agent-preset/selected`
 * (текущий пресет) — всё это ищется с конца, без парсинга остальных событий.
 *
 * @returns `{ header, lastSeq, preset, openTurn, torn, problems }`.
 */
export function summarizeLog(buf) {
  const { frames, torn } = splitFrames(buf);
  const problems = [];
  if (frames.length === 0) problems.push("в файле нет ни одного zstd-кадра");
  const lines = [];
  for (const [index, frame] of frames.entries()) {
    try {
      const text = zlib.zstdDecompressSync(frame).toString("utf8");
      for (const line of text.split("\n")) if (line.trim() !== "") lines.push(line);
    } catch (error) {
      problems.push(`кадр №${index + 1} не распакован: ${error?.message ?? error}`);
    }
  }
  let header = null;
  if (lines.length > 0) {
    try {
      const value = JSON.parse(lines[0]);
      if (value && value.type === "session") header = value;
      else problems.push("первая строка лога — не заголовок сессии");
    } catch (error) {
      problems.push(`заголовок не JSON: ${error?.message ?? error}`);
    }
  }
  if (header === null && lines.length === 0) problems.push("заголовок сессии не найден");
  let lastSeq = -1;
  let preset = typeof header?.agentPreset === "string" ? header.agentPreset : "";
  let openTurn = false;
  let seenTurn = false;
  for (let index = lines.length - 1; index >= 1; index -= 1) {
    const line = lines[index];
    if (lastSeq < 0) {
      const match = /"seq":(\d+)/.exec(line);
      if (match) lastSeq = Number(match[1]);
    }
    if (!seenTurn) {
      const turn = /"type":"turn\/(start|end|aborted)"/.exec(line);
      if (turn) {
        openTurn = turn[1] === "start";
        seenTurn = true;
      }
    }
    if (preset === "" || preset === header?.agentPreset) {
      if (line.includes('"type":"agent-preset/selected"')) {
        const selected = /"agentPreset":"((?:[^"\\]|\\.)*)"/.exec(line);
        if (selected) preset = JSON.parse(`"${selected[1]}"`);
      }
    }
  }
  if (lastSeq < 0 && lines.length > 1) {
    // Событие без `seq` — лог всё равно читаем, но номер неизвестен.
    lastSeq = -1;
  }
  return { header, lastSeq, preset, openTurn, torn, problems };
}

/**
 * Открыт ли последний ход: последнее `turn/start` новее последнего закрытия.
 *
 * Ход закрывают `turn/end` и `turn/aborted`; `turn/start` одного хода может
 * встретиться несколько раз (перезапуск шага) — важен порядок `seq`.
 */
export function openTurnOf(events) {
  let start = -1;
  let end = -1;
  for (const event of events) {
    const seq = Number(event?.seq);
    if (!Number.isInteger(seq)) continue;
    if (event?.type === "turn/start") start = Math.max(start, seq);
    else if (event?.type === "turn/end" || event?.type === "turn/aborted") end = Math.max(end, seq);
  }
  return start > end;
}

/** Кадр с одним событием — ровно так DSH досыпает событие в лог. */
export function encodeEventFrame(event) {
  const line = JSON.stringify(event);
  return zlib.zstdCompressSync(Buffer.from(`${line}\n`, "utf8"), ZSTD_OPTIONS);
}

/** Досыпать событие одним zstd-кадром и сбросить его на диск. */
export function appendEventFrame(file, event) {
  const frame = encodeEventFrame(event);
  const fd = fs.openSync(file, "a");
  try {
    fs.writeSync(fd, frame);
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  return frame.length;
}

// ── пресеты и роли ───────────────────────────────────────────────────────────

/** Роль по id пресета: `reviewer` и `preset-reviewer` — одно и то же. */
export function roleOfPreset(agentPreset) {
  const name = String(agentPreset ?? "").replace(/^preset-/, "");
  return ROLES.includes(name) ? name : null;
}

/**
 * Известные пресеты.
 *
 * Источник правды — артефакт плагина `dsh-kanban/presets/role-presets.json`
 * (его генерирует `bin/gen-role-presets.sh`, #062): определения ролей едут
 * вместе с плагином. Корневой `presets/role-presets.json` остаётся запасным
 * путём (старые раскладки/ручные копии). `standard` — встроенный пресет DSH —
 * известен всегда, как и четыре роли плагина: их регистрирует сам плагин
 * (самолечение, спека 005), поэтому переключение на роль не должно требовать
 * `--allow-unknown`, даже если артефакт отстал.
 *
 * @returns `{ ids, source }`: `ids` — известные id в стабильном порядке (встроенные
 *   сначала), `source` — откуда взяты дополнительные id.
 */
export function knownPresets(repoRoot) {
  const candidates = [
    path.join(repoRoot, "dsh-kanban", "presets", "role-presets.json"),
    path.join(repoRoot, "presets", "role-presets.json"),
  ];
  const ids = new Set(FALLBACK_PRESETS);
  let source = null;
  for (const file of candidates) {
    let data;
    try {
      data = JSON.parse(fs.readFileSync(file, "utf8"));
    } catch {
      continue; // файла нет или он битый — пробуем следующий путь
    }
    const list = Array.isArray(data) ? data : Array.isArray(data?.presets) ? data.presets : [];
    const found = list
      .map((entry) => (typeof entry === "string" ? entry : entry?.id))
      .filter((id) => typeof id === "string" && id !== "");
    if (found.length === 0) continue;
    for (const id of found) ids.add(id);
    source = file;
    break;
  }
  return { ids: [...ids], source: source ?? "встроенный список (артефакт dsh-kanban/presets/role-presets.json не найден)" };
}

/** Ближайший родитель каталога, у которого есть `.kanban/` — доска сессии. */
export function boardForCwd(cwd) {
  let dir = path.resolve(String(cwd ?? ""));
  for (;;) {
    try {
      if (fs.statSync(path.join(dir, ".kanban")).isDirectory()) return dir;
    } catch {
      /* нет `.kanban` в этом каталоге — идём выше */
    }
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/** Файл автокарты ролей доски (тот же, что заполняет сторож). */
export function rolesFileOf(boardRoot) {
  return path.join(boardRoot, ".kanban", ".watcher-roles.json");
}

/** Карта ролей: `{ "<role>": "<sessionId>" }`; битый файл — пустая карта. */
export function readRoleMap(boardRoot) {
  try {
    const data = JSON.parse(fs.readFileSync(rolesFileOf(boardRoot), "utf8"));
    if (data && typeof data === "object" && !Array.isArray(data)) {
      const map = {};
      for (const [role, sessionId] of Object.entries(data)) {
        if (typeof sessionId === "string" && sessionId !== "") map[role] = sessionId;
      }
      return map;
    }
  } catch {
    /* файла нет или он битый — сторожа ещё не было */
  }
  return {};
}

/**
 * Записать карту ролей: снять сессию со всех ролей и (если роль есть) поставить
 * на новую. Порядок ключей — как у сторожа (роли по порядку, затем остальное),
 * формат — `JSON.stringify(..., null, 2)` + перевод строки.
 *
 * @returns `{ file, changed, map }`: `changed` — файл действительно изменился.
 */
export function writeRoleMap(boardRoot, sessionId, role) {
  const map = readRoleMap(boardRoot);
  const before = JSON.stringify(map);
  for (const [key, value] of Object.entries(map)) {
    if (value === sessionId) delete map[key];
  }
  if (role) map[role] = sessionId;
  const ordered = {};
  for (const key of ROLES) if (Object.prototype.hasOwnProperty.call(map, key)) ordered[key] = map[key];
  for (const key of Object.keys(map).sort()) if (!Object.prototype.hasOwnProperty.call(ordered, key)) ordered[key] = map[key];
  const file = rolesFileOf(boardRoot);
  const changed = JSON.stringify(ordered) !== before;
  if (changed) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, `${JSON.stringify(ordered, null, 2)}\n`);
  }
  return { file, changed, map: ordered };
}

// ── блокировки и DSH ─────────────────────────────────────────────────────────

/**
 * Состояния сессионных локов — `"free" | "held" | "unknown"` по каждому пути.
 *
 * DSH берёт неблокирующий `flock(2)` на `<сессия>/session.lock`
 * (`dsh-session-persistence-jsonl`, LEASE_FILENAME). Node не умеет flock, поэтому
 * пробуем взять тот же лок питоном (одним процессом на все пути: в `--list`
 * сессий бывает сотни). Файл открывается только на чтение — прав на запись в
 * `~/.dsh` у инструмента нет и не должно быть; stderr питона глушится: отказ
 * доступа к файлу лока — это «не проверить», а не повод сыпать трейсбеком.
 *
 * @returns массив состояний в порядке `lockPaths`.
 */
export function lockStates(lockPaths, python = "python3") {
  const result = lockPaths.map(() => "free");
  const pending = [];
  lockPaths.forEach((lockPath, index) => {
    if (!fs.existsSync(lockPath)) result[index] = "free";
    else pending.push(index);
  });
  if (pending.length === 0) return result;
  const script = [
    "import fcntl, sys",
    "handles = []",
    "for line in sys.stdin:",
    "    path = line.rstrip('\\n')",
    "    if not path:",
    "        print('unknown'); continue",
    "    try:",
    "        handle = open(path, 'r')",
    "    except OSError:",
    "        print('unknown'); continue",
    "    handles.append(handle)",
    "    try:",
    "        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)",
    "    except OSError:",
    "        print('held')",
    "    else:",
    "        print('free')",
  ].join("\n");
  let out = "";
  try {
    out = execFileSync(python, ["-c", script], {
      input: pending.map((index) => lockPaths[index]).join("\n") + "\n",
      encoding: "utf8",
      timeout: 15000,
      stdio: ["pipe", "pipe", "ignore"],
    });
  } catch (error) {
    if (error?.code === "ENOENT") {
      for (const index of pending) result[index] = "unknown";
      return result;
    }
    out = String(error?.stdout ?? "");
  }
  const lines = out.split("\n");
  pending.forEach((index, offset) => {
    const state = lines[offset];
    result[index] = state === "held" || state === "free" ? state : "unknown";
  });
  return result;
}

/** Один лок: `{ held, unknown }` (для проб и тестов). */
export function sessionLockHeld(lockPath, python = "python3") {
  const [state] = lockStates([lockPath], python);
  return { held: state === "held", unknown: state === "unknown" };
}

/** Живой ли pid (`EPERM` тоже значит «живой», просто не наш). */
export function pidAlive(pid) {
  const value = Number(pid);
  if (!Number.isInteger(value) || value <= 0) return false;
  try {
    process.kill(value, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

/**
 * `SingletonLock` десктопного DSH (Chrome-style симлинк `host-pid`), если он есть.
 * Web-профиль такой файл не пишет, поэтому это дополнительная, а не основная
 * проверка.
 *
 * @returns `{ file, pid, alive }` или `null`.
 */
export function singletonLock(home) {
  const candidates = [path.join(home, "SingletonLock"), path.join(home, "profiles", "web", "SingletonLock"), path.join(home, "profiles", "desktop", "SingletonLock")];
  for (const file of candidates) {
    try {
      const target = fs.readlinkSync(file);
      const pid = Number(String(target).split("-").pop());
      return { file, pid, alive: pidAlive(pid) };
    } catch {
      /* не симлинк или файла нет */
    }
  }
  return null;
}

// ── сессии ───────────────────────────────────────────────────────────────────

/** Каталог DSH_HOME: `DSH_HOME` или `~/.dsh`. */
export function dshHome(env = process.env) {
  const fromEnv = String(env?.DSH_HOME ?? "").trim();
  return path.resolve(fromEnv !== "" ? fromEnv : path.join(os.homedir(), ".dsh"));
}

/**
 * Найти все v4-логи под `<home>/sessions`.
 *
 * @returns массив `{ id, project, dir, file }` (порядок — по пути).
 */
export function discoverSessions(home) {
  const root = path.join(home, "sessions");
  const found = [];
  let projects = [];
  try {
    projects = fs.readdirSync(root).sort();
  } catch {
    return found;
  }
  for (const project of projects) {
    const projectDir = path.join(root, project);
    let sessions = [];
    try {
      sessions = fs.readdirSync(projectDir).sort();
    } catch {
      continue;
    }
    for (const id of sessions) {
      const dir = path.join(projectDir, id);
      const file = path.join(dir, LOG_FILE);
      try {
        if (!fs.statSync(file).isFile()) continue;
      } catch {
        continue;
      }
      found.push({ id, project, dir, file });
    }
  }
  return found;
}

/**
 * Сводка по сессии: cwd, пресет, последний seq, открытый ход, состояние лока.
 * Битый лог не валит список — проблема попадает в `problems`.
 */
function infoFromSummary(session, lock, summary, error) {
  const info = { id: session.id, file: session.file, cwd: "", preset: "", headerPreset: "", lastSeq: -1, openTurn: false, problems: [], torn: 0, lock: lock ?? "unknown" };
  if (error) {
    info.problems.push(`лог не прочитан: ${error?.message ?? error}`);
    return info;
  }
  info.torn = summary.torn;
  info.problems.push(...summary.problems);
  if (summary.header) {
    info.headerPreset = typeof summary.header.agentPreset === "string" ? summary.header.agentPreset : "";
    info.cwd = typeof summary.header.cwd === "string" ? summary.header.cwd : "";
  }
  info.preset = summary.preset;
  info.lastSeq = summary.lastSeq;
  info.openTurn = summary.openTurn;
  return info;
}

/** Сводка по сессии (синхронно): для одной сессии в плане правки. */
export function describeSession(session, python = "python3", lockState = null) {
  const lock = lockState ?? lockStates([path.join(session.dir, "session.lock")], python)[0];
  try {
    return infoFromSummary(session, lock, summarizeLog(fs.readFileSync(session.file)), null);
  } catch (error) {
    return infoFromSummary(session, lock, null, error);
  }
}

/** То же, но чтение файла асинхронное: `--list` читает сотни логов параллельно. */
export async function describeSessionAsync(session, python = "python3", lockState = null) {
  const lock = lockState ?? lockStates([path.join(session.dir, "session.lock")], python)[0];
  try {
    return infoFromSummary(session, lock, summarizeLog(await fs.promises.readFile(session.file)), null);
  } catch (error) {
    return infoFromSummary(session, lock, null, error);
  }
}

/** Формат времени для имён бэкапов: `YYYYMMDD-HHMMSS-mmm` (UTC). */
export function backupStamp(now = new Date()) {
  const iso = now.toISOString();
  return `${iso.slice(0, 10).replace(/-/g, "")}-${iso.slice(11, 19).replace(/:/g, "")}-${String(now.getUTCMilliseconds()).padStart(3, "0")}`;
}

/** Каталог бэкапов (создаётся по требованию). */
export function backupPath(home, id, suffix = "", now = new Date()) {
  const name = `${id}.${backupStamp(now)}${suffix}.zst`;
  return path.join(home, BACKUP_DIR, name);
}

/** Скопировать лог в бэкап; возвращает путь бэкапа. */
export function backupLog(home, session, suffix = "") {
  const target = backupPath(home, session.id, suffix);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.copyFileSync(session.file, target);
  fs.chmodSync(target, 0o600);
  return target;
}

// ── план и применение ────────────────────────────────────────────────────────

/**
 * Проверить одну сессию и собрать план правки.
 *
 * Отказы (попадают в `problems`): лог не читается/не v4, битые кадры или строки,
 * оборванный хвост, живой DSH. Открытый ход — отдельный флаг `openTurn`:
 * без `--force` это отказ, с `--force` — предупреждение.
 */
export function planSessionChange(session, options) {
  const problems = [];
  const info = describeSession(session, options.python, options.lock ?? null);
  problems.push(...info.problems);
  if (info.problems.length === 0) {
    // Полная проверка — только для тех сессий, которые правда собираемся менять:
    // здесь важно увидеть битую строку в любом месте лога, а не только в хвосте.
    let log = null;
    try {
      log = readLog(session.file);
    } catch (error) {
      problems.push(`лог не прочитан: ${error?.message ?? error}`);
    }
    if (log !== null) {
      problems.push(...log.problems);
      if (log.header && Number(log.header.version) !== LOG_VERSION) problems.push(`формат лога v${log.header.version}, поддерживается только v${LOG_VERSION}`);
      if (log.torn > 0) problems.push(`оборванный хвост лога (${log.torn} байт): дождитесь старта DSH, он усечёт хвост, и повторите`);
    }
  }
  if (info.lock === "held") problems.push("DSH держит сессию: закройте DSH и повторите (сессионный flock занят)");
  if (info.lock === "unknown") problems.push("не удалось проверить, закрыт ли DSH (нет python3 для пробы flock): запуск без проверки запрещён");
  const role = options.role ?? roleOfPreset(options.to);
  const plan = {
    session,
    info,
    problems,
    openTurn: info.openTurn,
    from: info.preset,
    to: options.to,
    seq: info.lastSeq + 1,
    role: role ?? null,
    board: options.noRoles ? null : boardForCwd(info.cwd),
    backup: options.backup ? backupPath(options.home, session.id) : null,
  };
  if (info.openTurn && !options.force) {
    plan.problems.push("в сессии открытый ход (turn/start без turn/end): сменить роль посреди хода можно только с --force");
  }
  return plan;
}

/**
 * Применить план: бэкап, один новый кадр, проверка, карта ролей.
 *
 * @returns `{ ok, error?, backup?, appended?, role? }`.
 */
export function applyPlan(plan, options) {
  const { session } = plan;
  if (plan.problems.length > 0) return { ok: false, error: plan.problems.join("; ") };
  let backup = null;
  try {
    if (options.backup) backup = backupLog(options.home, session);
  } catch (error) {
    return { ok: false, error: `бэкап не создан: ${error?.message ?? error}` };
  }
  // Окно между планом и записью: пользователь мог запустить DSH. Перепроверяем
  // лок прямо перед досыпанием — запись в открытый лог недопустима.
  const lock = sessionLockHeld(path.join(session.dir, "session.lock"), options.python ?? "python3");
  if (lock.held) return { ok: false, error: "DSH запустился между проверкой и записью (сессионный flock занят): ничего не изменено", backup };
  if (lock.unknown) return { ok: false, error: "не удалось перепроверить блокировку перед записью: ничего не изменено", backup };
  const before = fs.readFileSync(session.file);
  const event = { type: PRESET_EVENT, seq: plan.seq, time: Date.now(), data: { agentPreset: plan.to } };
  let appended = 0;
  try {
    appended = appendEventFrame(session.file, event);
  } catch (error) {
    return { ok: false, error: `не удалось досыпать событие: ${error?.message ?? error}`, backup };
  }
  // Проверка после записи: оригинальные кадры на месте, хвост читается, последнее
  // событие — наше.
  const after = fs.readFileSync(session.file);
  const log = decodeLog(after);
  const last = log.events[log.events.length - 1];
  if (!after.subarray(0, before.length).equals(before)) {
    return { ok: false, error: "после записи изменились оригинальные кадры (это баг инструмента)", backup };
  }
  if (log.torn > 0 || log.problems.length > 0) {
    return { ok: false, error: `лог после записи не читается: ${log.problems.join("; ") || `torn ${log.torn}`}`, backup };
  }
  if (!last || last.type !== PRESET_EVENT || last.seq !== event.seq || last?.data?.agentPreset !== plan.to) {
    return { ok: false, error: "проверка после записи не увидела наше событие", backup };
  }
  let role = null;
  if (!options.noRoles) {
    if (plan.board) {
      try {
        role = writeRoleMap(plan.board, session.id, plan.role);
      } catch (error) {
        return { ok: false, error: `событие записано, но карта ролей не обновлена: ${error?.message ?? error}`, backup, appended };
      }
    } else {
      role = { file: null, changed: false, map: {}, missing: true };
    }
  }
  return { ok: true, backup, appended, role, event };
}

/** Восстановить лог из бэкапа (с бэкапом текущего состояния). */
export function restoreLog(options) {
  const backupFile = path.resolve(options.restore);
  if (!fs.existsSync(backupFile)) return { ok: false, error: `бэкап не найден: ${backupFile}` };
  let decoded;
  try {
    decoded = readLog(backupFile);
  } catch (error) {
    return { ok: false, error: `бэкап не прочитан: ${error?.message ?? error}` };
  }
  if (!decoded.header || Number(decoded.header.version) !== LOG_VERSION) {
    return { ok: false, error: "в бэкапе не v4-лог сессии" };
  }
  if (decoded.problems.length > 0) return { ok: false, error: `бэкап битый: ${decoded.problems.join("; ")}` };
  const sessionId = options.session ?? path.basename(backupFile).split(".")[0];
  const session = discoverSessions(options.home).find((item) => item.id === sessionId);
  if (!session) return { ok: false, error: `сессия ${sessionId} не найдена в ${path.join(options.home, "sessions")}` };
  const lock = sessionLockHeld(path.join(session.dir, "session.lock"), options.python);
  if (lock.held) return { ok: false, error: "DSH держит сессию: закройте DSH и повторите" };
  if (lock.unknown) return { ok: false, error: "не удалось проверить, закрыт ли DSH (нет python3 для пробы flock)" };
  const safety = backupLog(options.home, session, ".pre-restore");
  try {
    fs.copyFileSync(backupFile, session.file);
    fs.chmodSync(session.file, 0o600);
  } catch (error) {
    return { ok: false, error: `не удалось восстановить лог: ${error?.message ?? error}` };
  }
  const check = readLog(session.file);
  if (check.problems.length > 0 || !check.header) return { ok: false, error: `после восстановления лог не читается: ${check.problems.join("; ")}`, safety, session };
  return { ok: true, safety, session, log: check };
}

// ── CLI ──────────────────────────────────────────────────────────────────────

/** Текст справки (тот же, что печатает `--help`). */
export const USAGE = `Использование:
  bin/session-preset.sh --list
  bin/session-preset.sh --session <id|prefix> --to <preset> [опции]
  bin/session-preset.sh --cwd DIR --from <preset> --to <preset> [опции]
  bin/session-preset.sh --all --from <preset> --to <preset> [опции]
  bin/session-preset.sh --restore <бэкап> [--session <id>]

Смена пресета существующей сессии офлайн: при закрытом DSH досыпает в
session.v4.jsonl.zstd один zstd-кадр с событием agent-preset/selected. Нужен
перезапуск DSH — живая смена запрещена реестром пресетов.

  --list                список сессий: id, cwd, пресет, последний seq, состояние
  --session <id|prefix> одна сессия (id или уникальный префикс)
  --cwd DIR             сессии, чей cwd — DIR или внутри него
  --all                 все сессии
  --from <preset>       только сессии с этим текущим пресетом
  --to <preset>         новый пресет (обязателен для смены)
  --role <role>         роль для карты доски: dispatcher|worker|reviewer|verifier
                        (по умолчанию — по имени пресета для четырёх ролей)
  --dry-run             показать план, ничего не менять
  --backup              создать бэкап (по умолчанию и так создаётся)
  --no-backup           не создавать бэкап (осознанно)
  --restore <файл>      восстановить лог из бэкапа
  --allow-unknown       разрешить пресет вне presets/role-presets.json
  --force               разрешить смену при открытом ходе
  --no-roles            не трогать .kanban/.watcher-roles.json
  --yes                 не спрашивать подтверждения
  --json                машинный вывод (--list и план)
  -v, --verbose         подробный вывод
  -h, --help            эта справка

Коды возврата: 0 — ок; 1 — отказ/ошибка; 2 — ошибка аргументов.
`;

/** Разобрать аргументы. Возвращает `{ ok, value }` либо `{ ok: false, message }`. */
export function parseArgs(argv) {
  const value = {
    list: false,
    session: null,
    cwd: null,
    all: false,
    from: null,
    to: null,
    role: null,
    dryRun: false,
    backup: true,
    restore: null,
    allowUnknown: false,
    force: false,
    noRoles: false,
    yes: false,
    json: false,
    verbose: false,
    help: false,
  };
  const need = (index, flag) => {
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) throw new Error(`${flag} требует значения`);
    return next;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    try {
      switch (arg) {
        case "--list": value.list = true; break;
        case "--session": value.session = need(index, "--session"); index += 1; break;
        case "--cwd": value.cwd = need(index, "--cwd"); index += 1; break;
        case "--all": value.all = true; break;
        case "--from": value.from = need(index, "--from"); index += 1; break;
        case "--to": value.to = need(index, "--to"); index += 1; break;
        case "--role": value.role = need(index, "--role"); index += 1; break;
        case "--dry-run": value.dryRun = true; break;
        case "--backup": value.backup = true; break;
        case "--no-backup": value.backup = false; break;
        case "--restore": value.restore = need(index, "--restore"); index += 1; break;
        case "--allow-unknown": value.allowUnknown = true; break;
        case "--force": value.force = true; break;
        case "--no-roles": value.noRoles = true; break;
        case "--yes": value.yes = true; break;
        case "--json": value.json = true; break;
        case "-v":
        case "--verbose": value.verbose = true; break;
        case "-h":
        case "--help": value.help = true; break;
        default:
          return { ok: false, message: `неизвестный аргумент: ${arg}` };
      }
    } catch (error) {
      return { ok: false, message: error?.message ?? String(error) };
    }
  }
  if (value.role !== null && !ROLES.includes(value.role)) {
    return { ok: false, message: `--role: ожидается ${ROLES.join("|")}, получено ${value.role}` };
  }
  const selectors = [value.session !== null, value.cwd !== null, value.all].filter(Boolean).length;
  if (value.help || value.list) {
    if (value.list && (value.to !== null || value.restore !== null)) return { ok: false, message: "--list не сочетается с --to/--restore" };
    return { ok: true, value };
  }
  if (value.restore !== null) {
    if (value.to !== null) return { ok: false, message: "--restore не сочетается с --to" };
    return { ok: true, value };
  }
  if (selectors === 0) return { ok: false, message: "не указана цель: --session <id>, --cwd DIR, --all или --list" };
  if (selectors > 1) return { ok: false, message: "цель выбирается одним способом: --session, --cwd или --all" };
  if (value.to === null) return { ok: false, message: "не указан новый пресет: --to <preset>" };
  return { ok: true, value };
}

/** Совпадает ли cwd сессии с фильтром `--cwd` (каталог или вложенный в него). */
export function cwdMatches(sessionCwd, filter) {
  const target = path.resolve(String(filter));
  const cwd = path.resolve(String(sessionCwd ?? ""));
  return cwd === target || cwd.startsWith(target + path.sep);
}

/** Короткий id для вывода: `session-<uuid>` → оставляем как есть, но целиком. */
function sessionLabel(id) {
  return String(id);
}

/** Человеческий статус лока. */
function lockLabel(lock) {
  if (lock === "held") return "DSH держит";
  if (lock === "unknown") return "лок не проверен";
  return "свободна";
}

/**
 * Точка входа CLI.
 *
 * @param argv - аргументы без имени скрипта.
 * @param io - `{ out, err, stdin, repoRoot, env, python, now }` для тестов.
 * @returns код возврата (0/1/2).
 */
export async function main(argv, io = {}) {
  const out = io.out ?? process.stdout;
  const err = io.err ?? process.stderr;
  const stdin = io.stdin ?? process.stdin;
  const env = io.env ?? process.env;
  const repoRoot = io.repoRoot ?? path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const python = io.python ?? "python3";
  const now = io.now ?? (() => new Date());
  const say = (text = "") => out.write(`${text}\n`);
  const warn = (text = "") => err.write(`${text}\n`);

  const parsed = parseArgs(argv);
  if (!parsed.ok) {
    warn(`session-preset: ${parsed.message}`);
    warn("подсказка: bin/session-preset.sh --help");
    return 2;
  }
  const options = parsed.value;
  if (options.help) {
    out.write(USAGE);
    return 0;
  }

  const home = dshHome(env);
  const presets = knownPresets(repoRoot);

  if (options.list) {
    const sessions = discoverSessions(home);
    const locks = lockStates(sessions.map((session) => path.join(session.dir, "session.lock")), python);
    const rows = await mapLimit(sessions, 8, (session, index) => describeSessionAsync(session, python, locks[index]));
    if (options.json) {
      say(JSON.stringify(rows.map((row) => ({ id: row.id, cwd: row.cwd, preset: row.preset, headerPreset: row.headerPreset, lastSeq: row.lastSeq, openTurn: row.openTurn, lock: row.lock, file: row.file, problems: row.problems, torn: row.torn })), null, 2));
      return 0;
    }
    if (rows.length === 0) {
      say(`сессий нет: ${path.join(home, "sessions")}`);
      return 0;
    }
    say(`сессии в ${path.join(home, "sessions")}: ${rows.length}`);
    for (const row of rows) {
      const preset = row.preset === row.headerPreset ? row.preset : `${row.headerPreset}→${row.preset}`;
      const flags = [lockLabel(row.lock), row.openTurn ? "открытый ход" : "ход закрыт"];
      if (row.problems.length > 0) flags.push(`проблемы: ${row.problems.length}`);
      say(`  ${sessionLabel(row.id)}`);
      say(`    cwd: ${row.cwd || "?"}`);
      say(`    пресет: ${preset || "?"}  seq: ${row.lastSeq}  ${flags.join(", ")}`);
    }
    return 0;
  }

  if (options.restore !== null) {
    const result = restoreLog({ home, restore: options.restore, session: options.session, python });
    if (!result.ok) {
      warn(`session-preset: ${result.error}`);
      return 1;
    }
    say(`лог восстановлен из бэкапа: ${result.session.file}`);
    say(`страховочный бэкап текущего состояния: ${result.safety}`);
    say("перезапустите DSH, чтобы изменения вступили в силу");
    return 0;
  }

  if (!options.allowUnknown && !presets.ids.includes(options.to)) {
    warn(`session-preset: пресет «${options.to}» неизвестен (источник: ${presets.source})`);
    warn(`известные: ${presets.ids.join(", ")}; осознанный обход — --allow-unknown`);
    return 1;
  }

  const sessions = discoverSessions(home);
  if (sessions.length === 0) {
    warn(`session-preset: сессий нет в ${path.join(home, "sessions")}`);
    return 1;
  }

  let selected;
  if (options.session !== null) {
    const needle = String(options.session);
    const exact = sessions.find((session) => session.id === needle);
    const matches = exact
      ? [exact]
      : sessions.filter((session) => session.id.startsWith(needle) || (!needle.startsWith("session-") && session.id.startsWith(`session-${needle}`)));
    if (matches.length === 0) {
      warn(`session-preset: сессия «${needle}» не найдена`);
      return 1;
    }
    if (matches.length > 1 && !exact) {
      warn(`session-preset: префикс «${needle}» неоднозначен: ${matches.map((item) => item.id).join(", ")}`);
      return 1;
    }
    selected = matches;
  } else if (options.cwd !== null) {
    const filter = path.resolve(options.cwd);
    if (!fs.existsSync(filter)) {
      warn(`session-preset: нет каталога --cwd: ${filter}`);
      return 2;
    }
    selected = sessions.filter((session) => {
      try {
        return cwdMatches(readHeader(session.file)?.cwd, filter);
      } catch {
        return false;
      }
    });
    if (selected.length === 0) {
      warn(`session-preset: нет сессий с cwd в ${filter}`);
      return 1;
    }
  } else {
    selected = sessions;
  }

  const describe = options.verbose || options.json;
  const locks = lockStates(selected.map((session) => path.join(session.dir, "session.lock")), python);
  const plans = selected.map((session, index) => planSessionChange(session, { ...options, home, python, lock: locks[index] }));
  const skippedByFrom = [];
  const plansFiltered = [];
  for (const plan of plans) {
    if (options.from !== null && plan.from !== options.from) {
      skippedByFrom.push(plan);
      continue;
    }
    plansFiltered.push(plan);
  }
  const usable = plansFiltered.filter((plan) => plan.problems.length === 0);
  const broken = plansFiltered.filter((plan) => plan.problems.length > 0);

  if (options.json) {
    say(
      JSON.stringify(
        {
          dryRun: options.dryRun,
          to: options.to,
          from: options.from,
          sessions: plansFiltered.map((plan) => ({
            id: plan.session.id,
            cwd: plan.info.cwd,
            from: plan.from,
            to: plan.to,
            seq: plan.seq,
            role: plan.role,
            board: plan.board,
            openTurn: plan.openTurn,
            backup: plan.backup,
            problems: plan.problems,
          })),
        },
        null,
        2,
      ),
    );
    if (broken.length > 0) return 1;
    if (!options.dryRun && usable.length > 0 && !options.yes) {
      warn("session-preset: для записи нужен --yes (вывод в --json)");
      return 2;
    }
    if (options.dryRun) return usable.length > 0 ? 0 : 1;
  } else {
    say(`план: сессий под фильтром ${plansFiltered.length}, к изменению ${usable.length}, пресет «${options.from ?? "*"}» → «${options.to}»${options.dryRun ? " (dry-run)" : ""}`);
    for (const plan of plansFiltered) {
      const parts = [`  ${sessionLabel(plan.session.id)}`, `cwd: ${plan.info.cwd || "?"}`, `пресет: ${plan.from || "?"} → ${plan.to}`, `seq: ${plan.seq}`];
      if (plan.problems.length > 0) {
        // Отказ — в stderr: stdout остаётся планом, по которому можно действовать.
        warn(`${parts.join("  ")}  ОТКАЗ: ${plan.problems.join("; ")}`);
        continue;
      }
      parts.push("ок");
      if (describe) parts.push(`роль: ${plan.role ?? "нет"}`, `доска: ${plan.board ?? "не найдена"}`, `бэкап: ${plan.backup ?? "нет"}`);
      say(parts.join("  "));
    }
    for (const plan of skippedByFrom) say(`  ${sessionLabel(plan.session.id)}  пропущена: пресет ${plan.from || "?"} ≠ ${options.from}`);
  }
  if (broken.length > 0) {
    warn(`session-preset: отказ — ${broken.length} сессий не проходят проверки, ничего не изменено`);
    return 1;
  }
  if (usable.length === 0) {
    warn("session-preset: нечего менять — под фильтр не попала ни одна сессия");
    return 1;
  }

  if (!options.dryRun && !options.yes) {
    if (!stdin.isTTY) {
      warn("session-preset: нужен терминал для подтверждения; в скриптах добавьте --yes");
      return 2;
    }
    out.write(`Изменить пресет у ${usable.length} сессий? [y/N] `);
    const answer = await readLine(stdin);
    if (!/^y(es)?$/i.test(answer.trim())) {
      warn("session-preset: отменено");
      return 1;
    }
  }

  if (options.dryRun) {
    say("dry-run: файлы не изменены");
    return 0;
  }

  let failed = 0;
  for (const plan of usable) {
    const result = applyPlan(plan, { ...options, home });
    if (!result.ok) {
      warn(`session-preset: ${sessionLabel(plan.session.id)}: ${result.error}`);
      failed += 1;
      continue;
    }
    const roleText = options.noRoles ? "карта ролей не тронута (--no-roles)" : result.role?.file ? `карта ролей: ${result.role.file}` : "доска не найдена — карта ролей не тронута";
    say(`  ${sessionLabel(plan.session.id)}: ${plan.from || "?"} → ${plan.to}, seq ${plan.seq}; ${roleText}`);
    if (result.backup) say(`  бэкап: ${result.backup}`);
    if (options.verbose) say(`  досыпано байт: ${result.appended}; кадр события: ${JSON.stringify(result.event)}`);
  }
  if (failed > 0) {
    warn(`session-preset: ${failed} сессий не изменены`);
    return 1;
  }
  say("перезапустите DSH, чтобы изменения вступили в силу");
  return 0;
}

/** Ограничить параллельность: сотни файлов сразу — это очередь I/O и задержки. */
async function mapLimit(items, limit, worker) {
  const result = new Array(items.length);
  let next = 0;
  const runners = Array.from({ length: Math.min(limit, items.length) }, async () => {
    for (;;) {
      const index = next;
      next += 1;
      if (index >= items.length) return;
      result[index] = await worker(items[index], index);
    }
  });
  await Promise.all(runners);
  return result;
}

/** Прочитать одну строку из stdin (для подтверждения). */
function readLine(stream) {
  return new Promise((resolve) => {
    let buffer = "";
    const onData = (chunk) => {
      buffer += String(chunk);
      if (buffer.includes("\n")) {
        stream.off?.("data", onData);
        resolve(buffer.split("\n")[0]);
      }
    };
    stream.on?.("data", onData);
    stream.on?.("end", () => resolve(buffer));
  });
}

/** Запущен ли файл напрямую (а не импортирован тестом). */
function isEntryPoint() {
  const entry = process.argv[1];
  if (!entry) return false;
  try {
    return pathToFileURL(fs.realpathSync(entry)).href === pathToFileURL(fs.realpathSync(fileURLToPath(import.meta.url))).href;
  } catch {
    return false;
  }
}

if (isEntryPoint()) {
  // `bin/session-preset.sh --list | head` не должен падать стеком: читатель
  // закрыл пайп — это не ошибка инструмента.
  const ignorePipe = (error) => {
    if (error?.code !== "EPIPE") throw error;
  };
  process.stdout.on("error", ignorePipe);
  process.stderr.on("error", ignorePipe);
  process.exitCode = await main(process.argv.slice(2));
}
