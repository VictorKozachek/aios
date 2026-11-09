#!/usr/bin/env bash
# Negative check for the QEMU smoke gate: a broken image must NOT boot.
#
# Takes the working image produced by `make image`, replaces /boot/kernel with
# an empty file (a stand-in for a kernel whose BootInfo is broken and which
# therefore never reaches the boot marker) and asserts that run.sh fails.
#
# This is the CI-side proof of the T6 acceptance criterion: "a deliberately
# broken BootInfo turns CI red". The manual variant — build the kernel with an
# empty memory map — is described in README.md.
#
# Usage:
#   tests/qemu-smoke/negative-test.sh [--image PATH] [--timeout SECONDS]
#                                     [--qemu BIN] [--serial PATH]
#                                     [--debugcon PATH]
#
# Exit status: 0 = broken image correctly rejected, 1 = smoke gate was fooled.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
RUN="$ROOT/tests/qemu-smoke/run.sh"

IMAGE=${IMAGE:-build/aios.img}
TIMEOUT=${QEMU_TIMEOUT:-60}
QEMU=${QEMU:-qemu-system-x86_64}
SERIAL_LOG=${SERIAL_LOG:-build/negative/serial.log}
DEBUGCON_LOG=${DEBUGCON_LOG:-build/negative/debugcon.log}

die() {
    echo "negative: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE=$2; shift 2 ;;
        --timeout) TIMEOUT=$2; shift 2 ;;
        --qemu) QEMU=$2; shift 2 ;;
        --serial) SERIAL_LOG=$2; shift 2 ;;
        --debugcon) DEBUGCON_LOG=$2; shift 2 ;;
        -h|--help)
        awk 'NR > 1 { if (/^#/) { sub(/^# ?/, ""); print; next } if (NF == 0) { print ""; next } exit }' "$0"
            exit 0
            ;;
        *) die "unknown argument: $1" ;;
    esac
done

[ -f "$IMAGE" ] || die "no image at '$IMAGE' — build it first (make image)"
[ -n "$QEMU" ] || die "QEMU must not be empty"
command -v mcopy >/dev/null 2>&1 ||
    die "mtools not found ('mcopy') — install mtools"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/aios-smoke-negative.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

BROKEN_IMAGE="$WORK/broken.img"
cp "$IMAGE" "$BROKEN_IMAGE"

# An empty kernel cannot follow the Limine protocol, so QEMU never prints the
# boot marker. This is the cheapest faithful "broken BootInfo" stand-in.
: > "$WORK/empty-kernel"
mcopy -o -i "$BROKEN_IMAGE" "$WORK/empty-kernel" ::/boot/kernel ||
    die "failed to replace /boot/kernel in the image copy"

echo "negative: running smoke test against a deliberately broken image"
if "$RUN" \
    --qemu "$QEMU" \
    --image "$BROKEN_IMAGE" \
    --timeout "$TIMEOUT" \
    --serial "$SERIAL_LOG" \
    --debugcon "$DEBUGCON_LOG"; then
    echo "negative: FAIL — the smoke gate accepted a broken image" >&2
    exit 1
fi

echo "negative: OK — broken image rejected, logs in $(dirname "$SERIAL_LOG")"
