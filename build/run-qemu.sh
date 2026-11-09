#!/usr/bin/env bash
# Запуск AIOS в QEMU с UEFI-прошивкой OVMF.
#
# UEFI → Limine → ядро AIOS. Вывод ядра идёт в три канала, из них два
# сохраняются в файлы:
#   build/serial.log    — COM1 (я тут же ловлю вывод OVMF и Limine)
#   build/debugcon.log  — порт 0xE9 (только вывод ядра)
# Framebuffer виден при QEMU_DISPLAY=cocoa (или gtk/vnc), по умолчанию
# окно выключено — для CI и проверки по логам.
#
# Параметры (окружение):
#   QEMU             — бинарь QEMU                  (qemu-system-x86_64)
#   IMAGE            — образ ESP                    (build/aios.img)
#   OVMF_CODE        — файл прошивки (readonly pflash, обязателен)
#   OVMF_VARS_SRC    — исходный vars-файл OVMF      (опционально)
#   OVMF_VARS        — рабочая копия vars           (build/ovmf-vars.fd)
#   QEMU_TIMEOUT     — максимум секунд на загрузку (120)
#   QEMU_DONE_MARKER — строка «ядро дошло до конца» (boot: ok)
#   QEMU_DISPLAY     — none | cocoa | gtk | vnc=... (none)
#   QEMU_MEMORY      — память гостя в МиБ           (512)
#   SERIAL_LOG, DEBUGCON_LOG — куда писать логи

set -euo pipefail

QEMU=${QEMU:-qemu-system-x86_64}
IMAGE=${IMAGE:-build/aios.img}
OVMF_CODE=${OVMF_CODE:-}
OVMF_VARS_SRC=${OVMF_VARS_SRC:-}
OVMF_VARS=${OVMF_VARS:-build/ovmf-vars.fd}
QEMU_TIMEOUT=${QEMU_TIMEOUT:-120}
QEMU_DONE_MARKER=${QEMU_DONE_MARKER:-boot: ok, halting}
QEMU_DISPLAY=${QEMU_DISPLAY:-none}
QEMU_MEMORY=${QEMU_MEMORY:-512}
SERIAL_LOG=${SERIAL_LOG:-build/serial.log}
DEBUGCON_LOG=${DEBUGCON_LOG:-build/debugcon.log}

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

mkdir -p "$(dirname "$SERIAL_LOG")" "$(dirname "$DEBUGCON_LOG")"
: >"$SERIAL_LOG"
: >"$DEBUGCON_LOG"

qemu_args=(
    -M q35
    -m "$QEMU_MEMORY"
    # Код прошивки монтируем только на чтение...
    -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
    # ...а vars-файл — копией, чтобы каждый запуск начинался с чистого NVRAM.
    -drive "file=$IMAGE,format=raw,if=ide"
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

echo "qemu: старт (образ $IMAGE, лог $SERIAL_LOG, лимит ${QEMU_TIMEOUT}s)"

"$QEMU" "${qemu_args[@]}" &
qemu_pid=$!

finish() {
    kill "$qemu_pid" 2>/dev/null || true
    wait "$qemu_pid" 2>/dev/null || true
}

trap finish EXIT INT TERM

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
else
    echo "qemu: маркер '$QEMU_DONE_MARKER' не найден за ${elapsed}s" >&2
fi

echo
echo "=== serial.log (последние строки) ==="
tail -n 60 "$SERIAL_LOG" || true
echo "=== debugcon.log: $(wc -l <"$DEBUGCON_LOG" | tr -d ' ') строк ==="
