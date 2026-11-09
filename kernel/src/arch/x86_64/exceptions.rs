//! CPU exceptions: entry stubs, the interrupt frame, the dump and the halt.
//!
//! Rust stable has no `extern "x86-interrupt"` ABI, so the entry stubs are
//! written in assembly: each stub pushes a dummy error code (the CPU only
//! pushes one for vectors 8, 10–14, 17, 21, 29, 30) and the vector number, then
//! jumps to the common stub that saves the general-purpose registers and calls
//! [`aios_exception_handler`].
//!
//! Interrupts stay disabled: vectors 32..255 have no real handlers yet, so the
//! kernel never executes `sti`.

use core::arch::{asm, global_asm};

use crate::io;

/// Sentinel pushed by the generic stub for vectors 32..255 (not a real vector).
pub const UNEXPECTED_VECTOR: u64 = 0xffff;

global_asm!(
    r#"
.macro AIOS_EXC_NOERR num
    .global aios_exc_stub_\num
aios_exc_stub_\num:
    push 0
    push \num
    jmp aios_exc_common
.endm

.macro AIOS_EXC_ERR num
    .global aios_exc_stub_\num
aios_exc_stub_\num:
    push \num
    jmp aios_exc_common
.endm

AIOS_EXC_NOERR 0
AIOS_EXC_NOERR 1
AIOS_EXC_NOERR 2
AIOS_EXC_NOERR 3
AIOS_EXC_NOERR 4
AIOS_EXC_NOERR 5
AIOS_EXC_NOERR 6
AIOS_EXC_NOERR 7
AIOS_EXC_ERR   8
AIOS_EXC_NOERR 9
AIOS_EXC_ERR   10
AIOS_EXC_ERR   11
AIOS_EXC_ERR   12
AIOS_EXC_ERR   13
AIOS_EXC_ERR   14
AIOS_EXC_NOERR 15
AIOS_EXC_NOERR 16
AIOS_EXC_ERR   17
AIOS_EXC_NOERR 18
AIOS_EXC_NOERR 19
AIOS_EXC_NOERR 20
AIOS_EXC_ERR   21
AIOS_EXC_NOERR 22
AIOS_EXC_NOERR 23
AIOS_EXC_NOERR 24
AIOS_EXC_NOERR 25
AIOS_EXC_NOERR 26
AIOS_EXC_NOERR 27
AIOS_EXC_NOERR 28
AIOS_EXC_ERR   29
AIOS_EXC_ERR   30
AIOS_EXC_NOERR 31

.global aios_exc_stub_unexpected
aios_exc_stub_unexpected:
    push 0
    push 0xffff
    jmp aios_exc_common

.global aios_exc_stub_table
aios_exc_stub_table:
    .quad aios_exc_stub_0
    .quad aios_exc_stub_1
    .quad aios_exc_stub_2
    .quad aios_exc_stub_3
    .quad aios_exc_stub_4
    .quad aios_exc_stub_5
    .quad aios_exc_stub_6
    .quad aios_exc_stub_7
    .quad aios_exc_stub_8
    .quad aios_exc_stub_9
    .quad aios_exc_stub_10
    .quad aios_exc_stub_11
    .quad aios_exc_stub_12
    .quad aios_exc_stub_13
    .quad aios_exc_stub_14
    .quad aios_exc_stub_15
    .quad aios_exc_stub_16
    .quad aios_exc_stub_17
    .quad aios_exc_stub_18
    .quad aios_exc_stub_19
    .quad aios_exc_stub_20
    .quad aios_exc_stub_21
    .quad aios_exc_stub_22
    .quad aios_exc_stub_23
    .quad aios_exc_stub_24
    .quad aios_exc_stub_25
    .quad aios_exc_stub_26
    .quad aios_exc_stub_27
    .quad aios_exc_stub_28
    .quad aios_exc_stub_29
    .quad aios_exc_stub_30
    .quad aios_exc_stub_31

aios_exc_common:
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
    call aios_exception_handler
    ud2
"#
);

unsafe extern "C" {
    static aios_exc_stub_table: [u64; 32];
    fn aios_exc_stub_unexpected();
}

/// Address of the entry stub for `vector` (0..31).
pub fn stub_address(vector: u8) -> u64 {
    // SAFETY: the table is defined in the assembly above with exactly 32
    // entries, one per exception vector.
    unsafe { aios_exc_stub_table[vector as usize] }
}

/// Address of the generic stub used for vectors 32..255.
pub fn unexpected_stub_address() -> u64 {
    aios_exc_stub_unexpected as *const () as u64
}

