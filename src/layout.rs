//! Block layout: Doc IR + width + scale → positioned primitives.
//!
//! Every block has a cached height `h`, and its top `y` comes from prefix sums over the heights
//! (see `heights.rs`), so measuring a block never moves the whole tail. Heights start as
//! estimates and become exact when a block is measured: blocks near the viewport first
//! ([`Layout::ensure`], the first-frame path), the rest in time-boxed idle slices
//! ([`Layout::idle`]). Shaped content is kept only
//! near the viewport, so memory stays flat on huge documents. Positions are relative to the
//! document area: x from its left edge, y from the document top.

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use crate::doc::{self, Alert, Block, BlockKind, Doc, ImageRef, Marker, Span, style};
use crate::fonts::Fonts;
use crate::image::MAX_SIDE;
use crate::text::{
    Align, InlineBox, PBox, PGlyph, PHit, PLine, PRect, Para, ParaLayout, RectKind, TextCtx,
    TextStyle, deco,
};
use crate::theme::Role;

mod heights;
#[cfg(feature = "clipboard")]
mod locate;

use heights::Heights;

/// Average advance per character used to size the content column, in em. 80 characters at
/// 16 px ≈ 720 px.
const CHAR_EM: f32 = 0.5625;
const BODY_LH: f32 = 1.5;
const HEADING_LH: f32 = 1.25;
const CODE_LH: f32 = 1.45;
const CODE_SIZE: f32 = 0.85;
const HEADING_SIZES: [f32; 6] = [2.0, 1.5, 1.25, 1.0, 0.875, 0.85];
const QUOTE_INDENT: f32 = 1.25;
const QUOTE_BAR: f32 = 0.25;
const LIST_INDENT: f32 = 2.0;
const CELL_PAD_X: f32 = 0.8125;
const CELL_PAD_Y: f32 = 0.375;
const FOOTNOTE_SIZE: f32 = 0.875;
/// Width and height of an alert icon, in em.
const ALERT_ICON: f32 = 1.0;
/// Shaped content is kept within this many viewport heights of the viewport.
const KEEP_SCREENS: f32 = 4.0;
/// Blocks within this many viewport heights are measured exactly by `ensure`.
const MEASURE_SCREENS: f32 = 2.0;
/// Passes `ensure` makes over the blocks around the viewport (heights shift as they are measured).
const MEASURE_PASSES: usize = 4;
/// Line width for text that must never wrap (list markers).
const NO_WRAP: f32 = 1.0e6;
/// A table's column widths come from its first rows only: at most this many blocks (row
/// fragments) or this many bytes of cell text, whichever runs out first, so a huge table can't
/// block the first frame. Later rows wrap within the sampled widths.
const TABLE_SAMPLE_ROWS: usize = 256;
const TABLE_SAMPLE_BYTES: usize = 64 * 1024;
/// Width of the box a remote image loads in while its size is unknown (4:3), in em.
const LOADING_EM: f32 = 16.0;
/// Most pixels an image box may have (about two 4K screens): a tall, narrow image is shown
/// smaller rather than decoded into hundreds of MB.
const MAX_BOX_PIXELS: f32 = 16.0e6;

/// How an image takes part in layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSize {
    /// Its alt text in an outlined placeholder: nothing to show (a remote image while remote
    /// images are off, a missing file, not PNG or JPEG).
    Alt,
    /// Being fetched, size unknown: a 4:3 box.
    Loading,
    /// Its natural size in pixels: a box that many logical pixels, no wider than the line.
    Pixels(u32, u32),
}

/// Finds a local image's size when its block is first measured (see `image.rs`).
pub type ImageProbe = Box<dyn FnMut(&ImageRef) -> ImageSize>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Width of the document area in physical pixels.
    pub width: f32,
    /// Physical pixels per logical pixel: HiDPI scale × zoom.
    pub scale: f32,
    /// Base font size in logical pixels (`font_size` setting).
    pub font_size: f32,
    /// Maximum characters per line (`line_width` setting).
    pub line_width: f32,
    /// Wide layout (`wide` setting): the column has no cap and spans the window, margins aside.
    pub wide: bool,
    /// Space above the first block and below the last, in em. The app pads the page like
    /// GitHub (about 2 em); 0 by default, so block 0 starts at y = 0.
    pub page_pad: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            width: 900.0,
            scale: 1.0,
            font_size: 16.0,
            line_width: 80.0,
            wide: false,
            page_pad: 0.0,
        }
    }
}

/// Call `rect(x0, x1, key)` for each run of consecutive `hits` whose clusters share a `key`
/// (`key(offset)` of the cluster's byte offset in the document's text; `None` leaves it out):
/// the run's extent from its leftmost edge to its rightmost, whichever way its clusters run.
pub fn runs<K: Copy + PartialEq>(
    hits: &[crate::text::PHit],
    key: impl Fn(u32) -> Option<K>,
    mut rect: impl FnMut(f32, f32, K),
) {
    let mut run: Option<(f32, f32, K)> = None;
    for h in hits {
        let (a, b) = (h.x0.min(h.x1), h.x0.max(h.x1));
        run = match (run, key(h.doc)) {
            (Some((x0, x1, k)), Some(now)) if k == now => Some((x0.min(a), x1.max(b), k)),
            (Some((x0, x1, k)), now) => {
                rect(x0, x1, k);
                now.map(|k| (a, b, k))
            }
            (None, now) => now.map(|k| (a, b, k)),
        };
    }
    if let Some((x0, x1, k)) = run {
        rect(x0, x1, k);
    }
}

/// A block's laid-out content, in block coordinates (x from the content column's left edge,
/// y from the block's top). Draw order: rects, then glyphs, then boxes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockBox {
    pub glyphs: Vec<PGlyph>,
    pub styles: Vec<TextStyle>,
    pub rects: Vec<PRect>,
    pub lines: Vec<PLine>,
    pub hits: Vec<PHit>,
    pub boxes: Vec<PBox>,
    /// The box (`x0, y0, x1, y1`) around the marker or alert title, which has no hits: it is
    /// selected and copied along with the block's start.
    #[cfg(feature = "clipboard")]
    pub decor: Option<[f32; 4]>,
}

impl BlockBox {
    /// A paragraph laid out on its own (text outside the document, such as a message screen),
    /// with `styles` the paragraph's style table.
    pub fn from_para(pl: ParaLayout, styles: &[TextStyle]) -> BlockBox {
        let mut out = BlockBox::default();
        out.append(pl, styles, 0.0, 0.0);
        out
    }

    /// Append a paragraph layout at (`dx`, `dy`).
    fn append(&mut self, pl: ParaLayout, para_styles: &[TextStyle], dx: f32, dy: f32) {
        let style_base = self.styles.len() as u32;
        let hit_base = self.hits.len() as u32;
        self.styles.extend_from_slice(para_styles);
        self.glyphs.extend(pl.glyphs.into_iter().map(|g| PGlyph {
            x: g.x + dx,
            y: g.y + dy,
            style: g.style + style_base,
            ..g
        }));
        self.hits.extend(pl.hits.into_iter().map(|h| PHit {
            x0: h.x0 + dx,
            x1: h.x1 + dx,
            style: h.style + style_base,
            ..h
        }));
        self.lines.extend(pl.lines.into_iter().map(|l| PLine {
            top: l.top + dy,
            baseline: l.baseline + dy,
            bottom: l.bottom + dy,
            hits: l.hits.start + hit_base..l.hits.end + hit_base,
            ..l
        }));
        self.rects.extend(pl.rects.into_iter().map(|r| PRect {
            x: r.x + dx,
            y: r.y + dy,
            ..r
        }));
        self.boxes.extend(pl.boxes.into_iter().map(|b| PBox {
            x: b.x + dx,
            y: b.y + dy,
            ..b
        }));
    }

    /// Add a style for decoration text and return its index.
    fn push_style(&mut self, style: TextStyle) -> u32 {
        self.styles.push(style);
        self.styles.len() as u32 - 1
    }

    /// Append the glyphs of synthetic decoration text (markers, alert titles) at (`dx`, `dy`)
    /// with an existing style. It gets no lines or hits: it isn't document text.
    fn append_decor(&mut self, pl: &ParaLayout, style: u32, dx: f32, dy: f32) {
        self.glyphs.extend(pl.glyphs.iter().map(|g| PGlyph {
            x: g.x + dx,
            y: g.y + dy,
            style,
            ..*g
        }));
    }

