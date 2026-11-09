//! Interrupt Descriptor Table: 256 gates.
//!
//! Vectors 0..31 are CPU exceptions with real stubs; double fault, NMI and
//! page fault use their own IST stacks. Vectors 32..255 point at one generic
//! stub that reports an unexpected interrupt instead of triple-faulting.
//!
//! The table is built once on the BSP and shared: every CPU loads it with
//! [`load`]. Gates for the timer and the spurious vector are installed later
//! with [`install`] (before interrupts are enabled).

use core::arch::asm;
use core::mem::size_of;

use crate::arch::x86_64::{exceptions, gdt, tss};

const IDT_ENTRIES: usize = 256;
const PRESENT_INTERRUPT_GATE: u8 = 0x8e;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    const EMPTY: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        zero: 0,
    };

    const fn new(offset: u64, ist: u8) -> Self {
        Self {
            offset_low: offset as u16,
            selector: gdt::KERNEL_CODE,
            ist,
            type_attr: PRESENT_INTERRUPT_GATE,
            offset_mid: (offset >> 16) as u16,
            offset_high: (offset >> 32) as u32,
            zero: 0,
        }
    }
}

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::EMPTY; IDT_ENTRIES];

/// Build the table and load it on the current CPU.
pub fn init() {
    // SAFETY: single-threaded boot code, interrupts are disabled; the table is
    // a static array that outlives the CPU state.
    unsafe {
        let idt = &raw mut IDT;
        for vector in 0..IDT_ENTRIES {
            let ist = match vector {
                2 => tss::IST_NMI,
                8 => tss::IST_DOUBLE_FAULT,
                14 => tss::IST_PAGE_FAULT,
                _ => 0,
            };
            let offset = if vector < 32 {
                exceptions::stub_address(vector as u8)
            } else {
                exceptions::unexpected_stub_address()
            };
            (*idt)[vector] = IdtEntry::new(offset, ist);
        }
    }
    load();
}

/// Load the shared IDT on the current CPU (`lidt`).
pub fn load() {
    // SAFETY: the table is a static array; the pointer stays valid.
    unsafe {
        let pointer = DescriptorPointer {
            limit: (IDT_ENTRIES * size_of::<IdtEntry>() - 1) as u16,
            base: (&raw const IDT).cast::<IdtEntry>() as u64,
        };
        asm!("lidt [{}]", in(reg) &pointer, options(nostack, preserves_flags));
    }
}

/// Install a gate for `vector` (for example the LAPIC timer or spurious IRQ).
///
/// Must be called after [`init`] and before interrupts are enabled.
pub fn install(vector: usize, offset: u64, ist: u8) {
    if vector >= IDT_ENTRIES || offset == 0 {
        return;
    }
    // SAFETY: bounds checked; the table is a static array.
    unsafe {
        (*core::ptr::addr_of_mut!(IDT))
            .get_unchecked_mut(vector)
            .clone_from(&IdtEntry::new(offset, ist));
    }
}
