//! Host-тесты `libs/bootinfo`: типы, арифметика, валидация, сводка.
//!
//! Тесты гоняются обычным `cargo test` без QEMU: крейт arch-нейтрален и не
//! трогает железо, поэтому карту памяти можно проверять на синтетических данных.

use crate::*;

fn sample() -> BootInfo {
    let mut info = BootInfo::new();
    info.push_memmap(MemmapEntry::new(
        0x0000_0000,
        0x0009_fc00,
        MemmapKind::Usable,
    ));
    info.push_memmap(MemmapEntry::new(
        0x0009_fc00,
        0x0000_0400,
        MemmapKind::Reserved,
    ));
    info.push_memmap(MemmapEntry::new(
        0x0010_0000,
        0x07ee_0000,
        MemmapKind::Usable,
    ));
    info.push_memmap(MemmapEntry::new(
        0x07fe_0000,
        0x0002_0000,
        MemmapKind::Reserved,
    ));
    info.hhdm = Some(0xffff_8000_0000_0000);
    info.paging_mode = PagingMode::FourLevel;
    info.framebuffer = Some(Framebuffer {
        addr: 0xfd00_0000,
        width: 1920,
        height: 1080,
        pitch: 1920 * 4,
        bpp: 32,
        memory_model: MEMORY_MODEL_RGB,
        red: Channel::new(8, 16),
        green: Channel::new(8, 8),
        blue: Channel::new(8, 0),
    });
    info.rsdp = Some(0x07fe_0000);
    info.tsc_hz = Some(2_400_000_000);
    info.cpu_count = 1;
    info.bsp_lapic_id = 0;
    info.entropy = Some([0x5a; ENTROPY_BYTES]);
    info.entropy_source = EntropySource::Bootloader;
    info.push_module(Module::new(0x0800_0000, 0x0002_0000, "/boot/initrd.img"));
    info
}

// --- MemmapKind -----------------------------------------------------------

#[test]
fn memmap_kind_round_trips_through_raw() {
    for raw in 0..=8u64 {
        assert_eq!(MemmapKind::from_raw(raw).raw(), raw);
    }
    assert_eq!(MemmapKind::from_raw(42), MemmapKind::Other(42));
    assert_eq!(MemmapKind::Other(42).raw(), 42);
    assert_eq!(MemmapKind::from_raw(42).name(), "other");
}

#[test]
fn memmap_kind_classification_matches_protocol() {
    assert!(MemmapKind::Usable.is_usable());
    assert!(!MemmapKind::Reserved.is_usable());
    assert!(MemmapKind::AcpiReclaimable.is_reclaimable());
    assert!(MemmapKind::BootloaderReclaimable.is_reclaimable());
    assert!(!MemmapKind::Usable.is_reclaimable());
    assert!(MemmapKind::ReservedMapped.is_reserved_like());
    assert!(MemmapKind::AcpiNvs.is_reserved_like());
    assert!(!MemmapKind::Framebuffer.is_reserved_like());
    assert!(MemmapKind::Usable.is_disjoint_guaranteed());
    assert!(MemmapKind::BootloaderReclaimable.is_disjoint_guaranteed());
    assert!(!MemmapKind::Reserved.is_disjoint_guaranteed());
}

// --- MemmapEntry ----------------------------------------------------------

#[test]
fn entry_arithmetic_and_boundaries() {
    let entry = MemmapEntry::new(0x1000, 0x2000, MemmapKind::Usable);
    assert_eq!(entry.end(), 0x3000);
    assert_eq!(entry.end_exact(), Some(0x3000));
    assert!(entry.contains(0x1000));
    assert!(entry.contains(0x2fff));
    assert!(!entry.contains(0x3000));
    assert!(!entry.contains(0x0fff));
    assert!(!entry.is_empty());
    assert!(MemmapEntry::EMPTY.is_empty());
}

#[test]
fn entry_end_saturates_instead_of_overflowing() {
    let entry = MemmapEntry::new(u64::MAX - 1, 8, MemmapKind::Reserved);
    assert_eq!(entry.end(), u64::MAX);
    assert_eq!(entry.end_exact(), None);
    assert!(!entry.contains(0));
    assert!(!entry.overlaps(&entry));
    assert!(!entry.fits_phys_space());
}

