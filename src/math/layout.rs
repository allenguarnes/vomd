//! Syntax tree → a formula's glyphs and rules, in em.
//!
//! TeX's math typesetting (The TeXbook, Appendix G) on the OpenType MATH table's constants, as
//! the MathML Core layout chapter says to use them: atoms with classes and the inter-atom
//! spacing table, the four styles and cramping, scripts, fractions, radicals, big operators and
//! their limits, stretched delimiters (size variants, then assemblies), accents, `\text`.
//!
//! Each node is laid out into the output lists at its own origin, and returns its box; the
//! node above moves the glyphs and rules of its children into place (a child's are a range of
//! the lists, see [`Mark`]). y points up here, with 0 at the baseline. Nothing is unbounded:
//! the tree is (see [`super::parse`]), assemblies stop at [`MAX_PARTS`] extenders, the output at
//! [`MAX_GLYPHS`], and a formula beyond [`MAX_EXTENT`] em is an error.
//!
//! Environments (`matrix`, `aligned`, …), `\tag` and `\\` at the top level are not laid out
//! yet: an environment is an error, `\tag` and `\\` are left out.

use std::collections::HashMap;

use swash::GlyphMetrics;
use swash::scale::ScaleContext;

use super::parse::{
    self, Ast, Col, Env, Error, Lap, Limits, List, MathStyle, Node, NodeId, TextFont,
};
use super::symbols::{self, AccentKind, Class, Font};
use super::table::{Assembly, Constant as K, Corner, MathTable};
use super::{MAX_EXTENT, MAX_GLYPHS, MAX_PARTS};
use crate::fonts::{FaceId, Fonts, Slot};

/// A glyph of a formula: its origin at (`x`, `y`) em from the formula's (y up), drawn at
/// `size` times the formula's size (scripts are smaller).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MGlyph {
    pub x: f32,
    pub y: f32,
    pub id: u16,
    pub face: FaceId,
    pub size: f32,
    /// Only the part of the glyph inside this is drawn: the parts of an assembly overlap, and
    /// each is cut where the next takes over, so no joint is drawn twice (which shows as a
    /// darker dot) or left open.
    pub cut: Option<Cut>,
}

/// A band a glyph is drawn in: from `lo` to `hi` em along y (`vertical`) or x, in the
/// formula's coordinates; an open end is infinite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub vertical: bool,
    pub lo: f32,
    pub hi: f32,
}

/// A filled rectangle of a formula (a fraction bar, an overline): its bottom left corner at
/// (`x`, `y`) em, y up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MRule {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A laid-out formula, in em of the math font, with its origin at the left end of its
/// baseline: `height` above it, `depth` below.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Formula {
    pub width: f32,
    pub height: f32,
    pub depth: f32,
    pub glyphs: Vec<MGlyph>,
    pub rules: Vec<MRule>,
    /// Characters no loaded face has (drawn as the math font's `.notdef`), sorted: once a
    /// fallback face is added for them, the formula is to be laid out again.
    pub missing: Vec<char>,
    /// The math axis, in em: where fractions' bars are, and what delimiters centre on.
    pub axis: f32,
    /// `\tag{…}`, laid out on its own: the block sets it flush right.
    pub tag: Option<Box<Formula>>,
    /// The atoms of the top level, left to right, for breaking a display formula too wide
    /// for its column; empty when it can't be broken (rows of its own, `\middle`).
    pub pieces: Vec<Piece>,
}

/// An atom of a formula's top level: where it is, its height and depth, where its glyphs
/// and rules start in the lists (they run to the next piece's), whether a line may start
/// with it, and how many parentheses (`Open` atoms) are open where it starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub x: f32,
    pub w: f32,
    pub h: f32,
    pub d: f32,
    pub glyphs: u32,
    pub rules: u32,
    pub breaks: Option<Break>,
    pub depth: u16,
}

/// Where a too wide display formula may break, best first: before a relation, else before a
/// binary operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Break {
    Rel,
    Bin,
}

/// What laying out formulas keeps between them: the scaler outlines are measured with, and
/// the glyphs' ink boxes. One a window (or test).
pub struct Engine {
    scale: ScaleContext,
    /// Ink boxes in em: `[x_min, y_min, x_max, y_max]`.
    ink: HashMap<(FaceId, u16), [f32; 4]>,
}

/// Ink boxes kept, at most: past this the map starts again.
const INK_CAP: usize = 8192;

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        Engine {
            scale: ScaleContext::new(),
            ink: HashMap::new(),
        }
    }

    /// Lays out `src` (`display` for display math) with the math face `math` (see
    /// [`Fonts::load_math`]). An error is the parser's, or a limit's.
    pub fn layout(
        &mut self,
        fonts: &Fonts,
        math: FaceId,
        src: &str,
        display: bool,
    ) -> Result<Formula, Error> {
        let ast = parse::parse(src, display)?;
        self.layout_ast(fonts, math, &ast, display)
            .map_err(|message| Error {
                message,
                span: 0..src.len(),
            })
    }

    /// Lays out a parsed formula. An error is a limit's, or a construct not laid out yet.
    pub fn layout_ast(
        &mut self,
        fonts: &Fonts,
        math: FaceId,
        ast: &Ast,
        display: bool,
    ) -> Result<Formula, &'static str> {
        let font = fonts.face(math).font();
        let table = font
            .table(swash::tag_from_bytes(b"MATH"))
            .and_then(MathTable::new);
        let upem = f32::from(fonts.face(math).metrics().units_per_em.max(1));
        let mut lay = Lay {
            ast,
            fonts,
            face: math,
            table,
            upem,
            metrics: font.glyph_metrics(&[]),
            eng: self,
            out: Formula::default(),
            font: Font::Normal,
            depth: 0,
            error: None,
            pieces: None,
        };
        let st = Style {
            level: if display { 0 } else { 1 },
            cramped: false,
        };
        let b = lay.top(st);
        let pieces = lay.pieces.take();
        let tag = ast.tag.map(|(id, star)| {
            let outer = std::mem::take(&mut lay.out);
            let t = lay.tag(id, star);
            let mut f = std::mem::replace(&mut lay.out, outer);
            (f.width, f.height, f.depth) = (t.w.max(0.0), t.h.max(0.0), t.d.max(0.0));
            lay.out.missing.append(&mut f.missing);
            Box::new(f)
        });
        let axis = lay.axis(st);
        let mut out = lay.out;
        out.tag = tag;
        out.axis = axis;
        out.pieces = pieces.unwrap_or_default();
        if let Some(e) = lay.error {
            return Err(e);
        }
        // Negative space can make a box narrower than nothing; a formula's is never less
        // than empty (its glyphs may stick out).
        out.width = b.w.max(0.0);
        out.height = b.h.max(0.0);
        out.depth = b.d.max(0.0);
        out.missing.sort_unstable();
        out.missing.dedup();
        let inside = |v: f32| v.is_finite() && v.abs() <= MAX_EXTENT;
        let fits = inside(out.width)
            && inside(out.height + out.depth)
            && out.glyphs.iter().all(|g| inside(g.x) && inside(g.y))
            && out
                .rules
                .iter()
                .all(|r| inside(r.x) && inside(r.y) && inside(r.x + r.w) && inside(r.y + r.h))
            && out
                .tag
                .as_ref()
                .is_none_or(|t| inside(t.width) && inside(t.height + t.depth));
        if !fits {
            return Err("formula too large");
        }
        Ok(out)
    }
}

/// A style: D, T, S or SS (`level` 0 to 3), and whether it is cramped (superscripts lower).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Style {
    level: u8,
    cramped: bool,
}

impl Style {
    fn display(self) -> bool {
        self.level == 0
    }

    fn script(self) -> bool {
        self.level >= 2
    }

    fn sup(self) -> Style {
        Style {
            level: (self.level.max(1) + 1).min(3),
            ..self
        }
    }

    fn sub(self) -> Style {
        self.sup().cramp()
    }

    fn num(self) -> Style {
        Style {
            level: (self.level + 1).min(3),
            ..self
        }
    }

    fn den(self) -> Style {
        self.num().cramp()
    }

    fn cramp(self) -> Style {
        Style {
            cramped: true,
            ..self
        }
    }

    fn with(self, s: MathStyle) -> Style {
        let level = match s {
            MathStyle::Display => 0,
            MathStyle::Text => 1,
            MathStyle::Script => 2,
            MathStyle::ScriptScript => 3,
        };
        Style { level, ..self }
    }
}

/// A laid-out piece: its advance, height and depth (em, from its origin), and what scripts
/// and accents need to know of a single glyph.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Bx {
    w: f32,
    h: f32,
    d: f32,
    /// Italics correction: how far the top leans out past the advance.
    ital: f32,
    /// Where an accent goes over it, from its origin.
    accent: Option<f32>,
    /// It is one glyph of the math face.
    glyph: Option<u16>,
}

/// Where a piece's glyphs and rules start in the output lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mark(usize, usize);

