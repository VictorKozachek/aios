#!/usr/bin/env python3
"""Reference renderer for the AIOS boot splash design.

Everything here is drawn with the same primitives the bare-metal kernel will
use: a vertical gradient (one colour per row), 1 px rectangles, and RGBA /
8-bit-alpha bitmaps tinted at blit time.  No anti-aliased vector rasteriser is
assumed in the kernel: text is pre-rendered into a glyph atlas, the logo and
the wordmark are pre-rendered masks.

Outputs
-------
docs/design/variants/v1.png .. v3.png   1280x800 mockups
docs/design/variants/v1-800x600.png     800x600 proof for the chosen variant
docs/design/assets/*.png                RGBA masks (white ink + alpha)
docs/design/assets/wallpaper.jpg        1920x1080 Limine wallpaper

Usage: python3 tools/design/splash_render.py [--only v1,v2]
"""

from __future__ import annotations

import argparse
import math
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
DESIGN = ROOT / "docs" / "design"
FONTS = DESIGN / "fonts"
OUT_VARIANTS = DESIGN / "variants"
OUT_ASSETS = DESIGN / "assets"

INTER_LIGHT = FONTS / "Inter-Light.ttf"
INTER_REGULAR = FONTS / "Inter-Regular.ttf"
INTER_MEDIUM = FONTS / "Inter-Medium.ttf"
INTER_SEMIBOLD = FONTS / "Inter-SemiBold.ttf"
JBM_REGULAR = FONTS / "JetBrainsMono-Regular.ttf"
JBM_MEDIUM = FONTS / "JetBrainsMono-Medium.ttf"

SS = 4  # supersampling factor for masks

# --- typography -----------------------------------------------------------
MONO_SIZE = 15          # JetBrains Mono, integer advance 9 px (0.6 em)
MONO_ADVANCE = 9
MONO_LINE = 21
ATLAS_COLS = 16
ATLAS_ROWS = 6
ATLAS_FIRST = 0x20
ATLAS_LAST = 0x7E

# --- content (identical in every variant) ---------------------------------
STATUS_ROWS = [
    ("memory", "459 MiB"),
    ("acpi", "RSDP 0x1F3B3000"),
    ("cpus", "1"),
    ("tsc", "1005 MHz"),
]
GIT_HASH = "2214998"
VERSION = "v0.0.1"
CODENAME = "SkyNet"

LOG_LINES = [
    "[    0.000000 ] AIOS SkyNet v0.0.1 2214998 x86_64",
    "[    0.000412 ] framebuffer 1280x800 32bpp pitch 5120",
    "[    0.001907 ] memmap 33 entries, usable 459 MiB",
]


# ---------------------------------------------------------------------------
# small numeric helpers
# ---------------------------------------------------------------------------
def rgb(value: str) -> np.ndarray:
    value = value.lstrip("#")
    return np.array([int(value[i : i + 2], 16) for i in (0, 2, 4)], dtype=np.float64)


BAYER4 = np.array(
    [
        [0, 8, 2, 10],
        [12, 4, 14, 6],
        [3, 11, 1, 9],
        [15, 7, 13, 5],
    ],
    dtype=np.float64,
) / 16.0


def lerp(a, b, t):
    return a + (b - a) * t


