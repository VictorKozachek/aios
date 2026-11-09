//! Drawing primitives over the bootloader framebuffer.
//!
//! Task #007 keeps the splash renderer modular: `splash.rs` only composes
//! "fill the gradient", "blit a mask" and "draw a text line". All assets are
//! 8-bit coverage masks baked into `.rodata` by `tools/png2rust.py`; the kernel
//! has no PNG decoder.
//!
//! Blending is integer-only, per the design spec:
//! `dst = (dst * (255 - a) + tint * a) / 255` per channel, where `a` is the
//! mask coverage combined with the drawing alpha.

use crate::boot::limine::Framebuffer;

/// An RGB colour, 8 bits per channel.
pub type Rgb = (u8, u8, u8);

/// An 8-bit coverage mask (single ink colour), rows top to bottom.
#[derive(Clone, Copy)]
pub struct Mask<'a> {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width * height` coverage bytes.
    pub alpha: &'a [u8],
}

impl<'a> Mask<'a> {
    /// Wraps a coverage buffer, checking its dimensions and length.
    pub fn new(width: usize, height: usize, alpha: &'a [u8]) -> Option<Self> {
        if width == 0 || height == 0 || alpha.len() != width * height {
            return None;
        }
        Some(Self {
            width,
            height,
            alpha,
        })
    }
}

/// A row-major RGBA8 sprite (kept for future full-colour assets).
#[derive(Clone, Copy)]
pub struct Sprite<'a> {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width * height * 4` bytes in RGBA order.
    pub pixels: &'a [u8],
}

impl<'a> Sprite<'a> {
    /// Wraps an RGBA buffer, checking its dimensions and length.
    #[allow(dead_code)]
    pub fn new(width: usize, height: usize, pixels: &'a [u8]) -> Option<Self> {
        if width == 0 || height == 0 || pixels.len() != width * height * 4 {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }
}

/// 4x4 ordered-dither matrix from the design spec (§3).
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Midpoint of the three-stop gradient in 8.8 fixed point (0.52 * 256).
const GRADIENT_MID_Q: u32 = 133;

/// A pixel canvas over the bootloader framebuffer.
pub struct Canvas {
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
}

// SAFETY: the canvas only wraps a raw framebuffer pointer; the console mutex
// serialises all access, exactly like the previous `FramebufferConsole`.
unsafe impl Send for Canvas {}

impl Canvas {
    /// Creates a canvas over the Limine protocol framebuffer (used by the
    /// framebuffer text console, which attaches before `BootInfo` exists).
    pub fn new(framebuffer: &Framebuffer) -> Option<Self> {
        Self::build(
            framebuffer.address,
            framebuffer.width as usize,
            framebuffer.height as usize,
            framebuffer.pitch as usize,
            framebuffer.bpp,
            framebuffer.red_mask_size,
            framebuffer.red_mask_shift,
            framebuffer.green_mask_size,
            framebuffer.green_mask_shift,
            framebuffer.blue_mask_size,
            framebuffer.blue_mask_shift,
        )
    }

