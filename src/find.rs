//! Find in page: the matches of a query in a document's rendered text.
//!
//! The search runs over [`Doc::text`], so markup, URLs and the front matter the parser dropped
//! never match, one block at a time: a match never spans two blocks, except the chunks of one
//! that was split for size (see [`Block::joined`]). A table cell is a text of its own. Letters
//! match whatever their case, by the simple lowercase of each character (one that lowercases to
//! several, like `İ`, to all of them), and a match is reported as the byte range of the original
//! characters it covers. The scan is one pass over the text with a Knuth–Morris–Pratt automaton,
//! so it takes time in the size of the document, whatever the query. Every match is counted, but
//! only a window of them around the point the search is for is kept (see [`Found`]): a query of
//! one common letter in a large document would otherwise keep a match for every one.

use std::collections::VecDeque;
use std::ops::Range;

use crate::doc::{BlockKind, Doc, Span};

/// The most characters a query holds.
pub const MAX_QUERY: usize = 256;

/// A character the text layout ends a line at: CR, LF, NEL, VT, FF and the line and paragraph
/// separators. The query is one line, so none of them may get into it.
pub fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\r' | '\n' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// What of a typed or committed text goes into the query: no line break and no other control
/// character.
pub fn typed_query(text: &str) -> String {
    text.chars()
        .filter(|&c| !c.is_control() && !is_line_break(c))
        .collect()
}

/// What of a pasted text goes into the query: its first line, tabs as spaces, with no other
/// control character.
pub fn pasted_query(text: &str) -> String {
    let line = text.split(is_line_break).next().unwrap_or_default();
    typed_query(&line.replace('\t', " "))
}
/// The most matches kept, in a window around the point searched for: half of them behind it,
/// unless there are fewer than that ahead of it, or the other way round.
pub const WINDOW: usize = 10_000;
const BEHIND: usize = WINDOW / 2;

/// A match: the bytes `start..end` of [`Doc::text`], which begin in block `block`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub block: u32,
    pub start: u32,
    pub end: u32,
}

/// The point a search is for: the window of matches kept surrounds the first match from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The first match that starts at or after this block and byte offset of [`Doc::text`].
    At(u32, u32),
    /// The match of this number among all of them (the first, if there is no such match).
    Index(usize),
}

/// What a search found: the number of matches, and a window of them in reading order, which is
/// all there is when there are no more than [`WINDOW`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub matches: Vec<Match>,
    /// The number among all the matches of `matches[0]`.
    pub first: usize,
    /// The number of matches in the document.
    pub total: usize,
    /// The number among all the matches of the one the search was for: the first from the focus,
    /// the first of all when there is none (it is in the window, whenever there are matches).
    pub at: usize,
    /// The byte length of the longest match in `matches`.
    pub reach: u32,
}

impl Found {
    /// The matches as the painter wants them, `current` being an index into them.
    pub fn marks(&self, current: Option<usize>) -> Marks<'_> {
        Marks {
            matches: &self.matches,
            current,
            reach: self.reach,
        }
    }
}

/// Call `out` with the characters `c` matches as: its lowercase.
fn fold(c: char, mut out: impl FnMut(char)) {
    if c.is_ascii() {
        out(c.to_ascii_lowercase());
    } else {
        c.to_lowercase().for_each(out);
    }
}

/// The folded characters of the first [`MAX_QUERY`] characters of `query`.
fn needle(query: &str) -> Vec<char> {
    let mut chars = Vec::new();
    query
        .chars()
        .take(MAX_QUERY)
        .for_each(|c| fold(c, |f| chars.push(f)));
    chars
}

