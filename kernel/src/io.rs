//! Порты ввода-вывода x86-64 и остановка процессора.
//!
//! У ядра нет зависимости от крейта `x86_64`, поэтому байтовые инструкции
//! ввода-вывода заданы здесь напрямую.

use core::arch::asm;

/// Записать байт в порт.
///
/// # Safety
///
/// Порт должен существовать и допускать запись; вызывающий отвечает за то,
/// что запись не ломает состояние устройства.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY: гарантии делегированы вызывающему (см. # Safety выше).
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

/// Прочитать байт из порта.
///
/// # Safety
///
/// Порт должен существовать и допускать чтение.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: гарантии делегированы вызывающему (см. # Safety выше).
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

/// Остановить процессор до прерывания.
#[inline]
pub fn halt_once() {
    // SAFETY: hlt безопасен, если прерывания не придут — процессор просто спит.
    unsafe {
        asm!("hlt", options(nomem, nostack, preserves_flags));
    }
}

/// Остановить ядро навсегда.
pub fn halt_forever() -> ! {
    loop {
        halt_once();
    }
}
