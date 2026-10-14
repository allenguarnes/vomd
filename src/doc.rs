//! Markdown → Doc IR.
//!
//! The document is parsed once into a flat structure: one text arena, a span pool, and a block
//! list. Nesting (lists, quotes) becomes per-block depth counters and markers instead of a tree,
//! so layout and paint stay linear.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pulldown_cmark::{Alignment, BlockQuoteKind, Event, Options, Parser, Tag, TagEnd};

/// Inline style bits carried by each [`Span`].
pub mod style {
    pub const BOLD: u8 = 1 << 0;
    pub const ITALIC: u8 = 1 << 1;
    pub const CODE: u8 = 1 << 2;
    pub const STRIKE: u8 = 1 << 3;
    pub const LINK: u8 = 1 << 4;
    /// Span text is the image's alt text; `Span::image` identifies the image.
    pub const IMAGE: u8 = 1 << 5;
    /// Footnote reference, rendered as a superscript number.
    pub const FOOTREF: u8 = 1 << 6;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// Byte range into [`Doc::text`].
    pub start: u32,
    pub end: u32,
    pub style: u8,
    /// 1-based index into [`Doc::links`]; 0 = not a link.
    pub link: u32,
    /// 1-based index into [`Doc::images`]; 0 = not an image.
    pub image: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    Code,
    Rule,
    /// First line of a GitHub alert blockquote (`> [!NOTE]`); has no spans.
    AlertTitle(Alert),
    /// One table row. Cells are `Doc::cells[first_cell..first_cell + cells]`.
    TableRow {
        table: u32,
        header: bool,
        first_cell: u32,
        cells: u32,
    },
}

/// Decoration drawn before the first line of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    None,
    Bullet,
    Ordered(u32),
    Task(bool),
    Footnote(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Range into [`Doc::spans`].
    pub spans: Range<u32>,
    /// List nesting depth (0 = not in a list).
    pub list: u8,
    /// Blockquote nesting depth.
    pub quote: u8,
    /// Depth of the outermost blockquote that begins at this block (0 = none), so adjacent
    /// quotes stay distinct.
    pub quote_start: u8,
    /// Range into [`Doc::quote_lists`]: the list depth at which each of the block's quote levels
    /// opened, so layout can interleave quote bars and list indents in their real nesting order.
    pub quote_lists: Range<u32>,
    pub marker: Marker,
    /// Continues the previous block (an oversized block split at [`MAX_BLOCK_BYTES`]): same
    /// kind and nesting, no marker, and no spacing in between. For a table row, fragment `k`
    /// holds piece `k` of every cell (a cell that ran out is empty).
    pub joined: bool,
}

/// Paragraphs, headings and code blocks longer than this many text bytes are split into
/// consecutive chunk blocks, so no single block needs a huge shaping pass or allocation.
pub const MAX_BLOCK_BYTES: usize = 16 * 1024;

/// Smallest per-cell share of [`MAX_BLOCK_BYTES`] when a table row is split, so rows with very
/// many columns don't shatter into thousands of fragments.
const MIN_CELL_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub aligns: Vec<Align>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub block: u32,
    /// GitHub-compatible anchor slug (also present in [`Doc::anchors`]).
    pub slug: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub src: String,
    pub remote: bool,
}

#[derive(Debug, Default)]
pub struct Doc {
    pub text: String,
    pub spans: Vec<Span>,
    pub blocks: Vec<Block>,
    /// Span ranges, one per table cell.
    pub cells: Vec<Range<u32>>,
    pub tables: Vec<Table>,
    /// Link targets, as written in the source.
    pub links: Vec<String>,
    pub images: Vec<ImageRef>,
    pub headings: Vec<Heading>,
    /// `#fragment` targets (heading slugs, footnotes) → block index.
    pub anchors: Vec<(String, u32)>,
    /// Backing store for [`Block::quote_lists`]; consecutive blocks in the same quotes share
    /// one range.
    pub quote_lists: Vec<u8>,
}

impl Doc {
    pub fn parse(src: &str) -> Doc {
        let mut opts = Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_GFM
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
        opts.remove(Options::ENABLE_SMART_PUNCTUATION);
        let mut b = Builder::new(src.len());
        for event in Parser::new_ext(src, opts) {
            b.event(event);
        }
        b.finish()
    }

