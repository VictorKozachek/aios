# AIOS — сборка и запуск M0 (x86-64, UEFI, Limine).
#
# Тулчейн. В PATH первым идёт Homebrew-cargo, и это ловушка: у Homebrew-сборки
# Rust нет target'ов для голого железа, поэтому сборка падает на
# "can't find crate for core". Здесь явно берём rustup-cargo из ~/.cargo/bin.
# Плюс cargo запускает `rustc` по имени через PATH, поэтому ~/.cargo/bin
# ставим первым в PATH и дополнительно фиксируем RUSTC — иначе компилятор
# всё равно окажется Homebrew-шным.
CARGO ?= $(HOME)/.cargo/bin/cargo
RUSTC ?= $(HOME)/.cargo/bin/rustc

export PATH := $(HOME)/.cargo/bin:$(PATH)
export RUSTC

# Целевая платформа ядра. Без --target ядро соберётся под хост и не слинкуется.
TARGET := x86_64-unknown-none
PROFILE ?= debug

ifeq ($(PROFILE),release)
  PROFILE_DIR := release
  CARGO_PROFILE_FLAG := --release
else
  PROFILE_DIR := debug
  CARGO_PROFILE_FLAG :=
endif

# Ядро компилируется БЕЗ PIE: rustc для этого таргета по умолчанию включает
# position-independent codegen, из-за чего вызовы идут через GOT
# (`callq *sym@GOTPCREL(%rip)`). Наш linker.ld выбрасывает .got, и такой
# вызов превращается в переход по мусорному адресу (проверено: page fault
# по CR2=0x78 на первом же println!). Поэтому — static relocation model и
# kernel code model (ядро в старшей половине, topmost 2 GiB).
KERNEL_RUSTFLAGS := -C relocation-model=static -C code-model=kernel

KERNEL := target/$(TARGET)/$(PROFILE_DIR)/aios-kernel
IMAGE := build/aios.img
LIMINE_CONF := build/limine.conf

# --- Версия для сплэша и wallpaper меню (T7) -------------------------------
#
# Ядро читает AIOS_VERSION/GIT_HASH через option_env! (kernel/src/splash.rs),
# поэтому версия не хардкодится в коде. AIOS_VERSION — версия продукта
# (по умолчанию v0.0.1), переопределяется из окружения/командной строки;
# GIT_HASH всегда из git и печатается отдельной мелкой строкой.
# WALLPAPER кладётся в FAT-образ как /wallpaper.jpg (spec §10.1).
AIOS_VERSION ?= v0.0.1
GIT_HASH ?= $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)
WALLPAPER ?= docs/design/assets/wallpaper.jpg

ifeq ($(strip $(AIOS_VERSION)),)
$(error AIOS_VERSION пуст: версия сплэша должна быть задана (например v0.0.1))
endif

# --- Внешние инструменты ---------------------------------------------------
#
# Пути ищем сами: brew ставит всё в /opt/homebrew (Apple Silicon) или
# /usr/local (Intel), а в CI — в системные каталоги или локальный релиз
# limine-binary. Любую переменную можно переопределить с командной строки:
#   make qemu OVMF_CODE=/path/to/edk2-x86_64-code.fd
QEMU ?= qemu-system-x86_64
QEMU_TIMEOUT ?= 120
QEMU_DISPLAY ?= none

QEMU_PREFIX := $(shell brew --prefix qemu 2>/dev/null)

OVMF_CODE ?= $(firstword $(wildcard \
    $(QEMU_PREFIX)/share/qemu/edk2-x86_64-code.fd \
    /opt/homebrew/share/qemu/edk2-x86_64-code.fd \
    /usr/local/share/qemu/edk2-x86_64-code.fd \
    /usr/share/qemu/edk2-x86_64-code.fd \
    /usr/share/OVMF/OVMF_CODE.fd \
    /usr/share/OVMF/OVMF_CODE_4M.fd \
    /usr/share/edk2/x64/OVMF_CODE.fd))

OVMF_VARS_SRC ?= $(firstword $(wildcard \
    $(QEMU_PREFIX)/share/qemu/edk2-i386-vars.fd \
    /opt/homebrew/share/qemu/edk2-i386-vars.fd \
    /usr/local/share/qemu/edk2-i386-vars.fd \
    /usr/share/qemu/edk2-i386-vars.fd \
    /usr/share/OVMF/OVMF_VARS.fd \
    /usr/share/edk2/x64/OVMF_VARS.fd))

LIMINE_PREFIX := $(shell brew --prefix limine 2>/dev/null)
LIMINE_DIR ?= $(firstword $(wildcard \
    $(LIMINE_PREFIX)/share/limine \
    /opt/homebrew/opt/limine/share/limine \
    /usr/local/opt/limine/share/limine \
    /usr/share/limine \
    .ci/limine))

# Единая проверка «инструмент есть» с человеческим сообщением.
define require_tool
	@command -v $(1) >/dev/null 2>&1 || { \
		echo "make: не найден '$(1)'." >&2; \
		echo "      $(2)" >&2; \
		exit 1; \
	}
endef