#[test]
fn entry_overlap_is_half_open() {
    let a = MemmapEntry::new(0x1000, 0x1000, MemmapKind::Usable);
    let adjacent = MemmapEntry::new(0x2000, 0x1000, MemmapKind::Usable);
    let shared_byte = MemmapEntry::new(0x1fff, 0x2, MemmapKind::Reserved);
    let nested = MemmapEntry::new(0x1400, 0x10, MemmapKind::Reserved);
    assert!(!a.overlaps(&adjacent));
    assert!(a.overlaps(&shared_byte));
    assert!(a.overlaps(&nested));
    assert!(!a.overlaps(&MemmapEntry::EMPTY));
}

#[test]
fn entry_phys_space_limit_is_inclusive_at_end() {
    let inside = MemmapEntry::new(PHYS_ADDR_LIMIT - PAGE_SIZE, PAGE_SIZE, MemmapKind::Usable);
    assert!(inside.fits_phys_space());
    let beyond = MemmapEntry::new(PHYS_ADDR_LIMIT - 1, 2, MemmapKind::Usable);
    assert!(!beyond.fits_phys_space());
    let at_limit = MemmapEntry::new(PHYS_ADDR_LIMIT, 1, MemmapKind::Reserved);
    assert!(!at_limit.fits_phys_space());
}

// --- MemmapSummary --------------------------------------------------------

#[test]
fn summary_counts_bytes_per_kind() {
    let summary = sample().summary();
    assert_eq!(summary.entries, 4);
    assert_eq!(summary.usable, 0x0009_fc00 + 0x07ee_0000);
    assert_eq!(summary.reserved, 0x0000_0400 + 0x0002_0000);
    assert_eq!(summary.usable_mib(), summary.usable / MIB);
    assert!(!summary.overflowed);
    assert_eq!(summary.total_mapped(), summary.usable + summary.reserved);
}

#[test]
fn summary_saturates_and_flags_overflow() {
    let mut summary = MemmapSummary::default();
    summary.add(&MemmapEntry::new(0, u64::MAX / 2, MemmapKind::Usable));
    summary.add(&MemmapEntry::new(
        u64::MAX / 2,
        u64::MAX / 2,
        MemmapKind::Usable,
    ));
    assert_eq!(summary.usable, u64::MAX - 1);
    assert!(!summary.overflowed);
    summary.add(&MemmapEntry::new(0, 8, MemmapKind::Usable));
    assert_eq!(summary.usable, u64::MAX);
    assert!(summary.overflowed);
    let _ = summary.usable_mib();
    let _ = summary.total_mapped();
}

// --- Заполнение фиксированных массивов ------------------------------------

#[test]
fn push_marks_truncation_when_capacity_is_exhausted() {
    let mut info = BootInfo::new();
    for index in 0..MAX_MEMMAP {
        let base = index as u64 * PAGE_SIZE;
        assert!(info.push_memmap(MemmapEntry::new(base, PAGE_SIZE, MemmapKind::Usable)));
    }
    assert_eq!(info.memmap().len(), MAX_MEMMAP);
    assert_eq!(info.memmap_len as usize, MAX_MEMMAP);
    assert!(!info.memmap_truncated);
    assert!(!info.push_memmap(MemmapEntry::new(
        MAX_MEMMAP as u64 * PAGE_SIZE,
        PAGE_SIZE,
        MemmapKind::Usable
    )));
    assert!(info.memmap_truncated);
    assert_eq!(info.validate(), Err(BootInfoError::MemmapTruncated));
}

#[test]
fn memmap_len_above_capacity_is_rejected_and_slice_is_clamped() {
    let mut info = sample();
    info.memmap_len = MAX_MEMMAP as u16 + 72;
    assert_eq!(info.memmap().len(), MAX_MEMMAP);
    assert_eq!(info.validate(), Err(BootInfoError::MemmapTruncated));
}

