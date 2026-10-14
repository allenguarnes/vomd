//! Block layout: Doc IR + width + scale → positioned primitives.
//!
//! Every block has a cached `y` and `h`. Heights start as estimates and become exact when a
//! block is measured: blocks near the viewport first ([`Layout::ensure`], the first-frame
//! path), the rest in time-boxed idle slices ([`Layout::idle`]). Shaped content is kept only
//! near the viewport, so memory stays flat on huge documents. Positions are relative to the
//! document area: x from its left edge, y from the document top.

use std::ops::Range;
use std::time::Instant;

use crate::doc::{self, Alert, Block, BlockKind, Doc, Marker, Span, style};
use crate::fonts::Fonts;
use crate::text::{
    Align, PBox, PGlyph, PHit, PLine, PRect, Para, ParaLayout, RectKind, TextCtx, TextStyle, deco,
};
use crate::theme::Role;

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
/// Shaped content is kept within this many viewport heights of the viewport.
const KEEP_SCREENS: f32 = 4.0;
/// Blocks within this many viewport heights are measured exactly by `ensure`.
const MEASURE_SCREENS: f32 = 2.0;

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
}

impl Default for Params {
    fn default() -> Self {
        Params {
            width: 900.0,
            scale: 1.0,
            font_size: 16.0,
            line_width: 80.0,
        }
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
}

impl BlockBox {
    /// Append a paragraph layout at (`dx`, `dy`).
    fn append(&mut self, pl: ParaLayout, para_styles: &[TextStyle], dx: f32, dy: f32) {
        let style_base = self.styles.len() as u16;
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
    pub y: f32,
    pub h: f32,
    /// `h` is exact (otherwise an estimate).
    pub measured: bool,
    /// Laid-out content, present only near the viewport.
    pub content: Option<Box<BlockBox>>,
    /// Some character in the block had no face when it was measured.
    pub missing: bool,
}

/// Precomputed per-block context that depends on neighbors.
#[derive(Clone, Copy, Debug, Default)]
struct Meta {
    /// Alert whose quote this block belongs to, and that quote's depth.
    alert: Option<(Alert, u8)>,
    /// Even data row of a table (striped background).
    stripe: bool,
    first_row: bool,
    /// Part of the footnote section at the end of the document.
    footnote: bool,
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
    pub link: u16,
}

pub struct Layout {
    params: Params,
    em: f32,
    col_x: f32,
    col_w: f32,
    geoms: Vec<BlockGeom>,
    meta: Vec<Meta>,
    /// Block range of each table.
    table_rows: Vec<Range<u32>>,
    table_cols: Vec<Option<Vec<f32>>>,
    total: f32,
    /// Next block for idle measurement.
    cursor: usize,
    /// Blocks currently holding content.
    live: Vec<u32>,
    missing: Vec<char>,
    ctx: TextCtx,
    para: Para,
}

impl Layout {
    pub fn new(doc: &Doc, params: Params) -> Layout {
        let mut layout = Layout {
            params,
            em: 0.0,
            col_x: 0.0,
            col_w: 0.0,
            geoms: vec![BlockGeom::default(); doc.blocks.len()],
            meta: Vec::new(),
            table_rows: vec![0..0; doc.tables.len()],
            table_cols: vec![None; doc.tables.len()],
            total: 0.0,
            cursor: 0,
            live: Vec::new(),
            missing: Vec::new(),
            ctx: TextCtx::default(),
            para: Para::default(),
        };
        layout.meta = layout.prepass(doc);
        layout.set_geometry();
        for i in 0..doc.blocks.len() {
            layout.geoms[i].h = layout.estimate(doc, i);
        }
        layout.recompute_ys();
        layout
    }

    pub fn params(&self) -> Params {
        self.params
    }

