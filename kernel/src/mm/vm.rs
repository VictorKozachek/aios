//! Kernel page tables: clone Limine's PML4, add our mappings, switch CR3.
//!
//! Limine hands the kernel over with a working 4-level page table set: the
//! kernel image is mapped at its higher-half virtual addresses, the HHDM maps
//! physical memory and the boot stack lives in the HHDM. We cannot discard
//! that: the CPU is executing from those mappings right now. So the kernel
//! allocates a fresh PML4, copies all 512 entries (which keeps every Limine
//! mapping alive), adds the heap/MMIO mappings and only then switches CR3.
//!
//! Cache policy: MMIO is mapped uncached (PWT+PCD), the heap is RW+NX. The
//! framebuffer is already mapped write-combined by Limine inside the HHDM; we
//! verify that the cloned tables still see it, and map it ourselves if not.

use core::arch::asm;

use bootinfo::BootInfo;
use mmcore::page_table::ENTRY_FLAGS_MASK;
use mmcore::{
    CacheMode, Hhdm, MmError, PAGE_SIZE, PageTableEntry, PageTableFlags, PageTableLevel, PageWalk,
    PhysAddr, VirtAddr,
};

use crate::mm::{heap, phys};

/// Kernel heap virtual window: canonical higher half, away from HHDM (index 256)
/// and the kernel image (index 511).
pub const HEAP_BASE: u64 = 0xffff_9000_0000_0000;

/// Heap size: 1 MiB, one contiguous order-8 buddy block.
pub const HEAP_PAGES: u64 = 256;

/// Physical address of the local APIC: the MMIO example mapping (uncached).
const MMIO_PHYS: u64 = 0xfee0_0000;

const PAT_MSR: u32 = 0x277;
/// PAT entry 4 becomes write-combined, matching [`CacheMode::WriteCombined`].
const PAT_ENTRY_WC: u64 = 0x01;

