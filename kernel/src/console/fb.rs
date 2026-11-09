//! Framebuffer log console for the v1 splash design (T7).
//!
//! Text is rendered from the JetBrains Mono glyph atlas (`atlas-jbm15`), not
//! from the old 8x8 VGA font: the atlas is what makes the boot log look like
//! the designer's mock instead of "cubes". Geometry and colours follow
//! `docs/design/splash-spec.md` §4 and §9: the log lives in the 480 px column
//! below the splash block, one 21 px line at a time, and stops when the area
//! is full (the console does not scroll over the splash).
//!
//! Serial and debugcon output are untouched: this module only renders the
//! framebuffer copy of the log.

use crate::assets::atlas_jbm15::{ATLAS_JBM15_ALPHA, ATLAS_JBM15_HEIGHT, ATLAS_JBM15_WIDTH};
use crate::boot::limine::Framebuffer;
use crate::draw::{self, Canvas, Mask, Rgb};
use crate::splash::{LOG_LINE_STEP, LogArea};

/// Atlas cell advance in pixels (`spec §7`).
const CELL_WIDTH: i32 = 9;
/// Width of the log column in pixels (`spec §4`).
const LOG_WIDTH: i32 = 480;
/// Log text colour: `INK_FAINT` at 70 % (`spec §2`).
const LOG_COLOR: Rgb = (0x5c, 0x6b, 0x80);
const LOG_ALPHA: u8 = 255;

pub struct FramebufferConsole {
    canvas: Canvas,
    area: Option<LogArea>,
    columns: usize,
    rows: usize,
    cursor_column: usize,
    cursor_row: usize,
    full: bool,
}

impl FramebufferConsole {
    /// Creates the console over the bootloader framebuffer.
    ///
    /// The log area is configured later, once the splash has computed its
    /// geometry; until then framebuffer output is ignored (serial/debugcon
    /// still receive everything).
    pub fn new(framebuffer: &Framebuffer) -> Option<Self> {
        let canvas = Canvas::new(framebuffer)?;
        Some(Self {
            canvas,
            area: None,
            columns: (LOG_WIDTH / CELL_WIDTH) as usize,
            rows: 0,
            cursor_column: 0,
            cursor_row: 0,
            full: true,
        })
    }

    /// Configures the log area below the splash block.
    pub fn configure_log(&mut self, area: LogArea) {
        self.area = Some(area);
        let span = area.last_baseline - area.first_baseline;
        self.rows = if span < 0 {
            0
        } else {
            (span / LOG_LINE_STEP + 1) as usize
        };
        self.columns = (LOG_WIDTH / CELL_WIDTH) as usize;
        self.cursor_column = 0;
        self.cursor_row = 0;
        self.full = self.rows == 0;
    }

    /// Writes one byte of text (ASCII; everything else is drawn as `?`).
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cursor_column = 0,
            b'\t' => {
                for _ in 0..4 {
                    self.write_byte(b' ');
                }
            }
            // Control characters are not rendered.
            0x00..=0x1f => {}
            _ => self.write_glyph(byte),
        }
    }

    fn newline(&mut self) {
        self.cursor_column = 0;
        self.cursor_row += 1;
        if self.cursor_row >= self.rows {
            self.full = true;
        }
    }

    fn write_glyph(&mut self, byte: u8) {
        if self.full {
            return;
        }
        let Some(area) = self.area else {
            return;
        };
        if self.cursor_column >= self.columns {
            self.newline();
            if self.full {
                return;
            }
        }
        let Some(atlas) = helper_atlas() else {
            return;
        };
        let x = area.left + self.cursor_column as i32 * CELL_WIDTH;
        let baseline = area.first_baseline + self.cursor_row as i32 * LOG_LINE_STEP;
        let mut utf8 = [0u8; 4];
        let character = if (0x20..=0x7e).contains(&byte) {
            byte as char
        } else {
            '?'
        };
        let text = character.encode_utf8(&mut utf8);
        draw::draw_text_atlas(
            &mut self.canvas,
            &atlas,
            text,
            x,
            baseline,
            LOG_COLOR,
            LOG_ALPHA,
            0,
        );
        self.cursor_column += 1;
        if self.cursor_column >= self.columns {
            self.newline();
        }
    }
}

fn helper_atlas() -> Option<Mask<'static>> {
    Mask::new(
        ATLAS_JBM15_WIDTH as usize,
        ATLAS_JBM15_HEIGHT as usize,
        ATLAS_JBM15_ALPHA,
    )
}
