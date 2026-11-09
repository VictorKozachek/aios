#!/usr/bin/env bash
# AIOS QEMU boot smoke test.
#
# Boots a FAT-ESP image through OVMF + Limine in QEMU and fails unless the
# expected boot marker appears in one of the kernel's diagnostic channels
# (COM1 serial or debugcon/0xE9) within the timeout. QEMU is started with
# `-device isa-debug-exit`, so a kernel that supports it can end the VM with a
# deterministic status; see --expect-exit-code below.
#
# The image must be built first (make image — task #001). This script never
# builds anything itself: it must honestly fail when the image is missing
# instead of "passing on emptiness".
#
# Usage:
#   tests/qemu-smoke/run.sh [options]
#
# Options:
#   --image PATH            ESP image (default: build/aios.img)
#   --marker TEXT           boot marker to find (default: "boot: ok")
#   --timeout SECONDS       how long to wait for the marker (default: 120)
#   --serial PATH           COM1 log (default: build/serial.log)
#   --debugcon PATH         debugcon log (default: build/debugcon.log)
#   --qemu BIN              QEMU binary (default: $QEMU, then qemu-system-x86_64)
#   --memory MIB            guest RAM in MiB (default: 512)
#   --ovmf-code PATH        OVMF code firmware (auto-detected when omitted)
#   --ovmf-vars PATH        OVMF vars template (auto-detected when omitted)
#   --expect-exit-code N    require QEMU to terminate with this status after
#                           the marker. isa-debug-exit encodes the value the
#                           guest writes to port 0x501 as (value << 1) | 1,
#                           e.g. value 0x10 becomes exit status 33.
#   -h, --help              show this help
#
# Exit status: 0 = marker found (and exit code matched, when requested),
#              1 = smoke failure, 2 = usage error.

set -euo pipefail

IMAGE=${IMAGE:-build/aios.img}
MARKER=${MARKER:-boot: ok}
TIMEOUT=${QEMU_TIMEOUT:-120}
QEMU=${QEMU:-qemu-system-x86_64}
MEMORY=${QEMU_MEMORY:-512}
SERIAL_LOG=${SERIAL_LOG:-build/serial.log}
DEBUGCON_LOG=${DEBUGCON_LOG:-build/debugcon.log}
OVMF_CODE=${OVMF_CODE:-}
OVMF_VARS_SRC=${OVMF_VARS_SRC:-}
EXPECT_EXIT_CODE=""

usage() {
    awk 'NR > 1 { if (/^#/) { sub(/^# ?/, ""); print; next } if (NF == 0) { print ""; next } exit }' "$0"
}

die() {
    echo "smoke: $*" >&2
    exit 1
}

usage_error() {
    echo "smoke: $*" >&2
    usage >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE=$2; shift 2 ;;
        --marker) MARKER=$2; shift 2 ;;
        --timeout) TIMEOUT=$2; shift 2 ;;
        --serial) SERIAL_LOG=$2; shift 2 ;;
        --debugcon) DEBUGCON_LOG=$2; shift 2 ;;
        --qemu) QEMU=$2; shift 2 ;;
        --memory) MEMORY=$2; shift 2 ;;
        --ovmf-code) OVMF_CODE=$2; shift 2 ;;
        --ovmf-vars) OVMF_VARS_SRC=$2; shift 2 ;;
        --expect-exit-code) EXPECT_EXIT_CODE=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage_error "unknown argument: $1" ;;
    esac
done

case "$TIMEOUT" in
    ''|*[!0-9]*) usage_error "--timeout must be a non-negative integer" ;;
esac
case "$MEMORY" in
    ''|*[!0-9]*) usage_error "--memory must be a positive integer" ;;
esac
[ -n "$MARKER" ] || usage_error "--marker must not be empty"
if [ -n "$EXPECT_EXIT_CODE" ]; then
    case "$EXPECT_EXIT_CODE" in
        *[!0-9]*) usage_error "--expect-exit-code must be a non-negative integer" ;;
    esac
fi

[ -f "$IMAGE" ] || die "no image at '$IMAGE' — build it first (make image)"
command -v "$QEMU" >/dev/null 2>&1 ||
    die "QEMU not found: '$QEMU' — install qemu-system-x86"

# Firmware: prefer an explicit path, then the usual macOS/Linux locations.
if [ -z "$OVMF_CODE" ]; then
    if command -v brew >/dev/null 2>&1; then
        brew_qemu=$(brew --prefix qemu 2>/dev/null || true)
        if [ -n "$brew_qemu" ] && [ -f "$brew_qemu/share/qemu/edk2-x86_64-code.fd" ]; then
            OVMF_CODE=$brew_qemu/share/qemu/edk2-x86_64-code.fd
        fi
    fi
fi
if [ -z "$OVMF_CODE" ]; then
    for candidate in \
        /opt/homebrew/share/qemu/edk2-x86_64-code.fd \
        /usr/local/share/qemu/edk2-x86_64-code.fd \
        /usr/share/qemu/edk2-x86_64-code.fd \
        /usr/share/OVMF/OVMF_CODE.fd \
        /usr/share/OVMF/OVMF_CODE_4M.fd \
        /usr/share/edk2/x64/OVMF_CODE.fd
    do
        if [ -f "$candidate" ]; then
            OVMF_CODE=$candidate
            break
        fi
    done
fi
[ -n "$OVMF_CODE" ] || die "OVMF firmware not found — set --ovmf-code or OVMF_CODE"
[ -f "$OVMF_CODE" ] || die "OVMF firmware not found at '$OVMF_CODE'"

