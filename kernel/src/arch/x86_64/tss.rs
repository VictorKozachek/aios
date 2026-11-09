//! Task State Segment: `rsp0` and the IST stacks.
//!
//! IST (Interrupt Stack Table) stacks keep critical exceptions from running on
//! a corrupt kernel stack: double fault, NMI and page fault each get their own
//! 8 KiB stack. `rsp0` is used for interrupts coming from ring 3; M0 has no
//! ring 3 yet, but the field is filled as the architecture requires.

use core::mem::size_of;

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
#[allow(dead_code)]
struct Stack([u8; IST_STACK_SIZE]);

static mut IST_DOUBLE_FAULT_STACK: Stack = Stack([0; IST_STACK_SIZE]);
static mut IST_NMI_STACK: Stack = Stack([0; IST_STACK_SIZE]);
static mut IST_PAGE_FAULT_STACK: Stack = Stack([0; IST_STACK_SIZE]);
static mut RSP0_STACK: Stack = Stack([0; IST_STACK_SIZE]);

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

pub static mut TSS: Tss = Tss {
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

/// Fill the TSS: stack tops for `rsp0` and IST1/2/3, no I/O permission bitmap.
pub fn init() {
    let rsp0 = stack_top(&raw mut RSP0_STACK as *mut u8);
    let double_fault = stack_top(&raw mut IST_DOUBLE_FAULT_STACK as *mut u8);
    let nmi = stack_top(&raw mut IST_NMI_STACK as *mut u8);
    let page_fault = stack_top(&raw mut IST_PAGE_FAULT_STACK as *mut u8);

    // SAFETY: single-threaded boot code; the TSS is not visible to the CPU
    // until `ltr` in `gdt::init` runs after this function.
    unsafe {
        let tss = &raw mut TSS;
        (*tss).rsp0 = rsp0;
        (*tss).ist[(IST_DOUBLE_FAULT - 1) as usize] = double_fault;
        (*tss).ist[(IST_NMI - 1) as usize] = nmi;
        (*tss).ist[(IST_PAGE_FAULT - 1) as usize] = page_fault;
        // iomap_base past the limit means "no I/O permission bitmap".
        (*tss).iomap_base = size_of::<Tss>() as u16;
    }
}

fn stack_top(base: *mut u8) -> u64 {
    base as u64 + IST_STACK_SIZE as u64
}

/// Which IST stack contains `pointer` (1..=3), if any.
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
        if pointer >= base && pointer < base + IST_STACK_SIZE as u64 {
            return Some(index);
        }
    }
    None
}
