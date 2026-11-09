//! Address arithmetic: physical and virtual addresses, HHDM translation,
//! alignment helpers and 4-level page-table index extraction.
//!
//! Everything here is pure arithmetic — no hardware access.

use core::fmt;

use crate::error::MmError;

/// Shift of a 4 KiB base page.
pub const PAGE_SHIFT: u32 = 12;
/// Size of a 4 KiB base page, in bytes.
pub const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;
/// Mask of the offset inside a 4 KiB page.
pub const PAGE_MASK: u64 = PAGE_SIZE - 1;

/// Shift of a 2 MiB huge page (level 3 leaf).
pub const HUGE_PAGE_SHIFT: u32 = 21;
/// Size of a 2 MiB huge page, in bytes.
pub const HUGE_PAGE_SIZE: u64 = 1 << HUGE_PAGE_SHIFT;

/// Shift of a 1 GiB huge page (level 2 leaf).
pub const GIGA_PAGE_SHIFT: u32 = 30;
/// Size of a 1 GiB huge page, in bytes.
pub const GIGA_PAGE_SIZE: u64 = 1 << GIGA_PAGE_SHIFT;

/// Highest buddy order the allocator supports: `4 KiB << 18 == 1 GiB`.
pub const MAX_BUDDY_ORDER: u8 = 18;
/// Largest buddy block: 1 GiB.
pub const MAX_BUDDY_BLOCK_SIZE: u64 = PAGE_SIZE << MAX_BUDDY_ORDER;
/// Number of buddy orders, `0..=MAX_BUDDY_ORDER`.
pub const ORDER_COUNT: usize = MAX_BUDDY_ORDER as usize + 1;

/// Entries in one page table (all levels are 4 KiB and hold 512 entries).
pub const PAGE_TABLE_ENTRIES: u64 = 512;
/// Mask of a single page-table index.
pub const PAGE_TABLE_INDEX_MASK: u64 = PAGE_TABLE_ENTRIES - 1;

/// Bit position of the PML4 (level 4) index.
pub const PML4_SHIFT: u32 = 39;
/// Bit position of the PDPT (level 3) index.
pub const PDPT_SHIFT: u32 = 30;
/// Bit position of the PD (level 2) index.
pub const PD_SHIFT: u32 = 21;
/// Bit position of the PT (level 1) index.
pub const PT_SHIFT: u32 = 12;

/// Number of significant virtual-address bits in 4-level paging.
pub const VIRTUAL_ADDRESS_BITS_4LEVEL: u32 = 48;
/// Number of significant virtual-address bits in 5-level paging.
pub const VIRTUAL_ADDRESS_BITS_5LEVEL: u32 = 57;

/// Returns `true` if `value` is a non-zero power of two.
pub const fn is_power_of_two(value: u64) -> bool {
    value != 0 && value & (value - 1) == 0
}

/// Size of a buddy block of the given order, or `None` for an invalid order.
pub const fn order_to_size(order: u8) -> Option<u64> {
    if order > MAX_BUDDY_ORDER {
        None
    } else {
        Some(PAGE_SIZE << order)
    }
}

/// Order of a buddy block of the given size, or `None` if the size is not
/// a power of two in `PAGE_SIZE..=MAX_BUDDY_BLOCK_SIZE`.
pub const fn size_to_order(size: u64) -> Option<u8> {
    if size < PAGE_SIZE || size > MAX_BUDDY_BLOCK_SIZE || !is_power_of_two(size) {
        return None;
    }
    if size.trailing_zeros() < PAGE_SHIFT {
        return None;
    }
    Some((size.trailing_zeros() - PAGE_SHIFT) as u8)
}

/// Rounds `value` down to a multiple of `align`.
///
/// `align` must be a non-zero power of two; the caller guarantees this,
/// as a misaligned value cannot be represented otherwise.
pub const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

/// Rounds `value` up to a multiple of `align`, or `None` on `u64` overflow.
pub fn align_up(value: u64, align: u64) -> Option<u64> {
    let down = align_down(value, align);
    if down == value {
        Some(value)
    } else {
        down.checked_add(align)
    }
}

