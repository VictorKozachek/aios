//! Boot splash composition for the v1 "Void" design (T7).
//!
//! The renderer composes only primitives from [`crate::draw`]: a dithered
//! vertical gradient, baked mask blits (mark, wordmark, glyph atlas) and
//! integer alpha blending. All geometry follows `docs/design/splash-spec.md`
//! (variant v1): a single fixed scale, only `cx`/`left`/`top` depend on the
//! framebuffer size.
//!
//! The text console keeps its own log area below the block; its geometry is
//! derived from the same formulas ([`log_area`]) so the log starts under the
//! last status line and never overlaps the splash.

use core::fmt::{self, Write};

use crate::assets::{atlas_jbm15, logo_aios, wordmark_aios};
use bootinfo::{BootInfo, Framebuffer, MemmapKind, MIB};
use crate::draw::{self, Canvas, Mask, Rgb};

/// Version baked at compile time by the Makefile (`AIOS_VERSION`).
pub const AIOS_VERSION: &str = match option_env!("AIOS_VERSION") {
    Some(version) => version,
    None => "v0.0.0-unknown",
};

/// Git hash baked at compile time by the Makefile (`GIT_HASH`).
pub const GIT_HASH: &str = match option_env!("GIT_HASH") {
    Some(hash) => hash,
    None => "unknown",
};

/// Gradient stops (`spec §3`).
const BG_TOP: Rgb = (0x05, 0x07, 0x0c);
const BG_MID: Rgb = (0x0a, 0x11, 0x20);
const BG_BOTTOM: Rgb = (0x04, 0x06, 0x0a);
/// Palette (`spec §2`).
const INK: Rgb = (0xe8, 0xee, 0xf9);
const INK_DIM: Rgb = (0x93, 0xa3, 0xbc);
const INK_HASH: Rgb = (0x74, 0x83, 0x9b);
const ACCENT: Rgb = (0x86, 0xa9, 0xff);
const WHITE: Rgb = (0xff, 0xff, 0xff);

/// Atlas cell advance and status line step (`spec §4`, §7).
const CELL_WIDTH: i32 = 9;
const STATUS_STEP: i32 = 28;
/// Log line step; the console uses the same value.
pub const LOG_LINE_STEP: i32 = 21;

/// Geometry of the log area below the splash block, in pixels.
#[derive(Clone, Copy)]
pub struct LogArea {
    /// Left edge of the text column.
    pub left: i32,
    /// Baseline of the first log line.
    pub first_baseline: i32,
    /// Baseline of the last log line (rows that would go below are dropped).
    pub last_baseline: i32,
}

/// Everything the splash displays, captured once from `BootInfo`.
pub struct SplashMeta {
    /// Product version, e.g. `v0.0.1`.
    pub version: &'static str,
    /// Git hash printed in the small line.
    pub git_hash: &'static str,
    /// Usable physical memory, MiB.
    pub memory_mib: u64,
    /// RSDP address (ACPI root), if the bootloader provided one.
    pub acpi_rsdp: Option<u64>,
    /// Number of CPUs reported by the bootloader.
    pub cpus: u64,
    /// TSC frequency, MHz.
    pub tsc_mhz: Option<u64>,
}

impl SplashMeta {
    /// Captures the values shown by the splash from the boot information.
    pub fn from_boot_info(info: &BootInfo) -> Self {
        let mut usable_bytes = 0u64;
        for entry in info.memmap() {
            if entry.kind == MemmapKind::Usable {
                usable_bytes = usable_bytes.saturating_add(entry.length);
            }
        }
        Self {
            version: AIOS_VERSION,
            git_hash: GIT_HASH,
            memory_mib: usable_bytes / MIB,
            acpi_rsdp: info.rsdp,
            cpus: info.cpu_count,
            tsc_mhz: info.tsc_hz.map(|hz| hz / 1_000_000),
        }
    }
}

