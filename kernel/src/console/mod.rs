//! Консоль ядра: три независимых канала вывода.
//!
//! Как требует `docs/ARCHITECTURE.md` §3: serial + framebuffer + debugcon.
//! Причина именно такая: если что-то падает до инициализации framebuffer,
//! остаётся serial; после — остаются все три.
//!
//! Все каналы пишут одни и те же байты, поэтому картинка на экране и лог
//! в `serial.log` совпадают.

mod debugcon;
mod fb;
mod font;
mod serial;

use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::boot::limine::Framebuffer;

use self::fb::FramebufferConsole;
use self::serial::Serial;

/// Состояние всех каналов вывода.
struct ConsoleState {
    serial: Option<Serial>,
    debugcon: bool,
    framebuffer: Option<FramebufferConsole>,
}

impl ConsoleState {
    const fn new() -> Self {
        Self {
            serial: None,
            debugcon: false,
            framebuffer: None,
        }
    }

    fn write_str(&mut self, s: &str) {
        for byte in s.as_bytes() {
            if let Some(serial) = self.serial.as_mut() {
                serial.write_byte(*byte);
            }
            if self.debugcon {
                debugcon::write_byte(*byte);
            }
            if let Some(framebuffer) = self.framebuffer.as_mut() {
                framebuffer.write_byte(*byte);
            }
        }
    }
}

/// Простейший спин-мьютекс: в M0 вывод идёт только с BSP, но блокировка
/// нужна, чтобы `print!` из паники не перемешивал строки с обычным выводом.
struct StaticMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: доступ к данным сериализован флагом `locked`.
unsafe impl<T: Send> Sync for StaticMutex<T> {}

impl<T> StaticMutex<T> {
    const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: флаг захвачен, значит кроме нас к данным никто не обращается.
        let result = f(unsafe { &mut *self.data.get() });
        self.locked.store(false, Ordering::Release);
        result
    }
}

static CONSOLE: StaticMutex<ConsoleState> = StaticMutex::new(ConsoleState::new());

/// Включить ранние каналы: serial (COM1) и debugcon.
///
/// Вызывать первой строкой ядра — до любых запросов к загрузчику.
pub fn init_early() {
    let serial = Serial::init();
    let has_serial = serial.is_some();
    let has_debugcon = debugcon::is_available();
    CONSOLE.with(|state| {
        state.serial = serial;
        state.debugcon = has_debugcon;
    });

    // Сообщаем, какие каналы живы: без этого непонятно, откуда вообще лог.
    crate::println!(
        "console: serial={}, debugcon={}",
        if has_serial {
            "COM1 115200 8N1"
        } else {
            "none"
        },
        if has_debugcon { "0xE9" } else { "none" },
    );
}

/// Подключить framebuffer как третий канал вывода.
pub fn init_framebuffer(framebuffer: &Framebuffer) {
    let console = FramebufferConsole::new(framebuffer);
    let ready = console.is_some();
    CONSOLE.with(|state| state.framebuffer = console);
    if ready {
        let (columns, rows) = CONSOLE.with(|state| match state.framebuffer.as_ref() {
            Some(framebuffer) => (framebuffer.columns(), framebuffer.rows()),
            None => (0, 0),
        });
        crate::println!(
            "console: framebuffer {}x{} {}bpp, text {}x{} chars",
            framebuffer.width,
            framebuffer.height,
            framebuffer.bpp,
            columns,
            rows
        );
    } else {
        crate::println!("console: framebuffer unavailable (unsupported format)");
    }
}

/// Низкоуровневый вывод отформатированного текста во все каналы.
///
/// Используйте макросы [`crate::print!`] и [`crate::println!`].
pub fn _print(args: fmt::Arguments<'_>) {
    CONSOLE.with(|state| {
        let mut writer = Writer { state };
        // Ошибок у Writer не бывает, поэтому результат игнорируем.
        let _ = fmt::write(&mut writer, args);
    });
}

struct Writer<'a> {
    state: &'a mut ConsoleState,
}

impl Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.state.write_str(s);
        Ok(())
    }
}

/// Печать в консоль без перевода строки.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!($($arg)*))
    };
}

/// Печать в консоль с переводом строки.
#[macro_export]
macro_rules! println {
    () => {
        $crate::console::_print(format_args!("\n"))
    };
    ($($arg:tt)*) => {{
        $crate::console::_print(format_args!($($arg)*));
        $crate::console::_print(format_args!("\n"));
    }};
}