.PHONY: all help kernel image qemu smoke test clean

all: image

help:
	@echo "AIOS M0 — цели:"
	@echo "  make kernel   — собрать ядро ($(TARGET), профиль $(PROFILE))"
	@echo "  make image    — собрать FAT-ESP образ $(IMAGE) с Limine и ядром"
	@echo "  make qemu     — запустить QEMU; логи в build/run-<время>-<pid>/ (см. build/last-run)"
	@echo "  make smoke    — запустить QEMU и проверить ключевые строки лога"
	@echo "  make test     — тесты хоста (memfmt) + сборка ядра под target"
	@echo "  make clean    — удалить собранное и логи"
	@echo
	@echo "Переменные: PROFILE=debug|release, QEMU_DISPLAY=none|cocoa|gtk,"
	@echo "            QEMU_TIMEOUT=секунд (лимит на загрузку), OVMF_CODE=..., LIMINE_DIR=..."
	@echo "            AIOS_VERSION=... GIT_HASH=... WALLPAPER=... (сплэш и меню)"
	@echo
	@echo "Найдено: LIMINE_DIR='$(LIMINE_DIR)'"
	@echo "         OVMF_CODE='$(OVMF_CODE)'"
	@echo "         OVMF_VARS_SRC='$(OVMF_VARS_SRC)'"

# Сборка ядра под голое железо.
kernel:
	$(call require_tool,$(CARGO),Установите rustup (https://rustup.rs) или задайте CARGO=/path/to/cargo)
	RUSTFLAGS='$(KERNEL_RUSTFLAGS)' AIOS_VERSION='$(AIOS_VERSION)' GIT_HASH='$(GIT_HASH)' \
		$(CARGO) build --package aios-kernel --target $(TARGET) $(CARGO_PROFILE_FLAG)
	@echo "make: ядро готово: $(KERNEL)"

# FAT-образ ESP: EFI/BOOT/BOOTX64.EFI + limine.conf + /boot/kernel.
#
# Сборка образа защищена каталогом-замком: два параллельных `make smoke`
# (разные агенты) собирают один и тот же build/aios.img, и без замка второй
# mkimage мог бы переписать образ под работающим QEMU первого. Замок
# снимается по trap; замок мёртвого владельца подбирается.
image: kernel
	$(call require_tool,mformat,Установите mtools: brew install mtools)
	@set -e; \
	lock="$(CURDIR)/build/.image.lock"; \
	mkdir -p "$(CURDIR)/build"; \
	while ! mkdir "$$lock" 2>/dev/null; do \
		owner=$$(cat "$$lock/pid" 2>/dev/null || true); \
		if [ -n "$$owner" ] && ! kill -0 "$$owner" 2>/dev/null; then \
			rm -rf "$$lock"; \
			continue; \
		fi; \
		echo "make: image: ждём другую сборку образа ($$lock)"; \
		sleep 1; \
	done; \
	trap 'rm -rf "$$lock"' EXIT INT TERM; \
	echo $$$$ >"$$lock/pid"; \
	KERNEL=$(KERNEL) IMAGE=$(IMAGE) LIMINE_DIR='$(LIMINE_DIR)' CONF=$(LIMINE_CONF) \
		WALLPAPER='$(WALLPAPER)' ./build/mkimage.sh

# Запуск под UEFI (OVMF). По умолчанию вывод — в файлы, окно выключено.
qemu: image
	$(call require_tool,$(QEMU),Установите QEMU: brew install qemu)
	QEMU='$(QEMU)' IMAGE=$(IMAGE) \
	OVMF_CODE='$(OVMF_CODE)' OVMF_VARS_SRC='$(OVMF_VARS_SRC)' \
	QEMU_TIMEOUT=$(QEMU_TIMEOUT) QEMU_DISPLAY=$(QEMU_DISPLAY) \
		./build/run-qemu.sh

# Дымовой тест: QEMU + проверка маркеров в логе.
smoke: image
	$(call require_tool,$(QEMU),Установите QEMU: brew install qemu)
	QEMU='$(QEMU)' IMAGE=$(IMAGE) \
	OVMF_CODE='$(OVMF_CODE)' OVMF_VARS_SRC='$(OVMF_VARS_SRC)' \
	QEMU_TIMEOUT=$(QEMU_TIMEOUT) \
		./build/smoke.sh

# Тесты хоста + проверка, что ядро собирается под target.
test:
	$(call require_tool,$(CARGO),Установите rustup (https://rustup.rs) или задайте CARGO=/path/to/cargo)
	$(CARGO) test --package memfmt
	RUSTFLAGS='$(KERNEL_RUSTFLAGS)' AIOS_VERSION='$(AIOS_VERSION)' GIT_HASH='$(GIT_HASH)' \
		$(CARGO) build --package aios-kernel --target $(TARGET) $(CARGO_PROFILE_FLAG)
	@echo "make: тесты хоста прошли, ядро собирается"

clean:
	$(CARGO) clean
	rm -f $(IMAGE) build/ovmf-vars.fd build/serial.log build/debugcon.log
	@echo "make: артефакты удалены"