/// TeX's inter-atom spacing (The TeXbook, chapter 18), rows the left atom's class and columns
/// the right's, in the order of [`Class`]: 1 thin, 2 medium, 3 thick, +4 only in display and
/// text styles. Bin next to Bin and the like can't happen (Bin becomes Ord), so is 0.
const SPACING: [[u8; 8]; 8] = [
    [0, 1, 6, 7, 0, 0, 0, 5],
    [1, 1, 0, 7, 0, 0, 0, 5],
    [6, 6, 0, 0, 6, 0, 0, 6],
    [7, 7, 0, 0, 7, 0, 0, 7],
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 6, 7, 0, 0, 0, 5],
    [5, 5, 0, 5, 5, 5, 5, 5],
    [5, 1, 6, 7, 5, 0, 5, 5],
];

/// The null delimiter's width, `\nulldelimiterspace` (1.2 pt at 10 pt), around fractions and
/// in place of `\left.`.
const NULL_DELIMITER: f32 = 0.12;

struct Lay<'a> {
    ast: &'a Ast,
    fonts: &'a Fonts,
    face: FaceId,
    table: Option<MathTable<'a>>,
    upem: f32,
    metrics: GlyphMetrics<'a>,
    eng: &'a mut Engine,
    out: Formula,
    /// The alphabet of `\mathbf{…}` and the others.
    font: Font,
    depth: usize,
    error: Option<&'static str>,
    /// The top level's pieces, while its row is laid out (the row takes them, so the rows
    /// inside it don't).
    pieces: Option<Vec<Piece>>,
}

/// How an environment sets its cells: their style (the smallest level they take), the
/// space between columns and outside them, the strut every row has, the extra space between
/// rows, and the delimiters around it.
#[derive(Clone, Copy)]
struct Grid {
    level: u8,
    colsep: f32,
    outer: f32,
    strut: (f32, f32),
    jot: f32,
    delims: (Option<char>, Option<char>),
}

/// A row of a grid: its cells, the space after it (`\\[…]`), and the rules above it.
struct GridRow<'a> {
    cells: Vec<&'a [NodeId]>,
    gap: f32,
    lines: Vec<bool>,
}

/// A column of a grid: how its cells align, and whether a cell starts as if after an atom
/// (the right-hand column of `aligned`, so `&=` is spaced as a relation).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// LaTeX's array strut at `\arraystretch` 1: 0.7 and 0.3 of a 1.2 em baseline skip.
const STRUT: (f32, f32) = (0.84, 0.36);
/// `\jot`: the extra space between the rows of `aligned` and the like.
const JOT: f32 = 0.25;
/// The thickness of `\hline` and of an array's `|`, and the space between two of them.
const ARRAY_RULE: f32 = 0.05;
const DOUBLE_RULE_SEP: f32 = 0.2;
/// A dashed rule's dashes, and the gaps between them.
const DASH: (f32, f32) = (0.4, 0.2);

impl<'a> Lay<'a> {
    fn mark(&self) -> Mark {
        Mark(self.out.glyphs.len(), self.out.rules.len())
    }

    /// Moves what was laid out from `from` to `to`.
    fn shift(&mut self, from: Mark, to: Mark, dx: f32, dy: f32) {
        for g in self.out.glyphs.get_mut(from.0..to.0).unwrap_or(&mut []) {
            g.x += dx;
            g.y += dy;
            if let Some(c) = &mut g.cut {
                let d = if c.vertical { dy } else { dx };
                c.lo += d;
                c.hi += d;
            }
        }
        for r in self.out.rules.get_mut(from.1..to.1).unwrap_or(&mut []) {
            r.x += dx;
            r.y += dy;
        }
    }

    /// Moves what was laid out since `from`.
    fn shift_since(&mut self, from: Mark, dx: f32, dy: f32) {
        let to = self.mark();
        self.shift(from, to, dx, dy);
    }

    fn fail(&mut self, message: &'static str) {
        self.error.get_or_insert(message);
    }

    fn room(&mut self) -> bool {
        let ok = self.out.glyphs.len() + self.out.rules.len() < MAX_GLYPHS;
        if !ok {
            self.fail("formula too big");
        }
        ok
    }

    fn rule(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if self.room() && w > 0.0 && h > 0.0 {
            self.out.rules.push(MRule { x, y, w, h });
        }
    }

    fn scale(&self, st: Style) -> f32 {
        let (c, default) = match st.level {
            0 | 1 => return 1.0,
            2 => (K::ScriptPercentScaleDown, 70),
            _ => (K::ScriptScriptPercentScaleDown, 50),
        };
        let pct = self
            .table
            .and_then(|t| t.constant(c))
            .filter(|p| (10..=100).contains(p))
            .unwrap_or(default);
        pct as f32 / 100.0
    }

    /// A MATH constant at `st`'s size, in em.
    fn k(&self, c: K, st: Style) -> f32 {
        let v = self.table.and_then(|t| t.constant(c)).unwrap_or(0);
        v as f32 / self.upem * self.scale(st)
    }

    fn axis(&self, st: Style) -> f32 {
        self.k(K::AxisHeight, st)
    }

    /// `ch`'s face and glyph: `preferred`, else the math face, the body text's, or a fallback
    /// face. A character none has is noted and drawn as `.notdef`.
    fn find(&mut self, ch: char, preferred: FaceId) -> (FaceId, u16) {
        let regular = self.fonts.slot(Slot::Regular);
        for f in [preferred, self.face, regular] {
            let g = self.fonts.face(f).glyph(ch);
            if g != 0 {
                return (f, g);
            }
        }
        if let Some(f) = self.fonts.select(regular, ch) {
            return (f, self.fonts.face(f).glyph(ch));
        }
        if self.out.missing.len() < 64 && !ch.is_whitespace() {
            self.out.missing.push(ch);
        }
        (self.face, 0)
    }

    /// A glyph's ink box in em of its face, `[x_min, y_min, x_max, y_max]`.
    fn ink(&mut self, face: FaceId, id: u16) -> [f32; 4] {
        if let Some(b) = self.eng.ink.get(&(face, id)) {
            return *b;
        }
        let f = self.fonts.face(face);
        let upem = f32::from(f.metrics().units_per_em.max(1));
        let mut scaler = self.eng.scale.builder(f.font()).hint(false).build();
        let b = scaler
            .scale_outline(id)
            .map(|o| o.bounds())
            .map(|b| [b.min.x, b.min.y, b.max.x, b.max.y].map(|v| v / upem))
            .filter(|b| b.iter().all(|v| v.is_finite()) && b[0] <= b[2])
            .unwrap_or([0.0; 4]);
        if self.eng.ink.len() >= INK_CAP {
            self.eng.ink.clear();
        }
        self.eng.ink.insert((face, id), b);
        b
    }

    fn advance(&self, face: FaceId, id: u16) -> f32 {
        if face == self.face {
            return self.metrics.advance_width(id) / self.upem;
        }
        let f = self.fonts.face(face);
        let upem = f32::from(f.metrics().units_per_em.max(1));
        f.font().glyph_metrics(&[]).advance_width(id) / upem
    }

    /// A glyph at the origin, at `size`.
    fn glyph(&mut self, face: FaceId, id: u16, size: f32) -> Bx {
        let ink = self.ink(face, id);
        if self.room() {
            self.out.glyphs.push(MGlyph {
                x: 0.0,
                y: 0.0,
                id,
                face,
                size,
                cut: None,
            });
        }
        let math = face == self.face;
        let table = self.table.filter(|_| math);
        let ital = table.and_then(|t| t.italics_correction(id)).unwrap_or(0);
        let accent = table.and_then(|t| t.top_accent_attachment(id));
        Bx {
            w: self.advance(face, id) * size,
            h: ink[3] * size,
            d: -ink[1] * size,
            ital: f32::from(ital) / self.upem * size,
            accent: accent.map(|a| f32::from(a) / self.upem * size),
            glyph: math.then_some(id),
        }
    }

    /// A character of a formula, in the current alphabet.
    fn char(&mut self, ch: char, st: Style) -> Bx {
        let size = self.scale(st);
        let preferred = match self.font {
            Font::Sans => self.fonts.slot(Slot::Regular),
            Font::Mono => self.fonts.slot(Slot::Mono),
            _ => self.face,
        };
        let (face, id) = self.find(symbols::styled(ch, self.font), preferred);
        self.glyph(face, id, size)
    }

    /// The list `list`: its atoms side by side, spaced by their classes.
    fn list(&mut self, list: List, st: Style) -> Bx {
        let ast = self.ast;
        let ids = ast.list(list);
        self.row(ids, st, false)
    }

    /// The formula's top level: one row, whose pieces are kept for breaking it, or rows
    /// split by `\\`, centred one over the other.
    fn top(&mut self, st: Style) -> Bx {
        let ast = self.ast;
        let ids = ast.list(ast.root);
        if !ids
            .iter()
            .any(|&id| matches!(ast.node(id), Some(Node::Newline)))
        {
            self.pieces = Some(Vec::new());
            return self.row(ids, st, false);
        }
        let rows = ids
            .split(|&id| matches!(ast.node(id), Some(Node::Newline)))
            .map(|cells| GridRow {
                cells: vec![cells],
                gap: 0.0,
                lines: Vec::new(),
            })
            .collect();
        let grid = Grid {
            level: st.level,
            colsep: 0.0,
            outer: 0.0,
            strut: STRUT,
            jot: JOT,
            delims: (None, None),
        };
        self.grid(rows, Vec::new(), &grid, &[], st)
    }