/// Interrupt frame as built by the stubs (low addresses first).
#[repr(C)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// Exception entry point: dump the frame to every console channel and halt.
#[unsafe(no_mangle)]
pub extern "C" fn aios_exception_handler(frame: *const InterruptFrame) -> ! {
    // SAFETY: the stubs build the frame in exactly this layout before calling.
    let frame = unsafe { &*frame };
    let cr2 = read_cr2();

    crate::println!();
    if frame.vector == UNEXPECTED_VECTOR {
        crate::println!("EXCEPTION: unexpected interrupt (vector not configured)");
    } else {
        crate::println!(
            "EXCEPTION {} (vector {}) error=0x{:016x} rip=0x{:016x}",
            vector_name(frame.vector),
            frame.vector,
            frame.error_code,
            frame.rip,
        );
    }
    match crate::arch::x86_64::tss::ist_index_for_stack_pointer(current_stack_pointer()) {
        Some(index) => crate::println!("  stack: IST{index} (own exception stack)"),
        None => crate::println!("  stack: kernel stack (no IST for this vector)"),
    }
    crate::println!(
        "  cs=0x{:04x} rflags=0x{:016x} rsp=0x{:016x} ss=0x{:04x} cr2=0x{:016x}",
        frame.cs,
        frame.rflags,
        frame.rsp,
        frame.ss,
        cr2,
    );
    crate::println!(
        "  rax={:016x} rbx={:016x} rcx={:016x} rdx={:016x}",
        frame.rax,
        frame.rbx,
        frame.rcx,
        frame.rdx,
    );
    crate::println!(
        "  rsi={:016x} rdi={:016x} rbp={:016x} rsp={:016x}",
        frame.rsi,
        frame.rdi,
        frame.rbp,
        frame.rsp,
    );
    crate::println!(
        "  r8={:016x} r9={:016x} r10={:016x} r11={:016x}",
        frame.r8,
        frame.r9,
        frame.r10,
        frame.r11,
    );
    crate::println!(
        "  r12={:016x} r13={:016x} r14={:016x} r15={:016x}",
        frame.r12,
        frame.r13,
        frame.r14,
        frame.r15,
    );
    crate::println!("--- kernel halted ---");
    io::halt_forever();
}

fn vector_name(vector: u64) -> &'static str {
    match vector {
        0 => "#DE",
        1 => "#DB",
        2 => "NMI",
        3 => "#BP",
        4 => "#OF",
        5 => "#BR",
        6 => "#UD",
        7 => "#NM",
        8 => "#DF",
        10 => "#TS",
        11 => "#NP",
        12 => "#SS",
        13 => "#GP",
        14 => "#PF",
        16 => "#MF",
        17 => "#AC",
        18 => "#MC",
        19 => "#XM",
        20 => "#VE",
        21 => "#CP",
        28 => "#HV",
        29 => "#VC",
        30 => "#SX",
        _ => "vector",
    }
}

fn current_stack_pointer() -> u64 {
    let value: u64;
    // SAFETY: reads rsp only.
    unsafe {
        asm!("mov {}, rsp", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: CR2 is readable from ring 0 and only holds the faulting address.
    unsafe {
        asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Trigger an exception when the kernel is built with `AIOS_EXC_TEST`.
///
/// Used for the T3 smoke test without touching the normal boot path:
///
/// ```text
/// AIOS_EXC_TEST=int3 QEMU_TIMEOUT=50 make qemu   # EXCEPTION #BP ...
/// AIOS_EXC_TEST=div0 QEMU_TIMEOUT=50 make qemu   # EXCEPTION #DE
/// AIOS_EXC_TEST=pf   QEMU_TIMEOUT=50 make qemu   # EXCEPTION #PF, stack: IST3
/// ```
///
/// The variable must be set for the whole command: `make qemu` depends on
/// `make image`, and cargo tracks `option_env!`, so a separate rebuild without
/// the variable would silently produce a normal kernel.
///
/// `option_env!` is compile-time: rebuild the kernel after changing the value.
pub fn self_test_from_env() {
    match option_env!("AIOS_EXC_TEST") {
        Some("int3") => unsafe {
            asm!("int3", options(nomem, nostack, preserves_flags));
        },
        Some("ud2") => unsafe {
            asm!("ud2", options(nomem, nostack, preserves_flags));
        },
        Some("pf") => unsafe {
            // Canonical but unmapped address: a data read must raise #PF with
            // cr2 equal to the address. #PF has its own IST stack, so this
            // also demonstrates the IST wiring from the TSS.
            core::ptr::read_volatile(0xffff_9000_1000_0000_u64 as *const u8);
        },
        Some("div0") => unsafe {
            // A real hardware #DE. Rust integer division panics ("attempt to
            // divide by zero") instead of trapping, so issue `div` directly:
            // rdx:rax = 1, divisor rcx = 0.
            asm!(
                "xor rdx, rdx",
                "mov rax, 1",
                "div rcx",
                in("rcx") 0u64,
                out("rax") _,
                out("rdx") _,
                options(nostack),
            );
        },
        _ => {}
    }
}
