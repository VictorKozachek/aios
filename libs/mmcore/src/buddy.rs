//! Buddy allocator for physical frames.
//!
//! The allocator works over a list of abstract, page-aligned frame ranges
//! (typically taken from the bootloader memory map). It knows nothing about
//! the real memory map, MMIO or the CPU: seeding it and programming the
//! hardware belongs to the kernel (task #005b).
//!
//! Supported orders are 0..=[`MAX_BUDDY_ORDER`], i.e. 4 KiB .. 1 GiB.
//! Free blocks live in a fixed-size, address-sorted table, so the allocator
//! itself is allocation-free (`no_std`, no global allocator).

use crate::addr::{
    MAX_BUDDY_BLOCK_SIZE, MAX_BUDDY_ORDER, ORDER_COUNT, PAGE_SIZE, PhysAddr, is_power_of_two,
    order_to_size, size_to_order,
};
use crate::error::MmError;

/// Maximum number of free blocks the allocator can track at once.
///
/// A pool whose free space is fragmented into more blocks than this reports
/// [`MmError::TooManyFreeBlocks`] instead of silently losing memory.
pub const MAX_FREE_BLOCKS: usize = 8192;

/// Maximum number of frame ranges an allocator can hold.
pub const MAX_RANGES: usize = 16;

/// A page-aligned physical frame range `[base, base + len)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRange {
    base: PhysAddr,
    len: u64,
}

impl FrameRange {
    /// Creates a range, requiring a page-aligned base and a non-zero,
    /// page-aligned length that does not overflow `u64`.
    pub fn new(base: PhysAddr, len: u64) -> Result<Self, MmError> {
        if len == 0 {
            return Err(MmError::ZeroLength);
        }
        if !base.is_page_aligned() || !len.is_multiple_of(PAGE_SIZE) {
            return Err(MmError::InvalidAlignment);
        }
        base.checked_add(len).ok_or(MmError::AddressOverflow)?;
        Ok(Self { base, len })
    }

    /// Start of the range.
    pub const fn base(&self) -> PhysAddr {
        self.base
    }

    /// Length of the range in bytes.
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Returns `true` if the range is empty (never for a valid range).
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Exclusive end of the range.
    pub const fn end(&self) -> u64 {
        self.base.as_u64() + self.len
    }

    /// Number of 4 KiB pages in the range.
    pub const fn pages(&self) -> u64 {
        self.len / PAGE_SIZE
    }
}

/// Free-block statistics for one buddy order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderStats {
    /// Buddy order (`4 KiB << order`).
    pub order: u8,
    /// Number of free blocks of this order.
    pub blocks: u32,
    /// Total number of free bytes in blocks of this order.
    pub bytes: u64,
}

impl OrderStats {
    /// Zeroed entry, used to build the per-order table.
    pub const EMPTY: Self = Self {
        order: 0,
        blocks: 0,
        bytes: 0,
    };
}

/// A snapshot of allocator statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Per-order free-block counts, indexed by order.
    pub orders: [OrderStats; ORDER_COUNT],
    /// Total bytes managed by the allocator.
    pub total_bytes: u64,
    /// Total free bytes.
    pub free_bytes: u64,
}

impl Stats {
    /// Number of free blocks of `order`.
    pub fn blocks_at(&self, order: u8) -> u32 {
        self.orders
            .get(order as usize)
            .map_or(0, |stats| stats.blocks)
    }

    /// Free bytes in blocks of `order`.
    pub fn bytes_at(&self, order: u8) -> u64 {
        self.orders
            .get(order as usize)
            .map_or(0, |stats| stats.bytes)
    }

    /// Bytes currently handed out.
    pub fn allocated_bytes(&self) -> u64 {
        self.total_bytes - self.free_bytes
    }

    /// Highest order that currently has a free block.
    pub fn largest_free_order(&self) -> Option<u8> {
        self.orders
            .iter()
            .rev()
            .find(|stats| stats.blocks > 0)
            .map(|stats| stats.order)
    }
}

#[derive(Clone, Copy)]
struct FreeBlock {
    addr: u64,
    order: u8,
}

impl FreeBlock {
    const EMPTY: Self = Self { addr: 0, order: 0 };
}