    /// `\tag{…}` at the origin, in parentheses unless it is `\tag*`.
    fn tag(&mut self, id: NodeId, star: bool) -> Bx {
        let st = Style {
            level: 1,
            cramped: false,
        };
        let mut x = 0.0;
        let (mut h, mut d) = (0f32, 0f32);
        let mut put = |this: &mut Self, b: Bx, from: Mark| {
            this.shift_since(from, x, 0.0);
            x += b.w;
            h = h.max(b.h);
            d = d.max(b.d);
        };
        let paren = |this: &mut Self, ch: char| {
            let (face, id) = this.find(ch, this.face);
            this.glyph(face, id, 1.0)
        };
        if !star {
            let from = self.mark();
            let b = paren(self, '(');
            put(self, b, from);
        }
        let from = self.mark();
        let b = self.atom(id, st).1;
        put(self, b, from);
        if !star {
            let from = self.mark();
            let b = paren(self, ')');
            put(self, b, from);
        }
        Bx {
            w: x,
            h,
            d,
            ..Bx::default()
        }
    }

    /// `ids` side by side, spaced by their classes; `lead`: as if after an ord atom (`{}`).
    fn row(&mut self, ids: &[NodeId], mut st: Style, lead: bool) -> Bx {
        let record = self.pieces.take();
        struct Item {
            class: Option<Class>,
            b: Bx,
            from: Mark,
            to: Mark,
            middle: Option<Option<char>>,
            st: Style,
        }
        let mut items: Vec<Item> = Vec::with_capacity(ids.len());
        // `\rm` and the other switches last to the end of the row (the group).
        let outer_font = self.font;
        for &id in ids {
            let from = self.mark();
            let (class, b, middle) = match self.ast.node(id) {
                Some(Node::Style(s)) => {
                    st = st.with(*s);
                    continue;
                }
                Some(Node::FontSwitch(f)) => {
                    self.font = *f;
                    continue;
                }
                Some(Node::Middle(d)) => (Some(Class::Ord), Bx::default(), Some(*d)),
                _ => {
                    let (class, b) = self.atom(id, st);
                    (class, b, None)
                }
            };
            items.push(Item {
                class,
                b,
                from,
                to: self.mark(),
                middle,
                st,
            });
        }
        self.font = outer_font;
        // `\middle`: as tall as the rest.
        if items.iter().any(|i| i.middle.is_some()) {
            let (h, d) = items
                .iter()
                .fold((0f32, 0f32), |(h, d), i| (h.max(i.b.h), d.max(i.b.d)));
            for i in items.iter_mut() {
                if let Some(delim) = i.middle {
                    i.from = self.mark();
                    let size = self.delimiter_size(h, d, i.st);
                    i.b = self.delimiter(delim, size, i.st);
                    i.to = self.mark();
                }
            }
        }
        // A Bin with nothing to operate on is an Ord (rules 5 and 6 of Appendix G).
        let lead = lead.then_some(Class::Ord);
        let mut prev: Option<usize> = None;
        for i in 0..items.len() {
            let Some(class) = items[i].class else {
                continue;
            };
            let before = match prev {
                Some(p) => items[p].class,
                None => lead,
            };
            if class == Class::Bin
                && matches!(
                    before,
                    None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct)
                )
            {
                items[i].class = Some(Class::Ord);
            } else if matches!(class, Class::Rel | Class::Close | Class::Punct)
                && before == Some(Class::Bin)
                && let Some(p) = prev
            {
                items[p].class = Some(Class::Ord);
            }
            prev = Some(i);
        }
        if let Some(p) = prev
            && items[p].class == Some(Class::Bin)
        {
            items[p].class = Some(Class::Ord);
        }
        let (mut x, mut h, mut d) = (0f32, 0f32, 0f32);
        let mut last: Option<Class> = lead;
        // The pieces, when this is the top level (a `\middle` was laid out out of order).
        let mut pieces = record.filter(|_| !items.iter().any(|i| i.middle.is_some()));
        let mut fences = 0u16;
        for item in &items {
            if let Some(class) = item.class {
                if let Some(l) = last {
                    let s = SPACING[l as usize][class as usize];
                    if s & 4 == 0 || !item.st.script() {
                        let mu = [0.0, 3.0, 4.0, 5.0][usize::from(s & 3)];
                        x += mu / 18.0 * self.scale(item.st);
                    }
                }
                last = Some(class);
            }
            self.shift(item.from, item.to, x, 0.0);
            if let Some(pieces) = &mut pieces {
                pieces.push(Piece {
                    x,
                    w: item.b.w,
                    h: item.b.h,
                    d: item.b.d,
                    glyphs: item.from.0 as u32,
                    rules: item.from.1 as u32,
                    breaks: match item.class {
                        Some(Class::Rel) => Some(Break::Rel),
                        Some(Class::Bin) => Some(Break::Bin),
                        _ => None,
                    },
                    depth: fences,
                });
            }
            match item.class {
                Some(Class::Open) => fences = fences.saturating_add(1),
                Some(Class::Close) => fences = fences.saturating_sub(1),
                _ => {}
            }
            x += item.b.w;
            h = h.max(item.b.h);
            d = d.max(item.b.d);
        }
        self.pieces = pieces;
        match items.as_slice() {
            [one] => Bx {
                w: x,
                h,
                d,
                ..one.b
            },
            _ => Bx {
                w: x,
                h,
                d,
                ..Bx::default()
            },
        }
    }

    /// An atom: its class (none for space) and box.
    fn atom(&mut self, id: NodeId, st: Style) -> (Option<Class>, Bx) {
        self.depth += 1;
        let r = if self.depth > 4 * super::MAX_DEPTH {
            self.fail("formula nests too deep");
            (None, Bx::default())
        } else {
            self.node(id, st)
        };
        self.depth -= 1;
        r
    }

    fn node(&mut self, id: NodeId, st: Style) -> (Option<Class>, Bx) {
        let Some(&node) = self.ast.node(id) else {
            return (None, Bx::default());
        };
        let ord = Some(Class::Ord);
        match node {
            Node::Char { ch, class } => (Some(class), self.char(ch, st)),
            Node::Group(list) => (ord, self.list(list, st)),
            Node::Scripts {
                base,
                sup,
                sub,
                primes,
            } => self.scripts(base, (sup, sub, primes), st),
            Node::Op { body, .. } => (Some(Class::Op), self.op(body, st)),
            Node::Frac {
                num,
                den,
                bar,
                thick,
                style,
                left,
                right,
                cfrac,
            } => (
                Some(Class::Inner),
                self.frac(num, den, (bar, thick), style, (left, right), cfrac, st),
            ),
            Node::Sqrt { index, body } => (ord, self.sqrt(index, body, st)),
            Node::Delimited { left, right, body } => {
                (Some(Class::Inner), self.delimited(left, right, body, st))
            }
            Node::Middle(d) => (ord, self.delimiter(d, 0.0, st)),
            Node::Big { delim, size, class } => {
                let em = [1.2, 1.8, 2.4, 3.0][usize::from(size.clamp(1, 4) - 1)];
                (Some(class), self.delimiter(delim, em * self.scale(st), st))
            }
            Node::Accent { ch, kind, body } => (ord, self.accent(ch, kind, body, st)),
            Node::Overline(body) => (ord, self.overline(body, st)),
            Node::Underline(body) => (ord, self.underline(body, st)),
            Node::Brace { body, under } => (ord, self.brace(body, under, st)),
            Node::Stack {
                body,
                over,
                under,
                class,
            } => {
                let from = self.mark();
                let (_, b) = self.atom(body, st);
                (Some(class), self.limits(from, b, over, under, st))
            }
            Node::XArrow { ch, over, under } => {
                (Some(Class::Rel), self.xarrow(ch, over, under, st))
            }
            Node::Font { font, body } => {
                let saved = std::mem::replace(&mut self.font, font);
                let (class, b) = self.atom(body, st);
                self.font = saved;
                // `\mathbf{+}` keeps its class; a longer argument is an ord.
                let class = match self.ast.node(body) {
                    Some(Node::Group(l)) if l.end - l.start == 1 => self
                        .ast
                        .list(*l)
                        .first()
                        .and_then(|&c| match self.ast.node(c) {
                            Some(Node::Char { class, .. }) => Some(*class),
                            _ => None,
                        })
                        .or(class),
                    _ => class,
                };
                (class, b)
            }
            Node::Text(list) => (ord, self.text(list, st)),
            Node::TextRun { start, end, font } => (ord, self.text_run(start, end, font, st)),
            Node::Space { amount, mu } => {
                let em = if mu { amount / 18.0 } else { amount };
                let b = Bx {
                    w: em * self.scale(st),
                    ..Bx::default()
                };
                (None, b)
            }
            Node::Style(_)
            | Node::FontSwitch(_)
            | Node::Newline
            | Node::Cell(_)
            | Node::RowEnd { .. } => (None, Bx::default()),
            Node::Lap { body, side } => {
                let from = self.mark();
                let (_, b) = self.atom(body, st);
                let dx = match side {
                    Lap::Left => -b.w,
                    Lap::Right => 0.0,
                    Lap::Centre => -b.w / 2.0,
                };
                self.shift_since(from, dx, 0.0);
                let b = Bx {
                    w: 0.0,
                    h: b.h,
                    d: b.d,
                    ..Bx::default()
                };
                (ord, b)
            }
            Node::HLine { .. } => (None, Bx::default()),
            Node::Class { class, body } => (Some(class), self.atom(body, st).1),
            Node::Phantom {
                body,
                width,
                height,
            } => {
                let from = self.mark();
                let (_, b) = self.atom(body, st);
                self.out.glyphs.truncate(from.0);
                self.out.rules.truncate(from.1);
                let b = Bx {
                    w: if width { b.w } else { 0.0 },
                    h: if height { b.h } else { 0.0 },
                    d: if height { b.d } else { 0.0 },
                    ..Bx::default()
                };
                (ord, b)
            }
            Node::Smash { body, top, bottom } => {
                let (_, b) = self.atom(body, st);
                let b = Bx {
                    h: if top { 0.0 } else { b.h },
                    d: if bottom { 0.0 } else { b.d },
                    ..b
                };
                (ord, b)
            }
            Node::Boxed(body) => (ord, self.boxed(body, st)),
            Node::Rule {
                width,
                height,
                raise,
            } => {
                let s = self.scale(st);
                let (w, h, r) = (width * s, height * s, raise * s);
                self.rule(0.0, r, w, h);
                let b = Bx {
                    w: w.max(0.0),
                    h: (r + h).max(0.0),
                    d: (-r).max(0.0),
                    ..Bx::default()
                };
                (ord, b)
            }
            Node::Not(body) => {
                let (class, b) = self.atom(body, st);
                let (face, id) = self.find('\u{338}', self.face);
                let slash = self.mark();
                let size = self.scale(st);
                self.glyph(face, id, size);
                let ink = self.ink(face, id);
                let center = (ink[0] + ink[2]) / 2.0 * size;
                self.shift_since(slash, b.w / 2.0 - center, 0.0);
                let class = match class {
                    Some(Class::Ord) | None => Some(Class::Rel),
                    c => c,
                };
                (class, Bx { glyph: None, ..b })
            }
            Node::Array {
                env,
                cols,
                cells,
                pairs,
            } => self.array(env, cols, cells, pairs, st),
        }
    }

    /// An environment: its rows and columns as the environment sets them (see [`Grid`]),
    /// in its delimiters.
    fn array(
        &mut self,
        env: Env,
        cols: List,
        cells: List,
        pairs: u16,
        st: Style,
    ) -> (Option<Class>, Bx) {
        let ast = self.ast;
        let mut rows: Vec<GridRow<'a>> = Vec::new();
        let mut row = GridRow {
            cells: Vec::new(),
            gap: 0.0,
            lines: Vec::new(),
        };
        for &id in ast.list(cells) {
            match ast.node(id) {
                Some(Node::Cell(list)) => row.cells.push(ast.list(*list)),
                Some(Node::RowEnd { gap }) => {
                    row.gap = *gap;
                    let next = GridRow {
                        cells: Vec::new(),
                        gap: 0.0,
                        lines: Vec::new(),
                    };
                    rows.push(std::mem::replace(&mut row, next));
                }
                Some(Node::HLine { dashed }) => row.lines.push(*dashed),
                _ => {}
            }
        }
        // Rules after the last `\\` belong under the last row.
        let bottom = if row.cells.is_empty() {
            std::mem::take(&mut row.lines)
        } else {
            rows.push(row);
            Vec::new()
        };
        let text = Grid {
            level: 1,
            colsep: 1.0,
            outer: 0.0,
            strut: STRUT,
            jot: 0.0,
            delims: (None, None),
        };
        let small = Grid {
            level: 2,
            colsep: 0.5556,
            strut: (STRUT.0 / 2.0, STRUT.1 / 2.0),
            ..text
        };
        let display = Grid {
            level: 0,
            colsep: 0.0,
            jot: JOT,
            ..text
        };
        let cases = Grid {
            strut: (STRUT.0 * 1.2, STRUT.1 * 1.2),
            delims: (Some('{'), None),
            ..text
        };
        let n = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        let mut aligns = vec![Align::Center; n];
        let mut lead = vec![false; n];
        // Extra space before each column (`aligned`'s pairs).
        let mut gaps = vec![0.0f32; n];
        let aligned = |aligns: &mut [Align], lead: &mut [bool], gaps: &mut [f32], gap: f32| {
            for c in 0..aligns.len() {
                aligns[c] = if c % 2 == 0 {
                    Align::Right
                } else {
                    Align::Left
                };
                lead[c] = c % 2 == 1;
                if c > 0 && c % 2 == 0 {
                    gaps[c] = gap;
                }
            }
        };
        let mut grid = match env {
            Env::Matrix => text,
            Env::PMatrix => Grid {
                delims: (Some('('), Some(')')),
                ..text
            },
            Env::BMatrix => Grid {
                delims: (Some('['), Some(']')),
                ..text
            },
            Env::BraceMatrix => Grid {
                delims: (Some('{'), Some('}')),
                ..text
            },
            Env::VMatrix => Grid {
                delims: (Some('|'), Some('|')),
                ..text
            },
            Env::DoubleVMatrix => Grid {
                delims: (Some('‖'), Some('‖')),
                ..text
            },
            Env::SmallMatrix => small,
            Env::Cases | Env::DCases | Env::RCases => {
                aligns.fill(Align::Left);
                let level = if env == Env::DCases { 0 } else { 1 };
                let delims = if env == Env::RCases {
                    (None, Some('}'))
                } else {
                    (Some('{'), None)
                };
                Grid {
                    level,
                    delims,
                    ..cases
                }
            }
            Env::Aligned | Env::Split | Env::Align => {
                aligned(&mut aligns, &mut lead, &mut gaps, 1.0);
                display
            }
            Env::AlignedAt => {
                aligned(&mut aligns, &mut lead, &mut gaps, 0.0);
                let _ = pairs;
                display
            }
            Env::Gathered | Env::Gather | Env::Equation => display,
            Env::Eqnarray => {
                for (c, a) in aligns.iter_mut().enumerate() {
                    *a = [Align::Right, Align::Center, Align::Left][c % 3];
                }
                Grid {
                    colsep: 1.0,
                    ..display
                }
            }
            Env::Array => Grid {
                colsep: 1.0,
                outer: 0.5,
                ..text
            },
            Env::Subarray => Grid {
                colsep: 0.0,
                ..small
            },
            Env::Substack => Grid {
                colsep: 0.0,
                ..small
            },
        };
        // An `array`'s columns come from its spec: alignments, and rules between them.
        let mut rules: Vec<(usize, bool)> = Vec::new();
        if matches!(env, Env::Array | Env::Subarray) {
            let mut c = 0;
            for &col in ast
                .cols
                .get(cols.start as usize..cols.end as usize)
                .unwrap_or(&[])
            {
                match col {
                    Col::Rule { dashed } => rules.push((c, dashed)),
                    _ => {
                        if let Some(a) = aligns.get_mut(c) {
                            *a = match col {
                                Col::Left => Align::Left,
                                Col::Right => Align::Right,
                                _ => Align::Center,
                            };
                        }
                        c += 1;
                    }
                }
            }
        }
        // `matrix*`'s one alignment for every column.
        if !matches!(env, Env::Array | Env::Subarray)
            && let Some(&col) = ast
                .cols
                .get(cols.start as usize..cols.end as usize)
                .and_then(|c| c.first())
        {
            let a = match col {
                Col::Left => Align::Left,
                Col::Right => Align::Right,
                _ => Align::Center,
            };
            aligns.fill(a);
        }
        if env == Env::Subarray {
            grid.outer = 0.0;
        }
        let spec: Vec<(Align, bool, f32)> = (0..n).map(|c| (aligns[c], lead[c], gaps[c])).collect();
        let b = self.grid_with_rules(rows, bottom, &grid, &spec, &rules, st);
        let class = if grid.delims == (None, None) {
            Class::Ord
        } else {
            Class::Inner
        };
        (Some(class), b)
    }

    /// Rows of cells in columns (`spec`: each column's alignment, lead and extra space before
    /// it), the whole centred on the axis and put in the grid's delimiters.
    fn grid(
        &mut self,
        rows: Vec<GridRow<'a>>,
        bottom: Vec<bool>,
        grid: &Grid,
        spec: &[(Align, bool, f32)],
        st: Style,
    ) -> Bx {
        self.grid_with_rules(rows, bottom, grid, spec, &[], st)
    }

    /// [`Lay::grid`], with vertical rules before columns (`rules`: the column, dashed).
    fn grid_with_rules(
        &mut self,
        rows: Vec<GridRow<'a>>,
        bottom: Vec<bool>,
        grid: &Grid,
        spec: &[(Align, bool, f32)],
        rules: &[(usize, bool)],
        st: Style,
    ) -> Bx {
        let s = self.scale(st);
        let cell_st = Style {
            level: st.level.max(grid.level),
            cramped: false,
        };
        let start = self.mark();
        // Every cell laid out at the origin: its box, and where its glyphs and rules are.
        let mut laid: Vec<Vec<(Bx, Mark, Mark)>> = Vec::with_capacity(rows.len());
        let mut widths: Vec<f32> = Vec::new();
        for row in &rows {
            let mut cells = Vec::with_capacity(row.cells.len());
            for (c, ids) in row.cells.iter().enumerate() {
                let lead = spec.get(c).is_some_and(|&(_, lead, _)| lead);
                let from = self.mark();
                self.depth += 1;
                let b = if self.depth > 4 * super::MAX_DEPTH {
                    self.fail("formula nests too deep");
                    Bx::default()
                } else {
                    self.row(ids, cell_st, lead)
                };
                self.depth -= 1;
                cells.push((b, from, self.mark()));
                if widths.len() <= c {
                    widths.resize(c + 1, 0.0);
                }
                widths[c] = widths[c].max(b.w);
            }
            laid.push(cells);
        }
        // Columns left to right: rules, space, the cells, space.
        let rule_t = ARRAY_RULE * s;
        let mut xs = Vec::with_capacity(widths.len());
        let mut vrules: Vec<(f32, bool)> = Vec::new();
        let mut x = 0.0;
        for c in 0..=widths.len() {
            let mut first = true;
            for &(_, dashed) in rules.iter().filter(|r| r.0 == c) {
                if !first {
                    x += DOUBLE_RULE_SEP * s;
                }
                vrules.push((x, dashed));
                x += rule_t;
                first = false;
            }
            let Some(&w) = widths.get(c) else {
                break;
            };
            let gap = spec.get(c).map_or(0.0, |&(_, _, g)| g);
            x += if c == 0 {
                grid.outer
            } else {
                grid.colsep / 2.0 + gap
            } * s;
            xs.push(x);
            x += w;
            x += if c + 1 == widths.len() {
                grid.outer
            } else {
                grid.colsep / 2.0
            } * s;
        }
        let width = x;
        // Rows top to bottom: the first baseline at 0, each next one below the last's depth,
        // its gap and its rules.
        let (sh, sd) = (grid.strut.0 * s, grid.strut.1 * s);
        let mut top = 0.0;
        let mut hlines: Vec<(f32, bool)> = Vec::new();
        let mut below = 0.0;
        for (r, (row, cells)) in rows.iter().zip(&laid).enumerate() {
            let h = cells.iter().fold(sh, |h, c| h.max(c.0.h));
            let d = cells.iter().fold(sd, |d, c| d.max(c.0.d));
            let lines = row.lines.len() as f32;
            let rule_room = if lines > 0.0 {
                lines * rule_t + (lines - 1.0) * DOUBLE_RULE_SEP * s
            } else {
                0.0
            };
            let y = if r == 0 {
                top = h + rule_room;
                0.0
            } else {
                below - grid.jot * s - rule_room - h
            };
            let mut ly = y + h + rule_room;
            for &dashed in &row.lines {
                hlines.push((ly - rule_t, dashed));
                ly -= rule_t + DOUBLE_RULE_SEP * s;
            }
            for (c, &(b, from, to)) in cells.iter().enumerate() {
                let (w, cx) = (
                    widths.get(c).copied().unwrap_or(0.0),
                    xs.get(c).copied().unwrap_or(0.0),
                );
                let dx = match spec.get(c).map_or(Align::Center, |a| a.0) {
                    Align::Left => 0.0,
                    Align::Center => (w - b.w) / 2.0,
                    Align::Right => w - b.w,
                };
                self.shift(from, to, cx + dx, y);
            }
            below = y - d - row.gap.max(-(h + d)) * s;
        }
        let mut ly = below;
        for &dashed in &bottom {
            hlines.push((ly, dashed));
            ly -= rule_t + DOUBLE_RULE_SEP * s;
        }
        let bot = ly.min(below);
        for (y, dashed) in hlines {
            self.dashed(0.0, y, width, rule_t, dashed, false);
        }
        for (x, dashed) in vrules {
            self.dashed(x, bot, rule_t, top - bot, dashed, true);
        }
        // Centred on the axis.
        let dy = self.axis(st) - (top + bot) / 2.0;
        self.shift_since(start, 0.0, dy);
        let b = Bx {
            w: width,
            h: top + dy,
            d: -(bot + dy),
            ..Bx::default()
        };
        let (left, right) = grid.delims;
        if left.is_none() && right.is_none() {
            return b;
        }
        let body_end = self.mark();
        let size = self.delimiter_size(b.h, b.d, st);
        let lb = self.delimiter(left, size, st);
        let l_end = self.mark();
        self.shift(start, body_end, lb.w, 0.0);
        let rb = self.delimiter(right, size, st);
        self.shift_since(l_end, lb.w + b.w, 0.0);
        Bx {
            w: lb.w + b.w + rb.w,
            h: b.h.max(lb.h).max(rb.h),
            d: b.d.max(lb.d).max(rb.d),
            ..Bx::default()
        }
    }

    /// A rule from (`x`, `y`), `w` by `h`, solid or in dashes along y (`vertical`) or x.
    fn dashed(&mut self, x: f32, y: f32, w: f32, h: f32, dashed: bool, vertical: bool) {
        if !dashed {
            self.rule(x, y, w, h);
            return;
        }
        let len = if vertical { h } else { w };
        // Dashes scale with the rule, which is `ARRAY_RULE` em of the style.
        let s = if vertical { w } else { h } / ARRAY_RULE;
        let (dash, gap) = (DASH.0 * s, DASH.1 * s);
        if dash + gap <= 1e-6 || !(dash + gap).is_finite() || !len.is_finite() {
            return;
        }
        let mut at = 0.0;
        while at < len && self.room() {
            let piece = dash.min(len - at);
            if vertical {
                self.rule(x, y + at, w, piece);
            } else {
                self.rule(x + at, y, piece, h);
            }
            at += dash + gap;
        }
    }

    /// A big operator (in display style, its display size), an operator name, a brace or
    /// `\mathop{…}`.
    fn op(&mut self, body: NodeId, st: Style) -> Bx {
        let Some(&node) = self.ast.node(body) else {
            return Bx::default();
        };
        let Node::Char { ch, .. } = node else {
            return self.atom(body, st).1;
        };
        let size = self.scale(st);
        let (face, mut id) = self.find(ch, self.face);
        if st.display()
            && face == self.face
            && let Some(c) = self.table.and_then(|t| t.construction(id, true))
        {
            // At least the font's minimum, and √2 times the operator's own height (as Gecko
            // has it): some fonts' minimum is below their text-size `∫`.
            let own = self.ink(face, id);
            let min = self
                .table
                .and_then(|t| t.constant(K::DisplayOperatorMinHeight))
                .unwrap_or(0) as f32;
            let min = min.max((own[3] - own[1]) * self.upem * std::f32::consts::SQRT_2);
            for v in c.variants() {
                id = v.glyph;
                if f32::from(v.advance) >= min {
                    break;
                }
            }
        }
        let from = self.mark();
        let b = self.glyph(face, id, size);
        // Centred on the axis.
        let dy = self.axis(st) - (b.h - b.d) / 2.0;
        self.shift_since(from, 0.0, dy);
        Bx {
            h: b.h + dy,
            d: b.d - dy,
            accent: None,
            ..b
        }
    }

    fn scripts(
        &mut self,
        base: Option<NodeId>,
        (sup, sub, primes): (Option<NodeId>, Option<NodeId>, u16),
        st: Style,
    ) -> (Option<Class>, Bx) {
        let base_node = base.and_then(|b| self.ast.node(b)).copied();
        let from = self.mark();
        let (class, b, big_op) = match base_node {
            Some(Node::Op { body, limits }) => {
                let b = self.op(body, st);
                let limits = match limits {
                    Limits::Always => true,
                    Limits::Never => false,
                    Limits::Display => st.display(),
                };
                if limits && primes == 0 {
                    return (Some(Class::Op), self.limits(from, b, sup, sub, st));
                }
                let big = matches!(self.ast.node(body), Some(Node::Char { .. }));
                (Some(Class::Op), b, big)
            }
            Some(_) => {
                let (class, b) = base.map_or((None, Bx::default()), |id| self.atom(id, st));
                (class.or(Some(Class::Ord)), b, false)
            }
            None => (Some(Class::Ord), Bx::default(), false),
        };
        // Primes: at the base's size (the font's prime is drawn for that), after its overhang.
        let mut pw = 0.0;
        let mut ph = 0f32;
        for _ in 0..primes {
            let from = self.mark();
            let p = self.char('′', st);
            self.shift_since(from, b.w + b.ital + pw, 0.0);
            pw += p.w;
            ph = ph.max(p.h);
        }
        let one_glyph = b.glyph.is_some() && !big_op;
        let sup_mark = self.mark();
        let sp = sup.map(|s| self.atom(s, st.sup()).1);
        let sub_mark = self.mark();
        let sb = sub.map(|s| self.atom(s, st.sub()).1);
        let end = self.mark();
        // A glyph's scripts are placed by the constants alone; a bigger base's (a big
        // operator is boxed, as in TeX) by its edges too.
        let (mut u, mut v) = if one_glyph {
            (0.0, 0.0)
        } else {
            (
                b.h - self.k(K::SuperscriptBaselineDropMax, st),
                b.d + self.k(K::SubscriptBaselineDropMin, st),
            )
        };
        if let Some(sp) = sp {
            let shift = if st.cramped {
                K::SuperscriptShiftUpCramped
            } else {
                K::SuperscriptShiftUp
            };
            u = u
                .max(self.k(shift, st))
                .max(sp.d + self.k(K::SuperscriptBottomMin, st));
        }
        if let Some(sb) = sb {
            v = v.max(self.k(K::SubscriptShiftDown, st));
            if sp.is_none() {
                v = v.max(sb.h - self.k(K::SubscriptTopMax, st));
            }
        }
        if let (Some(sp), Some(sb)) = (sp, sb) {
            let gap = (u - sp.d) - (sb.h - v);
            let min = self.k(K::SubSuperscriptGapMin, st);
            if gap < min {
                v += min - gap;
                let lift = self.k(K::SuperscriptBottomMaxWithSubscript, st) - (u - sp.d);
                if lift > 0.0 {
                    u += lift;
                    v -= lift;
                }
            }
        }
        // The superscript clears the italics correction and the subscript doesn't (TeX's
        // rule); a big operator's advance covers its overhang, so its subscript tucks under
        // by the correction instead (MathML Core's).
        let (mut sup_x, mut sub_x) = if big_op {
            (b.w, b.w - b.ital)
        } else {
            (b.w + b.ital + pw, b.w)
        };
        // A glyph's cut-ins (MathKernInfo): a script moves in where the corner leaves room,
        // by the kern at the height of the script's baseline.
        if let (Some(id), Some(t), false) = (b.glyph, self.table, big_op || primes > 0) {
            let size = self.scale(st);
            let units = |y: f32| (y / size * self.upem).clamp(-1e5, 1e5) as i32;
            let kern = |corner, y| f32::from(t.kern(id, corner, units(y)).unwrap_or(0));
            if sp.is_some() {
                sup_x += kern(Corner::TopRight, u) / self.upem * size;
            }
            if sb.is_some() {
                sub_x += kern(Corner::BottomRight, -v) / self.upem * size;
            }
        }
        let mut w = if primes > 0 { b.w + b.ital + pw } else { b.w };
        let (mut h, mut d) = (b.h.max(ph), b.d);
        if let Some(sp) = sp {
            self.shift(sup_mark, sub_mark, sup_x, u);
            w = w.max(sup_x + sp.w);
            h = h.max(u + sp.h);
        }
        if let Some(sb) = sb {
            self.shift(sub_mark, end, sub_x, -v);
            w = w.max(sub_x + sb.w);
            d = d.max(v + sb.d);
        }
        if sp.is_some() || sb.is_some() {
            w += self.k(K::SpaceAfterScript, st);
        }
        (
            class,
            Bx {
                w,
                h,
                d,
                ..Bx::default()
            },
        )
    }

    /// Scripts above and below a base already laid out (since `from`): an operator's limits,
    /// a brace's label, `\overset`.
    fn limits(
        &mut self,
        from: Mark,
        b: Bx,
        over: Option<NodeId>,
        under: Option<NodeId>,
        st: Style,
    ) -> Bx {
        let base_end = self.mark();
        let sp = over.map(|s| self.atom(s, st.sup()).1);
        let over_end = self.mark();
        let sb = under.map(|s| self.atom(s, st.sub()).1);
        let end = self.mark();
        let w =
            b.w.max(sp.map_or(0.0, |s| s.w))
                .max(sb.map_or(0.0, |s| s.w));
        self.shift(from, base_end, (w - b.w) / 2.0, 0.0);
        let (mut h, mut d) = (b.h, b.d);
        if let Some(sp) = sp {
            let rise = self
                .k(K::UpperLimitBaselineRiseMin, st)
                .max(self.k(K::UpperLimitGapMin, st) + sp.d);
            let y = b.h + rise;
            self.shift(base_end, over_end, (w - sp.w + b.ital) / 2.0, y);
            h = y + sp.h;
        }
        if let Some(sb) = sb {
            let drop = self
                .k(K::LowerLimitBaselineDropMin, st)
                .max(self.k(K::LowerLimitGapMin, st) + sb.h);
            let y = b.d + drop;
            self.shift(over_end, end, (w - sb.w - b.ital) / 2.0, -y);
            d = y + sb.d;
        }
        Bx {
            w,
            h,
            d,
            ..Bx::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frac(
        &mut self,
        num: NodeId,
        den: NodeId,
        (bar, thick): (bool, Option<f32>),
        style: Option<MathStyle>,
        (left, right): (Option<char>, Option<char>),
        cfrac: bool,
        st: Style,
    ) -> Bx {
        let st = style.map_or(st, |s| st.with(s));
        let (num_st, den_st) = if cfrac {
            (st, st.cramp())
        } else {
            (st.num(), st.den())
        };
        let start = self.mark();
        let nb = self.atom(num, num_st).1;
        let mid = self.mark();
        let db = self.atom(den, den_st).1;
        let end = self.mark();
        let display = st.display();
        let axis = self.axis(st);
        let pick = |d: K, t: K| if display { d } else { t };
        let (u, v, t);
        if bar {
            // An explicit thickness is absolute, as TeX's: not scaled by the style.
            t = thick.unwrap_or_else(|| self.k(K::FractionRuleThickness, st));
            let num_gap = self.k(
                pick(K::FractionNumDisplayStyleGapMin, K::FractionNumeratorGapMin),
                st,
            );
            let den_gap = self.k(
                pick(
                    K::FractionDenomDisplayStyleGapMin,
                    K::FractionDenominatorGapMin,
                ),
                st,
            );
            u = self
                .k(
                    pick(
                        K::FractionNumeratorDisplayStyleShiftUp,
                        K::FractionNumeratorShiftUp,
                    ),
                    st,
                )
                .max(axis + t / 2.0 + num_gap + nb.d);
            v = self
                .k(
                    pick(
                        K::FractionDenominatorDisplayStyleShiftDown,
                        K::FractionDenominatorShiftDown,
                    ),
                    st,
                )
                .max(den_gap + t / 2.0 - axis + db.h);
        } else {
            t = 0.0;
            let mut up = self.k(pick(K::StackTopDisplayStyleShiftUp, K::StackTopShiftUp), st);
            let mut down = self.k(
                pick(K::StackBottomDisplayStyleShiftDown, K::StackBottomShiftDown),
                st,
            );
            let min = self.k(pick(K::StackDisplayStyleGapMin, K::StackGapMin), st);
            let gap = (up - nb.d) - (db.h - down);
            if gap < min {
                up += (min - gap) / 2.0;
                down += (min - gap) / 2.0;
            }
            (u, v) = (up, down);
        }
        let pad = if left.is_some() || right.is_some() {
            0.0
        } else {
            NULL_DELIMITER * self.scale(st)
        };
        let w = nb.w.max(db.w);
        self.shift(start, mid, pad + (w - nb.w) / 2.0, u);
        self.shift(mid, end, pad + (w - db.w) / 2.0, -v);
        if bar {
            self.rule(pad, axis - t / 2.0, w, t);
        }
        let mut b = Bx {
            w: w + 2.0 * pad,
            h: u + nb.h,
            d: v + db.d,
            ..Bx::default()
        };
        if left.is_some() || right.is_some() {
            // `\binom`: TeX's delimiters of fixed size, `\delim1` and `\delim2`.
            let size = if display { 2.39 } else { 1.01 } * self.scale(st);
            let body_end = self.mark();
            let lb = self.delimiter(left, size, st);
            let l_end = self.mark();
            self.shift(start, body_end, lb.w, 0.0);
            let rb = self.delimiter(right, size, st);
            self.shift_since(l_end, lb.w + b.w, 0.0);
            b = Bx {
                w: lb.w + b.w + rb.w,
                h: b.h.max(lb.h).max(rb.h),
                d: b.d.max(lb.d).max(rb.d),
                ..Bx::default()
            };
        }
        b
    }

    fn sqrt(&mut self, index: Option<NodeId>, body: NodeId, st: Style) -> Bx {
        let start = self.mark();
        let b = self.atom(body, st.cramp()).1;
        let body_end = self.mark();
        let t = self.k(K::RadicalRuleThickness, st);
        let mut gap = self.k(
            if st.display() {
                K::RadicalDisplayStyleVerticalGap
            } else {
                K::RadicalVerticalGap
            },
            st,
        );
        let target = b.h + b.d + gap + t;
        let r = self.stretch('√', target, true, st);
        let total = r.h + r.d;
        if total > target {
            gap += (total - target) / 2.0;
        }
        // The radical's top meets the bar's.
        let dy = b.h + gap + t - r.h;
        self.shift_since(body_end, 0.0, dy);
        self.shift(start, body_end, r.w, 0.0);
        self.rule(r.w, b.h + gap, b.w, t);
        let mut out = Bx {
            w: r.w + b.w,
            h: b.h + gap + t + self.k(K::RadicalExtraAscender, st),
            d: b.d.max(r.d - dy),
            ..Bx::default()
        };
        if let Some(index) = index {
            let idx_from = self.mark();
            let ib = self.atom(index, Style { level: 3, ..st }).1;
            let before = self.k(K::RadicalKernBeforeDegree, st);
            let after = self.k(K::RadicalKernAfterDegree, st);
            let pct = self
                .table
                .and_then(|t| t.constant(K::RadicalDegreeBottomRaisePercent))
                .unwrap_or(60) as f32
                / 100.0;
            // The degree's bottom at its percent of the radical sign's height (not of the
            // extra ascender above it).
            let (bottom, top) = (dy - r.d, dy + r.h);
            let y = bottom + pct * (top - bottom) + ib.d;
            // A bigger radical's hook is wider: the degree moves in with it, by a third of
            // what the sign gained over the plain one, so it stays over the hook.
            let (face, id) = self.find('√', self.face);
            let gained = (r.w - self.advance(face, id) * self.scale(st)).max(0.0);
            let after = after - gained / 3.0;
            self.shift_since(idx_from, before, y);
            let dx = (before + ib.w + after).max(0.0);
            let to = idx_from;
            self.shift(start, to, dx, 0.0);
            out.w += dx;
            out.h = out.h.max(y + ib.h);
        }
        out
    }

    /// The size `\left` and `\right` take around something `h` high and `d` deep: TeX's
    /// rule, with `\delimiterfactor` 901 and `\delimitershortfall` 5 pt.
    fn delimiter_size(&self, h: f32, d: f32, st: Style) -> f32 {
        let axis = self.axis(st);
        let delta = (h - axis).max(d + axis);
        (2.0 * delta * 0.901).max(2.0 * delta - 0.5 * self.scale(st))
    }

    /// A delimiter at least `size` tall (as tall as it gets), centred on the axis; the null
    /// delimiter is a space.
    fn delimiter(&mut self, delim: Option<char>, size: f32, st: Style) -> Bx {
        let Some(ch) = delim else {
            return Bx {
                w: NULL_DELIMITER * self.scale(st),
                ..Bx::default()
            };
        };
        let from = self.mark();
        let b = self.stretch(ch, size, true, st);
        let dy = self.axis(st) - (b.h - b.d) / 2.0;
        self.shift_since(from, 0.0, dy);
        Bx {
            h: b.h + dy,
            d: b.d - dy,
            glyph: None,
            accent: None,
            ..b
        }
    }

    fn delimited(&mut self, left: Option<char>, right: Option<char>, body: List, st: Style) -> Bx {
        let start = self.mark();
        let b = self.list(body, st);
        let body_end = self.mark();
        let size = self.delimiter_size(b.h, b.d, st);
        let lb = self.delimiter(left, size, st);
        let l_end = self.mark();
        self.shift(start, body_end, lb.w, 0.0);
        let rb = self.delimiter(right, size, st);
        self.shift_since(l_end, lb.w + b.w, 0.0);
        Bx {
            w: lb.w + b.w + rb.w,
            h: b.h.max(lb.h).max(rb.h),
            d: b.d.max(lb.d).max(rb.d),
            ..Bx::default()
        }
    }

    /// `ch` grown to at least `target` em, up (`vertical`) or across: the smallest size
    /// variant that is big enough, else an assembly of parts, else the biggest variant.
    fn stretch(&mut self, ch: char, target: f32, vertical: bool, st: Style) -> Bx {
        let size = self.scale(st);
        let (face, mut id) = self.find(ch, self.face);
        let construction = self
            .table
            .filter(|_| face == self.face)
            .and_then(|t| t.construction(id, vertical));
        let Some(c) = construction else {
            return self.glyph(face, id, size);
        };
        let units = target / size * self.upem;
        let own = self.ink(face, id);
        let own = if vertical {
            own[3] - own[1]
        } else {
            self.advance(face, id)
        } * self.upem;
        if own < units {
            for v in c.variants() {
                id = v.glyph;
                if f32::from(v.advance) >= units {
                    return self.glyph(face, id, size);
                }
            }
            if let Some(a) = c.assembly() {
                return self.assembly(a, units, vertical, size);
            }
        }
        self.glyph(face, id, size)
    }

    /// An assembly at least `target` design units long (or as long as [`MAX_PARTS`]
    /// extenders make it): bottom to top, the box's baseline at its bottom; or left to right.
    ///
    /// Parts overlap by at least the font's minimum, and by half of what their connectors
    /// allow if that is more (one more extender pays for it): each part is then cut in the
    /// middle of its joints (see [`Cut`]), and a cut lands well inside both parts' ink at any
    /// size, whatever the pixel grid does to them.
    fn assembly(&mut self, a: Assembly, target: f32, vertical: bool, size: f32) -> Bx {
        let parts: Vec<_> = a.parts().take(MAX_PARTS).collect();
        let overlap_min = f32::from(
            self.table
                .and_then(|t| t.min_connector_overlap())
                .unwrap_or(0),
        );
        let n_parts = parts.len();
        // The shortest connector at any joint there can be: every part's end that meets
        // another.
        let joint_max = parts
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                let start = (i > 0 || p.extender).then_some(p.start_connector);
                let end = (i + 1 < n_parts || p.extender).then_some(p.end_connector);
                start.into_iter().chain(end)
            })
            .map(f32::from)
            .fold(f32::INFINITY, f32::min)
            .min(1e6)
            .max(overlap_min);
        let want = (joint_max / 2.0).max(overlap_min);
        let extenders = parts.iter().filter(|p| p.extender).count();
        let fixed: f32 = parts
            .iter()
            .filter(|p| !p.extender)
            .map(|p| f32::from(p.full_advance))
            .sum();
        let ext: f32 = parts
            .iter()
            .filter(|p| p.extender)
            .map(|p| f32::from(p.full_advance))
            .sum();
        let fixed_n = n_parts - extenders;
        let mut repeats = 0;
        if extenders > 0 {
            while (repeats + 1) * extenders <= MAX_PARTS {
                let n = fixed_n + repeats * extenders;
                let longest = fixed + repeats as f32 * ext - n.saturating_sub(1) as f32 * want;
                if longest >= target {
                    break;
                }
                repeats += 1;
            }
        }
        let mut seq = Vec::with_capacity(fixed_n + repeats * extenders);
        for p in &parts {
            let n = if p.extender { repeats } else { 1 };
            seq.extend(std::iter::repeat_n(*p, n));
        }
        let n = seq.len();
        let total: f32 = seq.iter().map(|p| f32::from(p.full_advance)).sum();
        // One overlap for every joint: as much as the length allows, within what the
        // connectors allow.
        let overlap = if n > 1 {
            ((total - target) / (n - 1) as f32).clamp(overlap_min, joint_max)
        } else {
            0.0
        };
        let em = size / self.upem;
        let mut pos = 0.0;
        // Across, the box is the parts' ink, which may be all above the baseline (an arrow).
        let mut out = Bx {
            ital: f32::from(a.italics_correction) * em,
            h: if vertical { 0.0 } else { f32::NEG_INFINITY },
            d: if vertical { 0.0 } else { f32::NEG_INFINITY },
            ..Bx::default()
        };
        for (k, p) in seq.iter().enumerate() {
            let from = self.mark();
            let g = self.glyph(self.face, p.glyph, size);
            let lo = if k == 0 {
                f32::NEG_INFINITY
            } else {
                (pos + overlap / 2.0) * em
            };
            let hi = if k + 1 == n {
                f32::INFINITY
            } else {
                (pos + f32::from(p.full_advance) - overlap / 2.0) * em
            };
            if vertical {
                // Parts sit on their ink's bottom, which is where their advance starts.
                self.shift_since(from, 0.0, pos * em + g.d);
                out.w = out.w.max(g.w);
            } else {
                self.shift_since(from, pos * em, 0.0);
                out.h = out.h.max(g.h);
                out.d = out.d.max(g.d);
            }
            if let Some(glyph) = self.out.glyphs.get_mut(from.0) {
                glyph.cut = Some(Cut { vertical, lo, hi });
            }
            pos += f32::from(p.full_advance) - overlap;
        }
        let length = (total - n.saturating_sub(1) as f32 * overlap) * em;
        if vertical {
            out.h = length;
        } else {
            out.w = length;
            if out.h < -out.d {
                (out.h, out.d) = (0.0, 0.0);
            }
        }
        out
    }

    fn accent(&mut self, ch: char, kind: AccentKind, body: NodeId, st: Style) -> Bx {
        let start = self.mark();
        let b = self.atom(body, st.cramp()).1;
        let body_end = self.mark();
        match kind {
            AccentKind::Plain | AccentKind::Wide => {
                let a = if kind == AccentKind::Wide {
                    self.stretch(ch, b.w, false, st)
                } else {
                    let (face, id) = self.find(ch, self.face);
                    let size = self.scale(st);
                    self.glyph(face, id, size)
                };
                let base_x = b.accent.unwrap_or(b.w / 2.0);
                let accent_x = match (a.accent, a.glyph) {
                    (Some(x), _) => x,
                    (None, Some(id)) => {
                        let ink = self.ink(self.face, id);
                        (ink[0] + ink[2]) / 2.0 * self.scale(st)
                    }
                    (None, None) => a.w / 2.0,
                };
                let dy = (b.h - self.k(K::AccentBaseHeight, st)).max(0.0);
                self.shift_since(body_end, base_x - accent_x, dy);
                Bx {
                    h: b.h.max(a.h + dy),
                    accent: None,
                    glyph: None,
                    ..b
                }
            }
            AccentKind::Arrow | AccentKind::UnderArrow => {
                let a = self.stretch(ch, b.w, false, st);
                let w = b.w.max(a.w);
                self.shift(start, body_end, (w - b.w) / 2.0, 0.0);
                // As close as a stretched stack's parts: the arrow is a thin shaft.
                if kind == AccentKind::Arrow {
                    let y = b.h + self.k(K::StretchStackGapAboveMin, st) + a.d;
                    self.shift_since(body_end, (w - a.w) / 2.0, y);
                    Bx {
                        w,
                        h: y + a.h,
                        d: b.d,
                        ..Bx::default()
                    }
                } else {
                    let y = b.d + self.k(K::StretchStackGapBelowMin, st) + a.h;
                    self.shift_since(body_end, (w - a.w) / 2.0, -y);
                    Bx {
                        w,
                        h: b.h,
                        d: y + a.d,
                        ..Bx::default()
                    }
                }
            }
        }
    }

    fn overline(&mut self, body: NodeId, st: Style) -> Bx {
        let b = self.atom(body, st.cramp()).1;
        let t = self.k(K::OverbarRuleThickness, st);
        let gap = self.k(K::OverbarVerticalGap, st);
        self.rule(0.0, b.h + gap, b.w, t);
        Bx {
            h: b.h + gap + t + self.k(K::OverbarExtraAscender, st),
            ..Bx {
                w: b.w,
                d: b.d,
                ..Bx::default()
            }
        }
    }

    fn underline(&mut self, body: NodeId, st: Style) -> Bx {
        let b = self.atom(body, st).1;
        let t = self.k(K::UnderbarRuleThickness, st);
        let gap = self.k(K::UnderbarVerticalGap, st);
        self.rule(0.0, -(b.d + gap + t), b.w, t);
        Bx {
            w: b.w,
            h: b.h,
            d: b.d + gap + t + self.k(K::UnderbarExtraDescender, st),
            ..Bx::default()
        }
    }

    /// `\overbrace{…}`, `\underbrace{…}`: the body and the brace (its label is a limit).
    fn brace(&mut self, body: NodeId, under: bool, st: Style) -> Bx {
        let start = self.mark();
        let b = self.atom(body, st).1;
        let body_end = self.mark();
        let br = self.stretch(if under { '⏟' } else { '⏞' }, b.w, false, st);
        let w = b.w.max(br.w);
        self.shift(start, body_end, (w - b.w) / 2.0, 0.0);
        if under {
            let y = b.d + self.k(K::StretchStackGapBelowMin, st) + br.h;
            self.shift_since(body_end, (w - br.w) / 2.0, -y);
            Bx {
                w,
                h: b.h,
                d: y + br.d,
                ..Bx::default()
            }
        } else {
            let y = b.h + self.k(K::StretchStackGapAboveMin, st) + br.d;
            self.shift_since(body_end, (w - br.w) / 2.0, y);
            Bx {
                w,
                h: y + br.h,
                d: b.d,
                ..Bx::default()
            }
        }
    }

    /// `\xrightarrow[under]{over}`: an arrow as long as its labels, centred on the axis.
    fn xarrow(&mut self, ch: char, over: Option<NodeId>, under: Option<NodeId>, st: Style) -> Bx {
        let start = self.mark();
        let sp = over.map(|o| self.atom(o, st.sup()).1);
        let over_end = self.mark();
        let sb = under.map(|u| self.atom(u, st.sub()).1);
        let under_end = self.mark();
        let s = self.scale(st);
        let label = sp.map_or(0.0, |b| b.w).max(sb.map_or(0.0, |b| b.w));
        let target = (label + 0.6 * s).max(1.4 * s);
        let a = if ch == '=' {
            // `\xlongequal`: the font has no stretchy `=`, so two rules.
            let (t, gap) = (0.05 * s, 0.18 * s);
            self.rule(0.0, gap / 2.0, target, t);
            self.rule(0.0, -gap / 2.0 - t, target, t);
            Bx {
                w: target,
                h: gap / 2.0 + t,
                d: gap / 2.0 + t,
                ..Bx::default()
            }
        } else {
            self.stretch(ch, target, false, st)
        };
        let dy = self.axis(st) - (a.h - a.d) / 2.0;
        let w = a.w.max(label);
        self.shift_since(under_end, (w - a.w) / 2.0, dy);
        let (mut h, mut d) = (a.h + dy, a.d - dy);
        if let Some(sp) = sp {
            let y = h + self.k(K::StretchStackGapAboveMin, st) + sp.d;
            self.shift(start, over_end, (w - sp.w) / 2.0, y);
            h = y + sp.h;
        }
        if let Some(sb) = sb {
            let y = d + self.k(K::StretchStackGapBelowMin, st) + sb.h;
            self.shift(over_end, under_end, (w - sb.w) / 2.0, -y);
            d = y + sb.d;
        }
        Bx {
            w,
            h,
            d,
            ..Bx::default()
        }
    }

    fn boxed(&mut self, body: NodeId, st: Style) -> Bx {
        let s = self.scale(st);
        let (pad, t) = (0.3 * s, 0.04 * s);
        let from = self.mark();
        let b = self.atom(body, st).1;
        self.shift_since(from, pad + t, 0.0);
        let (w, h, d) = (b.w + 2.0 * (pad + t), b.h + pad + t, b.d + pad + t);
        self.rule(0.0, -d, w, t);
        self.rule(0.0, h - t, w, t);
        self.rule(0.0, -d, t, h + d);
        self.rule(w - t, -d, t, h + d);
        Bx {
            w,
            h,
            d,
            ..Bx::default()
        }
    }

    /// `\text{…}`: its runs, spaces and formulas side by side, without math spacing.
    fn text(&mut self, list: List, st: Style) -> Bx {
        let ast = self.ast;
        let saved = std::mem::replace(&mut self.font, Font::Normal);
        let (mut x, mut h, mut d) = (0f32, 0f32, 0f32);
        for &id in ast.list(list) {
            let from = self.mark();
            let b = match ast.node(id) {
                Some(Node::Group(l)) => self.list(*l, st),
                _ => self.atom(id, st).1,
            };
            self.shift_since(from, x, 0.0);
            x += b.w;
            h = h.max(b.h);
            d = d.max(b.d);
        }
        self.font = saved;
        Bx {
            w: x,
            h,
            d,
            ..Bx::default()
        }
    }

    /// Text in the math font's upright letters (or bold, or italic), or in the body text's
    /// sans or mono face; characters the face lacks come from the fallback faces.
    fn text_run(&mut self, start: u32, end: u32, font: TextFont, st: Style) -> Bx {
        let ast = self.ast;
        let size = self.scale(st);
        let (preferred, alphabet) = match font {
            TextFont::Upright => (self.face, Font::Roman),
            TextFont::Bold => (self.face, Font::Bold),
            TextFont::Italic => (self.face, Font::Italic),
            TextFont::Sans => (self.fonts.slot(Slot::Regular), Font::Roman),
            TextFont::Mono => (self.fonts.slot(Slot::Mono), Font::Roman),
        };
        let (mut x, mut h, mut d) = (0f32, 0f32, 0f32);
        for ch in ast.text_of(start, end).chars() {
            let (face, id) = self.find(symbols::styled(ch, alphabet), preferred);
            let from = self.mark();
            let b = self.glyph(face, id, size);
            self.shift_since(from, x, 0.0);
            x += b.w;
            h = h.max(b.h);
            d = d.max(b.d);
        }
        Bx {
            w: x,
            h,
            d,
            ..Bx::default()
        }
    }
}

/// (The tests lay out with the embedded STIX Two Math, whose numbers they know.)
#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