    /// Note the box of the block's decoration, for selecting it.
    #[cfg(feature = "clipboard")]
    fn set_decor(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32)) {
        self.decor = Some([x0, y0, x1, y1]);
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, role: Role) {
        self.rects.push(PRect {
            x,
            y,
            w,
            h,
            role,
            radius: 0.0,
            kind: RectKind::Fill,
        });
    }
}

#[derive(Clone, Debug, Default)]
pub struct BlockGeom {
    /// Height, spacing above included. Only [`Layout`] changes it, which keeps the tops in step.
    pub h: f32,
    /// `h` is exact (otherwise an estimate).
    pub measured: bool,
    /// Laid-out content, present only near the viewport.
    pub content: Option<Box<BlockBox>>,
    /// Some character in the block had no face when it was measured.
    pub missing: bool,
}

impl BlockGeom {
    /// The height stays as the estimate; the laid-out content and what it found are void.
    fn unmeasure(&mut self) {
        self.measured = false;
        self.content = None;
        self.missing = false;
    }
}

/// Precomputed per-block context that depends on neighbors, kept small: there is one per block.
#[derive(Clone, Copy, Debug, Default)]
struct Meta {
    /// Alert whose quote this block belongs to, and that quote's depth.
    alert: Option<(Alert, u8)>,
    /// What the height estimate reads, found once so a change of the column's width costs
    /// no pass over the text or the blocks: the kind of estimate, the bytes of text, the lines
    /// of a code block (a chunk ending in a newline has no empty line after it), the spacing
    /// above and the content's left edge in quarters of an em (every value is one; the page
    /// padding above the first block is added by the estimate), and the rows of padding of a
    /// code block or table row (in `flags`).
    shape: Est,
    flags: u8,
    gap: u8,
    indent: u16,
    chars: u32,
    code_lines: u32,
    /// The estimate of the height under the layout's current geometry, so a change of it
    /// scales a height by the estimate's change without computing the old one again. A block
    /// never measured has exactly this height.
    est: f32,
}

impl Meta {
    /// Even data row of a table (striped background); all fragments of a row agree.
    const STRIPE: u8 = 1;
    const FIRST_ROW: u8 = 2;
    /// Part of the footnote section at the end of the document.
    const FOOTNOTE: u8 = 4;
    /// The rows of padding, 0 to 2, in the two bits above.
    const PADS_SHIFT: u32 = 3;

    fn stripe(self) -> bool {
        self.flags & Self::STRIPE != 0
    }

    fn first_row(self) -> bool {
        self.flags & Self::FIRST_ROW != 0
    }

    fn footnote(self) -> bool {
        self.flags & Self::FOOTNOTE != 0
    }

    fn pads(self) -> u8 {
        self.flags >> Self::PADS_SHIFT
    }

    fn set(&mut self, flag: u8, on: bool) {
        self.flags = self.flags & !flag | if on { flag } else { 0 };
    }
}

/// `em` as a count of quarters, which every spacing and indent is (so f32 holds it exactly).
fn quarters(em: f32) -> u16 {
    debug_assert_eq!(em * 4.0, (em * 4.0).round());
    (em * 4.0) as u16
}

fn from_quarters(q: impl Into<f32>) -> f32 {
    q.into() * 0.25
}

/// How a block's height is estimated.
#[derive(Clone, Copy, Debug, Default)]
enum Est {
    #[default]
    Text,
    Heading(u8),
    Code,
    Rule,
    Row,
}

/// What the height estimate depends on besides the document: the layout's `em`, the column's
/// width and the page padding, passed explicitly so the estimates before and after a change
/// of them are both computed from their own values.
#[derive(Clone, Copy, Debug)]
struct Basis {
    em: f32,
    col_w: f32,
    page_pad: f32,
}

/// A position in the document that survives relayout: a block and a fraction of its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub block: u32,
    pub frac: f32,
}

impl Anchor {
    pub const TOP: Anchor = Anchor {
        block: 0,
        frac: 0.0,
    };
}

/// Result of [`Layout::hit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub block: u32,
    /// Byte offset into `Doc::text`.
    pub doc: u32,
    /// 1-based link id; 0 = not a link.
    pub link: u32,
}

pub struct Layout {
    params: Params,
    em: f32,
    col_x: f32,
    col_w: f32,
    geoms: Vec<BlockGeom>,
    /// The heights of `geoms` with their prefix sums: where each block starts.
    heights: Heights,
    meta: Vec<Meta>,
    /// Block range of each table.
    table_rows: Vec<Range<u32>>,
    /// Each table's column widths, shared with the rows being laid out.
    table_cols: Vec<Option<Arc<[f32]>>>,
    /// Some character in a table's sampled rows had no face when its columns were measured.
    table_missing: Vec<bool>,
    /// Next block for idle measurement.
    cursor: usize,
    /// Blocks currently holding content.
    live: Vec<u32>,
    /// Characters waiting for `take_missing`.
    missing: BTreeSet<char>,
    /// Characters already returned by `take_missing`.
    tried: HashSet<char>,
    /// Set operations on `missing` and `tried` so far (for tests of the cost).
    #[cfg(test)]
    set_ops: usize,
    /// Table cells laid out so far (for tests of the cost).
    #[cfg(test)]
    cells_laid: usize,
    ctx: TextCtx,
    para: Para,
    /// Each image's size (by id − 1); `None`: not looked up yet (its block was never measured).
    images: Vec<Option<ImageSize>>,
    probe: Option<ImageProbe>,
    /// The block each image is in (by id − 1), found when first needed.
    image_blocks: Vec<u32>,
}

impl Layout {
    pub fn new(doc: &Doc, params: Params) -> Layout {
        let mut layout = Layout {
            params,
            em: 0.0,
            col_x: 0.0,
            col_w: 0.0,
            geoms: vec![BlockGeom::default(); doc.blocks.len()],
            heights: Heights::new(std::iter::empty()),
            meta: Vec::new(),
            table_rows: vec![0..0; doc.tables.len()],
            table_cols: vec![None; doc.tables.len()],
            table_missing: vec![false; doc.tables.len()],
            cursor: 0,
            live: Vec::new(),
            missing: BTreeSet::new(),
            tried: HashSet::new(),
            #[cfg(test)]
            set_ops: 0,
            #[cfg(test)]
            cells_laid: 0,
            ctx: TextCtx::default(),
            para: Para::default(),
            images: vec![None; doc.images.len()],
            probe: None,
            image_blocks: Vec::new(),
        };
        layout.meta = layout.prepass(doc);
        layout.set_geometry();
        layout.estimate_all();
        for (g, m) in layout.geoms.iter_mut().zip(&layout.meta) {
            g.h = m.est;
        }
        layout.rebuild_heights();
        layout
    }

    pub fn params(&self) -> Params {
        self.params
    }

    pub fn total_height(&self) -> f32 {
        self.heights.prefix(self.geoms.len()) as f32
    }

    /// Top of block `i` (the total height for one past the last).
    pub fn y(&self, i: usize) -> f32 {
        self.heights.prefix(i) as f32
    }

    /// [`Layout::y`] without rounding to f32, for painting: on a tall document f32 tops are
    /// whole pixels or worse, and blocks would jitter against each other.
    pub fn top(&self, i: usize) -> f64 {
        self.heights.prefix(i)
    }

    /// Left edge and width of the content column.
    pub fn column(&self) -> (f32, f32) {
        (self.col_x, self.col_w)
    }

    pub fn geoms(&self) -> &[BlockGeom] {
        &self.geoms
    }

    /// Every block has an exact height.
    pub fn complete(&self) -> bool {
        self.cursor >= self.geoms.len() && self.geoms.iter().all(|g| g.measured)
    }

    /// Characters no loaded face covered, accumulated since the last call. Each character is
    /// returned once, so callers never rescan the system fonts for the same character. Blocks
    /// containing it are still flagged whenever they are measured, including while the scan
    /// runs, so `invalidate_missing` covers every block that was laid out without a face.
    pub fn take_missing(&mut self) -> Vec<char> {
        let missing = std::mem::take(&mut self.missing);
        #[cfg(test)]
        {
            self.set_ops += missing.len();
        }
        self.tried.extend(&missing);
        missing.into_iter().collect()
    }

