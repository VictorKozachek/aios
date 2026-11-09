//! Physical frame allocator: the kernel side of `mmcore::buddy`.
//!
//! The allocator itself lives in `libs/mmcore` (task #014); this module only
//! feeds it the usable ranges from `BootInfo`, and guards the shared state.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use bootinfo::BootInfo;
use mmcore::{BuddyAllocator, FrameRange, MmError, PhysAddr, Stats};

/// A tiny spin lock. The kernel is single-threaded in M0, but a future CPU or
/// an interrupt must never observe a half-updated allocator.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is serialized by `locked`.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    pub fn with<R>(&self, body: impl FnOnce(&mut T) -> R) -> R {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: `locked` gives exclusive access for the duration of `body`.
        let result = body(unsafe { &mut *self.data.get() });
        self.locked.store(false, Ordering::Release);
        result
    }
}

static BUDDY: SpinLock<BuddyAllocator> = SpinLock::new(BuddyAllocator::new());

/// Feed every usable range from `BootInfo` to the buddy allocator.
///
/// Returns the number of ranges accepted. Ranges the library rejects (for
/// example a length that is not page-aligned) are skipped, not fatal: the
/// remaining memory is still usable.
pub fn init(boot: &BootInfo) -> Result<usize, MmError> {
    BUDDY.with(|buddy| {
        let mut accepted = 0;
        for entry in boot.memmap() {
            if !entry.kind.is_usable() || entry.length == 0 {
                continue;
            }
            if let Ok(range) = FrameRange::new(PhysAddr::new(entry.base), entry.length) {
                buddy.add_range(range)?;
                accepted += 1;
            }
        }
        Ok(accepted)
    })
}

/// Allocate `pages` contiguous 4 KiB pages (`pages` must be a power of two).
pub fn alloc_pages(pages: u64) -> Option<PhysAddr> {
    BUDDY.with(|buddy| buddy.alloc_pages(pages).ok())
}

/// Allocate one block of the given buddy order (`4 KiB << order`).
pub fn alloc_order(order: u8) -> Option<PhysAddr> {
    BUDDY.with(|buddy| buddy.alloc(order).ok())
}

/// Allocate a single 4 KiB page.
pub fn alloc_page() -> Option<PhysAddr> {
    alloc_pages(1)
}

/// Free a block previously returned by [`alloc_order`].
pub fn free_order(address: PhysAddr, order: u8) -> Result<(), MmError> {
    BUDDY.with(|buddy| buddy.free(address, order))
}

/// Allocator statistics (total/free bytes and per-order counters).
pub fn stats() -> Stats {
    BUDDY.with(|buddy| buddy.stats())
}
