//! Painter: laid-out blocks → pixels in a `0x00RRGGBB` buffer.
//!
//! Everything here is plain CPU code over a `&mut [u32]`, so it runs headless (tests, goldens)
//! exactly as it does into a softbuffer surface. Blending is plain sRGB alpha, no gamma.
//! Rectangles snap to whole pixels so rules and borders stay crisp; only rounded corners are
//! anti-aliased. Glyph x positions keep quarter-pixel precision through the glyph cache.

use std::time::{Duration, Instant};

use crate::fonts::Fonts;
use crate::image::Bitmap;
use crate::layout::{BlockBox, Layout};
use crate::text::RectKind;
use crate::text::cache::GlyphCache;
use crate::theme::{Rgb, Theme};

/// A pixel buffer: `height` rows of `width` pixels, `stride` pixels apart.
pub struct Frame<'a> {
    buf: &'a mut [u32],
    width: usize,
    height: usize,
    stride: usize,
    /// Drawing is limited to this rectangle (always inside the buffer).
    clip: Clip,
}

/// A pixel rectangle, `x0..x1` × `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clip {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Clip {
    fn intersect(self, o: Clip) -> Clip {
        Clip {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }

    fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

impl<'a> Frame<'a> {
    /// Panics if `buf` is too small for `height` rows of `stride` (the last row may be just
    /// `width` long).
    pub fn new(buf: &'a mut [u32], width: usize, height: usize, stride: usize) -> Frame<'a> {
        assert!(stride >= width, "stride {stride} < width {width}");
        // Checked: a wrapped product would let a too-small buffer through.
        let needed = height
            .checked_sub(1)
            .map_or(Some(0), |rows| rows.checked_mul(stride)?.checked_add(width));
        assert!(
            needed.is_some_and(|n| buf.len() >= n),
            "buffer of {} pixels is too small for {width}×{height} (stride {stride})",
            buf.len()
        );
        // Sizes past i32 can't be addressed by the i32 drawing coordinates; clip to them.
        let (w, h) = (width.min(i32::MAX as usize), height.min(i32::MAX as usize));
        Frame {
            buf,
            width,
            height,
            stride,
            clip: Clip {
                x0: 0,
                y0: 0,
                x1: w as i32,
                y1: h as i32,
            },
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// The whole buffer.
    fn bounds(&self) -> Clip {
        Clip {
            x0: 0,
            y0: 0,
            x1: self.width.min(i32::MAX as usize) as i32,
            y1: self.height.min(i32::MAX as usize) as i32,
        }
    }

    /// Limit drawing to `clip` (within the buffer) and return the previous clip.
    pub fn set_clip(&mut self, clip: Clip) -> Clip {
        let clip = clip.intersect(self.bounds());
        std::mem::replace(&mut self.clip, clip)
    }

    fn row(&mut self, y: i32) -> &mut [u32] {
        let start = y as usize * self.stride;
        &mut self.buf[start..start + self.width]
    }

    /// Fill `x0..x1` × `y0..y1` with an opaque color.
    pub fn fill_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb) {
        let c = self.clip.intersect(Clip { x0, y0, x1, y1 });
        if c.is_empty() {
            return;
        }
        for y in c.y0..c.y1 {
            self.row(y)[c.x0 as usize..c.x1 as usize].fill(color);
        }
    }

    /// A one-pixel horizontal line `x0..x1` at row `y`.
    pub fn hline(&mut self, x0: i32, x1: i32, y: i32, color: Rgb) {
        self.fill_rect(x0, y, x1, y.saturating_add(1), color);
    }

    /// Fill a rounded rectangle, or stroke its outline `stroke` pixels wide, with corners of
    /// radius `radius` anti-aliased. Edges are whole pixels. `alpha` scales the coverage.
    #[allow(clippy::too_many_arguments)]
    pub fn rounded_rect(
        &mut self,
        (x0, y0, x1, y1): (i32, i32, i32, i32),
        radius: f32,
        stroke: Option<f32>,
        color: Rgb,
        alpha: u8,
    ) {
        let c = self.clip.intersect(Clip { x0, y0, x1, y1 });
        if c.is_empty() || alpha == 0 {
            return;
        }
        // f64 and i64 throughout: i32 extents overflow when subtracted, and f32 loses the
        // pixel when a box reaches far past the clip.
        let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
        let (w, h) = ((x1 - x0) as f64, (y1 - y0) as f64);
        let r = f64::from(radius).clamp(0.0, w.min(h) / 2.0);
        // A stroke that meets itself is a fill.
        let stroke = stroke
            .map(f64::from)
            .filter(|&s| s > 0.0 && 2.0 * s < w.min(h));
        if r < 0.5 && stroke.is_none() && alpha == 255 {
            self.fill_rect(c.x0, c.y0, c.x1, c.y1, color);
            return;
        }
        let outer = RoundBox::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64, r);
        let inner = stroke.map(|s| {
            RoundBox::new(
                x0 as f64 + s,
                y0 as f64 + s,
                x1 as f64 - s,
                y1 as f64 - s,
                (r - s).max(0.0),
            )
        });
        // Coverage varies with x only within `side` columns of each edge (the corners and the
        // stroke). In between it depends on the row alone: one value per row, then a span.
        let mid = match inner {
            Some(b) => b.hw - b.r,
            None => outer.hw - outer.r,
        };
        let side = ((outer.hw - mid.max(0.0)).ceil() as i64 + 1).min(x1 - x0);
        let (left_end, right_start) = (x0 + side, (x1 - side).max(x0 + side));
        // Both lie within x0..=x1, and the clip is inside i32.
        let (left_end, right_start) = (left_end as i32, right_start as i32);
        let a = f64::from(alpha);
        let coverage = |fx: f64, py: f64| {
            let mut cov = outer.coverage(fx, py);
            if let Some(inner) = inner {
                cov -= inner.coverage(fx, py);
            }
            ((cov * a + 0.5) as u32).min(255)
        };
        for y in c.y0..c.y1 {
            let py = f64::from(y) + 0.5;
            let row_start = y as usize * self.stride;
            let row = &mut self.buf[row_start..row_start + self.width];
            let mut px = |x: i32| {
                let cov = coverage(f64::from(x) + 0.5, py);
                if cov > 0 {
                    let d = &mut row[x as usize];
                    *d = blend(*d, color, cov);
                }
            };
            (c.x0..c.x1.min(left_end)).for_each(&mut px);
            (c.x0.max(right_start)..c.x1).for_each(&mut px);
            let (m0, m1) = (c.x0.max(left_end), c.x1.min(right_start));
            if m0 < m1 {
                let span = &mut row[m0 as usize..m1 as usize];
                match coverage(outer.cx, py) {
                    0 => {}
                    255 => span.fill(color),
                    cov => span.iter_mut().for_each(|d| *d = blend(*d, color, cov)),
                }
            }
        }
    }

    /// Tint an 8-bit coverage mask (`w` × `h`, rows packed) with `color`, its top-left corner at
    /// (`x`, `y`).
    pub fn blit_mask(&mut self, x: i32, y: i32, w: u32, h: u32, mask: &[u8], color: Rgb) {
        let Some((c, sx, sy)) = self.blit_area(x, y, w, h) else {
            return;
        };
        let w = w as usize;
        for (row_i, y) in (c.y0..c.y1).enumerate() {
            let src_start = (sy + row_i) * w + sx;
            let src = &mask[src_start..src_start + (c.x1 - c.x0) as usize];
            let dst = &mut self.row(y)[c.x0 as usize..c.x1 as usize];
            for (d, &a) in dst.iter_mut().zip(src) {
                match a {
                    0 => {}
                    255 => *d = color,
                    a => *d = blend(*d, color, a as u32),
                }
            }
        }
    }

    /// Composite premultiplied RGBA pixels (`w` × `h`, rows packed) with their top-left corner
    /// at (`x`, `y`).
    pub fn blit_rgba(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: &[u8]) {
        let Some((c, sx, sy)) = self.blit_area(x, y, w, h) else {
            return;
        };
        let w = w as usize;
        for (row_i, y) in (c.y0..c.y1).enumerate() {
            let src_start = ((sy + row_i) * w + sx) * 4;
            let src = &rgba[src_start..src_start + (c.x1 - c.x0) as usize * 4];
            let dst = &mut self.row(y)[c.x0 as usize..c.x1 as usize];
            for (d, p) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
                *d = over_premul(*d, p[0], p[1], p[2], p[3]);
            }
        }
    }

    /// Composite a premultiplied bitmap into the `w` × `h` box at (`x`, `y`), scaled (nearest
    /// pixel) when it has another size.
    pub fn blit_bitmap(&mut self, x: i32, y: i32, w: u32, h: u32, bm: &Bitmap) {
        let Some((c, sx, sy)) = self.blit_area(x, y, w, h) else {
            return;
        };
        if bm.width == 0 || bm.height == 0 {
            return;
        }
        let same = (bm.width, bm.height) == (w, h);
        let src_x = |dx: usize| {
            if same {
                dx
            } else {
                (dx as u64 * u64::from(bm.width) / u64::from(w)) as usize
            }
        };
        let cols: Vec<usize> = (0..(c.x1 - c.x0) as usize).map(|i| src_x(sx + i)).collect();
        for (row_i, y) in (c.y0..c.y1).enumerate() {
            let dy = sy + row_i;
            let src_y = if same {
                dy
            } else {
                (dy as u64 * u64::from(bm.height) / u64::from(h)) as usize
            };
            let row = &bm.data[src_y * bm.width as usize..][..bm.width as usize];
            let dst = &mut self.row(y)[c.x0 as usize..c.x1 as usize];
            for (d, &sx) in dst.iter_mut().zip(&cols) {
                let p = row[sx];
                let a = p >> 24;
                *d = match a {
                    255 => p & 0xff_ffff,
                    0 => *d,
                    _ => over_premul(*d, (p >> 16) as u8, (p >> 8) as u8, p as u8, a as u8),
                };
            }
        }
    }

    /// Visible part of a `w` × `h` image at (`x`, `y`): the clipped rectangle and the source
    /// offset of its top-left pixel.
    fn blit_area(&self, x: i32, y: i32, w: u32, h: u32) -> Option<(Clip, usize, usize)> {
        let (x1, y1) = (
            x.saturating_add(w.min(i32::MAX as u32) as i32),
            y.saturating_add(h.min(i32::MAX as u32) as i32),
        );
        let c = self.clip.intersect(Clip {
            x0: x,
            y0: y,
            x1,
            y1,
        });
        if c.is_empty() {
            return None;
        }
        Some((c, (c.x0 - x) as usize, (c.y0 - y) as usize))
    }
}

/// A rounded box for coverage tests: center, half extents, corner radius.
#[derive(Clone, Copy)]
struct RoundBox {
    cx: f64,
    cy: f64,
    hw: f64,
    hh: f64,
    r: f64,
}

impl RoundBox {
    fn new(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> RoundBox {
        RoundBox {
            cx: (x0 + x1) / 2.0,
            cy: (y0 + y1) / 2.0,
            hw: ((x1 - x0) / 2.0).max(0.0),
            hh: ((y1 - y0) / 2.0).max(0.0),
            r,
        }
    }

    /// Fraction of the pixel centered at (`px`, `py`) inside the box, from its signed distance.
    fn coverage(&self, px: f64, py: f64) -> f64 {
        let qx = (px - self.cx).abs() - (self.hw - self.r);
        let qy = (py - self.cy).abs() - (self.hh - self.r);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        let d = outside + qx.max(qy).min(0.0) - self.r;
        (0.5 - d).clamp(0.0, 1.0)
    }
}

/// `dst` blended toward `src` by `a`/255.
#[inline(always)]
fn blend(dst: u32, src: u32, a: u32) -> u32 {
    // 0..=255 → 0..=256 so full coverage reproduces `src` exactly.
    let a = a + (a >> 7);
    let na = 256 - a;
    // Rounded: + half of 256 in each channel.
    let rb = ((src & 0xff00ff) * a + (dst & 0xff00ff) * na + 0x800080) >> 8;
    let g = ((src & 0x00ff00) * a + (dst & 0x00ff00) * na + 0x008000) >> 8;
    (rb & 0xff00ff) | (g & 0x00ff00)
}

/// Premultiplied `rgba` over `dst`.
#[inline(always)]
fn over_premul(dst: u32, r: u8, g: u8, b: u8, a: u8) -> u32 {
    match a {
        0 if (r | g | b) == 0 => dst,
        255 => (r as u32) << 16 | (g as u32) << 8 | b as u32,
        _ => {
            let na = 255 - a as u32;
            let ch = |s: u8, d: u32| (s as u32 + (d * na + 127) / 255).min(255);
            ch(r, dst >> 16 & 0xff) << 16 | ch(g, dst >> 8 & 0xff) << 8 | ch(b, dst & 0xff)
        }
    }
}

/// Where the document area sits in the frame and how far it is scrolled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Document area in frame pixels (its width is the layout width).
    pub area: Clip,
    /// Document y shown at the top of the area. f64: tall documents outgrow f32's whole pixels.
    pub scroll: f64,
}

impl Viewport {
    /// The area's height in pixels (zero if inverted). f64 and i64: the i32 extents of an
    /// area reaching far off screen overflow when subtracted, and f32 loses whole pixels.
    pub fn height(&self) -> f64 {
        (i64::from(self.area.y1) - i64::from(self.area.y0)).max(0) as f64
    }
}

/// Paint the document area: background, then every block overlapping the viewport that has
/// content (call [`Layout::ensure`] first). Blocks without content are left blank.
pub fn paint(
    frame: &mut Frame,
    vp: Viewport,
    layout: &Layout,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    theme: &Theme,
) {
    let prev = frame.set_clip(vp.area);
    let a = frame.clip;
    if !a.is_empty() {
        frame.fill_rect(a.x0, a.y0, a.x1, a.y1, theme.bg);
        let (col_x, _) = layout.column();
        let ox = vp.area.x0 as f32 + col_x;
        // Only the part of the area inside the frame, in document y (exact: i64 offsets). A NaN
        // scroll counts as the top.
        let scroll = if vp.scroll.is_nan() { 0.0 } else { vp.scroll };
        let top = scroll + (i64::from(a.y0) - i64::from(vp.area.y0)) as f64;
        let bottom = scroll + (i64::from(a.y1) - i64::from(vp.area.y0)) as f64;
        for i in layout.range_exact(top, bottom) {
            let Some(content) = layout.geoms()[i].content.as_deref() else {
                continue;
            };
            let oy = (vp.area.y0 as f64 + layout.top(i) - scroll) as f32;
            paint_block(frame, content, ox, oy, fonts, cache, theme);
        }
    }
    frame.set_clip(prev);
}

/// Snap `v` to the pixel grid.
fn px(v: f32) -> i32 {
    v.round()
        .clamp(i32::MIN as f32 / 2.0, i32::MAX as f32 / 2.0) as i32
}

/// Snap the span `v..v + len` to whole pixels, at least one pixel long if `len` is positive.
fn span(v: f32, len: f32) -> (i32, i32) {
    let a = px(v);
    let b = px(v + len);
    if len > 0.0 { (a, b.max(a + 1)) } else { (a, b) }
}

/// Paint one block's content with its origin at (`ox`, `oy`) in frame pixels (within the
/// frame's clip). [`paint`] uses it for document blocks; the app shell for its own text.
pub fn paint_block(
    frame: &mut Frame,
    content: &BlockBox,
    ox: f32,
    oy: f32,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    theme: &Theme,
) {
    let clip = frame.clip;
    for r in &content.rects {
        let (x0, x1) = span(ox + r.x, r.w);
        let (y0, y1) = span(oy + r.y, r.h);
        if y1 <= clip.y0 || y0 >= clip.y1 {
            continue;
        }
        let color = theme.color(r.role);
        match r.kind {
            RectKind::Fill => frame.rounded_rect((x0, y0, x1, y1), r.radius, None, color, 255),
            RectKind::Stroke(s) => frame.rounded_rect(
                (x0, y0, x1, y1),
                r.radius,
                Some(s.round().max(1.0)),
                color,
                255,
            ),
        }
    }
    for g in &content.glyphs {
        let style = &content.styles[g.style as usize];
        let baseline = px(oy + g.y);
        // Glyphs reach at most about one size above and below their baseline.
        // (Capped, with `baseline` within i32/2: the sums below can't overflow.)
        let reach = style.size.min(1e6) as i32 * 2 + 2;
        if baseline + reach < clip.y0 || baseline - reach >= clip.y1 {
            continue;
        }
        let x = ox + g.x;
        if x + style.size * 4.0 < clip.x0 as f32 || x - style.size * 4.0 >= clip.x1 as f32 {
            continue;
        }
        let (key, xi) = crate::text::cache::GlyphKey::at(g.face, g.id, style.size, x, g.synth);
        let Some(img) = cache.get(fonts, key) else {
            continue;
        };
        let (gx, gy) = (
            xi.saturating_add(img.left),
            baseline.saturating_sub(img.top),
        );
        if img.color {
            frame.blit_rgba(gx, gy, img.width, img.height, &img.data);
        } else {
            frame.blit_mask(
                gx,
                gy,
                img.width,
                img.height,
                &img.data,
                theme.color(style.role),
            );
        }
    }
}

/// The pixels for inline image boxes (see `image.rs`).
pub trait Pixels {
    /// The bitmap to show in box `id`, `w` × `h` physical pixels: that size, another one
    /// while it's decoded again (scaled to fit), or none yet.
    fn pixels(&mut self, id: u32, w: u32, h: u32) -> Option<&Bitmap>;
}

/// Paint the image boxes of the blocks in the viewport, after [`paint`]: their pixels, or a
/// tinted box with an outline while they load.
pub fn paint_images(
    frame: &mut Frame,
    vp: Viewport,
    layout: &Layout,
    pixels: &mut dyn Pixels,
    theme: &Theme,
) {
    let prev = frame.set_clip(vp.area);
    let a = frame.clip;
    let (col_x, _) = layout.column();
    let ox = vp.area.x0 as f32 + col_x;
    let scroll = if vp.scroll.is_nan() { 0.0 } else { vp.scroll };
    let top = scroll + (i64::from(a.y0) - i64::from(vp.area.y0)) as f64;
    let bottom = scroll + (i64::from(a.y1) - i64::from(vp.area.y0)) as f64;
    for i in layout.range_exact(top, bottom) {
        let Some(content) = layout.geoms()[i].content.as_deref() else {
            continue;
        };
        let oy = (vp.area.y0 as f64 + layout.top(i) - scroll) as f32;
        for b in &content.boxes {
            let (x0, x1) = span(ox + b.x, b.b.width);
            let (y0, y1) = span(oy + b.y, b.b.ascent + b.b.descent);
            if y1 <= a.y0 || y0 >= a.y1 {
                continue;
            }
            let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
            match pixels.pixels(b.b.id, w, h) {
                Some(bm) => frame.blit_bitmap(x0, y0, w, h, bm),
                None => {
                    frame.fill_rect(x0, y0, x1, y1, theme.code_bg);
                    frame.rounded_rect((x0, y0, x1, y1), 0.0, Some(1.0), theme.border, 255);
                }
            }
        }
    }
    frame.set_clip(prev);
}

// ---- Scrollbar ----------------------------------------------------------------------------

/// How long the scrollbar stays visible after the last scroll.
pub const SCROLLBAR_LINGER: Duration = Duration::from_secs(1);
/// Thumb width, margin from the edge and minimum length, in logical pixels.
const THUMB_W: f32 = 6.0;
const THUMB_MARGIN: f32 = 2.0;
const THUMB_MIN: f32 = 24.0;
/// Thumb opacity over the content.
const THUMB_ALPHA: u8 = 140;

/// The scrollbar thumb in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

/// The thumb for a document `total` pixels tall in `vp`, or `None` when it all fits. `scale`
/// is physical pixels per logical pixel.
pub fn thumb(vp: Viewport, total: f32, scale: f32) -> Option<Thumb> {
    let view = vp.height();
    let total = total as f64;
    // `is_nan` too: a NaN `total` fits nothing either.
    if view <= 0.0 || total <= view || total.is_nan() {
        return None;
    }
    let scale = scale.max(0.01) as f64;
    let margin = (THUMB_MARGIN as f64 * scale).round();
    let track = view - 2.0 * margin;
    if track < 1.0 {
        return None;
    }
    let len = (track * view / total)
        .max(THUMB_MIN as f64 * scale)
        .max(1.0)
        .min(track);
    let max_scroll = total - view;
    // NaN (a NaN scroll, or infinity over infinity) counts as the top.
    let frac = (vp.scroll / max_scroll).clamp(0.0, 1.0);
    let pos = if frac.is_nan() { 0.0 } else { frac } * (track - len);
    let w = (THUMB_W as f64 * scale).round().max(1.0);
    // f64 throughout, narrowed last: an area near the i32 limits (or a huge `scale`) puts the
    // thumb past them. Saturating keeps whatever part is on screen exactly where it belongs.
    let x1 = f64::from(vp.area.x1) - margin;
    // Both ends are rounded from their own position (not top + rounded length): with .5
    // fractions the two roundings would add up to a pixel past the margin. `len >= 1` keeps
    // the thumb at least a pixel tall.
    let top = f64::from(vp.area.y0);
    Some(Thumb {
        x0: (x1 - w) as i32,
        y0: (top + (margin + pos).round()) as i32,
        x1: x1 as i32,
        y1: (top + (margin + pos + len).round()) as i32,
    })
}

/// Paint the overlay thumb over the document area (after [`paint`]).
pub fn paint_scrollbar(frame: &mut Frame, vp: Viewport, total: f32, scale: f32, theme: &Theme) {
    let Some(t) = thumb(vp, total, scale) else {
        return;
    };
    let prev = frame.set_clip(vp.area);
    // i64: a saturated thumb can span more than i32.
    let r = (i64::from(t.x1) - i64::from(t.x0)) as f32 / 2.0;
    frame.rounded_rect((t.x0, t.y0, t.x1, t.y1), r, None, theme.muted, THUMB_ALPHA);
    frame.set_clip(prev);
}

/// Scrollbar visibility: shown on scroll, hidden [`SCROLLBAR_LINGER`] after the last one (at
/// once, no animation). A single deadline, so an idle window wakes once to hide it and then sleeps.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollbarFade {
    until: Option<Instant>,
}