/// Build our own page tables and switch to them.
pub fn init(boot: &BootInfo) -> Result<(), MmError> {
    let hhdm = Hhdm::new(boot.hhdm.ok_or(MmError::InvalidEntry)?);

    // 1. New PML4 with every Limine entry copied: kernel image, HHDM and the
    //    current stack stay mapped, so the CPU keeps running after CR3.
    let limine_pml4 = PhysAddr::new(read_cr3() & !0xfff);
    let pml4 = alloc_zeroed_page(&hhdm)?;
    copy_page(&hhdm, limine_pml4, pml4);
    crate::println!(
        "mm: cloned Limine page tables: PML4 0x{:016x} -> 0x{:016x}",
        limine_pml4.as_u64(),
        pml4.as_u64(),
    );

    // 2. PAT: make entry 4 write-combined so `CacheMode::WriteCombined` really
    //    means WC. Limine uses the same table but different indices; changing
    //    entry 4 (WP) does not disturb its framebuffer mapping.
    let pat = read_msr(PAT_MSR);
    write_msr(PAT_MSR, (pat & !(0xff_u64 << 32)) | (PAT_ENTRY_WC << 32));

    // 3. Heap: one contiguous block from the buddy allocator, RW+NX.
    let heap_phys = phys::alloc_pages(HEAP_PAGES).ok_or(MmError::OutOfMemory)?;
    map_range(
        &hhdm,
        pml4,
        VirtAddr::new(HEAP_BASE),
        heap_phys,
        HEAP_PAGES,
        PageTableFlags::kernel_data(),
    )?;
    crate::println!(
        "mm: heap {} KiB at 0x{:016x} (phys 0x{:016x}) RW+NX",
        (HEAP_PAGES * PAGE_SIZE) / 1024,
        HEAP_BASE,
        heap_phys.as_u64(),
    );

    // 4. MMIO: the local APIC page mapped uncached (PWT+PCD).
    let mmio_phys = PhysAddr::new(MMIO_PHYS);
    let mmio_virt = hhdm
        .phys_to_virt(mmio_phys)
        .ok_or(MmError::AddressOverflow)?;
    map_range(
        &hhdm,
        pml4,
        mmio_virt,
        mmio_phys,
        1,
        PageTableFlags::kernel_data() | CacheMode::Uncached.flags(),
    )?;
    crate::println!(
        "mm: mmio 0x{:08x} at 0x{:016x} uncached",
        MMIO_PHYS,
        mmio_virt.as_u64(),
    );

    // 5. Framebuffer: Limine already mapped it write-combined in the HHDM.
    //    The cloned tables keep that mapping; verify it, map it ourselves if
    //    the clone somehow lost it.
    if let Some(framebuffer) = boot.framebuffer {
        let virt = VirtAddr::new(framebuffer.addr);
        match walk_flags(&hhdm, pml4, virt) {
            Some(flags) => {
                let mode = if flags.contains(PageTableFlags::HUGE_PAGE) {
                    "huge page, write-combined per Limine protocol"
                } else if flags.contains(PageTableFlags::PAT) {
                    "write-combined (PAT)"
                } else {
                    "inherited, write-combined per Limine protocol"
                };
                crate::println!(
                    "mm: framebuffer 0x{:016x} mapped: {}",
                    framebuffer.addr,
                    mode,
                );
            }
            None => {
                let phys = hhdm.virt_to_phys(virt).ok_or(MmError::AddressOverflow)?;
                let pages = framebuffer.frame_bytes().div_ceil(PAGE_SIZE);
                map_range(
                    &hhdm,
                    pml4,
                    virt,
                    phys,
                    pages,
                    PageTableFlags::kernel_data() | CacheMode::WriteCombined.flags(),
                )?;
                crate::println!(
                    "mm: framebuffer 0x{:016x} mapped write-combined ({} pages)",
                    framebuffer.addr,
                    pages,
                );
            }
        }
    } else {
        crate::println!("mm: framebuffer absent");
    }

    // 6. Switch. Everything the kernel needs is now in our own tables.
    write_cr3(pml4.as_u64());
    crate::println!(
        "mm: own page tables active (CR3=0x{:016x}), HHDM preserved",
        pml4.as_u64(),
    );

    // 7. Only now the heap mapping is live: touching it before the CR3 switch
    //    would fault, because the new tables are not active yet.
    heap::init(HEAP_BASE, HEAP_PAGES * PAGE_SIZE);
    Ok(())
}

// --- Page-table access -----------------------------------------------------

fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: CR3 is readable from ring 0; it only holds the PML4 address.
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn write_cr3(value: u64) {
    // SAFETY: the new PML4 is a valid, fully populated page-table root; the
    // write also flushes the TLB.
    unsafe {
        asm!("mov cr3, {}", in(reg) value, options(nostack));
    }
}

fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: privileged MSR read during kernel setup.
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | low as u64
}

fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    // SAFETY: privileged MSR write during kernel setup; the value is built
    // from the current MSR contents.
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nostack, preserves_flags),
        );
    }
}

/// Pointer to a physical page through the HHDM.
///
/// The buddy allocator only hands out usable memory, and the HHDM maps all of
/// it, so the translation cannot fail for our pages.
fn phys_ptr<T>(hhdm: &Hhdm, phys: PhysAddr) -> *mut T {
    let virt = hhdm
        .phys_to_virt(phys)
        .expect("HHDM must map usable physical memory");
    virt.as_u64() as *mut T
}

fn alloc_zeroed_page(hhdm: &Hhdm) -> Result<PhysAddr, MmError> {
    let page = phys::alloc_page().ok_or(MmError::OutOfMemory)?;
    // SAFETY: the page is ours until it is mapped or freed.
    unsafe {
        core::ptr::write_bytes(phys_ptr::<u8>(hhdm, page), 0, PAGE_SIZE as usize);
    }
    Ok(page)
}

fn copy_page(hhdm: &Hhdm, from: PhysAddr, to: PhysAddr) {
    // SAFETY: both pages are 4 KiB and owned by the kernel/Limine tables.
    unsafe {
        core::ptr::copy_nonoverlapping(
            phys_ptr::<u8>(hhdm, from),
            phys_ptr::<u8>(hhdm, to),
            PAGE_SIZE as usize,
        );
    }
}

