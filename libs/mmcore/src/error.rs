//! Error type shared by the memory-management core.

use core::fmt;

/// Errors returned by [`crate::addr`], [`crate::buddy`] and [`crate::page_table`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmError {
    /// The address or length is not aligned to the required boundary.
    InvalidAlignment,
    /// A zero length or a zero page count was supplied.
    ZeroLength,
    /// `base + len` does not fit into `u64`.
    AddressOverflow,
    /// Two frame ranges overlap.
    RangeOverlap,
    /// More frame ranges than the allocator can hold.
    TooManyRanges,
    /// The fixed free-block table is full: the pool is too fragmented for the configured capacity.
    TooManyFreeBlocks,
    /// The buddy order is outside `0..=MAX_BUDDY_ORDER`.
    InvalidOrder,
    /// The requested size is zero, not a power of two, or too large.
    InvalidSize,
    /// No free block of the requested order is available.
    OutOfMemory,
    /// The block was never allocated, is already free, or lies outside the configured ranges.
    UnknownBlock,
    /// A page mapping would be writable and executable at the same time.
    WxViolation,
    /// A page-table entry has reserved bits set or an address that does not fit the entry.
    InvalidEntry,
}

impl fmt::Display for MmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidAlignment => "invalid alignment",
            Self::ZeroLength => "zero length",
            Self::AddressOverflow => "address overflow",
            Self::RangeOverlap => "frame ranges overlap",
            Self::TooManyRanges => "too many frame ranges",
            Self::TooManyFreeBlocks => "free-block table is full",
            Self::InvalidOrder => "invalid buddy order",
            Self::InvalidSize => "invalid size",
            Self::OutOfMemory => "out of memory",
            Self::UnknownBlock => "unknown or already free block",
            Self::WxViolation => "writable and executable page (W^X violation)",
            Self::InvalidEntry => "invalid page-table entry",
        };
        f.write_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::MmError;
    use core::fmt::Write as _;
    use std::string::String;
    use std::vec::Vec;

    const ALL: [MmError; 12] = [
        MmError::InvalidAlignment,
        MmError::ZeroLength,
        MmError::AddressOverflow,
        MmError::RangeOverlap,
        MmError::TooManyRanges,
        MmError::TooManyFreeBlocks,
        MmError::InvalidOrder,
        MmError::InvalidSize,
        MmError::OutOfMemory,
        MmError::UnknownBlock,
        MmError::WxViolation,
        MmError::InvalidEntry,
    ];

    #[test]
    fn display_messages_are_distinct_and_non_empty() {
        let mut seen: Vec<String> = Vec::new();
        for error in ALL {
            let mut text = String::new();
            write!(&mut text, "{error}").expect("formatting into a String cannot fail");
            assert!(!text.is_empty());
            assert!(!seen.contains(&text), "duplicate message: {text}");
            seen.push(text);
        }
    }
}
