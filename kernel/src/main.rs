#![no_std]
#![no_main]

//! Ядро AIOS: точка входа по протоколу Limine (base revision 3).
//!
//! Что делает M0/T1+T2:
//! 1. поднимает три канала вывода (serial, debugcon, framebuffer);
//! 2. проверяет, что загрузчик понял запрошенную base revision;
//! 3. забирает у Limine карту памяти, HHDM, RSDP, TSC, число ядер, версию;
//! 4. печатает сводку и останавливается (планировщика в M0 ещё нет).
//!
//! Спецификация — `docs/ARCHITECTURE.md` §3, план — `docs/PLAN-M0.md` (T1, T2).

mod boot;
mod console;
mod io;

use core::panic::PanicInfo;

/// Точка входа ядра. Limine передаёт управление сюда без аргументов.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    console::init_early();

    println!();
    println!("=== AIOS kernel ===");
    println!("boot: entry reached (_start)");

    let (supported, loaded_revision) = boot::limine::base_revision();
    println!(
        "limine: base revision requested {}, loaded {}, supported={}",
        boot::limine::BASE_REVISION,
        loaded_revision,
        supported
    );
    if !supported {
        println!(
            "FATAL: bootloader does not support base revision {}",
            boot::limine::BASE_REVISION
        );
        panic!("unsupported Limine base revision");
    }

    match boot::limine::bootloader_info() {
        Some(info) => {
            // SAFETY: протокол гарантирует NUL-терминированные строки,
            // живущие всё время работы ядра.
            let name = unsafe { boot::limine::cstr(info.name) };
            // SAFETY: то же самое для второй строки.
            let version = unsafe { boot::limine::cstr(info.version) };
            println!("limine: {} {}", name, version);
        }
        None => println!("limine: <bootloader info недоступна>"),
    }

    // Framebuffer подключаем до основной сводки: тогда экран и serial
    // показывают один и тот же текст.
    if let Some(framebuffer) = boot::limine::primary_framebuffer() {
        console::init_framebuffer(framebuffer);
    } else {
        println!("console: framebuffer отсутствует, остаются serial и debugcon");
    }

    let info = boot::collect();
    boot::report(&info);

    println!();
    println!("boot: ok, halting (M0: планировщика ещё нет)");
    io::halt_forever();
}

/// Паника: печатаем причину во все каналы и навсегда останавливаемся.
///
/// Карта символов для разбора адресов появится вместе с IDT (T3); сейчас
/// задача — чтобы падение было видно в логе, а не выглядело как зависание.
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    println!();
    println!("PANIC: {}", info);
    println!("--- kernel halted ---");
    io::halt_forever();
}