/// The scan: a Knuth–Morris–Pratt automaton over the folded characters of the text, which keeps
/// where the last few characters began so a match can say where it started.
struct Scan<'a> {
    needle: &'a [char],
    /// `fail[i]`: the length of the longest proper border of `needle[..=i]`.
    fail: Vec<usize>,
    state: usize,
    /// Where the folded character number `n` began (its offset and block) is `ring[n & mask]`: the
    /// ring is a power of two long, which is at least as long as the needle.
    ring: Vec<(u32, u32)>,
    mask: usize,
    n: usize,
    focus: Focus,
    /// The matches behind the focus (the last [`WINDOW`] of them) and from it on (the first): the
    /// window takes what it needs of each, see [`Scan::finish`].
    behind: VecDeque<Match>,
    ahead: Vec<Match>,
    /// While the fragments of a split table row are fed a cell at a time, their matches wait here
    /// to be put in reading order (see [`Scan::row`]).
    held: Option<Vec<Match>>,
    /// The number of matches so far, and how many were behind the focus.
    total: usize,
    passed: usize,
}

impl<'a> Scan<'a> {
    fn new(needle: &'a [char], focus: Focus) -> Scan<'a> {
        let mut fail = vec![0; needle.len()];
        let mut k = 0;
        for i in 1..needle.len() {
            while k > 0 && needle[i] != needle[k] {
                k = fail[k - 1];
            }
            if needle[i] == needle[k] {
                k += 1;
            }
            fail[i] = k;
        }
        Scan {
            needle,
            fail,
            state: 0,
            ring: vec![(0, 0); needle.len().next_power_of_two()],
            mask: needle.len().next_power_of_two() - 1,
            n: 0,
            focus,
            behind: VecDeque::new(),
            ahead: Vec::new(),
            held: None,
            total: 0,
            passed: 0,
        }
    }

    /// The text before and after is not the text to come: no match goes across.
    fn cut(&mut self) {
        self.state = 0;
    }

    /// Feed `text`, which starts at byte `base` of the document's text, in block `block`.
    fn feed(&mut self, block: u32, base: u32, text: &str) {
        for (i, c) in text.char_indices() {
            let start = base + i as u32;
            let end = start + c.len_utf8() as u32;
            if c.is_ascii() {
                self.step(c.to_ascii_lowercase(), start, end, block);
            } else {
                c.to_lowercase()
                    .for_each(|f| self.step(f, start, end, block));
            }
        }
    }

    fn step(&mut self, f: char, start: u32, end: u32, block: u32) {
        let len = self.needle.len();
        // Nothing is under way and this cannot start a match: it is not even noted.
        if self.state == 0 && f != self.needle[0] {
            self.n += 1;
            return;
        }
        self.ring[self.n & self.mask] = (start, block);
        self.n += 1;
        while self.state > 0 && self.needle[self.state] != f {
            self.state = self.fail[self.state - 1];
        }
        if self.needle[self.state] == f {
            self.state += 1;
        }
        if self.state < len {
            return;
        }
        // The match's first character was the one `len` before this one; its slot has not been
        // reused, as the ring is at least `len` long.
        let (first, block) = self.ring[(self.n - len) & self.mask];
        self.state = 0;
        let m = Match {
            block,
            start: first,
            end,
        };
        match &mut self.held {
            Some(held) => held.push(m),
            None => self.record(m),
        }
    }

    /// Count the match `m`, the next in reading order, and keep it if it is in the window.
    fn record(&mut self, m: Match) {
        let number = self.total;
        self.total += 1;
        let behind = match self.focus {
            Focus::At(b, s) => (m.block, m.start) < (b, s),
            Focus::Index(i) => number < i,
        };
        if behind {
            if self.behind.len() == WINDOW {
                self.behind.pop_front();
            }
            self.behind.push_back(m);
            self.passed += 1;
        } else if self.ahead.len() < WINDOW {
            self.ahead.push(m);
        }
    }

    /// Feed the fragments `blocks` of a table row. Each is a row of its own that holds a piece of
    /// every cell that still had text, and a cell goes on in the next fragment where it left off:
    /// a word that the cut fell in is found, so what the scan of a cell had under way is carried
    /// over to its next piece. The matches come out a fragment at a time, cell by cell, and wait
    /// to be put in the order of the fragments and, in each, of the text: no longer than a match
    /// that is still under way can start in the fragment they are of.
    fn row(&mut self, doc: &Doc, blocks: Range<usize>) {
        if blocks.len() == 1 {
            for (_, spans) in doc.row_cells(&doc.blocks[blocks.start]) {
                self.cut();
                for span in spans {
                    self.feed(blocks.start as u32, span.start, doc.span_text(span));
                }
            }
            return;
        }
        self.held = Some(Vec::new());
        // By column: where the characters of the match under way at the end of a piece began.
        let mut open: Vec<(usize, Vec<(u32, u32)>)> = Vec::new();
        for b in blocks {
            let mut before = std::mem::take(&mut open).into_iter().peekable();
            for (col, spans) in doc.row_cells(&doc.blocks[b]) {
                while before.next_if(|o| o.0 < col).is_some() {}
                self.cut();
                if let Some((_, starts)) = before.next_if(|o| o.0 == col) {
                    for (i, &s) in starts.iter().enumerate() {
                        self.ring[i & self.mask] = s;
                    }
                    (self.state, self.n) = (starts.len(), starts.len());
                }
                for span in spans {
                    self.feed(b as u32, span.start, doc.span_text(span));
                }
                if self.state > 0 {
                    let from = self.n - self.state;
                    open.push((
                        col,
                        (from..self.n).map(|i| self.ring[i & self.mask]).collect(),
                    ));
                }
            }
            let under_way = open.iter().map(|o| o.1[0].1).min().unwrap_or(u32::MAX);
            self.release(under_way);
        }
        self.release(u32::MAX);
        self.held = None;
    }

    /// Record the held matches that start before block `block`, in reading order.
    fn release(&mut self, block: u32) {
        let mut held = self.held.take().unwrap_or_default();
        held.sort_unstable_by_key(|m| (m.block, m.start));
        let n = held.partition_point(|m| m.block < block);
        for m in held.drain(..n) {
            self.record(m);
        }
        self.held = Some(held);
    }

    /// The scan is over: the window, which is of all the matches, unless there are none from the
    /// focus on. It holds [`BEHIND`] of those behind the focus and as many of those from it on,
    /// and what one side lacks of that the other has, up to [`WINDOW`] in all.
    fn finish(self) -> Found {
        let wraps = self.passed == self.total;
        let (back, front) = (self.behind.len(), self.ahead.len());
        let back_kept = back.min(BEHIND.max(WINDOW - front));
        let front_kept = front.min(WINDOW - back_kept);
        let matches: Vec<Match> = self
            .behind
            .into_iter()
            .skip(back - back_kept)
            .chain(self.ahead.into_iter().take(front_kept))
            .collect();
        Found {
            first: self.passed - back_kept,
            at: if wraps { 0 } else { self.passed },
            total: self.total,
            reach: matches.iter().map(|m| m.end - m.start).max().unwrap_or(0),
            matches,
        }
    }
}

/// The matches of `query` in `doc`, none for an empty query, the window of them from the first.
/// Matches do not overlap.
pub fn search(doc: &Doc, query: &str) -> Found {
    search_from(doc, query, Focus::Index(0))
}

/// The matches of `query` in `doc`, with the window of them kept around `focus`. With no match
/// from the focus on the search goes around: the window is that of the first match.
pub fn search_from(doc: &Doc, query: &str, focus: Focus) -> Found {
    let needle = needle(query);
    if needle.is_empty() {
        return Found::default();
    }
    let found = scan(doc, &needle, focus);
    if found.at == 0 && found.first > 0 {
        return scan(doc, &needle, Focus::Index(0));
    }
    found
}

fn scan(doc: &Doc, needle: &[char], focus: Focus) -> Found {
    let mut scan = Scan::new(needle, focus);
    // Where the span last fed ended, to tell text that goes on from text that does not.
    let mut prev_end = None;
    let mut i = 0;
    while i < doc.blocks.len() {
        let block = &doc.blocks[i];
        if matches!(block.kind, BlockKind::TableRow { .. }) {
            // Each cell is a text of its own, the pieces of one in the fragments of a split row
            // together.
            let end = (i + 1..doc.blocks.len())
                .find(|&b| !doc.blocks[b].joined)
                .unwrap_or(doc.blocks.len());
            scan.row(doc, i..end);
            prev_end = None;
            i = end;
            continue;
        }
        if !block.joined {
            prev_end = None;
        }
        for span in doc.block_spans(block) {
            if prev_end != Some(span.start) {
                scan.cut();
            }
            prev_end = Some(span.end);
            scan.feed(i as u32, span.start, doc.span_text(span));
        }
        i += 1;
    }
    scan.finish()
}

/// The matches that fall in one block, for painting: the slice of [`Found::matches`] that may
/// touch it, and the index of the current match.
#[derive(Clone, Copy, Debug)]
pub struct BlockMarks<'a> {
    matches: &'a [Match],
    /// The index in `matches` of the current match, if it is among them.
    current: Option<usize>,
    /// The spans of the block painted.
    spans: &'a [Span],
    /// The text of the document.
    text: &'a str,
}

