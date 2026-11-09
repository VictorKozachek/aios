//! Page-table entries, flags and the machine-checkable W^X invariant.
//!
//! Only the encoding is modelled here; walking real page tables and touching
//! CR3 belongs to the kernel (task #005b).

use core::fmt;
use core::ops::{BitAnd, BitOr, BitOrAssign, Not};

use crate::addr::{PAGE_SHIFT, PhysAddr};
use crate::error::MmError;

/// Mask of the physical address stored in an entry: bits 12..=51.
pub const ENTRY_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
/// Mask of the flag bits an entry may carry: bits 0..=11 and NX (bit 63).
pub const ENTRY_FLAGS_MASK: u64 = 0x8000_0000_0000_0fff;
/// Reserved bits (52..=62) that must be zero in a valid entry.
pub const ENTRY_RESERVED_MASK: u64 = 0x7ff0_0000_0000_0000;

/// Access, cache and permission bits of a page-table entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PageTableFlags(u64);

impl PageTableFlags {
    /// No flags at all (a non-present entry).
    pub const NONE: Self = Self(0);
    /// Entry is valid and the mapping is active.
    pub const PRESENT: Self = Self(1 << 0);
    /// Writable mapping. Must never be combined with an executable mapping: W^X.
    pub const WRITABLE: Self = Self(1 << 1);
    /// Accessible from CPL3.
    pub const USER: Self = Self(1 << 2);
    /// Write-through caching (PWT).
    pub const WRITE_THROUGH: Self = Self(1 << 3);
    /// Disable caching (PCD). MMIO is mapped with this bit set.
    pub const NO_CACHE: Self = Self(1 << 4);
    /// Accessed by the CPU.
    pub const ACCESSED: Self = Self(1 << 5);
    /// Dirty (set by the CPU on writes).
    pub const DIRTY: Self = Self(1 << 6);
    /// The entry maps a huge page (bit 7 of a non-leaf entry).
    /// In a 4 KiB leaf the same bit is PAT.
    pub const HUGE_PAGE: Self = Self(1 << 7);
    /// Page attribute table bit of a 4 KiB leaf entry (same bit as [`Self::HUGE_PAGE`]).
    pub const PAT: Self = Self(1 << 7);
    /// Global translation (not flushed on CR3 reload).
    pub const GLOBAL: Self = Self(1 << 8);
    /// No-execute: the mapping must not be executed.
    pub const NX: Self = Self(1 << 63);

    /// Builds flags from raw bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the raw bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns `true` if all bits of `other` are set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the union of both flag sets.
    pub const fn insert(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns the flags with all bits of `other` cleared.
    pub const fn remove(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Returns `true` if the entry is present.
    pub const fn is_present(self) -> bool {
        self.contains(Self::PRESENT)
    }

    /// Returns `true` if the mapping is writable.
    pub const fn is_writable(self) -> bool {
        self.contains(Self::WRITABLE)
    }

    /// Returns `true` if the mapping may be executed (NX is clear).
    pub const fn is_executable(self) -> bool {
        !self.contains(Self::NX)
    }

    /// Returns `true` if the mapping is accessible from userspace.
    pub const fn is_user(self) -> bool {
        self.contains(Self::USER)
    }

    /// Returns `true` if the mapping disables caching or uses write-through.
    pub const fn is_uncached(self) -> bool {
        self.contains(Self::NO_CACHE)
    }

    /// Returns `true` if the mapping satisfies the W^X invariant:
    /// never writable and executable at the same time.
    pub const fn is_wx_safe(self) -> bool {
        !(self.is_writable() && self.is_executable())
    }

    /// Read-only, executable kernel mapping: `PRESENT` without `NX`.
    pub const fn kernel_code() -> Self {
        Self::PRESENT
    }

    /// Writable, non-executable kernel mapping: `PRESENT | WRITABLE | NX`.
    pub const fn kernel_data() -> Self {
        Self(Self::PRESENT.0 | Self::WRITABLE.0 | Self::NX.0)
    }
}

impl BitOr for PageTableFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.insert(rhs)
    }
}

impl BitOrAssign for PageTableFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for PageTableFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl Not for PageTableFlags {
    type Output = Self;

    fn not(self) -> Self {
        Self(!self.0)
    }
}

impl fmt::Debug for PageTableFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageTableFlags({:#x})", self.0)
    }
}

/// Caching policy of a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheMode {
    /// Normal write-back caching (no bits set).
    WriteBack,
    /// Write-through caching.
    WriteThrough,
    /// Uncached: used for MMIO.
    Uncached,
    /// Write-combined: used for the framebuffer.
    WriteCombined,
}

