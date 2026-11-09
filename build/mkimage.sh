#!/usr/bin/env bash
# Сборка загрузочного FAT-образа ESP (EFI System Partition) для AIOS.
#
# Внутри образа:
#   /EFI/BOOT/BOOTX64.EFI  — загрузчик Limine (UEFI x86-64)
#   /limine.conf           — конфигурация Limine
#   /boot/kernel           — наше ядро (ELF, протокол Limine)
#
# Зачем mtools, а не loopback-монтирование: на macOS нельзя смонтировать
# FAT-образ без прав root и сторонних пакетов, а mformat/mcopy работают
# с обычным файлом без монтирования.
#
# Параметры (все можно задать через окружение):
#   IMAGE       — путь к создаваемому образу        (по умолчанию build/aios.img)
#   KERNEL      — путь к собранному ядру            (обязателен)
#   LIMINE_DIR  — каталог с BOOTX64.EFI             (обязателен)
#   CONF        — путь к limine.conf                (по умолчанию build/limine.conf)
#   WALLPAPER   — фон меню Limine                   (опционально, кладётся как /wallpaper.jpg)
#   SIZE_MIB    — размер образа в МиБ               (по умолчанию 64)

set -euo pipefail

IMAGE=${IMAGE:-build/aios.img}
KERNEL=${KERNEL:-}
LIMINE_DIR=${LIMINE_DIR:-}
CONF=${CONF:-build/limine.conf}
WALLPAPER=${WALLPAPER:-}
SIZE_MIB=${SIZE_MIB:-64}

die() {
    echo "mkimage: $*" >&2
    exit 1
}

for tool in mformat mcopy mmd mdir; do
    command -v "$tool" >/dev/null 2>&1 ||
        die "не найден '$tool'. Установите mtools: brew install mtools"
done

[ -n "$KERNEL" ] || die "не задан KERNEL (make image делает это сам)"
[ -f "$KERNEL" ] || die "ядро не найдено: '$KERNEL'. Соберите его: make kernel"

[ -n "$LIMINE_DIR" ] || die "не задан LIMINE_DIR — каталог с BOOTX64.EFI.
       Установите загрузчик (brew install limine) или укажите путь к релизу
       limine-binary: make image LIMINE_DIR=/path/to/limine-binary"
[ -f "$LIMINE_DIR/BOOTX64.EFI" ] ||
    die "в '$LIMINE_DIR' нет BOOTX64.EFI. Для brew это обычно
       \$(brew --prefix limine)/share/limine, для релиза — каталог limine-binary."

[ -f "$CONF" ] || die "не найден конфиг Limine: '$CONF'"

# Собираем во временный файл и переименовываем в конце: если сборка упала,
# старый образ остаётся целым.
mkdir -p "$(dirname "$IMAGE")"
tmp_image="${IMAGE}.tmp"
rm -f "$tmp_image"

# FAT32: -F включает FAT32, -T задаёт полное число 512-байтных секторов,
# -C создаёт файл образа. Размер 64 МиБ с запасом покрывает ESP-минимум.
total_sectors=$((SIZE_MIB * 1024 * 1024 / 512))
mformat -i "$tmp_image" -C -F -T "$total_sectors" ::

mmd -i "$tmp_image" ::/EFI ::/EFI/BOOT ::/boot

mcopy -i "$tmp_image" "$LIMINE_DIR/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$tmp_image" "$CONF" ::/limine.conf
mcopy -i "$tmp_image" "$KERNEL" ::/boot/kernel

# Wallpaper меню Limine (spec §10.1). В ядро не компилируется; если файла нет
# в рабочем дереве — образ собирается без фона.
if [ -n "$WALLPAPER" ]; then
    if [ -f "$WALLPAPER" ]; then
        mcopy -i "$tmp_image" "$WALLPAPER" ::/wallpaper.jpg
    else
        echo "mkimage: wallpaper не найден: '$WALLPAPER' — образ без wallpaper" >&2
    fi
fi

mv "$tmp_image" "$IMAGE"

echo "mkimage: образ готов: $IMAGE ($(du -h "$IMAGE" | cut -f1))"
mdir -i "$IMAGE" ::/EFI/BOOT ::/boot
