//! Memory management: physical frames, page tables and the kernel heap.
//!
//! The pure logic lives in `libs/mmcore` (task #014); this module integrates it
//! with the machine: `BootInfo` ranges, CR3, PAT and one heap region.

pub mod heap;
pub mod phys;
pub mod tests;
pub mod vm;

use bootinfo::BootInfo;

const MIB: u64 = 1024 * 1024;

/// Initialize the buddy allocator, our page tables, the heap and run the
/// optional memory smoke tests (`AIOS_MEM_TEST`).
pub fn init(boot: &BootInfo) {
    let ranges = match phys::init(boot) {
        Ok(ranges) => ranges,
        Err(error) => {
            crate::println!("FATAL: physical allocator: {}", error);
            panic!("physical allocator init failed");
        }
    };
    let stats = phys::stats();
    crate::println!(
        "mm: buddy: {} usable range(s), total {} MiB, free {} MiB",
        ranges,
        stats.total_bytes / MIB,
        stats.free_bytes / MIB,
    );

    if let Err(error) = vm::init(boot) {
        crate::println!("FATAL: page tables: {}", error);
        panic!("page table init failed");
    }

    crate::println!(
        "mm: heap self-check: {}",
        if heap::self_check() { "OK" } else { "FAIL" }
    );

    let stats = phys::stats();
    crate::println!(
        "memory: total {} MiB, free {} MiB, heap {} KiB",
        stats.total_bytes / MIB,
        stats.free_bytes / MIB,
        heap::total_bytes() / 1024,
    );

    tests::run();
}