    pub fn total_height(&self) -> f32 {
        self.total
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

    /// Characters no loaded face covered, accumulated since the last call.
    pub fn take_missing(&mut self) -> Vec<char> {
        std::mem::take(&mut self.missing)
    }

    /// New width, scale, or font settings: everything is remeasured (old heights remain as
    /// estimates, scaled, so the scrollbar doesn't jump).
    pub fn set_params(&mut self, params: Params) {
        if params == self.params {
            return;
        }
        let old_em = self.em;
        self.params = params;
        self.set_geometry();
        let ratio = if old_em > 0.0 { self.em / old_em } else { 1.0 };
        for g in &mut self.geoms {
            g.h *= ratio;
            g.measured = false;
            g.content = None;
        }
        self.live.clear();
        self.table_cols.iter_mut().for_each(|c| *c = None);
        self.cursor = 0;
        self.recompute_ys();
    }

    /// Fallback faces were added: remeasure blocks that had uncovered characters.
    pub fn invalidate_missing(&mut self, doc: &Doc) {
        for i in 0..self.geoms.len() {
            if self.geoms[i].missing {
                let g = &mut self.geoms[i];
                g.measured = false;
                g.content = None;
                g.missing = false;
                if let BlockKind::TableRow { table, .. } = doc.blocks[i].kind {
                    self.table_cols[table as usize] = None;
                }
                self.cursor = self.cursor.min(i);
            }
        }
        self.live
            .retain(|&i| self.geoms[i as usize].content.is_some());
    }

    fn set_geometry(&mut self) {
        let p = self.params;
        self.em = p.font_size * p.scale;
        let margin = if p.width < 30.0 * self.em {
            self.em
        } else {
            2.0 * self.em
        };
        let cap = p.line_width * CHAR_EM * self.em;
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
            if let BlockKind::AlertTitle(a) = b.kind {
                alert = Some((a, b.quote));
            } else if alert.is_some_and(|(_, d)| b.quote < d) {
                alert = None;
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
                row = if header { 0 } else { row + 1 };
                meta[i].stripe = !header && row.is_multiple_of(2);
                meta[i].first_row = r.start == i as u32;
            }
            meta[i].footnote = footnotes.is_some_and(|f| i >= f);
        }
        meta
    }

    fn recompute_ys(&mut self) {
        let mut y = 0.0;
        for g in &mut self.geoms {
            g.y = y;
            y += g.h;
        }
        self.total = y;
    }

    // ---- Queries ----------------------------------------------------------------------

    /// Index of the block containing `y` (clamped to the document).
    pub fn block_at(&self, y: f32) -> usize {
        let i = self.geoms.partition_point(|g| g.y + g.h <= y);
        i.min(self.geoms.len().saturating_sub(1))
    }

    /// Blocks overlapping `[y0, y1)`.
    pub fn range(&self, y0: f32, y1: f32) -> Range<usize> {
        if self.geoms.is_empty() {
            return 0..0;
        }
        let start = self.geoms.partition_point(|g| g.y + g.h <= y0);
        let end = self.geoms.partition_point(|g| g.y < y1);
        start..end.max(start)
    }

    pub fn anchor_at(&self, y: f32) -> Anchor {
        if self.geoms.is_empty() {
            return Anchor::TOP;
        }
        let i = self.block_at(y.max(0.0));
        let g = &self.geoms[i];
        let frac = if g.h > 0.0 {
            ((y - g.y) / g.h).clamp(0.0, 1.0)
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
            Some(g) => g.y + a.frac * g.h,
            None => self.total,
        }
    }

