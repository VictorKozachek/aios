//! Текстовая консоль на framebuffer Limine.
//!
//! Рисуем знакогенератором 8×8 (см. `font.rs`) с масштабом 2 на достаточно
//! больших экранах. При заполнении экрана строки сдвигаются вверх —
//! простейшая прокрутка без аллокаций.
//!
//! Никакого форматирования здесь нет: консоль получает уже готовые байты.

use crate::boot::limine::Framebuffer;

use super::font::{FIRST_CHAR, FONT};

/// Сколько байт на пиксель допускаем: 32, 24 и 16 бит.
const MIN_BPP: u16 = 16;
const MAX_BPP: u16 = 32;

/// Цвет в формате «упакованный u32 по маскам каналов».
#[derive(Clone, Copy)]
struct Rgb(u32);

/// SAFETY: консоль — это указатель на отображённую память framebuffer и
/// счётчики. Владелец (мьютекс консоли) сериализует доступ, поэтому консоль
/// можно передавать между потоками.
unsafe impl Send for FramebufferConsole {}

pub struct FramebufferConsole {
    base: *mut u8,
    width: usize,
    height: usize,
    pitch: usize,
    bytes_per_pixel: usize,
    red_mask_size: u8,
    red_mask_shift: u8,
    green_mask_size: u8,
    green_mask_shift: u8,
    blue_mask_size: u8,
    blue_mask_shift: u8,
    foreground: Rgb,
    background: Rgb,
    scale: usize,
    cell_width: usize,
    cell_height: usize,
    columns: usize,
    rows: usize,
    cursor_column: usize,
    cursor_row: usize,
}

impl FramebufferConsole {
    /// Создать консоль поверх framebuffer загрузчика.
    ///
    /// Возвращает `None`, если формат экрана не поддерживается — тогда
    /// остаются serial и debugcon.
    pub fn new(framebuffer: &Framebuffer) -> Option<Self> {
        if framebuffer.address.is_null()
            || framebuffer.width == 0
            || framebuffer.height == 0
            || framebuffer.pitch == 0
            || framebuffer.bpp < MIN_BPP
            || framebuffer.bpp > MAX_BPP
            || framebuffer.bpp % 8 != 0
        {
            return None;
        }

        let bytes_per_pixel = (framebuffer.bpp / 8) as usize;
        if !matches!(bytes_per_pixel, 2 | 3 | 4) {
            return None;
        }

        let scale = if framebuffer.width >= 800 && framebuffer.height >= 600 {
            2
        } else {
            1
        };
        let cell_width = 8 * scale;
        let cell_height = 8 * scale;
        let columns = framebuffer.width as usize / cell_width;
        let rows = framebuffer.height as usize / cell_height;
        if columns == 0 || rows == 0 {
            return None;
        }

        let mut console = Self {
            base: framebuffer.address,
            width: framebuffer.width as usize,
            height: framebuffer.height as usize,
            pitch: framebuffer.pitch as usize,
            bytes_per_pixel,
            red_mask_size: framebuffer.red_mask_size,
            red_mask_shift: framebuffer.red_mask_shift,
            green_mask_size: framebuffer.green_mask_size,
            green_mask_shift: framebuffer.green_mask_shift,
            blue_mask_size: framebuffer.blue_mask_size,
            blue_mask_shift: framebuffer.blue_mask_shift,
            foreground: Rgb(0),
            background: Rgb(0),
            scale,
            cell_width,
            cell_height,
            columns,
            rows,
            cursor_column: 0,
            cursor_row: 0,
        };
        console.foreground = Rgb(console.pack(0xff, 0xff, 0xff));
        console.background = Rgb(console.pack(0x0a, 0x0e, 0x18));
        console.clear();
        Some(console)
    }