    pub fn span_text(&self, span: &Span) -> &str {
        &self.text[span.start as usize..span.end as usize]
    }

    pub fn block_spans(&self, block: &Block) -> &[Span] {
        &self.spans[block.spans.start as usize..block.spans.end as usize]
    }

    /// List depth at which each of the block's quote levels opened.
    pub fn block_quote_lists(&self, block: &Block) -> &[u8] {
        &self.quote_lists[block.quote_lists.start as usize..block.quote_lists.end as usize]
    }

    /// Spans of each cell in a table row (empty for other blocks).
    pub fn row_cells(&self, block: &Block) -> impl Iterator<Item = &[Span]> {
        let cells = match block.kind {
            BlockKind::TableRow {
                first_cell, cells, ..
            } => &self.cells[first_cell as usize..(first_cell + cells) as usize],
            _ => &[],
        };
        cells
            .iter()
            .map(|r| &self.spans[r.start as usize..r.end as usize])
    }

    pub fn remote_image_count(&self) -> usize {
        self.images.iter().filter(|i| i.remote).count()
    }
}

/// Block being filled with spans.
struct Open {
    kind: BlockKind,
    span_start: u32,
}

#[derive(Default)]
struct Footnotes {
    numbers: HashMap<String, u32>,
    /// Definition blocks, appended after the main flow at the end.
    blocks: Vec<Block>,
    /// One per definition: (number, label, first index into `blocks`).
    defs: Vec<(u32, String, usize)>,
    /// Labels whose first reference already has a `fnref-` anchor.
    referenced: HashSet<String>,
    /// `fnref-` anchors of the open block, as (index into `Doc::anchors`, text offset of the
    /// reference): they get the block holding the reference once the block is split.
    pending: Vec<(usize, u32)>,
    active: bool,
    /// Main-flow `quote_start` set aside while a definition is open.
    quote_start: u8,
}

struct Builder {
    doc: Doc,
    open: Option<Open>,
    /// Spans at or after this index may be merged with the next text; protects cell boundaries.
    merge_floor: usize,
    bold: u16,
    italic: u16,
    strike: u16,
    links: Vec<u32>,
    /// Next number for each open list (`None` = bullet list).
    lists: Vec<Option<u64>>,
    quote: u8,
    /// List depth at each open quote's start.
    quote_lists: Vec<u8>,
    /// Outermost quote depth started since the last block was pushed (0 = none).
    quote_start: u8,
    marker: Marker,
    /// Open image: (image id, alt text collected so far).
    image: Option<(u32, String)>,
    html_block: Option<String>,
    metadata: bool,
    row: Option<(bool, u32)>,
    footnotes: Footnotes,
    /// Every heading slug handed out so far.
    slugs: HashSet<String>,
    /// Next `-N` suffix to try for each duplicated slug.
    slug_suffix: HashMap<String, u32>,
    /// Range of the last block's entry in `Doc::quote_lists`, for sharing.
    quote_pool: Range<u32>,
}

impl Builder {
    fn new(src_len: usize) -> Self {
        Builder {
            doc: Doc {
                text: String::with_capacity(src_len),
                ..Doc::default()
            },
            open: None,
            merge_floor: 0,
            bold: 0,
            italic: 0,
            strike: 0,
            links: Vec::new(),
            lists: Vec::new(),
            quote: 0,
            quote_lists: Vec::new(),
            quote_start: 0,
            marker: Marker::None,
            image: None,
            html_block: None,
            metadata: false,
            row: None,
            footnotes: Footnotes::default(),
            slugs: HashSet::new(),
            slug_suffix: HashMap::new(),
            quote_pool: 0..0,
        }
    }