#[test]
fn modules_truncated_is_rejected() {
    let mut info = sample();
    info.modules_truncated = true;
    assert_eq!(info.validate(), Err(BootInfoError::ModulesTruncated));
    let mut info = sample();
    info.modules_len = MAX_MODULES as u8 + 1;
    assert_eq!(info.modules().len(), MAX_MODULES);
    assert_eq!(info.validate(), Err(BootInfoError::ModulesTruncated));
}

#[test]
fn cpu_records_push_and_slice() {
    let mut info = BootInfo::new();
    assert!(info.cpus().is_empty());
    for index in 0..MAX_CPUS {
        assert!(info.push_cpu(CpuInfo::new(index as u32, 100 + index as u32, index as u32)));
    }
    assert_eq!(info.cpus().len(), MAX_CPUS);
    assert_eq!(info.cpus()[2].cpu_id, 2);
    assert_eq!(info.cpus()[2].processor_id, 102);
    assert_eq!(info.cpus()[2].lapic_id, 2);
    assert!(!info.push_cpu(CpuInfo::new(99, 99, 99)));
    assert_eq!(info.cpus().len(), MAX_CPUS);
}

#[test]
fn cpu_count_above_capacity_is_rejected() {
    let mut info = sample();
    info.cpus_len = MAX_CPUS as u8 + 1;
    assert_eq!(info.cpus().len(), MAX_CPUS);
    assert_eq!(info.validate(), Err(BootInfoError::CpusTruncated));
}

// --- Валидация ------------------------------------------------------------

#[test]
fn validate_accepts_qemu_like_map() {
    assert_eq!(sample().validate(), Ok(()));
}

#[test]
fn validate_rejects_empty_map() {
    let info = BootInfo::new();
    assert_eq!(info.validate(), Err(BootInfoError::EmptyMemmap));
    assert!(!info.hhdm_available());
}

#[test]
fn validate_rejects_no_usable_memory() {
    let mut info = BootInfo::new();
    info.push_memmap(MemmapEntry::new(0, PAGE_SIZE, MemmapKind::Reserved));
    assert_eq!(info.validate(), Err(BootInfoError::NoUsableMemory));
}

#[test]
fn validate_rejects_zero_length_range() {
    let mut info = sample();
    info.memmap[1] = MemmapEntry::new(0x10_0000, 0, MemmapKind::Reserved);
    assert_eq!(
        info.validate(),
        Err(BootInfoError::ZeroLengthRange { index: 1 })
    );
}

#[test]
fn validate_rejects_range_outside_physical_space() {
    let mut info = sample();
    info.memmap[0] = MemmapEntry::new(PHYS_ADDR_LIMIT, PAGE_SIZE, MemmapKind::Usable);
    assert_eq!(
        info.validate(),
        Err(BootInfoError::RangeOutOfAddressSpace { index: 0 })
    );
}

#[test]
fn validate_rejects_overlap_with_usable() {
    let mut info = sample();
    info.memmap[1] = MemmapEntry::new(0x1000, 0x2000, MemmapKind::Reserved);
    assert_eq!(
        info.validate(),
        Err(BootInfoError::OverlappingRanges {
            first: 0,
            second: 1
        })
    );
}

#[test]
fn validate_allows_overlap_between_non_disjoint_kinds_but_reports_it() {
    let mut info = sample();
    // Framebuffer и reserved-mapped могут пересекаться (протокол это не запрещает).
    info.push_memmap(MemmapEntry::new(
        0x07fe_8000,
        0x1000,
        MemmapKind::Framebuffer,
    ));
    info.push_memmap(MemmapEntry::new(
        0x07fe_9000,
        0x1000,
        MemmapKind::ReservedMapped,
    ));
    assert_eq!(info.overlaps(), Some((3, 4)));
    assert_eq!(info.validate(), Ok(()));
}

#[test]
fn validate_rejects_misaligned_hhdm() {
    let mut info = sample();
    info.hhdm = Some(0xffff_8000_0000_0001);
    assert_eq!(
        info.validate(),
        Err(BootInfoError::HhdmMisaligned(0xffff_8000_0000_0001))
    );
    info.hhdm = Some(0);
    assert_eq!(info.validate(), Err(BootInfoError::HhdmMisaligned(0)));
    info.hhdm = None;
    assert_eq!(info.validate(), Ok(()));
}