    /// New width, scale, or font settings: everything is remeasured (old heights remain as
    /// estimates, so the scrollbar doesn't jump): each is scaled by how much its estimate
    /// changed with the column and the font size, as text rewraps in a new width, so a
    /// window or the wide layout making the column several times wider doesn't leave the
    /// document seeming that much longer until it is measured. Nothing happens, beyond
    /// recording `params`, when the column and everything else laid out stay the same (the
    /// wide layout in a window narrower than the cap).
    pub fn set_params(&mut self, params: Params) {
        let old_params = std::mem::replace(&mut self.params, params);
        if params == old_params {
            return;
        }
        let old = (self.em, self.col_x, self.col_w);
        self.set_geometry();
        let same = Params {
            wide: old_params.wide,
            line_width: old_params.line_width,
            ..params
        } == old_params;
        if same && old == (self.em, self.col_x, self.col_w) {
            return;
        }
        let new = (self.em, self.col_x, self.col_w);
        // The text wraps as before when the column is the same: only the font size scales
        // the heights of measured blocks.
        let ratio = if old.2 == new.2 && old.0 > 0.0 {
            new.0 / old.0
        } else {
            1.0
        };
        let basis = self.basis();
        let n = self.meta.len();
        for (i, (g, m)) in self.geoms.iter_mut().zip(&mut self.meta).enumerate() {
            let est = Self::estimate_at(*m, i, n, basis);
            g.h = if g.h == m.est {
                // Never measured: the estimate itself.
                est
            } else if old.2 == new.2 {
                g.h * ratio
            } else if m.est > 0.0 && g.h > 0.0 {
                g.h * est / m.est
            } else {
                est
            };
            m.est = est;
            g.unmeasure();
        }
        self.reset_measuring();
    }

    /// The faces text is set in changed (`font_sans` / `font_mono`): everything is remeasured.
    pub fn invalidate_all(&mut self) {
        self.remeasure();
    }

    /// Mark every block unmeasured, its old height remaining as the estimate. What was queued
    /// for `take_missing` came from measurements that are now void (the faces may have
    /// changed): measuring again queues whatever is still uncovered. `tried` is kept, so a
    /// character already returned is never returned again.
    fn remeasure(&mut self) {
        for g in &mut self.geoms {
            g.unmeasure();
        }
        self.reset_measuring();
    }

    /// Start measuring again from the top, every block having been [`BlockGeom::unmeasure`]d.
    fn reset_measuring(&mut self) {
        self.missing.clear();
        self.live.clear();
        self.table_cols.iter_mut().for_each(|c| *c = None);
        self.table_missing.iter_mut().for_each(|m| *m = false);
        self.cursor = 0;
        self.rebuild_heights();
    }

    /// Fallback faces were added: remeasure blocks that had uncovered characters (even ones
    /// already reported). A table whose sampled rows were affected is remeasured whole, since
    /// its column widths depend on them: either a sampled row was laid out with a missing
    /// character, or the columns were measured with one (the sampled rows may not have been
    /// laid out at all).
    pub fn invalidate_missing(&mut self, doc: &Doc) {
        for table in 0..self.table_missing.len() {
            if std::mem::take(&mut self.table_missing[table]) {
                self.table_cols[table] = None;
                self.reset_rows(self.table_rows[table].clone());
            }
        }
        for i in 0..self.geoms.len() {
            if self.geoms[i].missing {
                self.invalidate_block(doc, i);
            }
        }
    }

    /// Lay block `i` out again: a table row in its table's sampled rows remeasures the whole
    /// table, whose column widths depend on it.
    fn invalidate_block(&mut self, doc: &Doc, i: usize) {
        let rows = match doc.blocks[i].kind {
            BlockKind::TableRow { table, .. }
                if self.table_sample(doc, table as usize).contains(&(i as u32)) =>
            {
                self.table_cols[table as usize] = None;
                self.table_rows[table as usize].clone()
            }
            _ => i as u32..i as u32 + 1,
        };
        self.reset_rows(rows);
        self.live
            .retain(|&i| self.geoms[i as usize].content.is_some());
    }

    /// How local images are sized (`None`: they all show their alt text). Sizes looked up so
    /// far are forgotten, and their blocks laid out again.
    pub fn set_image_probe(&mut self, doc: &Doc, probe: Option<ImageProbe>) {
        self.probe = probe;
        for id in 1..=self.images.len() as u32 {
            if self.images[id as usize - 1].take().is_some() {
                self.invalidate_image(doc, id);
            }
        }
    }

    /// Image `id` (1-based) has a new size (fetched, failed, turned on or off): its block is
    /// laid out again.
    pub fn set_image(&mut self, doc: &Doc, id: u32, size: ImageSize) {
        let Some(slot) = id
            .checked_sub(1)
            .and_then(|i| self.images.get_mut(i as usize))
        else {
            return;
        };
        // A block measured with the image has looked its size up.
        if slot.replace(size).is_some_and(|was| was != size) {
            self.invalidate_image(doc, id);
        }
    }

    /// The size image `id` was laid out with, if its block was measured.
    pub fn image_size(&self, id: u32) -> Option<ImageSize> {
        *self.images.get(id.checked_sub(1)? as usize)?
    }

    fn invalidate_image(&mut self, doc: &Doc, id: u32) {
        if self.image_blocks.is_empty() {
            self.image_blocks = vec![u32::MAX; self.images.len()];
            for (b, block) in doc.blocks.iter().enumerate() {
                let cells = doc.row_cells(block).map(|(_, spans)| spans);
                for s in std::iter::once(doc.block_spans(block))
                    .chain(cells)
                    .flatten()
                {
                    if let Some(slot) = s
                        .image
                        .checked_sub(1)
                        .and_then(|i| self.image_blocks.get_mut(i as usize))
                        && *slot == u32::MAX
                    {
                        *slot = b as u32;
                    }
                }
            }
        }
        if let Some(&b) = self.image_blocks.get(id as usize - 1)
            && (b as usize) < self.geoms.len()
        {
            self.invalidate_block(doc, b as usize);
            // The chunks its alt text goes on in (a long block is cut) show it, or not, too.
            for k in b as usize + 1..self.geoms.len() {
                let block = &doc.blocks[k];
                let cells = doc.row_cells(block).map(|(_, spans)| spans);
                let has = std::iter::once(doc.block_spans(block))
                    .chain(cells)
                    .flatten()
                    .any(|s| s.image == id);
                if !block.joined || !has {
                    break;
                }
                self.invalidate_block(doc, k);
            }
        }
    }

    /// Look up the sizes of the images in `spans` not looked up yet.
    fn resolve_images(&mut self, doc: &Doc, spans: &[Span]) {
        for s in spans.iter().filter(|s| s.image != 0) {
            let i = s.image as usize - 1;
            if self.images.get(i) == Some(&None) {
                let size = match (&mut self.probe, doc.images.get(i)) {
                    (Some(probe), Some(img)) => probe(img),
                    _ => ImageSize::Alt,
                };
                self.images[i] = Some(size);
            }
        }
    }

    /// Mark the blocks measured again.
    fn reset_rows(&mut self, rows: Range<u32>) {
        for g in &mut self.geoms[rows.start as usize..rows.end as usize] {
            g.measured = false;
            g.content = None;
            g.missing = false;
        }
        self.cursor = self.cursor.min(rows.start as usize);
    }

    /// Queue characters no face covered for `take_missing`, unless already reported.
    fn report_missing(&mut self, chars: &[char]) {
        #[cfg(test)]
        {
            self.set_ops += chars.len();
        }
        for &c in chars {
            if !self.tried.contains(&c) {
                self.missing.insert(c);
            }
        }
    }

    fn set_geometry(&mut self) {
        let p = self.params;
        self.em = p.font_size * p.scale;
        let margin = if p.width < 30.0 * self.em {
            self.em
        } else {
            2.0 * self.em
        };
        let cap = if p.wide {
            f32::INFINITY
        } else {
            p.line_width * CHAR_EM * self.em
        };
        self.col_w = cap.min(p.width - 2.0 * margin).max(self.em);
        self.col_x = ((p.width - self.col_w) / 2.0).max(0.0);
    }