/// A buddy allocator over abstract physical frame ranges.
pub struct BuddyAllocator {
    blocks: [FreeBlock; MAX_FREE_BLOCKS],
    block_count: usize,
    ranges: [FrameRange; MAX_RANGES],
    range_count: usize,
    total_bytes: u64,
    free_bytes: u64,
}

impl BuddyAllocator {
    /// Creates an empty allocator with no ranges.
    pub const fn new() -> Self {
        Self {
            blocks: [FreeBlock::EMPTY; MAX_FREE_BLOCKS],
            block_count: 0,
            ranges: [FrameRange {
                base: PhysAddr::new(0),
                len: 0,
            }; MAX_RANGES],
            range_count: 0,
            total_bytes: 0,
            free_bytes: 0,
        }
    }

    /// Adds a frame range and turns its bytes into free buddy blocks.
    ///
    /// Ranges must not overlap; adjacent ranges stay separate, so blocks are
    /// never merged across a range boundary.
    pub fn add_range(&mut self, range: FrameRange) -> Result<(), MmError> {
        if self.range_count == MAX_RANGES {
            return Err(MmError::TooManyRanges);
        }
        // Every range adds at most ORDER_COUNT blocks; check once, so that a
        // failure cannot leave the allocator half-updated.
        if self.block_count + ORDER_COUNT > MAX_FREE_BLOCKS {
            return Err(MmError::TooManyFreeBlocks);
        }
        let start = range.base().as_u64();
        let end = range.end();
        for existing in &self.ranges[..self.range_count] {
            if start < existing.end() && existing.base().as_u64() < end {
                return Err(MmError::RangeOverlap);
            }
        }

        // Keep the range list sorted by base address.
        let mut pos = self.range_count;
        for (i, existing) in self.ranges[..self.range_count].iter().enumerate() {
            if start < existing.base().as_u64() {
                pos = i;
                break;
            }
        }
        let mut i = self.range_count;
        while i > pos {
            self.ranges[i] = self.ranges[i - 1];
            i -= 1;
        }
        self.ranges[pos] = range;
        self.range_count += 1;
        self.total_bytes += range.len();
        self.free_bytes += range.len();

        // Carve the range into maximal aligned power-of-two blocks.
        let mut cursor = start;
        let mut remaining = range.len();
        while remaining > 0 {
            let size = largest_aligned_block(cursor, remaining.min(MAX_BUDDY_BLOCK_SIZE));
            let order = match size_to_order(size) {
                Some(order) => order,
                None => return Err(MmError::InvalidAlignment),
            };
            self.insert_sorted(FreeBlock {
                addr: cursor,
                order,
            })?;
            cursor += size;
            remaining -= size;
        }
        Ok(())
    }

    /// Allocates one block of `order` (`4 KiB << order`) and returns its
    /// physical base address.
    pub fn alloc(&mut self, order: u8) -> Result<PhysAddr, MmError> {
        let size = match order_to_size(order) {
            Some(size) => size,
            None => return Err(MmError::InvalidOrder),
        };

        // Pick the smallest suitable order (lowest address on ties).
        let mut best: Option<usize> = None;
        for i in 0..self.block_count {
            let block = self.blocks[i];
            if block.order < order {
                continue;
            }
            match best {
                Some(j) if self.blocks[j].order <= block.order => {}
                _ => best = Some(i),
            }
        }
        let index = match best {
            Some(index) => index,
            None => return Err(MmError::OutOfMemory),
        };

        let candidate = self.blocks[index];
        let splits = (candidate.order - order) as usize;
        if self.block_count - 1 + splits > MAX_FREE_BLOCKS {
            return Err(MmError::TooManyFreeBlocks);
        }
        self.remove_at(index);

        let mut current_order = candidate.order;
        while current_order > order {
            current_order -= 1;
            let half = match order_to_size(current_order) {
                Some(size) => size,
                None => return Err(MmError::InvalidOrder),
            };
            self.insert_sorted(FreeBlock {
                addr: candidate.addr + half,
                order: current_order,
            })?;
        }

        self.free_bytes -= size;
        Ok(PhysAddr::new(candidate.addr))
    }

    /// Allocates `pages` contiguous pages. `pages` must be a power of two.
    pub fn alloc_pages(&mut self, pages: u64) -> Result<PhysAddr, MmError> {
        if pages == 0 {
            return Err(MmError::ZeroLength);
        }
        if !is_power_of_two(pages) {
            return Err(MmError::InvalidSize);
        }
        let size = pages.checked_mul(PAGE_SIZE).ok_or(MmError::InvalidSize)?;
        let order = size_to_order(size).ok_or(MmError::InvalidSize)?;
        self.alloc(order)
    }

