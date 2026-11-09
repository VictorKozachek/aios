#!/usr/bin/env bash
# M0 acceptance in QEMU: boot the image and verify the log contract.
#
# This script does not touch the kernel or the build system: it takes the ESP
# image produced by `make image`, boots it under OVMF, captures serial and
# debugcon logs, saves a framebuffer screenshot and checks the markers that
# docs/PLAN-M0.md and tests/acceptance/M0.md require.
#
# Usage:
#   tests/acceptance/qemu-accept.sh                 # build image (make image), boot, check
#   tests/acceptance/qemu-accept.sh --no-build      # use the existing build/aios.img
#   tests/acceptance/qemu-accept.sh --image IMG     # boot a specific image
#   tests/acceptance/qemu-accept.sh --negative      # check the CHECKER: no kernel -> must fail
#   tests/acceptance/qemu-accept.sh --timeout 60    # boot timeout, seconds (default 30)
#
# Exit codes: 0 — all required checks passed (or the negative check failed to
# boot, as expected); 1 — a required check failed; 2 — environment/usage error.
#
# Requirements: qemu-system-x86_64, OVMF (edk2-x86_64-code.fd); for the build
# step also make + the tools that `make image` needs (mtools, Limine).
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE="build/aios.img"
OUTDIR="build/acceptance"
TIMEOUT=30
MEMORY=${QEMU_MEMORY:-512}
BUILD=1
NEGATIVE=0
BOOT_LIMIT_MS=${ACCEPT_BOOT_MS_LIMIT:-5000}
QEMU=${QEMU:-qemu-system-x86_64}
OVMF_CODE=${OVMF_CODE:-}
OVMF_VARS_SRC=${OVMF_VARS_SRC:-}

usage() {
    awk 'NR == 1 { next }
         /^set -u/ { exit }
         { sub(/^# ?/, ""); print }' "$0"
}

die() {
    echo "qemu-accept: $*" >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="${2:?--image needs a value}"; shift 2 ;;
        --image=*) IMAGE="${1#*=}"; shift ;;
        --outdir) OUTDIR="${2:?--outdir needs a value}"; shift 2 ;;
        --outdir=*) OUTDIR="${1#*=}"; shift ;;
        --timeout) TIMEOUT="${2:?--timeout needs a value}"; shift 2 ;;
        --timeout=*) TIMEOUT="${1#*=}"; shift ;;
        --no-build) BUILD=0; shift ;;
        --negative) NEGATIVE=1; BUILD=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

cd "$ROOT" || die "cannot cd to $ROOT"
mkdir -p "$OUTDIR" || die "cannot create $OUTDIR"
SERIAL_LOG="$OUTDIR/serial.log"
DEBUGCON_LOG="$OUTDIR/debugcon.log"
MON_SOCK="$OUTDIR/monitor.sock"
FB_PPM="$OUTDIR/framebuffer.ppm"
QEMU_PID=""

cleanup() {
    if [ -n "$QEMU_PID" ] && kill -0 "$QEMU_PID" 2>/dev/null; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    rm -f "$MON_SOCK"
}
trap cleanup EXIT INT TERM

now_ms() {
    perl -MTime::HiRes=time -e 'printf "%.0f\n", time * 1000' 2>/dev/null ||
        { echo $(( $(date +%s) * 1000 )); }
}

command -v "$QEMU" >/dev/null 2>&1 ||
    die "QEMU '$QEMU' not found (brew install qemu)"

if [ -z "$OVMF_CODE" ]; then
    qemu_prefix="$(brew --prefix qemu 2>/dev/null || true)"
    for candidate in \
        "$qemu_prefix/share/qemu/edk2-x86_64-code.fd" \
        /opt/homebrew/share/qemu/edk2-x86_64-code.fd \
        /usr/local/share/qemu/edk2-x86_64-code.fd \
        /usr/share/OVMF/OVMF_CODE.fd \
        /usr/share/OVMF/OVMF_CODE_4M.fd \
        /usr/share/edk2/x64/OVMF_CODE.fd; do
        if [ -n "$candidate" ] && [ -f "$candidate" ]; then
            OVMF_CODE="$candidate"
            break
        fi
    done
fi
[ -n "$OVMF_CODE" ] && [ -f "$OVMF_CODE" ] ||
    die "OVMF_CODE not found; set OVMF_CODE=/path/to/edk2-x86_64-code.fd"

if [ "$BUILD" -eq 1 ]; then
    [ -f Makefile ] || die "no Makefile: cannot build the image (use --image/--no-build)"
    echo "qemu-accept: make image"
    make image || die "make image failed"
fi

[ -f "$IMAGE" ] || die "image not found: $IMAGE"

