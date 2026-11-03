//! LaTeX math: `$…$`, `$$…$$`, `` $`…`$ `` and ```` ```math ```` blocks.
//!
//! The document keeps each formula as its source (a span with [`crate::doc::style::MATH`]);
//! nothing is parsed when the document is. VOMD lays formulas out itself, on the OpenType MATH
//! table of a math font ([`table`]): STIX Two Math, embedded with `embed-fonts`, else an
//! installed one ([`crate::fonts::Fonts::load_math`]).
//!
//! A formula goes source → [`parse`] (a syntax tree, with the names of [`symbols`]) →
//! [`layout`] (a [`layout::Formula`]: glyphs and rules in em) → [`emit`] (glyphs and rectangles
//! in pixels, which the painter draws like text). Every step is bounded by the limits below;
//! past one, the formula is an error and is shown as its source.
//!
//! A formula is laid out once, in em, and kept in a [`Cache`] that lives with the window (it
//! outlives a document's layout: a resize, a font change or a live reload doesn't lay an
//! unchanged formula out again); the document's layout only scales it, and fits a display
//! formula too wide for its column ([`fit`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use crate::fonts::{FaceId, Fonts, Slot};

pub mod emit;
pub mod fit;
pub mod layout;
pub mod lex;
pub mod parse;
pub mod symbols;
pub mod table;

/// The longest formula source, in bytes. A longer one is shown as its source, as an error
/// (`style::MATH_ERROR`): it is under half of [`crate::doc::MAX_BLOCK_BYTES`], so a block is
/// always cut before or after a formula, never inside one.
pub const MAX_SOURCE: usize = 8 * 1024;
/// How deep groups, arguments, `\left…\right`, environments and scripts may nest.
pub const MAX_DEPTH: usize = 48;
/// Nodes of a formula's syntax tree.
pub const MAX_NODES: usize = 8192;
/// Glyphs and rules a formula lays out to.
pub const MAX_GLYPHS: usize = 16_384;
/// Cells of an environment, and columns.
pub const MAX_CELLS: usize = 4096;
pub const MAX_COLS: usize = 256;
/// Extenders in one stretched delimiter or accent: past it, the delimiter stops growing.
pub const MAX_PARTS: usize = 64;
/// Explicit dimensions (`\kern`, `\rule`, `\hspace`) are clamped to this many em either way.
pub const MAX_DIMEN: f32 = 100.0;
/// An explicit fraction rule (`\above 1pt`, `\genfrac`) is at most this many em thick.
pub const MAX_RULE: f32 = 2.0;
/// The widest and tallest a formula may be, in em.
pub const MAX_EXTENT: f32 = 1000.0;
/// Formulas kept laid out, at most.
pub const CACHE_CAP: usize = 8192;
/// Glyphs, rules and pieces kept laid out, at most (about 32 bytes each, so 8 MB): a thousand
/// typical formulas are some 30,000, one at [`MAX_GLYPHS`] 16,384.
pub const CACHE_WEIGHT: usize = 1 << 18;

/// A formula's size over the text around it: STIX Two's x-height (0.473 em) set to Inter's
/// (0.546 em), as MathJax matches x-heights on GitHub.
pub const MATH_SCALE: f32 = 1.15;

/// A laid-out formula, or why it couldn't be.
pub type Laid = Result<layout::Formula, parse::Error>;

/// The cache, shared by the window and the document's layout (which a reload replaces).
pub type Shared = Rc<RefCell<Cache>>;

/// Formulas laid out so far, by source and mode, with the engine that lays them out. A
/// formula is laid out again when what it was set with changed: the math face, the body
/// faces (`\mathsf`, `\texttt` and characters the math face lacks come from them), or, for
/// one with characters no face had, the faces there are (fallbacks were added). At most
/// [`CACHE_CAP`] are kept, of [`CACHE_WEIGHT`] glyphs in all; past that the least recently
/// used go first (a clock), but never the one just laid out.
#[derive(Default)]
pub struct Cache {
    engine: layout::Engine,
    map: HashMap<(u64, bool), Entry>,
    /// The keys in the order they came, for the clock's hand to go round.
    ring: Vec<(u64, bool)>,
    hand: usize,
    /// The weight of the formulas kept (see [`weight`]).
    weight: usize,
    /// Formulas laid out (not found in the cache), for tests.
    pub laid: usize,
}

