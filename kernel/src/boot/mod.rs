//! Сбор информации от загрузчика и её печать.
//!
//! `BootInfo` — та самая структура из `docs/ARCHITECTURE.md` §3 (в M0/T2 —
//! минимальный набор: карта памяти, HHDM, framebuffer, RSDP, TSC, число ядер).
//! Поля, которые заполняет не Limine, а наши подсистемы (paging_mode,
//! entropy, modules), появятся в T4/T6.

pub mod limine;

use limine::MemmapEntry;

use crate::println;

/// Всё, что ядро узнало от загрузчика на входе.
pub struct BootInfo {
    /// Смещение Higher Half Direct Map (all physical memory → VA).
    pub hhdm_base: Option<u64>,
    /// Основной framebuffer (первый из списка загрузчика).
    pub framebuffer: Option<&'static limine::Framebuffer>,
    /// Карта памяти: каждая запись — физический диапазон и его тип.
    pub memmap: &'static [&'static MemmapEntry],
    /// Физический адрес RSDP (корень ACPI).
    pub rsdp: Option<u64>,
    /// Частота TSC в герцах.
    pub tsc_hz: Option<u64>,
    /// Сколько ядер видит загрузчик (включая BSP).
    pub cpu_count: u64,
    /// LAPIC ID загрузочного процессора.
    pub bsp_lapic_id: u32,
}

/// Собрать `BootInfo` из ответов Limine.
pub fn collect() -> BootInfo {
    let memmap = match limine::memmap() {
        Some(response) => limine::memmap_entries(response),
        None => &[],
    };

    BootInfo {
        hhdm_base: limine::hhdm().map(|response| response.offset),
        framebuffer: limine::primary_framebuffer(),
        memmap,
        rsdp: limine::rsdp().map(|response| response.address as u64),
        tsc_hz: limine::tsc_frequency().map(|response| response.frequency),
        cpu_count: limine::mp().map_or(0, |response| response.cpu_count),
        bsp_lapic_id: limine::mp().map_or(0, |response| response.bsp_lapic_id),
    }
}

/// Напечатать сводку загрузки во все каналы консоли.
pub fn report(info: &BootInfo) {
    println!("--- boot info ---");

    match info.hhdm_base {
        Some(base) => println!("HHDM base: 0x{:016x}", base),
        None => println!("HHDM base: <загрузчик не ответил>"),
    }

    match info.rsdp {
        Some(address) => println!("RSDP: 0x{:016x}", address),
        None => println!("RSDP: <не найден>"),
    }

    println!(
        "CPUs: {} (BSP LAPIC id {})",
        info.cpu_count, info.bsp_lapic_id
    );

    match info.tsc_hz {
        Some(hz) => println!("TSC: {} MHz ({} Hz)", hz / 1_000_000, hz),
        None => println!("TSC: <частота неизвестна>"),
    }

    if let Some(framebuffer) = info.framebuffer {
        println!(
            "framebuffer: {}x{}, {} bpp, pitch {}, rgb masks {}/{}/{}",
            framebuffer.width,
            framebuffer.height,
            framebuffer.bpp,
            framebuffer.pitch,
            framebuffer.red_mask_size,
            framebuffer.green_mask_size,
            framebuffer.blue_mask_size,
        );
    } else {
        println!("framebuffer: <нет>");
    }

    report_memmap(info.memmap);
}

fn report_memmap(entries: &[&MemmapEntry]) {
    println!("--- memory map ---");
    println!("entries: {}", entries.len());
    if entries.is_empty() {
        println!("<карта памяти пуста: загрузчик не ответил на запрос>");
        return;
    }

    let mut usable_bytes: u64 = 0;
    let mut kernel_bytes: u64 = 0;
    let mut reclaimable_bytes: u64 = 0;
    let mut framebuffer_bytes: u64 = 0;
    let mut reserved_bytes: u64 = 0;

    for entry in entries {
        match entry.kind {
            limine::MEMMAP_USABLE => usable_bytes += entry.length,
            limine::MEMMAP_EXECUTABLE_AND_MODULES => kernel_bytes += entry.length,
            limine::MEMMAP_BOOTLOADER_RECLAIMABLE | limine::MEMMAP_ACPI_RECLAIMABLE => {
                reclaimable_bytes += entry.length
            }
            limine::MEMMAP_FRAMEBUFFER => framebuffer_bytes += entry.length,
            _ => reserved_bytes += entry.length,
        }
    }

    println!(
        "summary: usable {} MiB, kernel+modules {} MiB, reclaimable {} MiB, framebuffer {} MiB, reserved/other {} MiB",
        mib(usable_bytes),
        mib(kernel_bytes),
        mib(reclaimable_bytes),
        mib(framebuffer_bytes),
        mib(reserved_bytes),
    );

    for (index, entry) in entries.iter().enumerate() {
        let end = entry.base.saturating_add(entry.length);
        println!(
            "  [{:>2}] {:<22} 0x{:016x}-0x{:016x} {:>10} KiB",
            index,
            limine::memmap_type_name(entry.kind),
            entry.base,
            end,
            entry.length / 1024,
        );
    }
}

fn mib(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}
