//! Text selection: where it is, what a gesture makes of it, and which clusters of a block are
//! inside it.
//!
//! A position is an offset into [`Doc::text`], not a pixel, so a selection survives zoom,
//! resize and font changes: layout is only asked where the pointer is (see [`Layout::locate`])
//! and which clusters are selected when painting (see [`BlockSel`]). The text that gets copied
//! comes from the document, never from the layout (see `copy.rs`).

use std::ops::Range;

use crate::doc::{Block, BlockKind, Doc, Span};
use crate::layout::BlockBox;
use crate::text::{PHit, is_combining_mark};

/// The offset that means the end of a block, whatever its text.
pub const END: u32 = u32::MAX;

/// A caret position: before the cluster that starts at byte `off` of [`Doc::text`], in block
/// `block`. Offset 0 is at or before the block's start and [`END`] its end. They sort in reading
/// order: blocks are listed the way they are drawn, and the offsets within a block (chunks, table
/// cells left to right) only grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub block: u32,
    pub off: u32,
}

impl Pos {
    pub fn new(block: u32, off: u32) -> Pos {
        Pos { block, off }
    }

    pub fn start(block: u32) -> Pos {
        Pos { block, off: 0 }
    }

    pub fn end(block: u32) -> Pos {
        Pos { block, off: END }
    }
}

/// What a drag extends by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gran {
    Char,
    Word,
    Block,
}

/// The selected text: from the position where a gesture started to where it is now, widened to
/// whole words or blocks by a double or triple click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    gran: Gran,
    /// What the gesture started on (the anchor itself for characters), which stays selected.
    unit: (Pos, Pos),
    /// What the pointer is on now, by the same granularity.
    reach: (Pos, Pos),
}

impl Selection {
    /// A gesture that starts at `at` and has selected nothing yet.
    pub fn caret(at: Pos) -> Selection {
        Selection {
            gran: Gran::Char,
            unit: (at, at),
            reach: (at, at),
        }
    }

    /// The word at `at` (a double click).
    pub fn word(doc: &Doc, at: Pos) -> Selection {
        Selection::unit_of(Gran::Word, word_unit(doc, at))
    }

    /// The whole block at `at` (a triple click): a paragraph, heading or code block with all its
    /// chunks, a table row with all its fragments.
    pub fn block(doc: &Doc, at: Pos) -> Selection {
        Selection::unit_of(Gran::Block, block_unit(doc, at.block))
    }

    fn unit_of(gran: Gran, unit: (Pos, Pos)) -> Selection {
        Selection {
            gran,
            unit,
            reach: unit,
        }
    }

    /// The whole document, `None` when it has no blocks.
    pub fn all(doc: &Doc) -> Option<Selection> {
        let last = doc.blocks.len().checked_sub(1)? as u32;
        let (first, end) = (Pos::start(0), Pos::end(last));
        Some(Selection {
            gran: Gran::Char,
            unit: (first, first),
            reach: (end, end),
        })
    }

    /// Move the pointer's end of the selection to `to`, by the selection's granularity. What the
    /// gesture started on stays selected (Shift+click uses this too).
    pub fn extend(&mut self, doc: &Doc, to: Pos) {
        self.reach = match self.gran {
            Gran::Char => (to, to),
            Gran::Block => block_unit(doc, to.block),
            // Inside the first word it stays that word; past it the unit snaps to the word the
            // pointer is in the middle of.
            Gran::Word if to > self.unit.1 => {
                let p = snap(doc, to, true);
                (p, p)
            }
            Gran::Word if to < self.unit.0 => {
                let p = snap(doc, to, false);
                (p, p)
            }
            Gran::Word => self.unit,
        };
    }

    /// The selected span, from the earlier position to the later.
    pub fn range(&self) -> (Pos, Pos) {
        (self.unit.0.min(self.reach.0), self.unit.1.max(self.reach.1))
    }

    /// Nothing is selected (a press that hasn't moved).
    pub fn is_empty(&self) -> bool {
        let (lo, hi) = self.range();
        lo >= hi
    }
}

/// The first and last position of the block `block` belongs to, chunks of an oversized one
/// included (see [`Block::joined`]).
pub fn block_unit(doc: &Doc, block: u32) -> (Pos, Pos) {
    let n = doc.blocks.len();
    let mut first = (block as usize).min(n.saturating_sub(1));
    while first > 0 && doc.blocks[first].joined {
        first -= 1;
    }
    let mut last = first;
    while doc.blocks.get(last + 1).is_some_and(|b| b.joined) {
        last += 1;
    }
    (Pos::start(first as u32), Pos::end(last as u32))
}

