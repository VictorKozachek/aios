//! Panic handler: reason, location and a register dump, then halt.
//!
//! By the time M0 can panic the console (serial + debugcon + framebuffer) is
//! alive, so the dump is visible both on screen and in the logs. The kernel
//! halts afterwards: without an IDT (T3) there is nowhere to return to, and a
//! silent loop would look like a hang instead of a crash.

use core::arch::asm;

use crate::io;

/// Panic: print the reason, the source location and a register dump, then halt.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let registers = Registers::capture();

    crate::println!();
    crate::println!("PANIC: {}", info);
    crate::println!(
        "  rax={:016x} rbx={:016x} rcx={:016x} rdx={:016x}",
        registers.rax,
        registers.rbx,
        registers.rcx,
        registers.rdx,
    );
    crate::println!(
        "  rsi={:016x} rdi={:016x} rbp={:016x} rsp={:016x}",
        registers.rsi,
        registers.rdi,
        registers.rbp,
        registers.rsp,
    );
    crate::println!(
        "  r8={:016x} r9={:016x} r10={:016x} r11={:016x}",
        registers.r8,
        registers.r9,
        registers.r10,
        registers.r11,
    );
    crate::println!(
        "  r12={:016x} r13={:016x} r14={:016x} r15={:016x}",
        registers.r12,
        registers.r13,
        registers.r14,
        registers.r15,
    );
    crate::println!(
        "  rip={:016x} rflags={:016x} cr2={:016x}",
        registers.rip,
        registers.rflags,
        registers.cr2,
    );
    crate::println!("--- kernel halted ---");
    io::halt_forever();
}

/// Snapshot of the general-purpose registers at the panic site.
#[derive(Default)]
struct Registers {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    rbp: u64,
    rsp: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rip: u64,
    rflags: u64,
    cr2: u64,
}

impl Registers {
    /// Read the registers into a snapshot.
    ///
    /// The values are those of the panic entry point (after the compiler has
    /// already called the handler), not of the faulting instruction: without
    /// an IDT (T3) the original frame is gone. The source location from
    /// `PanicInfo` still points at the panic site.
    fn capture() -> Self {
        let mut registers = Self::default();
        // SAFETY: reads CPU registers only; no memory is dereferenced.
        //
        // Split into three blocks: one asm! with all 19 outputs runs out of
        // registers ("inline assembly requires more registers than available").
        // The split is harmless for a crash dump: values may already be
        // clobbered by the panic machinery anyway.
        unsafe {
            asm!(
                "mov {rax}, rax",
                "mov {rbx}, rbx",
                "mov {rcx}, rcx",
                "mov {rdx}, rdx",
                "mov {rsi}, rsi",
                "mov {rdi}, rdi",
                "mov {rbp}, rbp",
                "mov {rsp}, rsp",
                rax = out(reg) registers.rax,
                rbx = out(reg) registers.rbx,
                rcx = out(reg) registers.rcx,
                rdx = out(reg) registers.rdx,
                rsi = out(reg) registers.rsi,
                rdi = out(reg) registers.rdi,
                rbp = out(reg) registers.rbp,
                rsp = out(reg) registers.rsp,
            );
            asm!(
                "mov {r8}, r8",
                "mov {r9}, r9",
                "mov {r10}, r10",
                "mov {r11}, r11",
                "mov {r12}, r12",
                "mov {r13}, r13",
                "mov {r14}, r14",
                "mov {r15}, r15",
                r8 = out(reg) registers.r8,
                r9 = out(reg) registers.r9,
                r10 = out(reg) registers.r10,
                r11 = out(reg) registers.r11,
                r12 = out(reg) registers.r12,
                r13 = out(reg) registers.r13,
                r14 = out(reg) registers.r14,
                r15 = out(reg) registers.r15,
            );
            asm!(
                "mov {cr2}, cr2",
                "lea {rip}, [rip]",
                "pushfq",
                "pop {rflags}",
                rip = out(reg) registers.rip,
                rflags = out(reg) registers.rflags,
                cr2 = out(reg) registers.cr2,
            );
        }
        registers
    }
}