fn entry_ptr(hhdm: &Hhdm, table: PhysAddr, index: u16) -> *mut u64 {
    // SAFETY: the index is 0..511 by construction (PageWalk/PageTableLevel).
    unsafe { phys_ptr::<u64>(hhdm, table).add(index as usize) }
}

fn read_entry(hhdm: &Hhdm, table: PhysAddr, index: u16) -> u64 {
    // SAFETY: see `entry_ptr`.
    unsafe { *entry_ptr(hhdm, table, index) }
}

fn write_entry(hhdm: &Hhdm, table: PhysAddr, index: u16, value: u64) {
    // SAFETY: see `entry_ptr`.
    unsafe {
        *entry_ptr(hhdm, table, index) = value;
    }
}

fn intermediate_flags() -> PageTableFlags {
    PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NX
}

/// Map `pages` 4 KiB pages starting at `virt` to `phys` with `flags`.
fn map_range(
    hhdm: &Hhdm,
    pml4: PhysAddr,
    virt: VirtAddr,
    phys: PhysAddr,
    pages: u64,
    flags: PageTableFlags,
) -> Result<(), MmError> {
    for page in 0..pages {
        let delta = page * PAGE_SIZE;
        let page_virt = VirtAddr::new(
            virt.as_u64()
                .checked_add(delta)
                .ok_or(MmError::AddressOverflow)?,
        );
        let page_phys = PhysAddr::new(
            phys.as_u64()
                .checked_add(delta)
                .ok_or(MmError::AddressOverflow)?,
        );
        map_page(hhdm, pml4, page_virt, page_phys, flags)?;
    }
    Ok(())
}

fn map_page(
    hhdm: &Hhdm,
    pml4: PhysAddr,
    virt: VirtAddr,
    phys: PhysAddr,
    flags: PageTableFlags,
) -> Result<(), MmError> {
    let walk = PageWalk::of(virt);
    let mut table = pml4;

    for level in [
        PageTableLevel::Pml4,
        PageTableLevel::Pdpt,
        PageTableLevel::Pd,
    ] {
        let index = walk.index(level);
        let value = read_entry(hhdm, table, index);
        let present = PageTableFlags::from_bits(value & ENTRY_FLAGS_MASK);
        if !present.is_present() {
            let next = alloc_zeroed_page(hhdm)?;
            let next_entry = PageTableEntry::new(next, intermediate_flags())?;
            write_entry(hhdm, table, index, next_entry.raw());
            table = next;
        } else if present.contains(PageTableFlags::HUGE_PAGE) {
            // Splitting huge pages is not needed for the heap/MMIO windows we
            // map; refuse instead of silently corrupting the mapping.
            return Err(MmError::InvalidEntry);
        } else {
            table = PhysAddr::new(value & mmcore::page_table::ENTRY_ADDR_MASK);
        }
    }

    let leaf = PageTableEntry::new(phys, flags)?;
    write_entry(hhdm, table, walk.pt, leaf.raw());
    Ok(())
}

/// Flags of the leaf mapping covering `virt`, if it is present.
fn walk_flags(hhdm: &Hhdm, pml4: PhysAddr, virt: VirtAddr) -> Option<PageTableFlags> {
    let walk = PageWalk::of(virt);
    let mut table = pml4;
    for level in [
        PageTableLevel::Pml4,
        PageTableLevel::Pdpt,
        PageTableLevel::Pd,
    ] {
        let value = read_entry(hhdm, table, walk.index(level));
        let flags = PageTableFlags::from_bits(value & ENTRY_FLAGS_MASK);
        if !flags.is_present() {
            return None;
        }
        if flags.contains(PageTableFlags::HUGE_PAGE) {
            return Some(flags);
        }
        table = PhysAddr::new(value & mmcore::page_table::ENTRY_ADDR_MASK);
    }
    let value = read_entry(hhdm, table, walk.pt);
    let flags = PageTableFlags::from_bits(value & ENTRY_FLAGS_MASK);
    flags.is_present().then_some(flags)
}
