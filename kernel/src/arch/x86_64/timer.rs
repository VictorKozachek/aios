//! LAPIC timer: vector 0x20, TSC calibration and the tick counter.
//!
//! The timer is the only interrupt source in M1a. It is programmed but left
//! masked until the scheduler is ready; [`start`] unmasks it and the caller
//! issues `sti` immediately after.
//!
//! Frequency: the LAPIC timer counts at `bus / divide`; the bus frequency is
//! not discoverable, so it is calibrated against the TSC (`tsc_hz` from
//! `BootInfo`). If the TSC frequency is unknown, a PIT-channel-2 reference is
//! used instead.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::exceptions::InterruptFrame;
use crate::arch::x86_64::{apic, idt, rdtsc};
use crate::io;
use crate::sched;

/// Interrupt vector used by the LAPIC timer.
pub const TIMER_VECTOR: u8 = 0x20;
/// Scheduler tick rate.
pub const TICKS_PER_SECOND: u64 = 100;

/// Ticks since boot (relaxed counter, read by the idle thread).
static TICKS: AtomicU64 = AtomicU64::new(0);
/// Calibrated LAPIC count rate with the configured divider.
static TIMER_HZ: AtomicU64 = AtomicU64::new(0);

global_asm!(
    r#"
    .global aios_timer_stub
aios_timer_stub:
    push 0
    push 0x20
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rdi, rsp
    and rsp, -16
    call aios_timer_handler
    mov rsp, rax

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    iretq
"#
);

unsafe extern "C" {
    fn aios_timer_stub();
}

/// Timer entry: acknowledge the LAPIC, count the tick, ask the scheduler for
/// the next context (the stub switches to the returned frame and `iretq`s).
#[unsafe(no_mangle)]
pub extern "C" fn aios_timer_handler(frame: *mut InterruptFrame) -> *mut InterruptFrame {
    apic::eoi();
    TICKS.fetch_add(1, Ordering::Relaxed);
    crate::smp::this_cpu()
        .ticks
        .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    sched::on_timer(frame)
}

/// Ticks since boot.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Install the timer gate, calibrate the LAPIC and program the periodic timer
/// (masked; [`start`] unmasks it).
pub fn init(tsc_hz: Option<u64>) {
    idt::install(
        TIMER_VECTOR as usize,
        aios_timer_stub as *const () as u64,
        0,
    );

    apic::enable();
    let lapic_hz = match tsc_hz.filter(|hz| *hz != 0) {
        Some(tsc_hz) => calibrate_with_tsc(tsc_hz),
        None => calibrate_with_pit(),
    };
    TIMER_HZ.store(lapic_hz, Ordering::Release);

    let initial = (lapic_hz / TICKS_PER_SECOND).clamp(1, u32::MAX as u64) as u32;
    apic::timer_periodic_start(TIMER_VECTOR, initial);
    apic::timer_mask(true);
    crate::println!(
        "timer: LAPIC {} Hz (divide {}), {} Hz ticks, initial {}",
        lapic_hz,
        apic::TIMER_DIVIDE,
        TICKS_PER_SECOND,
        initial,
    );
}

/// Unmask the timer; the caller must issue `sti` immediately afterwards.
pub fn start() {
    apic::timer_mask(false);
}

/// LAPIC count rate with the configured divider (0 if not calibrated).
pub fn lapic_hz() -> u64 {
    TIMER_HZ.load(Ordering::Relaxed)
}

const CALIBRATION_COUNTS: u32 = 10_000_000;

fn calibrate_with_tsc(tsc_hz: u64) -> u64 {
    apic::timer_one_shot_masked(u32::MAX);
    let start_tsc = rdtsc();
    // Do not spin forever if the LAPIC counter does not move (broken QoS):
    // give it 200 ms of TSC time, then fall back to zero (ticks become slow
    // but the kernel still boots and reports the failure).
    let max_tsc = (tsc_hz / 5).max(1);
    let start_count = apic::timer_current();
    while start_count.wrapping_sub(apic::timer_current()) < CALIBRATION_COUNTS {
        crate::arch::x86_64::pause();
        if rdtsc().saturating_sub(start_tsc) > max_tsc {
            crate::println!("timer: LAPIC calibration timed out (counter stuck?)");
            return 0;
        }
    }
    let elapsed_tsc = rdtsc().saturating_sub(start_tsc);
    if elapsed_tsc == 0 {
        return 0;
    }
    (CALIBRATION_COUNTS as u64).saturating_mul(tsc_hz) / elapsed_tsc
}

/// Fallback: measure LAPIC counts over a 10 ms PIT-channel-2 interval.
fn calibrate_with_pit() -> u64 {
    // 10 ms at 1.193182 MHz = 11932 PIT ticks.
    const PIT_DIVISOR: u16 = 11_932;
    // Gate channel 2 off, speaker data off.
    // SAFETY: legacy PIT/speaker ports; this runs once during boot with
    // interrupts disabled and does not touch any other device state.
    let value = unsafe { io::inb(0x61) };
    unsafe {
        io::outb(0x61, value & 0xfc);
        // Channel 2, mode 0 (interrupt on terminal count), LSB then MSB.
        io::outb(0x43, 0xb0);
        io::outb(0x42, PIT_DIVISOR as u8);
        io::outb(0x42, (PIT_DIVISOR >> 8) as u8);
    }

    apic::timer_one_shot_masked(u32::MAX);
    let before = apic::timer_current();
    // Gate on; OUT2 goes high on terminal count.
    // SAFETY: same as above.
    unsafe {
        io::outb(0x61, (value & 0xfc) | 0x01);
    }
    let mut guard = 0u32;
    // SAFETY: reading the PIT status port.
    while unsafe { io::inb(0x61) } & 0x20 == 0 {
        crate::arch::x86_64::pause();
        guard += 1;
        if guard > 100_000_000 {
            crate::println!("timer: PIT calibration timed out");
            return 0;
        }
    }
    let after = apic::timer_current();
    before.wrapping_sub(after) as u64 * 100 // per second
}