#[test]
fn validate_rejects_broken_framebuffer() {
    let mut info = sample();
    info.framebuffer.as_mut().unwrap().width = 0;
    assert_eq!(info.validate(), Err(BootInfoError::FramebufferZeroSize));

    let mut info = sample();
    info.framebuffer.as_mut().unwrap().pitch = 1920 * 3;
    assert_eq!(
        info.validate(),
        Err(BootInfoError::FramebufferBadPitch {
            pitch: 1920 * 3,
            min_pitch: 1920 * 4
        })
    );

    let mut info = sample();
    info.framebuffer.as_mut().unwrap().bpp = 12;
    assert_eq!(info.validate(), Err(BootInfoError::FramebufferBadBpp(12)));

    let mut info = sample();
    info.framebuffer = None;
    assert_eq!(info.validate(), Ok(()));
}

#[test]
fn validate_rejects_zero_rsdp_and_tsc() {
    let mut info = sample();
    info.rsdp = Some(0);
    assert_eq!(info.validate(), Err(BootInfoError::InvalidRsdp));

    let mut info = sample();
    info.tsc_hz = Some(0);
    assert_eq!(info.validate(), Err(BootInfoError::TscZero));
}

#[test]
fn validate_rejects_entropy_source_without_entropy() {
    let mut info = sample();
    info.entropy = None;
    assert_eq!(info.validate(), Err(BootInfoError::MissingEntropy));
    info.entropy_source = EntropySource::None;
    assert_eq!(info.validate(), Ok(()));
}

#[test]
fn validate_rejects_broken_modules() {
    let mut info = sample();
    info.modules[0].addr = 0;
    assert_eq!(
        info.validate(),
        Err(BootInfoError::ModuleZeroAddress { index: 0 })
    );

    let mut info = sample();
    info.modules[0].size = 0;
    assert_eq!(
        info.validate(),
        Err(BootInfoError::ModuleZeroSize { index: 0 })
    );

    let mut info = sample();
    assert!(info.push_module(Module::new(0x0801_0000, 0x1000, "/boot/overlap")));
    assert_eq!(
        info.validate(),
        Err(BootInfoError::OverlappingModules {
            first: 0,
            second: 1
        })
    );
}

// --- Хелперы --------------------------------------------------------------

#[test]
fn largest_usable_finds_biggest_and_ignores_reserved() {
    let info = sample();
    let largest = info.largest_usable().expect("usable есть");
    assert_eq!(largest.base, 0x10_0000);
    assert_eq!(largest.length, 0x07ee_0000);

    let mut without_usable = BootInfo::new();
    without_usable.push_memmap(MemmapEntry::new(0, PAGE_SIZE, MemmapKind::Reserved));
    assert!(without_usable.largest_usable().is_none());
    assert_eq!(without_usable.usable_bytes(), 0);
}

#[test]
fn rsdp_entry_and_kind_are_looked_up_in_memmap() {
    let info = sample();
    assert_eq!(info.rsdp_kind(), Some(MemmapKind::Reserved));
    let mut info = sample();
    info.rsdp = Some(0xffff_ffff_ffff_0000);
    assert!(info.rsdp_entry().is_none());
    assert_eq!(info.rsdp_kind(), None);
}

#[test]
fn mp_fields_are_reported() {
    let info = sample();
    assert!(info.mp_available());
    assert_eq!(info.cpu_count, 1);
    let empty = BootInfo::new();
    assert!(!empty.mp_available());
    assert_eq!(empty.cpu_count, 0);
}

