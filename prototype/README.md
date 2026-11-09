# AIOS prototype (PR-015)

Прототип «схемы» до переезда в ядро: модель, консоль, воркспейсы, лог сессий.
Полное описание и место прототипа относительно ядра — `docs/PROTOTYPE.md`
(§3 модель, §5 воркспейсы, §6 консоль). Память/RAG — карточка 016.

Это **отдельный Cargo-workspace** (`prototype/`), не входящий в корневой: корень
собирается под `x86_64-unknown-none` (`no_std`), а прототип — обычные `std`-крейты.

## Запуск

Нужен ollama на `127.0.0.1:11434` и модель:

```bash
ollama serve &
ollama pull qwen2.5:0.5b

cd prototype
cargo run -p aios-console                 # воркспейс work
cargo run -p aios-console -- --workspace home
cargo run -p aios-console -- --model qwen2.5:0.5b
```

Внутри: обычный ввод уходит модели (ответ печатается потоком), `/help` — команды,
Ctrl-D — выход.

```
aios[work] > привет, кто ты?
Привет! Я искусственный интеллект…
[токены: prompt 68 · completion 25]
```

## Команды

| Команда | Что делает |
|---|---|
| `/ws [home\|work]` | показать воркспейсы или переключиться |
| `/status` | модель, воркспейс, токены, путь лога |
| `/new` | новая сессия (старый лог сохраняется) |
| `/help` | справка |
| `/quit` | выход (или Ctrl-D) |

## Каталоги и окружение

```
~/.aios/workspaces/
  home/  config.toml  files/  sessions/  memory/
  work/  config.toml  files/  sessions/  memory/
```

`config.toml` воркспейса: `model`, `temperature`, `max_context_tokens`,
`system_prompt`. Создаётся при первом запуске, правится руками.

| Переменная | Значение по умолчанию |
|---|---|
| `AIOS_HOME` | `$HOME/.aios` |
| `AIOS_OLLAMA_URL` | `http://127.0.0.1:11434` |
| `AIOS_MODEL` | `model` из `config.toml` воркспейса |

Лог сессии — `sessions/<YYYYMMDD-HHMMSS>.ndjson`: `session_start`, `user`,
`assistant` (с `prompt_tokens`/`completion_tokens` из ollama), `command`, `error`,
`session_end`. При перезапуске консоль открывает последний лог воркспейса и
продолжает диалог в нём же.

## Крейты

* `modeld` (`aios-modeld`) — trait `ModelBackend` (`info`, `generate`, `count_tokens`)
  и `OllamaBackend`: `POST /api/chat` со `stream: true`, NDJSON-поток,
  внятные ошибки «ollama не запущен» / «модель не скачана».
* `console` (`aios-console`) — REPL, воркспейсы, лог сессии, `/status`.

## Тесты

```bash
cd prototype
cargo test          # 41 тест: NDJSON-фикстура, воркспейсы, сессии, REPL-команды
cargo clippy --all-targets
```

Интеграционный тест запускает настоящий бинарник с подменённым `HOME` и
проверяет создание воркспейсов, переключение `/ws` и продолжение лога после
перезапуска — без сети: `AIOS_OLLAMA_URL` указывает на закрытый порт.
