#!/usr/bin/env bash
# Запуск AIOS в QEMU с UEFI-прошивкой OVMF.
#
# UEFI → Limine → ядро AIOS. Вывод ядра идёт в три канала, из них два
# сохраняются в файлы. Каждый прогон получает СВОЙ каталог:
#
#   build/run-<UTC-время>-<pid>-<rand>/
#       serial.log    — COM1 (вывод OVMF, Limine и ядра)
#       debugcon.log  — порт 0xE9 (только ядро)
#       qemu.log      — stderr самого QEMU
#       ovmf-vars.fd  — своя копия NVRAM (чистая на каждый прогон)
#
# Так два параллельных прогона (разные агенты, `make smoke` в двух окнах) не
# смешивают логи и не мешают друг другу: образ монтируется с `snapshot=on`
# (без блокировки файла), vars-файл у каждого свой. Указатель на последний
# прогон — `build/last-run`.
#
# Защита от «зависшего QEMU»: в `build/runs/<run>.pid` лежат pid владельца
# (этого скрипта) и pid QEMU; это и есть источник правды для уборки. Перед
# стартом скрипт снимает только те процессы, чей владелец уже мёртв, — живые
# параллельные прогоны не трогаются. `build/qemu.pid` — указатель на QEMU
# текущего прогона (pid + каталог) для людей и внешних инструментов; под
# параллельными прогонами он неоднозначен, поэтому уборка им не пользуется.
# При любом выходе (включая таймаут и SIGINT/SIGTERM) QEMU гасится по trap.
#
# Параметры (окружение):
#   QEMU             — бинарь QEMU                  (qemu-system-x86_64)
#   IMAGE            — образ ESP                    (build/aios.img)
#   RUN_DIR          — каталог прогона              (build/run-... уникальный)
#   OVMF_CODE        — файл прошивки (readonly pflash, обязателен)
#   OVMF_VARS_SRC    — исходный vars-файл OVMF      (опционально)
#   OVMF_VARS        — рабочая копия vars           ($RUN_DIR/ovmf-vars.fd)
#   QEMU_TIMEOUT     — максимум секунд на загрузку  (120)
#   QEMU_DONE_MARKER — строка «ядро дошло до конца» (boot: ok, halting)
#   QEMU_DISPLAY     — none | cocoa | gtk | vnc=... (none)
#   QEMU_MEMORY      — память гостя в МиБ           (512)
#   SERIAL_LOG, DEBUGCON_LOG, QEMU_LOG — переопределить пути логов
#
# Скрипт-запускалка не падает при отсутствии маркера: он печатает последние
# строки логов и путь к полным логам, а решение «это провал» принимает
# `make smoke`/`build/smoke.sh` (у него свои проверки).

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

QEMU=${QEMU:-qemu-system-x86_64}
IMAGE=${IMAGE:-build/aios.img}
OVMF_CODE=${OVMF_CODE:-}
OVMF_VARS_SRC=${OVMF_VARS_SRC:-}
QEMU_TIMEOUT=${QEMU_TIMEOUT:-120}
QEMU_DONE_MARKER=${QEMU_DONE_MARKER:-boot: ok, halting}
QEMU_DISPLAY=${QEMU_DISPLAY:-none}
QEMU_MEMORY=${QEMU_MEMORY:-512}

# Абсолютные пути: скрипт может запускаться из любого каталога.
case "$IMAGE" in
    /*) ;;
    *) IMAGE="$ROOT/$IMAGE" ;;
esac

# Уникальный каталог прогона. Параллельные прогоны не пересекаются.
RUN_DIR=${RUN_DIR:-"$ROOT/build/run-$(date -u +%Y%m%d-%H%M%S)-$$-$RANDOM"}
case "$RUN_DIR" in
    /*) ;;
    *) RUN_DIR="$ROOT/$RUN_DIR" ;;
esac
mkdir -p "$RUN_DIR"

SERIAL_LOG=${SERIAL_LOG:-$RUN_DIR/serial.log}
DEBUGCON_LOG=${DEBUGCON_LOG:-$RUN_DIR/debugcon.log}
QEMU_LOG=${QEMU_LOG:-$RUN_DIR/qemu.log}
OVMF_VARS=${OVMF_VARS:-$RUN_DIR/ovmf-vars.fd}

LEDGER_DIR="$ROOT/build/runs"
RUN_ID=$(basename "$RUN_DIR")
PID_ENTRY="$LEDGER_DIR/$RUN_ID.pid"
LAST_RUN_FILE="$ROOT/build/last-run"

die() {
    echo "qemu: $*" >&2
    exit 1
}

command -v "$QEMU" >/dev/null 2>&1 ||
    die "не найден QEMU '$QEMU'. Установите: brew install qemu"

[ -f "$IMAGE" ] || die "нет образа '$IMAGE'. Соберите: make image"

# OVMF обязателен: без UEFI-прошивки Limine (UEFI-бинарь) не загрузится.
[ -n "$OVMF_CODE" ] || die "не задан OVMF_CODE (файл edk2-x86_64-code.fd).
       Обычно это \$(brew --prefix qemu)/share/qemu/edk2-x86_64-code.fd"
[ -f "$OVMF_CODE" ] || die "файл прошивки не найден: '$OVMF_CODE'"

mkdir -p "$LEDGER_DIR"
: >"$SERIAL_LOG"
: >"$DEBUGCON_LOG"
: >"$QEMU_LOG"

# --- Уборка сирот прошлых прогонов -----------------------------------------
#
# Запись в ledger содержит pid скрипта-владельца и pid QEMU. Если владелец
# мёртв, а QEMU жив — это сирота от упавшего/убитого прогона: его надо снять,
# иначе он будет писать в свой (уже не читаемый) лог и жечь CPU. Записи живых
# параллельных прогонов не трогаем.
reap_orphans() {
    local entry owner qemu_pid
    for entry in "$LEDGER_DIR"/*.pid; do
        [ -e "$entry" ] || continue
        owner=$(sed -n 's/^owner=//p' "$entry" 2>/dev/null | head -n 1 || true)
        qemu_pid=$(sed -n 's/^qemu=//p' "$entry" 2>/dev/null | head -n 1 || true)
        if [ -z "$owner" ]; then
            rm -f "$entry"
            continue
        fi
        if kill -0 "$owner" 2>/dev/null; then
            continue # живой параллельный прогон
        fi
        if [ -n "$qemu_pid" ] && kill -0 "$qemu_pid" 2>/dev/null; then
            echo "qemu: снимаю осиротевший QEMU pid=$qemu_pid (прогон ${entry##*/})" >&2
            kill -TERM "$qemu_pid" 2>/dev/null || true
            for _ in 1 2 3 4 5 6 7 8 9 10; do
                kill -0 "$qemu_pid" 2>/dev/null || break
                sleep 0.1
            done
            kill -KILL "$qemu_pid" 2>/dev/null || true
        fi
        rm -f "$entry"
    done
}