#[test]
fn paging_mode_and_entropy_source_are_readable() {
    assert_eq!(PagingMode::from_raw(0), PagingMode::FourLevel);
    assert_eq!(PagingMode::from_raw(1), PagingMode::FiveLevel);
    assert_eq!(PagingMode::from_raw(9), PagingMode::Unknown);
    assert_eq!(PagingMode::FourLevel.levels(), Some(4));
    assert_eq!(PagingMode::FiveLevel.levels(), Some(5));
    assert_eq!(PagingMode::Unknown.levels(), None);
    assert_eq!(PagingMode::FourLevel.name(), "4-level");

    assert!(EntropySource::Bootloader.is_strong());
    assert!(EntropySource::Rdrand.is_strong());
    assert!(!EntropySource::TscJitter.is_strong());
    assert!(!EntropySource::None.is_strong());
    assert_eq!(EntropySource::TscJitter.name(), "tsc-jitter");
}

#[test]
fn framebuffer_geometry_helpers() {
    let fb = sample().framebuffer.unwrap();
    assert_eq!(fb.bytes_per_pixel(), 4);
    assert_eq!(fb.min_pitch(), 1920 * 4);
    assert_eq!(fb.frame_bytes(), 1920 * 4 * 1080);
    assert!(fb.is_direct_color());
    assert!(fb.is_writable_32bpp());
    assert_eq!(fb.pixel_offset(0, 0), Some(0));
    assert_eq!(fb.pixel_offset(1, 0), Some(4));
    assert_eq!(fb.pixel_offset(0, 1), Some(7680));
    assert_eq!(fb.pixel_offset(1920, 0), None);
    assert_eq!(fb.pixel_offset(0, 1080), None);

    let mut mono = fb;
    mono.bpp = 16;
    mono.pitch = 1920 * 2;
    assert_eq!(mono.bytes_per_pixel(), 2);
    assert!(!mono.is_writable_32bpp());
    assert_eq!(Channel::new(8, 16).mask(), 0x00ff_0000);
    assert_eq!(Channel::new(0, 0).mask(), 0);
}

#[test]
fn module_geometry_and_overlap() {
    let a = Module::new(0x1000, 0x1000, "/a");
    let b = Module::new(0x1fff, 0x10, "/b");
    let c = Module::new(0x2000, 0x1000, "/c");
    assert_eq!(a.end(), 0x2000);
    assert!(a.overlaps(&b));
    assert!(!a.overlaps(&c));
    assert!(!Module::EMPTY.overlaps(&a));
}

// --- Сводка ---------------------------------------------------------------

#[test]
fn summary_text_mentions_every_field() {
    let info = sample();
    let text = std::format!("{}", info.summary_display());
    for needle in [
        "memmap: 4 entries",
        "usable",
        "HHDM base: 0xffff800000000000",
        "paging: 4-level",
        "RSDP: 0x0000000007fe0000",
        "TSC: 2400 MHz (2400000000 Hz)",
        "entropy: bootloader",
        "CPUs: 1 (BSP LAPIC id 0)",
        "framebuffer: 1920x1080, 32 bpp, pitch 7680",
        "modules: 1",
        "/boot/initrd.img",
    ] {
        assert!(
            text.contains(needle),
            "сводка не содержит {needle:?}: {text}"
        );
    }
}

#[test]
fn summary_text_reports_absent_fields() {
    let info = BootInfo::new();
    let text = std::format!("{}", info.summary_display());
    assert!(text.contains("HHDM base: absent"));
    assert!(text.contains("RSDP: absent"));
    assert!(text.contains("TSC: absent"));
    assert!(text.contains("framebuffer: absent"));
}

#[test]
fn write_summary_fills_buffer_and_returns_length() {
    let info = sample();
    let mut buf = [0u8; 1024];
    let written = info.write_summary(&mut buf);
    assert!(written > 0);
    assert!(written < buf.len());
    let text = core::str::from_utf8(&buf[..written]).unwrap();
    assert!(text.contains("memmap:"));
    assert!(text.contains("framebuffer: 1920x1080"));
}

#[test]
fn write_summary_truncates_without_panic() {
    let info = sample();
    let mut tiny = [0u8; 8];
    let written = info.write_summary(&mut tiny);
    assert_eq!(written, 8);
    let mut empty: [u8; 0] = [];
    assert_eq!(info.write_summary(&mut empty), 0);
}
