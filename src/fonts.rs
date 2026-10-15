//! Font faces: the embedded set plus fallback faces added at runtime.
//!
//! Startup never scans system fonts. Embedded faces are `&'static` slices; fallback faces are
//! memory-mapped so only the pages actually used (cmap, glyphs drawn) become resident.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use swash::proxy::{CharmapProxy, MetricsProxy};
use swash::text::Codepoint;
use swash::{CacheKey, FontRef, Metrics};

/// Index into [`Fonts`]. Stable for the life of the process (faces are only ever added).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u16);

/// Synthesized style applied at rasterization when a face lacks the requested style.
pub mod synth {
    pub const BOLD: u8 = 1 << 0;
    pub const ITALIC: u8 = 1 << 1;
}

enum Data {
    #[cfg_attr(not(feature = "embed-fonts"), allow(dead_code))]
    Static(&'static [u8]),
    Mapped(Arc<memmap2::Mmap>),
}

pub struct Face {
    data: Data,
    offset: u32,
    key: CacheKey,
    charmap: CharmapProxy,
    metrics: Metrics,
    pub bold: bool,
    pub italic: bool,
    /// Source file and collection index for faces loaded from disk.
    pub source: Option<(PathBuf, u32)>,
}

impl Face {
    fn new(data: Data, index: u32, source: Option<(PathBuf, u32)>) -> Option<Face> {
        let bytes: &[u8] = match &data {
            Data::Static(s) => s,
            Data::Mapped(m) => m,
        };
        let font = FontRef::from_index(bytes, index as usize)?;
        let attrs = font.attributes();
        let charmap = CharmapProxy::from_font(&font);
        let metrics = MetricsProxy::from_font(&font).materialize_metrics(&font, &[]);
        Some(Face {
            offset: font.offset,
            key: font.key,
            charmap,
            metrics,
            bold: attrs.weight().0 >= 600,
            italic: attrs.style() != swash::Style::Normal,
            data,
            source,
        })
    }

    pub fn font(&self) -> FontRef<'_> {
        let data: &[u8] = match &self.data {
            Data::Static(s) => s,
            Data::Mapped(m) => m,
        };
        FontRef {
            data,
            offset: self.offset,
            key: self.key,
        }
    }

    /// Glyph for `ch`, or 0 (`.notdef`) if the face doesn't cover it.
    pub fn glyph(&self, ch: char) -> u16 {
        self.charmap.materialize(&self.font()).map(ch)
    }

    /// Whether the face can draw `base` followed by combining `marks`: each mark has a glyph or
    /// composes into the base (shaping turns `e` + U+0301 into `é`, so the face needs no glyph
    /// for the mark itself).
    pub fn covers_sequence(&self, base: char, marks: &[char]) -> bool {
        let mut cluster = base;
        for &mark in marks {
            match char::compose(cluster, mark) {
                Some(composed) if self.glyph(composed) != 0 => cluster = composed,
                _ if self.glyph(mark) != 0 => {}
                _ => return false,
            }
        }
        self.glyph(cluster) != 0
    }

    /// Design-unit metrics; scale with `metrics.scale(px)`.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }
}

/// The six faces body text is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Regular = 0,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    MonoBold,
}

pub struct Fonts {
    faces: Vec<Face>,
    slots: [FaceId; 6],
    /// Fallback faces in the order they were added.
    fallbacks: Vec<FaceId>,
}

#[cfg(feature = "embed-fonts")]
static EMBEDDED: [(Slot, &[u8]); 6] = [
    (
        Slot::Regular,
        include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    ),
    (Slot::Bold, include_bytes!("../assets/fonts/Inter-Bold.ttf")),
    (
        Slot::Italic,
        include_bytes!("../assets/fonts/Inter-Italic.ttf"),
    ),
    (
        Slot::BoldItalic,
        include_bytes!("../assets/fonts/Inter-BoldItalic.ttf"),
    ),
    (
        Slot::Mono,
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    ),
    (
        Slot::MonoBold,
        include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    ),
];