    /// Link and document position under (`x`, `y`), document-area coordinates. Only blocks
    /// with content (near the viewport) can be hit.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let i = self.range(y, y + f32::EPSILON).start;
        let g = self.geoms.get(i)?;
        let content = g.content.as_ref()?;
        let (lx, ly) = (x - self.col_x, y - g.y);
        for line in content
            .lines
            .iter()
            .filter(|l| l.top <= ly && ly < l.bottom)
        {
            let hits = &content.hits[line.hits.start as usize..line.hits.end as usize];
            if let Some(h) = hits.iter().find(|h| h.x0 <= lx && lx < h.x1) {
                let link = content.styles[h.style as usize].link;
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
        for _ in 0..4 {
            let y = self.y_of(top);
            let r = self.range(
                y - MEASURE_SCREENS * view_h,
                y + (1.0 + MEASURE_SCREENS) * view_h,
            );
            let mut changed = false;
            for i in r {
                if self.geoms[i].content.is_some() {
                    continue;
                }
                let (content, h) = self.build(doc, fonts, i);
                let g = &mut self.geoms[i];
                changed |= !g.measured || (g.h - h).abs() > 1e-3;
                g.h = h;
                g.measured = true;
                g.content = Some(Box::new(content));
                self.live.push(i as u32);
            }
            if !changed {
                break;
            }
            self.recompute_ys();
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
        let mut any = false;
        while self.cursor < self.geoms.len() {
            let i = self.cursor;
            if !self.geoms[i].measured {
                let (_, h) = self.build(doc, fonts, i);
                self.geoms[i].h = h;
                self.geoms[i].measured = true;
                any = true;
            }
            self.cursor += 1;
            if Instant::now() >= deadline {
                break;
            }
        }
        if any {
            self.recompute_ys();
        }
        self.cursor >= self.geoms.len()
    }

    /// Lay out block `i`: its content and total height (spacing above included).
    fn build(&mut self, doc: &Doc, fonts: &Fonts, i: usize) -> (BlockBox, f32) {
        let block = &doc.blocks[i];
        let meta = self.meta[i];
        let em = self.em;
        let gap = self.gap_before(doc, i) * em;
        let indent = self.indent(block, meta);
        let avail = (self.col_w - indent).max(em);
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
                fill_para(&mut self.para, doc, doc.block_spans(block), body);
                let pl = self.ctx.layout(fonts, &self.para, avail, Align::Start, lh);
                missing.extend_from_slice(&pl.missing);
                let first_baseline = pl.lines.first().map(|l| l.baseline + gap);
                let mut h = pl.height;
                let styles = std::mem::take(&mut self.para.styles);
                out.append(pl, &styles, indent, gap);
                self.para.styles = styles;
                if let BlockKind::Heading(1 | 2) = block.kind {
                    h += 0.3 * em;
                    out.rect(indent, gap + h, avail, self.hairline(), Role::BorderMuted);
                    h += self.hairline();
                }
                if let Some(baseline) = first_baseline {
                    self.marker(fonts, block, meta, indent, baseline, &mut out, &mut missing);
                }
                h
            }
            BlockKind::Code => self.code(
                doc,
                fonts,
                block,
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
                self.para.clear();
                self.para.push_synthetic(label, 0, style);
                let pl = self
                    .ctx
                    .layout(fonts, &self.para, avail, Align::Start, BODY_LH);
                let h = pl.height;
                let styles = std::mem::take(&mut self.para.styles);
                out.append(pl, &styles, indent, gap);
                self.para.styles = styles;
                h
            }
            BlockKind::TableRow { table, .. } => self.table_row(
                doc,
                fonts,
                block,
                meta,
                table as usize,
                indent,
                gap,
                &mut out,
                &mut missing,
            ),
        };
        self.quote_bars(doc, i, meta, gap, content_h, &mut out);
        if !missing.is_empty() {
            self.geoms[i].missing = true;
            for c in missing {
                if let Err(at) = self.missing.binary_search(&c) {
                    self.missing.insert(at, c);
                }
            }
        }
        (out, gap + content_h)
    }

    /// Physical width of a 1-logical-pixel line.
    fn hairline(&self) -> f32 {
        self.params.scale.round().max(1.0)
    }

