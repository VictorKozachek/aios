//! Boot information: collection from Limine and printing.
//!
//! The types live in `libs/bootinfo` (task #011, T2a) and are host-tested;
//! this module only converts Limine protocol responses into them and prints
//! the result. Keeping the conversion here means the rest of the kernel never
//! sees a Limine structure (ARCHITECTURE.md §9: `kernel` knows the protocol
//! only in `arch`/`boot`, everything else gets `bootinfo::BootInfo`).

pub mod crash;
pub mod limine;

use bootinfo::{
    BootInfo, Channel, CpuInfo, EntropySource, Framebuffer, MAX_MODULES, MemmapEntry, MemmapKind,
    Module, PagingMode,
};

use crate::println;

/// Collect everything the bootloader reported into one `BootInfo`.
pub fn collect() -> BootInfo {
    let mut info = BootInfo::new();

    if let Some(response) = limine::memmap() {
        for entry in limine::memmap_entries(response) {
            info.push_memmap(MemmapEntry::new(
                entry.base,
                entry.length,
                MemmapKind::from_raw(entry.kind),
            ));
        }
    }

    // Zero offsets/addresses mean "not provided" for every optional field:
    // the protocol omits a response rather than sending zeroes, but a zero
    // would break validation and the summary, so filter it out here.
    info.hhdm = limine::hhdm()
        .map(|response| response.offset)
        .filter(|offset| *offset != 0);
    info.paging_mode = limine::paging_mode().map_or(PagingMode::Unknown, PagingMode::from_raw);
    info.framebuffer = limine::primary_framebuffer().map(convert_framebuffer);
    info.rsdp = limine::rsdp()
        .map(|response| response.address as u64)
        .filter(|address| *address != 0);
    info.tsc_hz = limine::tsc_frequency()
        .map(|response| response.frequency)
        .filter(|frequency| *frequency != 0);

    if let Some(entropy) = limine::entropy() {
        info.entropy = Some(entropy);
        info.entropy_source = EntropySource::Bootloader;
    }

    if let Some(mp) = limine::mp() {
        info.cpu_count = mp.cpu_count;
        info.bsp_lapic_id = mp.bsp_lapic_id;
    }
    if let Some(cpus) = limine::mp_cpus() {
        for (index, cpu) in cpus.iter().enumerate() {
            if !info.push_cpu(CpuInfo::new(index as u32, cpu.processor_id, cpu.lapic_id)) {
                break;
            }
        }
    }

    if let Some(modules) = limine::modules() {
        for module in modules {
            if info.modules_len as usize >= MAX_MODULES {
                info.modules_truncated = true;
                break;
            }
            // SAFETY: the protocol guarantees a NUL-terminated path string
            // that lives as long as the kernel (bootloader-reclaimable memory
            // is never freed).
            let path = unsafe { limine::cstr(module.path) };
            info.push_module(Module::new(module.address as u64, module.size, path));
        }
    }

    info
}

/// Convert a Limine framebuffer into the arch-neutral description.
fn convert_framebuffer(framebuffer: &limine::Framebuffer) -> Framebuffer {
    Framebuffer {
        addr: framebuffer.address as u64,
        width: framebuffer.width as u32,
        height: framebuffer.height as u32,
        pitch: framebuffer.pitch as u32,
        bpp: framebuffer.bpp,
        memory_model: framebuffer.memory_model,
        red: Channel::new(framebuffer.red_mask_size, framebuffer.red_mask_shift),
        green: Channel::new(framebuffer.green_mask_size, framebuffer.green_mask_shift),
        blue: Channel::new(framebuffer.blue_mask_size, framebuffer.blue_mask_shift),
    }
}

/// Print the boot summary and memory map to every console channel.
///
/// The same text goes to serial, debugcon and the framebuffer: the console
/// duplicates bytes, so the screen and `serial.log` match (T2 acceptance).
pub fn report(info: &BootInfo) {
    println!();
    println!("--- boot info ---");
    println!("{}", info.summary_display());

    println!();
    println!("--- memory map ---");
    for (index, entry) in info.memmap().iter().enumerate() {
        println!(
            "  [{:>2}] {:<22} 0x{:016x}-0x{:016x} {:>8} KiB",
            index,
            entry.kind.name(),
            entry.base,
            entry.end(),
            entry.length / 1024,
        );
    }
}