struct Entry {
    src: Box<str>,
    laid: Arc<Laid>,
    with: With,
    used: bool,
    weight: usize,
}

/// What a laid-out formula weighs in the cache: its glyphs, rules and pieces, its tag's, and
/// one for the entry.
fn weight(laid: &Laid) -> usize {
    let f = |f: &layout::Formula| f.glyphs.len() + f.rules.len() + f.pieces.len();
    laid.as_ref()
        .map_or(0, |l| f(l) + l.tag.as_deref().map_or(0, f))
        + 1
}

/// What a formula was laid out with.
#[derive(Clone, Copy, PartialEq, Eq)]
struct With {
    math: FaceId,
    regular: FaceId,
    mono: FaceId,
    /// The number of faces, if a character had none.
    faces: Option<usize>,
}

impl Cache {
    pub fn shared() -> Shared {
        Rc::new(RefCell::new(Cache::default()))
    }

    /// `src` laid out (`display` for display math), from the cache if it can be. `None` if
    /// there is no math face (see [`Fonts::load_math`]): formulas are then shown as source.
    pub fn get(&mut self, fonts: &Fonts, src: &str, display: bool) -> Option<Arc<Laid>> {
        let math = fonts.math()?;
        let mut h = DefaultHasher::new();
        src.hash(&mut h);
        let key = (h.finish(), display);
        let with = |missing: bool| With {
            math,
            regular: fonts.slot(Slot::Regular),
            mono: fonts.slot(Slot::Mono),
            faces: missing.then(|| fonts.faces_len()),
        };
        if let Some(e) = self.map.get_mut(&key)
            && *e.src == *src
            && e.with == with(e.with.faces.is_some())
        {
            e.used = true;
            return Some(e.laid.clone());
        }
        self.laid += 1;
        let laid = Arc::new(self.engine.layout(fonts, math, src, display));
        let missing = laid.as_ref().as_ref().is_ok_and(|f| !f.missing.is_empty());
        let entry = Entry {
            src: src.into(),
            laid: laid.clone(),
            with: with(missing),
            used: false,
            weight: weight(&laid),
        };
        self.weight += entry.weight;
        match self.map.insert(key, entry) {
            Some(old) => self.weight = self.weight.saturating_sub(old.weight),
            None => {
                if self.ring.len() >= CACHE_CAP {
                    self.evict(key);
                }
                self.ring.push(key);
            }
        }
        // Each eviction drops one: at most every formula but this one.
        for _ in 0..self.ring.len() {
            if self.weight <= CACHE_WEIGHT || !self.evict(key) {
                break;
            }
        }
        Some(laid)
    }

    /// Drops one formula other than `keep` not used since the hand last passed it: the hand
    /// goes round at most twice (the second time every mark is clear). False if there was
    /// none to drop.
    fn evict(&mut self, keep: (u64, bool)) -> bool {
        for _ in 0..2 * self.ring.len() {
            self.hand %= self.ring.len().max(1);
            let Some(&key) = self.ring.get(self.hand) else {
                return false;
            };
            match self.map.get_mut(&key) {
                Some(e) if e.used || key == keep => {
                    e.used = false;
                    self.hand += 1;
                }
                _ => {
                    if let Some(e) = self.map.remove(&key) {
                        self.weight = self.weight.saturating_sub(e.weight);
                    }
                    self.ring.swap_remove(self.hand);
                    return true;
                }
            }
        }
        false
    }

    /// The glyphs, rules and pieces of the formulas kept.
    pub fn weight(&self) -> usize {
        self.weight
    }

    /// Formulas kept.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests;