    fn event(&mut self, event: Event) {
        if self.metadata {
            if let Event::End(TagEnd::MetadataBlock(_)) = event {
                self.metadata = false;
            }
            return;
        }
        if let Some(html) = &mut self.html_block {
            match event {
                Event::Html(s) | Event::Text(s) => html.push_str(&s),
                Event::End(TagEnd::HtmlBlock) => {
                    let html = self.html_block.take().unwrap_or_default();
                    self.html(&html, true);
                }
                _ => {}
            }
            return;
        }
        if let Some((_, alt)) = &mut self.image {
            match event {
                Event::Text(s) | Event::Code(s) => alt.push_str(&s),
                Event::SoftBreak | Event::HardBreak => alt.push(' '),
                Event::End(TagEnd::Image) => self.end_image(),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(s) => {
                let extra = if self.code_block() { style::CODE } else { 0 };
                self.text(&s, extra, 0);
            }
            Event::Code(s) => self.text(&s, style::CODE, 0),
            Event::InlineMath(s) | Event::DisplayMath(s) => self.text(&s, 0, 0),
            Event::Html(s) | Event::InlineHtml(s) => self.html(&s, false),
            Event::FootnoteReference(label) => self.footnote_ref(&label),
            Event::SoftBreak => self.text(" ", 0, 0),
            Event::HardBreak => self.text("\n", 0, 0),
            Event::Rule => {
                self.close();
                self.flush_marker();
                self.push_block(BlockKind::Rule, 0..0);
            }
            Event::TaskListMarker(checked) => self.marker = Marker::Task(checked),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.open(BlockKind::Paragraph),
            Tag::Heading { level, .. } => self.open(BlockKind::Heading(level as u8)),
            Tag::CodeBlock(_) => {
                self.close();
                self.flush_marker();
                self.open(BlockKind::Code);
            }
            Tag::HtmlBlock => {
                self.close();
                self.html_block = Some(String::new());
            }
            Tag::BlockQuote(kind) => {
                self.close();
                if kind.is_some() {
                    self.flush_marker();
                }
                self.quote = self.quote.saturating_add(1);
                self.quote_lists
                    .push(self.lists.len().min(u8::MAX as usize) as u8);
                if self.quote_start == 0 {
                    self.quote_start = self.quote;
                }
                if let Some(kind) = kind {
                    let alert = match kind {
                        BlockQuoteKind::Note => Alert::Note,
                        BlockQuoteKind::Tip => Alert::Tip,
                        BlockQuoteKind::Important => Alert::Important,
                        BlockQuoteKind::Warning => Alert::Warning,
                        BlockQuoteKind::Caution => Alert::Caution,
                    };
                    self.push_block(BlockKind::AlertTitle(alert), 0..0);
                }
            }
            Tag::List(start) => {
                self.close();
                self.flush_marker();
                self.lists.push(start);
            }
            Tag::Item => {
                self.close();
                self.flush_marker();
                self.marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = Marker::Ordered((*n).min(u32::MAX as u64) as u32);
                        *n += 1;
                        m
                    }
                    _ => Marker::Bullet,
                };
            }
            Tag::FootnoteDefinition(label) => {
                self.close();
                let n = self.footnote_number(&label);
                self.footnotes.active = true;
                self.footnotes.quote_start = std::mem::take(&mut self.quote_start);
                let at = self.footnotes.blocks.len();
                self.footnotes.defs.push((n, label.into_string(), at));
                self.marker = Marker::Footnote(n);
            }
            Tag::Table(aligns) => {
                self.close();
                self.flush_marker();
                let aligns = aligns
                    .into_iter()
                    .map(|a| match a {
                        Alignment::None => Align::None,
                        Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    })
                    .collect();
                self.doc.tables.push(Table { aligns });
            }
            Tag::TableHead | Tag::TableRow => {
                self.close();
                let header = matches!(tag, Tag::TableHead);
                self.row = Some((header, self.doc.cells.len() as u32));
                let table = self.doc.tables.len().saturating_sub(1) as u32;
                self.open(BlockKind::TableRow {
                    table,
                    header,
                    first_cell: 0,
                    cells: 0,
                });
            }
            Tag::TableCell => self.merge_floor = self.doc.spans.len(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                let id = push_id(&mut self.doc.links, dest_url.into_string());
                self.links.push(id);
            }
            Tag::Image { dest_url, .. } => {
                let id = self.push_image(dest_url.into_string());
                self.image = Some((id, String::new()));
            }
            Tag::MetadataBlock(_) => self.metadata = true,
            // Not enabled (GitHub doesn't support them); content flows as plain text.
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock => self.close(),
            TagEnd::Item => {
                self.close();
                self.flush_marker();
            }
            TagEnd::BlockQuote(_) => {
                self.close();
                self.quote = self.quote.saturating_sub(1);
                self.quote_lists.pop();
                // An empty quote has no block to start.
                if self.quote_start > self.quote {
                    self.quote_start = 0;
                }
            }
            TagEnd::List(_) => {
                self.close();
                self.marker = Marker::None;
                self.lists.pop();
            }
            TagEnd::FootnoteDefinition => {
                self.close();
                self.flush_marker();
                self.footnote_backlink();
                self.footnotes.active = false;
                self.quote_start = self.footnotes.quote_start;
            }
            TagEnd::TableHead | TagEnd::TableRow => self.close(),
            TagEnd::TableCell => {
                let start = self.merge_floor as u32;
                self.doc.cells.push(start..self.doc.spans.len() as u32);
                self.merge_floor = self.doc.spans.len();
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => self.end_image(),
            TagEnd::HtmlBlock
            | TagEnd::Table
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    fn code_block(&self) -> bool {
        matches!(
            self.open,
            Some(Open {
                kind: BlockKind::Code,
                ..
            })
        )
    }

    fn style(&self) -> u8 {
        let mut s = 0;
        if self.bold > 0 {
            s |= style::BOLD;
        }
        if self.italic > 0 {
            s |= style::ITALIC;
        }
        if self.strike > 0 {
            s |= style::STRIKE;
        }
        if self.link() != 0 {
            s |= style::LINK;
        }
        s
    }

    fn link(&self) -> u32 {
        self.links.last().copied().unwrap_or(0)
    }

    fn open(&mut self, kind: BlockKind) {
        self.close();
        self.open = Some(Open {
            kind,
            span_start: self.doc.spans.len() as u32,
        });
        self.merge_floor = self.doc.spans.len();
    }

    /// Append text to the open block, opening an implicit paragraph if needed (tight list
    /// items and inline content after HTML have no explicit paragraph).
    fn text(&mut self, s: &str, extra_style: u8, image: u32) {
        if s.is_empty() {
            return;
        }
        if self.open.is_none() {
            self.open(BlockKind::Paragraph);
        }
        let style = self.style() | extra_style;
        let link = self.link();
        let start = self.doc.text.len() as u32;
        self.doc.text.push_str(s);
        let end = self.doc.text.len() as u32;
        if image == 0
            && self.doc.spans.len() > self.merge_floor
            && let Some(last) = self.doc.spans.last_mut()
            && last.end == start
            && last.style == style
            && last.link == link
            && last.image == 0
        {
            last.end = end;
            return;
        }
        self.doc.spans.push(Span {
            start,
            end,
            style,
            link,
            image,
        });
    }

    fn close(&mut self) {
        let Some(open) = self.open.take() else { return };
        let mut kind = open.kind;
        if kind == BlockKind::Code {
            self.trim_trailing_newline(open.span_start as usize);
        }
        if let BlockKind::TableRow {
            first_cell, cells, ..
        } = &mut kind
            && let Some((_, first)) = self.row.take()
        {
            *first_cell = first;
            *cells = self.doc.cells.len() as u32 - first;
        }
        let spans = open.span_start..self.doc.spans.len() as u32;
        if kind == BlockKind::Paragraph && spans.is_empty() && self.marker == Marker::None {
            return;
        }
        if let BlockKind::Heading(level) = kind
            && !self.footnotes.active
            && let Some(slug) = self.slug(spans.clone())
        {
            // Anchors point at the first chunk of a split heading.
            let block = self.doc.blocks.len() as u32;
            self.doc.anchors.push((slug.clone(), block));
            self.doc.headings.push(Heading { level, block, slug });
        }
        let first_block = self.doc.blocks.len();
        let block_spans = &self.doc.spans[spans.start as usize..];
        let bytes = match (block_spans.first(), block_spans.last()) {
            (Some(first), Some(last)) => last.end as usize - first.start as usize,
            _ => 0,
        };
        match kind {
            BlockKind::Paragraph | BlockKind::Heading(_) | BlockKind::Code
                if bytes > MAX_BLOCK_BYTES =>
            {
                self.push_chunks(kind, spans.start as usize);
            }
            BlockKind::TableRow { first_cell, .. } if bytes > MAX_BLOCK_BYTES => {
                self.push_row_fragments(kind, spans.start as usize, first_cell as usize);
            }
            _ => self.push_block(kind, spans),
        }
        self.resolve_refs(first_block);
    }

    /// Split the open block's spans (everything from `first` on) into chunk blocks. The marker
    /// and quote start go to the first chunk; the rest are `joined` to it.
    fn push_chunks(&mut self, kind: BlockKind, first: usize) {
        let tail = self.doc.spans.split_off(first);
        let (spans, chunks) = split_spans(&self.doc.text, &tail, MAX_BLOCK_BYTES);
        self.doc.spans.extend(spans);
        for (i, chunk) in chunks.into_iter().enumerate() {
            let range = (first as u32 + chunk.start)..(first as u32 + chunk.end);
            self.push_joined(kind, range, i > 0);
        }
    }

    /// Split an oversized table row (spans from `first_span`, cells from `first_cell`) into
    /// fragment blocks: each cell is cut into pieces of at most its share of
    /// [`MAX_BLOCK_BYTES`], and fragment `k` takes piece `k` of every cell, so the fragments
    /// stay rectangular rows. The first fragment keeps the quote start; the rest are `joined`.
    fn push_row_fragments(&mut self, kind: BlockKind, first_span: usize, first_cell: usize) {
        let BlockKind::TableRow { table, header, .. } = kind else {
            return;
        };
        let cells = self.doc.cells.split_off(first_cell);
        let tail = self.doc.spans.split_off(first_span);
        let budget = (MAX_BLOCK_BYTES / cells.len().max(1)).max(MIN_CELL_BYTES);
        let pieces: Vec<_> = cells
            .iter()
            .map(|c| {
                let spans = &tail[c.start as usize - first_span..c.end as usize - first_span];
                split_spans(&self.doc.text, spans, budget)
            })
            .collect();
        let count = pieces.iter().map(|(_, chunks)| chunks.len()).max();
        for k in 0..count.unwrap_or(1) {
            let (span_from, cell_from) = (self.doc.spans.len(), self.doc.cells.len());
            for (spans, chunks) in &pieces {
                let from = self.doc.spans.len() as u32;
                if let Some(chunk) = chunks.get(k) {
                    self.doc
                        .spans
                        .extend_from_slice(&spans[chunk.start as usize..chunk.end as usize]);
                }
                self.doc.cells.push(from..self.doc.spans.len() as u32);
            }
            let kind = BlockKind::TableRow {
                table,
                header,
                first_cell: cell_from as u32,
                cells: pieces.len() as u32,
            };
            let spans = span_from as u32..self.doc.spans.len() as u32;
            self.push_joined(kind, spans, k > 0);
        }
    }

    /// Point the `fnref-` anchors of the block just closed at the block (of the `first..` run it
    /// was split into) that holds the reference.
    fn resolve_refs(&mut self, first: usize) {
        let pending = std::mem::take(&mut self.footnotes.pending);
        let blocks = &self.doc.blocks[first.min(self.doc.blocks.len())..];
        if pending.is_empty() || blocks.is_empty() {
            return;
        }
        if blocks.len() == 1 {
            for (anchor, _) in pending {
                self.doc.anchors[anchor].1 = first as u32;
            }
            return;
        }
        // Spans of a run's blocks, ordered by text position (a row's fragments interleave).
        let mut starts: Vec<(u32, u32)> = blocks
            .iter()
            .enumerate()
            .flat_map(|(k, b)| {
                self.doc.spans[b.spans.start as usize..b.spans.end as usize]
                    .iter()
                    .map(move |s| (s.start, k as u32))
            })
            .collect();
        starts.sort_unstable();
        for (anchor, offset) in pending {
            let at = starts.partition_point(|&(start, _)| start <= offset);
            let k = at.checked_sub(1).map_or(0, |at| starts[at].1);
            self.doc.anchors[anchor].1 = first as u32 + k;
        }
    }

    /// A pending marker no block claimed (empty item or footnote definition, or a nested list
    /// starting right after the marker) gets an empty paragraph to render on.
    fn flush_marker(&mut self) {
        if self.open.is_none() && self.marker != Marker::None {
            let at = self.doc.spans.len() as u32;
            self.push_block(BlockKind::Paragraph, at..at);
        }
    }

    fn push_block(&mut self, kind: BlockKind, spans: Range<u32>) {
        self.push_joined(kind, spans, false);
    }

    fn push_joined(&mut self, kind: BlockKind, spans: Range<u32>, joined: bool) {
        let quote_lists = self.quote_lists_range();
        let block = Block {
            kind,
            spans,
            list: self.lists.len().min(u8::MAX as usize) as u8,
            quote: self.quote,
            quote_start: std::mem::take(&mut self.quote_start),
            quote_lists,
            marker: std::mem::replace(&mut self.marker, Marker::None),
            joined,
        };
        if self.footnotes.active {
            self.footnotes.blocks.push(block);
        } else {
            self.doc.blocks.push(block);
        }
    }

    /// The open quotes' list depths as a range of `Doc::quote_lists`, reusing the previous
    /// block's entry when the quotes are the same.
    fn quote_lists_range(&mut self) -> Range<u32> {
        let open = &self.quote_lists[..(self.quote as usize).min(self.quote_lists.len())];
        let pool = &mut self.doc.quote_lists;
        if open.is_empty() {
            return 0..0;
        }
        let last = &self.quote_pool;
        if pool[last.start as usize..last.end as usize] != *open {
            let start = pool.len() as u32;
            pool.extend_from_slice(open);
            self.quote_pool = start..pool.len() as u32;
        }
        self.quote_pool.clone()
    }

    /// Fenced code text ends with a newline that isn't part of the content.
    fn trim_trailing_newline(&mut self, span_start: usize) {
        if self.doc.spans.len() > span_start
            && let Some(last) = self.doc.spans.last_mut()
            && self.doc.text.ends_with('\n')
            && last.end as usize == self.doc.text.len()
        {
            self.doc.text.pop();
            last.end -= 1;
            if last.start == last.end {
                self.doc.spans.pop();
            }
        }
    }

    /// GitHub-compatible heading anchor: lowercase, keep alphanumerics, `-` and `_`, spaces
    /// become `-`, duplicates get the first unused `-1`, `-2`, ... suffix.
    fn slug(&mut self, spans: Range<u32>) -> Option<String> {
        let mut slug = String::new();
        for span in &self.doc.spans[spans.start as usize..spans.end as usize] {
            for c in self.doc.text[span.start as usize..span.end as usize].chars() {
                if c == ' ' || c == '\n' {
                    slug.push('-');
                } else if c.is_alphanumeric() || c == '-' || c == '_' {
                    slug.extend(c.to_lowercase());
                }
            }
        }
        if slug.is_empty() {
            return None;
        }
        if !self.slugs.insert(slug.clone()) {
            // Skip suffixes that earlier headings already took (`foo-1` written out by hand).
            let next = self.slug_suffix.entry(slug.clone()).or_insert(0);
            loop {
                *next += 1;
                let candidate = format!("{slug}-{next}");
                if self.slugs.insert(candidate.clone()) {
                    slug = candidate;
                    break;
                }
            }
        }
        Some(slug)
    }

    fn push_image(&mut self, src: String) -> u32 {
        let remote = src.starts_with("http://") || src.starts_with("https://");
        push_id(&mut self.doc.images, ImageRef { src, remote })
    }

    fn end_image(&mut self) {
        let Some((id, alt)) = self.image.take() else {
            return;
        };
        let alt = if alt.trim().is_empty() {
            "image".to_owned()
        } else {
            alt
        };
        self.text(&alt, style::IMAGE, id);
    }

    /// Footnotes are numbered in order of first mention, reference or definition.
    fn footnote_number(&mut self, label: &str) -> u32 {
        if let Some(&n) = self.footnotes.numbers.get(label) {
            return n;
        }
        let n = self.footnotes.numbers.len() as u32 + 1;
        self.footnotes.numbers.insert(label.to_owned(), n);
        n
    }

    fn footnote_ref(&mut self, label: &str) {
        let n = self.footnote_number(label);
        let first = !self.footnotes.active && self.footnotes.referenced.insert(label.to_owned());
        let offset = self.doc.text.len() as u32;
        let link = push_id(&mut self.doc.links, format!("#fn-{label}"));
        self.links.push(link);
        self.text(&n.to_string(), style::FOOTREF, 0);
        self.links.pop();
        if first {
            // The block isn't known until the open one is split, see `resolve_refs`.
            self.footnotes
                .pending
                .push((self.doc.anchors.len(), offset));
            self.doc.anchors.push((format!("fnref-{label}"), 0));
        }
    }

    /// Append a "↩" link back to the reference at the end of the definition's last block.
    fn footnote_backlink(&mut self) {
        let Some((_, label, _)) = self.footnotes.defs.last() else {
            return;
        };
        let target = format!("#fnref-{label}");
        let Some(block) = self.footnotes.blocks.last() else {
            return;
        };
        if block.spans.end as usize != self.doc.spans.len() || block.kind == BlockKind::Code {
            return;
        }
        let link = push_id(&mut self.doc.links, target);
        let start = self.doc.text.len() as u32;
        self.doc.text.push_str(" ↩");
        let end = self.doc.text.len() as u32;
        self.doc.spans.push(Span {
            start,
            end,
            style: style::LINK,
            link,
            image: 0,
        });
        if let Some(block) = self.footnotes.blocks.last_mut() {
            block.spans.end += 1;
        }
    }

    /// Raw HTML is never interpreted: tags are stripped and text kept, except
    /// `<br>` → line break and `<img>` → image. Block-level tags break paragraphs.
    fn html(&mut self, html: &str, block: bool) {
        let mut rest = html;
        while !rest.is_empty() {
            let Some(lt) = rest.find('<') else {
                self.html_text(rest, block);
                break;
            };
            self.html_text(&rest[..lt], block);
            rest = &rest[lt..];
            if let Some(after) = rest.strip_prefix("<!--") {
                rest = after.find("-->").map_or("", |i| &after[i + 3..]);
                continue;
            }
            let Some(gt) = tag_end(rest) else {
                self.html_text(rest, block);
                break;
            };
            let tag = &rest[1..gt];
            rest = &rest[gt + 1..];
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect();
            match name.as_str() {
                "br" => self.text("\n", 0, 0),
                "img" => {
                    let src = attr(tag, "src").map_or_else(String::new, |s| decode_entities(&s));
                    let alt = attr(tag, "alt").map(|a| decode_entities(&a));
                    let id = self.push_image(src);
                    let alt = alt
                        .filter(|a| !a.trim().is_empty())
                        .unwrap_or("image".into());
                    self.text(&alt, style::IMAGE, id);
                }
                "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "tr" | "table"
                | "details" | "summary" | "ul" | "ol" | "blockquote" | "pre" | "hr"
                    if block =>
                {
                    self.close()
                }
                _ => {}
            }
        }
        if block {
            self.close();
        }
    }

    fn html_text(&mut self, s: &str, block: bool) {
        if !block {
            self.text(&decode_entities(s), 0, 0);
            return;
        }
        // Block HTML: collapse whitespace like a browser would.
        let decoded = decode_entities(s);
        let mut out = String::with_capacity(decoded.len());
        let mut space = self.doc.text.ends_with(' ') || self.open.is_none();
        for c in decoded.chars() {
            if c.is_whitespace() {
                if !space {
                    out.push(' ');
                    space = true;
                }
            } else {
                out.push(c);
                space = false;
            }
        }
        if !out.trim().is_empty() {
            self.text(&out, 0, 0);
        }
    }

    fn finish(mut self) -> Doc {
        self.close();
        let fn_blocks = std::mem::take(&mut self.footnotes.blocks);
        if !fn_blocks.is_empty() {
            self.push_block(BlockKind::Rule, 0..0);
            // List definitions in reference-number order, as GitHub does.
            let defs = std::mem::take(&mut self.footnotes.defs);
            let ends: Vec<usize> = defs
                .iter()
                .skip(1)
                .map(|d| d.2)
                .chain([fn_blocks.len()])
                .collect();
            let mut groups: Vec<_> = defs.into_iter().zip(ends).collect();
            groups.sort_by_key(|((n, ..), _)| *n);
            for ((_, label, start), end) in groups {
                let block = self.doc.blocks.len() as u32;
                self.doc.anchors.push((format!("fn-{label}"), block));
                self.doc.blocks.extend_from_slice(&fn_blocks[start..end]);
            }
        }
        self.doc
    }
}

/// Index of the `>` closing the tag at the start of `s`, skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    let mut prev = ' ';
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => {
                quote = None;
                prev = c;
            }
            Some(_) => {}
            None if c == '>' => return Some(i),
            None if (c == '"' || c == '\'') && prev == '=' => quote = Some(c),
            None if !c.is_whitespace() => prev = c,
            None => {}
        }
    }
    None
}