    fn prepass(&mut self, doc: &Doc) -> Vec<Meta> {
        let mut meta = vec![Meta::default(); doc.blocks.len()];
        let mut alert: Option<(Alert, u8)> = None;
        let mut row = 0usize;
        let footnotes = doc
            .blocks
            .iter()
            .position(|b| matches!(b.marker, Marker::Footnote(_)));
        for (i, b) in doc.blocks.iter().enumerate() {
            // A quote that starts at or outside the alert's depth is a different quote.
            if alert.is_some_and(|(_, d)| b.quote < d || (b.quote_start != 0 && b.quote_start <= d))
            {
                alert = None;
            }
            if let BlockKind::AlertTitle(a) = b.kind {
                alert = Some((a, b.quote));
            }
            meta[i].alert = alert;
            if let BlockKind::TableRow { table, header, .. } = b.kind {
                // A table's rows are consecutive blocks.
                let r = &mut self.table_rows[table as usize];
                if r.start == r.end {
                    *r = i as u32..i as u32 + 1;
                } else {
                    r.end = i as u32 + 1;
                }
                // Fragments of one row (`joined`) share its parity.
                row = if header {
                    0
                } else if b.joined {
                    row
                } else {
                    row + 1
                };
                meta[i].set(Meta::STRIPE, !header && row.is_multiple_of(2));
                meta[i].set(Meta::FIRST_ROW, r.start == i as u32);
            }
            meta[i].set(Meta::FOOTNOTE, footnotes.is_some_and(|f| i >= f));
            let spans = doc.block_spans(b);
            let m = &mut meta[i];
            m.chars = text_bytes(doc, b).try_into().unwrap_or(u32::MAX);
            m.gap = if i == 0 {
                0
            } else {
                quarters(Self::gap_em(doc, i)) as u8
            };
            let pads = !b.joined as u8 + !joined_next(doc, i) as u8;
            m.flags |= pads << Meta::PADS_SHIFT;
            m.indent = quarters(nesting_x(doc, b, b.quote, b.list) + Self::footnote_x(b, *m));
            m.shape = match b.kind {
                BlockKind::Paragraph | BlockKind::AlertTitle(_) => Est::Text,
                BlockKind::Heading(level) => Est::Heading(level.clamp(1, 6)),
                BlockKind::Code => {
                    let newlines: usize = spans
                        .iter()
                        .map(|s| doc.span_text(s).matches('\n').count())
                        .sum();
                    let ends_line = spans
                        .last()
                        .is_some_and(|s| doc.span_text(s).ends_with('\n'));
                    m.code_lines = (newlines + !ends_line as usize)
                        .try_into()
                        .unwrap_or(u32::MAX);
                    Est::Code
                }
                BlockKind::Rule => Est::Rule,
                BlockKind::TableRow { .. } => Est::Row,
            };
        }
        meta
    }

    /// Prefix sums over all heights again, after they were all set at once.
    fn rebuild_heights(&mut self) {
        self.heights = Heights::new(self.geoms.iter().map(|g| g.h));
    }

    /// Give block `i` the height `h`.
    fn set_height(&mut self, i: usize, h: f32) {
        let delta = f64::from(h) - f64::from(self.geoms[i].h);
        if delta != 0.0 {
            self.heights.add(i, delta);
        }
        self.geoms[i].h = h;
    }

    /// Prefix-sum nodes visited so far (for tests of the cost).
    #[cfg(all(test, feature = "embed-fonts"))]
    fn position_ops(&self) -> usize {
        self.heights.ops.get()
    }

    /// How many blocks end at or before `y` (strict: start before `y`), by the f32 tops
    /// [`Layout::y`] hands out, so a top maps back to its own block.
    fn blocks_before(&self, y: f32, strict: bool) -> usize {
        self.blocks_before_by(f64::from(y), strict, |i| f64::from(self.y(i)))
    }

    /// [`Layout::blocks_before`] counting by `top(i)`, the top of block `i`.
    fn blocks_before_by(&self, y: f64, strict: bool, top: impl Fn(usize) -> f64) -> usize {
        let n = self.geoms.len();
        let counted = |i: usize| {
            if strict { top(i) < y } else { top(i + 1) <= y }
        };
        // The tree searches in f64; the rounding to f32 can move the answer by a block or two.
        let within = self.heights.blocks_within(y, strict);
        let mut count = if strict {
            if y > 0.0 { (within + 1).min(n) } else { 0 }
        } else {
            within
        };
        while count < n && counted(count) {
            count += 1;
        }
        while count > 0 && !counted(count - 1) {
            count -= 1;
        }
        count
    }

    // ---- Queries ----------------------------------------------------------------------

    /// Index of the block containing `y` (clamped to the document).
    pub fn block_at(&self, y: f32) -> usize {
        let i = self.blocks_before(y, false);
        i.min(self.geoms.len().saturating_sub(1))
    }

    /// Blocks overlapping `[y0, y1)`.
    pub fn range(&self, y0: f32, y1: f32) -> Range<usize> {
        if self.geoms.is_empty() {
            return 0..0;
        }
        let start = self.blocks_before(y0, false);
        let end = self.blocks_before(y1, true);
        start..end.max(start)
    }

    /// [`Layout::range`] by the exact f64 tops ([`Layout::top`]), for painting: rounding the
    /// bounds to f32 can drop a block that still overlaps the viewport deep into a tall document.
    pub fn range_exact(&self, y0: f64, y1: f64) -> Range<usize> {
        if self.geoms.is_empty() {
            return 0..0;
        }
        let start = self.blocks_before_by(y0, false, |i| self.top(i));
        let end = self.blocks_before_by(y1, true, |i| self.top(i));
        start..end.max(start)
    }