    /// Creates a canvas over the validated `bootinfo::Framebuffer` (T2b API).
    pub fn from_bootinfo(framebuffer: &bootinfo::Framebuffer) -> Option<Self> {
        Self::build(
            framebuffer.addr as *mut u8,
            framebuffer.width as usize,
            framebuffer.height as usize,
            framebuffer.pitch as usize,
            framebuffer.bpp,
            framebuffer.red.size,
            framebuffer.red.shift,
            framebuffer.green.size,
            framebuffer.green.shift,
            framebuffer.blue.size,
            framebuffer.blue.shift,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        base: *mut u8,
        width: usize,
        height: usize,
        pitch: usize,
        bpp: u16,
        red_mask_size: u8,
        red_mask_shift: u8,
        green_mask_size: u8,
        green_mask_shift: u8,
        blue_mask_size: u8,
        blue_mask_shift: u8,
    ) -> Option<Self> {
        if base.is_null()
            || width == 0
            || height == 0
            || pitch == 0
            || bpp < 16
            || bpp > 32
            || !bpp.is_multiple_of(8)
        {
            return None;
        }
        let bytes_per_pixel = (bpp / 8) as usize;
        if !matches!(bytes_per_pixel, 2..=4) {
            return None;
        }
        Some(Self {
            base,
            width,
            height,
            pitch,
            bytes_per_pixel,
            red_mask_size,
            red_mask_shift,
            green_mask_size,
            green_mask_shift,
            blue_mask_size,
            blue_mask_shift,
        })
    }

    /// Canvas width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Canvas height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Paints the first `rows` rows with the three-stop vertical gradient,
    /// one row at a time, with 4x4 ordered dithering (`spec §3`).
    ///
    /// The interpolation always spans the full canvas height, so repainting
    /// only the top block produces exactly the same colours as a full paint.
    pub fn fill_gradient(&mut self, rows: usize, top: Rgb, mid: Rgb, bottom: Rgb) {
        let rows = rows.min(self.height);
        if rows == 0 {
            return;
        }
        let height = self.height.max(1) as u32;
        for y in 0..rows {
            let t = ((y as u32) * 2 + 1) * 128 / height;
            let (from, to, u) = if t < GRADIENT_MID_Q {
                (top, mid, t * 256 / GRADIENT_MID_Q)
            } else {
                (
                    mid,
                    bottom,
                    (t - GRADIENT_MID_Q) * 256 / (256 - GRADIENT_MID_Q),
                )
            };
            let base = (
                u32::from(from.0) * (256 - u) + u32::from(to.0) * u,
                u32::from(from.1) * (256 - u) + u32::from(to.1) * u,
                u32::from(from.2) * (256 - u) + u32::from(to.2) * u,
            );
            for x in 0..self.width {
                let dither = u32::from(BAYER[y & 3][x & 3]) << 4;
                let color = self.pack(
                    ((base.0 + dither) >> 8) as u8,
                    ((base.1 + dither) >> 8) as u8,
                    ((base.2 + dither) >> 8) as u8,
                );
                self.put_pixel(x, y, color);
            }
        }
    }

    /// Blends a solid rectangle over the existing pixels.
    pub fn blend_rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: Rgb, alpha: u8) {
        for row in 0..height {
            for column in 0..width {
                let px = x + column;
                let py = y + row;
                if px < 0 || py < 0 {
                    continue;
                }
                self.blend_pixel(px as usize, py as usize, color, alpha);
            }
        }
    }

    fn blend_pixel(&mut self, x: usize, y: usize, tint: Rgb, alpha: u8) {
        if alpha == 0 || x >= self.width || y >= self.height {
            return;
        }
        let dst = self.read_pixel(x, y);
        let a = u32::from(alpha);
        let blended = (
            blend_channel(tint.0, dst.0, a),
            blend_channel(tint.1, dst.1, a),
            blend_channel(tint.2, dst.2, a),
        );
        let packed = self.pack(blended.0, blended.1, blended.2);
        self.put_pixel(x, y, packed);
    }

    fn pack(&self, red: u8, green: u8, blue: u8) -> u32 {
        pack_channel(red, self.red_mask_size, self.red_mask_shift)
            | pack_channel(green, self.green_mask_size, self.green_mask_shift)
            | pack_channel(blue, self.blue_mask_size, self.blue_mask_shift)
    }

    fn unpack(&self, value: u32) -> Rgb {
        (
            unpack_channel(value, self.red_mask_size, self.red_mask_shift),
            unpack_channel(value, self.green_mask_size, self.green_mask_shift),
            unpack_channel(value, self.blue_mask_size, self.blue_mask_shift),
        )
    }

    fn put_pixel(&mut self, x: usize, y: usize, color: u32) {
        let offset = y * self.pitch + x * self.bytes_per_pixel;
        // SAFETY: callers clip coordinates to the canvas, so `offset` stays
        // inside the mapped framebuffer.
        unsafe {
            let pixel = self.base.add(offset);
            match self.bytes_per_pixel {
                4 => core::ptr::write(pixel.cast::<u32>(), color),
                3 => {
                    let bytes = color.to_le_bytes();
                    pixel.write(bytes[0]);
                    pixel.add(1).write(bytes[1]);
                    pixel.add(2).write(bytes[2]);
                }
                2 => core::ptr::write(pixel.cast::<u16>(), color as u16),
                _ => {}
            }
        }
    }

