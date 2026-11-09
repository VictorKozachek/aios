//! Framebuffer: геометрия, формат пикселя и проверки.
//!
//! Ядро получает framebuffer от загрузчика как обычную память. Пока нет таблиц
//! страниц (T4), писать можно только в уже отображённую область, поэтому все
//! проверки здесь — про геометрию и формат, а не про кэш/маппинг.

use crate::error::BootInfoError;

/// Линейная RGB-модель памяти (`LIMINE_FRAMEBUFFER_RGB` в протоколе Limine).
pub const MEMORY_MODEL_RGB: u8 = 1;

/// Маска одного цветового канала: размер в битах и сдвиг в пикселе.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    /// Сколько бит занимает канал.
    pub size: u8,
    /// На сколько бит канал сдвинут внутри пикселя.
    pub shift: u8,
}

impl Channel {
    pub const fn new(size: u8, shift: u8) -> Self {
        Self { size, shift }
    }

    /// Маска канала в 32-битном пикселе; 0, если канал не задан или задан странно.
    pub const fn mask(&self) -> u32 {
        if self.size == 0 || self.size >= 32 || self.shift >= 32 {
            0
        } else {
            ((1u32 << self.size) - 1) << self.shift
        }
    }
}

/// Основной framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Framebuffer {
    /// Физический/виртуальный (HHDM) адрес начала видеопамяти.
    pub addr: u64,
    /// Ширина в пикселях.
    pub width: u32,
    /// Высота в пикселях.
    pub height: u32,
    /// Байт на строку (может быть больше `width * bytes_per_pixel`).
    pub pitch: u32,
    /// Бит на пиксель.
    pub bpp: u16,
    /// Модель памяти: 1 — RGB, 0 — индексированная.
    pub memory_model: u8,
    /// Красный канал.
    pub red: Channel,
    /// Зелёный канал.
    pub green: Channel,
    /// Синий канал.
    pub blue: Channel,
}

impl Framebuffer {
    /// Байт на пиксель (округление вверх для 15/24-битных форматов).
    pub const fn bytes_per_pixel(&self) -> usize {
        (self.bpp as usize).div_ceil(8)
    }

    /// Минимальный корректный pitch: сколько байт занимает одна строка.
    pub const fn min_pitch(&self) -> u64 {
        self.width as u64 * self.bytes_per_pixel() as u64
    }

    /// Размер всего буфера в байтах.
    pub const fn frame_bytes(&self) -> u64 {
        self.pitch as u64 * self.height as u64
    }

    /// Прямой цвет (RGB), а не палитра.
    pub const fn is_direct_color(&self) -> bool {
        self.memory_model == MEMORY_MODEL_RGB
    }

    /// Можно ли рисовать 32-битными пикселями напрямую (так работает консоль T2b).
    pub const fn is_writable_32bpp(&self) -> bool {
        self.bpp == 32 && self.is_direct_color() && self.pitch as u64 >= self.min_pitch()
    }

    /// Проверить геометрию и формат.
    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.addr == 0 || self.width == 0 || self.height == 0 || self.pitch == 0 {
            return Err(BootInfoError::FramebufferZeroSize);
        }
        if !matches!(self.bpp, 8 | 16 | 24 | 32) {
            return Err(BootInfoError::FramebufferBadBpp(self.bpp));
        }
        let min_pitch = self.min_pitch();
        if (self.pitch as u64) < min_pitch {
            return Err(BootInfoError::FramebufferBadPitch {
                pitch: self.pitch,
                min_pitch,
            });
        }
        Ok(())
    }

    /// Смещение пикселя (в байтах) от начала буфера.
    pub fn pixel_offset(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = y as u64 * self.pitch as u64 + x as u64 * self.bytes_per_pixel() as u64;
        usize::try_from(offset).ok()
    }
}
