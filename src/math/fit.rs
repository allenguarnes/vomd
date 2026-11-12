//! A display formula too wide for its column (VOMD has no horizontal scrolling): broken into
//! lines at its full size, at its top-level relations, else at its binary operators too, each
//! line after the first indented and starting with the operator; and shrunk only if it can't
//! be broken (one matrix, one long fraction) or its lines are still too wide, as far as it
//! takes. A break outside parentheses is preferred to one inside them, and a first line of
//! one narrow atom (`A` before `=`) is never made. Nothing is ever cut off.

use std::borrow::Cow;

use super::MAX_EXTENT;
use super::layout::{Break, Formula, Piece};

/// How far the lines after the first are indented, in em.
pub const CONT_INDENT: f32 = 2.0;
/// The space between the lines of a broken formula (beyond their depth and height), and the
/// least distance between their baselines, in em.
const LINE_GAP: f32 = 0.25;
const MIN_SKIP: f32 = 1.2;

/// A first line of one piece narrower than this (`A` before `=`) isn't worth a line.
const LONE_PIECE: f32 = 2.0 * CONT_INDENT;

/// A scale factor rounded down to 1/32, or below that to 1/1024 (and at least that), so the
/// glyph cache sees few sizes as a window is resized. A formula is at most
/// [`MAX_EXTENT`] em wide, so at 1/1024 it fits in a column an em wide.
pub fn quantize(s: f32) -> f32 {
    if s.is_nan() {
        return 1.0 / 1024.0;
    }
    let step = if s < 1.0 / 32.0 { 1024.0 } else { 32.0 };
    ((s * step).floor() / step).clamp(1.0 / 1024.0, 1.0)
}

/// Which breaks a try at breaking a formula may use.
#[derive(Clone, Copy, Debug)]
pub struct Pass {
    /// Before relations only, or binary operators too.
    pub worst: Break,
    /// Not inside more parentheses than this.
    pub depth: u16,
    /// A first line of one narrow piece (under [`LONE_PIECE`] em) is allowed.
    pub lone: bool,
}

impl Pass {
    /// Any break at `worst` or better.
    #[cfg(test)]
    pub fn any(worst: Break) -> Pass {
        Pass {
            worst,
            depth: u16::MAX,
            lone: true,
        }
    }
}

/// What to try, in order: the best breaks first. The first that makes every line fit wins;
/// if none does, the one whose lines come out the biggest (the earlier on a tie).
const PASSES: [Pass; 4] = [
    Pass {
        worst: Break::Rel,
        depth: 0,
        lone: false,
    },
    Pass {
        worst: Break::Bin,
        depth: 0,
        lone: false,
    },
    Pass {
        worst: Break::Rel,
        depth: u16::MAX,
        lone: false,
    },
    Pass {
        worst: Break::Bin,
        depth: u16::MAX,
        lone: false,
    },
];

/// `f` fitted to `avail` em: itself or broken into lines, and the scale to draw it at.
pub fn fit(f: &Formula, avail: f32) -> (Cow<'_, Formula>, f32) {
    if f.width <= avail {
        return (Cow::Borrowed(f), 1.0);
    }
    let shrink = quantize(avail / f.width);
    if f.pieces.is_empty() {
        return (Cow::Borrowed(f), shrink);
    }
    let mut best: Option<(Vec<usize>, f32)> = None;
    for pass in PASSES {
        let starts = lines(&f.pieces, avail, pass);
        if starts.len() < 2 {
            continue;
        }
        let w = widest(&f.pieces, &starts);
        if w <= avail {
            best = Some((starts, 1.0));
            break;
        }
        let q = quantize(avail / w);
        if best.as_ref().is_none_or(|b| q > b.1) {
            best = Some((starts, q));
        }
    }
    let Some((starts, _)) = best else {
        return (Cow::Borrowed(f), shrink);
    };
    let broken = split(f, &starts);
    // So many lines that they would be taller than any formula may be, or ink that reached
    // past the extent once its line moved (see [`within`]): one line, shrunk.
    if broken.height + broken.depth > MAX_EXTENT || !within(&broken) {
        return (Cow::Borrowed(f), shrink);
    }
    if broken.width <= avail {
        return (Cow::Owned(broken), 1.0);
    }
    // Still too wide: shrunk too, unless the lines would come out no bigger than the formula
    // shrunk whole (a stretch with no break in it as wide as the rest).
    let s = quantize(avail / broken.width);
    if s <= shrink {
        return (Cow::Borrowed(f), shrink);
    }
    (Cow::Owned(broken), s)
}

