//! The plain text of a selection: what Ctrl+C copies.
//!
//! It is built from the document, never from the layout, so blocks far from the viewport (which
//! have none) copy as well as those on screen. [`TextCursor`] walks the selected blocks and hands
//! the text out in pieces of the size asked for, remembering where it is, so the text of a whole
//! huge document is never held at once: a big selection is kept as [`Content::Lazy`], the
//! document and a range, and the text is made as a program pastes it.

use std::sync::Arc;

use crate::doc::{Alert, Block, BlockKind, Cell, Doc, Marker, Span, style};
use crate::select::{BlockSel, Pos};

/// The size of the pieces the clipboard hands to a program (a little under it, at a character).
pub const CHUNK: usize = 64 * 1024;
/// A selection with no more text than this is made when it is copied; a bigger one when it is
/// pasted.
const EAGER: usize = 256 * 1024;

/// What was copied.
#[derive(Clone, Debug)]
pub enum Content {
    Text(Arc<str>),
    /// Text that isn't made yet: `lo..hi` of the document, which this keeps alive.
    Lazy {
        doc: Arc<Doc>,
        lo: Pos,
        hi: Pos,
    },
}

impl Content {
    /// The copy of `lo..hi`: its text, if it is short, else what makes it. Short is judged by
    /// the text of the selected spans, which costs a walk over the blocks and no copy.
    pub fn of(doc: &Arc<Doc>, (lo, hi): (Pos, Pos)) -> Content {
        let mut cursor = TextCursor::new(doc.clone(), lo, hi);
        if cursor.size_hint() > EAGER {
            return Content::Lazy {
                doc: doc.clone(),
                lo,
                hi,
            };
        }
        let mut text = String::new();
        cursor.fill(&mut text, usize::MAX);
        Content::Text(text.into())
    }

    /// The text in pieces of [`CHUNK`] bytes, as it is made.
    pub fn chunks(&self) -> Chunks {
        Chunks(match self {
            Content::Text(text) => Source::Text(text.clone(), 0),
            Content::Lazy { doc, lo, hi } => {
                Source::Lazy(Box::new(TextCursor::new(doc.clone(), *lo, *hi)))
            }
        })
    }

    /// At least the bytes of text in it (a transfer announces this much).
    pub fn size_hint(&self) -> usize {
        match self {
            Content::Text(text) => text.len(),
            Content::Lazy { doc, lo, hi } => TextCursor::new(doc.clone(), *lo, *hi).size_hint(),
        }
    }
}

/// The pieces of a [`Content`]'s text, none empty.
pub struct Chunks(Source);

enum Source {
    Text(Arc<str>, usize),
    Lazy(Box<TextCursor>),
}

impl Iterator for Chunks {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        match &mut self.0 {
            Source::Text(text, at) => {
                let rest = &text[*at..];
                let mut n = CHUNK.min(rest.len());
                while !rest.is_char_boundary(n) {
                    n -= 1;
                }
                *at += n;
                (n > 0).then(|| rest[..n].to_owned())
            }
            Source::Lazy(cursor) => {
                let mut piece = String::with_capacity(CHUNK);
                cursor.fill(&mut piece, CHUNK);
                (!piece.is_empty()).then_some(piece)
            }
        }
    }
}

/// A table row being written: its blocks (the row and the fragments it was split into), and
/// the field (column, fragment) it is at.
#[derive(Clone, Copy, Debug)]
struct Row {
    first: u32,
    last: u32,
    /// The columns that are written.
    lo: usize,
    hi: usize,
    col: usize,
    frag: u32,
}

/// The text of `lo..hi` of a document, made a piece at a time.
#[derive(Debug)]
pub struct TextCursor {
    doc: Arc<Doc>,
    lo: Pos,
    hi: Pos,
    /// The next block to look at, and the last one that gave text (what a separator depends on).
    next: u32,
    prev: Option<u32>,
    /// Made but not yet handed out: `pending[sent..]`.
    pending: String,
    sent: usize,
    /// The text so far ended a line, so the next text starts with the block's line prefix.
    bol: bool,
    row: Option<Row>,
    done: bool,
}

impl TextCursor {
    pub fn new(doc: Arc<Doc>, lo: Pos, hi: Pos) -> TextCursor {
        TextCursor {
            doc,
            lo,
            hi,
            next: lo.block,
            prev: None,
            pending: String::new(),
            sent: 0,
            bol: false,
            row: None,
            done: false,
        }
    }