    fn read_pixel(&self, x: usize, y: usize) -> Rgb {
        let offset = y * self.pitch + x * self.bytes_per_pixel;
        // SAFETY: callers clip coordinates to the canvas, so `offset` stays
        // inside the mapped framebuffer.
        let value = unsafe {
            let pixel = self.base.add(offset);
            match self.bytes_per_pixel {
                4 => core::ptr::read(pixel.cast::<u32>()),
                3 => {
                    u32::from_le_bytes([pixel.read(), pixel.add(1).read(), pixel.add(2).read(), 0])
                }
                2 => u32::from(core::ptr::read(pixel.cast::<u16>())),
                _ => 0,
            }
        };
        self.unpack(value)
    }
}

/// Blits a rectangular part of an 8-bit mask at (`x`, `y`), tinting it with
/// `color` and multiplying coverage by `alpha`.
#[allow(clippy::too_many_arguments)]
pub fn blit_mask(
    canvas: &mut Canvas,
    mask: &Mask<'_>,
    src_x: usize,
    src_y: usize,
    src_width: usize,
    src_height: usize,
    x: i32,
    y: i32,
    color: Rgb,
    alpha: u8,
) {
    for row in 0..src_height {
        let my = src_y + row;
        if my >= mask.height {
            break;
        }
        for column in 0..src_width {
            let mx = src_x + column;
            if mx >= mask.width {
                break;
            }
            let coverage = u32::from(mask.alpha[my * mask.width + mx]);
            if coverage == 0 {
                continue;
            }
            let combined = (coverage * u32::from(alpha) / 255) as u8;
            if combined == 0 {
                continue;
            }
            let px = x + column as i32;
            let py = y + row as i32;
            if px < 0 || py < 0 {
                continue;
            }
            canvas.blend_pixel(px as usize, py as usize, color, combined);
        }
    }
}

/// Draws a solid circle with integer edge coverage (`r` in pixels).
pub fn fill_disc(canvas: &mut Canvas, cx: i32, cy: i32, radius: i32, color: Rgb, alpha: u8) {
    let r2 = radius * radius;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > r2 {
                continue;
            }
            let x = cx + dx;
            let y = cy + dy;
            if x < 0 || y < 0 {
                continue;
            }
            canvas.blend_pixel(x as usize, y as usize, color, alpha);
        }
    }
}

/// Draws one line of text from the JetBrains Mono glyph atlas.
///
/// The atlas is a 16x6 grid of 9x20 cells covering ASCII `0x20..=0x7E`.
/// `baseline` is the text baseline; cell tops are `baseline - 15`. Non-ASCII
/// bytes are drawn as `?` so the advance stays correct.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_atlas(
    canvas: &mut Canvas,
    atlas: &Mask<'_>,
    text: &str,
    mut x: i32,
    baseline: i32,
    color: Rgb,
    alpha: u8,
    tracking: i32,
) {
    for byte in text.bytes() {
        let byte = if (0x20..=0x7e).contains(&byte) {
            byte
        } else {
            b'?'
        };
        let index = usize::from(byte - 0x20);
        let sx = (index % 16) * 9;
        let sy = (index / 16) * 20;
        blit_mask(canvas, atlas, sx, sy, 9, 20, x, baseline - 15, color, alpha);
        x += 9 + tracking;
    }
}

/// Blends an RGBA sprite into `canvas` at (`x`, `y`), scaling each sprite
/// pixel to `scale x scale` destination pixels.
///
/// Kept for future full-colour assets; the v1 design uses masks only.
#[allow(dead_code)]
pub fn blend_rgba(canvas: &mut Canvas, x: usize, y: usize, sprite: &Sprite<'_>, scale: usize) {
    let scale = scale.max(1);
    for sy in 0..sprite.height {
        for sx in 0..sprite.width {
            let index = (sy * sprite.width + sx) * 4;
            let alpha = u32::from(sprite.pixels[index + 3]);
            if alpha == 0 {
                continue;
            }
            let src = (
                sprite.pixels[index],
                sprite.pixels[index + 1],
                sprite.pixels[index + 2],
            );
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = x + sx * scale + dx;
                    let py = y + sy * scale + dy;
                    if px >= canvas.width || py >= canvas.height {
                        continue;
                    }
                    let dst = canvas.read_pixel(px, py);
                    let blended = (
                        blend_channel(src.0, dst.0, alpha),
                        blend_channel(src.1, dst.1, alpha),
                        blend_channel(src.2, dst.2, alpha),
                    );
                    let packed = canvas.pack(blended.0, blended.1, blended.2);
                    canvas.put_pixel(px, py, packed);
                }
            }
        }
    }
}

fn blend_channel(src: u8, dst: u8, alpha: u32) -> u8 {
    ((u32::from(src) * alpha + u32::from(dst) * (255 - alpha)) / 255) as u8
}

fn pack_channel(value: u8, mask_size: u8, mask_shift: u8) -> u32 {
    if mask_size == 0 || mask_size > 8 {
        return 0;
    }
    let max_value = (1u32 << mask_size) - 1;
    (u32::from(value) * max_value / 255) << mask_shift
}

fn unpack_channel(value: u32, mask_size: u8, mask_shift: u8) -> u8 {
    if mask_size == 0 || mask_size > 8 {
        return 0;
    }
    let max_value = (1u32 << mask_size) - 1;
    let raw = (value >> mask_shift) & max_value;
    (raw * 255 / max_value) as u8
}
