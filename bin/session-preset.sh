#!/usr/bin/env bash
#
# Смена пресета существующей сессии — офлайн (спека 006, задача #064).
#
# Пресет уже стартовавшей сессии DSH менять не даёт (реестр бросает
# `agent-preset/locked`), поэтому инструмент работает при ЗАКРЫТОМ DSH: досыпает
# в `session.v4.jsonl.zstd` один zstd-кадр с событием `agent-preset/selected` и
# обновляет карту ролей доски. После этого нужен перезапуск DSH.
#
# Логика — в `bin/session-preset.mjs` (Node, `node:zlib` zstd); эта оболочка
# только находит Node (`bin/node.sh`) и передаёт аргументы.
#
#   bin/session-preset.sh --list
#   bin/session-preset.sh --session <id|prefix> --to worker
#   bin/session-preset.sh --cwd DIR --from standard --to worker --dry-run
#   bin/session-preset.sh --all --from standard --to worker --yes
#   bin/session-preset.sh --restore ~/.dsh/session-backups/<id>.<ts>.zst
#
# Коды возврата: 0 — ок; 1 — отказ/ошибка; 2 — ошибка аргументов.
#
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$DIR/node.sh" "$DIR/session-preset.mjs" "$@"
