//! Painter: laid-out blocks → pixels in a `0x00RRGGBB` buffer.
//!
//! Everything here is plain CPU code over a `&mut [u32]`, so it runs headless (tests, goldens)
//! exactly as it does into a softbuffer surface. Blending is plain sRGB alpha, no gamma.
//! Rectangles snap to whole pixels so rules and borders stay crisp; only rounded corners are
//! anti-aliased. Glyph x positions keep quarter-pixel precision through the glyph cache.

use std::time::{Duration, Instant};

use crate::fonts::Fonts;
use crate::layout::{BlockBox, Layout};
use crate::text::RectKind;
use crate::text::cache::GlyphCache;
use crate::theme::{Rgb, Role, Theme};

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
            for (d, p) in dst.iter_mut().zip(src.chunks_exact(4)) {
                *d = over_premul(*d, p[0], p[1], p[2], p[3]);
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
    pub fn height(&self) -> f32 {
        (self.area.y1 - self.area.y0).max(0) as f32
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
        for i in layout.range_exact(vp.scroll, vp.scroll + f64::from(vp.height())) {
            let Some(content) = layout.geoms()[i].content.as_deref() else {
                continue;
            };
            let oy = (vp.area.y0 as f64 + layout.top(i) - vp.scroll) as f32;
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

/// Paint one block's content with its origin at (`ox`, `oy`) in frame pixels.
fn paint_block(
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
        let reach = style.size as i32 * 2 + 2;
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
        let (gx, gy) = (xi + img.left, baseline - img.top);
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
    // Inline boxes have no pixels of their own yet (images land in P10): outline the space.
    for b in &content.boxes {
        let (x0, x1) = span(ox + b.x, b.b.width);
        let (y0, y1) = span(oy + b.y, b.b.ascent + b.b.descent);
        frame.rounded_rect(
            (x0, y0, x1, y1),
            0.0,
            Some(1.0),
            theme.color(Role::Border),
            255,
        );
    }
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
    let view = vp.height() as f64;
    let total = total as f64;
    if view <= 0.0 || total <= view {
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
        .min(track);
    let max_scroll = total - view;
    let pos = (vp.scroll / max_scroll).clamp(0.0, 1.0) * (track - len);
    let w = (THUMB_W as f64 * scale).round().max(1.0) as i32;
    let x1 = vp.area.x1 - margin as i32;
    let y0 = vp.area.y0 + (margin + pos).round() as i32;
    Some(Thumb {
        x0: x1 - w,
        y0,
        x1,
        y1: y0 + len.round().max(1.0) as i32,
    })
}

/// Paint the overlay thumb over the document area (after [`paint`]).
pub fn paint_scrollbar(frame: &mut Frame, vp: Viewport, total: f32, scale: f32, theme: &Theme) {
    let Some(t) = thumb(vp, total, scale) else {
        return;
    };
    let prev = frame.set_clip(vp.area);
    let r = (t.x1 - t.x0) as f32 / 2.0;
    frame.rounded_rect((t.x0, t.y0, t.x1, t.y1), r, None, theme.muted, THUMB_ALPHA);
    frame.set_clip(prev);
}

/// Scrollbar visibility: shown on scroll, hidden [`SCROLLBAR_LINGER`] after the last one. A
/// single deadline, so an idle window wakes once to hide it and then sleeps.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollbarFade {
    until: Option<Instant>,
}

impl ScrollbarFade {
    /// Show the scrollbar (on scroll) and push the hide deadline back.
    pub fn poke(&mut self, now: Instant) {
        self.until = Some(now + SCROLLBAR_LINGER);
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

#[cfg(test)]
mod tests;
