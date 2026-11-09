//! Вывод в debugcon (порт 0xE9).
//!
//! QEMU с `-debugcon file:...` (или `-debugcon stdio`) показывает всё, что
//! ядро пишет в этот порт. Канал ничего не стоит и работает даже тогда, когда
//! serial и framebuffer ещё не готовы.

use crate::io::outb;

const DEBUGCON_PORT: u16 = 0xe9;

/// Инициализация не нужна: порт пишет байт «как есть».
pub const fn is_available() -> bool {
    true
}

/// Записать байт в debugcon.
pub fn write_byte(byte: u8) {
    // SAFETY: порт 0xE9 — отладочный порт QEMU; на реальном железе это
    // безвредный неиспользуемый порт.
    unsafe {
        outb(DEBUGCON_PORT, byte);
    }
}