/// The matches of a document, and the current one, as the painter wants them (see
/// [`Found::marks`]).
#[derive(Clone, Copy, Debug)]
pub struct Marks<'a> {
    matches: &'a [Match],
    current: Option<usize>,
    /// The byte length of the longest of `matches`.
    reach: u32,
}

impl<'a> Marks<'a> {
    /// The one match `m`, which is the current one.
    pub fn one(m: &'a Match) -> Marks<'a> {
        Marks {
            matches: std::slice::from_ref(m),
            current: Some(0),
            reach: m.end - m.start,
        }
    }

    /// The marks of block `block`, `None` if no match can touch it. A match that starts in an
    /// earlier chunk of a split block may run on into this one.
    pub fn of_block(&self, doc: &'a Doc, block: u32) -> Option<BlockMarks<'a>> {
        let spans = doc
            .blocks
            .get(block as usize)
            .map_or(&[][..], |b| doc.block_spans(b));
        let (first, last) = (spans.first()?, spans.last()?);
        let mut chunk = block as usize;
        while chunk > 0 && doc.blocks.get(chunk).is_some_and(|b| b.joined) {
            chunk -= 1;
        }
        let mut lo = self.matches.partition_point(|m| (m.block as usize) < chunk);
        let mut hi = self.matches.partition_point(|m| m.block <= block);
        // Those of the chunks of a block start in the order of the text, so the ones that can
        // reach this chunk are found without looking at the rest. The fragments of a split table
        // row are not in that order, and are all looked at.
        if !matches!(doc.blocks[block as usize].kind, BlockKind::TableRow { .. }) {
            let (all, reach) = (&self.matches[lo..hi], self.reach);
            let from = all.partition_point(|m| m.start.saturating_add(reach) <= first.start);
            let to = all.partition_point(|m| m.start < last.end);
            (lo, hi) = (lo + from, lo + to.max(from));
        }
        let marks = BlockMarks {
            matches: &self.matches[lo..hi],
            current: self
                .current
                .and_then(|c| c.checked_sub(lo))
                .filter(|&c| c < hi - lo),
            spans,
            text: &doc.text,
        };
        marks
            .matches
            .iter()
            .any(|m| marks.reaches(m))
            .then_some(marks)
    }
}

/// The top and bottom of the line of `content`, the content of block `block`, that shows the
/// text at byte `off` of the document: the line of the cluster it is in, or, if it is left out
/// (the whitespace at a break or the end of a line is), of the one before it in its span. A span
/// that has no cluster at all (whitespace in emphasis, say) takes the nearest one of the spans
/// beside it in the block, or in the cell of a table row.
pub fn line_of(
    doc: &Doc,
    block: u32,
    content: &crate::layout::BlockBox,
    off: u32,
) -> Option<(f32, f32)> {
    let block = doc.blocks.get(block as usize)?;
    let spans = if matches!(block.kind, BlockKind::TableRow { .. }) {
        doc.row_cells(block)
            .map(|(_, spans)| spans)
            .find(|spans| spans.iter().any(|s| s.start <= off && off < s.end))?
    } else {
        doc.block_spans(block)
    };
    let span = spans.iter().find(|s| s.start <= off && off < s.end)?;
    let nearest = |lo: u32, hi: u32| {
        // The latest cluster at or before `off` in the range, else the earliest after it.
        let (mut before, mut after) = (None, None);
        for line in &content.lines {
            for h in &content.hits[line.hits.start as usize..line.hits.end as usize] {
                if h.doc < lo || h.doc >= hi {
                    continue;
                }
                if h.doc <= off {
                    if before.is_none_or(|(d, _)| h.doc > d) {
                        before = Some((h.doc, line));
                    }
                } else if after.is_none_or(|(d, _)| h.doc < d) {
                    after = Some((h.doc, line));
                }
            }
        }
        before.or(after).map(|(_, line)| (line.top, line.bottom))
    };
    nearest(span.start, span.end).or_else(|| nearest(spans.first()?.start, spans.last()?.end))
}

impl BlockMarks<'_> {
    /// Whether `m` may touch the block: it is in the bytes the block's spans reach from the first
    /// to the last (a split table row's fragments hold only parts of them, so this holds more).
    fn reaches(&self, m: &Match) -> bool {
        match (self.spans.first(), self.spans.last()) {
            (Some(first), Some(last)) => m.end > first.start && m.start < last.end,
            _ => false,
        }
    }

    /// Call `rect(x0, y0, x1, y1, current)` (block coordinates) for each rectangle that shows a
    /// match: per line, one for each run of clusters of one match.
    pub fn rects(
        &self,
        content: &crate::layout::BlockBox,
        mut rect: impl FnMut(f32, f32, f32, f32, bool),
    ) {
        let (offs, owner) = self.owners(content);
        for line in &content.lines {
            let hits = &content.hits[line.hits.start as usize..line.hits.end as usize];
            crate::layout::runs(
                hits,
                |off| owner[offs.binary_search(&off).ok()?],
                |x0, x1, i| {
                    rect(x0, line.top, x1, line.bottom, self.current == Some(i));
                },
            );
        }
    }

    /// Where the cluster that begins at `offs[k]` ends: where the next begins, or the span, but
    /// no later than the first whitespace after its first character. The layout drops the
    /// whitespace at a break or the end of a line, and shaping never puts it in a cluster that
    /// is not its own (a space may have a combining mark with it).
    fn cluster_end(&self, offs: &[u32], k: usize) -> u32 {
        let at = offs[k];
        let span = self.spans[..self.spans.partition_point(|s| s.start <= at)].last();
        let limit = span
            .map_or(at, |s| s.end)
            .min(offs.get(k + 1).copied().unwrap_or(u32::MAX));
        let text = self
            .text
            .get(at as usize..limit as usize)
            .unwrap_or_default();
        let mut chars = text.char_indices().skip(1);
        at + chars
            .find(|(_, c)| c.is_whitespace())
            .map_or(text.len(), |(i, _)| i) as u32
    }

    /// The clusters of `content` by their offsets in the text, and the index of the match each
    /// shows. A cluster belongs to the matches that overlap the bytes it was made from (see
    /// [`BlockMarks::cluster_end`]): those from its offset to the next cluster's, which hold a
    /// ligature's second letter or a combining mark, and the one that starts in the cluster shows
    /// the whole of it. Whitespace that was left out is nobody's, so a match of it alone shows
    /// nothing. Of several that share a cluster the current one has it, else the first.
    fn owners(&self, content: &crate::layout::BlockBox) -> (Vec<u32>, Vec<Option<usize>>) {
        let mut offs: Vec<u32> = content.hits.iter().map(|h| h.doc).collect();
        if !offs.is_sorted() {
            offs.sort_unstable();
        }
        offs.dedup();
        let mut owner = vec![None; offs.len()];
        for (i, m) in self.matches.iter().enumerate() {
            if !self.reaches(m) {
                continue;
            }
            let first = offs.partition_point(|&d| d < m.start);
            let end = first + offs[first..].partition_point(|&d| d < m.end);
            // The cluster before the start holds it unless one begins there.
            let lead = first
                .checked_sub(1)
                .filter(|_| offs.get(first) != Some(&m.start))
                .filter(|&k| m.start < self.cluster_end(&offs, k));
            for k in lead.into_iter().chain(first..end) {
                if owner[k].is_none() || self.current == Some(i) {
                    owner[k] = Some(i);
                }
            }
        }
        (offs, owner)
    }
}

#[cfg(test)]
mod tests;
