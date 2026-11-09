#![no_std]
#![no_main]

//! Ядро AIOS: точка входа по протоколу Limine (base revision 3).
//!
//! Что делает M0/T1+T2:
//! 1. поднимает три канала вывода (serial, debugcon, framebuffer);
//! 2. проверяет, что загрузчик понял запрошенную base revision;
//! 3. собирает `bootinfo::BootInfo` из ответов Limine и валидирует его;
//! 4. печатает сводку и останавливается (планировщика в M0 ещё нет).
//!
//! Спецификация — `docs/ARCHITECTURE.md` §3, план — `docs/PLAN-M0.md` (T1, T2).

mod arch;
mod boot;
mod console;
mod io;

/// Точка входа ядра. Limine передаёт управление сюда без аргументов.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    console::init_early();
    arch::x86_64::init();

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
        None => println!("limine: <bootloader info unavailable>"),
    }

    // Framebuffer подключаем до основной сводки: тогда экран и serial
    // показывают один и тот же текст.
    if let Some(framebuffer) = boot::limine::primary_framebuffer() {
        console::init_framebuffer(framebuffer);
    } else {
        println!("console: no framebuffer, serial + debugcon only");
    }

    let info = boot::collect();
    // Битые данные от загрузчика лучше ловить здесь, чем в аллокаторе (T4):
    // валидация проверяет карту памяти, HHDM, framebuffer, RSDP, TSC, модули.
    if let Err(error) = info.validate() {
        println!("FATAL: invalid BootInfo: {}", error);
        panic!("invalid BootInfo: {}", error);
    }
    boot::report(&info);

    println!();
    println!("boot: ok, halting (M0: no scheduler yet)");
    io::halt_forever();
}
