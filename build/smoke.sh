#!/usr/bin/env bash
# Дымовой тест загрузки: запускаем QEMU и проверяем, что в serial-логе есть
# ключевые строки ядра. Это заготовка T6 (полноценный smoke-тест в CI);
# здесь она нужна, чтобы `make qemu` и `make smoke` проверяли одно и то же.
#
# Каждый прогон идёт в собственный каталог build/run-<время>-<pid>-<rand>/
# (см. build/run-qemu.sh): параллельный `make smoke` не может подмешать свои
# строки в наш лог. При провале печатаются последние 20 строк serial и путь к
# полным логам.
#
# Тест не падает из-за таймаута: ядро после сводки намеренно уходит в hlt,
# поэтому завершение QEMU по таймауту — ожидаемое поведение.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)

# Свой каталог прогона на каждый запуск (в том числе параллельный).
RUN_DIR=${RUN_DIR:-"$ROOT/build/run-$(date -u +%Y%m%d-%H%M%S)-$$-$RANDOM"}
case "$RUN_DIR" in
    /*) ;;
    *) RUN_DIR="$ROOT/$RUN_DIR" ;;
esac
mkdir -p "$RUN_DIR"
export RUN_DIR
export SERIAL_LOG=${SERIAL_LOG:-$RUN_DIR/serial.log}
export DEBUGCON_LOG=${DEBUGCON_LOG:-$RUN_DIR/debugcon.log}
export QEMU_LOG=${QEMU_LOG:-$RUN_DIR/qemu.log}

# Старые прогоны не копим вечно: оставляем 10 последних каталогов. Каталоги
# живых параллельных прогонов не трогаем (у них есть запись в build/runs).
prune_old_runs() {
    local keep=10 index=0 dir newest
    newest=$(ls -1dt "$ROOT"/build/run-* 2>/dev/null || true)
    [ -n "$newest" ] || return 0
    while IFS= read -r dir; do
        [ -n "$dir" ] || continue
        index=$((index + 1))
        [ "$index" -gt "$keep" ] || continue
        [ -d "$dir" ] || continue
        if [ -e "$ROOT/build/runs/$(basename "$dir").pid" ]; then
            continue # прогон ещё жив
        fi
        rm -rf "$dir"
    done <<<"$newest"
}

prune_old_runs

"$ROOT/build/run-qemu.sh"

fail=0

check() {
    local marker=$1
    if grep -qF -- "$marker" "$SERIAL_LOG"; then
        echo "smoke: OK   '$marker'"
    else
        echo "smoke: НЕТ  '$marker'" >&2
        fail=1
    fi
}

check "=== AIOS kernel ==="
check "boot: entry reached (_start)"
check "limine: Limine "
check "limine: base revision requested 3"
check "HHDM base: 0x"
check "RSDP: 0x"
check "CPUs: "
check "TSC: "
check "--- memory map ---"
check "boot: ok, halting"

# debugcon должен содержать тот же текст, что и serial (без CRLF от UART).
if grep -qF "=== AIOS kernel ===" "$DEBUGCON_LOG"; then
    echo "smoke: OK   'debugcon: === AIOS kernel ==='"
else
    echo "smoke: НЕТ  'debugcon: === AIOS kernel ==='" >&2
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    {
        echo "smoke: ПРОВАЛ — прогон $RUN_DIR"
        echo "smoke: последние 20 строк serial.log:"
        tail -n 20 "$SERIAL_LOG" 2>/dev/null || true
        echo "smoke: полные логи: $RUN_DIR (serial.log, debugcon.log, qemu.log)"
    } >&2
    exit 1
fi

echo "smoke: ПРОЙДЕН (логи: $RUN_DIR)"
