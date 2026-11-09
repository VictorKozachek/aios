#![no_std]
#![no_main]

//! Ядро AIOS. Точка входа и протокол Limine — задача T1 (см. docs/PLAN-M0.md).

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
