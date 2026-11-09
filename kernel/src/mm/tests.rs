//! Memory smoke tests, enabled at compile time like the T3 exception tests.
//!
//! - `AIOS_MEM_TEST=1` — allocate 64 MiB from the buddy allocator in 2 MiB
//!   blocks, free them and check that the free counter returns to its start;
//! - `AIOS_MEM_TEST=wx` — execute bytes from the heap: the page is RW+NX, so
//!   the CPU must raise `#PF` (the T3 handler prints `cr2` and halts).
//!
//! Both are `option_env!` based: the value is baked into the kernel at build
//! time, so rebuild between runs (`make image` does that, cargo tracks it).

use mmcore::{PAGE_SIZE, PhysAddr};

use crate::mm::{heap, phys};

/// Buddy order used by the allocator test: 2 MiB blocks.
const ORDER: u8 = 9;
/// 32 blocks of 2 MiB = 64 MiB.
const BLOCKS: usize = 32;
const MIB: u64 = 1024 * 1024;

/// Run the test selected by `AIOS_MEM_TEST`, if any.
pub fn run() {
    match option_env!("AIOS_MEM_TEST") {
        Some("1") => alloc_free_test(),
        Some("wx") => wx_test(),
        _ => {}
    }
}

fn alloc_free_test() {
    let before = phys::stats();
    let mut addresses = [PhysAddr::new(0); BLOCKS];
    let mut allocated = 0usize;

    for slot in addresses.iter_mut() {
        match phys::alloc_order(ORDER) {
            Some(address) => {
                *slot = address;
                allocated += 1;
            }
            None => break,
        }
    }

    let after_alloc = phys::stats();
    for address in addresses.iter().take(allocated) {
        let _ = phys::free_order(*address, ORDER);
    }
    let after_free = phys::stats();

    let block_bytes = PAGE_SIZE << ORDER;
    let restored = after_free.free_bytes == before.free_bytes;
    let ok = restored && allocated == BLOCKS;

    crate::println!(
        "mem-test: 1: allocated {} x {} KiB = {} MiB",
        allocated,
        block_bytes / 1024,
        (allocated as u64 * block_bytes) / MIB,
    );
    crate::println!(
        "mem-test: 1: free before {} MiB, after alloc {} MiB, after free {} MiB, restored={}",
        before.free_bytes / MIB,
        after_alloc.free_bytes / MIB,
        after_free.free_bytes / MIB,
        restored,
    );
    crate::println!("mem-test: 1: {}", if ok { "OK" } else { "FAIL" });
}

fn wx_test() {
    let Some(code) = heap::alloc(64) else {
        crate::println!("mem-test: wx: FAIL — heap allocation failed");
        return;
    };

    // SAFETY: the heap block is 64 writable bytes; we fill it with `ret`
    // (0xC3) and then try to execute it. The page is mapped NX, so the CPU
    // raises #PF instead of running the code.
    unsafe {
        core::ptr::write_bytes(code, 0xc3, 64);
    }

    crate::println!(
        "mem-test: wx: executing 64 bytes at 0x{:016x} (heap is RW+NX, expect #PF)",
        code as u64,
    );
    let entry: extern "C" fn() = unsafe { core::mem::transmute(code) };
    entry();

    crate::println!("mem-test: wx: FAIL — execution from the heap did not fault");
}
