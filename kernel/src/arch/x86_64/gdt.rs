//! Global Descriptor Table per CPU, segment reload and syscall MSR groundwork.
//!
//! Each CPU has its own GDT with a descriptor for its own TSS: `rsp0` and the
//! IST stacks are per-CPU. [`prepare`] fills the static tables (safe on the BSP
//! for every CPU); [`load`] performs the CPU-local operations (`lgdt`, far
//! return to reload CS, `ltr`) and must run on the target CPU.
//!
//! STAR/LSTAR are prepared here as groundwork for `SYSCALL`; EFER.SCE stays
//! disabled: there is no user mode and no syscall path yet, so a stray
//! `syscall` must trap instead of silently entering a placeholder.

use core::arch::{asm, global_asm};
use core::mem::size_of;

use crate::arch::x86_64::MAX_CPUS;
use crate::arch::x86_64::tss;

/// Kernel code selector (GDT index 1).
pub const KERNEL_CODE: u16 = 0x08;
/// Kernel data selector (GDT index 2).
pub const KERNEL_DATA: u16 = 0x10;
/// TSS selector (GDT index 3).
pub const TSS_SELECTOR: u16 = 0x18;

const GDT_ENTRIES: usize = 5;

/// 64-bit code segment: present, DPL0, executable/readable, long mode (L=1).
const CODE_SEGMENT: u64 = 0x00af_9a00_0000_ffff;
/// 64-bit data segment: present, DPL0, writable.
const DATA_SEGMENT: u64 = 0x00cf_9200_0000_ffff;

const MSR_STAR: u32 = 0xc000_0081;
const MSR_LSTAR: u32 = 0xc000_0082;

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

static mut GDT: [[u64; GDT_ENTRIES]; MAX_CPUS] = [[0; GDT_ENTRIES]; MAX_CPUS];

unsafe extern "C" {
    fn aios_reload_cs();
    fn aios_syscall_stub();
}

// Segment reload cannot be written in plain Rust (`mov cs` does not exist),
// so the far return lives in assembly.
global_asm!(
    r#"
    .global aios_reload_cs
aios_reload_cs:
    pop rax
    push 0x08
    push rax
    retfq

    .global aios_syscall_stub
aios_syscall_stub:
    cli
1:
    hlt
    jmp 1b
"#
);

/// Fill the static GDT/TSS tables for `cpu` (safe on the BSP for any CPU).
pub fn prepare(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    tss::init(cpu);

    let tss_base = tss::tss_ptr(cpu) as u64;
    let tss_limit = (size_of::<tss::Tss>() - 1) as u32;
    let (tss_low, tss_high) = tss_descriptor(tss_base, tss_limit);

    // SAFETY: `cpu` is bounds-checked; the table is static.
    unsafe {
        let gdt = &raw mut GDT;
        (*gdt)[cpu][0] = 0;
        (*gdt)[cpu][1] = CODE_SEGMENT;
        (*gdt)[cpu][2] = DATA_SEGMENT;
        (*gdt)[cpu][3] = tss_low;
        (*gdt)[cpu][4] = tss_high;
    }
}

/// CPU-local part: `lgdt`, reload CS/DS/ES/SS/FS/GS, then `ltr`.
pub fn load(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    // SAFETY: single-threaded per CPU; interrupts are disabled while the
    // tables are switched, and the static arrays outlive the CPU state.
    unsafe {
        let gdt = &raw mut GDT;
        let pointer = DescriptorPointer {
            limit: (GDT_ENTRIES * size_of::<u64>() - 1) as u16,
            base: (*gdt)[cpu].as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &pointer, options(nostack, preserves_flags));

        aios_reload_cs();

        asm!("mov ds, ax", in("ax") KERNEL_DATA, options(nostack, preserves_flags));
        asm!("mov es, ax", in("ax") KERNEL_DATA, options(nostack, preserves_flags));
        asm!("mov ss, ax", in("ax") KERNEL_DATA, options(nostack, preserves_flags));
        asm!(
            "xor eax, eax",
            "mov fs, ax",
            "mov gs, ax",
            out("eax") _,
            options(nostack, preserves_flags),
        );
        asm!("ltr ax", in("ax") TSS_SELECTOR, options(nostack, preserves_flags));
    }
}

/// BSP setup: GDT/TSS for CPU 0.
pub fn init() {
    prepare(0);
    load(0);
}

/// Two halves of a 64-bit TSS descriptor.
fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let low = (limit as u64 & 0xffff)
        | ((base & 0xff_ffff) << 16)
        // type = 0x9 (available 64-bit TSS), DPL = 0, present = 1
        | (0x89_u64 << 40)
        | (((limit as u64 >> 16) & 0xf) << 48)
        | (((base >> 24) & 0xff) << 56);
    let high = base >> 32;
    (low, high)
}

/// STAR/LSTAR groundwork for future `SYSCALL`; EFER.SCE is left disabled.
pub fn init_syscall_msrs() {
    // STAR[47:32] = SYSCALL CS/SS (kernel code/data); SYSRET fields stay zero
    // until user segments exist (M2+).
    let star = ((KERNEL_DATA as u64) << 48) | ((KERNEL_CODE as u64) << 32);
    wrmsr(MSR_STAR, star);
    // LSTAR points at a halt stub: placeholder until the syscall path exists.
    wrmsr(MSR_LSTAR, aios_syscall_stub as *const () as u64);
}

fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    // SAFETY: privileged MSR writes during kernel setup; MSR numbers and
    // values are architecture constants documented above.
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nostack, preserves_flags),
        );
    }
}
