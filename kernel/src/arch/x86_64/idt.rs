//! Interrupt Descriptor Table: 256 gates.
//!
//! Vectors 0..31 are CPU exceptions with real stubs; double fault, NMI and
//! page fault use their own IST stacks. Vectors 32..255 are not implemented in
//! M0 (interrupts stay disabled), so they point at one generic stub that
//! reports an unexpected interrupt instead of triple-faulting.

use core::arch::asm;
use core::mem::size_of;

use crate::arch::x86_64::{exceptions, tss};

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
            selector: 0x08,
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

/// Fill and load the IDT.
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

        let pointer = DescriptorPointer {
            limit: (IDT_ENTRIES * size_of::<IdtEntry>() - 1) as u16,
            base: (*idt).as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &pointer, options(nostack, preserves_flags));
    }
}
