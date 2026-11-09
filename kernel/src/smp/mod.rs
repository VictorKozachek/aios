//! SMP bring-up through the Limine MP response.
//!
//! Limine starts every AP and parks it; writing the entry point to
//! `goto_address` (release) releases that CPU with a stack provided by the
//! bootloader and `RDI = *limine_mp_info`. Each AP then installs its own
//! GDT/TSS (per-CPU `rsp0` and IST stacks), loads the shared IDT, enables its
//! LAPIC and reports online. The BSP waits for all APs with a TSC timeout.
//!
//! Interrupts on APs are enabled only after their LAPIC/IDT are ready; the
//! only vector they can receive in M1a is the spurious 0xFF (no timer).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bootinfo::BootInfo;

use crate::arch::x86_64::{self as arch, apic, gdt, idt};
use crate::boot::limine::{self, MpInfo};

/// Per-CPU state. Reached through `gs:[0]`-style helpers: the GS base of each
/// CPU points at its own `PerCpu`.
#[allow(dead_code)] // ticks is incremented by the timer handler
pub struct PerCpu {
    pub cpu_id: u32,
    pub lapic_id: u32,
    pub online: AtomicBool,
    pub ticks: AtomicU64,
}

impl PerCpu {
    const fn new(cpu_id: u32) -> Self {
        Self {
            cpu_id,
            lapic_id: 0,
            online: AtomicBool::new(false),
            ticks: AtomicU64::new(0),
        }
    }
}

static mut PER_CPU: [PerCpu; MAX_CPUS] = [const { PerCpu::new(0) }; MAX_CPUS];

/// APs that have finished their bring-up (BSP not counted).
static AP_ONLINE: AtomicU32 = AtomicU32::new(0);

const MSR_GS_BASE: u32 = 0xc000_0101;

/// Bring up the BSP per-CPU state and release all APs.
pub fn init(boot: &BootInfo) {
    let Some(mp) = limine::mp() else {
        crate::println!("smp: no MP response, running single-CPU");
        setup_bsp(0);
        return;
    };
    let cpus = limine::mp_cpus().unwrap_or(&[]);
    let count = core::cmp::min(cpus.len(), MAX_CPUS);

    // Fill per-CPU tables and GDT/TSS for every CPU on the BSP.
    for (index, info) in cpus.iter().enumerate().take(count) {
        // SAFETY: index < MAX_CPUS.
        unsafe {
            let per_cpu = &raw mut PER_CPU;
            (*per_cpu)[index].lapic_id = info.lapic_id;
            (*per_cpu)[index].cpu_id = index as u32;
        }
        gdt::prepare(index);
    }

    setup_bsp(0);
    let hhdm = boot.hhdm.unwrap_or(0);
    apic::init(hhdm);
    crate::println!(
        "smp: xAPIC mode, BSP lapic id {}, {} CPU(s) reported",
        mp.bsp_lapic_id,
        count,
    );

    // Release the APs: everything they read must be initialised before the
    // release store to `goto_address`.
    let mut expected = 1u32;
    for info in cpus.iter().take(count).skip(1) {
        let info = *info as *const MpInfo as *mut MpInfo;
        // SAFETY: the bootloader owns the structure but guarantees that a
        // writable `goto_address` is the release mechanism. The extra argument
        // is not used: the AP finds its slot by lapic id.
        unsafe {
            let slot = core::ptr::addr_of_mut!((*info).goto_address) as *const AtomicU64;
            (*slot).store(aios_ap_main as *const () as u64, Ordering::Release);
        }
        expected += 1;
    }

    // Wait for the APs with a 2 s TSC timeout (~generous under TCG).
    let deadline = arch::rdtsc().saturating_add(boot.tsc_hz.unwrap_or(0).saturating_mul(2));
    while AP_ONLINE.load(Ordering::Acquire) + 1 < expected && arch::rdtsc() < deadline {
        arch::pause();
    }
    let online = AP_ONLINE.load(Ordering::Acquire) + 1;
    crate::println!(
        "smp: {}/{} CPUs online (BSP lapic id {})",
        online,
        count,
        mp.bsp_lapic_id,
    );
    if online < count as u32 {
        for index in 0..count {
            // SAFETY: index < MAX_CPUS.
            let per_cpu = unsafe { &*core::ptr::addr_of!(PER_CPU).cast::<PerCpu>().add(index) };
            if !per_cpu.online.load(Ordering::Relaxed) {
                crate::println!(
                    "smp: CPU {} (lapic {}) did not come online",
                    index,
                    per_cpu.lapic_id
                );
            }
        }
    }
}