if [ -z "$OVMF_VARS_SRC" ]; then
    qemu_prefix="$(brew --prefix qemu 2>/dev/null || true)"
    for candidate in \
        "$qemu_prefix/share/qemu/edk2-i386-vars.fd" \
        /opt/homebrew/share/qemu/edk2-i386-vars.fd \
        /usr/local/share/qemu/edk2-i386-vars.fd; do
        if [ -n "$candidate" ] && [ -f "$candidate" ]; then
            OVMF_VARS_SRC="$candidate"
            break
        fi
    done
fi

: >"$SERIAL_LOG"
: >"$DEBUGCON_LOG"
rm -f "$MON_SOCK" "$FB_PPM"

qemu_args=(
    -M q35
    -m "$MEMORY"
    -no-reboot
    -display none
    -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
    -drive "file=$IMAGE,format=raw,if=ide"
    -serial "file:$SERIAL_LOG"
    -debugcon "file:$DEBUGCON_LOG"
    -global isa-debugcon.iobase=0xE9
    -monitor "unix:$MON_SOCK,server,nowait"
)
if [ -n "$OVMF_VARS_SRC" ] && [ -f "$OVMF_VARS_SRC" ]; then
    cp -f "$OVMF_VARS_SRC" "$OUTDIR/ovmf-vars.fd"
    qemu_args+=(-drive "if=pflash,format=raw,file=$OUTDIR/ovmf-vars.fd")
fi

DONE_MARKER="boot: ok, halting"
MEMMAP_MARKER="--- memory map ---"
START_MS=$(now_ms)
echo "qemu-accept: booting $IMAGE (limit ${TIMEOUT}s, memory ${MEMORY} MiB)"
"$QEMU" "${qemu_args[@]}" >/dev/null 2>&1 &
QEMU_PID=$!

booted=0
memmap_ms=""
deadline=$(( START_MS + TIMEOUT * 1000 ))
while :; do
    if [ -z "$memmap_ms" ] && grep -qF -- "$MEMMAP_MARKER" "$SERIAL_LOG" 2>/dev/null; then
        memmap_ms=$(( $(now_ms) - START_MS ))
    fi
    if grep -qF -- "$DONE_MARKER" "$SERIAL_LOG" 2>/dev/null; then
        booted=1
        break
    fi
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    [ "$(now_ms)" -ge "$deadline" ] && break
    sleep 0.05
done

screenshot="skipped"
if [ "$booted" -eq 1 ] && [ -S "$MON_SOCK" ]; then
    printf 'screendump %s\n' "$FB_PPM" | nc -U "$MON_SOCK" >/dev/null 2>&1 || true
    tries=0
    while [ ! -s "$FB_PPM" ] && [ "$tries" -lt 20 ]; do
        tries=$((tries + 1))
        sleep 0.1
    done
    if [ -s "$FB_PPM" ]; then
        command -v sips >/dev/null 2>&1 &&
            sips -s format png "$FB_PPM" --out "$OUTDIR/framebuffer.png" >/dev/null 2>&1 || true
        screenshot="$(perl -e '
            my $file = shift;
            open(my $fh, "<:raw", $file) or exit 2;
            local $/;
            my $data = <$fh>;
            $data =~ /^P6\s+(\d+)\s+(\d+)\s+\d+\s/ or exit 3;
            my $header_len = length($&);
            my $pixels = substr($data, $header_len);
            my $bright = 0;
            for (my $i = 0; $i + 2 < length($pixels); $i += 3) {
                $bright++ if ord(substr($pixels, $i, 1)) > 24
                          || ord(substr($pixels, $i + 1, 1)) > 24
                          || ord(substr($pixels, $i + 2, 1)) > 24;
            }
            print $bright;
        ' "$FB_PPM" 2>/dev/null || echo 0)"
    else
        screenshot="unavailable"
    fi
    printf 'quit\n' | nc -U "$MON_SOCK" >/dev/null 2>&1 || true
fi

kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""

if [ "$NEGATIVE" -eq 1 ]; then
    if [ "$booted" -eq 1 ]; then
        echo "negative: FAIL — an image without the kernel still reached '$DONE_MARKER'" >&2
        echo "negative: the checker cannot detect a boot regression" >&2
        exit 1
    fi
    echo "negative: OK — no kernel marker in ${TIMEOUT}s, the checker fails as required"
    echo "negative: serial log tail:"
    tail -n 3 "$SERIAL_LOG" 2>/dev/null | cut -c1-160 | sed 's/^/  /'
    exit 0
fi

ok=0
bad=0

report() {
    # report <ok|fail|warn> <name> <detail>
    case "$1" in
        ok) ok=$((ok + 1));   printf '[OK]   %s\n' "$2" ;;
        fail) bad=$((bad + 1)); printf '[FAIL] %s%s\n' "$2" "${3:+ — $3}" ;;
        warn) printf '[WARN] %s%s\n' "$2" "${3:+ — $3}" ;;
    esac
}

