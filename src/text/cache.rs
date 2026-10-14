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
    /// RGBA (premultiplied as produced by swash) instead of an 8-bit alpha mask.
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
        let mut render = Render::new(&sources);
        render
            .format(Format::Alpha)
            .offset(Vector::new(key.subpx as f32 / SUBPX, 0.0));
        if key.synth & synth::BOLD != 0 {
            render.embolden(size / 40.0);
        }
        if key.synth & synth::ITALIC != 0 {
            render.transform(Some(Transform::skew(
                Angle::from_degrees(12.0),
                Angle::ZERO,
            )));
        }
        let img = render.render(&mut scaler, key.id)?;
        let p = img.placement;
        if p.width == 0 || p.height == 0 {
            return None;
        }
        Some(GlyphImage {
            left: p.left,
            top: p.top,
            width: p.width,
            height: p.height,
            color: matches!(img.content, Content::Color),
            data: img.data,
        })
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
