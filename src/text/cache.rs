//! Glyph image cache: swash-rendered alpha masks (tinted at blit time, so both
//! themes share entries) and RGBA images for color emoji, under an LRU byte cap.

use std::collections::HashMap;

use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};

use crate::fonts::{FaceId, Fonts, synth};

/// Default cap on cached image bytes.
pub const DEFAULT_CAP: usize = 4 << 20;
/// Horizontal subpixel positions per pixel.
const SUBPX: f32 = 4.0;
/// Bookkeeping cost charged per entry on top of its pixel data.
const ENTRY_OVERHEAD: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub face: FaceId,
    pub id: u16,
    /// Size in quarter pixels.
    pub size_q: u32,
    /// Subpixel x offset in quarters of a pixel.
    pub subpx: u8,
    pub synth: u8,
}

impl GlyphKey {
    /// Key for a glyph whose origin is at `x`, plus the whole-pixel x to draw it at.
    pub fn at(face: FaceId, id: u16, size: f32, x: f32, synth: u8) -> (GlyphKey, i32) {
        let xi = x.floor();
        let subpx = (((x - xi) * SUBPX) as u8).min(SUBPX as u8 - 1);
        let size_q = (size * 4.0).round().max(1.0) as u32;
        (
            GlyphKey {
                face,
                id,
                size_q,
                subpx,
                synth,
            },
            xi as i32,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphImage {
    /// Offset from the glyph origin to the image's left edge.
    pub left: i32,
    /// Offset from the baseline up to the image's top edge.
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA instead of an 8-bit alpha mask. COLR output is premultiplied by
    /// swash; bitmap color glyphs (CBDT/sbix) are premultiplied by the cache.
    pub color: bool,
    pub data: Vec<u8>,
}

struct Entry {
    /// `None` for glyphs with no pixels (spaces).
    img: Option<GlyphImage>,
    used: u64,
}

pub struct GlyphCache {
    scale: ScaleContext,
    map: HashMap<GlyphKey, Entry>,
    bytes: usize,
    cap: usize,
    tick: u64,
}

impl Default for GlyphCache {
    fn default() -> Self {
        GlyphCache::with_cap(DEFAULT_CAP)
    }
}

impl GlyphCache {
    pub fn with_cap(cap: usize) -> Self {
        GlyphCache {
            scale: ScaleContext::new(),
            map: HashMap::new(),
            bytes: 0,
            cap,
            tick: 0,
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&mut self, fonts: &Fonts, key: GlyphKey) -> Option<&GlyphImage> {
        self.tick += 1;
        if !self.map.contains_key(&key) {
            let img = self.render(fonts, key);
            let cost = ENTRY_OVERHEAD + img.as_ref().map_or(0, |i| i.data.len());
            if self.bytes + cost > self.cap {
                self.evict(cost);
            }
            self.bytes += cost;
            self.map.insert(
                key,
                Entry {
                    img,
                    used: self.tick,
                },
            );
        }
        let entry = self.map.get_mut(&key)?;
        entry.used = self.tick;
        entry.img.as_ref()
    }

    /// Drop least-recently-used entries until `incoming` more bytes fit under 3/4 of the cap,
    /// so eviction (O(n log n)) runs rarely.
    fn evict(&mut self, incoming: usize) {
        let target = (self.cap * 3 / 4).saturating_sub(incoming);
        let mut by_age: Vec<(u64, GlyphKey)> = self.map.iter().map(|(k, e)| (e.used, *k)).collect();
        by_age.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in by_age {
            if self.bytes <= target {
                break;
            }
            if let Some(e) = self.map.remove(&key) {
                self.bytes -= ENTRY_OVERHEAD + e.img.map_or(0, |i| i.data.len());
            }
        }
    }

    fn render(&mut self, fonts: &Fonts, key: GlyphKey) -> Option<GlyphImage> {
        let size = key.size_q as f32 / 4.0;
        let font = fonts.face(key.face).font();
        let mut scaler = self.scale.builder(font).size(size).hint(false).build();
        let sources = [
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ];
        // TODO(P14): COLR layers that use palette index 0xFFFF ("current text color") come out
        // swash's default gray, since no foreground is set here. Linux's Noto Color Emoji is
        // CBDT, so only COLRv0 fonts (Segoe UI Emoji, Twemoji Mozilla) are affected. The fix:
        // key color glyphs by the text color (not mask glyphs, so their key stays small) and
        // pass it to `Render::default_color`.
        let mut render = Render::new(&sources);
        render
            .format(Format::Alpha)
            .offset(Vector::new(key.subpx as f32 / SUBPX, 0.0));
        if key.synth & synth::ITALIC != 0 {
            render.transform(Some(Transform::skew(
                Angle::from_degrees(12.0),
                Angle::ZERO,
            )));
        }
        let mut img = render.render(&mut scaler, key.id)?;
        let p = img.placement;
        if p.width == 0 || p.height == 0 {
            return None;
        }
        let color = matches!(img.content, Content::Color);
        // Color outlines come out premultiplied; embedded bitmaps (CBDT/sbix PNGs) don't.
        if color && matches!(img.source, Source::ColorBitmap(_)) {
            premultiply(&mut img.data);
        }
        let mut out = GlyphImage {
            left: p.left,
            top: p.top,
            width: p.width,
            height: p.height,
            color,
            data: img.data,
        };
        // Synthetic bold thickens the rendered mask, not the outline: swash's outline embolden
        // thins glyphs whose contours run the other way (some CJK glyphs in Noto Sans CJK lose
        // most of their ink). Same strength as before: size/20 wider, size/40 taller.
        if key.synth & synth::BOLD != 0 && !color {
            embolden(&mut out, size / 20.0, size / 40.0);
        }
        Some(out)
    }
}

/// Dilate a coverage mask (a max filter) `ex` pixels to the right and `ey` pixels up, the
/// fractional part at partial strength, as FreeType emboldens bitmaps.
fn embolden(img: &mut GlyphImage, ex: f32, ey: f32) {
    let (w, h) = (img.width as usize, img.height as usize);
    let (dx, dy) = (ex.ceil() as usize, ey.ceil() as usize);
    let (nw, nh) = (w + dx, h + dy);
    // One-dimensional dilation of `len` samples read through `get` into `put`, over `n` whole
    // pixels plus a fraction `f` of the next.
    let dilate = |len: usize,
                  out_len: usize,
                  e: f32,
                  get: &dyn Fn(usize) -> u8,
                  put: &mut dyn FnMut(usize, u8)| {
        let n = e.floor() as usize;
        let f = e - n as f32;
        for o in 0..out_len {
            let mut v = 0u8;
            for k in 0..=n {
                if let Some(i) = o.checked_sub(k).filter(|&i| i < len) {
                    v = v.max(get(i));
                }
            }
            if let Some(i) = o.checked_sub(n + 1).filter(|&i| i < len) {
                v = v.max((get(i) as f32 * f) as u8);
            }
            put(o, v);
        }
    };
    let mut wide = vec![0u8; nw * h];
    for y in 0..h {
        let row = &img.data[y * w..(y + 1) * w];
        let out = &mut wide[y * nw..(y + 1) * nw];
        dilate(w, nw, ex, &|i| row[i], &mut |o, v| out[o] = v);
    }
    // Upward: output row r reads source rows below it, so rows are taken bottom up.
    let mut tall = vec![0u8; nw * nh];
    for x in 0..nw {
        dilate(h, nh, ey, &|i| wide[(h - 1 - i) * nw + x], &mut |o, v| {
            tall[(nh - 1 - o) * nw + x] = v
        });
    }
    img.width = nw as u32;
    img.height = nh as u32;
    img.top += dy as i32;
    img.data = tall;
}

/// Straight RGBA → premultiplied RGBA, in place.
fn premultiply(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests {
    use super::*;

    fn key(fonts: &Fonts, ch: char, size: f32, synth: u8) -> GlyphKey {
        let face = fonts.primary(false, false, true);
        let id = fonts.face(face).glyph(ch);
        GlyphKey::at(face, id, size, 0.0, synth).0
    }

    #[test]
    fn renders_and_caches() {
        let fonts = Fonts::new().unwrap();
        let mut cache = GlyphCache::default();
        let k = key(&fonts, 'A', 16.0, 0);
        let img = cache.get(&fonts, k).cloned().expect("A has pixels");
        assert!(!img.color && img.width > 0 && img.data.len() == (img.width * img.height) as usize);
        assert_eq!(cache.get(&fonts, k), Some(&img));
        assert_eq!(cache.len(), 1);
        // Space has no pixels but is cached as empty.
        assert!(cache.get(&fonts, key(&fonts, ' ', 16.0, 0)).is_none());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn subpixel_keys() {
        let (k0, x0) = GlyphKey::at(FaceId(0), 1, 16.0, 10.0, 0);
        let (k1, x1) = GlyphKey::at(FaceId(0), 1, 16.0, 10.6, 0);
        let (k2, x2) = GlyphKey::at(FaceId(0), 1, 16.0, -0.3, 0);
        assert_eq!((x0, x1, x2), (10, 10, -1));
        assert_eq!((k0.subpx, k1.subpx, k2.subpx), (0, 2, 2));
    }

    #[test]
    fn eviction_respects_cap() {
        let fonts = Fonts::new().unwrap();
        let mut cache = GlyphCache::with_cap(20_000);
        for (i, ch) in ('A'..='Z').chain('a'..='z').enumerate() {
            cache.get(&fonts, key(&fonts, ch, 24.0 + i as f32, 0));
            assert!(cache.bytes() <= 20_000, "{} bytes", cache.bytes());
        }
        // The most recent glyph survives eviction.
        assert!(
            cache
                .get(&fonts, key(&fonts, 'z', 24.0 + 51.0, 0))
                .is_some()
        );
    }

    #[test]
    fn premultiplies() {
        let mut px = [200, 100, 50, 128, 10, 20, 30, 255, 99, 99, 99, 0];
        premultiply(&mut px);
        assert_eq!(px, [100, 50, 25, 128, 10, 20, 30, 255, 0, 0, 0, 0]);
    }

    /// Synthetic bold adds ink to every glyph and grows the mask right and up.
    #[test]
    fn synthetic_bold_thickens() {
        let fonts = Fonts::new().unwrap();
        let mut cache = GlyphCache::default();
        for ch in ['A', 'g', '|', 'o'] {
            let plain = cache
                .get(&fonts, key(&fonts, ch, 32.0, 0))
                .cloned()
                .unwrap();
            let bold = cache
                .get(&fonts, key(&fonts, ch, 32.0, synth::BOLD))
                .cloned()
                .unwrap();
            let ink = |i: &GlyphImage| i.data.iter().map(|&a| a as u64).sum::<u64>();
            assert!(ink(&bold) > ink(&plain) * 11 / 10, "{ch}");
            assert_eq!(
                (bold.width, bold.height),
                (plain.width + 2, plain.height + 1)
            );
            assert_eq!((bold.left, bold.top), (plain.left, plain.top + 1));
        }
    }

    #[test]
    fn embolden_dilates_right_and_up() {
        let mut img = GlyphImage {
            left: 0,
            top: 2,
            width: 2,
            height: 2,
            color: false,
            data: vec![0, 0, 200, 0],
        };
        embolden(&mut img, 1.5, 1.0);
        assert_eq!((img.width, img.height, img.top), (4, 3, 3));
        #[rustfmt::skip]
        assert_eq!(img.data, vec![
            0, 0, 0, 0,
            200, 200, 100, 0,
            200, 200, 100, 0,
        ]);
    }

    /// Synthetic italic must lean right: ink in the top rows sits further right than ink in
    /// the bottom rows. Uses `|`, which is perfectly vertical upright (`l` has a tail).
    #[test]
    fn synthetic_italic_leans_right() {
        let fonts = Fonts::new().unwrap();
        let mut cache = GlyphCache::default();
        let img = cache
            .get(&fonts, key(&fonts, '|', 48.0, synth::ITALIC))
            .cloned()
            .unwrap();
        let centroid = |rows: std::ops::Range<u32>| {
            let (mut sum, mut n) = (0.0, 0.0);
            for y in rows {
                for x in 0..img.width {
                    let a = img.data[(y * img.width + x) as usize] as f32;
                    sum += a * x as f32;
                    n += a;
                }
            }
            sum / n.max(1.0)
        };
        let h = img.height;
        assert!(centroid(0..h / 4) > centroid(h * 3 / 4..h) + 1.0);
    }
}

/// A CBDT/sbix-style bitmap (straight alpha) takes the cache's conversion and then the
/// painter's premultiplied blit; the result must match plain straight-alpha compositing.
#[cfg(test)]
mod color_bitmap_tests {
    use super::*;
    use crate::render::Frame;

    #[test]
    fn straight_alpha_bitmap_composites_like_source_over() {
        const BG: u32 = 0x00_20_80_e0;
        let straight: [[u8; 4]; 4] = [
            [255, 0, 0, 255],
            [255, 0, 0, 128],
            [10, 200, 90, 51],
            [200, 200, 200, 0],
        ];
        let mut data: Vec<u8> = straight.concat();
        premultiply(&mut data);
        let mut px = [BG; 4];
        Frame::new(&mut px, 4, 1, 4).blit_rgba(0, 0, 4, 1, &data);
        for (i, s) in straight.iter().enumerate() {
            let a = u32::from(s[3]);
            let bg = [(BG >> 16) & 255, (BG >> 8) & 255, BG & 255];
            let want: Vec<u32> = (0..3)
                .map(|c| (u32::from(s[c]) * a + bg[c] * (255 - a) + 127) / 255)
                .collect();
            let got = [(px[i] >> 16) & 255, (px[i] >> 8) & 255, px[i] & 255];
            for c in 0..3 {
                assert!(
                    got[c].abs_diff(want[c]) <= 1,
                    "pixel {i} channel {c}: got {got:?}, want {want:?}"
                );
            }
        }
        // Opaque stays exact, fully transparent leaves the background alone.
        assert_eq!(px[0], 0x00_ff_00_00);
        assert_eq!(px[3], BG);
    }
}