/// The text of `block` that one word can't leave: the run of touching spans (or of table cells'
/// spans) that holds `off`. An `off` at the end of a run counts as in it only if no run starts
/// there.
fn extent(doc: &Doc, block: &Block, off: u32) -> Option<Range<u32>> {
    let around = |spans: &[Span], strict: bool| {
        let i = if strict {
            spans.iter().position(|s| s.start <= off && off < s.end)?
        } else {
            spans.iter().rposition(|s| s.end == off)?
        };
        let (mut a, mut b) = (i, i);
        while a > 0 && spans[a - 1].end == spans[a].start {
            a -= 1;
        }
        while b + 1 < spans.len() && spans[b].end == spans[b + 1].start {
            b += 1;
        }
        Some(spans[a].start..spans[b].end)
    };
    let find = |strict: bool| match block.kind {
        BlockKind::TableRow { .. } => doc
            .row_cells(block)
            .find_map(|(_, spans)| around(spans, strict)),
        _ => around(doc.block_spans(block), strict),
    };
    find(true).or_else(|| find(false))
}

/// What a character is, for deciding where a word ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    /// Ideographs and kana: no spaces between words, so a run counts as one (a dictionary would
    /// split it better, but would cost a table).
    Ideo,
    Space,
    /// Runs of one punctuation character.
    Punct(char),
}

fn is_ideo(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{2FFFF}')
}

