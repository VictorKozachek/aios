#![no_std]

//! Pure memory-management logic for AIOS: the buddy allocator for physical
//! frames, address arithmetic, page-table structures and their invariants.
//!
//! This crate deliberately contains **no hardware access** — no port I/O,
//! no MSR, no CR3. The kernel (task #005b) owns the real memory map and the
//! CPU, while everything here is testable on the host with plain `cargo test`,
//! exactly like `libs/memfmt`.
//!
//! Task #014: implementation (docs/PLAN-M0.md, T4; docs/ARCHITECTURE.md §4.1).
//!
//! * [`addr`] — physical/virtual addresses, HHDM translation, alignment and
//!   4-level page-table index extraction;
//! * [`buddy`] — buddy allocator over abstract frame ranges, with per-order
//!   statistics, coalescing and explicit tracking of live allocations;
//! * [`page_table`] — entry/flags encoding, cache modes and the W^X invariant;
//! * [`error`] — the shared [`MmError`] type.

pub mod addr;
pub mod buddy;
pub mod error;
pub mod page_table;

pub use addr::{
    GIGA_PAGE_SIZE, HUGE_PAGE_SIZE, Hhdm, MAX_BUDDY_BLOCK_SIZE, MAX_BUDDY_ORDER, ORDER_COUNT,
    PAGE_SIZE, PageTableLevel, PageWalk, PhysAddr, VirtAddr, align_down, align_up, is_canonical_48,
    is_canonical_57, is_power_of_two, order_to_size, size_to_order,
};
pub use buddy::{BuddyAllocator, FrameRange, MAX_FREE_BLOCKS, MAX_RANGES, OrderStats, Stats};
pub use error::MmError;
pub use page_table::{CacheMode, PageTableEntry, PageTableFlags};

#[cfg(test)]
extern crate std;
