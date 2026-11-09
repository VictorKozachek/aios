//! x86-64 CPU primitives: GDT/TSS, IDT, exceptions, LAPIC and the timer.
//!
//! Interrupts are enabled only when the scheduler starts: the IDT, the LAPIC
//! timer and the scheduler must all be ready before the first `sti`.

use core::arch::asm;

pub mod apic;
pub mod exceptions;
pub mod gdt;
pub mod idt;
pub mod timer;
pub mod tss;

/// Maximum number of CPUs the kernel supports (per-CPU tables are sized to it).
pub const MAX_CPUS: usize = 8;

/// Install GDT/TSS and IDT, then run the optional exception self-test.
///
/// Call once, early in `_start`, right after the early console is up.
pub fn init() {
    gdt::init();
    idt::init();
    gdt::init_syscall_msrs();
    exceptions::self_test_from_env();
}

/// Mask interrupts on the current CPU.
#[inline]
pub fn disable_interrupts() {
    // SAFETY: `cli` is always allowed at ring 0.
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

/// Idle until the next interrupt: `sti; hlt`.
#[inline]
pub fn idle_once() {
    // SAFETY: `hlt` wakes on an interrupt or NMI; `sti` must precede it so the
    // CPU does not sleep forever.
    unsafe {
        asm!("sti", "hlt", options(nomem, nostack, preserves_flags));
    }
}

/// Read the timestamp counter.
#[inline]
pub fn rdtsc() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: RDTSC is readable from ring 0 and has no side effects.
    unsafe {
        asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | low as u64
}

/// Spin-wait hint.
#[inline]
pub fn pause() {
    // SAFETY: PAUSE has no side effects.
    unsafe {
        asm!("pause", options(nomem, nostack, preserves_flags));
    }
}