    /// Spacing above block `i`, in em.
    fn gap_before(&self, doc: &Doc, i: usize) -> f32 {
        if i == 0 {
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

    fn indent(&self, block: &Block, meta: Meta) -> f32 {
        let footnote = matches!(block.marker, Marker::Footnote(_))
            || (meta.footnote && block.list == 0 && block.kind != BlockKind::Rule);
        let lists = block.list as f32 + if footnote { 1.0 } else { 0.0 };
        (block.quote as f32 * QUOTE_INDENT + lists * LIST_INDENT) * self.em
    }

    fn body_style(&self, block: &Block, meta: Meta) -> TextStyle {
        let em = self.em;
        let mut s = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: if meta.footnote {
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

    /// Bullet, number, checkbox, or footnote number in the gutter left of `indent`, aligned to
    /// the first line's baseline.
    #[allow(clippy::too_many_arguments)]
    fn marker(
        &mut self,
        fonts: &Fonts,
        block: &Block,
        meta: Meta,
        indent: f32,
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
                let x = indent - 1.375 * em;
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
                if !checked {
                    return;
                }
                self.para.clear();
                self.para.push_synthetic(
                    "✓",
                    0,
                    TextStyle {
                        bold: true,
                        size: 0.8 * em,
                        role: Role::Bg,
                        ..body
                    },
                );
                let pl = self.ctx.layout(fonts, &self.para, size, Align::Center, 1.0);
                let dy = y + size / 2.0 - pl.height / 2.0;
                let styles = std::mem::take(&mut self.para.styles);
                out.append(pl, &styles, x, dy);
                self.para.styles = styles;
                return;
            }
        };
        let style = TextStyle {
            role: Role::Muted,
            ..body
        };
        let gutter = LIST_INDENT * em - 0.5 * em;
        self.para.clear();
        self.para.push_synthetic(&text, 0, style);
        let pl = self
            .ctx
            .layout(fonts, &self.para, gutter, Align::Right, BODY_LH);
        missing.extend_from_slice(&pl.missing);
        let dy = baseline - pl.lines.first().map_or(0.0, |l| l.baseline);
        let styles = std::mem::take(&mut self.para.styles);
        out.append(pl, &styles, indent - LIST_INDENT * em, dy);
        self.para.styles = styles;
    }

    #[allow(clippy::too_many_arguments)]
    fn code(
        &mut self,
        doc: &Doc,
        fonts: &Fonts,
        block: &Block,
        indent: f32,
        avail: f32,
        gap: f32,
        out: &mut BlockBox,
        missing: &mut Vec<char>,
    ) -> f32 {
        let em = self.em;
        let pad = em;
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
        let h = pl.height + 2.0 * pad;
        out.rects.push(PRect {
            x: indent,
            y: gap,
            w: avail,
            h,
            role: Role::CodeBg,
            radius: 6.0 * self.params.scale,
            kind: RectKind::Fill,
        });
        let conts: Vec<f32> = pl
            .lines
            .iter()
            .filter(|l| l.cont)
            .map(|l| l.baseline)
            .collect();
        let styles = std::mem::take(&mut self.para.styles);
        out.append(pl, &styles, indent + pad, gap + pad);
        self.para.styles = styles;
        // Continuation markers in the left padding for soft-wrapped lines.
        if !conts.is_empty() {
            self.para.clear();
            self.para.push_synthetic(
                "↪",
                0,
                TextStyle {
                    role: Role::Muted,
                    ..style
                },
            );
            let mark = self
                .ctx
                .layout(fonts, &self.para, pad, Align::Center, CODE_LH);
            let mark_baseline = mark.lines.first().map_or(0.0, |l| l.baseline);
            let styles = std::mem::take(&mut self.para.styles);
            for baseline in conts {
                out.append(
                    mark.clone(),
                    &styles,
                    indent,
                    gap + pad + baseline - mark_baseline,
                );
            }
            self.para.styles = styles;
        }
        h
    }

    #[allow(clippy::too_many_arguments)]
    fn table_row(
        &mut self,
        doc: &Doc,
        fonts: &Fonts,
        block: &Block,
        meta: Meta,
        table: usize,
        indent: f32,
        gap: f32,
        out: &mut BlockBox,
        missing: &mut Vec<char>,
    ) -> f32 {
        let em = self.em;
        let cols = self.table_cols(doc, fonts, table, indent);
        let (px, py) = (CELL_PAD_X * em, CELL_PAD_Y * em);
        let header = matches!(block.kind, BlockKind::TableRow { header: true, .. });
        let body = TextStyle {
            bold: header,
            ..self.body_style(block, meta)
        };
        let aligns = &doc.tables[table].aligns;
        let mut cells = Vec::with_capacity(cols.len());
        let mut row_h: f32 = 0.0;
        for (c, spans) in doc.row_cells(block).take(cols.len()).enumerate() {
            fill_para(&mut self.para, doc, spans, body);
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
            missing.extend_from_slice(&pl.missing);
            row_h = row_h.max(pl.height);
            cells.push((pl, self.para.styles.clone()));
        }
        if cells.is_empty() {
            self.para.clear();
            self.para.styles.push(body);
            let pl = self
                .ctx
                .layout(fonts, &self.para, 1.0, Align::Start, BODY_LH);
            row_h = pl.height;
        }
        let h = row_h + 2.0 * py;
        let width: f32 = cols.iter().sum();
        let line = self.hairline();
        if meta.stripe {
            out.rect(indent, gap, width, h, Role::TableStripe);
        }
        if meta.first_row {
            out.rect(indent, gap, width, line, Role::Border);
        }
        out.rect(indent, gap + h - line, width, line, Role::Border);
        let mut x = indent;
        out.rect(x, gap, line, h, Role::Border);
        for (c, w) in cols.iter().enumerate() {
            if let Some((pl, styles)) = cells.get(c).cloned() {
                out.append(pl, &styles, x + px, gap + py);
            }
            x += w;
            out.rect(x - line, gap, line, h, Role::Border);
        }
        h
    }

    /// Column widths for a table (cell padding included), from the min/natural widths of
    /// every cell: natural widths if they fit, otherwise the slack is shared in proportion
    /// to how much each column wants to grow (CSS auto-layout, simplified).
    fn table_cols(&mut self, doc: &Doc, fonts: &Fonts, table: usize, indent: f32) -> Vec<f32> {
        if let Some(cols) = &self.table_cols[table] {
            return cols.clone();
        }
        let em = self.em;
        let px = CELL_PAD_X * em;
        let n = doc.tables[table].aligns.len().max(1);
        let (mut min, mut max) = (vec![0.0f32; n], vec![0.0f32; n]);
        let rows = self.table_rows[table].clone();
        for r in rows.start..rows.end.max(rows.start) {
            let block = &doc.blocks[r as usize];
            let header = matches!(block.kind, BlockKind::TableRow { header: true, .. });
            let body = TextStyle {
                bold: header,
                ..self.body_style(block, self.meta[r as usize])
            };
            for (c, spans) in doc.row_cells(block).take(n).enumerate() {
                fill_para(&mut self.para, doc, spans, body);
                let (lo, hi) = self.ctx.measure(fonts, &self.para);
                min[c] = min[c].max(lo);
                max[c] = max[c].max(hi);
            }
        }
        let avail = (self.col_w - indent - n as f32 * 2.0 * px).max(0.0);
        let (sum_min, sum_max): (f32, f32) = (min.iter().sum(), max.iter().sum());
        let widths: Vec<f32> = if sum_max <= avail {
            max
        } else if sum_min >= avail || sum_max <= sum_min {
            min
        } else {
            let t = (avail - sum_min) / (sum_max - sum_min);
            min.iter()
                .zip(&max)
                .map(|(lo, hi)| lo + (hi - lo) * t)
                .collect()
        };
        let cols: Vec<f32> = widths.into_iter().map(|w| w.ceil() + 2.0 * px).collect();
        self.table_cols[table] = Some(cols.clone());
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
        let prev_quote = if i > 0 { doc.blocks[i - 1].quote } else { 0 };
        for q in 0..block.quote {
            let top = if prev_quote > q { 0.0 } else { gap };
            let role = match meta.alert {
                Some((a, depth)) if depth == q + 1 => alert_role(a),
                _ => Role::QuoteBar,
            };
            out.rect(
                q as f32 * QUOTE_INDENT * self.em,
                top,
                QUOTE_BAR * self.em,
                gap + content_h - top,
                role,
            );
        }
    }

    /// Height estimate for an unmeasured block.
    fn estimate(&self, doc: &Doc, i: usize) -> f32 {
        let block = &doc.blocks[i];
        let em = self.em;
        let gap = self.gap_before(doc, i) * em;
        let avail = (self.col_w - self.indent(block, self.meta[i])).max(em);
        let chars: usize = doc
            .block_spans(block)
            .iter()
            .map(|s| (s.end - s.start) as usize)
            .sum();
        let lines = |size: f32| ((chars as f32 * CHAR_EM * size) / avail).ceil().max(1.0);
        gap + match block.kind {
            BlockKind::Paragraph | BlockKind::AlertTitle(_) => lines(em) * BODY_LH * em,
            BlockKind::Heading(level) => {
                let size = HEADING_SIZES[(level.clamp(1, 6) - 1) as usize] * em;
                lines(size) * HEADING_LH * size
            }
            BlockKind::Code => {
                let text_lines = doc
                    .block_spans(block)
                    .iter()
                    .map(|s| doc.span_text(s).matches('\n').count())
                    .sum::<usize>()
                    + 1;
                (text_lines as f32).max(lines(CODE_SIZE * em)) * CODE_LH * CODE_SIZE * em + 2.0 * em
            }
            BlockKind::Rule => 0.25 * em,
            BlockKind::TableRow { .. } => BODY_LH * em + 2.0 * CELL_PAD_Y * em,
        }
    }
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
    if span.style & style::FOOTREF != 0 {
        s.size = body.size * 0.75;
        s.shift = body.size * 0.4;
    }
    s
}

fn fill_para(para: &mut Para, doc: &Doc, spans: &[Span], body: TextStyle) {
    para.clear();
    for span in spans {
        para.push_text(doc.span_text(span), span.start, span_style(span, body));
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
