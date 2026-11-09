//! Человекочитаемая сводка `BootInfo` для лога ядра.
//!
//! Сводка печатается и через `core::fmt` (ядро может завернуть её в свой
//! `println!`), и в фиксированный буфер — без аллокации, с обрезкой по границе
//! буфера. Второй путь нужен панике и ранней загрузке, где форматирование с
//! `format_args!` уже есть, а heap — ещё/уже нет.

use core::fmt::{self, Write};

use crate::{BootInfo, MIB};

/// Обёртка для `Display`: `format!("{}", info.summary_display())`.
pub struct Summary<'a>(pub &'a BootInfo);

impl fmt::Display for Summary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let info = self.0;
        let summary = info.summary();

        write!(
            f,
            "memmap: {} entries, usable {} MiB, largest usable {} MiB",
            summary.entries,
            summary.usable / MIB,
            info.largest_usable().map_or(0, |entry| entry.length / MIB),
        )?;
        if info.memmap_truncated {
            f.write_str(" (TRUNCATED)")?;
        }
        write!(
            f,
            "; reserved {} MiB, acpi-reclaimable {} MiB, acpi-nvs {} MiB, \
             bootloader-reclaimable {} MiB, executable-and-modules {} MiB, \
             framebuffer {} MiB, reserved-mapped {} MiB, other {} MiB",
            summary.reserved / MIB,
            summary.acpi_reclaimable / MIB,
            summary.acpi_nvs / MIB,
            summary.bootloader_reclaimable / MIB,
            summary.executable_and_modules / MIB,
            summary.framebuffer / MIB,
            summary.reserved_mapped / MIB,
            summary.other / MIB,
        )?;

        // One field per line: the framebuffer console has 80 columns, and
        // `make smoke` greps for these labels (T1 contract, kept for T2).
        match info.hhdm {
            Some(offset) => write!(f, "\nHHDM base: 0x{offset:016x}")?,
            None => f.write_str("\nHHDM base: absent")?,
        }
        write!(f, "; paging: {}", info.paging_mode.name())?;
        match info.rsdp {
            Some(address) => write!(f, "\nRSDP: 0x{address:016x}")?,
            None => f.write_str("\nRSDP: absent")?,
        }
        match info.tsc_hz {
            Some(hz) => write!(f, "\nTSC: {} MHz ({} Hz)", hz / 1_000_000, hz)?,
            None => f.write_str("\nTSC: absent")?,
        }
        write!(
            f,
            "\nCPUs: {} (BSP LAPIC id {})",
            info.cpu_count, info.bsp_lapic_id
        )?;
        write!(f, "\nentropy: {}", info.entropy_source.name())?;

        match info.framebuffer {
            Some(fb) => write!(
                f,
                "; framebuffer: {}x{}, {} bpp, pitch {}",
                fb.width, fb.height, fb.bpp, fb.pitch
            )?,
            None => f.write_str("; framebuffer: absent")?,
        }

        if !info.modules().is_empty() {
            write!(f, "\nmodules: {}", info.modules().len())?;
            for module in info.modules() {
                write!(
                    f,
                    " [0x{:016x}+{} KiB {}]",
                    module.addr,
                    module.size / 1024,
                    module.path
                )?;
            }
        }
        if info.modules_truncated {
            f.write_str(" (TRUNCATED)")?;
        }

        Ok(())
    }
}

impl BootInfo {
    /// Сводка для печати через `core::fmt` (без аллокации).
    pub fn summary_display(&self) -> Summary<'_> {
        Summary(self)
    }

    /// Записать сводку в буфер; возвращает число записанных байт.
    ///
    /// Если буфер меньше сводки, пишется её префикс (без паники). Завершающий
    /// `\0` не добавляется: буфер — просто байты для serial/framebuffer.
    pub fn write_summary(&self, out: &mut [u8]) -> usize {
        let mut writer = SliceWriter::new(out);
        // Ошибку обрезки игнорируем осознанно: частичная сводка полезнее пустой.
        let _ = write!(&mut writer, "{}", self.summary_display());
        writer.len
    }
}

/// Писатель в срез байт: никогда не выходит за границы и не паникует.
struct SliceWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> SliceWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }
}

impl Write for SliceWriter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let bytes = text.as_bytes();
        let space = self.buf.len().saturating_sub(self.len);
        let take = core::cmp::min(space, bytes.len());
        self.buf[self.len..self.len + take].copy_from_slice(&bytes[..take]);
        self.len += take;
        if take < bytes.len() {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}
