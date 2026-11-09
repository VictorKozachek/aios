#!/usr/bin/env bash
# Install the Limine UEFI loader for the CI image build.
#
# `make image` needs a directory containing BOOTX64.EFI (see build/mkimage.sh
# and the LIMINE_DIR variable in the Makefile). GitHub-hosted runners have no
# `limine` package, so CI downloads the official release binary here.
#
# Usage:
#   tests/qemu-smoke/install-limine.sh [--version VERSION] [--dest DIR]
#
# Defaults: version 12.9.6, destination .ci/limine (the Makefile's last
# LIMINE_DIR fallback). Environment variables LIMINE_VERSION and LIMINE_DIR
# are honoured as well.

set -euo pipefail

VERSION=${LIMINE_VERSION:-12.9.6}
DEST=${LIMINE_DIR:-.ci/limine}

die() {
    echo "limine: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=$2; shift 2 ;;
        --dest) DEST=$2; shift 2 ;;
        -h|--help)
        awk 'NR > 1 { if (/^#/) { sub(/^# ?/, ""); print; next } if (NF == 0) { print ""; next } exit }' "$0"
            exit 0
            ;;
        *) die "unknown argument: $1" ;;
    esac
done

[ -n "$VERSION" ] || die "version must not be empty"
[ -n "$DEST" ] || die "destination must not be empty"
command -v curl >/dev/null 2>&1 || die "curl not found"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/aios-limine.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

base_url="https://github.com/limine-bootloader/limine/releases/download/v${VERSION}"
efi=""

for asset in limine-binary.tar.xz limine-binary.tar.gz limine-binary.zip; do
    url="$base_url/$asset"
    echo "limine: trying $url"
    if ! curl -fsSL --retry 3 --retry-delay 2 "$url" -o "$WORK/$asset"; then
        continue
    fi
    case "$asset" in
        *.tar.xz) tar -xJf "$WORK/$asset" -C "$WORK" || continue ;;
        *.tar.gz) tar -xzf "$WORK/$asset" -C "$WORK" || continue ;;
        *.zip)
            command -v unzip >/dev/null 2>&1 || die "unzip not found (needed for $asset)"
            unzip -q "$WORK/$asset" -d "$WORK" || continue
            ;;
    esac
    efi=$(find "$WORK" -type f -name BOOTX64.EFI -print -quit)
    if [ -n "$efi" ]; then
        break
    fi
done

[ -n "$efi" ] || die "could not find BOOTX64.EFI in the Limine ${VERSION} release"

mkdir -p "$DEST"
cp -f "$efi" "$DEST/BOOTX64.EFI"
echo "limine: installed $DEST/BOOTX64.EFI (version $VERSION)"
