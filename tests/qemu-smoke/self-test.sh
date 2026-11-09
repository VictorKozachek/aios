#!/usr/bin/env bash
# Self-test for tests/qemu-smoke/run.sh.
#
# Exercises the smoke runner with a fake QEMU binary, so the gating logic can
# be verified on any host — no QEMU, OVMF, image or bootloader required. This
# is exactly what CI runs in the host-tests job, and it is the proof that the
# smoke gate fails when the kernel does not reach the marker.
#
# Exit status: 0 = all cases passed, 1 = at least one case failed.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
RUN="$ROOT/tests/qemu-smoke/run.sh"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/aios-smoke-selftest.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

FAKE_QEMU="$WORK/qemu-fake"
cat > "$FAKE_QEMU" <<'FAKE_QEMU_SCRIPT'
#!/usr/bin/env bash
# Minimal QEMU stand-in: parses -serial/-debugcon file targets, then behaves
# according to FAKE_QEMU_MODE.
set -u

mode=${FAKE_QEMU_MODE:-good}
serial=""
debugcon=""
while [ $# -gt 0 ]; do
    case "$1" in
        -serial) shift; serial=${1#file:} ;;
        -debugcon) shift; debugcon=${1#file:} ;;
    esac
    shift
done

case "$mode" in
    good)
        printf '=== AIOS kernel ===\nboot: ok, halting\n' > "$serial"
        printf '=== AIOS kernel ===\nboot: ok, halting\n' > "$debugcon"
        exit 33
        ;;
    debugcon-only)
        printf 'OVMF\nLimine\n' > "$serial"
        printf '=== AIOS kernel ===\nboot: ok, halting\n' > "$debugcon"
        exit 0
        ;;
    no-marker)
        printf 'OVMF\nLimine started\n' > "$serial"
        printf 'OVMF\n' > "$debugcon"
        exit 0
        ;;
    crash)
        printf 'triple fault\n' > "$serial"
        exit 1
        ;;
    slow)
        sleep 2
        printf 'boot: ok, halting\n' > "$serial"
        exit 0
        ;;
    hang)
        sleep 30
        ;;
    *)
        exit 1
        ;;
esac
FAKE_QEMU_SCRIPT
chmod +x "$FAKE_QEMU"

touch "$WORK/aios.img" "$WORK/ovmf-code.fd" "$WORK/ovmf-vars.fd"
SERIAL="$WORK/serial.log"
DEBUGCON="$WORK/debugcon.log"
OUT="$WORK/command.out"

run_smoke() {
    local mode=$1
    shift
    FAKE_QEMU_MODE="$mode" "$RUN" \
        --qemu "$FAKE_QEMU" \
        --image "$WORK/aios.img" \
        --ovmf-code "$WORK/ovmf-code.fd" \
        --ovmf-vars "$WORK/ovmf-vars.fd" \
        --serial "$SERIAL" \
        --debugcon "$DEBUGCON" \
        --timeout 5 \
        "$@"
}

passed=0
failed=0

ok() {
    passed=$((passed + 1))
    echo "self-test: PASS  $*"
}

bad() {
    failed=$((failed + 1))
    echo "self-test: FAIL  $*" >&2
}

expect_ok() {
    local name=$1
    shift
    if "$@" >"$OUT" 2>&1; then
        ok "$name"
    else
        bad "$name (expected success)"
        sed 's/^/    /' "$OUT" >&2
    fi
}

expect_fail() {
    local name=$1
    shift
    if "$@" >"$OUT" 2>&1; then
        bad "$name (expected failure, got success)"
        sed 's/^/    /' "$OUT" >&2
    else
        ok "$name"
    fi
}

expect_ok "marker in serial log" run_smoke good
expect_ok "marker only in debugcon log" run_smoke debugcon-only
expect_ok "marker accepted when exit status matches" run_smoke good --expect-exit-code 33
expect_fail "missing marker fails" run_smoke no-marker
expect_fail "QEMU crash fails" run_smoke crash
expect_fail "timeout without marker fails" run_smoke hang --timeout 2
expect_fail "unexpected exit status fails" run_smoke good --expect-exit-code 0

# A missing image must be a hard failure, not a "green on emptiness".
expect_fail "missing image fails" "$RUN" \
    --qemu "$FAKE_QEMU" \
    --image "$WORK/does-not-exist.img" \
    --ovmf-code "$WORK/ovmf-code.fd" \
    --serial "$SERIAL" \
    --debugcon "$DEBUGCON" \
    --timeout 2

# Usage errors must be reported as such (exit code 2).
expect_fail "non-numeric timeout is a usage error" "$RUN" \
    --qemu "$FAKE_QEMU" \
    --image "$WORK/aios.img" \
    --ovmf-code "$WORK/ovmf-code.fd" \
    --timeout nope

# Logs must be preserved for debugging after a failed run.
if FAKE_QEMU_MODE=no-marker "$RUN" \
    --qemu "$FAKE_QEMU" \
    --image "$WORK/aios.img" \
    --ovmf-code "$WORK/ovmf-code.fd" \
    --serial "$SERIAL" \
    --debugcon "$DEBUGCON" \
    --timeout 5 >"$OUT" 2>&1; then
    bad "failed run keeps logs (expected failure)"
else
    if grep -qF "OVMF" "$SERIAL"; then
        ok "failed run keeps serial log for debugging"
    else
        bad "failed run keeps serial log for debugging"
    fi
    if grep -qF -- "--- $SERIAL (tail) ---" "$OUT" && grep -qF "OVMF" "$OUT"; then
        ok "failed run prints log tails to the console"
    else
        bad "failed run prints log tails to the console"
        sed 's/^/    /' "$OUT" >&2
    fi
fi

echo "self-test: ${passed} passed, ${failed} failed"
if [ "$failed" -ne 0 ]; then
    exit 1
fi
echo "self-test: ALL PASSED"
