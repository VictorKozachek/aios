//! Kernel heap: a linked-list allocator over one contiguous buddy region.
//!
//! The region itself comes from `mmcore::buddy` (via [`super::phys`]); the
//! allocator keeps a free list of in-place blocks, splits on allocation and
//! coalesces neighbours on free. It is deliberately small: the kernel does not
//! use `alloc` yet, so this is the single place where dynamic memory appears.

use crate::mm::phys::SpinLock;

/// Alignment of every returned pointer.
const ALIGN: usize = 16;
/// In-place block header: payload size and the next free block.
const HEADER: usize = size_of::<Block>();
/// A block is split only if the remainder can hold a header plus a minimal payload.
const MIN_BLOCK: usize = HEADER + ALIGN;

#[repr(C)]
struct Block {
    /// Usable payload size of this block in bytes.
    size: usize,
    /// Next free block, or null.
    next: *mut Block,
}

struct HeapState {
    start: usize,
    size: usize,
    free_head: *mut Block,
    free_bytes: usize,
}

// SAFETY: the heap is only touched through the `SpinLock` below.
unsafe impl Send for HeapState {}

impl HeapState {
    const fn new() -> Self {
        Self {
            start: 0,
            size: 0,
            free_head: core::ptr::null_mut(),
            free_bytes: 0,
        }
    }
}

static HEAP: SpinLock<HeapState> = SpinLock::new(HeapState::new());

/// Lay out one free block over the whole region.
pub fn init(start: u64, size: u64) {
    let start = start as usize;
    let size = size as usize;
    if size < HEADER + ALIGN {
        return;
    }
    HEAP.with(|heap| {
        // SAFETY: the region is a dedicated, writable, NX mapping owned by us.
        unsafe {
            let first = start as *mut Block;
            first.write(Block {
                size: size - HEADER,
                next: core::ptr::null_mut(),
            });
            heap.start = start;
            heap.size = size;
            heap.free_head = first;
            heap.free_bytes = size;
        }
    });
}

/// Allocate `size` bytes; returns a 16-byte aligned pointer or `None`.
pub fn alloc(size: usize) -> Option<*mut u8> {
    if size == 0 {
        return None;
    }
    let needed = (size + ALIGN - 1) & !(ALIGN - 1);
    HEAP.with(|heap| {
        // SAFETY: all block pointers come from `init`/`alloc` and point inside
        // the region; accesses are serialized by the lock.
        unsafe {
            let mut previous: *mut Block = core::ptr::null_mut();
            let mut current = heap.free_head;
            while !current.is_null() {
                let block = &mut *current;
                if block.size >= needed {
                    let remainder = block.size - needed;
                    if remainder >= MIN_BLOCK {
                        let split = (current as usize + HEADER + needed) as *mut Block;
                        split.write(Block {
                            size: remainder - HEADER,
                            next: block.next,
                        });
                        block.size = needed;
                        if previous.is_null() {
                            heap.free_head = split;
                        } else {
                            (*previous).next = split;
                        }
                    } else if previous.is_null() {
                        heap.free_head = block.next;
                    } else {
                        (*previous).next = block.next;
                    }
                    heap.free_bytes -= needed + HEADER;
                    return Some((current as usize + HEADER) as *mut u8);
                }
                previous = current;
                current = block.next;
            }
            None
        }
    })
}

/// Return a pointer previously obtained from [`alloc`].
pub fn dealloc(pointer: *mut u8) {
    if pointer.is_null() {
        return;
    }
    HEAP.with(|heap| {
        // SAFETY: `pointer - HEADER` is the block header written by `alloc`.
        unsafe {
            let block = (pointer as usize - HEADER) as *mut Block;
            let size = (*block).size;
            heap.free_bytes += size + HEADER;

            // Insert into the address-ordered free list.
            let mut previous: *mut Block = core::ptr::null_mut();
            let mut current = heap.free_head;
            while !current.is_null() && (current as usize) < (block as usize) {
                previous = current;
                current = (*current).next;
            }
            (*block).next = current;
            if previous.is_null() {
                heap.free_head = block;
            } else {
                (*previous).next = block;
            }

            // Coalesce with the following block.
            if !current.is_null() && (block as usize + HEADER + (*block).size) == current as usize {
                (*block).size += HEADER + (*current).size;
                (*block).next = (*current).next;
            }
            // Coalesce with the preceding block.
            if !previous.is_null()
                && (previous as usize + HEADER + (*previous).size) == block as usize
            {
                (*previous).size += HEADER + (*block).size;
                (*previous).next = (*block).next;
            }
        }
    });
}

/// Small end-to-end check: three allocations, pattern writes, frees, and the
/// free counter must return to its start.
pub fn self_check() -> bool {
    let total = total_bytes();
    let Some(small) = alloc(32) else {
        return false;
    };
    let Some(medium) = alloc(128) else {
        return false;
    };
    let Some(page) = alloc(4096) else {
        return false;
    };
    // SAFETY: blocks were just allocated with exactly these sizes.
    unsafe {
        core::ptr::write_bytes(small, 0xaa, 32);
        core::ptr::write_bytes(medium, 0xbb, 128);
        core::ptr::write_bytes(page, 0xcc, 4096);
    }
    dealloc(small);
    dealloc(medium);
    dealloc(page);
    free_bytes() == total
}

/// Total heap size in bytes.
pub fn total_bytes() -> u64 {
    HEAP.with(|heap| heap.size as u64)
}

/// Free heap bytes (headers included: that is what "free in the region" means).
pub fn free_bytes() -> u64 {
    HEAP.with(|heap| heap.free_bytes as u64)
}