/// Paints the v1 splash; returns `false` when the framebuffer is unusable.
pub fn draw_splash(framebuffer: Framebuffer, meta: &SplashMeta) -> bool {
    let Some(mut canvas) = Canvas::from_bootinfo(&framebuffer) else {
        return false;
    };
    let width = canvas.width() as i32;
    let height = canvas.height() as i32;
    let cx = width / 2;
    let left = cx - 240;
    let top = (height * 42 + 50) / 100 - 179;

    // 1. Background: three-stop gradient, row by row, with ordered dithering.
    canvas.fill_gradient(canvas.height(), BG_TOP, BG_MID, BG_BOTTOM);

    // 2. Mark and wordmark: pre-rendered masks, fixed size on every screen.
    if let Some(logo) = logo_mask() {
        draw::blit_mask(&mut canvas, &logo, 0, 0, 64, 64, cx - 32, top, ACCENT, 242);
    }
    if let Some(wordmark) = wordmark_mask() {
        draw::blit_mask(
            &mut canvas,
            &wordmark,
            0,
            0,
            252,
            65,
            cx - 126,
            top + 102,
            INK,
            255,
        );
    }

    // 3. Version and git hash, centred on the column, +1 px tracking.
    let Some(atlas) = atlas_mask() else {
        return true;
    };
    let mut subtitle = LineBuffer::<48>::new();
    let _ = write!(subtitle, "SkyNet {}", meta.version);
    draw::draw_text_atlas(
        &mut canvas,
        &atlas,
        subtitle.as_str(),
        cx - text_width(subtitle.as_str(), 1) / 2,
        top + 194,
        INK_DIM,
        255,
        1,
    );
    let mut hash = LineBuffer::<48>::new();
    let _ = write!(hash, "git {}", meta.git_hash);
    draw::draw_text_atlas(
        &mut canvas,
        &atlas,
        hash.as_str(),
        cx - text_width(hash.as_str(), 1) / 2,
        top + 218,
        INK_HASH,
        255,
        1,
    );

    // 4. Hairline: 480 x 1, white at 9 %.
    canvas.blend_rect(left, top + 240, 480, 1, WHITE, 23);

    // 5. Four status rows: dot, label on the left, value right-aligned.
    let mut memory = LineBuffer::<32>::new();
    let _ = write!(memory, "{} MiB", meta.memory_mib);
    let mut acpi = LineBuffer::<40>::new();
    match meta.acpi_rsdp {
        Some(address) => {
            let _ = write!(acpi, "RSDP {:#X}", address);
        }
        None => {
            let _ = acpi.write_str("n/a");
        }
    }
    let mut cpus = LineBuffer::<16>::new();
    let _ = write!(cpus, "{}", meta.cpus);
    let mut tsc = LineBuffer::<24>::new();
    match meta.tsc_mhz {
        Some(mhz) => {
            let _ = write!(tsc, "{} MHz", mhz);
        }
        None => {
            let _ = tsc.write_str("unknown");
        }
    }
    let statuses: [(&str, &str); 4] = [
        ("memory", memory.as_str()),
        ("acpi", acpi.as_str()),
        ("cpus", cpus.as_str()),
        ("tsc", tsc.as_str()),
    ];
    for (index, (label, value)) in statuses.iter().enumerate() {
        let baseline = top + 282 + STATUS_STEP * index as i32;
        draw::fill_disc(&mut canvas, left + 2, baseline - 5, 2, ACCENT, 178);
        draw::draw_text_atlas(
            &mut canvas,
            &atlas,
            label,
            left + 16,
            baseline,
            INK_DIM,
            255,
            0,
        );
        draw::draw_text_atlas(
            &mut canvas,
            &atlas,
            value,
            left + 480 - text_width(value, 0),
            baseline,
            INK,
            255,
            0,
        );
    }
    true
}

/// Log area below the splash block, for the text console (`spec §4`, §9).
pub fn log_area(framebuffer: Framebuffer) -> LogArea {
    let width = framebuffer.width as i32;
    let height = framebuffer.height as i32;
    let cx = width / 2;
    let left = cx - 240;
    let top = (height * 42 + 50) / 100 - 179;
    LogArea {
        left,
        first_baseline: top + 410,
        last_baseline: height - 48,
    }
}

/// Serial-only fallback when there is no usable framebuffer.
pub fn fallback(meta: &SplashMeta) {
    crate::println!("AIOS SkyNet {} {}", meta.version, meta.git_hash);
}

fn text_width(text: &str, tracking: i32) -> i32 {
    let glyphs = text.len() as i32;
    if glyphs == 0 {
        0
    } else {
        glyphs * CELL_WIDTH + (glyphs - 1) * tracking
    }
}

fn atlas_mask() -> Option<Mask<'static>> {
    Mask::new(
        atlas_jbm15::ATLAS_JBM15_WIDTH as usize,
        atlas_jbm15::ATLAS_JBM15_HEIGHT as usize,
        atlas_jbm15::ATLAS_JBM15_ALPHA,
    )
}

fn logo_mask() -> Option<Mask<'static>> {
    Mask::new(
        logo_aios::LOGO_AIOS_WIDTH as usize,
        logo_aios::LOGO_AIOS_HEIGHT as usize,
        logo_aios::LOGO_AIOS_ALPHA,
    )
}

fn wordmark_mask() -> Option<Mask<'static>> {
    Mask::new(
        wordmark_aios::WORDMARK_AIOS_WIDTH as usize,
        wordmark_aios::WORDMARK_AIOS_HEIGHT as usize,
        wordmark_aios::WORDMARK_AIOS_ALPHA,
    )
}

/// Small stack-only string used to format splash lines without an allocator.
struct LineBuffer<const N: usize> {
    data: [u8; N],
    len: usize,
}

impl<const N: usize> LineBuffer<N> {
    const fn new() -> Self {
        Self {
            data: [0; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.data[..self.len]).unwrap_or("?")
    }
}

impl<const N: usize> fmt::Write for LineBuffer<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let bytes = text.as_bytes();
        let end = self.len + bytes.len();
        if end > N {
            return Err(fmt::Error);
        }
        self.data[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
}
