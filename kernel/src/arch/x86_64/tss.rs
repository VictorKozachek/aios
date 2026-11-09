//! Task State Segment per CPU: `rsp0` and IST stacks.
//!
//! Every CPU needs its own TSS: `rsp0` and the IST stacks are per-CPU
//! resources. The GDT of each CPU therefore contains a descriptor pointing at
//! its own TSS (see `gdt.rs`). IST (Interrupt Stack Table) stacks keep critical
//! exceptions from running on a corrupt kernel stack: double fault, NMI and
//! page fault each get their own 8 KiB stack per CPU.

use core::mem::size_of;

use crate::arch::x86_64::MAX_CPUS;

/// IST index for double fault (`#DF`, vector 8).
pub const IST_DOUBLE_FAULT: u8 = 1;
/// IST index for NMI (vector 2).
pub const IST_NMI: u8 = 2;
/// IST index for page fault (`#PF`, vector 14).
pub const IST_PAGE_FAULT: u8 = 3;

const IST_STACK_SIZE: usize = 8 * 1024;

/// A stack with 16-byte alignment: the ABI and our exception stubs expect it.
///
/// The contents are never read: only the address (the stack top) matters.
#[repr(align(16))]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Stack([u8; IST_STACK_SIZE]);

static mut IST_DOUBLE_FAULT_STACK: [Stack; MAX_CPUS] = [Stack([0; IST_STACK_SIZE]); MAX_CPUS];
static mut IST_NMI_STACK: [Stack; MAX_CPUS] = [Stack([0; IST_STACK_SIZE]); MAX_CPUS];
static mut IST_PAGE_FAULT_STACK: [Stack; MAX_CPUS] = [Stack([0; IST_STACK_SIZE]); MAX_CPUS];
static mut RSP0_STACK: [Stack; MAX_CPUS] = [Stack([0; IST_STACK_SIZE]); MAX_CPUS];

/// 64-bit Task State Segment (packed: the hardware layout has no padding).
#[repr(C, packed)]
pub struct Tss {
    reserved0: u32,
    pub rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    pub ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl Tss {
    const EMPTY: Self = Self {
        reserved0: 0,
        rsp0: 0,
        rsp1: 0,
        rsp2: 0,
        reserved1: 0,
        ist: [0; 7],
        reserved2: 0,
        reserved3: 0,
        iomap_base: 0,
    };
}

pub static mut TSS: [Tss; MAX_CPUS] = [Tss::EMPTY; MAX_CPUS];

/// Fill the TSS of `cpu`: stack tops for `rsp0` and IST1/2/3, no I/O bitmap.
///
/// Safe to call on the BSP for every CPU before that CPU starts: only the
/// static tables are written, the CPU-visible state changes in `gdt::load`.
pub fn init(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let double_fault = stack_top(&raw mut IST_DOUBLE_FAULT_STACK as *mut u8, cpu);
    let nmi = stack_top(&raw mut IST_NMI_STACK as *mut u8, cpu);
    let page_fault = stack_top(&raw mut IST_PAGE_FAULT_STACK as *mut u8, cpu);
    let rsp0 = stack_top(&raw mut RSP0_STACK as *mut u8, cpu);

    // SAFETY: `cpu` is bounds-checked; the arrays are static and outlive the
    // CPU state. The TSS becomes visible only via `ltr` in `gdt::load`.
    unsafe {
        let tss = &raw mut TSS;
        (*tss)[cpu].rsp0 = rsp0;
        (*tss)[cpu].ist[(IST_DOUBLE_FAULT - 1) as usize] = double_fault;
        (*tss)[cpu].ist[(IST_NMI - 1) as usize] = nmi;
        (*tss)[cpu].ist[(IST_PAGE_FAULT - 1) as usize] = page_fault;
        // iomap_base past the limit means "no I/O permission bitmap".
        (*tss)[cpu].iomap_base = size_of::<Tss>() as u16;
    }
}

/// Pointer to the TSS of `cpu` (for the GDT descriptor).
pub fn tss_ptr(cpu: usize) -> *mut Tss {
    if cpu >= MAX_CPUS {
        return core::ptr::null_mut();
    }
    // SAFETY: the array is static; the pointer is only used to build a GDT
    // descriptor, never to create a long-lived `&mut`.
    unsafe { (&raw mut TSS).cast::<Tss>().add(cpu) }
}

fn stack_top(base: *mut u8, cpu: usize) -> u64 {
    base as u64 + (cpu * IST_STACK_SIZE + IST_STACK_SIZE) as u64
}

/// Which IST stack (of any CPU) contains `pointer` (1..=3), if any.
///
/// An exception handler can prove it runs on an IST stack by checking its own
/// `rsp` against these ranges; `AIOS_EXC_TEST=pf` prints exactly that.
pub fn ist_index_for_stack_pointer(pointer: u64) -> Option<u8> {
    let stacks = [
        (
            IST_DOUBLE_FAULT,
            &raw const IST_DOUBLE_FAULT_STACK as *const u8 as u64,
        ),
        (IST_NMI, &raw const IST_NMI_STACK as *const u8 as u64),
        (
            IST_PAGE_FAULT,
            &raw const IST_PAGE_FAULT_STACK as *const u8 as u64,
        ),
    ];
    for (index, base) in stacks {
        for cpu in 0..MAX_CPUS {
            let start = base + (cpu * IST_STACK_SIZE) as u64;
            if pointer >= start && pointer < start + IST_STACK_SIZE as u64 {
                return Some(index);
            }
        }
    }
    None
}