    /// Append about `max` bytes of the text to `out` (never half a character, and at least one
    /// character). Returns true if there may be more: false once everything was handed out.
    pub fn fill(&mut self, out: &mut String, max: usize) -> bool {
        let start = out.len();
        loop {
            if out.len() - start >= max {
                return !(self.done && self.sent == self.pending.len());
            }
            if self.sent == self.pending.len() {
                self.pending.clear();
                self.sent = 0;
                if !self.step() {
                    return false;
                }
                continue;
            }
            let rest = &self.pending[self.sent..];
            let mut n = (max - (out.len() - start)).min(rest.len());
            while !rest.is_char_boundary(n) {
                n += 1;
            }
            out.push_str(&rest[..n]);
            self.sent += n;
        }
    }

    /// At least the bytes of document text in the range (no prefixes, separators or marks).
    pub fn size_hint(&self) -> usize {
        let doc = &*self.doc;
        let last = self.hi.block.min(doc.blocks.len().saturating_sub(1) as u32);
        let mut total = 0;
        for b in self.lo.block..=last {
            let Some(sel) = BlockSel::of(b, (self.lo, self.hi)) else {
                continue;
            };
            let (from, to) = sel.span();
            let block = &doc.blocks[b as usize];
            // (A table row's spans are those of its cells.)
            let own = (!matches!(block.kind, BlockKind::TableRow { .. }))
                .then(|| (block.spans.start as usize, doc.block_spans(block)));
            let cells = cells_of(doc, block)
                .iter()
                .map(|c| (c.spans.start as usize, spans_of(doc, c)));
            for (base, spans) in own.into_iter().chain(cells) {
                for (i, s) in spans.iter().enumerate() {
                    if !is_backlink(doc, base + i) {
                        total += s.end.min(to).saturating_sub(s.start.max(from)) as usize;
                    }
                }
            }
            if ends_code(doc, b, to) {
                total = total.saturating_sub(1);
            }
        }
        total
    }

    /// Make the text of the next block, or the next field of a row, into `pending`. False when
    /// there is nothing more.
    fn step(&mut self) -> bool {
        if self.done {
            return false;
        }
        if let Some(row) = self.row.take() {
            self.field(row);
            return true;
        }
        let doc = self.doc.clone();
        let last = self.hi.block.min(doc.blocks.len().saturating_sub(1) as u32);
        while self.next <= last && !doc.blocks.is_empty() {
            let b = self.next;
            self.next += 1;
            if matches!(doc.blocks[b as usize].kind, BlockKind::TableRow { .. }) {
                let mut end = b;
                while doc.blocks.get(end as usize + 1).is_some_and(|f| f.joined) {
                    end += 1;
                }
                self.next = end + 1;
                if self.begin_row(&doc, b, end) {
                    return true;
                }
            } else if self.block(&doc, b) {
                return true;
            }
        }
        self.done = true;
        false
    }

    /// Make the text of block `b` (not a table row). False if none of it is selected, or it has
    /// no text to give.
    fn block(&mut self, doc: &Doc, b: u32) -> bool {
        let block = &doc.blocks[b as usize];
        let Some(sel) = BlockSel::of(b, (self.lo, self.hi)) else {
            return false;
        };
        let (from, to) = sel.span();
        let spans = doc.block_spans(block);
        let mut body = String::new();
        match block.kind {
            BlockKind::Rule => return false,
            BlockKind::AlertTitle(alert) => body.push_str(alert_label(alert)),
            _ => text_of(
                doc,
                spans,
                block.spans.start as usize,
                (from, to),
                &mut body,
            ),
        }
        if ends_code(doc, b, to) && body.ends_with('\n') {
            body.pop();
        }
        // The prefix (a bullet, quote marks, an indent) goes with the first character of the
        // block; an empty item has none, and its prefix goes with its start.
        let starts = !block.joined && from <= first_off(doc, block);
        if body.is_empty() && !(spans.is_empty() && starts) {
            return false;
        }
        self.begin(doc, b, starts);
        let (_, cont) = prefixes(doc, block);
        self.write(&body, &cont);
        true
    }

