# Технические факты для спецификации AIOS (ОС с нуля, x86-64, Ryzen 9 AM5, UEFI, HTTP + LLM)

Дата сбора данных: **2026-11-09**. Все версии/цифры — на эту дату.
Пометки: `[изм.]` — измерено мной (команды в §9), `[док.]` — из документации/спеки, `[оц.]` — моя оценка, требует проверки.

---

## 0. Сводка в 8 строк

1. Загрузка: **UEFI-only** — правильный выбор; CSM/legacy на AM5 уже рудимент. Для MVP брать **Limine** (даёт HHDM, 5-level paging, модули, ACPI RSDP, EFI-memmap, entropy, и, главное, **bring-up всех AP + x2APIC**), а не свой загрузчик и не `bootloader`-crate.
2. Язык: **Rust** (`#![no_std]`), версия stable 1.99.1, ночные фичи всё ещё нужны частично. Экосистема: `x86_64` 0.15.5, `uefi` 0.41.0, `bootloader` 0.11.17, `smoltcp` 0.14.0, `rustls` 0.23.46.
3. Минимум ядра на Zen 5: GDT/IDT/TSS → APIC/x2APIC → ACPI MADT/MCFG → SMP (INIT-SIPI-SIPI, но с Limine — не нужно) → paging 4-level → таймеры (TSC deadline) → SYSCALL/SYSRET → XSAVE/AVX-512.
4. Драйверы: **NVMe реален в ~1.3 тыс. строк** (Redox `nvmed` = 1 318 `[изм.]`), **xHCI — самый дорогой** (Linux 15.6 тыс. `[изм.]`, Redox 8 955 `[изм.]`), **RTL8125 — главный риск** (Linux `r8169` 7 080 `[изм.]`, FreeBSD `if_rge` 2 787 `[изм.]`, у Redox 2.5G **не поддержан**).
5. Сеть: свой TCP/IP — это ~58 тыс. строк уровня `smoltcp` `[изм.]`; правильно — взять `smoltcp` (no_std) и не писать TCP.
6. TLS: не «ещё один модуль», а отдельный проект (rustls 45.7 тыс. строк `[изм.]`, embedded-tls 11.1 тыс. `[изм.]`). Для MVP — оффлайн-доставка весов + SHA-256, HTTP в доверенной LAN.
7. ФС: для MVP — свой минимальный log-structured/блочный слой + **сырое блочное устройство под веса**; FAT32 читать незачем (UEFI-доступ умирает после `ExitBootServices`), EXT4 — нет.
8. Трудозатраты до демо: **~30–45 человеко-месяцев** (команда 4–6 низкоуровневых инженеров → 9–12 мес.; один разработчик → 2.5–4 года). Ядро — не самое дорогое; дороже драйверы, порт инференса и TLS.

**Отдельно, самое важное для продуктовой части:** на Ryzen 9 (2 канала DDR5, ~80–100 ГБ/с) **DeepSeek-V3/R1 671B физически не работает** (Q4 ≈ 377 ГБ, не влезает в 192 ГБ RAM; с NVMe-стримингом 1–3.5 ток/с). Целевая модель MVP — **дистиллят 7B–32B, Q4_K_M**.

---

## 1. Точка старта на современном железе (2026): UEFI-only

### 1.1 Состояние платформы