if [ -z "$OVMF_VARS_SRC" ]; then
    if command -v brew >/dev/null 2>&1; then
        brew_qemu=$(brew --prefix qemu 2>/dev/null || true)
        if [ -n "$brew_qemu" ] && [ -f "$brew_qemu/share/qemu/edk2-i386-vars.fd" ]; then
            OVMF_VARS_SRC=$brew_qemu/share/qemu/edk2-i386-vars.fd
        fi
    fi
fi
if [ -z "$OVMF_VARS_SRC" ]; then
    for candidate in \
        /opt/homebrew/share/qemu/edk2-i386-vars.fd \
        /usr/local/share/qemu/edk2-i386-vars.fd \
        /usr/share/qemu/edk2-i386-vars.fd \
        /usr/share/OVMF/OVMF_VARS.fd \
        /usr/share/OVMF/OVMF_VARS_4M.fd \
        /usr/share/edk2/x64/OVMF_VARS.fd
    do
        if [ -f "$candidate" ]; then
            OVMF_VARS_SRC=$candidate
            break
        fi
    done
fi

serial_dir=$(dirname "$SERIAL_LOG")
debugcon_dir=$(dirname "$DEBUGCON_LOG")
mkdir -p "$serial_dir" "$debugcon_dir"
: > "$SERIAL_LOG"
: > "$DEBUGCON_LOG"

# OVMF needs a writable NVRAM image; start every run from a fresh copy.
VARS_COPY=""
if [ -n "$OVMF_VARS_SRC" ] && [ -f "$OVMF_VARS_SRC" ]; then
    VARS_COPY="$serial_dir/ovmf-vars.fd"
    if [ "$OVMF_VARS_SRC" != "$VARS_COPY" ]; then
        cp -f "$OVMF_VARS_SRC" "$VARS_COPY"
    fi
fi

qemu_stderr="$serial_dir/qemu.stderr.log"
: > "$qemu_stderr"

qemu_args=(
    -M q35
    -m "$MEMORY"
    -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
    # snapshot=on: the smoke test must never mutate the image, and it must be
    # able to run while another QEMU holds the same file open (readonly=on is
    # not accepted by the IDE drive model).
    -drive "if=ide,format=raw,snapshot=on,file=$IMAGE"
    -device isa-debug-exit
    -serial "file:$SERIAL_LOG"
    -debugcon "file:$DEBUGCON_LOG"
    -global isa-debugcon.iobase=0xE9
    -display none
    -monitor none
    -no-reboot
)
if [ -n "$VARS_COPY" ]; then
    qemu_args+=(-drive "if=pflash,format=raw,file=$VARS_COPY")
fi

qemu_pid=""
cleanup() {
    if [ -n "$qemu_pid" ] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    qemu_pid=""
}
trap cleanup EXIT INT TERM

marker_found() {
    grep -qF -- "$MARKER" "$SERIAL_LOG" 2>/dev/null ||
        grep -qF -- "$MARKER" "$DEBUGCON_LOG" 2>/dev/null
}

echo "smoke: image=$IMAGE timeout=${TIMEOUT}s qemu=$QEMU"
echo "smoke: marker='$MARKER' (serial + debugcon)"

"$QEMU" "${qemu_args[@]}" >/dev/null 2>"$qemu_stderr" &
qemu_pid=$!

found=0
qemu_exited=0
qemu_status=0
elapsed=0
while [ "$elapsed" -lt "$TIMEOUT" ]; do
    if ! kill -0 "$qemu_pid" 2>/dev/null; then
        qemu_exited=1
        if wait "$qemu_pid"; then
            qemu_status=0
        else
            qemu_status=$?
        fi
        qemu_pid=""
        break
    fi
    # Without an expected exit code the marker alone decides; with one we keep
    # waiting for QEMU to terminate so the status can be checked too.
    if [ -z "$EXPECT_EXIT_CODE" ] && marker_found; then
        found=1
        break
    fi
    sleep 1
    elapsed=$((elapsed + 1))
done
if [ "$found" -eq 0 ] && marker_found; then
    found=1
fi

dump_logs() {
    echo "smoke: --- $SERIAL_LOG (tail) ---" >&2
    tail -n 40 "$SERIAL_LOG" >&2 2>/dev/null || true
    echo "smoke: --- $DEBUGCON_LOG (tail) ---" >&2
    tail -n 40 "$DEBUGCON_LOG" >&2 2>/dev/null || true
    if [ -s "$qemu_stderr" ]; then
        echo "smoke: --- QEMU stderr (tail) ---" >&2
        tail -n 20 "$qemu_stderr" >&2 2>/dev/null || true
    fi
}

if [ -n "$EXPECT_EXIT_CODE" ]; then
    if [ "$found" -eq 1 ] && [ "$qemu_exited" -eq 1 ] && [ "$qemu_status" -eq "$EXPECT_EXIT_CODE" ]; then
        echo "smoke: OK — marker '$MARKER' found, QEMU exit status $qemu_status"
        exit 0
    fi
    echo "smoke: FAIL — expected marker and exit status $EXPECT_EXIT_CODE" \
        "(marker_found=$found, qemu_exited=$qemu_exited, qemu_status=$qemu_status)" >&2
    cleanup
    dump_logs
    exit 1
fi

if [ "$found" -eq 1 ]; then
    echo "smoke: OK — marker '$MARKER' found after ${elapsed}s"
    exit 0
fi

if [ "$qemu_exited" -eq 1 ]; then
    echo "smoke: FAIL — QEMU exited (status $qemu_status) before marker '$MARKER'" >&2
else
    echo "smoke: FAIL — marker '$MARKER' not found within ${TIMEOUT}s" >&2
fi
cleanup
dump_logs
exit 1