    pub fn anchor_at(&self, y: f32) -> Anchor {
        if self.geoms.is_empty() {
            return Anchor::TOP;
        }
        let i = self.block_at(y.max(0.0));
        let g = &self.geoms[i];
        let frac = if g.h > 0.0 {
            ((y - self.y(i)) / g.h).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Anchor {
            block: i as u32,
            frac,
        }
    }

    pub fn y_of(&self, a: Anchor) -> f32 {
        match self.geoms.get(a.block as usize) {
            Some(g) => self.y(a.block as usize) + a.frac * g.h,
            None => self.total_height(),
        }
    }

    /// [`Layout::anchor_at`] for an f64 `y`, without rounding through f32: scrolling by a pixel
    /// must move the anchor however far down a tall document it is.
    pub fn anchor_exact(&self, y: f64) -> Anchor {
        if self.geoms.is_empty() {
            return Anchor::TOP;
        }
        let y = y.max(0.0);
        let i = self
            .blocks_before_by(y, false, |i| self.top(i))
            .min(self.geoms.len() - 1);
        let h = f64::from(self.geoms[i].h);
        let frac = if h > 0.0 {
            ((y - self.top(i)) / h).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Anchor {
            block: i as u32,
            frac: frac as f32,
        }
    }

    /// [`Layout::y_of`] in f64.
    pub fn y_exact(&self, a: Anchor) -> f64 {
        match self.geoms.get(a.block as usize) {
            Some(g) => self.top(a.block as usize) + f64::from(a.frac) * f64::from(g.h),
            None => self.top(self.geoms.len()),
        }
    }

    /// Link and document position under (`x`, `y`), document-area coordinates. Only blocks
    /// with content (near the viewport) can be hit.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let i = self.range(y, y + f32::EPSILON).start;
        let g = self.geoms.get(i)?;
        let content = g.content.as_ref()?;
        let (lx, ly) = (x - self.col_x, y - self.y(i));
        for line in content
            .lines
            .iter()
            .filter(|l| l.top <= ly && ly < l.bottom)
        {
            let hits = &content.hits[line.hits.start as usize..line.hits.end as usize];
            if let Some(h) = hits.iter().find(|h| h.x0 <= lx && lx < h.x1) {
                let link = content.styles[h.style_index()].link;
                return Some(Hit {
                    block: i as u32,
                    doc: h.doc,
                    link,
                });
            }
        }
        None
    }

    // ---- Measuring --------------------------------------------------------------------

    /// Measure blocks around the viewport (anchor at its top, `view_h` tall) exactly and keep
    /// their content; drop content far away. Returns the anchor's new y, which moves when
    /// blocks above it change height.
    pub fn ensure(&mut self, doc: &Doc, fonts: &Fonts, top: Anchor, view_h: f32) -> f32 {
        let view_h = view_h.max(self.em);
        for pass in 0.. {
            let y = self.y_of(top);
            let view = self.range(y, y + view_h);
            // The margin around the viewport is measured for a few passes only; after that
            // just the viewport itself is filled in.
            let r = if pass < MEASURE_PASSES {
                self.range(
                    y - MEASURE_SCREENS * view_h,
                    y + (1.0 + MEASURE_SCREENS) * view_h,
                )
            } else {
                view
            };
            let mut changed = false;
            for i in r {
                if self.geoms[i].content.is_some() {
                    continue;
                }
                let (content, h) = self.build(doc, fonts, i);
                let g = &mut self.geoms[i];
                changed |= !g.measured || (g.h - h).abs() > 1e-3;
                g.measured = true;
                g.content = Some(Box::new(content));
                self.set_height(i, h);
                self.live.push(i as u32);
            }
            // Exact heights move blocks, and strongly overestimated ones can leave the
            // viewport showing blocks that were never built. Each such pass builds at least one
            // block, so this ends.
            let y = self.y_of(top);
            let bare = self
                .range(y, y + view_h)
                .any(|i| self.geoms[i].content.is_none());
            if !bare && (!changed || pass + 1 >= MEASURE_PASSES) {
                break;
            }
        }
        let y = self.y_of(top);
        let keep = self.range(y - KEEP_SCREENS * view_h, y + (1.0 + KEEP_SCREENS) * view_h);
        let geoms = &mut self.geoms;
        self.live.retain(|&i| {
            let keep = keep.contains(&(i as usize));
            if !keep {
                geoms[i as usize].content = None;
            }
            keep
        });
        y
    }

    /// Measure unmeasured blocks until `deadline`. Returns true when the whole document has
    /// exact heights. Callers keep their scroll position with an [`Anchor`].
    pub fn idle(&mut self, doc: &Doc, fonts: &Fonts, deadline: Instant) -> bool {
        while self.cursor < self.geoms.len() {
            let i = self.cursor;
            if !self.geoms[i].measured {
                let (_, h) = self.build(doc, fonts, i);
                self.set_height(i, h);
                self.geoms[i].measured = true;
            }
            self.cursor += 1;
            if Instant::now() >= deadline {
                break;
            }
        }
        self.cursor >= self.geoms.len()
    }

    /// Lay out block `i`: its content and total height (spacing above included).
    fn build(&mut self, doc: &Doc, fonts: &Fonts, i: usize) -> (BlockBox, f32) {
        let block = &doc.blocks[i];
        let meta = self.meta[i];
        let em = self.em;
        let gap = self.gap_before(doc, i) * em;
        let indent = self.indent(doc, block, meta);
        let avail = (self.col_w - indent).max(em);
        let last = !joined_next(doc, i);
        let mut out = BlockBox::default();
        let mut missing = Vec::new();
        let body = self.body_style(block, meta);

        let content_h = match block.kind {
            BlockKind::Paragraph | BlockKind::Heading(_) => {
                let lh = if matches!(block.kind, BlockKind::Heading(_)) {
                    HEADING_LH
                } else {
                    BODY_LH
                };
                self.resolve_images(doc, doc.block_spans(block));
                let boxes = Boxes {
                    sizes: &self.images,
                    scale: self.params.scale,
                    em,
                    max_w: avail,
                };
                fill_para(&mut self.para, doc, doc.block_spans(block), body, &boxes);
                let pl = self.ctx.layout(fonts, &self.para, avail, Align::Start, lh);
                missing.extend_from_slice(&pl.missing);
                let first_baseline = pl.lines.first().map(|l| l.baseline + gap);
                let mut h = pl.height;
                let styles = std::mem::take(&mut self.para.styles);
                out.append(pl, &styles, indent, gap);
                self.para.styles = styles;
                if let BlockKind::Heading(1 | 2) = block.kind
                    && last
                {
                    h += 0.3 * em;
                    out.rect(indent, gap + h, avail, self.hairline(), Role::BorderMuted);
                    h += self.hairline();
                }
                if let Some(baseline) = first_baseline {
                    let gutter = self.gutter(doc, block, meta);
                    self.marker(fonts, block, meta, gutter, baseline, &mut out, &mut missing);
                }
                h
            }
            BlockKind::Code => self.code(
                doc,
                fonts,
                (block, last),
                indent,
                avail,
                gap,
                &mut out,
                &mut missing,
            ),
            BlockKind::Rule => {
                let h = 0.25 * em;
                out.rect(indent, gap, avail, h, Role::Border);
                h
            }
            BlockKind::AlertTitle(alert) => {
                let label = match alert {
                    Alert::Note => "Note",
                    Alert::Tip => "Tip",
                    Alert::Important => "Important",
                    Alert::Warning => "Warning",
                    Alert::Caution => "Caution",
                };
                let style = TextStyle {
                    bold: true,
                    role: alert_role(alert),
                    ..body
                };
                let icon = ALERT_ICON * em;
                let text_x = icon + 0.5 * em;
                self.para.clear();
                self.para.push_synthetic(label, 0, style);
                let pl = self.ctx.layout(
                    fonts,
                    &self.para,
                    (avail - text_x).max(em),
                    Align::Start,
                    BODY_LH,
                );
                let s = out.push_style(style);
                out.append_decor(&pl, s, indent + text_x, gap);
                #[cfg(feature = "clipboard")]
                out.set_decor((indent, gap, indent + text_x + pl.width, gap + pl.height));
                if let Some(line) = pl.lines.first() {
                    // Centered on the label's capitals: about 0.36 em above the baseline.
                    let top = gap + line.baseline - 0.36 * em - icon / 2.0;
                    self.alert_icon(fonts, alert, (indent, top), &mut out, &mut missing);
                }
                pl.height
            }
            BlockKind::TableRow { table, .. } => self.table_row(
                doc,
                fonts,
                (block, meta, last),
                table as usize,
                indent,
                gap,
                &mut out,
                &mut missing,
            ),
        };
        self.quote_bars(doc, i, meta, gap, content_h, &mut out);
        // Always flag the block: a fallback scan may still be running for these characters.
        self.geoms[i].missing |= !missing.is_empty();
        self.report_missing(&missing);
        (out, gap + content_h + self.page_end(doc, i))
    }

    /// The icon in front of an alert's title (GitHub draws Octicons; ours are built from an
    /// outline and a glyph of the embedded font): Note ⓘ, Tip ✓ and Caution ! in a circle,
    /// Important ! in a rounded square, Warning ! cut out of a triangle. `top_left` is the corner
    /// of its square, [`ALERT_ICON`] em wide.
    fn alert_icon(
        &mut self,
        fonts: &Fonts,
        alert: Alert,
        (x, y): (f32, f32),
        out: &mut BlockBox,
        missing: &mut Vec<char>,
    ) {
        let em = self.em;
        let size = ALERT_ICON * em;
        let role = alert_role(alert);
        let stroke = (0.09 * em).round().max(1.0);
        let (radius, mark) = match alert {
            Alert::Note => (size / 2.0, "i"),
            Alert::Tip => (size / 2.0, "✓"),
            Alert::Important => (0.25 * size, "!"),
            Alert::Caution => (size / 2.0, "!"),
            Alert::Warning => (0.0, "!"),
        };
        if alert != Alert::Warning {
            out.rects.push(PRect {
                x,
                y,
                w: size,
                h: size,
                role,
                radius,
                kind: RectKind::Stroke(stroke),
            });
        }
        let mut glyph = |this: &mut Self, text: &str, size_em: f32, role: Role, center_y: f32| {
            let style = TextStyle {
                bold: true,
                italic: false,
                mono: false,
                size: size_em * em,
                role,
                deco: 0,
                shift: 0.0,
                link: 0,
            };
            this.para.clear();
            this.para.push_synthetic(text, 0, style);
            let pl = this.ctx.layout(fonts, &this.para, size, Align::Center, 1.0);
            missing.extend_from_slice(&pl.missing);
            let s = out.push_style(style);
            out.append_decor(&pl, s, x, center_y - pl.height / 2.0);
        };
        if alert == Alert::Warning {
            // A filled triangle with the mark in the background color, low in it, where the
            // triangle is widest.
            glyph(self, "▲", 1.25, role, y + size / 2.0);
            glyph(self, mark, 0.55, Role::Bg, y + size * 0.62);
            return;
        }
        glyph(self, mark, 0.62, role, y + size / 2.0);
    }

    /// Physical width of a 1-logical-pixel line.
    fn hairline(&self) -> f32 {
        self.params.scale.round().max(1.0)
    }

    /// Spacing above block `i`, in em.
    fn gap_before(&self, doc: &Doc, i: usize) -> f32 {
        if i == 0 {
            return self.params.page_pad;
        }
        Self::gap_em(doc, i)
    }

    /// Spacing above block `i`, which is not the first, in em.
    fn gap_em(doc: &Doc, i: usize) -> f32 {
        if doc.blocks[i].joined {
            return 0.0;
        }
        let (prev, cur) = (&doc.blocks[i - 1], &doc.blocks[i]);
        match (prev.kind, cur.kind) {
            (_, BlockKind::Heading(_)) => 1.5,
            (BlockKind::Rule, _) | (_, BlockKind::Rule) => 1.5,
            (BlockKind::TableRow { table: a, .. }, BlockKind::TableRow { table: b, .. })
                if a == b =>
            {
                0.0
            }
            (BlockKind::AlertTitle(_), _) => 0.25,
            (BlockKind::Heading(_), _) => 1.0,
            _ if prev.list > 0 && cur.list > 0 => {
                if cur.marker == Marker::None {
                    0.75
                } else {
                    0.25
                }
            }
            _ => 1.0,
        }
    }

    /// Extra indent in em for blocks inside the footnote section, applied to everything in the
    /// block: its number in the gutter, quote bars, and text.
    fn footnote_x(block: &Block, meta: Meta) -> f32 {
        let footnote = matches!(block.marker, Marker::Footnote(_))
            || (meta.footnote() && block.list == 0 && block.kind != BlockKind::Rule);
        if footnote { LIST_INDENT } else { 0.0 }
    }

    /// Left edge of the block's content.
    fn indent(&self, doc: &Doc, block: &Block, meta: Meta) -> f32 {
        (nesting_x(doc, block, block.quote, block.list) + Self::footnote_x(block, meta)) * self.em
    }

    /// Where the block's marker hangs from: the content edge of the item's list level, left of
    /// any quote that opened inside the item.
    fn gutter(&self, doc: &Doc, block: &Block, meta: Meta) -> f32 {
        let outer = doc
            .block_quote_lists(block)
            .iter()
            .take_while(|&&opened| opened < block.list)
            .count();
        (nesting_x(doc, block, outer as u8, block.list) + Self::footnote_x(block, meta)) * self.em
    }

    fn body_style(&self, block: &Block, meta: Meta) -> TextStyle {
        let em = self.em;
        let mut s = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: if meta.footnote() {
                FOOTNOTE_SIZE * em
            } else {
                em
            },
            role: if block.quote > 0 && meta.alert.is_none() {
                Role::Muted
            } else {
                Role::Text
            },
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        if let BlockKind::Heading(level) = block.kind {
            s.size = HEADING_SIZES[(level.clamp(1, 6) - 1) as usize] * em;
            s.bold = true;
            if level == 6 {
                s.role = Role::Muted;
            }
        }
        s
    }

    /// Bullet, number, checkbox, or footnote number in the gutter left of `gutter`, aligned to
    /// the first line's baseline.
    #[allow(clippy::too_many_arguments)]
    fn marker(
        &mut self,
        fonts: &Fonts,
        block: &Block,
        meta: Meta,
        gutter: f32,
        baseline: f32,
        out: &mut BlockBox,
        missing: &mut Vec<char>,
    ) {
        let em = self.em;
        let body = self.body_style(block, meta);
        let text = match block.marker {
            Marker::None => return,
            Marker::Bullet => match block.list {
                0 | 1 => "•".to_owned(),
                2 => "◦".to_owned(),
                _ => "▪".to_owned(),
            },
            Marker::Ordered(n) => format!("{n}."),
            Marker::Footnote(n) => format!("{n}."),
            Marker::Task(checked) => {
                let size = 0.875 * em;
                let x = gutter - 1.375 * em;
                let y = baseline - 0.8 * em;
                out.rects.push(PRect {
                    x,
                    y,
                    w: size,
                    h: size,
                    role: if checked { Role::Link } else { Role::Border },
                    radius: 0.2 * em,
                    kind: if checked {
                        RectKind::Fill
                    } else {
                        RectKind::Stroke(self.hairline())
                    },
                });
                #[cfg(feature = "clipboard")]
                out.set_decor((x, y, x + size, y + size));
                if !checked {
                    return;
                }
                let style = TextStyle {
                    bold: true,
                    size: 0.8 * em,
                    role: Role::Bg,
                    ..body
                };
                self.para.clear();
                self.para.push_synthetic("✓", 0, style);
                let pl = self.ctx.layout(fonts, &self.para, size, Align::Center, 1.0);
                let dy = y + size / 2.0 - pl.height / 2.0;
                let s = out.push_style(style);
                out.append_decor(&pl, s, x, dy);
                return;
            }
        };
        let style = TextStyle {
            role: Role::Muted,
            ..body
        };
        self.para.clear();
        self.para.push_synthetic(&text, 0, style);
        // Laid out unwrapped and right-aligned by hand: long numbers extend into the margin
        // instead of breaking inside the gutter.
        let pl = self
            .ctx
            .layout(fonts, &self.para, NO_WRAP, Align::Start, BODY_LH);
        missing.extend_from_slice(&pl.missing);
        let dy = baseline - pl.lines.first().map_or(0.0, |l| l.baseline);
        let s = out.push_style(style);
        let x = gutter - 0.5 * em - pl.width;
        out.append_decor(&pl, s, x, dy);
        #[cfg(feature = "clipboard")]
        out.set_decor((x, dy, x + pl.width, dy + pl.height));
    }

    #[allow(clippy::too_many_arguments)]
    fn code(
        &mut self,
        doc: &Doc,
        fonts: &Fonts,
        (block, last): (&Block, bool),
        indent: f32,
        avail: f32,
        gap: f32,
        out: &mut BlockBox,
        missing: &mut Vec<char>,
    ) -> f32 {
        let em = self.em;
        let pad = em;
        // A chunked block is one continuous background: padding only at the run's outer edges.
        let (top, bottom) = (
            if block.joined { 0.0 } else { pad },
            if last { pad } else { 0.0 },
        );
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: true,
            size: CODE_SIZE * em,
            role: Role::Text,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        self.para.clear();
        for span in doc.block_spans(block) {
            push_expanding_tabs(&mut self.para, doc.span_text(span), span.start, style);
        }
        if self.para.styles.is_empty() {
            self.para.styles.push(style);
        }
        let inner = (avail - 2.0 * pad).max(em);
        let pl = self
            .ctx
            .layout(fonts, &self.para, inner, Align::Start, CODE_LH);
        missing.extend_from_slice(&pl.missing);
        let h = pl.height + top + bottom;
        out.rects.push(PRect {
            x: indent,
            y: gap,
            w: avail,
            h,
            role: Role::CodeBg,
            radius: if block.joined || !last {
                0.0
            } else {
                6.0 * self.params.scale
            },
            kind: RectKind::Fill,
        });
        let conts: Vec<f32> = pl
            .lines
            .iter()
            .filter(|l| l.cont)
            .map(|l| l.baseline)
            .collect();
        let styles = std::mem::take(&mut self.para.styles);
        out.append(pl, &styles, indent + pad, gap + top);
        self.para.styles = styles;
        // Continuation markers in the left padding for soft-wrapped lines.
        if !conts.is_empty() {
            let style = TextStyle {
                role: Role::Muted,
                ..style
            };
            self.para.clear();
            self.para.push_synthetic("↪", 0, style);
            let mark = self
                .ctx
                .layout(fonts, &self.para, pad, Align::Center, CODE_LH);
            let mark_baseline = mark.lines.first().map_or(0.0, |l| l.baseline);
            let s = out.push_style(style);
            for baseline in conts {
                out.append_decor(&mark, s, indent, gap + top + baseline - mark_baseline);
            }
        }
        h
    }

    /// One table row, or one fragment of a split row: a fragment has no rule or padding where
    /// it joins its neighbor, but keeps the row's stripe and vertical rules.
    #[allow(clippy::too_many_arguments)]
    fn table_row(
        &mut self,
        doc: &Doc,
        fonts: &Fonts,
        (block, meta, last): (&Block, Meta, bool),
        table: usize,
        indent: f32,
        gap: f32,
        out: &mut BlockBox,
        missing: &mut Vec<char>,
    ) -> f32 {
        let em = self.em;
        let cols = self.table_cols(doc, fonts, table, indent);
        let (px, py) = (CELL_PAD_X * em, CELL_PAD_Y * em);
        let (top, bottom) = (
            if block.joined { 0.0 } else { py },
            if last { py } else { 0.0 },
        );
        let header = matches!(block.kind, BlockKind::TableRow { header: true, .. });
        let body = TextStyle {
            bold: header,
            ..self.body_style(block, meta)
        };
        let aligns = &doc.tables[table].aligns;
        // There is no horizontal scrolling: columns that start past the document's right edge
        // can't be seen, so a row costs its visible columns (and the populated cells among them),
        // however many columns the table has. A column's cell and rules count from its left edge.
        let reach = (self.params.width - self.col_x - indent).max(0.0);
        let (mut visible, mut edge) = (0, 0.0f32);
        while visible < cols.len() && edge < reach {
            edge += cols[visible];
            visible += 1;
        }
        let mut cells = Vec::new();
        let mut row_h: f32 = 0.0;
        // A fragment of a split row only stores the cells that still have text.
        let stored = doc.row_cells(block).take_while(|&(c, _)| c < visible);
        for (c, spans) in stored {
            self.resolve_images(doc, spans);
            let boxes = Boxes {
                sizes: &self.images,
                scale: self.params.scale,
                em,
                max_w: (cols[c] - 2.0 * px).max(1.0),
            };
            fill_para(&mut self.para, doc, spans, body, &boxes);
            let align = match aligns.get(c) {
                Some(doc::Align::Center) => Align::Center,
                Some(doc::Align::Right) => Align::Right,
                _ => Align::Start,
            };
            let pl = self.ctx.layout(
                fonts,
                &self.para,
                (cols[c] - 2.0 * px).max(1.0),
                align,
                BODY_LH,
            );
            #[cfg(test)]
            {
                self.cells_laid += 1;
            }
            missing.extend_from_slice(&pl.missing);
            row_h = row_h.max(pl.height);
            cells.push((c, pl, std::mem::take(&mut self.para.styles)));
        }
        if cells.is_empty() {
            self.para.clear();
            self.para.styles.push(body);
            let pl = self
                .ctx
                .layout(fonts, &self.para, 1.0, Align::Start, BODY_LH);
            row_h = pl.height;
        }
        let h = row_h + top + bottom;
        let width = edge.min(reach);
        let line = self.hairline();
        if meta.stripe() {
            out.rect(indent, gap, width, h, Role::TableStripe);
        }
        if meta.first_row() {
            out.rect(indent, gap, width, line, Role::Border);
        }
        if last {
            out.rect(indent, gap + h - line, width, line, Role::Border);
        }
        out.rect(indent, gap, line, h, Role::Border);
        let mut x = indent;
        let mut cells = cells.into_iter().peekable();
        for (c, w) in cols[..visible].iter().enumerate() {
            if let Some((_, pl, styles)) = cells.next_if(|(col, ..)| *col == c) {
                out.append(pl, &styles, x + px, gap + top);
            }
            x += w;
            // A rule on the far side of the visible width can't be seen.
            if x - indent <= reach {
                out.rect(x - line, gap, line, h, Role::Border);
            }
        }
        h
    }

    /// The blocks of a table whose cells size its columns: its first rows, up to
    /// [`TABLE_SAMPLE_ROWS`] blocks or [`TABLE_SAMPLE_BYTES`] of text, counted per block (a
    /// fragment of a split row that would pass the budget isn't sampled). Always at least the
    /// header's first block.
    fn table_sample(&self, doc: &Doc, table: usize) -> Range<u32> {
        let rows = self.table_rows[table].clone();
        let (mut end, mut bytes) = (rows.start, 0);
        while end < rows.end && ((end - rows.start) as usize) < TABLE_SAMPLE_ROWS {
            let next = text_bytes(doc, &doc.blocks[end as usize]);
            if end > rows.start && bytes + next > TABLE_SAMPLE_BYTES {
                break;
            }
            bytes += next;
            end += 1;
        }
        rows.start..end
    }

    /// Column widths for a table (cell padding included), from the min/natural widths of the
    /// cells in its [`table_sample`](Self::table_sample): natural widths if they fit,
    /// otherwise the slack is shared in proportion to how much each column wants to grow (CSS
    /// auto-layout, simplified). Rows past the sample never influence the columns; their text
    /// wraps within them (a table that fits and has such rows is stretched to the full width,
    /// leaving them room).
    fn table_cols(&mut self, doc: &Doc, fonts: &Fonts, table: usize, indent: f32) -> Arc<[f32]> {
        if let Some(cols) = &self.table_cols[table] {
            return Arc::clone(cols);
        }
        let em = self.em;
        let px = CELL_PAD_X * em;
        let n = doc.tables[table].aligns.len().max(1);
        let (mut min, mut max) = (vec![0.0f32; n], vec![0.0f32; n]);
        let sample = self.table_sample(doc, table);
        let truncated = sample.end < self.table_rows[table].end;
        let mut missing = Vec::new();
        for r in sample {
            let block = &doc.blocks[r as usize];
            let header = matches!(block.kind, BlockKind::TableRow { header: true, .. });
            let body = TextStyle {
                bold: header,
                ..self.body_style(block, self.meta[r as usize])
            };
            for (c, spans) in doc.row_cells(block).take_while(|&(c, _)| c < n) {
                self.resolve_images(doc, spans);
                // An image claims at most its column's share of the line, so one doesn't push
                // the others off the edge (a cell shrinks it to fit).
                let boxes = Boxes {
                    sizes: &self.images,
                    scale: self.params.scale,
                    em,
                    max_w: ((self.col_w - indent) / n as f32 - 2.0 * px).max(em),
                };
                fill_para(&mut self.para, doc, spans, body, &boxes);
                let (lo, hi, absent) = self.ctx.measure_missing(fonts, &self.para);
                min[c] = min[c].max(lo);
                max[c] = max[c].max(hi);
                missing.extend(absent);
            }
        }
        // The sampled rows may never be laid out themselves, so their missing characters are
        // recorded with the table: loading a fallback must remeasure its columns.
        if !missing.is_empty() {
            self.table_missing[table] = true;
            missing.sort_unstable();
            missing.dedup();
            self.report_missing(&missing);
        }
        let avail = (self.col_w - indent - n as f32 * 2.0 * px).max(0.0);
        let (sum_min, sum_max): (f32, f32) = (min.iter().sum(), max.iter().sum());
        let widths: Vec<f32> = if sum_max <= avail {
            if truncated && sum_max > 0.0 {
                // Rows past the sample may be a little wider than any in it: spend the slack
                // on the columns instead of wrapping them for a digit.
                max.iter().map(|w| w * avail / sum_max).collect()
            } else {
                max
            }
        } else if sum_min >= avail || sum_max <= sum_min {
            min
        } else {
            let t = (avail - sum_min) / (sum_max - sum_min);
            min.iter()
                .zip(&max)
                .map(|(lo, hi)| lo + (hi - lo) * t)
                .collect()
        };
        let cols: Arc<[f32]> = widths.into_iter().map(|w| w.ceil() + 2.0 * px).collect();
        self.table_cols[table] = Some(Arc::clone(&cols));
        cols
    }

    /// Quote bars for each nesting level, joined across the gap when the previous block is
    /// in the same quote.
    fn quote_bars(
        &self,
        doc: &Doc,
        i: usize,
        meta: Meta,
        gap: f32,
        content_h: f32,
        out: &mut BlockBox,
    ) {
        let block = &doc.blocks[i];
        let opened = doc.block_quote_lists(block);
        let prev_quote = if i > 0 { doc.blocks[i - 1].quote } else { 0 };
        for q in 0..block.quote {
            // Joined to the previous block only when it is in the same quote.
            let joined = prev_quote > q && !(block.quote_start != 0 && block.quote_start <= q + 1);
            let top = if joined { 0.0 } else { gap };
            let role = match meta.alert {
                Some((a, depth)) if depth == q + 1 => alert_role(a),
                _ => Role::QuoteBar,
            };
            out.rect(
                (nesting_x(
                    doc,
                    block,
                    q,
                    opened.get(q as usize).map_or(0, |&d| d.min(block.list)),
                ) + Self::footnote_x(block, meta))
                    * self.em,
                top,
                QUOTE_BAR * self.em,
                gap + content_h - top,
                role,
            );
        }
    }

    /// Padding below the last block (part of its height, so scrolling reaches it).
    fn page_end(&self, doc: &Doc, i: usize) -> f32 {
        if i + 1 == doc.blocks.len() {
            self.params.page_pad * self.em
        } else {
            0.0
        }
    }

    /// What the height estimates depend on now.
    fn basis(&self) -> Basis {
        Basis {
            em: self.em,
            col_w: self.col_w,
            page_pad: self.params.page_pad,
        }
    }

    /// Estimate every block's height under the current geometry.
    fn estimate_all(&mut self) {
        let (basis, n) = (self.basis(), self.meta.len());
        for (i, m) in self.meta.iter_mut().enumerate() {
            m.est = Self::estimate_at(*m, i, n, basis);
        }
    }

    /// Height estimate for an unmeasured block `i` of `n`, whose `meta` the prepass filled,
    /// for any `basis`; reads nothing else.
    #[inline(always)]
    fn estimate_at(meta: Meta, i: usize, n: usize, basis: Basis) -> f32 {
        let em = basis.em;
        let pad = basis.page_pad * em;
        let edges = if i == 0 { pad } else { 0.0 } + if i + 1 == n { pad } else { 0.0 };
        let avail = (basis.col_w - from_quarters(meta.indent) * em).max(em);
        let lines = |size: f32| {
            ((meta.chars as f32 * CHAR_EM * size) / avail)
                .ceil()
                .max(1.0)
        };
        from_quarters(meta.gap) * em
            + edges
            + match meta.shape {
                Est::Text => lines(em) * BODY_LH * em,
                Est::Heading(level) => {
                    let size = HEADING_SIZES[(level - 1) as usize] * em;
                    lines(size) * HEADING_LH * size
                }
                Est::Code => {
                    (meta.code_lines as f32).max(lines(CODE_SIZE * em)) * CODE_LH * CODE_SIZE * em
                        + f32::from(meta.pads()) * em
                }
                Est::Rule => 0.25 * em,
                // The cells share the width: a row is about as tall as its text set on one line.
                Est::Row => lines(em) * BODY_LH * em + f32::from(meta.pads()) * CELL_PAD_Y * em,
            }
    }
}

/// Bytes of text in a block's spans.
fn text_bytes(doc: &Doc, block: &Block) -> usize {
    doc.block_spans(block)
        .iter()
        .map(|s| (s.end - s.start) as usize)
        .sum()
}

/// The next block continues this one (see [`Block::joined`]).
fn joined_next(doc: &Doc, i: usize) -> bool {
    doc.blocks.get(i + 1).is_some_and(|b| b.joined)
}

/// Offset in em along the block's nesting after its first `quotes` quote levels and `lists`
/// list levels, walked in their real order (a quote inside a list item is indented by the list;
/// a list inside a quote, by the quote). `quotes == block.quote` and `lists == block.list` give
/// the content's indent; a quote level's bar sits at `quotes` = that level and `lists` = the
/// list depth where it opened.
fn nesting_x(doc: &Doc, block: &Block, quotes: u8, lists: u8) -> f32 {
    let opened = doc.block_quote_lists(block);
    let (mut x, mut depth) = (0.0, 0);
    for q in 0..quotes as usize {
        let at = opened
            .get(q)
            .copied()
            .unwrap_or(depth)
            .min(lists)
            .max(depth);
        x += (at - depth) as f32 * LIST_INDENT + QUOTE_INDENT;
        depth = at;
    }
    x + lists.saturating_sub(depth) as f32 * LIST_INDENT
}

fn alert_role(a: Alert) -> Role {
    match a {
        Alert::Note => Role::AlertNote,
        Alert::Tip => Role::AlertTip,
        Alert::Important => Role::AlertImportant,
        Alert::Warning => Role::AlertWarning,
        Alert::Caution => Role::AlertCaution,
    }
}

/// Style for one span on top of the block's body style.
fn span_style(span: &Span, body: TextStyle) -> TextStyle {
    let mut s = body;
    s.bold |= span.style & style::BOLD != 0;
    s.italic |= span.style & style::ITALIC != 0;
    if span.style & style::STRIKE != 0 {
        s.deco |= deco::STRIKE;
    }
    if span.style & style::CODE != 0 {
        s.mono = true;
        s.size = body.size * CODE_SIZE;
        s.deco |= deco::CODE_BG;
    }
    if span.style & style::LINK != 0 {
        s.role = Role::Link;
        s.link = span.link;
    }
    if span.style & style::IMAGE != 0 {
        s.italic = true;
        s.role = if span.link != 0 {
            Role::Link
        } else {
            Role::Muted
        };
        s.deco |= deco::IMAGE_BOX;
    }
    if span.style & (style::FOOTREF | style::SUP) != 0 {
        s.size = body.size * 0.75;
        s.shift = body.size * 0.4;
    } else if span.style & style::SUB != 0 {
        s.size = body.size * 0.75;
        s.shift = -body.size * 0.2;
    }
    s
}

/// What [`fill_para`] needs to size image boxes: each image's size, the scale, and the line's
/// width, which a box never exceeds.
struct Boxes<'a> {
    sizes: &'a [Option<ImageSize>],
    scale: f32,
    em: f32,
    max_w: f32,
}

