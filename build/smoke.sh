#!/usr/bin/env bash
# Дымовой тест загрузки: запускаем QEMU и проверяем, что в serial-логе есть
# ключевые строки ядра. Это заготовка T6 (полноценный smoke-тест в CI);
# здесь она нужна, чтобы `make qemu` и `make smoke` проверяли одно и то же.
#
# Тест не падает из-за таймаута: ядро после сводки намеренно уходит в hlt,
# поэтому завершение QEMU по таймауту — ожидаемое поведение.

set -euo pipefail

SERIAL_LOG=${SERIAL_LOG:-build/serial.log}
DEBUGCON_LOG=${DEBUGCON_LOG:-build/debugcon.log}
export SERIAL_LOG DEBUGCON_LOG

"$(dirname "$0")/run-qemu.sh"

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
    echo "smoke: ПРОВАЛ — смотри $SERIAL_LOG" >&2
    exit 1
fi

echo "smoke: ПРОЙДЕН"