impl Fonts {
    /// The embedded faces. Infallible: the data is compiled in.
    #[cfg(feature = "embed-fonts")]
    pub fn new() -> Result<Fonts, String> {
        let faces: Vec<Face> = EMBEDDED
            .iter()
            .map(|(_, data)| Face::new(Data::Static(data), 0, None).expect("embedded font"))
            .collect();
        let mut slots = [FaceId(0); 6];
        for (i, (slot, _)) in EMBEDDED.iter().enumerate() {
            slots[*slot as usize] = FaceId(i as u16);
        }
        Ok(Fonts {
            faces,
            slots,
            fallbacks: Vec::new(),
        })
    }

    /// Without embedded fonts, load well-known system faces by file name.
    #[cfg(not(feature = "embed-fonts"))]
    pub fn new() -> Result<Fonts, String> {
        use crate::text::fallback::find_file;
        const NAMES: [&[&str]; 6] = [
            &[
                "NotoSans-Regular.ttf",
                "DejaVuSans.ttf",
                "LiberationSans-Regular.ttf",
                "segoeui.ttf",
                "Arial.ttf",
            ],
            &[
                "NotoSans-Bold.ttf",
                "DejaVuSans-Bold.ttf",
                "LiberationSans-Bold.ttf",
                "segoeuib.ttf",
                "Arial Bold.ttf",
            ],
            &[
                "NotoSans-Italic.ttf",
                "DejaVuSans-Oblique.ttf",
                "LiberationSans-Italic.ttf",
                "segoeuii.ttf",
            ],
            &[
                "NotoSans-BoldItalic.ttf",
                "DejaVuSans-BoldOblique.ttf",
                "LiberationSans-BoldItalic.ttf",
                "segoeuiz.ttf",
            ],
            &[
                "NotoSansMono-Regular.ttf",
                "DejaVuSansMono.ttf",
                "LiberationMono-Regular.ttf",
                "consola.ttf",
                "Menlo.ttc",
            ],
            &[
                "NotoSansMono-Bold.ttf",
                "DejaVuSansMono-Bold.ttf",
                "LiberationMono-Bold.ttf",
                "consolab.ttf",
            ],
        ];
        let mut fonts = Fonts {
            faces: Vec::new(),
            slots: [FaceId(0); 6],
            fallbacks: Vec::new(),
        };
        let mut loaded: [Option<FaceId>; 6] = [None; 6];
        for (i, names) in NAMES.iter().enumerate() {
            if let Some(path) = find_file(names) {
                loaded[i] = fonts.load(&path, 0);
            }
        }
        let regular = loaded[0]
            .ok_or("no usable system font found (tried Noto Sans, DejaVu Sans, Liberation Sans)")?;
        // Missing styles fall back to the nearest loaded face and get synthesized.
        let mono = loaded[4].unwrap_or(regular);
        fonts.slots = [
            regular,
            loaded[1].unwrap_or(regular),
            loaded[2].unwrap_or(regular),
            loaded[3].or(loaded[1]).unwrap_or(regular),
            mono,
            loaded[5].unwrap_or(mono),
        ];
        fonts.fallbacks.clear();
        Ok(fonts)
    }

    pub fn face(&self, id: FaceId) -> &Face {
        &self.faces[id.0 as usize]
    }

    pub fn slot(&self, slot: Slot) -> FaceId {
        self.slots[slot as usize]
    }

    /// Primary face for a text style.
    pub fn primary(&self, bold: bool, italic: bool, mono: bool) -> FaceId {
        self.slot(match (mono, bold, italic) {
            (true, true, _) => Slot::MonoBold,
            (true, false, _) => Slot::Mono,
            (false, true, true) => Slot::BoldItalic,
            (false, true, false) => Slot::Bold,
            (false, false, true) => Slot::Italic,
            (false, false, false) => Slot::Regular,
        })
    }

