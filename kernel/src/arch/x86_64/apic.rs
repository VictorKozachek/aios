//! Local APIC driver (xAPIC, memory-mapped).
//!
//! The MP request asks Limine for xAPIC (`flags = 0`); the memory subsystem
//! already maps the LAPIC page at `hhdm + 0xFEE0_0000` as uncached (T4b), so
//! the driver only needs that offset. x2APIC (MSR based) is a later task.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::idt;

/// Physical address of the local APIC page (xAPIC).
const LAPIC_PHYS: u64 = 0xfee0_0000;

/// Spurious interrupt vector (delivered when the LAPIC has no pending vector).
pub const SPURIOUS_VECTOR: u8 = 0xFF;

const REG_TPR: u32 = 0x80;
const REG_EOI: u32 = 0xb0;
const REG_SVR: u32 = 0xf0;
const REG_LVT_TIMER: u32 = 0x320;
const REG_TIMER_INIT: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3e0;

const SVR_ENABLE: u32 = 0x1ff; // APIC software enable | spurious vector 0xFF
const LVT_MASKED: u32 = 1 << 16;
const LVT_PERIODIC: u32 = 1 << 17;
/// LAPIC timer divide configuration for /16.
const DIVIDE_16: u32 = 0b0011;
/// How many LAPIC ticks one period of the divided timer counts.
pub const TIMER_DIVIDE: u32 = 16;

static BASE: AtomicU64 = AtomicU64::new(0);

global_asm!(
    r#"
    .global aios_spurious_stub
aios_spurious_stub:
    iretq
"#
);

unsafe extern "C" {
    fn aios_spurious_stub();
}

fn base() -> u64 {
    BASE.load(Ordering::Relaxed)
}

fn read(offset: u32) -> u32 {
    // SAFETY: the LAPIC page is mapped uncached; offsets are register numbers.
    unsafe { core::ptr::read_volatile((base() + offset as u64) as *const u32) }
}

fn write(offset: u32, value: u32) {
    // SAFETY: see `read`.
    unsafe { core::ptr::write_volatile((base() + offset as u64) as *mut u32, value) };
}

/// Map the LAPIC at `hhdm + LAPIC_PHYS`, install the spurious gate and enable
/// the APIC on the current CPU.
pub fn init(hhdm_offset: u64) {
    if BASE.load(Ordering::Relaxed) == 0 {
        BASE.store(hhdm_offset + LAPIC_PHYS, Ordering::Release);
        idt::install(
            SPURIOUS_VECTOR as usize,
            aios_spurious_stub as *const () as u64,
            0,
        );
    }
    enable();
}

/// Enable the LAPIC on the current CPU and mask its timer until asked.
pub fn enable() {
    write(REG_SVR, SVR_ENABLE);
    write(REG_TPR, 0);
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_TIMER_DIVIDE, DIVIDE_16);
}

/// Acknowledge the in-service interrupt.
pub fn eoi() {
    write(REG_EOI, 0);
}

/// Program the timer as a masked one-shot (used for calibration).
pub fn timer_one_shot_masked(initial: u32) {
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_TIMER_INIT, initial);
}

/// Program the timer as a periodic interrupt source (`vector`, unmasked).
pub fn timer_periodic_start(vector: u8, initial: u32) {
    write(REG_TIMER_DIVIDE, DIVIDE_16);
    write(REG_LVT_TIMER, vector as u32 | LVT_PERIODIC | LVT_MASKED);
    write(REG_TIMER_INIT, initial);
    // Unmask only now: with IF=0 no interrupt can be delivered yet, and the
    // caller issues `sti` as the very next step.
    let lvt = read(REG_LVT_TIMER);
    write(REG_LVT_TIMER, lvt & !LVT_MASKED);
}

/// Current timer count.
pub fn timer_current() -> u32 {
    read(REG_TIMER_CURRENT)
}

/// Mask (true) or unmask (false) the timer interrupt.
pub fn timer_mask(masked: bool) {
    let lvt = read(REG_LVT_TIMER);
    write(
        REG_LVT_TIMER,
        if masked {
            lvt | LVT_MASKED
        } else {
            lvt & !LVT_MASKED
        },
    );
}