impl ScrollbarFade {
    /// Show the scrollbar (on scroll) and push the hide deadline back.
    pub fn poke(&mut self, now: Instant) {
        // An instant too close to the end of time to add to has no deadline to wait for.
        self.until = Some(now.checked_add(SCROLLBAR_LINGER).unwrap_or(now));
    }

    pub fn visible(&self, now: Instant) -> bool {
        self.until.is_some_and(|t| now < t)
    }

    /// When the event loop must wake to hide the scrollbar; `None` once hidden.
    pub fn deadline(&self) -> Option<Instant> {
        self.until
    }

    /// Hide the scrollbar if its deadline has passed. Returns true if that changed what's on
    /// screen (repaint) and clears the deadline, so the loop stops waking for it.
    pub fn expire(&mut self, now: Instant) -> bool {
        match self.until {
            Some(t) if now >= t => {
                self.until = None;
                true
            }
            _ => false,
        }
    }
}

/// Set the unused top byte of every pixel to 0xFF just before presenting. softbuffer labels
/// frames XRGB8888, so that byte should be ignored, but some compositors read it as alpha:
/// KWin 6.8 with NVIDIA 615 (Wayland) shows a `0x00RRGGBB` frame as black or see-through.
/// Four pixels per step, because the size-optimized build doesn't vectorize a per-pixel loop
/// (about 1 ms at 4K instead of 2).
pub fn opaque(buf: &mut [u32]) {
    const A4: u128 = 0xff00_0000_ff00_0000_ff00_0000_ff00_0000;
    // SAFETY: every bit pattern is a valid `u32` and `u128`, and `align_to_mut` keeps the
    // middle slice aligned.
    let (head, mid, tail) = unsafe { buf.align_to_mut::<u128>() };
    for p in mid {
        *p |= A4;
    }
    for p in head.iter_mut().chain(tail) {
        *p |= 0xff00_0000;
    }
}

#[cfg(test)]
mod tests;