    /// Frees a block returned by [`Self::alloc`].
    ///
    /// Merges with free buddies up to the range boundary. Freeing a block that
    /// is not allocated (including a double free) returns [`MmError::UnknownBlock`].
    pub fn free(&mut self, addr: PhysAddr, order: u8) -> Result<(), MmError> {
        let size = match order_to_size(order) {
            Some(size) => size,
            None => return Err(MmError::InvalidOrder),
        };
        let start = addr.as_u64();
        if !addr.is_aligned_to(size) {
            return Err(MmError::InvalidAlignment);
        }
        let range_index = self.range_index(start).ok_or(MmError::UnknownBlock)?;
        let range_end = self.ranges[range_index].end();
        match start.checked_add(size) {
            Some(end) if end <= range_end => {}
            _ => return Err(MmError::UnknownBlock),
        }
        if self.find_containing(start).is_some() {
            return Err(MmError::UnknownBlock);
        }

        // Plan the merges without touching the table, then apply them.
        let mut merged = [0u64; ORDER_COUNT];
        let mut merged_count = 0usize;
        let mut current_addr = start;
        let mut current_order = order;
        while current_order < MAX_BUDDY_ORDER {
            let block_size = match order_to_size(current_order) {
                Some(size) => size,
                None => return Err(MmError::InvalidOrder),
            };
            let buddy = current_addr ^ block_size;
            if !self.same_range(current_addr, buddy) {
                break;
            }
            match self.find_block(buddy) {
                Some(index) if self.blocks[index].order == current_order => {
                    merged[merged_count] = buddy;
                    merged_count += 1;
                    current_addr = current_addr.min(buddy);
                    current_order += 1;
                }
                _ => break,
            }
        }

        if self.block_count - merged_count + 1 > MAX_FREE_BLOCKS {
            return Err(MmError::TooManyFreeBlocks);
        }
        for buddy in &merged[..merged_count] {
            let index = self.find_block(*buddy).ok_or(MmError::UnknownBlock)?;
            self.remove_at(index);
        }
        self.insert_sorted(FreeBlock {
            addr: current_addr,
            order: current_order,
        })?;
        self.free_bytes += size;
        Ok(())
    }

    /// Frees `pages` pages previously allocated by [`Self::alloc_pages`].
    pub fn free_pages(&mut self, addr: PhysAddr, pages: u64) -> Result<(), MmError> {
        if pages == 0 {
            return Err(MmError::ZeroLength);
        }
        if !is_power_of_two(pages) {
            return Err(MmError::InvalidSize);
        }
        let size = pages.checked_mul(PAGE_SIZE).ok_or(MmError::InvalidSize)?;
        let order = size_to_order(size).ok_or(MmError::InvalidSize)?;
        self.free(addr, order)
    }

    /// Total bytes managed by the allocator.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Free bytes currently available.
    pub fn free_bytes(&self) -> u64 {
        self.free_bytes
    }

    /// Bytes currently handed out.
    pub fn allocated_bytes(&self) -> u64 {
        self.total_bytes - self.free_bytes
    }

    /// Number of free blocks in the internal table.
    pub fn free_block_count(&self) -> usize {
        self.block_count
    }

    /// Returns `true` if `addr` lies inside one of the configured ranges.
    pub fn contains(&self, addr: PhysAddr) -> bool {
        self.range_index(addr.as_u64()).is_some()
    }

    /// Configured ranges, sorted by base address.
    pub fn ranges(&self) -> &[FrameRange] {
        &self.ranges[..self.range_count]
    }

    /// Returns a statistics snapshot with per-order pools.
    pub fn stats(&self) -> Stats {
        let mut orders = [OrderStats::EMPTY; ORDER_COUNT];
        for (order, slot) in orders.iter_mut().enumerate() {
            slot.order = order as u8;
        }
        for block in &self.blocks[..self.block_count] {
            let slot = &mut orders[block.order as usize];
            slot.blocks += 1;
            slot.bytes += order_to_size(block.order).unwrap_or(0);
        }
        Stats {
            orders,
            total_bytes: self.total_bytes,
            free_bytes: self.free_bytes,
        }
    }