| Факт | Статус 2026 | Источник |
|---|---|---|
| Legacy BIOS / CSM | Уходит. Fedora с 37 не ставится в BIOS-режиме; Windows 11 требует UEFI+Secure Boot; на Intel-платформах CSM вырезан с 400-й серии; на AM5 (600/800-е чипсеты) CSM либо отсутствует, либо рудиментарен | [Fedora changes](https://fedoraproject.org/wiki/Changes/DeprecateLegacyBIOS), [MS: Windows 11 requirements](https://learn.microsoft.com/en-us/windows/whats-new/windows-11-requirements) |
| Класс прошивки на AM5 | UEFI class 3 (без CSM) — типично; на части плат остаётся пункт «CSM/Compatibility Support Module» в BIOS для старых GPU `[оц.]` | проверить на конкретной плате |
| Secure Boot CA | **Сертификаты Microsoft UEFI CA 2011 истекают в июне 2026**, идёт массовое обновление на CA 2023 + обновление KEK/dbx. Практический эффект: на плате, обновлённой в 2026, старый подписанный загрузчик может перестать валидироваться | [Microsoft Learn: Secure boot](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/oem-secure-boot) (стр. обновлена 2026-03-12) |
| ACPI на AM5 | ACPI 6.x, MADT (APIC/x2APIC/IOAPIC), MCFG (ECAM), SRAT/SLIT (для 2 CCD полезно), PPTT, FADT, HPET, DMAR/IVRS (AMD-Vi) | [ACPI 6.6](https://uefi.org/specs/ACPI/6.6/) |
| Прошивка | AMI Aptio V / AGESA; GOP-драйвер даёт linear framebuffer; `EFI_SIMPLE_FILE_SYSTEM` даёт FAT32 | UEFI 2.11 spec |

### 1.2 Свой UEFI-загрузчик vs готовые

| Вариант | Что даёт | Чего не даёт | Трудозатраты | Контроль |
|---|---|---|---|---|
| **Свой UEFI-app** (Rust + `uefi` 0.41.0) | Полный контроль: сам читаешь `GetMemoryMap`, `AllocatePages`, `LoadImage`, GOP, `ExitBootServices`, сам строишь таблицы страниц | Нет SMP-bring-up, нет парсера ACPI, нет файловой системы после `ExitBootServices`, отладка firmware-квирков на каждой плате | 2–4 чел.-нед. до «ядро стартовало» `[оц.]` | Максимальный; это и есть «своя ОС» |
| **Limine** (v12.9.6, 2026-10-27) `[док.]` | UEFI+BIOS, x86-64/aarch64/riscv64/loongarch64; протокол с запросами: HHDM, **Paging Mode (4LVL/5LVL + LA57)**, Framebuffer, **MP (bring-up всех AP + опционально x2APIC)**, Memory Map, RSDP (ACPI), SMBIOS, EFI System Table/EFI memmap, Module, **TSC frequency**, **Entropy**, TPM event log, bootloader performance | Нет парсера ACPI MADT/MCFG (RSDP даёт только корень), нет драйверов, нет ФС после handoff | 1–3 дня до «ядро стартовало» `[оц.]` | Высокий: ядро полностью ваше, загрузчик — сменный компонент |
| **GRUB + Multiboot2** | Знакомо, есть в QEMU, много документации | Multiboot2 — legacy-путь; на UEFI-only железе требует EFI-сборки GRUB; нет paging/HHDM/SMP; 32-bit entry по умолчанию; модули грузятся в RAM | 2–5 дней | Средний, но архитектурно тупиковый |
| **`bootloader` crate 0.11.17** (rust-osdev) | Свой «BIOS+UEFI» загрузчик на Rust, `BootInfo` (framebuffer, memory map, physical memory offset), конфигурируемый стек, artifact-dependencies | **Только nightly Rust** + `llvm-tools-preview`; нет SMP; нет ACPI; 4-level paging; ориентирован на tutorial-сценарий | 1–2 дня | Средний, но привязывает к ночному тулчейну |

**Ключевой факт (проверено по спеке Limine `PROTOCOL.md`, trunk):** запрос `LIMINE_MP_REQUEST` заставляет загрузчик **самому поднять вторичные процессоры**; каждый AP «припаркован» и стартует по атомарной записи в `goto_address`, со стеком, размер которого задаётся Stack Size feature; при `flags & LIMINE_MP_REQUEST_X86_64_X2APIC` включается x2APIC; MTRR AP синхронизируются с BSP, как требует Intel SDM Vol.3A 12.11.5. Также есть `LIMINE_ENTROPY_REQUEST` — готовый источник энтропии для TLS, и `LIMINE_TSC_FREQUENCY_REQUEST` — частота TSC без калибровки.

**Вывод:** это снимает с проекта самую противную часть SMP bring-up (INIT-SIPI-SIPI, trampoline в real mode, A20, MTRR). Для MVP — Limine. Свой загрузчик писать **позже** как отдельную задачу «продуктового контроля» (он не является источником технического риска).

### 1.3 Secure Boot — мешает ли

Мешает загрузке неподписанного загрузчика; варианты по возрастанию цены:

1. **Выключить Secure Boot в прошивке** — 30 секунд, для dev-стенда/апплаенса приемлемо; ломает «продуктовую» историю.
2. **Свои ключи**: `sbctl` / `sbsign`, enroll own PK/KEK/db (нужен физический доступ, один раз, `SetupMode=1`), подписывать `BOOTX64.EFI` своим ключом. Рабочий путь для железа под контролем.
3. **shim + свой MOK** — цепочка `shim` (подписан MS UEFI CA) → свой ключ в MOK. Путь для «чужих» машин.
4. **Подпись у Microsoft (UEFI CA)** — EV-сертификат, аудит, недели и деньги; только при поставке устройства массово.

Доп. риск: обновления **dbx** могут отозвать известные загрузчики; после июня 2026 — новая цепочка CA 2023. Для прошивки своего железа фиксируйте версию прошивки и не делайте «обновление BIOS» частью сценария.

---

## 2. Языки и фреймворки для ОС с нуля

### 2.1 Версии (2026-11-09)

| Компонент | Версия | Комментарий |
|---|---|---|
| Rust (stable) | **1.99.1** (2026-10-16) | `x86_64-unknown-none`, `x86_64-unknown-uefi` — tier-2 цели |
| `x86_64` | 0.15.5 (2026-09-24) | GDT/IDT/TSS/paging/port I/O; ~5.9M загрузок |
| `uefi` (uefi-rs) | 0.41.0 (2026-09-21) | 2.87M загрузок; стабильный Rust |
| `bootloader` | 0.11.17 (2026-07-27) | требует nightly |
| `smoltcp` | 0.14.0 (2026-08-17) | no_std TCP/IP (ARP, IPv4/IPv6, ICMP, UDP, TCP, DHCP) |
| `rustls` | 0.23.46 (2026-10-14) | TLS 1.2/1.3; есть no_std-режим |
| `embedded-tls` | 0.19.0 (2026-06-01) | TLS 1.3 only, минимальный, свой crypto |
| `virtio-drivers` | 0.13.0 (2026-03-03) | готовые virtio-blk/net/gpu для QEMU |
| `acpi` | 6.1.1 (2026-10-23) | парсер RSDP/XSDT/MADT/MCFG |
| `linked_list_allocator` / `spin` / `volatile` | 0.10.6 / 0.12.3 / 0.6.1 | базовый heap/locking для no_std |
| `uart_16550` | 0.9.0 | COM1-логирование (в QEMU и на железе) |
| `xhci` | 0.9.2 (2023) | **заброшен, 3 года без обновлений** — писать самим |
| Zig | 0.17.1 | freestanding-цель есть, экосистема ядра — почти нулевая |

### 2.2 Сравнение

| Критерий | Rust (`no_std`) | C (multiboot2) | Zig |
|---|---|---|---|
| Память | borrow checker в `unsafe`-границах; типизация MMIO через типы-обёртки | ручной аудит, UAF/двойные free — норма | safety-фичи есть, но `@ptrCast`-стиль возвращает C-подход |
| Экосистема ядра | лучшая: `x86_64`, `uefi`, `smoltcp`, `rustls`, `virtio-drivers`, `acpi` | максимальная, но всё через вендорские копипасты | тонкая |
| Асинхронность | `async/await` + свой executor (в `no_std` работает, `alloc` нужен) | ручные state machine | `async` есть |
| Аллокатор | `#[global_allocator]` + свой heap; `alloc_error_handler` **до сих пор нестабилен** (есть в Unstable Book) → нужен nightly на эту фичу `[док.]` | `malloc`-замена | свой |
| panic-handling | `panic="abort"` + `#[panic_handler]`; unwinding в no_std недоступен | `-fno-exceptions`, руками | `@panic` |
| Совместимость с существующим кодом | C-крейты через `bindgen` в no_std — больно; C++ (llama.cpp) — только через FFI+libc-шим | тривиально | C-ABI легко |
| Реальные примеры | Redox, Asterinas, Theseus, `bootloader`, `smoltcp`, Redox-драйверы | SerenityOS (C++), Linux, FreeBSD | — (нет заметных ОС) |

### 2.3 Что реально используют новые ОС (2024–2026) `[изм. по GitHub API]`

| Проект | Язык | Метрика кода | Статус 2026 | Заметки |
|---|---|---|---|---|
| **SerenityOS** | C++ | дерево ≈ **54.5 МБ** исходников (~1.4M LOC оц.), **`Kernel/` = 150 953 строки** `[изм.]` | активен, 33 954★, коммиты 2026-11-09 | Один человек + сообщество с 2018; доказательство, что «с нуля и красиво» возможно, но это 8 лет и сотни контрибьюторов |
| **Redox** | Rust (microkernel, драйверы в userspace) | `kernel/` = **43 133 строки** `[изм.]`, драйверы отдельно = 44 040 `[изм.]` | **0.9.0 (2025-09-22)**, 16 704★ | 10 лет; boot с NVMe, USB HID, ZFS-подобная ФС; **есть Rust-референсы нужных нам драйверов** |
| **Asterinas** | Rust (framekernel, Linux ABI) | ≈ **284 тыс. LOC** оц. (9.2 МБ Rust + 1.0 МБ C) `[изм.]` | USENIX ATC'25; >210 syscall'ов; TCB ≈14% кода | Академия (SUSTech/PKU) + Ant Group, с 2022 |
| **Theseus** | Rust | ≈117 тыс. LOC оц. `[изм.]` | **не активен с 2024-09** | research-проект, «intralingual» дизайн |
| **Linux** | C | — | — | Как эталон объёма: `net/ipv4/tcp_input.c` = 7 862 `[изм.]`, `fs/fat/dir.c` = 1 477 `[изм.]` |

**Вывод по стеку:** Rust + Limine. C рассматривать только как FFI-слой для llama.cpp/ggml (и то лучше — вынести инференс в отдельный «сервис» позже).

---

## 3. Обязательный минимум инициализации x86-64 на Ryzen 9 (Zen 5)

### 3.1 Что именно даёт железо

| Свойство | Значение | Источник |
|---|---|---|
| CPU | Ryzen 9 9950X: Zen 5 «Granite Ridge», 16C/32T, AM5, до 5.7 ГГц, L2 1 МБ/ядро, L3 32 МБ на CCD (2 CCD) | [Wikipedia: Zen 5](https://en.wikipedia.org/wiki/Zen_5) |
| CPUID family | **Family 1Ah** (CPUID-уровень для проверок) | там же |
| Память | 2 канала DDR5 (JEDEC 5600 → ~89.6 ГБ/с теор.; EXPO 6400 → ~102 ГБ/с); официально до **192 ГБ** (4×48), 4 DIMM = падение частоты | спецификации плат AM5; [обзор ASUS X870](https://gear-up.me/sar/asus-prime-x870-p-wifi-am5-atx.html) |
| SIMD | AVX-512 **полной 512-битной ширины** на десктопных Ryzen 9000, AVX512-VNNI, **BF16**, AVX-VNNI; **AMX нет** | [Wikipedia: Zen 5](https://en.wikipedia.org/wiki/Zen_5) |
| Paging | 4-level и 5-level (LA57) — Limine умеет включать по запросу | [Limine PROTOCOL.md](https://github.com/Limine-Bootloader/limine-protocol/blob/trunk/PROTOCOL.md) |
| Прерывания | APIC + x2APIC (MSR-режим), IOAPIC, MSI/MSI-X | ACPI 6.6 / AMD APM Vol.2 |
| Таймеры | TSC (invariant), TSC-deadline, HPET, APIC timer | Intel SDM Vol.3A / AMD APM Vol.2 |
| Топология | AMD-специфично: **CPUID Fn8000_001E** (CoreId/NodeId), APIC ID в CPUID Fn1 EBX[31:24], размеры адресов в Fn8000_0008 | AMD APM Vol.3 |
| Крипто | RDRAND/RDSEED, AES-NI, SHA | там же |
| Виртуализация | AMD-V (SVM) — не нужна для MVP | — |

### 3.2 Порядок инициализации (минимум, без которого нет демо)

| # | Шаг | Детали | Риск | Оценка |
|---|---|---|---|---|
| 1 | Приём управления от загрузчика | Limine: HHDM-оффсет, `boot_info`, framebuffer, memmap, RSDP | низкий | 2 дня |
| 2 | Свой стек + ранний вывод | serial `uart_16550` (0x3F8) + framebuffer-консоль (GOP, BGRA32) + `debugcon` 0xE9 в QEMU | низкий | 3 дня |
| 3 | GDT + TSS + IDT | IST для double-fault/NMI/MCE; `x86_64` crate закрывает 80% | низкий | 3 дня |
| 4 | Heap | `linked_list_allocator` поверх собственного frame allocator'а | низкий | 3 дня |
| 5 | Физ. память | разбор Limine memmap (Usable/BootloaderReclaimable/ACPI), bitmap/buddy | средний | 1–1.5 нед. |
| 6 | Paging | 4-level, identity для MMIO, HHDM для доступа к физпамяти, `NX`/`WP`, per-page флаги (UC для MMIO, WC для framebuffer) | **средний-высокий** | 2–3 нед. |
| 7 | ACPI | RSDP → XSDT → MADT (LAPIC/IOAPIC/x2APIC/override'ы), MCFG (ECAM → PCIe config), FADT; SRAT/PPTT позже | средний | 2 нед. |
| 8 | APIC | x2APIC (MSR 0x800+), LAPIC timer либо **TSC-deadline** (проще и точнее), IOAPIC для legacy-IRQ, MSI/MSI-X для PCIe | средний | 1.5–2 нед. |
| 9 | SMP | **С Limine — только per-CPU структуры + запуск AP-функции через `goto_address`**; без Limine: INIT-SIPI-SIPI + trampoline в 16-бит + A20 + GDT для AP | без Limine — **высокий** | с Limine 1 нед.; без — 4–6 нед. |
| 10 | Планировщик | round-robin по таймеру → позже work-stealing; per-CPU run queue; IPI (reschedule/TLB shootdown) | средний | 3–4 нед. |
| 11 | SYSCALL/SYSRET | EFER.SCE=1, STAR/LSTAR/FMASK, swapgs (GSBASE 0xC0000101/2), `TSS.RSP0` | средний | 1 нед. |
| 12 | FPU/AVX-512 | XCR0 (SSE|AVX|opmask|ZMM_Hi256|Hi16_ZMM), XSAVE/XRSTOR в контекст-свитче; **проверять CPUID перед использованием** | средний | 1 нед. |
| 13 | Userspace | ELF64-загрузчик, ring3, страничные права | средний | 2–3 нед. |

**Микроархитектурные заметки под Zen 5 (важно для инференса, не для ядра):**
- 2 CCD × 8 ядер: L3 разделён, межчиповые обмены дороже; для LLM-потоков лучше affinity «внутри CCD», а модель, шардированную по NUMA-домену, — избегать (см. §8.4).
- AVX-512 без даунклока на Zen 5 (в отличие от Intel) — можно использовать везде, но `XCR0` и `XSAVE`-область должны быть корректны, иначе получаем `#UD`/`#NM`.
- Нет AMX → нет tile state, XSAVE-область меньше.
- Инструкции `MONITOR/MWAIT` есть; `RDTSCP` есть (нужен для per-CPU TSC_AUX).

---

## 4. Драйверы: что писать и сколько это стоит

### 4.1 Реальные размеры референсных реализаций `[изм.: wc -l по git HEAD]`

| Драйвер | Linux | FreeBSD | Redox (Rust) | Вывод |
|---|---|---|---|---|
| **NVMe** (host) | `drivers/nvme/host/pci.c` = **4 587**, `core.c` = 5 620 | `nvme_pci.c` (аналогично) | **`storage/nvmed` = 1 318** (admin+IO очереди, interrupt vectors) | MVP реален в **1.5–2.5 тыс. строк** |
| **virtio-blk / net** | `virtio_net.c` = 7 351 | — | `net/virtio-netd` = **254**, `virtio-core` = 1 545 | В QEMU — самый дешёвый путь |
| **xHCI** | `xhci.c` 5 741 + `xhci-ring.c` 4 496 + `xhci-mem.c` 2 519 + `xhci-dbgcap.c` 1 664 + … = **15 608**; плюс USB-core (`hcd.c` 3 194 + `hub.c` 6 585 + `message.c` 2 533 + `urb.c` 1 029 = 13 341) | — | `usb/xhcid` = **8 955** | **Самый дорогой драйвер**; для MVP не нужен |
| **Realtek RTL8125/8126 (2.5G/5G)** | `drivers/net/ethernet/realtek/r8169_main.c` = **7 080** (вся семья 8169/8168/8125/8126, LTR, LED, firmware) | `sys/dev/rge/if_rge.c` = **2 787**, RTL8125 поддержан (7 упоминаний) | `net/rtl8168d` = **462** — **только 8168/8169, 2.5G НЕТ** | Есть BSD-лицензионный референс на 2.8k строк — это лучший донор |
| **Intel I225-V/I226-V (2.5G)** | `drivers/net/ethernet/intel/igc/igc_main.c` = **7 875** | `if_igc.c` есть | `net/e1000d` = **459** (это 1G e1000, не igc) | Дороже Realtek (больше offload'ов), но документация/референсы чище |
| **e1000 (эмуляция в QEMU)** | `e1000_main.c` = **5 328** | — | — | В QEMU проще `virtio-net` |
| **Framebuffer (GOP)** | — | — | `graphics` = 3 814 (весь стек) | Линейный BGRA-буфер: **300–800 строк**, если без шрифтов/2D |

### 4.2 Какие NIC стоят на типичных AM5-платах (2026)

| Чипсет/класс | Типичный LAN | Примеры |
|---|---|---|
| X870E / X670E премиум | **Realtek RTL8126 (5GbE)**, иногда Intel I226-V + Wi-Fi 7 | Gigabyte X870E AORUS PRO X3D — RTL8126 ([dateks](https://www.dateks.lv/cenas/sistemplates-amd-procesoriem/1299743-gigabyte-x870e-aorus-pro-x3d?vat=0)), ASUS ROG Crosshair X870E Hero ([ixbt](https://www.ixbt.com/platform/asus-rog-crosshair-x870e-hero-review.html)) |
| B850/B650 мид | **Realtek RTL8125 (2.5GbE)** — доминирует | массовые платы |
| Отдельные платы | Intel I225-V / I226-V (2.5G) | часть ASUS/MSI |
| Wi-Fi 7 | MediaTek MT7925 / Qualcomm | не трогаем в MVP |

**Практический вывод:** «типичная AM5-плата» = **Realtek**. Значит драйвер RTL8125/8126 нужен, а он самый закрытый (Realtek не публикует register guide; референсы — reverse-engineered: Linux `r8169`, FreeBSD `if_rge.c`, OpenBSD `if_rge.c`).

### 4.3 Оценка сложности (по компонентам)

| Драйвер | Сложность | Почему | Оценка MVP-объёма | Оценка труда |
|---|---|---|---|---|
| GOP framebuffer | ★☆☆☆☆ | линейный буфер + шрифт | 0.5–1 тыс. строк | 1–2 нед. |
| virtio-blk/net | ★★☆☆☆ | спека публичная, QEMU-эталон, `virtio-drivers` 0.13 | 0.5–1.5 тыс. | 1–2 нед. |
| NVMe | ★★★☆☆ | admin/IO очереди, PRP-списки, IDENTIFY, MSI-X, таймауты, reset/recovery | 1.5–2.5 тыс. | 3–5 нед. |
| e1000 (QEMU) | ★★★☆☆ | простые ring'и, но legacy | 1–2 тыс. | 2–3 нед. |
| xHCI + HID | ★★★★★ | TRB-ринги, port state machine, hubs, class drivers | 6–10 тыс. | 2–4 мес. |
| RTL8125/8126 | ★★★★☆ | закрытые регистры, PHY/MDIO, DMA-дескрипторы, errata | 2–3 тыс. | 2–4 мес., **высокий риск** |
| I226-V (igc) | ★★★★☆ | больше offload'ов и состояний | 3–4 тыс. | 2–3 мес. |

**Рекомендация (не для спецификации, а как факт):** в QEMU — `virtio-net` + `virtio-blk`; на железе — NVMe + один реальный NIC (Realtek, т.к. она почти наверняка стоит на плате).

---

## 5. Сетевой стек с нуля

### 5.1 Объём работ по слоям

| Слой | Что входит | Строк (референс) | Комментарий |
|---|---|---|---|
| Ethernet + ARP | кадры, ARP-кэш, retry | ~1–2 тыс. | просто |
| IPv4 | заголовки, checksum, фрагментация (можно не делать), маршрутизация | ~2–3 тыс. | просто |
| ICMP | echo reply (нужен для ping-проверки) | ~300 | тривиально |
| UDP | сокеты/датаграммы | ~1 тыс. | просто |
| **TCP** | state machine, окна, ретрансмиты, RTT, SACK (опц.), TIME_WAIT | **~8–12 тыс.** | **самая дорогая часть**, тонкая на баги |
| DHCP-клиент | DISCOVER/OFFER/REQUEST/ACK, lease | ~1 тыс. | просто |
| HTTP/1.1 сервер | парсинг запроса, keep-alive, chunked, статика, JSON-эндпоинты | ~1.5–3 тыс. | просто, но много крайних случаев |
| **Итого свой стек** | | **≈58 тыс. строк — это `smoltcp/src` `[изм.]`** (wire 22 540, socket 17 167, iface 12 142, phy 2 523, storage 1 976) | 9 лет разработки, зрелый |

**Вывод:** писать TCP самому — худшая инвестиция проекта. `smoltcp` 0.14.0 — `no_std`, `alloc`-опционален, поддерживает Ethernet/ARP/IPv4/IPv6/ICMP/UDP/TCP/DHCP, интегрируется с драйвером через трейт `phy::Device`. Объём работ: интеграция + HTTP-слой, не реализация TCP.

### 5.2 Порядок работ (реалистичный)

1. virtio-net в QEMU + ARP/ICMP → ping извне (1–2 нед.)
2. UDP + DHCP → получение адреса (1 нед.)
3. TCP (через smoltcp) + `nc`-подобная проверка (1 нед.)
4. HTTP/1.1: `GET /health`, `POST /v1/chat/completions` (2–3 нед.)
5. Реальный NIC на железе (см. §4) — параллельная ветка

### 5.3 TLS с нуля — насколько это отдельный проект

**Да, это отдельный большой проект.**

| Вариант | Объём | Труд | Комментарий |
|---|---|---|---|
| Свой TLS 1.3 | ~11 тыс. строк (уровень `embedded-tls` `[изм.]`) + свой crypto (X25519, ChaCha20-Poly1305/AES-GCM, SHA-256, HKDF, X.509-парсер) | 4–6 мес. | Плюс аудит; легко получить несовместимость с CDN |
| `rustls` 0.23 в no_std | 45.7 тыс. строк `[изм.]` — **берём, не пишем** | 1–2 мес. на порт (провайдер crypto, `getrandom`-источник = RDRAND/RDSEED или Limine Entropy, время, `std::io`-шим) | Реалистично: нужен только `std`-шим + entropy |
| Не делать TLS | 0 | 0 | HTTP в доверенной LAN + целостность через SHA-256 |

**Альтернативы TLS для скачивания модели (по возрастанию усилий):**
1. **Оффлайн-доставка**: веса заранее на NVMe (образ записывается с другой машины) — 0 строк кода, лучший вариант для MVP.
2. **HTTP в доверенной LAN** + **SHA-256 файла, зашитый в образ/подписанный манифест** — защита от повреждения и подмены «в пределах LAN».
3. **Подпись манифеста Ed25519** (публичный ключ в образе) + скачивание по HTTP — криптографическая целостность без TLS-стека; ~500–1000 строк (Ed25519 + SHA-256).
4. Полный TLS (rustls).
5. «Скачиваем при первой загрузке по HTTPS» — как в исходном замысле — **самый дорогой** путь, потому что требует TLS + DNS + надёжного TCP + обработки обрывов + дискового кэша.

---

## 6. Файловые системы и где хранить веса

| Вариант | Плюсы | Минусы | Вердикт |
|---|---|---|---|
| **Своя минимальная ФС** (inode- или extent-based, read-only для системного раздела + append-only для логов) | Контроль, простота, ~2–4 тыс. строк | Нужны crash-consistency, fsck, инструменты на хосте | **Да, для MVP**: read-only системный образ (можно грузить как модуль Limine) + отдельный раздел под веса |
| **FAT32 через UEFI** | Ноль кода: `EFI_SIMPLE_FILE_SYSTEM` | **Доступен только пока живы Boot Services**; после `ExitBootServices` — недоступен; 4 ГБ лимит файла; медленный | Нет (кроме загрузки самого ядра/модулей, что уже делает загрузчик) |
| **FAT32 свой драйвер** | Универсально, есть чем записать образ с хоста | FAT не журналируемый, плохо для десятков ГБ, длинные имена/UTF-16 — боль | Только как «обменный» раздел |
| **EXT4** | Родная для Linux-инструментов | Journal, extents, htree, checksums, 64-бит, JBD2 — **это месяцы**; огромная спека | Нет для MVP |
| **Сырое блочное устройство под веса** (фиксированные extents + суперблок с хешами) | Тривиально, максимум скорости, легко проверить SHA-256 | Нет частичных обновлений | **Да** |

**Хранение весов (десятки–сотни ГБ):**

| Модель | Размер Q4 | Влезает в 192 ГБ RAM? | Скорость на Ryzen 9 (2 канала) `[оц. по модели bandwidth/model]` |
|---|---|---|---|
| 7B–8B | ~5 ГБ | да | 10–20 ток/с |
| 14B | ~9 ГБ | да | 6–10 ток/с |
| 32B | ~20 ГБ | да | 3–4 ток/с |
| 70B dense | ~40 ГБ | да | **~1.3 ток/с** (подтверждено моделью: 80 ГБ/с ÷ 40 ГБ × 0.65) |
| DeepSeek-V3/R1 **671B MoE** | **~377 ГБ** (Q4), ~25 ГБ «активных» на токен | **нет** (нужно 512+ ГБ) | с NVMe-стримингом **1–3.5 ток/с** |

Источник цифр по размерам/скоростям: [LLM Inference on CPUs: Sizing & Performance Guide](https://raw.githubusercontent.com/sentient-agi/sentient-enclaves-framework/71055f7cd482503a39c35fc85d2ec19ec873e1e2/docs/llm-cpu-inference-sizing-guide.md) (март 2026) и [unsloth/DeepSeek-V3-GGUF](https://huggingface.co/unsloth/DeepSeek-V3-GGUF).

**Практика для MVP:** веса — на отдельном NVMe-разделе как **набор extent'ов + манифест с SHA-256**; ядро маппит их `mmap`-подобно (page cache = прямое чтение с диска в страницы), без копирования в heap. Это же решение снимает вопрос «192 ГБ не хватает»: работает и для 32B (полностью в RAM), и для 671B (стриминг с NVMe, медленно, но работает).

---

## 7. Инструменты разработки и отладки; CI

### 7.1 Без ОС на железе

| Инструмент | Что даёт | Замечания |
|---|---|---|
| **QEMU 11.1.3** (2026-10-28) | `-machine q35`, `virtio-blk-pci`, `virtio-net-pci`, **`-device nvme`** (эмуляция NVMe есть), `-serial stdio`, `-display none`, `-s -S` для GDB | Основная среда 95% времени |
| **GDB stub** (`-s -S`) | пошаговая отладка ядра, breakpoints, `info registers`, чтение памяти | Работает и с Rust (`rust-gdb`), и с QEMU-монитором |
| **QEMU debugcon (порт 0xE9)** | `out 0xE9, al` → строка в лог хоста (`-debugcon file:debug.log -global isa-debugcon.iobase=0xE9`) | Дешевле serial, работает до инициализации UART |
| **Serial (COM1 0x3F8)** | универсальный лог, работает и в QEMU, и на железе (`console=ttyS0`-подобно) | На AM5 физического COM нет → нужен USB-UART на header (есть на многих платах) или переходник |
| **OVMF** | UEFI-прошивка для QEMU (`-drive if=pflash,format=raw,file=OVMF_CODE.fd`) | Обязательно тестировать UEFI-путь, а не только direct-kernel |
| **`cargo-binutils`/`objdump`/`nm`** | разбор символов, `addr2line` по стек-трейсу из QEMU | Обязательный инструмент: паника без символов = часы вслепую |
| **Физическая плата** | единственный способ поймать firmware-квирки (GOP, memmap, MTRR, SMM) | **Риск «залочить» машину**: держите JTAG/SPI-программатор (CH341A + клипса) и/или вторую плату; сначала грузите с USB, никогда не пишите в SPI-flash из своей ОС |

**Правило безопасности:** на реальной плате первым делом грузимся с **USB-носителя** (UEFI Boot Manager), ничего не пишем в NVRAM. Свою ОС не ставим в `BOOTX64.EFI` на внутренний диск до стабилизации; прошивку не трогаем.

### 7.2 CI: тестирование ядра в QEMU

| Уровень | Как | Стоимость |
|---|---|---|
| Unit-тесты `no_std` | кастомный test framework (`#![feature(custom_test_frameworks)]`, `#![test_runner]`), `exit_qemu()` через `isa-debug-exit`, вывод в serial — классика из [blog_os: Testing](https://os.phil-opp.com/testing/) | 1–2 дня настройки |
| Интеграционные | запуск QEMU с образом, ожидание строки в serial-логе, `-device isa-debug-exit` возвращает код → `assert` в скрипте | 2–3 дня |
| Smoke на UEFI | QEMU + OVMF + `BOOTX64.EFI` на FAT-образе (`mtools`) | 1 день |
| CI-раннер | GitHub Actions `ubuntu-latest`: `apt-get install qemu-system-x86 ovmf mtools`; кэш `~/.cargo`, `sccache` | дёшево |
| Проверка на «железе» | self-hosted runner на реальной AM5-машине + USB-реле/сетевой KVM (PiKVM) для ребута | дорого, но необходимо на поздних стадиях |

Замечание: тесты ядра идут медленно (секунды на запуск QEMU) — держите их отдельным job'ом от `cargo check`/`clippy`.

---

## 8. Практические оценки трудозатрат

### 8.1 Якоря (реальные проекты)

| Проект | Объём | Время | Команда | Что достигнуто |
|---|---|---|---|---|
| **seL4** (микроядро, без драйверов) | **8 700 строк C + 600 asm**, доказательство 200 тыс. строк Isabelle | **≈25 человеко-лет** (включая верификацию); «если бы заново — <10 чел.-лет» | UNSW/NICTA | Формально верифицированное микроядро. **Ключевая цифра: даже 9 тыс. строк «правильного» ядра — это годы** ([источник](https://www.trustworthy.systems/projects/OLD/l4.verified/numbers)) |
| **SerenityOS** | `Kernel/` = 150 953 строки `[изм.]`, всё дерево ~1.4M LOC | с 2018-12, ~8 лет | 1 лидер full-time + сотни контрибьюторов | Полноценная десктоп-ОС с браузером |
| **Redox** | kernel 43 133 + драйверы 44 040 `[изм.]` | с 2015-04, **10 лет**, 0.9.0 в 2025-09 | сообщество | Boot с NVMe, USB HID, сеть, ZFS-подобная ФС |
| **Asterinas** | ~284 тыс. LOC оц. `[изм.]` | с 2022-05, ~3.5 года | ~10–15 (академия + Ant Group) | >210 Linux syscall'ов, ATC'25 |
| **Theseus** | ~117 тыс. LOC оц. `[изм.]` | 2017–2024, остановлен | research | — |
| **blog_os** (Ph. Oppermann) | учебное ядро, ~1.5 тыс. строк | 15 статей | 1 человек | boot → heap → async executor; **без SMP, без драйверов, без ФС** |

### 8.2 Модель оценки (принятые допущения)

- Производительность: **800–1 300 отлаженных строк/мес.** на низкоуровневый код с железом (ядро/драйверы), 1 500–2 500 — на протокольный/прикладной.
- Оценки — «до работающего демо», без полировки, документации, безопасности, тестов на 100% покрытие.
- Не входит: UEFI Secure Boot подпись, обновления, телеметрия, GUI.

### 8.3 Оценка по подсистемам (человеко-месяцы, 1 опытный разработчик)

| Подсистема | PM (мин–макс) | Комментарий |
|---|---|---|
| Загрузка (Limine) + ранняя консоль + panic + heap | 1.5–2.5 | |
| Свой UEFI-загрузчик (если нужен) | +1.5–3 | не блокирует MVP |
| Память: frame allocator, paging, MMIO-маппинги | 2–3.5 | |
| ACPI (MADT/MCFG/FADT) + PCIe ECAM | 1.5–2.5 | |
| APIC/x2APIC/IOAPIC/MSI + таймеры | 1.5–2.5 | |
| SMP + per-CPU + IPI + TLB shootdown | 1–1.5 (Limine) / 4–6 (сами) | **главный выигрыш от Limine** |
| Планировщик + контекст-свитч + XSAVE/AVX-512 | 1.5–2.5 | |
| SYSCALL/SYSRET + userspace + ELF + минимальный POSIX | 4–6 | |
| NVMe + своя ФС + манифест весов | 3–5 | |
| virtio-net + smoltcp + DHCP + IP-стек | 2.5–4 | |
| HTTP/1.1-сервер + API | 1.5–3 | |
| Реальный NIC (RTL8125/8126) | 3–8 | **самый неопределённый пункт** |
| GOP-консоль/простая графика | 0.5–1 | |
| xHCI + клавиатура | 6–10 | **вне MVP** |
| TLS (rustls-порт) | 1.5–3 | |
| TLS с нуля | 12–18 | не рекомендую |
| Порт инференс-движка (ggml/llama.cpp → своя ОС: libc-шим, pthread, mmap, время, SIMD-диспатч) | 8–15 | **недооценивается почти всегда**; llama.cpp ≈ 45 МБ исходников, ~1.2M LOC оц. `[изм.]` |

**Итого:**
- **MVP-демо** (Limine + SMP + NVMe + своя ФС + virtio-net + smoltcp + HTTP + GGUF 7–32B через портированный ggml): **≈30–45 PM** → **4–6 инженеров ≈ 9–12 месяцев**, один человек ≈ 2.5–4 года.
- **MVP + LAN-скачивание модели с подписью** (Ed25519+SHA-256, без TLS): +3–5 PM.
- **Полный продукт** (Secure Boot, TLS, реальные NIC, xHCI, обновления, устойчивость): **60–90 PM**.

### 8.4 Отдельно: стоимость инференса (продуктовый риск, не инженерный)

- decode ограничен **пропускной способностью памяти**, не ядрами: `ток/с ≈ BW / размер_модели × КПД(≈0.65)`.
- 2-канальный DDR5 на AM5: **~80–100 ГБ/с** — это «ноутбучный» уровень для LLM; 8-канальный EPYC даёт ~310 ГБ/с (в 3–4 раза быстрее).
- 2 CCD Ryzen 9: для decode лучше удерживать потоки в пределах CCD; для MoE-модели с малыми активными матрицами синхронизация дороже вычислений.
- **Следствие для позиционирования:** «ОС для LLM» на Ryzen 9 — это либо маленькие модели (7–32B, полностью в RAM, 3–20 ток/с), либо оффлоад 671B на NVMe (1–3.5 ток/с, «работает, но не интерактивно»). Если нужен интерактивный DeepSeek-V3/R1 — нужно железо с ≥512 ГБ памяти и 4+ каналами, либо GPU.

---

## 9. Что именно я измерил (воспроизводимо)

```bash
# размеры референсных драйверов
curl -s https://raw.githubusercontent.com/torvalds/linux/master/drivers/nvme/host/pci.c | wc -l      # 4587
curl -s https://raw.githubusercontent.com/torvalds/linux/master/drivers/net/ethernet/realtek/r8169_main.c | wc -l  # 7080
curl -s https://raw.githubusercontent.com/torvalds/linux/master/drivers/net/ethernet/intel/igc/igc_main.c   | wc -l  # 7875
curl -s https://raw.githubusercontent.com/freebsd/freebsd-src/main/sys/dev/rge/if_rge.c | wc -l       # 2787
# xHCI: сумма xhci.c+xhci-ring.c+xhci-mem.c+xhci-dbgcap.c+xhci-dbgtty.c+xhci-sideband.c = 15608
# USB core: hcd.c+hub.c+message.c+urb.c = 13341

# Redox-драйверы (git clone --depth 1 redox-os/drivers)
#   storage/nvmed 1318 | usb/xhcid 8955 | net/rtl8168d 462 | net/e1000d 459 | net/virtio-netd 254

# SerenityOS kernel (sparse checkout Kernel)
#   Kernel/**/*.{cpp,h} = 150953

# Redox kernel (git clone --depth 1 redox-os/kernel)
#   src/**/*.rs = 43133

# smoltcp (git clone --depth 1 smoltcp-rs/smoltcp)
#   src/**/*.rs = 58200  (wire 22540, socket 17167, iface 12142, phy 2523, storage 1976)

# rustls 45694 | embedded-tls 11115 | llama.cpp ≈1.2M LOC оц. (45.4 МБ по GitHub languages API)
```

Версии крейтов — через `https://crates.io/api/v1/crates/<name>`; версии Rust/QEMU/Limine/Zig — через соответствующие API/сайты; состояния репозиториев — через GitHub API.

## 10. Что НЕ удалось верифицировать (честные пробелы)

1. **OSDev Wiki** (`wiki.osdev.org/NVMe`, `/XHCI`, `/Symmetric_Multiprocessing`, `/APIC`) отдаёт **403** (Cloudflare) — на эти страницы я ссылаюсь как на канонический справочник, но их текст в этой сессии не читал.
2. **AMD64 APM (Vol.1–4, док. 24592–24595) и AMD PPR для Family 1Ah** — PDF на amd.com отдают заглушку вместо файла; конкретные MSR-номера/биты я привожу по общим знаниям, **перед спецификацией надо сверить с актуальной ревизией APM** и PPR (`https://www.amd.com/en/search/documentation/hub.html`).
3. **UEFI 2.11 / ACPI 6.6 / xHCI 1.2c** — сайты uefi.org и intel.com отдают 403 бот-защиту; ссылки валидны, содержимое не перечитывал (xHCI r1.2c подтверждён по индексу Intel: [страница r1.2c](https://www.intel.com/content/www/us/en/content-details/868295/extensible-host-controller-interface-for-universal-serial-bus-xhci-requirements-specification-r1-2c.html)).
4. **Наличие CSM именно на вашей AM5-плате** — надо смотреть BIOS конкретной модели; вывод «UEFI-only» — платформенный тренд, не гарантия для каждой платы.
5. **Точный состав LAN-чипов** — проверено на 2 платах (RTL8126 на Gigabyte X870E, обзор ASUS X870E); модельный ряд в целом надо сверять по спецификации выбранной платы.
6. Оценки трудозатрат в §8 — **моя модель**, а не измерение; якоря (§8.1) — факты, множители — допущения.

---

## Выводы для спецификации

1. **Зафиксировать стек: Rust + Limine + `x86_64`/`uefi`/`acpi`/`smoltcp`/`rustls`.** Limine берём именно из-за `LIMINE_MP_REQUEST` (AP bring-up + x2APIC), `HHDM`, `5LVL`, `ENTROPY`, `TSC_FREQUENCY` — это недели работы, отданные бесплатно. Свой загрузчик — отдельная поздняя задача, не риск для MVP.
2. **Не писать TCP/IP и TLS.** TCP/IP — `smoltcp` 0.14 (свой TCP ≈ 8–12 тыс. строк и годы багов; весь smoltcp = 58 тыс. строк). TLS — либо `rustls` в no_std (1.5–3 PM на порт), либо **не делать вовсе** (см. п. 3).
3. **Модель доставлять оффлайн (или по LAN), целостность — SHA-256 + подпись Ed25519 манифеста.** «Скачивание LLM по HTTPS при первой загрузке» — самый дорогой из возможных путей (TLS + DNS + устойчивый TCP + дисковый кэш + обработка обрывов) и не должен быть в MVP.
4. **Целевая модель MVP — дистиллят DeepSeek 7B–32B в Q4_K_M (5–20 ГБ).** DeepSeek-V3/R1 671B (Q4 ≈ 377 ГБ) не влезает в 192 ГБ и даёт 1–3.5 ток/с с NVMe-стримингом. Это надо написать в спецификации прямым текстом, иначе проект упрётся в физику памяти, а не в код.
5. **Веса — на отдельном NVMe-разделе как extent'ы + манифест с хешами; чтение напрямую в страницы (mmap-подобно).** Это одновременно решает вопрос ФС (своя минимальная read-only ФС для системы + сырой раздел для весов) и вопрос «не хватает RAM» для больших моделей.
6. **Драйверный минимум MVP: NVMe + virtio-net (QEMU) + один реальный NIC + GOP-framebuffer + serial.** xHCI — **вынести за MVP** (самый дорогой: 15.6 тыс. строк в Linux, 9 тыс. у Redox). Realtek RTL8125/8126 — признать риск: донор `if_rge.c` (2 787 строк, BSD) вместо `r8169` (7 080).
7. **Зафиксировать «железную» цель: одна конкретная плата AM5 + один конкретный NIC + один NVMe.** Поддержка «любого AM5» — это множитель ×5 к драйверной работе. В спеке — таблица «поддерживаемое железо (whitelist)».
8. **Secure Boot: выключить на dev-стенде, свои ключи (`sbctl`) — на демо-железе.** Не планировать подпись у Microsoft. Учесть, что CA 2011 истекает в июне 2026 и прошивки обновляют KEK/dbx — зафиксировать версию BIOS.
9. **Бюджет: 30–45 PM до работающего демо** (4–6 инженеров × 9–12 мес.). Самые недооцениваемые статьи: порт ggml/llama.cpp (8–15 PM), реальный NIC (3–8 PM), TLS (1.5–3 PM даже с rustls). Ядро и SMP — не самое дорогое.
10. **Инженерный процесс сразу:** QEMU+OVMF как основная среда, GDB stub, `debugcon` 0xE9 + serial, CI на GitHub Actions с `isa-debug-exit`, SPI-программатор и вторая плата для железа, запрет записи в NVRAM/прошивку из своей ОС.