def vertical_gradient(width: int, height: int, stops, dither: bool = True) -> np.ndarray:
    """Linear vertical gradient with 4x4 ordered dither; returns float HxWx3."""
    ys = (np.arange(height, dtype=np.float64) + 0.5) / height
    pos = np.array([s[0] for s in stops], dtype=np.float64)
    cols = np.stack([rgb(s[1]) for s in stops])
    out = np.empty((height, width, 3), dtype=np.float64)
    for c in range(3):
        out[:, :, c] = np.interp(ys, pos, cols[:, c])[:, None]
    if dither:
        tile = np.tile(BAYER4, (height // 4 + 1, width // 4 + 1))[:height, :width]
        out = np.floor(out + tile[..., None])
    else:
        out = np.round(out)
    return np.clip(out, 0, 255)


def blend(canvas: np.ndarray, mask: np.ndarray, color, opacity: float = 1.0,
          tint_rows: np.ndarray | None = None) -> None:
    """Integer-style alpha blend of a 0..1 mask onto a float HxWx3 canvas.

    tint_rows: optional (H,3) array, one colour per row (gradient tint).
    """
    if mask.shape[:2] != canvas.shape[:2]:
        raise ValueError(f"mask {mask.shape} != canvas {canvas.shape}")
    a = (mask * opacity)[..., None]
    if tint_rows is not None:
        col = tint_rows
    else:
        col = np.asarray(color, dtype=np.float64).reshape(1, 1, 3)
    canvas *= 1.0 - a
    canvas += col * a


# ---------------------------------------------------------------------------
# mask construction
# ---------------------------------------------------------------------------
def _down(img: Image.Image, size: tuple[int, int]) -> np.ndarray:
    return np.array(img.resize(size, Image.LANCZOS), dtype=np.float32) / 255.0


def text_mask(text: str, font_path: Path, size: int, tracking: float = 0.0):
    """Render text at `size` px with letter tracking (px) -> (alpha, baseline_px)."""
    f = ImageFont.truetype(str(font_path), size * SS)
    adv = [f.getlength(ch) for ch in text]
    total = sum(adv) + tracking * SS * max(0, len(text) - 1)
    asc, desc = f.getmetrics()
    w = int(math.ceil(total)) + 8 * SS
    h = asc + desc + 4 * SS
    img = Image.new("L", (w, h), 0)
    d = ImageDraw.Draw(img)
    x = 4 * SS
    base = 2 * SS + asc
    for ch, a in zip(text, adv):
        if ch != " ":
            d.text((x, base), ch, font=f, fill=255, anchor="ls")
        x += a + tracking * SS
    mask = _down(img, (max(1, round(w / SS)), max(1, round(h / SS))))
    return mask, base / SS


def ink_bounds(mask: np.ndarray, threshold: float = 0.01):
    ys, xs = np.where(mask > threshold)
    if len(ys) == 0:
        return None
    return xs.min(), ys.min(), xs.max(), ys.max()


def blit_text(canvas, text, font_path, size, color, *, baseline, x=None, cx=None,
              right=None, tracking=0.0, opacity=1.0, mask=None):
    """Place text by left edge, centre, or right edge (ink bounds)."""
    if mask is None:
        mask, base = text_mask(text, font_path, size, tracking)
    else:
        base = size
    b = ink_bounds(mask)
    if b is None:
        return
    x0, y0, x1, y1 = b
    if cx is not None:
        px = int(round(cx - (x0 + x1 + 1) / 2))
    elif right is not None:
        px = int(round(right - (x1 + 1)))
    else:
        px = int(round(x))
    py = int(round(baseline - base))
    h, w = mask.shape
    xx, yy = px, py
    # clip
    sx0, sy0 = max(0, -xx), max(0, -yy)
    dx0, dy0 = max(0, xx), max(0, yy)
    dx1, dy1 = min(canvas.shape[1], xx + w), min(canvas.shape[0], yy + h)
    if dx1 <= dx0 or dy1 <= dy0:
        return
    sub = mask[sy0 : sy0 + (dy1 - dy0), sx0 : sx0 + (dx1 - dx0)]
    a = (sub * opacity)[..., None]
    canvas[dy0:dy1, dx0:dx1] *= 1.0 - a
    canvas[dy0:dy1, dx0:dx1] += np.asarray(color, dtype=np.float64) * a


def rect_mask(canvas_shape, x0, y0, x1, y1) -> np.ndarray:
    m = np.zeros(canvas_shape[:2], dtype=np.float32)
    m[max(0, y0) : max(0, y1), max(0, x0) : max(0, x1)] = 1.0
    return m


def line_mask(canvas_shape, x0, y0, x1, y1, thickness=1.0) -> np.ndarray:
    """Anti-aliased straight segment, drawn at SS then downsampled."""
    H, W = canvas_shape[:2]
    img = Image.new("L", (W * SS, H * SS), 0)
    d = ImageDraw.Draw(img)
    d.line(
        [(x0 * SS, y0 * SS), (x1 * SS, y1 * SS)],
        fill=255,
        width=max(1, int(round(thickness * SS))),
    )
    return _down(img, (W, H))


def circle_mask(canvas_shape, cx, cy, radius, thickness=None, n=8) -> np.ndarray:
    """Filled disc or ring (thickness in px), analytic AA at n supersamples."""
    H, W = canvas_shape[:2]
    yy, xx = np.mgrid[0 : H * n, 0 : W * n].astype(np.float64)
    d = np.hypot(xx / n + 0.5 / n - cx - 0.5, yy / n + 0.5 / n - cy - 0.5)
    if thickness is None:
        a = np.clip(radius + 0.5 / n - d, 0, 1)
    else:
        a = np.clip(thickness / 2 + 0.5 / n - np.abs(d - radius), 0, 1)
    return a.reshape(H, n, W, n).mean(axis=(1, 3)).astype(np.float32)


def arc_mask(canvas_shape, cx, cy, radius, a0, a1, thickness=1.0, n=8) -> np.ndarray:
    """Annular arc with round caps, a0/a1 in degrees (screen coords, y down)."""
    H, W = canvas_shape[:2]
    yy, xx = np.mgrid[0 : H * n, 0 : W * n].astype(np.float64)
    dx = xx / n + 0.5 / n - cx - 0.5
    dy = yy / n + 0.5 / n - cy - 0.5
    d = np.hypot(dx, dy)
    a = np.clip(thickness / 2 + 0.5 / n - np.abs(d - radius), 0, 1)
    ang = (np.degrees(np.arctan2(dy, dx)) + 360.0) % 360.0
    if a0 <= a1:
        inside = (ang >= a0) & (ang <= a1)
    else:
        inside = (ang >= a0) | (ang <= a1)
    # round caps
    caps = np.zeros_like(a)
    for ang_deg in (a0, a1):
        r = math.radians(ang_deg)
        caps = np.maximum(caps, np.clip(thickness / 2 + 0.5 / n - np.hypot(dx - math.cos(r) * radius, dy - math.sin(r) * radius), 0, 1))
    a = np.maximum(np.where(inside, a, 0), caps)
    return a.reshape(H, n, W, n).mean(axis=(1, 3)).astype(np.float32)


# ---------------------------------------------------------------------------
# logo mark (shared by all variants and by the wallpaper)
# ---------------------------------------------------------------------------
def logo_mask(size: int, stroke: float = 4.6, gap=(-52, 214), pupil: float = 0.062) -> np.ndarray:
    """AIOS mark: an open 'aperture' ring with a centred pupil.

    Reads as a lens / eye, which is what the Limine wallpaper repeats at scale.
    `stroke` and `pupil` are percentages of `size`.
    """
    n = 8
    yy, xx = np.mgrid[0 : size * n, 0 : size * n].astype(np.float64)
    cx = cy = (size - 1) / 2 * n
    dx, dy = xx - cx, yy - cy
    d = np.hypot(dx, dy)

    radius = 0.462 * size * n
    half = stroke / 200.0 * size * n
    a = np.clip(half + 0.5 - np.abs(d - radius), 0, 1)
    ang = (np.degrees(np.arctan2(dy, dx)) + 360.0) % 360.0
    a0, a1 = gap
    inside = ((ang >= a0) & (ang <= a1)) if a0 <= a1 else ((ang >= a0) | (ang <= a1))
    caps = np.zeros_like(a)
    for ang_deg in (a0, a1):
        r = math.radians(ang_deg)
        caps = np.maximum(caps, np.clip(half + 0.5 - np.hypot(dx - math.cos(r) * radius, dy - math.sin(r) * radius), 0, 1))
    ring = np.maximum(np.where(inside, a, 0), caps)
    dot = np.clip(pupil * size * n + 0.5 - d, 0, 1)
    out = np.maximum(ring, dot)
    return out.reshape(size, n, size, n).mean(axis=(1, 3)).astype(np.float32)


# ---------------------------------------------------------------------------
# glyph atlas
# ---------------------------------------------------------------------------
class Atlas:
    def __init__(self):
        self.cell_w, self.cell_h, self.baseline = MONO_ADVANCE, 20, MONO_SIZE
        self.cols, self.rows = ATLAS_COLS, ATLAS_ROWS
        sheet = np.zeros((self.rows * self.cell_h, self.cols * self.cell_w), np.float32)
        gray = Image.new("L", (self.cols * self.cell_w, self.rows * self.cell_h), 0)
        d = ImageDraw.Draw(gray)
        f = ImageFont.truetype(str(JBM_REGULAR), MONO_SIZE * SS)
        self.cells = {}
        for code in range(ATLAS_FIRST, ATLAS_LAST + 1):
            i = code - ATLAS_FIRST
            gx, gy = (i % self.cols) * self.cell_w, (i // self.cols) * self.cell_h
            cell = Image.new("L", (self.cell_w * SS, self.cell_h * SS), 0)
            ImageDraw.Draw(cell).text((0, self.baseline * SS), chr(code), font=f, fill=255, anchor="ls")
            sheet[gy : gy + self.cell_h, gx : gx + self.cell_w] = _down(
                cell, (self.cell_w, self.cell_h)
            )
        self.sheet = sheet

    def width(self, text: str, tracking: int = 0) -> int:
        return len(text) * self.cell_w + max(0, len(text) - 1) * tracking

    def draw(self, canvas, text, x, baseline, color, *, tracking=0, opacity=1.0,
             right=None, cx=None, alpha=1.0):
        if cx is not None:
            x = cx - self.width(text, tracking) / 2
        if right is not None:
            x = right - self.width(text, tracking)
        x = int(round(x))
        y0 = int(round(baseline - self.baseline))
        for i, ch in enumerate(text):
            code = ord(ch)
            if code < ATLAS_FIRST or code > ATLAS_LAST:
                continue
            idx = code - ATLAS_FIRST
            gx, gy = (idx % self.cols) * self.cell_w, (idx // self.cols) * self.cell_h
            cell = self.sheet[gy : gy + self.cell_h, gx : gx + self.cell_w]
            px = x + i * (self.cell_w + tracking)
            xx, yy = int(round(px)), y0
            if xx >= canvas.shape[1] or yy >= canvas.shape[0]:
                continue
            x0, y0c = max(0, xx), max(0, yy)
            x1 = min(canvas.shape[1], xx + self.cell_w)
            y1 = min(canvas.shape[0], yy + self.cell_h)
            sub = cell[y0c - yy : y1 - yy, x0 - xx : x1 - xx]
            a = (sub * opacity)[..., None]
            canvas[y0c:y1, x0:x1] *= 1.0 - a
            canvas[y0c:y1, x0:x1] += np.asarray(color, dtype=np.float64) * a


ATLAS = Atlas()


# ---------------------------------------------------------------------------
# variant model
# ---------------------------------------------------------------------------
@dataclass
class Variant:
    key: str
    title: str
    mood: str
    bg: list
    ink: str
    ink2: str
    ink3: str
    accent: str
    hairline: str
    hairline_alpha: float
    wordmark_font: Path
    wordmark_cap: int
    wordmark_tracking: float
    wordmark_color: str
    logo_alpha: float = 1.0
    extra: dict = field(default_factory=dict)


VARIANTS = {
    "v1": Variant(
        key="v1",
        title="Void",
        mood="глубокий космос",
        bg=[(0.0, "#05070C"), (0.52, "#0A1120"), (1.0, "#04060A")],
        ink="#E8EEF9",
        ink2="#93A3BC",
        ink3="#4F5D73",
        accent="#86A9FF",
        hairline="#FFFFFF",
        hairline_alpha=0.07,
        wordmark_font=INTER_REGULAR,
        wordmark_cap=56,
        wordmark_tracking=0.30,
        wordmark_color="#E8EEF9",
    ),
    "v2": Variant(
        key="v2",
        title="Terminal",
        mood="терминальный минимализм",
        bg=[(0.0, "#070809"), (0.50, "#0A0C0F"), (1.0, "#060708")],
        ink="#D7DEE7",
        ink2="#7C8794",
        ink3="#414A54",
        accent="#8CD6A9",
        hairline="#FFFFFF",
        hairline_alpha=0.06,
        wordmark_font=JBM_MEDIUM,
        wordmark_cap=58,
        wordmark_tracking=0.10,
        wordmark_color="#E3E9F0",
    ),
    "v3": Variant(
        key="v3",
        title="Champagne",
        mood="премиальный тёмный",
        bg=[(0.0, "#0A0806"), (0.50, "#14110C"), (1.0, "#080605")],
        ink="#F4EEE3",
        ink2="#AFA490",
        ink3="#625A4C",
        accent="#C9A96E",
        hairline="#FFFFFF",
        hairline_alpha=0.055,
        wordmark_font=INTER_LIGHT,
        wordmark_cap=54,
        wordmark_tracking=0.42,
        wordmark_color="#F4EEE3",
    ),
}


# ---------------------------------------------------------------------------
# wordmark asset
# ---------------------------------------------------------------------------
def wordmark_asset(v: Variant) -> tuple[np.ndarray, int]:
    """Tight RGBA-ready mask of the AIOS wordmark -> (mask, cap_height)."""
    cap = v.wordmark_cap
    size = int(round(cap / 0.727))  # Inter cap height ~0.727 em
    tracking = v.wordmark_tracking * size
    mask, base = text_mask("AIOS", v.wordmark_font, size, tracking)
    b = ink_bounds(mask)
    x0, y0, x1, y1 = b
    tight = mask[y0 : y1 + 1, x0 : x1 + 1]
    return tight, base - y0


def save_mask_png(mask: np.ndarray, path: Path, ink=(255, 255, 255)) -> None:
    rgba = np.zeros((mask.shape[0], mask.shape[1], 4), np.uint8)
    rgba[..., 0] = ink[0]
    rgba[..., 1] = ink[1]
    rgba[..., 2] = ink[2]
    rgba[..., 3] = np.clip(mask * 255.0, 0, 255).astype(np.uint8)
    Image.fromarray(rgba, "RGBA").save(path)


def save_atlas_png(atlas: Atlas, path: Path) -> None:
    save_mask_png(atlas.sheet, path)


# ---------------------------------------------------------------------------
# variant layouts
# ---------------------------------------------------------------------------
def hero_geometry(W: int, H: int) -> dict:
    """Single-scale geometry: identical asset sizes on 1280x800 and 800x600."""
    mark = 64
    gap_mark_word = 38
    row_step = 28
    hero_h = mark + gap_mark_word + 58 + 34 + 24 + 22 + 4 * row_step + 6
    top = int(round(H * 0.42 - hero_h / 2))
    col_w = 480
    left = (W - col_w) // 2
    right = left + col_w
    return {
        "W": W, "H": H,
        "cx": W // 2,
        "mark_size": mark,
        "mark_top": top,
        "word_top": top + mark + gap_mark_word,
        "word_cap": 58,
        "sub_baseline": top + mark + gap_mark_word + 58 + 34,
        "hash_baseline": top + mark + gap_mark_word + 58 + 34 + 24,
        "rule_y": top + mark + gap_mark_word + 58 + 34 + 24 + 22,
        "status_baseline0": top + mark + gap_mark_word + 58 + 34 + 24 + 22 + 42,
        "status_step": row_step,
        "left": left, "right": right, "col_w": col_w,
        "log_baseline0": top + mark + gap_mark_word + 58 + 34 + 24 + 22 + 42 + 3 * row_step + 44,
        "log_step": MONO_LINE,
    }


def render_v1(v: Variant, W: int, H: int) -> np.ndarray:
    g = hero_geometry(W, H)
    canvas = vertical_gradient(W, H, v.bg)
    ink, ink2, ink3 = rgb(v.ink), rgb(v.ink2), rgb(v.ink3)
    accent = rgb(v.accent)
    cx = g["cx"]

    # mark
    m = logo_mask(g["mark_size"])
    ms = g["mark_size"]
    blend(canvas, _place(m, W, H, cx - ms // 2, g["mark_top"]), accent, 0.95)

    # wordmark
    wm, _ = wordmark_asset(v)
    blend(canvas, _place(wm, W, H, cx - wm.shape[1] // 2, g["word_top"]), rgb(v.wordmark_color))

    # subtitle:  SkyNet  ·  v0.0.1
    sub = f"{CODENAME} · {VERSION}"
    ATLAS.draw(canvas, sub, 0, g["sub_baseline"], ink2, tracking=1, cx=cx)

    # git hash
    ATLAS.draw(canvas, f"git {GIT_HASH}", 0, g["hash_baseline"], rgb("#6B7A91"), tracking=1, cx=cx)

    # divider
    r = g["rule_y"]
    blend(canvas, _place(line_mask((H, W), 0, 0, g["col_w"] - 1, 0, 1.0), W, H, g["left"], r),
          rgb(v.hairline), v.hairline_alpha)

    # status rows
    for i, (label, value) in enumerate(STATUS_ROWS):
        base = g["status_baseline0"] + i * g["status_step"]
        dot = circle_mask((H, W), g["left"] + 2, base - 5, 2.0)
        blend(canvas, dot, accent, 0.85)
        ATLAS.draw(canvas, label, g["left"] + 16, base, ink2)
        ATLAS.draw(canvas, value, 0, base, ink, right=g["right"])

    # log continuation
    for i, text in enumerate(LOG_LINES):
        base = g["log_baseline0"] + i * g["log_step"]
        ATLAS.draw(canvas, text, g["left"], base, ink3, opacity=0.75)
    return canvas


def render_v2(v: Variant, W: int, H: int) -> np.ndarray:
    canvas = vertical_gradient(W, H, v.bg)
    ink, ink2, ink3 = rgb(v.ink), rgb(v.ink2), rgb(v.ink3)
    accent = rgb(v.accent)
    margin = 104 if W >= 1100 else 88
    rail_x = margin - 40

    # vertical rail + ticks
    rail = line_mask((H, W), 0, 0, 0, H - 160, 1.0)
    blend(canvas, _place(rail, W, H, rail_x, 120), rgb(v.hairline), 0.12)
    for y in range(120, H - 40, 24):
        t = line_mask((H, W), 0, 0, 5, 0, 1.0)
        blend(canvas, _place(t, W, H, rail_x, y), rgb(v.hairline), 0.10)

    # top-right hash
    ATLAS.draw(canvas, f"git {GIT_HASH}", 0, 150, ink3, right=W - margin)

    # wordmark (mono, big)
    cap = v.wordmark_cap
    size = int(round(cap / 0.73))
    tracking = v.wordmark_tracking * size
    wmask, wbase = text_mask("AIOS", v.wordmark_font, size, tracking)
    b = ink_bounds(wmask)
    wmask = wmask[b[1] : b[3] + 1, b[0] : b[2] + 1]
    word_top = 186
    blend(canvas, _place(wmask, W, H, margin, word_top), rgb(v.wordmark_color))

    # subtitle
    sub_base = word_top + wmask.shape[0] + 30
    ATLAS.draw(canvas, f"{CODENAME} {VERSION}", margin, sub_base, ink2, tracking=1)

    # status table
    base0 = sub_base + 62
    for i, (label, value) in enumerate(STATUS_ROWS):
        base = base0 + i * 28
        ATLAS.draw(canvas, "[ ok ]", margin, base, accent, opacity=0.9)
        ATLAS.draw(canvas, label, margin + 84, base, ink2)
        ATLAS.draw(canvas, value, margin + 232, base, ink)

    # log continuation
    log0 = base0 + 4 * 28 + 44
    for i, text in enumerate(LOG_LINES):
        base = log0 + i * MONO_LINE
        ATLAS.draw(canvas, text, margin, base, ink3, opacity=0.8)
    if log0 + len(LOG_LINES) * MONO_LINE + 8 < H - 40:
        cur = rect_mask((H, W), 0, 0, MONO_ADVANCE - 2, 13)
        blend(canvas, _place(cur, W, H, margin, log0 + len(LOG_LINES) * MONO_LINE + 3), ink3, 0.75)
    return canvas


def render_v3(v: Variant, W: int, H: int) -> np.ndarray:
    canvas = vertical_gradient(W, H, v.bg)
    ink, ink2, ink3 = rgb(v.ink), rgb(v.ink2), rgb(v.ink3)
    accent = rgb(v.accent)
    cx = W // 2
    # slightly airier vertical rhythm than v1
    mark = 56
    top = int(round(H * 0.40 - 170))
    word_top = top + mark + 32
    wm, _ = wordmark_asset(v)
    blend(canvas, _place(logo_mask(mark), W, H, cx - mark // 2, top), accent, 0.95)
    blend(canvas, _place(wm, W, H, cx - wm.shape[1] // 2, word_top), rgb(v.wordmark_color))

    # short accent rule under the wordmark
    rule_y = word_top + wm.shape[0] + 22
    blend(canvas, _place(line_mask((H, W), 0, 0, 72, 0, 1.0), W, H, cx - 36, rule_y), accent, 0.55)

    sub_base = rule_y + 34
    ATLAS.draw(canvas, f"{CODENAME}  {VERSION}", 0, sub_base, ink2, tracking=2, cx=cx)
    ATLAS.draw(canvas, f"git {GIT_HASH}", 0, sub_base + 24, ink3, tracking=1, cx=cx)

    # spec-sheet status block
    col_w = 520
    left = cx - col_w // 2
    right = left + col_w
    y0 = sub_base + 24 + 34
    for i, (label, value) in enumerate(STATUS_ROWS):
        row_y = y0 + i * 32
        blend(canvas, _place(line_mask((H, W), 0, 0, col_w - 1, 0, 1.0), W, H, left, row_y - 14),
              rgb(v.hairline), v.hairline_alpha)
        base = row_y + 8
        ATLAS.draw(canvas, label.upper(), left, base, ink3, tracking=2)
        ATLAS.draw(canvas, value, 0, base, ink, right=right)

    log0 = y0 + 4 * 32 + 24
    for i, text in enumerate(LOG_LINES):
        ATLAS.draw(canvas, text, left, log0 + i * MONO_LINE, ink3, opacity=0.7)
    return canvas


def _place(mask: np.ndarray, W: int, H: int, x: int, y: int) -> np.ndarray:
    out = np.zeros((H, W), np.float32)
    h, w = mask.shape
    x, y = int(x), int(y)
    sx0, sy0 = max(0, -x), max(0, -y)
    dx0, dy0 = max(0, x), max(0, y)
    dx1, dy1 = min(W, x + w), min(H, y + h)
    if dx1 <= dx0 or dy1 <= dy0:
        return out
    out[dy0:dy1, dx0:dx1] = mask[sy0 : sy0 + (dy1 - dy0), sx0 : sx0 + (dx1 - dx0)]
    return out


RENDERERS = {"v1": render_v1, "v2": render_v2, "v3": render_v3}


# ---------------------------------------------------------------------------
# Limine wallpaper
# ---------------------------------------------------------------------------
def eyelid_mask(W, H, cx, cy, rx, ry, thickness, n=2):
    """Eyelid curves (upper + lower) - the 'eye' motif, drawn as two arcs."""
    yy, xx = np.mgrid[0 : H * n, 0 : W * n].astype(np.float64)
    dx = (xx / n + 0.5 / n - cx) / rx
    dy = (yy / n + 0.5 / n - cy) / ry
    # upper lid: y = -sqrt(1-x^2) part of ellipse; distance approx via |dy + sqrt(1-dx^2)| * ry
    upper = np.abs(dy + np.sqrt(np.clip(1 - dx * dx, 0, 1))) * ry
    lower = np.abs(dy - np.sqrt(np.clip(1 - dx * dx, 0, 1))) * ry
    d = np.minimum(upper, lower)
    a = np.clip((thickness / 2 + 0.5 / n - d) * 1.0, 0, 1) * (np.abs(dx) <= 1.02)
    return a.reshape(H, n, W, n).mean(axis=(1, 3)).astype(np.float32)


def make_wallpaper(v: Variant = VARIANTS["v1"]) -> Image.Image:
    W, H = 1920, 1080
    canvas = vertical_gradient(W, H, v.bg, dither=True)
    accent = np.array([0.62, 0.74, 1.0])
    ex, ey = 1420, 540

    # soft halo around the eye (very low contrast, wide)
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float64)
    rr = np.hypot((xx - ex) / 1.25, (yy - ey))
    halo = np.exp(-(rr / 520.0) ** 2) * 10.0  # up to +10 levels
    canvas += halo[..., None] * accent.reshape(1, 1, 3) * 0.55

    def add(mask, color, alpha, mode="over"):
        a = (mask * alpha)[..., None]
        canvas[:] = canvas * (1 - a) + np.asarray(color).reshape(1, 1, 3) * a

    # concentric rings - the echo of the splash mark
    rings = [
        (86, 1.2, 0.10, -70, 250),
        (112, 0.8, 0.07, 20, 200),
        (158, 1.4, 0.06, -110, 190),
        (214, 1.0, 0.05, 40, 260),
        (288, 1.6, 0.045, -80, 240),
        (378, 1.0, 0.038, 10, 210),
        (492, 1.6, 0.032, -100, 200),
        (638, 1.2, 0.026, 30, 250),
        (812, 1.8, 0.020, -70, 240),
    ]
    for r, thick, alpha, a0, a1 in rings:
        add(arc_mask((H, W), ex, ey, r, a0, a1, thick, n=1), accent, alpha)

    # iris ticks between two rings
    for i in range(64):
        ang = -72 + i * 4.6
        rad = math.radians(ang)
        r0, r1 = 236, 256 + (i % 5) * 9
        x0, y0 = ex + math.cos(rad) * r0, ey + math.sin(rad) * r0
        x1, y1 = ex + math.cos(rad) * r1, ey + math.sin(rad) * r1
        m = line_mask((H, W), x0, y0, x1, y1, 1.0)
        add(m, accent, 0.028)

    # eyelid hint (very faint, keeps the "eye" reading)
    add(eyelid_mask(W, H, ex, ey, 560, 330, 1.2, n=1), accent, 0.030)

    # pupil: soft disc + bright centre
    pupil = circle_mask((H, W), ex, ey, 34, n=1)
    add(pupil, accent, 0.055)
    glow = np.exp(-((np.hypot(xx - ex, yy - ey)) / 120.0) ** 2)
    canvas += glow[..., None] * accent.reshape(1, 1, 3) * 26.0
    add(circle_mask((H, W), ex, ey, 9, n=1), accent, 0.35)

    # vignette: calm centre, darker edges
    vx = np.clip(np.abs(xx - W / 2) / (W / 2), 0, 1)
    vy = np.clip(np.abs(yy - H / 2) / (H / 2), 0, 1)
    vig = 1.0 - 0.30 * (0.55 * vx**2 + 0.45 * vy**2)
    canvas *= vig[..., None]

    # fine grain (also keeps JPEG from banding in the flat gradient)
    rng = np.random.default_rng(7)
    canvas += rng.normal(0.0, 0.9, size=(H, W, 1))

    return Image.fromarray(np.clip(canvas, 0, 255).astype(np.uint8), "RGB")


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------
def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="v1,v2,v3")
    args = ap.parse_args()

    OUT_VARIANTS.mkdir(parents=True, exist_ok=True)
    OUT_ASSETS.mkdir(parents=True, exist_ok=True)

    keys = args.only.split(",")

    # shared assets -------------------------------------------------------
    logo = logo_mask(64)
    save_mask_png(logo, OUT_ASSETS / "logo-aios.png")
    for key in ("v1", "v3"):
        wm, _ = wordmark_asset(VARIANTS[key])
        save_mask_png(wm, OUT_ASSETS / f"wordmark-aios-{key}.png")
    wm2, _ = wordmark_asset(VARIANTS["v2"])
    save_mask_png(wm2, OUT_ASSETS / "wordmark-aios-mono.png")
    save_atlas_png(ATLAS, OUT_ASSETS / "atlas-jbm15.png")

    # mockups -------------------------------------------------------------
    for key in keys:
        v = VARIANTS[key]
        if key == "v3":
            # v3 uses a slightly wider tracking; regenerate its own wordmark
            pass
        for W, H, name in ((1280, 800, f"{key}.png"), (800, 600, f"{key}-800x600.png")):
            canvas = RENDERERS[key](v, W, H)
            img = Image.fromarray(np.clip(np.round(canvas), 0, 255).astype(np.uint8), "RGB")
            img.save(OUT_VARIANTS / name)
            print("wrote", OUT_VARIANTS / name)

    # wallpaper -----------------------------------------------------------
    if "wallpaper" not in keys:
        return
    wp = make_wallpaper(VARIANTS["v1"])
    wp.save(OUT_ASSETS / "wallpaper.jpg", quality=92, subsampling=0, optimize=True)
    print("wrote", OUT_ASSETS / "wallpaper.jpg", (OUT_ASSETS / "wallpaper.jpg").stat().st_size)


if __name__ == "__main__":
    main()