/// Returns `true` if `value` is canonical for 48-bit (4-level) paging.
pub const fn is_canonical_48(value: u64) -> bool {
    let upper = value >> (VIRTUAL_ADDRESS_BITS_4LEVEL - 1);
    upper == 0 || upper == 0x1_ffff
}

/// Returns `true` if `value` is canonical for 57-bit (5-level) paging.
pub const fn is_canonical_57(value: u64) -> bool {
    let upper = value >> (VIRTUAL_ADDRESS_BITS_5LEVEL - 1);
    upper == 0 || upper == 0xff
}

/// A physical memory address.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Wraps a raw physical address.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw address.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns `true` if the address is aligned to a 4 KiB page.
    pub const fn is_page_aligned(self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    /// Returns `true` if the address is aligned to `align` (a power of two).
    pub const fn is_aligned_to(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }

    /// Adds `delta`, or `None` on overflow.
    pub fn checked_add(self, delta: u64) -> Option<Self> {
        self.0.checked_add(delta).map(Self)
    }

    /// Subtracts `delta`, or `None` on underflow.
    pub fn checked_sub(self, delta: u64) -> Option<Self> {
        self.0.checked_sub(delta).map(Self)
    }

    /// Rounds the address down to a multiple of `align`.
    pub const fn align_down(self, align: u64) -> Self {
        Self(align_down(self.0, align))
    }

    /// Rounds the address up to a multiple of `align`, or `None` on overflow.
    pub fn align_up(self, align: u64) -> Option<Self> {
        align_up(self.0, align).map(Self)
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhysAddr({:#x})", self.0)
    }
}

/// A virtual memory address.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct VirtAddr(u64);

impl VirtAddr {
    /// Wraps a raw virtual address.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw address.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns `true` if the address is aligned to a 4 KiB page.
    pub const fn is_page_aligned(self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    /// Returns `true` if the address is aligned to `align` (a power of two).
    pub const fn is_aligned_to(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }

    /// Returns `true` if the address is canonical for 48-bit paging.
    pub const fn is_canonical(self) -> bool {
        is_canonical_48(self.0)
    }

    /// Adds `delta`, or `None` on overflow.
    pub fn checked_add(self, delta: u64) -> Option<Self> {
        self.0.checked_add(delta).map(Self)
    }

    /// Subtracts `delta`, or `None` on underflow.
    pub fn checked_sub(self, delta: u64) -> Option<Self> {
        self.0.checked_sub(delta).map(Self)
    }

    /// Rounds the address down to a multiple of `align`.
    pub const fn align_down(self, align: u64) -> Self {
        Self(align_down(self.0, align))
    }

    /// Rounds the address up to a multiple of `align`, or `None` on overflow.
    pub fn align_up(self, align: u64) -> Option<Self> {
        align_up(self.0, align).map(Self)
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VirtAddr({:#x})", self.0)
    }
}

/// The HHDM ("higher-half direct map") offset reported by the bootloader:
/// every physical address `p` is mapped at `p + offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hhdm {
    offset: u64,
}

impl Hhdm {
    /// Creates an HHDM translation with the given offset.
    pub const fn new(offset: u64) -> Self {
        Self { offset }
    }

    /// Returns the raw offset.
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// Returns `true` if the offset is page aligned.
    pub const fn is_page_aligned(self) -> bool {
        self.offset & PAGE_MASK == 0
    }

    /// Translates a physical address into its HHDM virtual address.
    pub fn phys_to_virt(self, phys: PhysAddr) -> Option<VirtAddr> {
        phys.as_u64().checked_add(self.offset).map(VirtAddr::new)
    }

    /// Translates an HHDM virtual address back into a physical address.
    pub fn virt_to_phys(self, virt: VirtAddr) -> Option<PhysAddr> {
        virt.as_u64().checked_sub(self.offset).map(PhysAddr::new)
    }
}

/// One of the four levels of a 4-level page table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PageTableLevel {
    /// Level 4, indexed by bits 47..39.
    Pml4,
    /// Level 3, indexed by bits 38..30.
    Pdpt,
    /// Level 2, indexed by bits 29..21.
    Pd,
    /// Level 1, indexed by bits 20..12.
    Pt,
}

impl PageTableLevel {
    /// Bit position of this level's index inside a virtual address.
    pub const fn shift(self) -> u32 {
        match self {
            Self::Pml4 => PML4_SHIFT,
            Self::Pdpt => PDPT_SHIFT,
            Self::Pd => PD_SHIFT,
            Self::Pt => PT_SHIFT,
        }
    }

    /// Extracts this level's index from `virt`.
    pub const fn index(self, virt: VirtAddr) -> u16 {
        ((virt.as_u64() >> self.shift()) & PAGE_TABLE_INDEX_MASK) as u16
    }

    /// Number of bytes covered by one entry at this level.
    pub const fn entry_span(self) -> u64 {
        match self {
            Self::Pml4 => GIGA_PAGE_SIZE * PAGE_TABLE_ENTRIES,
            Self::Pdpt => GIGA_PAGE_SIZE,
            Self::Pd => HUGE_PAGE_SIZE,
            Self::Pt => PAGE_SIZE,
        }
    }

    /// Size of the leaf page mapped at this level; `None` for the PML4,
    /// which cannot hold a leaf mapping.
    pub const fn leaf_page_size(self) -> Option<u64> {
        match self {
            Self::Pml4 => None,
            Self::Pdpt | Self::Pd | Self::Pt => Some(self.entry_span()),
        }
    }

    /// Short name of the level (`pml4`, `pdpt`, `pd`, `pt`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pml4 => "pml4",
            Self::Pdpt => "pdpt",
            Self::Pd => "pd",
            Self::Pt => "pt",
        }
    }
}

/// The complete index path of a 4 KiB page in a 4-level page table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageWalk {
    /// PML4 index (bits 47..39).
    pub pml4: u16,
    /// PDPT index (bits 38..30).
    pub pdpt: u16,
    /// PD index (bits 29..21).
    pub pd: u16,
    /// PT index (bits 20..12).
    pub pt: u16,
    /// Offset inside the 4 KiB page (bits 11..0).
    pub offset: u16,
}