fn setup_bsp(index: usize) {
    // SAFETY: index < MAX_CPUS; the BSP is the current CPU.
    unsafe {
        let per_cpu = &raw mut PER_CPU;
        (*per_cpu)[index].online.store(true, Ordering::Relaxed);
        let pointer = core::ptr::addr_of!((*per_cpu)[index]) as u64;
        write_msr(MSR_GS_BASE, pointer);
    }
}

/// AP entry: `RDI = *limine_mp_info`, stack provided by the bootloader.
#[unsafe(no_mangle)]
pub extern "C" fn aios_ap_main(info: *mut MpInfo) -> ! {
    // SAFETY: the bootloader passes a valid MP info pointer.
    let lapic_id = unsafe { (*info).lapic_id };
    let index = find_cpu(lapic_id).unwrap_or(0);

    // Load the per-CPU GDT/TSS first: `mov gs, ax` inside `gdt::load` reloads
    // the GS segment descriptor and zeroes the GS base, so the base must be
    // programmed *after* the GDT switch.
    gdt::load(index);
    idt::load();
    apic::enable();
    // SAFETY: index < MAX_CPUS; this CPU runs our code with interrupts off.
    unsafe {
        let per_cpu = &raw mut PER_CPU;
        let pointer = core::ptr::addr_of!((*per_cpu)[index]) as u64;
        write_msr(MSR_GS_BASE, pointer);
    }
    let per_cpu = this_cpu();
    // Single `_print` call (with the newline) so concurrent APs cannot
    // interleave half-lines on the shared console.
    crate::print!(
        "smp: AP {} online (lapic {})\n",
        per_cpu.cpu_id,
        per_cpu.lapic_id,
    );
    // SAFETY: index < MAX_CPUS.
    unsafe {
        (*core::ptr::addr_of!(PER_CPU).cast::<PerCpu>().add(index))
            .online
            .store(true, Ordering::Release);
    }
    AP_ONLINE.fetch_add(1, Ordering::AcqRel);

    // Idle: interrupts wake the CPU only for the spurious vector in M1a.
    loop {
        arch::idle_once();
    }
}

fn find_cpu(lapic_id: u32) -> Option<usize> {
    for index in 0..MAX_CPUS {
        // SAFETY: index < MAX_CPUS.
        let per_cpu = unsafe { core::ptr::addr_of!(PER_CPU).cast::<PerCpu>().add(index) };
        if unsafe { (*per_cpu).lapic_id } == lapic_id {
            return Some(index);
        }
    }
    None
}

/// The per-CPU record of the current CPU (GS base points at it).
pub fn this_cpu() -> &'static PerCpu {
    let pointer = read_msr(MSR_GS_BASE);
    // SAFETY: the GS base is set by `setup_bsp`/`aios_ap_main` and points at a
    // static `PerCpu`.
    unsafe { &*(pointer as *const PerCpu) }
}

fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    // SAFETY: privileged MSR write during CPU bring-up.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nostack, preserves_flags),
        );
    }
}

fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: privileged MSR read.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | low as u64
}

use crate::arch::x86_64::MAX_CPUS;
use core::sync::atomic::AtomicU32;