    /// Styles the face lacks and the rasterizer must synthesize.
    pub fn synth_for(&self, id: FaceId, bold: bool, italic: bool) -> u8 {
        let face = self.face(id);
        let mut s = 0;
        if bold && !face.bold {
            s |= synth::BOLD;
        }
        if italic && !face.italic {
            s |= synth::ITALIC;
        }
        s
    }

    /// First face covering `ch`: the primary, then fallbacks. `None` if nothing covers it.
    pub fn select(&self, primary: FaceId, ch: char) -> Option<FaceId> {
        if self.face(primary).glyph(ch) != 0 {
            return Some(primary);
        }
        self.fallbacks
            .iter()
            .copied()
            .find(|&id| self.face(id).glyph(ch) != 0)
    }

    /// First face drawing `base` with all of `marks`: the primary, then fallbacks. `loose`
    /// marks are separated from the base by an ignorable character (CGJ, ZWSP, ...), so they
    /// never compose with it and need a glyph of their own.
    pub fn select_sequence(
        &self,
        primary: FaceId,
        base: char,
        marks: &[char],
        loose: &[char],
    ) -> Option<FaceId> {
        std::iter::once(primary)
            .chain(self.fallbacks.iter().copied())
            .find(|&id| {
                let face = self.face(id);
                face.covers_sequence(base, marks) && loose.iter().all(|&m| face.glyph(m) != 0)
            })
    }

    pub fn has_source(&self, path: &Path, index: u32) -> bool {
        self.faces.iter().any(|f| {
            f.source
                .as_ref()
                .is_some_and(|(p, i)| p == path && *i == index)
        })
    }

    /// Files and collection indexes of the faces loaded from disk, for a fallback scan running
    /// on another thread to skip.
    pub fn sources(&self) -> Vec<(PathBuf, u32)> {
        self.faces.iter().filter_map(|f| f.source.clone()).collect()
    }

    /// Memory-map a font file and add one of its faces as a fallback.
    pub fn add_fallback(&mut self, path: &Path, index: u32) -> Option<FaceId> {
        if self.has_source(path, index) {
            return None;
        }
        let id = self.load(path, index)?;
        self.fallbacks.push(id);
        Some(id)
    }

    fn load(&mut self, path: &Path, index: u32) -> Option<FaceId> {
        if self.faces.len() >= u16::MAX as usize {
            return None;
        }
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: font files are treated as read-only; a file truncated underneath us could
        // fault, which is the accepted trade-off of mapping (same as every font stack).
        let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
        let face = Face::new(
            Data::Mapped(Arc::new(map)),
            index,
            Some((path.to_owned(), index)),
        )?;
        self.faces.push(face);
        Some(FaceId(self.faces.len() as u16 - 1))
    }
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests {
    use super::*;

    #[test]
    fn embedded_slots() {
        let fonts = Fonts::new().unwrap();
        let regular = fonts.primary(false, false, false);
        let bold = fonts.primary(true, false, false);
        let mono = fonts.primary(false, true, true);
        assert!(!fonts.face(regular).bold && fonts.face(bold).bold);
        assert!(fonts.face(fonts.primary(false, true, false)).italic);
        assert_ne!(mono, regular);
        // Mono has no italic face: synthesize it.
        assert_eq!(fonts.synth_for(mono, false, true), synth::ITALIC);
        assert_eq!(fonts.synth_for(bold, true, false), 0);
        assert_eq!(fonts.select(regular, 'A'), Some(regular));
        assert_eq!(fonts.select(regular, '中'), None);
        assert!(fonts.face(regular).metrics().units_per_em > 0);
    }

    #[test]
    fn sequences_need_marks_or_a_composition() {
        let fonts = Fonts::new().unwrap();
        let face = fonts.face(fonts.primary(false, false, false));
        // No glyph for U+0301 in the subset, but `é` exists.
        assert_eq!(face.glyph('\u{0301}'), 0);
        assert!(face.covers_sequence('e', &['\u{0301}']));
        assert!(!face.covers_sequence('q', &['\u{0301}']));
        assert!(!face.covers_sequence('中', &[]));
        assert!(face.covers_sequence('q', &[]));
    }
}