check_line() {
    # check_line <name> <exact substring>
    if grep -qF -- "$2" "$SERIAL_LOG" 2>/dev/null; then
        report ok "$1"
    else
        report fail "$1" "no '$2' in $SERIAL_LOG"
    fi
}

echo
echo "== checks (serial: $SERIAL_LOG, debugcon: $DEBUGCON_LOG) =="
if [ "$booted" -ne 1 ]; then
    report fail "boot completed" "'$DONE_MARKER' not found within ${TIMEOUT}s (QEMU exited or timeout)"
else
    report ok "boot completed ('$DONE_MARKER')"
fi

check_line "kernel banner" "=== AIOS kernel ==="
check_line "entry reached" "boot: entry reached (_start)"
check_line "Limine version" "limine: Limine "
check_line "Limine base revision 3" "limine: base revision requested 3"
check_line "memory map header" "--- memory map ---"
check_line "HHDM base" "HHDM base: 0x"
check_line "RSDP" "RSDP: 0x"
check_line "CPU count" "CPUs: "
check_line "TSC frequency" "TSC: "

if [ -n "$memmap_ms" ]; then
    if [ "$memmap_ms" -le "$BOOT_LIMIT_MS" ]; then
        report ok "boot to memory map ${memmap_ms} ms <= ${BOOT_LIMIT_MS} ms"
    else
        report fail "boot to memory map" "${memmap_ms} ms > ${BOOT_LIMIT_MS} ms"
    fi
else
    report fail "boot to memory map" "marker '$MEMMAP_MARKER' never appeared"
fi

memmap_lines=$(grep -oE '^entries: [0-9]+' "$SERIAL_LOG" 2>/dev/null | head -n 1 | grep -oE '[0-9]+$' || true)
if [ -z "$memmap_lines" ]; then
    memmap_lines=$(grep -cE '^[[:space:]]*\[[[:space:]]*[0-9]+\]' "$SERIAL_LOG" 2>/dev/null || true)
fi
if [ "${memmap_lines:-0}" -ge 3 ]; then
    report ok "memory map has ${memmap_lines} entries"
else
    report fail "memory map is filled" "only ${memmap_lines:-0} entries printed"
fi

cpus=$(grep -oE 'CPUs: [0-9]+' "$SERIAL_LOG" 2>/dev/null | head -n 1 | grep -oE '[0-9]+$' || true)
if [ -n "$cpus" ] && [ "$cpus" -ge 1 ]; then
    report ok "CPUs: $cpus"
else
    report fail "CPUs value" "no positive CPU count in serial log"
fi

# Contract markers of M0 memory work (T3/T4b, see tests/acceptance/M0.md).
check_line "1 GiB allocation" "mem: alloc 1 GiB ok"
check_line "1 GiB free" "mem: free ok"
if grep -qE '^mem: kernel used [0-9]+ KiB' "$SERIAL_LOG" 2>/dev/null; then
    report ok "kernel memory counter ($(grep -E '^mem: kernel used [0-9]+ KiB' "$SERIAL_LOG" | head -n 1))"
else
    report fail "kernel memory counter" "no 'mem: kernel used <N> KiB' in $SERIAL_LOG"
fi
check_line "W^X page fault" "w^x: execute from heap -> page fault"
if [ "$booted" -eq 1 ]; then
    if grep -qF -- "PANIC:" "$SERIAL_LOG" 2>/dev/null; then
        report fail "no panic after boot" "kernel panicked: $(grep -m1 -F 'PANIC:' "$SERIAL_LOG")"
    else
        report ok "no panic after boot"
    fi
fi

if grep -qF -- "=== AIOS kernel ===" "$DEBUGCON_LOG" 2>/dev/null; then
    report ok "debugcon channel (0xE9)"
else
    report fail "debugcon channel (0xE9)" "banner missing in $DEBUGCON_LOG"
fi

case "$screenshot" in
    unavailable) report warn "framebuffer screenshot" "monitor unavailable; check the screen manually (M0.md §2)" ;;
    skipped)     report warn "framebuffer screenshot" "not captured (boot unfinished or no QEMU monitor socket)" ;;
    *)
        if [ "$screenshot" -ge 50 ]; then
            report ok "framebuffer shows content ($screenshot bright pixels, $FB_PPM)"
        else
            report fail "framebuffer shows content" "only $screenshot bright pixels in $FB_PPM"
        fi
        ;;
esac

echo
echo "== result: $ok ok, $bad failed =="
if [ "$bad" -ne 0 ]; then
    echo "M0 QEMU acceptance: FAILED" >&2
    echo "logs: $SERIAL_LOG, $DEBUGCON_LOG" >&2
    exit 1
fi
echo "M0 QEMU acceptance: PASSED"