    /// Start the text of block `b`: the separator after the block before it, and the prefix of
    /// its first line if the selection has its start.
    fn begin(&mut self, doc: &Doc, b: u32, starts: bool) {
        let block = &doc.blocks[b as usize];
        let mut lead = false;
        if let Some(p) = self.prev.filter(|_| !block.joined) {
            // A block cut into chunks is what its first chunk is.
            let first = (0..=p).rev().find(|&k| !doc.blocks[k as usize].joined);
            let first = first.unwrap_or(p);
            let prev = &doc.blocks[first as usize];
            // An item that starts with a block that isn't a paragraph (a table, code) has an
            // empty paragraph to carry its marker: that block goes on the marker's line.
            lead = is_item(prev)
                && doc.block_spans(prev).is_empty()
                && prev.kind == BlockKind::Paragraph
                && block.marker == Marker::None
                && (block.list, block.quote) == (prev.list, prev.quote);
            if !lead {
                let item = previous_item(doc, block, first);
                separator(prev, item, block, &mut self.pending);
            }
        }
        if !block.joined {
            self.bol = false;
            if starts && !lead {
                self.pending.push_str(&prefixes(doc, block).0);
            }
        }
        self.prev = Some(b);
    }

    /// Write `body`, giving each line after a break the block's continuation prefix. A blank
    /// line has the prefix without its trailing spaces.
    fn write(&mut self, body: &str, cont: &str) {
        let mut rest = body;
        while !rest.is_empty() {
            let end = rest.find('\n').map_or(rest.len(), |i| i + 1);
            let (line, tail) = rest.split_at(end);
            if self.bol {
                self.pending
                    .push_str(if line == "\n" { cont.trim_end() } else { cont });
                self.bol = false;
            }
            self.pending.push_str(line);
            self.bol = line.ends_with('\n');
            rest = tail;
        }
    }

    /// Start the row of blocks `first..=last` (a row and the fragments it was split into): which
    /// columns the selection reaches, then the separator and prefix. False if it reaches none.
    fn begin_row(&mut self, doc: &Doc, first: u32, last: u32) -> bool {
        let BlockKind::TableRow { table, .. } = doc.blocks[first as usize].kind else {
            return false;
        };
        let (lo, hi) = (self.lo, self.hi);
        if lo.block > last || hi.block < first {
            return false;
        }
        let cols = |b: u32| cells_of(doc, &doc.blocks[b as usize]);
        let ncols = cols(first)
            .last()
            .map_or(0, |c| c.col as usize + 1)
            .max(doc.tables[table as usize].aligns.len());
        let head = &doc.blocks[first as usize];
        let at = first_off(doc, head);
        // The prefix goes with the first character of the row's text.
        let starts = lo.block < first || (lo.block == first && lo.off <= at);
        // The columns from the first one (leading empty cells too) are taken by a selection that
        // starts before the row's text. A caret at the text's first character is in that cell, not
        // before it, except that offset 0 is also the start of the block: a selection from there
        // that goes past the first cell is taken to start at the block (Ctrl+A on a document that
        // begins with a table).
        let from_start = lo.block < first
            || (lo.block == first
                && (lo.off < at
                    || (lo.off == 0 && (hi.block > first || hi.off > first_cell_end(doc, head)))));
        let to_end = hi.block > last || (hi.block == last && hi.off == crate::select::END);
        // The columns with selected text in any fragment: a selection can start in a later
        // column of one fragment and end in an earlier column of the next.
        let mut picked: Option<(usize, usize)> = None;
        for b in lo.block.max(first)..=hi.block.min(last) {
            let Some(sel) = BlockSel::of(b, (lo, hi)) else {
                continue;
            };
            let (from, to) = sel.span();
            for c in cols(b) {
                if spans_of(doc, c)
                    .iter()
                    .any(|s| s.end > from && s.start < to)
                {
                    let col = c.col as usize;
                    picked = Some(picked.map_or((col, col), |(l, h)| (l.min(col), h.max(col))));
                }
            }
        }
        let col_lo = if from_start {
            Some(0)
        } else {
            picked.map(|p| p.0)
        };
        let col_hi = if to_end {
            Some(ncols.saturating_sub(1))
        } else {
            picked.map(|p| p.1)
        };
        let (Some(col_lo), Some(col_hi)) = (col_lo, col_hi) else {
            return false;
        };
        self.begin(doc, first, starts);
        self.row = Some(Row {
            first,
            last,
            lo: col_lo,
            hi: col_hi,
            col: col_lo,
            frag: first,
        });
        true
    }