impl PageWalk {
    /// Decomposes `virt` into the four page-table indices and the page offset.
    pub const fn of(virt: VirtAddr) -> Self {
        Self {
            pml4: PageTableLevel::Pml4.index(virt),
            pdpt: PageTableLevel::Pdpt.index(virt),
            pd: PageTableLevel::Pd.index(virt),
            pt: PageTableLevel::Pt.index(virt),
            offset: (virt.as_u64() & PAGE_MASK) as u16,
        }
    }

    /// Returns the index for `level` from this walk.
    pub const fn index(&self, level: PageTableLevel) -> u16 {
        match level {
            PageTableLevel::Pml4 => self.pml4,
            PageTableLevel::Pdpt => self.pdpt,
            PageTableLevel::Pd => self.pd,
            PageTableLevel::Pt => self.pt,
        }
    }
}

/// Builds a physical address, validating alignment.
pub fn checked_page_phys(addr: u64) -> Result<PhysAddr, MmError> {
    if addr & PAGE_MASK != 0 {
        return Err(MmError::InvalidAlignment);
    }
    Ok(PhysAddr::new(addr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_size_roundtrip() {
        for order in 0..=MAX_BUDDY_ORDER {
            let size = order_to_size(order).expect("valid order");
            assert_eq!(size, PAGE_SIZE << order);
            assert_eq!(size_to_order(size), Some(order));
        }
        assert_eq!(order_to_size(MAX_BUDDY_ORDER + 1), None);
        assert_eq!(order_to_size(255), None);
        assert_eq!(size_to_order(0), None);
        assert_eq!(size_to_order(PAGE_SIZE - 1), None);
        assert_eq!(size_to_order(PAGE_SIZE + PAGE_SIZE / 2), None);
        assert_eq!(size_to_order(MAX_BUDDY_BLOCK_SIZE * 2), None);
        assert_eq!(size_to_order(MAX_BUDDY_BLOCK_SIZE * 2 - PAGE_SIZE), None);
    }

    #[test]
    fn alignment_helpers() {
        assert_eq!(align_down(0, PAGE_SIZE), 0);
        assert_eq!(align_down(4095, PAGE_SIZE), 0);
        assert_eq!(align_down(4096, PAGE_SIZE), 4096);
        assert_eq!(align_down(4097, PAGE_SIZE), 4096);
        assert_eq!(align_up(0, PAGE_SIZE), Some(0));
        assert_eq!(align_up(1, PAGE_SIZE), Some(PAGE_SIZE));
        assert_eq!(align_up(4096, PAGE_SIZE), Some(4096));
        assert_eq!(align_up(4097, PAGE_SIZE), Some(8192));

        // Alignment overflow at the top of the address space.
        assert_eq!(align_up(u64::MAX - 5, PAGE_SIZE), None);
        assert_eq!(align_down(u64::MAX, PAGE_SIZE), u64::MAX - PAGE_MASK);
    }

    #[test]
    fn power_of_two_and_canonical() {
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(PAGE_SIZE));
        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(3));

        assert!(is_canonical_48(0));
        assert!(is_canonical_48(0x0000_7fff_ffff_ffff));
        assert!(is_canonical_48(0xffff_8000_0000_0000));
        assert!(is_canonical_48(0xffff_ffff_ffff_ffff));
        assert!(!is_canonical_48(0x0000_8000_0000_0000));
        assert!(!is_canonical_48(0xffff_7fff_ffff_ffff));

        assert!(is_canonical_57(0x00ff_ffff_ffff_ffff));
        assert!(is_canonical_57(0xff00_0000_0000_0000));
        assert!(!is_canonical_57(0x0100_0000_0000_0000));
        assert!(!is_canonical_57(0xfeff_ffff_ffff_ffff));
    }

    #[test]
    fn phys_addr_arithmetic() {
        let addr = PhysAddr::new(0x1000);
        assert!(addr.is_page_aligned());
        assert!(!PhysAddr::new(0x1001).is_page_aligned());
        assert!(!addr.is_aligned_to(HUGE_PAGE_SIZE));
        assert!(PhysAddr::new(HUGE_PAGE_SIZE).is_aligned_to(HUGE_PAGE_SIZE));
        assert_eq!(addr.checked_add(PAGE_SIZE), Some(PhysAddr::new(0x2000)));
        assert_eq!(addr.checked_sub(1), Some(PhysAddr::new(0x0fff)));
        assert_eq!(addr.checked_sub(0x1001), None);
        assert_eq!(PhysAddr::new(u64::MAX - 1).checked_add(2), None);
        assert_eq!(
            PhysAddr::new(0x1fff).align_down(PAGE_SIZE),
            PhysAddr::new(0x1000)
        );
        assert_eq!(
            PhysAddr::new(0x1001).align_up(PAGE_SIZE),
            Some(PhysAddr::new(0x2000))
        );
        assert_eq!(PhysAddr::new(u64::MAX - 1).align_up(PAGE_SIZE), None);
    }

    #[test]
    fn virt_addr_arithmetic() {
        let addr = VirtAddr::new(0xffff_8000_0000_1000);
        assert!(addr.is_page_aligned());
        assert!(addr.is_canonical());
        assert!(!VirtAddr::new(0x0000_8000_0000_0000).is_canonical());
        assert_eq!(
            addr.checked_add(8),
            Some(VirtAddr::new(0xffff_8000_0000_1008))
        );
        assert_eq!(VirtAddr::new(4).checked_sub(8), None);
        assert_eq!(
            VirtAddr::new(0x1fff).align_down(PAGE_SIZE),
            VirtAddr::new(0x1000)
        );
    }

    #[test]
    fn hhdm_translation_roundtrip() {
        let hhdm = Hhdm::new(0xffff_8000_0000_0000);
        assert!(hhdm.is_page_aligned());
        let phys = PhysAddr::new(0x0000_0000_1234_5000);
        let virt = hhdm.phys_to_virt(phys).expect("translation fits");
        assert_eq!(virt, VirtAddr::new(0xffff_8000_1234_5000));
        assert_eq!(hhdm.virt_to_phys(virt), Some(phys));

        // Physical addresses whose translation does not fit in `u64`.
        let huge = PhysAddr::new(u64::MAX);
        assert_eq!(hhdm.phys_to_virt(huge), None);
        // Virt addresses below the offset are not HHDM mappings.
        assert_eq!(hhdm.virt_to_phys(VirtAddr::new(0x1000)), None);
    }

    #[test]
    fn page_walk_indices() {
        let virt = VirtAddr::new(0x0000_7fff_ffff_ffff);
        let walk = PageWalk::of(virt);
        assert_eq!(walk.pml4, 0x00ff);
        assert_eq!(walk.pdpt, 0x01ff);
        assert_eq!(walk.pd, 0x01ff);
        assert_eq!(walk.pt, 0x01ff);
        assert_eq!(walk.offset, 0x0fff);
        assert_eq!(walk.index(PageTableLevel::Pml4), 0x00ff);
        assert_eq!(walk.index(PageTableLevel::Pdpt), 0x01ff);
        assert_eq!(walk.index(PageTableLevel::Pd), 0x01ff);
        assert_eq!(walk.index(PageTableLevel::Pt), 0x01ff);

        // Kernel half: 0xffff_8000_0000_0000.
        let kernel = PageWalk::of(VirtAddr::new(0xffff_8000_0000_0000));
        assert_eq!(kernel.pml4, 0x0100);
        assert_eq!(kernel.pdpt, 0);
        assert_eq!(kernel.pd, 0);
        assert_eq!(kernel.pt, 0);
        assert_eq!(kernel.offset, 0);

        // A page-aligned address with all four indices non-zero:
        // 0x0000_0141_0042_3000 -> pml4=0x002, pdpt=0x104, pd=0x002, pt=0x023.
        let w = PageWalk::of(VirtAddr::new(0x0000_0141_0042_3000));
        assert_eq!(
            (w.pml4, w.pdpt, w.pd, w.pt, w.offset),
            (0x002, 0x104, 0x002, 0x023, 0)
        );
    }

    #[test]
    fn page_walk_known_address() {
        // 0xffff_8000_1234_5678:
        //   pml4 = (v >> 39) & 0x1ff = 0x100
        //   pdpt = (v >> 30) & 0x1ff = 0x000
        //   pd   = (v >> 21) & 0x1ff = 0x091
        //   pt   = (v >> 12) & 0x1ff = 0x145
        //   off  = 0x678
        let walk = PageWalk::of(VirtAddr::new(0xffff_8000_1234_5678));
        assert_eq!(
            (walk.pml4, walk.pdpt, walk.pd, walk.pt, walk.offset),
            (0x100, 0, 0x091, 0x145, 0x678)
        );
    }

    #[test]
    fn level_spans_and_leaf_sizes() {
        assert_eq!(PageTableLevel::Pt.entry_span(), PAGE_SIZE);
        assert_eq!(PageTableLevel::Pd.entry_span(), HUGE_PAGE_SIZE);
        assert_eq!(PageTableLevel::Pdpt.entry_span(), GIGA_PAGE_SIZE);
        assert_eq!(PageTableLevel::Pml4.entry_span(), GIGA_PAGE_SIZE * 512);

        assert_eq!(PageTableLevel::Pt.leaf_page_size(), Some(PAGE_SIZE));
        assert_eq!(PageTableLevel::Pd.leaf_page_size(), Some(HUGE_PAGE_SIZE));
        assert_eq!(PageTableLevel::Pdpt.leaf_page_size(), Some(GIGA_PAGE_SIZE));
        assert_eq!(PageTableLevel::Pml4.leaf_page_size(), None);

        assert_eq!(PageTableLevel::Pml4.name(), "pml4");
        assert_eq!(PageTableLevel::Pdpt.name(), "pdpt");
        assert_eq!(PageTableLevel::Pd.name(), "pd");
        assert_eq!(PageTableLevel::Pt.name(), "pt");
        assert_eq!(PageTableLevel::Pml4.shift(), PML4_SHIFT);
        assert_eq!(PageTableLevel::Pdpt.shift(), PDPT_SHIFT);
        assert_eq!(PageTableLevel::Pd.shift(), PD_SHIFT);
        assert_eq!(PageTableLevel::Pt.shift(), PT_SHIFT);
    }

    #[test]
    fn checked_page_phys_rejects_misalignment() {
        assert_eq!(checked_page_phys(0x1000), Ok(PhysAddr::new(0x1000)));
        assert_eq!(checked_page_phys(0x1001), Err(MmError::InvalidAlignment));
    }
}