/// Push and return the item's 1-based id (0 is reserved for "none").
fn push_id<T>(v: &mut Vec<T>, item: T) -> u32 {
    v.push(item);
    v.len() as u32
}

/// Value of an HTML attribute (quoted or unquoted), case-insensitive name match. Walks the
/// tag's real attribute boundaries, so text inside another attribute's value never matches.
fn attr(tag: &str, name: &str) -> Option<String> {
    let b = tag.as_bytes();
    let skip_space = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    // Past the tag name.
    let mut i = tag
        .find(|c: char| c.is_ascii_whitespace())
        .unwrap_or(tag.len());
    loop {
        i = skip_space(i);
        while i < b.len() && b[i] == b'/' {
            i = skip_space(i + 1);
        }
        if i >= b.len() {
            return None;
        }
        let name_at = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        let found = tag[name_at..i].eq_ignore_ascii_case(name);
        i = skip_space(i);
        if b.get(i) != Some(&b'=') {
            if found {
                return None;
            }
            continue;
        }
        i = skip_space(i + 1);
        let value = match b.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let body = &tag[i + 1..];
                let len = body.find(q as char).unwrap_or(body.len());
                i += len + 2;
                &body[..len]
            }
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                // A `/` right before the closing `>` is self-closing syntax, not part of the URL.
                let value = &tag[start..i];
                match i == b.len() {
                    true => value.strip_suffix('/').unwrap_or(value),
                    false => value,
                }
            }
        };
        if found {
            return Some(value.to_owned());
        }
    }
}