impl Boxes<'_> {
    /// The box image `id` is shown in, whole physical pixels; `None` for its alt text.
    fn size(&self, id: u32) -> Option<(f32, f32)> {
        let (w, h) = match (*self.sizes.get(id.checked_sub(1)? as usize)?)? {
            ImageSize::Alt => return None,
            ImageSize::Loading => {
                let w = (LOADING_EM * self.em).min(self.max_w);
                (w, w * 0.75)
            }
            ImageSize::Pixels(w, h) => (w as f32 * self.scale, h as f32 * self.scale),
        };
        // No side beyond what `image::decode` makes, whatever the pixel count; a side that
        // shrinks to less than a pixel stays one, and the other is what's left of the budget.
        let side = MAX_SIDE as f32;
        let max_w = self.max_w.floor().clamp(1.0, side);
        let fit = (max_w / w)
            .min((MAX_BOX_PIXELS / (w * h)).sqrt())
            .min(side / h)
            .min(1.0);
        Some((
            (w * fit).round().clamp(1.0, max_w),
            (h * fit).round().clamp(1.0, side),
        ))
    }
}

fn fill_para(para: &mut Para, doc: &Doc, spans: &[Span], body: TextStyle, boxes: &Boxes) {
    para.clear();
    for span in spans {
        let style = span_style(span, body);
        if span.image != 0
            && let Some((w, h)) = boxes.size(span.image)
        {
            // An alt text split over two spans (a long block cut in chunks) is one box, in the
            // chunk its text starts in; the rest of it shows nothing.
            let first = doc
                .images
                .get(span.image as usize - 1)
                .is_none_or(|img| img.start == span.start);
            if first {
                let b = InlineBox {
                    width: w,
                    ascent: h,
                    descent: 0.0,
                    id: span.image,
                };
                let style = TextStyle {
                    italic: false,
                    deco: style.deco & !deco::IMAGE_BOX,
                    ..style
                };
                para.push_box(b, span.start, style);
            }
            continue;
        }
        para.push_text(doc.span_text(span), span.start, style);
    }
    if para.styles.is_empty() {
        para.styles.push(body);
    }
}

/// Copy code text with tabs expanded to the next multiple-of-4 column.
fn push_expanding_tabs(para: &mut Para, text: &str, doc: u32, style: TextStyle) {
    let mut col = para
        .text
        .rsplit('\n')
        .next()
        .map_or(0, |l| l.chars().count());
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '\t' => {
                para.push_text(&text[start..i], doc + start as u32, style);
                let n = 4 - col % 4;
                para.push_synthetic(&"    "[..n], doc + i as u32, style);
                col += n;
                start = i + 1;
            }
            '\n' => col = 0,
            _ => col += 1,
        }
    }
    para.push_text(&text[start..], doc + start as u32, style);
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