    fn insert_sorted(&mut self, block: FreeBlock) -> Result<(), MmError> {
        if self.block_count >= MAX_FREE_BLOCKS {
            return Err(MmError::TooManyFreeBlocks);
        }
        let mut lo = 0usize;
        let mut hi = self.block_count;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.blocks[mid].addr < block.addr {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo < self.block_count && self.blocks[lo].addr == block.addr {
            return Err(MmError::UnknownBlock);
        }
        let end = self.block_count;
        self.blocks.copy_within(lo..end, lo + 1);
        self.blocks[lo] = block;
        self.block_count += 1;
        Ok(())
    }

    fn remove_at(&mut self, index: usize) -> FreeBlock {
        let block = self.blocks[index];
        let end = self.block_count;
        self.blocks.copy_within(index + 1..end, index);
        self.block_count -= 1;
        block
    }

    fn find_block(&self, addr: u64) -> Option<usize> {
        let mut lo = 0usize;
        let mut hi = self.block_count;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.blocks[mid].addr < addr {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo < self.block_count && self.blocks[lo].addr == addr {
            Some(lo)
        } else {
            None
        }
    }

    fn find_containing(&self, addr: u64) -> Option<usize> {
        let mut lo = 0usize;
        let mut hi = self.block_count;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.blocks[mid].addr <= addr {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return None;
        }
        let index = lo - 1;
        let block = self.blocks[index];
        let size = order_to_size(block.order)?;
        if addr < block.addr + size {
            Some(index)
        } else {
            None
        }
    }

    fn range_index(&self, addr: u64) -> Option<usize> {
        let mut lo = 0usize;
        let mut hi = self.range_count;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.ranges[mid].base().as_u64() <= addr {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return None;
        }
        let index = lo - 1;
        if addr < self.ranges[index].end() {
            Some(index)
        } else {
            None
        }
    }

    fn same_range(&self, a: u64, b: u64) -> bool {
        match self.range_index(a) {
            Some(index) => b >= self.ranges[index].base().as_u64() && b < self.ranges[index].end(),
            None => false,
        }
    }
}

impl Default for BuddyAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Largest power-of-two block that is `addr`-aligned and no larger than
/// `max_size`. Both inputs are assumed to be 4 KiB aligned.
fn largest_aligned_block(addr: u64, max_size: u64) -> u64 {
    let mut size = PAGE_SIZE;
    while size * 2 <= max_size && addr & (size * 2 - 1) == 0 {
        size *= 2;
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::PAGE_MASK;
    use std::vec::Vec;

    fn allocator(base: u64, len: u64) -> BuddyAllocator {
        let mut allocator = BuddyAllocator::new();
        allocator
            .add_range(FrameRange::new(PhysAddr::new(base), len).expect("valid range"))
            .expect("range fits");
        allocator
    }

    fn assert_stats_consistent(allocator: &BuddyAllocator) {
        let stats = allocator.stats();
        let sum: u64 = stats.orders.iter().map(|order| order.bytes).sum();
        assert_eq!(
            sum, stats.free_bytes,
            "free bytes must equal the sum of per-order pools"
        );
        assert_eq!(
            stats.allocated_bytes(),
            stats.total_bytes - stats.free_bytes
        );
        let blocks: u32 = stats.orders.iter().map(|order| order.blocks).sum();
        assert_eq!(blocks as usize, allocator.free_block_count());
        for (index, block) in allocator.blocks[..allocator.block_count].iter().enumerate() {
            let size = order_to_size(block.order).expect("valid order");
            assert_eq!(
                block.addr & (size - 1),
                0,
                "free block must be aligned to its order"
            );
            assert_eq!(allocator.find_block(block.addr), Some(index));
            assert_eq!(allocator.find_containing(block.addr), Some(index));
        }
    }

    #[test]
    fn frame_range_validation() {
        assert_eq!(
            FrameRange::new(PhysAddr::new(0), 0),
            Err(MmError::ZeroLength)
        );
        assert_eq!(
            FrameRange::new(PhysAddr::new(1), PAGE_SIZE),
            Err(MmError::InvalidAlignment)
        );
        assert_eq!(
            FrameRange::new(PhysAddr::new(0), PAGE_SIZE + 1),
            Err(MmError::InvalidAlignment)
        );
        assert_eq!(
            FrameRange::new(PhysAddr::new(!PAGE_MASK), PAGE_SIZE),
            Err(MmError::AddressOverflow)
        );
        let range = FrameRange::new(PhysAddr::new(0x1000), 3 * PAGE_SIZE).expect("valid");
        assert_eq!(range.base(), PhysAddr::new(0x1000));
        assert_eq!(range.len(), 3 * PAGE_SIZE);
        assert_eq!(range.end(), 0x1000 + 3 * PAGE_SIZE);
        assert_eq!(range.pages(), 3);
        assert!(!range.is_empty());
    }

    #[test]
    fn default_allocator_is_empty() {
        let allocator = BuddyAllocator::default();
        assert_eq!(allocator.total_bytes(), 0);
        assert_eq!(allocator.free_block_count(), 0);
    }

    #[test]
    fn empty_allocator_has_no_memory() {
        let mut allocator = BuddyAllocator::new();
        assert_eq!(allocator.total_bytes(), 0);
        assert_eq!(allocator.free_bytes(), 0);
        assert_eq!(allocator.alloc(0), Err(MmError::OutOfMemory));
        let stats = allocator.stats();
        assert_eq!(stats.allocated_bytes(), 0);
        assert_eq!(stats.largest_free_order(), None);
        assert!(allocator.ranges().is_empty());
    }

    #[test]
    fn single_page_roundtrip() {
        let mut allocator = allocator(0x1000_0000, 64 * 1024 * 1024);
        let total = allocator.free_bytes();
        assert_eq!(total, 64 * 1024 * 1024);
        assert_eq!(allocator.stats().blocks_at(14), 1);
        assert_eq!(allocator.stats().bytes_at(14), 64 * 1024 * 1024);

        let addr = allocator.alloc(0).expect("page available");
        assert!(addr.is_page_aligned());
        assert_eq!(allocator.allocated_bytes(), PAGE_SIZE);
        assert_eq!(allocator.free_bytes(), total - PAGE_SIZE);
        assert_stats_consistent(&allocator);

        allocator.free(addr, 0).expect("block was allocated");
        assert_eq!(allocator.free_bytes(), total);
        assert_eq!(allocator.allocated_bytes(), 0);
        assert_eq!(allocator.free_block_count(), 1);
        assert_eq!(allocator.stats().largest_free_order(), Some(14));
        assert_stats_consistent(&allocator);
    }

    #[test]
    fn alloc_splits_and_free_coalesces() {
        let mut allocator = allocator(0, 1024 * 1024);
        let total = allocator.free_bytes();

        let addr = allocator.alloc(2).expect("16 KiB available");
        assert_eq!(addr, PhysAddr::new(0));
        assert_eq!(allocator.allocated_bytes(), 16 * 1024);
        // Splitting leaves the buddy half of every split order on the free list.
        assert_eq!(allocator.stats().blocks_at(2), 1);

        allocator.free(addr, 2).expect("block was allocated");
        assert_eq!(allocator.free_bytes(), total);
        assert_eq!(allocator.free_block_count(), 1);
        assert_eq!(allocator.stats().blocks_at(8), 1);
        assert_stats_consistent(&allocator);
    }

    #[test]
    fn allocate_every_page_and_return_them() {
        let mut allocator = allocator(0, 64 * 1024 * 1024);
        let total = allocator.free_bytes();
        let pages = 64 * 1024 * 1024 / PAGE_SIZE;

        let mut addrs = Vec::new();
        for _ in 0..pages {
            addrs.push(allocator.alloc(0).expect("page available"));
        }
        assert_eq!(allocator.free_bytes(), 0);
        assert_eq!(allocator.alloc(0), Err(MmError::OutOfMemory));
        assert_eq!(addrs.len(), pages as usize);

        for addr in addrs {
            allocator.free(addr, 0).expect("block was allocated");
        }
        assert_eq!(allocator.free_bytes(), total);
        assert_eq!(allocator.free_block_count(), 1);
        assert_eq!(allocator.stats().blocks_at(14), 1);
        assert_stats_consistent(&allocator);
    }

    #[test]
    fn thousands_of_alloc_and_free_leave_no_leak() {
        let mut allocator = allocator(0, 32 * 1024 * 1024);
        let total = allocator.free_bytes();
        let mut live: Vec<(PhysAddr, u8)> = Vec::new();
        let mut seed = 0x1234_5678_9abc_def0u64;

        for _ in 0..5000 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pages = 1u64 << ((seed >> 60) % 4);
            let order = size_to_order(pages * PAGE_SIZE).expect("power of two pages");
            match allocator.alloc(order) {
                Ok(addr) => {
                    assert!(addr.is_aligned_to(pages * PAGE_SIZE));
                    live.push((addr, order));
                }
                Err(MmError::OutOfMemory) => {
                    assert!(
                        !live.is_empty(),
                        "allocator must not report OOM while holding nothing"
                    );
                    let victim = (seed as usize) % live.len();
                    let (addr, order) = live.swap_remove(victim);
                    allocator.free(addr, order).expect("block was allocated");
                }
                Err(other) => panic!("unexpected error: {other}"),
            }
            assert_stats_consistent(&allocator);
        }

        for (addr, order) in live {
            allocator.free(addr, order).expect("block was allocated");
        }
        assert_eq!(allocator.free_bytes(), total);
        assert_eq!(allocator.free_block_count(), 1);
        assert_eq!(allocator.stats().largest_free_order(), Some(13));
        assert_stats_consistent(&allocator);
    }

    #[test]
    fn out_of_memory_is_reported() {
        let mut allocator = allocator(0, PAGE_SIZE);
        let addr = allocator.alloc(0).expect("one page");
        assert_eq!(allocator.alloc(0), Err(MmError::OutOfMemory));
        allocator.free(addr, 0).expect("block was allocated");
        assert_eq!(allocator.free_bytes(), PAGE_SIZE);
    }

    #[test]
    fn double_free_is_rejected() {
        let mut allocator = allocator(0, 64 * 1024);
        let addr = allocator.alloc(0).expect("page");
        allocator.free(addr, 0).expect("first free");
        assert_eq!(allocator.free(addr, 0), Err(MmError::UnknownBlock));
        // Freeing an address that lies inside a coalesced free block is also wrong.
        assert_eq!(
            allocator.free(PhysAddr::new(PAGE_SIZE), 0),
            Err(MmError::UnknownBlock)
        );
        assert_eq!(allocator.free_bytes(), 64 * 1024);
    }

    #[test]
    fn invalid_requests_are_rejected() {
        // 12 KiB decomposes into an 8 KiB and a 4 KiB block: no 16 KiB block exists.
        let mut pool = allocator(0, 12 * 1024);
        assert_eq!(pool.alloc(MAX_BUDDY_ORDER + 1), Err(MmError::InvalidOrder));
        assert_eq!(pool.alloc(255), Err(MmError::InvalidOrder));
        assert_eq!(pool.alloc(4), Err(MmError::OutOfMemory));

        assert_eq!(
            pool.free(PhysAddr::new(1), 0),
            Err(MmError::InvalidAlignment)
        );
        assert_eq!(pool.free(PhysAddr::new(0), 255), Err(MmError::InvalidOrder));
        // Outside every configured range.
        assert_eq!(
            pool.free(PhysAddr::new(0x1_0000), 0),
            Err(MmError::UnknownBlock)
        );
        // Block does not fit into the range.
        let mut small = allocator(0, PAGE_SIZE);
        assert_eq!(small.free(PhysAddr::new(0), 1), Err(MmError::UnknownBlock));
    }

    #[test]
    fn ranges_do_not_merge_across_boundaries() {
        let mut allocator = BuddyAllocator::new();
        allocator
            .add_range(FrameRange::new(PhysAddr::new(0), 64 * 1024).expect("range"))
            .expect("fits");
        allocator
            .add_range(FrameRange::new(PhysAddr::new(64 * 1024), 64 * 1024).expect("range"))
            .expect("fits");

        assert_eq!(allocator.stats().blocks_at(4), 2);
        assert_eq!(allocator.alloc(5), Err(MmError::OutOfMemory));

        let first = allocator.alloc(0).expect("page");
        let second = allocator.alloc(0).expect("page");
        allocator.free(first, 0).expect("block was allocated");
        allocator.free(second, 0).expect("block was allocated");
        assert_eq!(allocator.stats().blocks_at(4), 2);
        assert_stats_consistent(&allocator);
    }

    #[test]
    fn misaligned_range_is_split_into_aligned_blocks() {
        let mut allocator = allocator(0x1000, 12 * 1024);
        let stats = allocator.stats();
        assert_eq!(stats.blocks_at(0), 1);
        assert_eq!(stats.blocks_at(1), 1);
        assert_eq!(allocator.free_bytes(), 12 * 1024);

        let big = allocator.alloc(1).expect("8 KiB block");
        assert_eq!(big, PhysAddr::new(0x2000));
        let small = allocator.alloc(0).expect("4 KiB block");
        assert_eq!(small, PhysAddr::new(0x1000));
        allocator.free(big, 1).expect("block was allocated");
        allocator.free(small, 0).expect("block was allocated");
        assert_eq!(allocator.free_bytes(), 12 * 1024);
        assert_eq!(allocator.free_block_count(), 2);
        assert_stats_consistent(&allocator);
    }

    #[test]
    fn one_gib_block_is_representable() {
        let mut allocator = allocator(0, 1024 * 1024 * 1024);
        assert_eq!(allocator.stats().blocks_at(MAX_BUDDY_ORDER), 1);
        let addr = allocator.alloc(MAX_BUDDY_ORDER).expect("1 GiB block");
        assert_eq!(addr, PhysAddr::new(0));
        assert_eq!(allocator.allocated_bytes(), 1024 * 1024 * 1024);
        assert_eq!(allocator.alloc(0), Err(MmError::OutOfMemory));
        allocator
            .free(addr, MAX_BUDDY_ORDER)
            .expect("block was allocated");
        assert_eq!(allocator.free_bytes(), 1024 * 1024 * 1024);
        assert_eq!(allocator.stats().blocks_at(MAX_BUDDY_ORDER), 1);
    }

    #[test]
    fn alloc_pages_requires_power_of_two() {
        let mut allocator = allocator(0, 128 * 1024);
        assert_eq!(allocator.alloc_pages(0), Err(MmError::ZeroLength));
        assert_eq!(allocator.alloc_pages(3), Err(MmError::InvalidSize));
        let too_big = allocator.alloc_pages(MAX_BUDDY_BLOCK_SIZE / PAGE_SIZE * 2);
        assert_eq!(too_big, Err(MmError::InvalidSize));

        assert_eq!(
            allocator.free_pages(PhysAddr::new(0), 0),
            Err(MmError::ZeroLength)
        );
        assert_eq!(
            allocator.free_pages(PhysAddr::new(0), 3),
            Err(MmError::InvalidSize)
        );
        let addr = allocator.alloc_pages(8).expect("32 KiB available");
        assert!(addr.is_aligned_to(32 * 1024));
        assert_eq!(allocator.allocated_bytes(), 32 * 1024);
        allocator.free_pages(addr, 8).expect("block was allocated");
        assert_eq!(allocator.free_bytes(), 128 * 1024);
        assert_stats_consistent(&allocator);
    }

    #[test]
    fn add_range_rejects_overlap_and_overflow() {
        let mut allocator = allocator(0, 64 * 1024);
        assert_eq!(
            allocator.add_range(FrameRange::new(PhysAddr::new(0), PAGE_SIZE).expect("range")),
            Err(MmError::RangeOverlap)
        );
        assert_eq!(
            allocator
                .add_range(FrameRange::new(PhysAddr::new(32 * 1024), PAGE_SIZE).expect("range")),
            Err(MmError::RangeOverlap)
        );
        // Adjacent and non-overlapping is fine.
        allocator
            .add_range(FrameRange::new(PhysAddr::new(64 * 1024), PAGE_SIZE).expect("range"))
            .expect("adjacent range is fine");
        assert_eq!(allocator.ranges().len(), 2);
    }

    #[test]
    fn contains_and_sorted_ranges() {
        let mut allocator = BuddyAllocator::new();
        allocator
            .add_range(FrameRange::new(PhysAddr::new(0x8000), PAGE_SIZE).expect("range"))
            .expect("fits");
        allocator
            .add_range(FrameRange::new(PhysAddr::new(0), PAGE_SIZE).expect("range"))
            .expect("fits");
        let ranges = allocator.ranges();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].base(), PhysAddr::new(0));
        assert_eq!(ranges[1].base(), PhysAddr::new(0x8000));
        assert!(allocator.contains(PhysAddr::new(0)));
        assert!(allocator.contains(PhysAddr::new(0x8fff)));
        assert!(!allocator.contains(PhysAddr::new(0x1000)));
        assert!(!allocator.contains(PhysAddr::new(0x9000)));
    }
}