    /// Сколько символов помещается в строке.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Сколько текстовых строк помещается на экране.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Записать байт текста (ASCII; всё остальное рисуется как `?`).
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cursor_column = 0,
            b'\t' => {
                for _ in 0..4 {
                    self.write_byte(b' ');
                }
            }
            0x20..=0x7e => self.write_glyph(byte),
            // Управляющие символы не рисуем.
            0x00..=0x1f => {}
            _ => self.write_glyph(b'?'),
        }
    }

    fn newline(&mut self) {
        self.cursor_column = 0;
        self.cursor_row += 1;
        if self.cursor_row >= self.rows {
            self.scroll();
            self.cursor_row = self.rows - 1;
        }
    }

    fn write_glyph(&mut self, byte: u8) {
        if self.cursor_column >= self.columns {
            self.newline();
        }

        let glyph = if (FIRST_CHAR..=super::font::LAST_CHAR).contains(&byte) {
            &FONT[(byte - FIRST_CHAR) as usize]
        } else {
            &FONT[(b'?' - FIRST_CHAR) as usize]
        };

        let origin_x = self.cursor_column * self.cell_width;
        let origin_y = self.cursor_row * self.cell_height;

        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..8 {
                let on = bits & (1 << column) != 0;
                if on {
                    self.fill_rect(
                        origin_x + column * self.scale,
                        origin_y + row * self.scale,
                        self.scale,
                        self.scale,
                        self.foreground,
                    );
                }
            }
        }

        self.cursor_column += 1;
        if self.cursor_column >= self.columns {
            self.newline();
        }
    }

    /// Сдвинуть содержимое экрана на одну текстовую строку вверх.
    fn scroll(&mut self) {
        let line_bytes = self.cell_height * self.pitch;
        let remaining = self.pitch * self.height - line_bytes;

        // SAFETY: base указывает на отображённый framebuffer размером
        // height * pitch байт; копируем внутри этого диапазона.
        unsafe {
            core::ptr::copy(self.base.add(line_bytes), self.base, remaining);
        }

        let top = self.height - self.cell_height;
        self.fill_rect(0, top, self.width, self.cell_height, self.background);
    }

    /// Очистить экран и вернуть курсор в левый верхний угол.
    pub fn clear(&mut self) {
        self.fill_rect(0, 0, self.width, self.height, self.background);
        self.cursor_column = 0;
        self.cursor_row = 0;
    }

    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Rgb) {
        let x_end = (x + width).min(self.width);
        let y_end = (y + height).min(self.height);
        for row in y..y_end {
            for column in x..x_end {
                self.put_pixel(column, row, color);
            }
        }
    }

    fn put_pixel(&mut self, x: usize, y: usize, color: Rgb) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.pitch + x * self.bytes_per_pixel;
        // SAFETY: offset < height * pitch по проверке выше — в пределах
        // отображённого буфера framebuffer.
        unsafe {
            let pixel = self.base.add(offset);
            match self.bytes_per_pixel {
                4 => core::ptr::write(pixel.cast::<u32>(), color.0),
                3 => {
                    let bytes = color.0.to_le_bytes();
                    pixel.write(bytes[0]);
                    pixel.add(1).write(bytes[1]);
                    pixel.add(2).write(bytes[2]);
                }
                2 => core::ptr::write(pixel.cast::<u16>(), color.0 as u16),
                _ => {}
            }
        }
    }

    /// Упаковать 8-битные каналы согласно маскам загрузчика.
    fn pack(&self, red: u8, green: u8, blue: u8) -> u32 {
        pack_channel(red, self.red_mask_size, self.red_mask_shift)
            | pack_channel(green, self.green_mask_size, self.green_mask_shift)
            | pack_channel(blue, self.blue_mask_size, self.blue_mask_shift)
    }
}

fn pack_channel(value: u8, mask_size: u8, mask_shift: u8) -> u32 {
    if mask_size == 0 || mask_size > 8 {
        return 0;
    }
    let max_value = (1u32 << mask_size) - 1;
    (u32::from(value) * max_value / 255) << mask_shift
}