/// Cut a block's `spans` (contiguous text) into chunks of at most `max` text bytes. Returns the
/// re-cut spans (a span crossing a cut becomes two with the same style, link and image) and
/// each chunk's range into them. A cut goes just after the last newline in reach (the newline
/// stays at the end of its chunk, so the line count is unchanged), else after the last
/// whitespace, else at the last char boundary.
fn split_spans(text: &str, spans: &[Span], max: usize) -> (Vec<Span>, Vec<Range<u32>>) {
    let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
        return (Vec::new(), Vec::new());
    };
    let (mut at, end) = (first.start as usize, last.end as usize);
    let mut out = Vec::with_capacity(spans.len() + end.saturating_sub(at) / max.max(1) + 1);
    let mut chunks = Vec::new();
    let mut next = 0;
    loop {
        let cut = if end - at > max {
            split_point(text, at, max)
        } else {
            end
        };
        while next < spans.len() && spans[next].end as usize <= at {
            next += 1;
        }
        let from = out.len() as u32;
        for span in spans[next..]
            .iter()
            .take_while(|s| (s.start as usize) < cut)
        {
            let (start, end) = (span.start.max(at as u32), span.end.min(cut as u32));
            if start < end {
                out.push(Span {
                    start,
                    end,
                    ..*span
                });
            }
        }
        chunks.push(from..out.len() as u32);
        if cut >= end {
            return (out, chunks);
        }
        at = cut;
    }
}