    /// Write the field of `row` at its column and fragment: the piece of that cell in that
    /// fragment, selected part only, with breaks and tabs made spaces; a tab starts a column
    /// after the first.
    fn field(&mut self, mut row: Row) {
        let doc = self.doc.clone();
        if row.frag == row.first && row.col > row.lo {
            self.pending.push('\t');
        }
        let list = cells_of(&doc, &doc.blocks[row.frag as usize]);
        if let (Some(sel), Ok(i)) = (
            BlockSel::of(row.frag, (self.lo, self.hi)),
            list.binary_search_by_key(&(row.col as u32), |c| c.col),
        ) {
            let mut piece = String::new();
            let base = list[i].spans.start as usize;
            text_of(&doc, spans_of(&doc, &list[i]), base, sel.span(), &mut piece);
            let plain = |c| if matches!(c, '\n' | '\t') { ' ' } else { c };
            self.pending.extend(piece.chars().map(plain));
        }
        row.frag += 1;
        if row.frag > row.last {
            row.frag = row.first;
            row.col += 1;
        }
        if row.col <= row.hi {
            self.row = Some(row);
        }
    }
}

fn joined_next(doc: &Doc, b: u32) -> bool {
    doc.blocks.get(b as usize + 1).is_some_and(|n| n.joined)
}

/// The stored cells of a table row (none for another block).
fn cells_of<'a>(doc: &'a Doc, block: &Block) -> &'a [Cell] {
    match block.kind {
        BlockKind::TableRow {
            first_cell, cells, ..
        } => &doc.cells[first_cell as usize..(first_cell + cells) as usize],
        _ => &[],
    }
}

fn spans_of<'a>(doc: &'a Doc, cell: &Cell) -> &'a [Span] {
    &doc.spans[cell.spans.start as usize..cell.spans.end as usize]
}

/// Span `i` of `doc.spans` is a footnote's link back to its reference.
fn is_backlink(doc: &Doc, i: usize) -> bool {
    doc.backlinks.binary_search(&(i as u32)).is_ok()
}

/// A selection up to `to` takes the end of code block `b`, whose last line ends in a newline
/// that isn't part of what reads as a line when it is pasted.
fn ends_code(doc: &Doc, b: u32, to: u32) -> bool {
    let block = &doc.blocks[b as usize];
    block.kind == BlockKind::Code
        && !joined_next(doc, b)
        && doc.block_spans(block).last().is_some_and(|s| to >= s.end)
}

/// Where the block's text starts in [`Doc::text`] (0 for a block without any).
fn first_off(doc: &Doc, block: &Block) -> u32 {
    let first = |spans: &[Span]| spans.first().map(|s| s.start);
    match block.kind {
        BlockKind::TableRow { .. } => doc.row_cells(block).find_map(|(_, s)| first(s)),
        _ => first(doc.block_spans(block)),
    }
    .unwrap_or(0)
}

/// Where the text of a table row's first cell with any ends (0 for a row without text).
fn first_cell_end(doc: &Doc, block: &Block) -> u32 {
    let end = |spans: &[Span]| spans.last().map(|s| s.end);
    doc.row_cells(block).find_map(|(_, s)| end(s)).unwrap_or(0)
}

/// The words of an alert's title, as it is drawn.
fn alert_label(alert: Alert) -> &'static str {
    match alert {
        Alert::Note => "Note",
        Alert::Tip => "Tip",
        Alert::Important => "Important",
        Alert::Warning => "Warning",
        Alert::Caution => "Caution",
    }
}

/// The text of `spans` (the ones from `doc.spans[base..]`) that starts in `from..to`, as it is
/// read: no markup, a footnote's number in brackets (so it doesn't glue onto the word), and
/// no "↩" back to the reference.
fn text_of(doc: &Doc, spans: &[Span], base: usize, (from, to): (u32, u32), out: &mut String) {
    for (i, s) in spans.iter().enumerate() {
        let (mut a, mut z) = (s.start.max(from), s.end.min(to));
        if a >= z || is_backlink(doc, base + i) {
            continue;
        }
        // An offset inside a character moves to the nearer end of the text kept.
        while !doc.text.is_char_boundary(a as usize) {
            a += 1;
        }
        while !doc.text.is_char_boundary(z as usize) {
            z -= 1;
        }
        let piece = &doc.text[a as usize..z.max(a) as usize];
        if s.style & style::FOOTREF != 0 && (a, z) == (s.start, s.end) {
            out.push('[');
            out.push_str(piece);
            out.push(']');
        } else {
            out.push_str(piece);
        }
    }
}