/// Where each line starts (piece indices, the first 0): as many pieces on a line as fit in
/// `room` em, breaking before pieces that `pass` allows. A stretch with no break in it that
/// is wider than `room` stays one line.
fn lines(pieces: &[Piece], room: f32, pass: Pass) -> Vec<usize> {
    let n = pieces.len();
    let mut starts = vec![0];
    let mut start = 0;
    // The latest break the line could end at and still fit.
    let mut fits: Option<usize> = None;
    let breaks = (1..n).filter(|&i| {
        pieces[i].breaks.is_some_and(|b| b <= pass.worst)
            && pieces[i].depth <= pass.depth
            && (pass.lone || i > 1 || pieces.first().is_none_or(|p| p.w >= LONE_PIECE))
    });
    for at in breaks.chain(std::iter::once(n)) {
        if width(pieces, start, at) <= room {
            fits = Some(at);
            continue;
        }
        // Too wide up to `at`: end the line at the last break that fitted, else at `at`.
        let end = fits.filter(|&f| f > start && f < n).unwrap_or(at);
        if end < n {
            starts.push(end);
            start = end;
        }
        fits = None;
        if end != at && at < n {
            if width(pieces, start, at) <= room {
                fits = Some(at);
            } else {
                starts.push(at);
                start = at;
            }
        }
    }
    starts
}

/// The width of a line of pieces `start..end`, its indent included.
fn width(pieces: &[Piece], start: usize, end: usize) -> f32 {
    let (Some(first), Some(last)) = (pieces.get(start), pieces.get(end.max(1) - 1)) else {
        return 0.0;
    };
    let indent = if start > 0 { CONT_INDENT } else { 0.0 };
    indent + last.x + last.w - first.x
}

/// The widest line.
fn widest(pieces: &[Piece], starts: &[usize]) -> f32 {
    starts
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            width(
                pieces,
                s,
                starts.get(k + 1).copied().unwrap_or(pieces.len()),
            )
        })
        .fold(0.0, f32::max)
}

/// Whether every coordinate of a broken formula is inside [`MAX_EXTENT`] em, plus the indent
/// of its lines: as far as a fitted formula may reach. A glyph or rule can stick out of its
/// piece (`\mathllap`, negative space, `\smash`) by as much as the formula is wide or tall;
/// whole, it stays inside the extent because what it overlaps does, but a line moves its
/// pieces left (to its indent) and down, and what sticks out of a piece far to the right, or
/// on a line far down, can then reach past it (fuzz finding: `\mathllap` around 1,000 em of
/// rules, on the second line, put a glyph at −1,010 em;
/// fuzz/seeds/math/llap-past-the-extent-after-fit.bin).
fn within(f: &Formula) -> bool {
    let inside = |v: f32| v.is_finite() && v.abs() <= MAX_EXTENT + CONT_INDENT;
    // A cut's end may be open (infinite): the end of a stretched delimiter.
    let edge = |v: f32| v.is_infinite() || inside(v);
    f.glyphs
        .iter()
        .all(|g| inside(g.x) && inside(g.y) && g.cut.is_none_or(|c| edge(c.lo) && edge(c.hi)))
        && f.rules
            .iter()
            .all(|r| inside(r.x) && inside(r.y) && inside(r.x + r.w) && inside(r.y + r.h))
}

/// `f` with its pieces in lines starting at `starts`, one under the other: the first line's
/// baseline is the formula's.
fn split(f: &Formula, starts: &[usize]) -> Formula {
    let pieces = &f.pieces;
    let mut out = Formula {
        missing: f.missing.clone(),
        axis: f.axis,
        ..Formula::default()
    };
    let mut baseline = 0.0f32;
    let mut prev_depth = 0.0f32;
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(pieces.len());
        let line = pieces.get(start..end).unwrap_or(&[]);
        let h = line.iter().fold(0.0f32, |h, p| h.max(p.h));
        let d = line.iter().fold(0.0f32, |d, p| d.max(p.d));
        if k > 0 {
            baseline -= (prev_depth + LINE_GAP + h).max(MIN_SKIP);
        } else {
            out.height = h;
        }
        prev_depth = d;
        let indent = if k > 0 { CONT_INDENT } else { 0.0 };
        let x0 = line.first().map_or(0.0, |p| p.x);
        let (dx, dy) = (indent - x0, baseline);
        let glyphs = |i: usize| pieces.get(i).map_or(f.glyphs.len(), |p| p.glyphs as usize);
        let rules = |i: usize| pieces.get(i).map_or(f.rules.len(), |p| p.rules as usize);
        for g in f.glyphs.get(glyphs(start)..glyphs(end)).unwrap_or(&[]) {
            let mut g = *g;
            g.x += dx;
            g.y += dy;
            if let Some(c) = &mut g.cut {
                let d = if c.vertical { dy } else { dx };
                c.lo += d;
                c.hi += d;
            }
            out.glyphs.push(g);
        }
        for r in f.rules.get(rules(start)..rules(end)).unwrap_or(&[]) {
            let mut r = *r;
            r.x += dx;
            r.y += dy;
            out.rules.push(r);
        }
        out.width = out.width.max(width(pieces, start, end));
        out.depth = -baseline + d;
    }
    out
}

#[cfg(test)]
mod tests;