impl CacheMode {
    /// Returns the entry flags that select this cache mode for a 4 KiB leaf.
    pub const fn flags(self) -> PageTableFlags {
        match self {
            Self::WriteBack => PageTableFlags::NONE,
            Self::WriteThrough => PageTableFlags::WRITE_THROUGH,
            Self::Uncached => {
                PageTableFlags(PageTableFlags::WRITE_THROUGH.0 | PageTableFlags::NO_CACHE.0)
            }
            Self::WriteCombined => PageTableFlags::PAT,
        }
    }
}

/// One 64-bit page-table entry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Encodes `addr` and `flags` into an entry.
    ///
    /// Rejects unaligned or too-wide addresses and mappings that would violate W^X.
    pub fn new(addr: PhysAddr, flags: PageTableFlags) -> Result<Self, MmError> {
        if !addr.is_page_aligned() {
            return Err(MmError::InvalidAlignment);
        }
        if addr.as_u64() & !ENTRY_ADDR_MASK != 0 {
            return Err(MmError::InvalidEntry);
        }
        if !flags.is_wx_safe() {
            return Err(MmError::WxViolation);
        }
        Ok(Self(addr.as_u64() | flags.bits()))
    }

    /// Reinterprets a raw entry, for example one read from a live table.
    ///
    /// The value is not validated; call [`Self::validate`] for a checked view.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Returns the raw 64-bit value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Returns the physical address encoded in the entry.
    pub const fn phys_addr(self) -> PhysAddr {
        PhysAddr::new(self.0 & ENTRY_ADDR_MASK)
    }

    /// Returns the flag bits of the entry.
    pub const fn flags(self) -> PageTableFlags {
        PageTableFlags::from_bits(self.0 & ENTRY_FLAGS_MASK)
    }

    /// Returns `true` if the entry is present.
    pub const fn is_present(self) -> bool {
        self.flags().is_present()
    }

    /// Returns `true` if the entry satisfies W^X.
    pub const fn is_wx_safe(self) -> bool {
        self.flags().is_wx_safe()
    }

    /// Validates reserved bits and the W^X invariant.
    pub fn validate(self) -> Result<(), MmError> {
        if self.0 & ENTRY_RESERVED_MASK != 0 {
            return Err(MmError::InvalidEntry);
        }
        if !self.is_wx_safe() {
            return Err(MmError::WxViolation);
        }
        Ok(())
    }
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageTableEntry({:#018x})", self.0)
    }
}

/// Returns `true` if `addr` can be stored in a page-table entry.
pub const fn is_entry_addressable(addr: PhysAddr) -> bool {
    addr.as_u64() & !ENTRY_ADDR_MASK == 0
}

/// Returns the largest physical address an entry can encode (bits 12..=51).
pub const fn max_entry_address() -> u64 {
    ENTRY_ADDR_MASK
}

/// Offset of the physical address inside an entry.
pub const fn entry_addr_shift() -> u32 {
    PAGE_SHIFT
}