/// Where to cut the chunk starting at `from` (at most `max` bytes): the next chunk's start.
/// A newline ends its line, so it stays with the chunk before the cut; a cut right before one
/// would turn the end of a line into a blank line of its own.
fn split_point(text: &str, from: usize, max: usize) -> usize {
    let mut limit = (from + max).min(text.len());
    while !text.is_char_boundary(limit) {
        limit -= 1;
    }
    if limit == from {
        // `max` is smaller than the next char: take it whole.
        limit = from + text[from..].chars().next().map_or(0, char::len_utf8);
    }
    if let Some(i) = text[from..limit].rfind('\n') {
        return from + i + 1;
    }
    // The window has no newline: leave its last char to the next chunk if a newline follows.
    if text[limit..].starts_with('\n')
        && let Some(c) = text[from..limit].chars().next_back()
        && limit - c.len_utf8() > from
    {
        limit -= c.len_utf8();
    }
    let window = &text[from..limit];
    match window.char_indices().rev().find(|(_, c)| c.is_whitespace()) {
        Some((i, c)) => from + i + c.len_utf8(),
        None => limit,
    }
}

/// Decode the HTML entities that show up in hand-written READMEs.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let semi = rest.find(';').filter(|&j| j <= 10);
        let decoded = semi.and_then(|j| {
            let ent = &rest[1..j];
            let c = match ent {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => '\u{a0}',
                "copy" => '©',
                "mdash" => '—',
                "ndash" => '–',
                _ => {
                    let num = ent.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, j))
        });
        match decoded {
            Some((c, j)) => {
                out.push(c);
                rest = &rest[j + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