fn class(c: char) -> Class {
    if is_ideo(c) {
        Class::Ideo
    } else if c.is_alphanumeric() || c == '_' || is_combining_mark(c) {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punct(c)
    }
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

/// The run of like characters around the character at byte `i` of `text`, where "like" is
/// [`Class`], with an apostrophe between two word characters (`don't`) staying in the word.
fn run_at(text: &str, i: usize) -> Range<usize> {
    let Some(c) = text[i..].chars().next() else {
        return i..i;
    };
    let word = |c: Option<char>| c.is_some_and(|c| class(c) == Class::Word);
    let between =
        word(text[..i].chars().next_back()) && word(text[i + c.len_utf8()..].chars().next());
    let want = if is_apostrophe(c) && between {
        Class::Word
    } else {
        class(c)
    };
    let joins = |before: Option<char>, c: char, after: Option<char>| {
        class(c) == want
            || (want == Class::Word
                && is_apostrophe(c)
                && before.is_some_and(|b| class(b) == Class::Word)
                && after.is_some_and(|a| class(a) == Class::Word))
    };
    let mut start = i;
    let mut back = text[..i].char_indices().rev().peekable();
    // The character after the one being looked at, going backwards.
    let mut after = Some(c);
    while let Some((k, p)) = back.next() {
        if !joins(back.peek().map(|&(_, b)| b), p, after) {
            break;
        }
        start = k;
        after = Some(p);
    }
    let mut end = i;
    let mut fwd = text[i..].char_indices().peekable();
    let mut before = text[..i].chars().next_back();
    while let Some((k, n)) = fwd.next() {
        if !joins(before, n, fwd.peek().map(|&(_, a)| a)) {
            break;
        }
        end = i + k + n.len_utf8();
        before = Some(n);
    }
    start..end
}

/// The word at the character that starts at `at` (a double click). Without text to hold one (a
/// rule, an alert's title) it is the block.
pub fn word_unit(doc: &Doc, at: Pos) -> (Pos, Pos) {
    let whole = (Pos::start(at.block), Pos::end(at.block));
    let Some(block) = doc.blocks.get(at.block as usize) else {
        return whole;
    };
    // The character before the end belongs to the word at the end.
    let off = if at.off == END {
        doc.block_spans(block).last().map_or(0, |s| s.end)
    } else {
        at.off
    };
    let Some(range) = extent(doc, block, off) else {
        return whole;
    };
    let text = &doc.text[range.start as usize..range.end as usize];
    let mut i = (off.clamp(range.start, range.end) - range.start) as usize;
    if i == text.len() {
        i = text.char_indices().next_back().map_or(0, |(k, _)| k);
    }
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    let run = run_at(text, i);
    (
        Pos::new(at.block, range.start + run.start as u32),
        Pos::new(at.block, range.start + run.end as u32),
    )
}

/// The position a drag to `to` snaps to: inside a word, the end of it going forward or its start
/// going back; at a word's edge, `to` itself.
fn snap(doc: &Doc, to: Pos, forward: bool) -> Pos {
    let Some(block) = doc.blocks.get(to.block as usize) else {
        return to;
    };
    let Some(range) = extent(doc, block, to.off).filter(|r| to.off < r.end) else {
        return to;
    };
    let text = &doc.text[range.start as usize..range.end as usize];
    let i = (to.off - range.start) as usize;
    if !text.is_char_boundary(i) {
        return to;
    }
    let run = run_at(text, i);
    if run.start == i {
        return to;
    }
    let edge = if forward { run.end } else { run.start };
    Pos::new(to.block, range.start + edge as u32)
}

/// What is selected of one block, for painting.
#[derive(Clone, Copy, Debug)]
pub struct BlockSel {
    /// Clusters starting in `from..to` are selected.
    from: u32,
    to: u32,
    /// The selection goes on into the next block.
    past: bool,
}

impl BlockSel {
    /// The part of block `block` inside `(lo, hi)`, `None` if nothing of it is.
    pub fn of(block: u32, (lo, hi): (Pos, Pos)) -> Option<BlockSel> {
        if block < lo.block || block > hi.block {
            return None;
        }
        let from = if block == lo.block { lo.off } else { 0 };
        let to = if block == hi.block { hi.off } else { END };
        // A selection that ends at the very start of a block has none of it.
        (from < to).then_some(BlockSel {
            from,
            to,
            past: block < hi.block,
        })
    }

    /// The offsets selected: clusters (and text) starting in `from..to`.
    pub fn span(&self) -> (u32, u32) {
        (self.from, self.to)
    }

    fn has(&self, off: u32) -> bool {
        self.from <= off && off < self.to
    }

    /// Whether the cluster that starts at `off` is selected (an image box, say).
    pub fn contains(&self, off: u32) -> bool {
        self.has(off)
    }

    /// Call `rect(x0, y0, x1, y1)` (block coordinates) for each rectangle that shows the
    /// selection: per line, one for each run of selected clusters; a mark one space wide where
    /// the selection goes on past the end of a line, or over an empty line inside it; and the
    /// block's marker or alert title when the selection includes its start.
    pub fn rects(&self, content: &BlockBox, mut rect: impl FnMut(f32, f32, f32, f32)) {
        let lines = &content.lines;
        let hits_of =
            |k: usize| &content.hits[lines[k].hits.start as usize..lines[k].hits.end as usize];
        if let Some([x0, y0, x1, y1]) = content.decor
            && self.has(content.hits.iter().map(|h| h.doc).min().unwrap_or(0))
        {
            rect(x0, y0, x1, y1);
        }
        // The first character of the next line that has any (where the text goes on after
        // line `k`), and the last of the previous one.
        let next_doc =
            |k: usize| (k + 1..lines.len()).find_map(|n| hits_of(n).iter().map(|h| h.doc).min());
        let prev_doc = |k: usize| {
            (0..k)
                .rev()
                .find_map(|n| hits_of(n).iter().map(|h| h.doc).max())
        };
        let space = |h: &PHit| {
            let s = &content.styles[h.style_index()];
            s.size * if s.mono { 0.6 } else { 0.3 }
        };
        for (k, line) in lines.iter().enumerate() {
            let hits = hits_of(k);
            let (y0, y1) = (line.top, line.bottom);
            let Some(last) = logical_last(hits) else {
                // An empty line is marked when the text on both sides of it is selected.
                let inside = match (prev_doc(k), next_doc(k)) {
                    (Some(p), Some(n)) => self.has(p) && self.to >= n,
                    (Some(p), None) => self.has(p) && self.past,
                    (None, Some(n)) => self.from == 0 && self.to >= n,
                    (None, None) => false,
                };
                if inside && let Some(first) = content.hits.first() {
                    let left = content.hits.iter().map(|h| h.x0.min(h.x1));
                    let left = left.fold(f32::INFINITY, f32::min);
                    rect(left, y0, left + space(first), y1);
                }
                continue;
            };
            let mut run: Option<(f32, f32)> = None;
            for h in hits {
                if self.has(h.doc) {
                    let (a, b) = (h.x0.min(h.x1), h.x0.max(h.x1));
                    run = Some(run.map_or((a, b), |(x0, x1)| (x0.min(a), x1.max(b))));
                } else if let Some((x0, x1)) = run.take() {
                    rect(x0, y0, x1, y1);
                }
            }
            if let Some((x0, x1)) = run {
                rect(x0, y0, x1, y1);
            }
            // The end of the line is marked when the selection reaches the next line (the space
            // between them hangs, with no cluster of its own) or the next block.
            let continues = next_doc(k).map_or(self.past, |d| self.to >= d);
            if self.has(last.doc) && continues {
                let (a, b) = (last.x0.min(last.x1), last.x0.max(last.x1));
                if last.rtl() {
                    rect(a - space(last), y0, a, y1);
                } else {
                    rect(b, y0, b + space(last), y1);
                }
            }
        }
    }
}

/// The hit of the text that comes last on a line, which is not always the rightmost: right to
/// left text ends at the left.
fn logical_last(hits: &[PHit]) -> Option<&PHit> {
    hits.iter().reduce(|best, h| {
        let later = h.doc > best.doc
            || (h.doc == best.doc
                && if h.rtl() {
                    h.x0.min(h.x1) < best.x0.min(best.x1)
                } else {
                    h.x0.max(h.x1) > best.x0.max(best.x1)
                });
        if later { h } else { best }
    })
}

#[cfg(test)]
mod tests;