/// The block starts an item (the marker of a list, not a footnote's).
fn is_item(block: &Block) -> bool {
    matches!(
        block.marker,
        Marker::Bullet | Marker::Ordered(_) | Marker::Task(_)
    )
}

/// The item before `block` (the one at index `before`, or one earlier) in the list it is in:
/// the last block at its depth that has a marker, unless the list's start comes first.
fn previous_item<'a>(doc: &'a Doc, block: &Block, before: u32) -> Option<&'a Block> {
    if !is_item(block) {
        return None;
    }
    for b in doc.blocks[..=before as usize].iter().rev() {
        if b.list < block.list {
            return None;
        }
        if b.list == block.list && is_item(b) {
            return Some(b);
        }
    }
    None
}

/// `next` begins another list than `before` is in: bullets and numbers don't mix in one list,
/// and a number that doesn't go up starts over.
fn another_list(before: &Block, next: &Block) -> bool {
    match (before.marker, next.marker) {
        (Marker::Ordered(a), Marker::Ordered(b)) => b <= a,
        (Marker::Ordered(_), _) | (_, Marker::Ordered(_)) => true,
        _ => false,
    }
}

/// What comes between a block and the one after it: a line break between the items of a list
/// (an item of several blocks has a blank line after it), the rows of a table, and an alert's
/// title and its text; else a blank line, which has the quote marks the two blocks share.
/// `item` is the item before `next` in its list, if it has one: a list that begins right after
/// another is not a continuation of it, though no block is between them.
fn separator(prev: &Block, item: Option<&Block>, next: &Block, out: &mut String) {
    let tight = match (prev.kind, next.kind) {
        (BlockKind::TableRow { table: a, .. }, BlockKind::TableRow { table: b, .. }) => a == b,
        (BlockKind::AlertTitle(_), _) => true,
        _ => {
            prev.list > 0
                && next.list > 0
                && is_item(prev)
                && is_item(next)
                && !item.is_some_and(|i| another_list(i, next))
        }
    };
    out.push('\n');
    if tight {
        return;
    }
    // A quote that starts at `next` is another quote.
    let shared = match next.quote_start {
        0 => prev.quote.min(next.quote),
        n => prev.quote.min(next.quote).min(n - 1),
    };
    out.push_str(&vec![">"; shared as usize].join(" "));
    out.push('\n');
}

/// What a block's lines start with: for its first line (with the item's marker), and for the
/// others. Quote marks and list indents are in the order they nest, as layout draws them.
fn prefixes(doc: &Doc, block: &Block) -> (String, String) {
    let (mut first, mut cont) = (String::new(), String::new());
    let marker = match block.marker {
        Marker::Bullet => Some("- ".to_owned()),
        Marker::Ordered(n) => Some(format!("{n}. ")),
        Marker::Task(done) => Some(format!("- [{}] ", if done { 'x' } else { ' ' })),
        Marker::Footnote(_) | Marker::None => None,
    };
    let opened = doc.block_quote_lists(block);
    // List levels `from + 1..=to`: the innermost one carries the item's marker.
    let levels = |from: u8, to: u8, first: &mut String, cont: &mut String| {
        for level in from + 1..=to {
            cont.push_str("  ");
            match &marker {
                Some(m) if level == block.list => first.push_str(m),
                _ => first.push_str("  "),
            }
        }
    };
    let mut depth = 0;
    for q in 0..block.quote as usize {
        let at = opened
            .get(q)
            .copied()
            .unwrap_or(depth)
            .min(block.list)
            .max(depth);
        levels(depth, at, &mut first, &mut cont);
        first.push_str("> ");
        cont.push_str("> ");
        depth = at;
    }
    levels(depth, block.list, &mut first, &mut cont);
    if let Marker::Footnote(n) = block.marker {
        first.push_str(&format!("[{n}]: "));
    }
    (first, cont)
}

#[cfg(test)]
mod tests;
