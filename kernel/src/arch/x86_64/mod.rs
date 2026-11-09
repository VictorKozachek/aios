//! x86-64 CPU primitives: GDT/TSS, IDT and exception handlers (task #004, T3).
//!
//! Interrupts stay disabled on purpose: there are no handlers for vectors 32+
//! yet (they arrive with the scheduler), so `sti` is never issued here.

pub mod exceptions;
pub mod gdt;
pub mod idt;
pub mod tss;

/// Install GDT/TSS and IDT, then run the optional exception self-test.
///
/// Call once, early in `_start`, right after the early console is up.
pub fn init() {
    gdt::init();
    idt::init();
    gdt::init_syscall_msrs();
    exceptions::self_test_from_env();
}