/// Size of the region addressable through one entry: `2^52` bytes.
pub const fn entry_address_space() -> u64 {
    1u64 << 52
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::{HUGE_PAGE_SIZE, PAGE_SIZE};

    #[test]
    fn flags_bit_operations() {
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        assert!(flags.contains(PageTableFlags::PRESENT));
        assert!(flags.contains(PageTableFlags::WRITABLE));
        assert!(!flags.contains(PageTableFlags::USER));
        assert_eq!(
            flags.remove(PageTableFlags::WRITABLE),
            PageTableFlags::PRESENT
        );
        assert_eq!(
            PageTableFlags::PRESENT.insert(PageTableFlags::USER).bits(),
            0b101
        );
        assert_eq!((PageTableFlags::PRESENT & PageTableFlags::USER).bits(), 0);
        assert_eq!((!PageTableFlags::PRESENT).bits(), !1u64);
        assert_eq!(PageTableFlags::from_bits(0b101).bits(), 0b101);
        assert!(!PageTableFlags::NONE.is_present());
        assert!(PageTableFlags::PRESENT.is_present());
        assert!(!PageTableFlags::PAT.is_uncached());
        let mut assigned = PageTableFlags::PRESENT;
        assigned |= PageTableFlags::USER;
        assert!(assigned.contains(PageTableFlags::USER));
        assert!(PageTableFlags::NO_CACHE.is_uncached());
    }

    #[test]
    fn wx_invariant() {
        // Writable and executable: forbidden.
        let bad = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        assert!(!bad.is_wx_safe());
        // Writable and non-executable: fine.
        let data = bad | PageTableFlags::NX;
        assert!(data.is_wx_safe());
        assert!(!data.is_executable());
        // Read-only and executable: fine.
        let code = PageTableFlags::kernel_code();
        assert!(code.is_wx_safe());
        assert!(code.is_executable());
        assert!(!code.is_writable());
        // Kernel data constructor obeys W^X.
        let kdata = PageTableFlags::kernel_data();
        assert!(kdata.is_present());
        assert!(kdata.is_writable());
        assert!(!kdata.is_executable());
        assert!(kdata.is_wx_safe());
        // An entry without PRESENT still reports the raw bits consistently.
        let absent = PageTableFlags::WRITABLE;
        assert!(!absent.is_present());
    }

    #[test]
    fn cache_modes_select_expected_bits() {
        assert_eq!(CacheMode::WriteBack.flags(), PageTableFlags::NONE);
        assert_eq!(
            CacheMode::WriteThrough.flags(),
            PageTableFlags::WRITE_THROUGH
        );
        assert_eq!(
            CacheMode::Uncached.flags(),
            PageTableFlags::WRITE_THROUGH | PageTableFlags::NO_CACHE
        );
        assert_eq!(CacheMode::WriteCombined.flags(), PageTableFlags::PAT);
    }

    #[test]
    fn entry_roundtrip() {
        let addr = PhysAddr::new(0x1234_5000);
        let flags = PageTableFlags::PRESENT | PageTableFlags::USER | PageTableFlags::NX;
        let entry = PageTableEntry::new(addr, flags).expect("valid entry");
        assert!(entry.is_present());
        assert_eq!(entry.phys_addr(), addr);
        assert_eq!(entry.flags(), flags);
        assert_eq!(entry.raw(), 0x1234_5000 | flags.bits());
        assert!(entry.is_wx_safe());
        assert_eq!(entry.validate(), Ok(()));

        // Parsing the same bits back from memory gives an identical entry.
        assert_eq!(PageTableEntry::from_raw(entry.raw()), entry);
    }

    #[test]
    fn entry_rejects_wx_violation() {
        let addr = PhysAddr::new(0x1000);
        assert_eq!(
            PageTableEntry::new(addr, PageTableFlags::PRESENT | PageTableFlags::WRITABLE),
            Err(MmError::WxViolation)
        );
        // Adding NX makes the same mapping valid.
        assert!(
            PageTableEntry::new(
                addr,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NX
            )
            .is_ok()
        );
    }

    #[test]
    fn entry_rejects_unaligned_and_wide_addresses() {
        assert_eq!(
            PageTableEntry::new(PhysAddr::new(0x1001), PageTableFlags::PRESENT),
            Err(MmError::InvalidAlignment)
        );
        // Bit 52 does not fit into the address field.
        assert_eq!(
            PageTableEntry::new(PhysAddr::new(1 << 52), PageTableFlags::PRESENT),
            Err(MmError::InvalidEntry)
        );
        assert_eq!(
            PageTableEntry::new(PhysAddr::new(1 << 63), PageTableFlags::PRESENT),
            Err(MmError::InvalidEntry)
        );
        assert!(is_entry_addressable(PhysAddr::new(ENTRY_ADDR_MASK)));
        assert!(!is_entry_addressable(PhysAddr::new(
            ENTRY_ADDR_MASK + PAGE_SIZE
        )));
    }

    #[test]
    fn entry_validate_checks_reserved_bits() {
        let raw_with_reserved = (1u64 << 52) | 0x1000 | PageTableFlags::PRESENT.bits();
        let entry = PageTableEntry::from_raw(raw_with_reserved);
        assert_eq!(entry.validate(), Err(MmError::InvalidEntry));

        // A W^X-violating entry read from memory is also rejected.
        let raw_wx = 0x1000 | (PageTableFlags::PRESENT | PageTableFlags::WRITABLE).bits();
        assert_eq!(
            PageTableEntry::from_raw(raw_wx).validate(),
            Err(MmError::WxViolation)
        );

        // NX is a legal flag bit, not a reserved one.
        let raw_nx = 0x1000 | PageTableFlags::kernel_data().bits();
        assert_eq!(PageTableEntry::from_raw(raw_nx).validate(), Ok(()));
    }

    #[test]
    fn huge_page_entries_are_representable() {
        // A 2 MiB leaf entry: present, huge, writable, NX.
        let addr = PhysAddr::new(HUGE_PAGE_SIZE);
        assert!(addr.is_aligned_to(HUGE_PAGE_SIZE));
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::HUGE_PAGE
            | PageTableFlags::WRITABLE
            | PageTableFlags::NX;
        let entry = PageTableEntry::new(addr, flags).expect("valid huge-page entry");
        assert!(entry.flags().contains(PageTableFlags::HUGE_PAGE));
        assert_eq!(entry.phys_addr(), addr);
    }

    #[test]
    fn entry_address_limits() {
        assert_eq!(max_entry_address(), 0x000f_ffff_ffff_f000);
        assert_eq!(entry_addr_shift(), PAGE_SHIFT);
        assert_eq!(entry_address_space(), 1u64 << 52);
    }
}