write_pid_entry() {
    local qemu_pid=${1:-}
    printf 'owner=%s\nqemu=%s\nrun=%s\nstarted=%s\n' \
        "$$" "$qemu_pid" "$RUN_DIR" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$PID_ENTRY"
}

# --- Чистая уборка при любом выходе ---------------------------------------
qemu_pid=""
cleanup() {
    local status=$?
    trap - EXIT INT TERM
    if [ -n "$qemu_pid" ] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill -TERM "$qemu_pid" 2>/dev/null || true
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            kill -0 "$qemu_pid" 2>/dev/null || break
            sleep 0.1
        done
        kill -KILL "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    rm -f "$PID_ENTRY"
    # Снимаем build/qemu.pid только если он всё ещё указывает на наш QEMU:
    # у параллельного прогона там уже может быть его pid.
    if [ -n "$qemu_pid" ] && [ -f "$ROOT/build/qemu.pid" ] &&
        grep -q "^pid=$qemu_pid$" "$ROOT/build/qemu.pid" 2>/dev/null; then
        rm -f "$ROOT/build/qemu.pid"
    fi
    # Указатель для человека: где лежат последние логи. Сами логи остаются в
    # RUN_DIR (build/run-*/ игнорируется git) — они и есть артефакт прогона.
    printf '%s\n' "$RUN_DIR" >"$LAST_RUN_FILE"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

reap_orphans
write_pid_entry

# OVMF нужен записываемый NVRAM: своя копия на каждый прогон.
qemu_args=(
    -M q35
    -m "$QEMU_MEMORY"
    # Код прошивки монтируем только на чтение...
    -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
    # ...а образ — с snapshot=on: без блокировки файла и без записи в него,
    #   чтобы параллельный прогон мог открыть тот же образ.
    -drive "if=ide,format=raw,snapshot=on,file=$IMAGE"
    -serial "file:$SERIAL_LOG"
    -debugcon "file:$DEBUGCON_LOG"
    -global isa-debugcon.iobase=0xE9
    -display "$QEMU_DISPLAY"
    -monitor none
    -no-reboot
)

if [ -n "$OVMF_VARS_SRC" ] && [ -f "$OVMF_VARS_SRC" ]; then
    cp -f "$OVMF_VARS_SRC" "$OVMF_VARS"
    qemu_args+=(-drive "if=pflash,format=raw,file=$OVMF_VARS")
fi

echo "qemu: прогон $RUN_ID"
echo "qemu: образ $IMAGE, лимит ${QEMU_TIMEOUT}s, логи в $RUN_DIR"

"$QEMU" "${qemu_args[@]}" >/dev/null 2>"$QEMU_LOG" &
qemu_pid=$!
write_pid_entry "$qemu_pid"
# Указатель «где сейчас QEMU»: только для человека/инструментов, не для уборки.
printf 'pid=%s\nrun=%s\n' "$qemu_pid" "$RUN_DIR" >"$ROOT/build/qemu.pid"

elapsed=0
found=0
while [ "$elapsed" -lt "$QEMU_TIMEOUT" ]; do
    if ! kill -0 "$qemu_pid" 2>/dev/null; then
        break
    fi
    if grep -qF -- "$QEMU_DONE_MARKER" "$SERIAL_LOG" 2>/dev/null; then
        found=1
        break
    fi
    sleep 1
    elapsed=$((elapsed + 1))
done

if [ "$found" -eq 1 ]; then
    echo "qemu: ядро дошло до маркера '$QEMU_DONE_MARKER' за ${elapsed}s"
    echo "qemu: логи: $RUN_DIR"
    exit 0
fi

# Таймаут или ранний выход QEMU: печатаем хвост serial и путь к полным логам,
# чтобы разбор не начинался с «а где лог?».
{
    echo "qemu: маркер '$QEMU_DONE_MARKER' не найден за ${elapsed}s"
    echo "qemu: последние 20 строк serial.log ($SERIAL_LOG):"
    tail -n 20 "$SERIAL_LOG" 2>/dev/null || true
    if [ -s "$QEMU_LOG" ]; then
        echo "qemu: последние 5 строк qemu.log ($QEMU_LOG):"
        tail -n 5 "$QEMU_LOG" 2>/dev/null || true
    fi
    echo "qemu: полные логи: $RUN_DIR (serial.log, debugcon.log, qemu.log)"
} >&2

exit 0
