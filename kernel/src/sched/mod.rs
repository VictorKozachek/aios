//! Scheduler: kernel threads, round-robin on the timer tick and the idle loop.
//!
//! Context switching happens on the timer interrupt frame: the timer stub saves
//! `rax..r15` plus the CPU frame (`rip/cs/rflags/rsp/ss`) and calls
//! [`on_timer`], which saves the current thread's frame pointer, picks the next
//! runnable thread and returns its frame. The stub then switches `rsp` to that
//! frame, restores the registers and `iretq`s into the thread.
//!
//! Thread stacks are buddy-allocated pages addressed through the HHDM. They are
//! not NX yet (Limine maps the HHDM as RWX); mapping them NX is a follow-up for
//! the memory subsystem.
//!
//! TODO(M1b): map thread stacks NX once the mm subsystem grows exception-safe
//! NX mapping of stack windows. Until then M1a runs with a temporary W^X
//! relaxation: stacks are RWX (see the card report).

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use bootinfo::BootInfo;

use crate::arch::x86_64::exceptions::InterruptFrame;
use crate::arch::x86_64::{self as arch, gdt, timer};
use crate::io;
use crate::mm::phys;

/// Thread slots (idle included).
pub const MAX_THREADS: usize = 8;
/// Per-thread stack: 16 pages = 64 KiB.
const STACK_PAGES: u64 = 16;
const STACK_BYTES: u64 = STACK_PAGES * 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unused,
    Ready,
    Running,
    Finished,
}

#[allow(dead_code)] // name/stack_base are kept for diagnostics (ps-like dumps)
struct Thread {
    name: &'static str,
    state: State,
    /// Saved frame pointer (`rsp` at the interrupt frame), 0 if never ran.
    frame: u64,
    /// HHDM address of the stack base (0 for the boot/idle thread).
    stack_base: u64,
    iterations: AtomicU64,
}

impl Thread {
    // Empty slot for the static array; the interior mutability (iterations)
    // is fine because the constant is only used to initialise statics.
    #[allow(clippy::declare_interior_mutable_const)]
    const EMPTY: Self = Self {
        name: "",
        state: State::Unused,
        frame: 0,
        stack_base: 0,
        iterations: AtomicU64::new(0),
    };
}

static mut THREADS: [Thread; MAX_THREADS] = [const { Thread::EMPTY }; MAX_THREADS];
static mut CURRENT: usize = 0;
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static mut HHDM: u64 = 0;
static mut TSC_HZ: u64 = 0;

unsafe extern "C" {
    fn aios_thread_start();
}

global_asm!(
    r#"
    .global aios_thread_start
aios_thread_start:
    and rsp, -16
    mov rax, rdi
    call rax
    call aios_thread_exit
    ud2
"#
);

/// Prepare the idle thread (the boot context) and remember the HHDM/TSC data.
pub fn init(boot: &BootInfo) {
    // SAFETY: single-threaded boot code, interrupts are disabled.
    unsafe {
        HHDM = boot.hhdm.unwrap_or(0);
        TSC_HZ = boot.tsc_hz.unwrap_or(0);
        let threads = &raw mut THREADS;
        (*threads)[0] = Thread {
            name: "idle",
            state: State::Running,
            frame: 0,
            stack_base: 0,
            iterations: AtomicU64::new(0),
        };
        CURRENT = 0;
    }
    crate::println!("sched: idle thread ready, {} slots", MAX_THREADS);
}

/// Spawn a kernel thread running `entry`; returns its slot.
pub fn spawn(name: &'static str, entry: extern "C" fn()) -> Option<usize> {
    let hhdm = unsafe { HHDM };
    if hhdm == 0 {
        crate::println!("sched: no HHDM, cannot allocate a stack for '{name}'");
        return None;
    }
    let physical = phys::alloc_pages(STACK_PAGES)?;
    let stack_base = hhdm + physical.as_u64();
    let stack_top = (stack_base + STACK_BYTES) & !0xf;
    let frame = build_frame(stack_top, entry);

    // SAFETY: interrupts are disabled by the caller (boot code) or the timer
    // handler never observes a half-initialised slot because it runs on the
    // same CPU with interrupts off while we write.
    let slot = unsafe {
        let threads = &raw mut THREADS;
        let mut found = None;
        for index in 1..MAX_THREADS {
            if (*threads)[index].state == State::Unused {
                found = Some(index);
                break;
            }
        }
        let slot = found?;
        (*threads)[slot] = Thread {
            name,
            state: State::Ready,
            frame: frame as u64,
            stack_base,
            iterations: AtomicU64::new(0),
        };
        slot
    };
    crate::println!(
        "sched: spawn #{} '{}' stack 0x{:016x}+{} KiB",
        slot,
        name,
        stack_base,
        STACK_BYTES / 1024,
    );
    Some(slot)
}

/// Fabricate an interrupt frame that starts `entry` via [`aios_thread_start`].
fn build_frame(stack_top: u64, entry: extern "C" fn()) -> *mut InterruptFrame {
    let size = core::mem::size_of::<InterruptFrame>() as u64;
    let frame_address = (stack_top - size) & !0xf;
    let frame = frame_address as *mut InterruptFrame;
    // SAFETY: the frame lies inside the freshly allocated thread stack.
    unsafe {
        core::ptr::write_bytes(frame as *mut u8, 0, size as usize);
        let frame = &mut *frame;
        frame.rip = aios_thread_start as *const () as u64;
        // The trampoline calls the entry point passed in `rdi`.
        frame.rdi = entry as *const () as u64;
        frame.cs = gdt::KERNEL_CODE as u64;
        frame.ss = gdt::KERNEL_DATA as u64;
        frame.rflags = 0x202; // IF | reserved bit 1
        frame.rsp = stack_top;
    }
    frame
}

