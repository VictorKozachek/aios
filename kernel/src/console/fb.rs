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
/// Log text colour: `INK_FAINT` (`spec §2`).
const LOG_COLOR: Rgb = (0x5c, 0x6b, 0x80);
const LOG_ALPHA: u8 = 255;
/// Maximum characters per log line (`spec §4`: 480 px / 9 px).
const MAX_COLUMNS: usize = (LOG_WIDTH / CELL_WIDTH) as usize;

pub struct FramebufferConsole {
    canvas: Canvas,
    area: Option<LogArea>,
    columns: usize,
    rows: usize,
    cursor_column: usize,
    cursor_row: usize,
    full: bool,
    /// Current word, buffered so line breaks fall on word boundaries.
    word: [u8; MAX_COLUMNS],
    word_len: usize,
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
            word: [0; MAX_COLUMNS],
            word_len: 0,
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
        self.word_len = 0;
    }

    /// Writes one byte of text (ASCII; everything else is drawn as `?`).
    ///
    /// Words are buffered until a space/newline so long lines wrap on word
    /// boundaries instead of splitting words mid-token (the v0.1 boot summary
    /// has lines wider than the 53-column log). Tokens longer than a line are
    /// hard-broken.
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.flush_word();
                self.newline();
            }
            b'\r' => {
                self.flush_word();
                self.cursor_column = 0;
            }
            b'\t' => {
                for _ in 0..4 {
                    self.write_byte(b' ');
                }
            }
            // Control characters are not rendered.
            0x00..=0x1f => {}
            b' ' => {
                self.flush_word();
                self.write_space();
            }
            _ => {
                if self.word_len < MAX_COLUMNS {
                    self.word[self.word_len] = byte;
                    self.word_len += 1;
                }
                if self.word_len == MAX_COLUMNS {
                    self.flush_word();
                }
            }
        }
    }

    fn newline(&mut self) {
        self.cursor_column = 0;
        self.cursor_row += 1;
        if self.cursor_row >= self.rows {
            self.full = true;
        }
    }

    /// Draws the buffered word, wrapping before it if it does not fit.
    fn flush_word(&mut self) {
        if self.word_len == 0 {
            return;
        }
        if self.full {
            self.word_len = 0;
            return;
        }
        if self.cursor_column > 0 && self.cursor_column + self.word_len > self.columns {
            self.newline();
        }
        let mut index = 0;
        while index < self.word_len && !self.full {
            self.draw_glyph(self.word[index]);
            index += 1;
            if self.cursor_column >= self.columns {
                self.newline();
            }
        }
        self.word_len = 0;
    }

    /// Draws one separating space, skipping it when the line just wrapped.
    fn write_space(&mut self) {
        if self.full || self.cursor_column == 0 {
            return;
        }
        self.draw_glyph(b' ');
        if self.cursor_column >= self.columns {
            self.newline();
        }
    }

    /// Draws one glyph at the cursor; wrapping is handled by the callers.
    fn draw_glyph(&mut self, byte: u8) {
        if self.full || self.cursor_column >= self.columns {
            return;
        }
        let Some(area) = self.area else {
            return;
        };
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
    }
}

fn helper_atlas() -> Option<Mask<'static>> {
    Mask::new(
        ATLAS_JBM15_WIDTH as usize,
        ATLAS_JBM15_HEIGHT as usize,
        ATLAS_JBM15_ALPHA,
    )
}
