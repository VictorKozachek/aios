//! Вывод в COM1 (0x3F8) — самый ранний и самый надёжный канал отладки.
//!
//! Инициализация: 115200 бод, 8 бит данных, без чётности, 1 стоп-бит, FIFO
//! включён. Регистры классического 16550: DLAB, IER, LCR, LSR.

use crate::io::{inb, outb};

const COM1: u16 = 0x3f8;

const DATA: u16 = 0; // RX/TX буфер (DLAB=0) или младший байт делителя (DLAB=1)
const IER: u16 = 1; // регистр разрешения прерываний
const FCR: u16 = 2; // регистр управления FIFO
const LCR: u16 = 3; // регистр управления линией
const MCR: u16 = 4; // регистр управления модемом
const LSR: u16 = 5; // регистр состояния линии
const DIVISOR_HI: u16 = 1; // старший байт делителя при DLAB=1

const LSR_THR_EMPTY: u8 = 1 << 5;

/// Порт COM1 в режиме «только вывод».
pub struct Serial {
    port: u16,
}

impl Serial {
    /// Инициализировать COM1. Возвращает `None`, если порт не отвечает.
    pub fn init() -> Option<Self> {
        let port = COM1;

        // SAFETY: порты 0x3F8..0x3FF — стандартный UART 16550; при их
        // отсутствии запись просто игнорируется железом.
        unsafe {
            outb(port + IER, 0x00); // прерывания не нужны — опрашиваем LSR
            outb(port + LCR, 0x80); // DLAB=1: задаём делитель
            outb(port + DATA, 0x01); // 115200 бод: делитель = 1
            outb(port + DIVISOR_HI, 0x00);
            outb(port + LCR, 0x03); // 8N1, DLAB=0
            outb(port + FCR, 0xC7); // FIFO включён, очищен, порог 14 байт
            outb(port + MCR, 0x03); // DTR + RTS
        }

        // Проверка работоспособности: без UART шина возвращает 0xFF,
        // и LSR никогда не выставит бит готовности передатчика.
        let mut port = Self { port };
        if port.wait_tx_empty(10_000) {
            Some(port)
        } else {
            None
        }
    }

    /// Записать байт (перевод строки превращается в CRLF).
    pub fn write_byte(&mut self, byte: u8) {
        if byte == b'\n' {
            self.write_raw(b'\r');
        }
        self.write_raw(byte);
    }

    fn write_raw(&mut self, byte: u8) {
        // Ждём освобождения передатчика, но не бесконечно: если UART мёртв,
        // теряем символы, а не зависаем на загрузке.
        if !self.wait_tx_empty(1_000_000) {
            return;
        }
        // SAFETY: порт инициализирован в `init`.
        unsafe {
            outb(self.port + DATA, byte);
        }
    }

    fn wait_tx_empty(&mut self, limit: u32) -> bool {
        for _ in 0..limit {
            // SAFETY: LSR — регистр состояния того же UART.
            let status = unsafe { inb(self.port + LSR) };
            if status & LSR_THR_EMPTY != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
}