/// Timer entry: save the interrupted frame, pick the next thread, return its
/// frame (the stub switches to it and `iretq`s).
pub fn on_timer(frame: *mut InterruptFrame) -> *mut InterruptFrame {
    let current = unsafe { CURRENT };
    unsafe {
        (*core::ptr::addr_of_mut!(THREADS))[current].frame = frame as u64;
    }

    let mut next = current;
    for step in 1..=MAX_THREADS {
        let candidate = (current + step) % MAX_THREADS;
        // SAFETY: index is modulo MAX_THREADS.
        let state = unsafe { (*core::ptr::addr_of!(THREADS))[candidate].state };
        if state == State::Ready || state == State::Running {
            next = candidate;
            break;
        }
    }

    if next != current {
        SWITCHES.fetch_add(1, Ordering::Relaxed);
    }
    // SAFETY: both indices are < MAX_THREADS.
    unsafe {
        let threads = &raw mut THREADS;
        if (*threads)[current].state == State::Running {
            (*threads)[current].state = State::Ready;
        }
        (*threads)[next].state = State::Running;
        CURRENT = next;
        (*threads)[next].iterations.fetch_add(1, Ordering::Relaxed);
        (*threads)[next].frame as *mut InterruptFrame
    }
}

/// A thread function returned: mark it finished and let the timer switch away.
#[unsafe(no_mangle)]
pub extern "C" fn aios_thread_exit() -> ! {
    let current = unsafe { CURRENT };
    // SAFETY: CURRENT is a valid slot.
    unsafe {
        (*core::ptr::addr_of_mut!(THREADS))[current].state = State::Finished;
    }
    loop {
        arch::idle_once();
    }
}

/// Number of context switches.
pub fn switches() -> u64 {
    SWITCHES.load(Ordering::Relaxed)
}

/// Start the scheduler: unmask the timer, enable interrupts and run the idle
/// loop. Never returns.
pub fn start() -> ! {
    // Two demo kernel threads run by default so that a plain `make qemu`
    // demonstrates preemption (switches > 0 and growing counters); the
    // AIOS_SMP_TEST switch only adds the verdict and stops the VM.
    spawn("demo-a", demo_thread_a);
    spawn("demo-b", demo_thread_b);
    let test_mode = option_env!("AIOS_SMP_TEST") == Some("1");

    // Smoke contract (changed in #016): the marker is the prefix `boot: ok`;
    // the boot stage itself no longer halts, the scheduler keeps running.
    crate::println!("boot: ok");
    crate::println!(
        "boot: scheduler running, threads={}, ticks={}, timer={} Hz",
        thread_count(),
        timer::ticks(),
        timer::TICKS_PER_SECOND,
    );

    timer::start();
    let tsc_hz = unsafe { TSC_HZ };
    let tsc_start = arch::rdtsc();
    let tick_start = timer::ticks();
    let switch_start = switches();
    let mut reported_1s = false;
    let mut reported_5s = false;

    loop {
        let elapsed = arch::rdtsc().saturating_sub(tsc_start);
        if !reported_1s && tsc_hz > 0 && elapsed >= tsc_hz {
            let ticks = timer::ticks().saturating_sub(tick_start);
            crate::println!(
                "timer: {} ticks/s (expected {}), LAPIC {} Hz",
                ticks,
                timer::TICKS_PER_SECOND,
                timer::lapic_hz(),
            );
            reported_1s = true;
        }
        if !reported_5s && tsc_hz > 0 && elapsed >= tsc_hz.saturating_mul(5) {
            let ticks = timer::ticks().saturating_sub(tick_start);
            let switched = switches().saturating_sub(switch_start);
            crate::println!(
                "sched: 5s: ticks={}, switches={} ({} switches/s), threads={}",
                ticks,
                switched,
                switched / 5,
                thread_count(),
            );
            let (a, b) = demo_counters();
            crate::println!("sched: demo threads: A iterations={} B iterations={}", a, b);
            if test_mode {
                crate::println!(
                    "smp-test: 5s: thread A iterations={} thread B iterations={}",
                    a,
                    b
                );
                if a > 0 && b > 0 {
                    crate::println!("smp-test: OK — both threads made progress");
                } else {
                    crate::println!("smp-test: FAIL — a={} b={}", a, b);
                }
                // Stop the world for a deterministic test run.
                arch::disable_interrupts();
                io::halt_forever();
            }
            reported_5s = true;
        }
        arch::idle_once();
    }
}

fn thread_count() -> usize {
    let mut count = 0;
    for index in 0..MAX_THREADS {
        // SAFETY: index < MAX_THREADS.
        let state = unsafe { (*core::ptr::addr_of!(THREADS))[index].state };
        if state != State::Unused {
            count += 1;
        }
    }
    count
}

// --- Demo kernel threads (and the optional AIOS_SMP_TEST verdict) ----------

static DEMO_A: AtomicU64 = AtomicU64::new(0);
static DEMO_B: AtomicU64 = AtomicU64::new(0);

extern "C" fn demo_thread_a() {
    loop {
        DEMO_A.fetch_add(1, Ordering::Relaxed);
        arch::pause();
    }
}

extern "C" fn demo_thread_b() {
    loop {
        DEMO_B.fetch_add(1, Ordering::Relaxed);
        arch::pause();
    }
}

fn demo_counters() -> (u64, u64) {
    (
        DEMO_A.load(Ordering::Relaxed),
        DEMO_B.load(Ordering::Relaxed),
    )
}
