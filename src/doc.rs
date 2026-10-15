//! Markdown → Doc IR.
//!
//! The document is parsed once into a flat structure: one text arena, a span pool, and a block
//! list. Nesting (lists, quotes) becomes per-block depth counters and markers instead of a tree,
//! so layout and paint stay linear.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, BrokenLink, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd,
};
use unicase::UniCase;

mod segment;

fn options() -> Options {
    let mut opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    opts.remove(Options::ENABLE_SMART_PUNCTUATION);
    opts
}

/// Length of the YAML front matter at the very start of `src`, BOM included (0 if there is
/// none): a line `---` (trailing spaces and tabs allowed) with text on the next line, up to the
/// first line that is `---` or `...`, which is part of it. Like GitHub, only the start of the
/// document counts, so one forward scan settles it; a `---` anywhere else (or one that is never
/// closed) is an ordinary thematic break or setext underline for pulldown-cmark.
fn front_matter_len(src: &str) -> usize {
    fn line_at(src: &str, pos: usize) -> (&str, usize) {
        let end = src[pos..].find('\n').map_or(src.len(), |i| pos + i);
        let line = &src[pos..end];
        (
            line.strip_suffix('\r').unwrap_or(line),
            (end + 1).min(src.len()),
        )
    }
    fn is_fence(line: &str, c: &str) -> bool {
        line.strip_prefix(c)
            .is_some_and(|rest| rest.bytes().all(|b| b == b' ' || b == b'\t'))
    }
    let bom = if src.starts_with('\u{feff}') { 3 } else { 0 };
    let (open, mut pos) = line_at(src, bom);
    if !is_fence(open, "---") || pos >= src.len() {
        return 0;
    }
    // An empty first line or an immediate closer: a rule, not front matter.
    let first = line_at(src, pos).0;
    if first.bytes().all(|b| b == b' ' || b == b'\t')
        || is_fence(first, "---")
        || is_fence(first, "...")
    {
        return 0;
    }
    while pos < src.len() {
        let (line, next) = line_at(src, pos);
        if is_fence(line, "---") || is_fence(line, "...") {
            return next;
        }
        pos = next;
    }
    0
}

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
    /// One table row (or fragment of one). The stored cells are
    /// `Doc::cells[first_cell..first_cell + cells]`: every cell of a whole row, only the cells
    /// that still have text in a later fragment.
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
    /// holds piece `k` of every cell that has one (the first fragment keeps all cells).
    pub joined: bool,
}

/// Paragraphs, headings and code blocks longer than this many text bytes are split into
/// consecutive chunk blocks, so no single block needs a huge shaping pass or allocation.
pub const MAX_BLOCK_BYTES: usize = 16 * 1024;

/// Smallest share of a split table row's fragment per cell that still has text. The fragment's
/// [`MAX_BLOCK_BYTES`] is divided among those cells; this floor only guarantees every cell makes
/// progress, so a fragment holds about `max(MAX_BLOCK_BYTES, MIN_CELL_BYTES × active cells)`.
const MIN_CELL_BYTES: usize = 32;

/// Documents over this many bytes are parsed in segments; smaller ones in one go.
const SEGMENT_THRESHOLD: usize = 1 << 20;
/// Target segment size: pulldown-cmark builds a tree for a whole segment before emitting events,
/// so this bounds its peak memory.
const SEGMENT_BYTES: usize = 128 * 1024;
/// What pulldown-cmark lets a parser spend on copying link definitions into links, at least.
const MIN_EXPANSION: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// One stored cell of a table row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Column index; a row fragment stores only its populated cells, so columns can be skipped.
    pub col: u32,
    /// Range into [`Doc::spans`].
    pub spans: Range<u32>,
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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Doc {
    pub text: String,
    pub spans: Vec<Span>,
    pub blocks: Vec<Block>,
    /// Table cells, in row order.
    pub cells: Vec<Cell>,
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
    /// Parse a document. Large ones go through [`Doc::parse_segments`]; the result is the same.
    pub fn parse(src: &str) -> Doc {
        if src.len() > SEGMENT_THRESHOLD
            && let Some(doc) = Doc::parse_segments(src, SEGMENT_BYTES)
        {
            return doc;
        }
        Doc::parse_whole(src)
    }

    /// One parser for the whole text.
    fn parse_whole(src: &str) -> Doc {
        let src = &src[front_matter_len(src)..];
        let mut b = Builder::new(src.len());
        for event in Parser::new_ext(src, options()) {
            b.event(event);
        }
        b.finish()
    }

    /// Parse in segments of about `budget` bytes, all feeding one builder, or `None` when the
    /// text can't be split safely (see [`segment`]). Each segment's parser is dropped before the
    /// next one starts, so the parser's tree is never larger than a segment.
    ///
    /// Link reference definitions and footnote definitions may lie in another segment than their
    /// use, so a first pass parses every segment that could hold one (see
    /// [`segment::Plan::definitions`]) and the second pass gives each segment's parser what it
    /// found: the link definitions through the broken-link callback, the footnote definitions as
    /// stubs in front of the segment's text.
    fn parse_segments(src: &str, budget: usize) -> Option<Doc> {
        let src = &src[front_matter_len(src)..];
        let plan = segment::plan(src, budget)?;
        let opts = options();
        let defs = plan.definitions(src, opts)?;
        let mut b = Builder::new(src.len());
        let mut spent = 0usize;
        for (n, seg) in plan.segments.iter().enumerate() {
            let stubs = defs.footnote_stubs(&src[seg.range.clone()]);
            let text = plan.text(src, n, &stubs);
            b.begin_segment(stubs.len(), seg.head.is_some());
            let links = &defs.links;
            let resolve = move |link: BrokenLink<'_>| {
                let (dest, title) = links.get(&UniCase::new(link.reference.into_string()))?;
                Some((CowStr::from(dest.clone()), CowStr::from(title.clone())))
            };
            // Without definitions there is nothing to look up (and no callback to make).
            let resolve = (!links.is_empty()).then_some(resolve);
            let parser = Parser::new_with_broken_link_callback(&text, opts, resolve);
            let mut open = 0i64;
            let mut expanded = 0usize;
            for event in parser {
                match &event {
                    Event::Start(tag) => {
                        open += 1;
                        expanded += expansion(tag);
                    }
                    Event::End(_) => open -= 1,
                    _ => {}
                }
                b.event(event);
            }
            // pulldown-cmark ends its events early on some malformed input (a link definition
            // in a list item followed by a line of whitespace, say), losing the rest of the
            // text. Whatever one parser makes of that, segments can't repeat it. (Nor if a stub
            // didn't come out as a definition.)
            if open != 0 || b.footnotes.stubs != 0 {
                return None;
            }
            // A parser stops resolving references once their destinations and titles add up to
            // its text's length (at least 100 KB). A segment's parser has less to spend than the
            // one for the whole text, so spending that much makes them differ.
            spent += expanded;
            if expanded >= text.len().max(MIN_EXPANSION) || spent >= src.len().max(MIN_EXPANSION) {
                return None;
            }
        }
        Some(b.finish())
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

    /// The stored cells of a table row as (column, spans), in column order (nothing for other
    /// blocks). A whole row has every column; a later fragment of a split row only the columns
    /// that still have text, so callers treat missing columns as empty.
    pub fn row_cells(&self, block: &Block) -> impl Iterator<Item = (usize, &[Span])> {
        let cells = match block.kind {
            BlockKind::TableRow {
                first_cell, cells, ..
            } => &self.cells[first_cell as usize..(first_cell + cells) as usize],
            _ => &[],
        };
        cells.iter().map(|c| {
            let spans = &self.spans[c.spans.start as usize..c.spans.end as usize];
            (c.col as usize, spans)
        })
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
    /// Number of each label that a reference mentioned, keyed the way pulldown-cmark matches
    /// labels (Unicode case folding), so `[^A]` and `[^a]` are one note.
    numbers: HashMap<UniCase<String>, u32>,
    /// Labels by number (index = number - 1), as first spelled: by the first reference, or by
    /// the definition for a note nothing refers to (numbered last, see `finish`).
    labels: Vec<String>,
    /// Definition blocks, appended after the main flow at the end.
    blocks: Vec<Block>,
    /// The definitions kept, in document order: the first definition of a label wins.
    defs: Vec<FootnoteDef>,
    defined: HashSet<UniCase<String>>,
    /// Definitions open inside the open one (in a quote or list item of it), whose events are
    /// left alone: their text stays in the outer note.
    nested: usize,
    /// Those definitions' labels, with the anchor that points at their text.
    nested_defs: Vec<(usize, String)>,
    /// `fnref-` anchors of the open block, as (index into `Doc::anchors`, text offset of the
    /// reference): they get the block holding the reference once the block is split.
    pending: Vec<(usize, u32)>,
    active: bool,
    /// Main-flow `quote_start` set aside while a definition is open.
    quote_start: u8,
    /// Anchors that point into `blocks` (references inside definitions) until `finish` moves
    /// the definitions into the document.
    local: Vec<usize>,
    /// Anchor and link targets that name a footnote by number. Their ids are allocated in
    /// `finish`, after every heading, so a heading's slug is never taken from it.
    ref_anchors: Vec<(usize, u32)>,
    ref_links: Vec<(usize, u32)>,
    /// Footnote definitions the segment declares in front of its text to stand for definitions
    /// in other segments: their events are dropped (see `Doc::parse_segments`).
    stubs: usize,
}

/// A footnote definition, whose blocks are `Footnotes::blocks[first..]` up to the next one.
struct FootnoteDef {
    label: String,
    first: usize,
    /// The "↩" back-link at its end, which `finish` keeps only if something refers to the note.
    back: Option<BackLink>,
}

struct BackLink {
    /// The link span; its link id is allocated in `finish`.
    span: usize,
    /// The block holding it.
    block: usize,
    /// The block is a paragraph of its own, else the span ends a paragraph or heading of the note.
    own: bool,
}

/// Open image: its id, the alt text collected so far, and how many images are open inside it
/// (their text is part of the outer alt text).
struct ImageAlt {
    id: u32,
    alt: String,
    depth: usize,
}

struct Builder {
    doc: Doc,
    open: Option<Open>,
    /// Spans at or after this index may be merged with the next text; protects cell boundaries.
    merge_floor: usize,
    bold: usize,
    italic: usize,
    strike: usize,
    links: Vec<u32>,
    /// Next number for each open list (`None` = bullet list).
    lists: Vec<Option<u64>>,
    quote: u8,
    /// List depth at each open quote's start.
    quote_lists: Vec<u8>,
    /// Outermost quote depth started since the last block was pushed (0 = none).
    quote_start: u8,
    marker: Marker,
    image: Option<ImageAlt>,
    html_block: Option<String>,
    row: Option<(bool, u32)>,
    /// The next table continues the previous segment's (its header is a repeat, see
    /// `Doc::parse_segments`).
    continue_table: bool,
    /// Skipping the repeated header rows of a continued table.
    skip_head: bool,
    /// Skipping a footnote definition (a stub, or a repeat of a label): its depth (see
    /// `fn_depth`), 0 when not skipping. Definitions of new labels inside it are not skipped.
    skip_def: usize,
    /// Footnote definitions open now, nested ones included.
    fn_depth: usize,
    /// Skips that a definition inside a skipped one interrupted: the `skip_def` to go back to
    /// and the definition's `fn_depth`.
    suspended: Vec<(usize, usize)>,
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
            row: None,
            continue_table: false,
            skip_head: false,
            skip_def: 0,
            fn_depth: 0,
            suspended: Vec::new(),
            footnotes: Footnotes::default(),
            slugs: HashSet::new(),
            slug_suffix: HashMap::new(),
            quote_pool: 0..0,
        }
    }

    /// Start a segment of a segmented parse. It begins with `stubs` footnote definitions that
    /// only declare labels; `continued` means it starts with a repeat of the previous segment's
    /// last table header.
    fn begin_segment(&mut self, stubs: usize, continued: bool) {
        self.footnotes.stubs = stubs;
        self.continue_table = continued;
    }

    fn event(&mut self, event: Event) {
        if self.skip_head {
            self.skip_head = !matches!(event, Event::End(TagEnd::TableHead));
            return;
        }
        if let Event::Start(Tag::FootnoteDefinition(_)) = event {
            self.fn_depth += 1;
        }
        if self.skip_def > 0 {
            match &event {
                // What a dropped definition holds is dropped, but a definition of a label that is
                // still free inside it counts as it would anywhere (a reference to it needs
                // its target).
                Event::Start(Tag::FootnoteDefinition(label))
                    if self.footnotes.stubs == 0
                        && !self
                            .footnotes
                            .defined
                            .contains(&UniCase::new(label.to_string())) =>
                {
                    self.suspended.push((self.skip_def, self.fn_depth));
                    self.skip_def = 0;
                }
                Event::End(TagEnd::FootnoteDefinition) => {
                    if self.fn_depth == self.skip_def {
                        self.skip_def = 0;
                    }
                    self.fn_depth -= 1;
                    return;
                }
                _ => return,
            }
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
        if let Some(image) = &mut self.image {
            match event {
                Event::Text(s) | Event::Code(s) => image.alt.push_str(&s),
                Event::SoftBreak | Event::HardBreak => image.alt.push(' '),
                Event::Start(Tag::Image { .. }) => image.depth += 1,
                Event::End(TagEnd::Image) if image.depth > 0 => image.depth -= 1,
                Event::End(TagEnd::Image) => self.end_image(),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => {
                let def = matches!(tag, TagEnd::FootnoteDefinition);
                self.end(tag);
                if def {
                    // The definition that interrupted a skip is over: the skip goes on.
                    if let Some(&(skip, depth)) = self.suspended.last()
                        && depth == self.fn_depth
                    {
                        self.skip_def = skip;
                        self.suspended.pop();
                    }
                    self.fn_depth -= 1;
                }
            }
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
                // A stub stands for a definition elsewhere; of several definitions of a label the
                // first counts (as on GitHub), the others are dropped.
                if self.footnotes.stubs > 0 {
                    self.footnotes.stubs -= 1;
                    self.skip_def = self.fn_depth;
                    return;
                }
                if !self
                    .footnotes
                    .defined
                    .insert(UniCase::new(label.to_string()))
                {
                    self.skip_def = self.fn_depth;
                    return;
                }
                if self.footnotes.active {
                    // A note inside another one stays part of it: there is one section entry
                    // per outermost note. The label names the spot, so references to it work.
                    self.footnotes.nested += 1;
                    let anchor = self.doc.anchors.len();
                    self.doc.anchors.push((String::new(), 0));
                    let offset = self.doc.text.len() as u32;
                    self.footnotes.pending.push((anchor, offset));
                    self.footnotes
                        .nested_defs
                        .push((anchor, label.into_string()));
                    return;
                }
                self.close();
                self.footnotes.active = true;
                self.footnotes.quote_start = std::mem::take(&mut self.quote_start);
                self.footnotes.defs.push(FootnoteDef {
                    label: label.into_string(),
                    first: self.footnotes.blocks.len(),
                    back: None,
                });
                // The number is only known once every reference has been seen (`finish`).
                self.marker = Marker::Footnote(0);
            }
            Tag::Table(_) if std::mem::take(&mut self.continue_table) => self.skip_head = true,
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
                self.image = Some(ImageAlt {
                    id,
                    alt: String::new(),
                    depth: 0,
                });
            }
            // Not enabled (GitHub doesn't support them); content flows as plain text.
            Tag::MetadataBlock(_)
            | Tag::DefinitionList
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
            TagEnd::FootnoteDefinition if self.footnotes.nested > 0 => self.footnotes.nested -= 1,
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
                let first = self.row.map_or(0, |(_, first)| first as usize);
                self.doc.cells.push(Cell {
                    col: (self.doc.cells.len() - first) as u32,
                    spans: start..self.doc.spans.len() as u32,
                });
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
        let first_block = self.block_count();
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
    /// fragment blocks, so the fragments stay rectangular rows: fragment `k` takes piece `k` of
    /// every cell that has one. A fragment holds about [`MAX_BLOCK_BYTES`] of text in all, shared
    /// equally by the cells that still have text (at least [`MIN_CELL_BYTES`] each, so every
    /// cell progresses). The first fragment keeps every cell; later ones only store the cells
    /// that still have text, so exhausted cells cost nothing. The first fragment keeps the quote
    /// start; the rest are `joined`.
    fn push_row_fragments(&mut self, kind: BlockKind, first_span: usize, first_cell: usize) {
        let BlockKind::TableRow { table, header, .. } = kind else {
            return;
        };
        let cells = self.doc.cells.split_off(first_cell);
        let tail = self.doc.spans.split_off(first_span);
        // Per cell: its spans, and where its remaining text starts and ends.
        let mut rest: Vec<CellRest> = cells
            .iter()
            .map(|c| {
                let spans = &tail[c.spans.start as usize - first_span..][..c.spans.len()];
                let (at, end) = match (spans.first(), spans.last()) {
                    (Some(first), Some(last)) => (first.start as usize, last.end as usize),
                    _ => (0, 0),
                };
                CellRest {
                    col: c.col,
                    spans,
                    next: 0,
                    at,
                    end,
                }
            })
            .collect();
        let mut k = 0;
        loop {
            // The first fragment goes through every cell; after it `rest` only holds the cells
            // that still have text.
            let active = rest.iter().filter(|r| r.at < r.end).count();
            let budget = (MAX_BLOCK_BYTES / active.max(1)).max(MIN_CELL_BYTES);
            let (span_from, cell_from) = (self.doc.spans.len(), self.doc.cells.len());
            for r in rest.iter_mut() {
                let from = self.doc.spans.len() as u32;
                take_chunk(&self.doc.text, r, budget, &mut self.doc.spans);
                let spans = from..self.doc.spans.len() as u32;
                self.doc.cells.push(Cell { col: r.col, spans });
            }
            #[cfg(test)]
            ROW_CELL_VISITS.with(|n| n.set(n.get() + 2 * rest.len()));
            let kind = BlockKind::TableRow {
                table,
                header,
                first_cell: cell_from as u32,
                cells: (self.doc.cells.len() - cell_from) as u32,
            };
            let spans = span_from as u32..self.doc.spans.len() as u32;
            self.push_joined(kind, spans, k > 0);
            k += 1;
            rest.retain(|r| r.at < r.end);
            if rest.is_empty() {
                break;
            }
        }
    }

    /// Number of blocks in the list the open block will be pushed to.
    fn block_count(&self) -> usize {
        if self.footnotes.active {
            self.footnotes.blocks.len()
        } else {
            self.doc.blocks.len()
        }
    }

    /// Point the `fnref-` anchors of the block just closed at the block (of the `first..` run it
    /// was split into) that holds the reference. Inside a footnote definition the block index is
    /// into the definitions' blocks for now (see `finish`).
    fn resolve_refs(&mut self, first: usize) {
        let pending = std::mem::take(&mut self.footnotes.pending);
        let active = self.footnotes.active;
        let all = if active {
            &self.footnotes.blocks
        } else {
            &self.doc.blocks
        };
        let blocks = &all[first.min(all.len())..];
        if pending.is_empty() || blocks.is_empty() {
            return;
        }
        let mut targets = Vec::with_capacity(pending.len());
        if blocks.len() == 1 {
            targets.extend(pending.iter().map(|&(anchor, _)| (anchor, first)));
        } else {
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
            for &(anchor, offset) in &pending {
                let at = starts.partition_point(|&(start, _)| start <= offset);
                let k = at.checked_sub(1).map_or(0, |at| starts[at].1);
                targets.push((anchor, first + k as usize));
            }
        }
        for (anchor, block) in targets {
            self.doc.anchors[anchor].1 = block as u32;
            if active {
                self.footnotes.local.push(anchor);
            }
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
    /// become `-`, duplicates get the first unused suffix (see `unique_id`).
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
        (!slug.is_empty()).then(|| self.unique_id(slug))
    }

    /// Claim `id` as an anchor id: itself if nobody has it, else the first unused `-1`, `-2`,
    /// ... suffix. Headings and footnotes share this registry, so no two anchors collide.
    fn unique_id(&mut self, id: String) -> String {
        if self.slugs.insert(id.clone()) {
            return id;
        }
        // Skip suffixes that earlier ids already took (`foo-1` written out by hand).
        let next = self.slug_suffix.entry(id.clone()).or_insert(0);
        loop {
            *next += 1;
            let candidate = format!("{id}-{next}");
            if self.slugs.insert(candidate.clone()) {
                return candidate;
            }
        }
    }

    fn push_image(&mut self, src: String) -> u32 {
        let remote = src.starts_with("http://") || src.starts_with("https://");
        push_id(&mut self.doc.images, ImageRef { src, remote })
    }

    fn end_image(&mut self) {
        let Some(ImageAlt { id, alt, .. }) = self.image.take() else {
            return;
        };
        let alt = if alt.trim().is_empty() {
            "image".to_owned()
        } else {
            alt
        };
        self.text(&alt, style::IMAGE, id);
    }

    /// Footnotes are numbered in order of first reference, whether it is in the text or inside
    /// another note. Returns the number and whether this is the first reference.
    fn footnote_number(&mut self, label: &str) -> (u32, bool) {
        let key = UniCase::new(label.to_owned());
        if let Some(&n) = self.footnotes.numbers.get(&key) {
            return (n, false);
        }
        let n = self.footnotes.labels.len() as u32 + 1;
        self.footnotes.numbers.insert(key, n);
        self.footnotes.labels.push(label.to_owned());
        (n, true)
    }

    /// The first reference to a label (in the main text or inside a definition) gets a
    /// `fnref-` anchor, which the definition's back-link points at. Link and anchor ids are
    /// filled in by `finish`.
    fn footnote_ref(&mut self, label: &str) {
        let (n, first) = self.footnote_number(label);
        let offset = self.doc.text.len() as u32;
        let link = push_id(&mut self.doc.links, String::new());
        self.footnotes.ref_links.push((link as usize - 1, n));
        self.links.push(link);
        self.text(&n.to_string(), style::FOOTREF, 0);
        self.links.pop();
        if first {
            // The block isn't known until the open one is split, see `resolve_refs`.
            let anchor = self.doc.anchors.len();
            self.footnotes.pending.push((anchor, offset));
            self.footnotes.ref_anchors.push((anchor, n));
            self.doc.anchors.push((String::new(), 0));
        }
    }

    /// End a footnote definition with a "↩" link back to the reference: appended to the
    /// definition's last block when that is a paragraph (or heading), else a paragraph of its
    /// own (a table row, code block or rule has no paragraph text to hold it). Whether anything
    /// refers to the note isn't known yet, so `finish` takes the link out again if not.
    fn footnote_backlink(&mut self) {
        let Some(first) = self.footnotes.defs.last().map(|d| d.first) else {
            return;
        };
        let spans_end = self.doc.spans.len();
        let last = self.footnotes.blocks[first..].last_mut();
        let (text, own) = match last {
            Some(block)
                if matches!(block.kind, BlockKind::Paragraph | BlockKind::Heading(_))
                    && block.spans.end as usize == spans_end =>
            {
                block.spans.end += 1;
                (" ↩", false)
            }
            _ => ("↩", true),
        };
        let start = self.doc.text.len() as u32;
        self.doc.text.push_str(text);
        let end = self.doc.text.len() as u32;
        self.doc.spans.push(Span {
            start,
            end,
            style: style::LINK,
            link: 0,
            image: 0,
        });
        if own {
            let at = spans_end as u32;
            self.push_block(BlockKind::Paragraph, at..at + 1);
        }
        let block = self.footnotes.blocks.len() - 1;
        if let Some(def) = self.footnotes.defs.last_mut() {
            def.back = Some(BackLink {
                span: spans_end,
                block,
                own,
            });
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
        let mut fn_blocks = std::mem::take(&mut self.footnotes.blocks);
        if !fn_blocks.is_empty() {
            self.push_block(BlockKind::Rule, 0..0);
        }
        // Notes nothing refers to are numbered after the referenced ones.
        let referenced = self.footnotes.labels.len();
        let defs = std::mem::take(&mut self.footnotes.defs);
        let nested = std::mem::take(&mut self.footnotes.nested_defs);
        let defined = defs
            .iter()
            .map(|d| &d.label)
            .chain(nested.iter().map(|(_, label)| label));
        for label in defined {
            let key = UniCase::new(label.clone());
            if !self.footnotes.numbers.contains_key(&key) {
                let n = self.footnotes.labels.len() as u32 + 1;
                self.footnotes.numbers.insert(key, n);
                self.footnotes.labels.push(label.clone());
            }
        }
        // Anchor ids for every footnote, in number order, now that all headings have theirs. Only
        // a referenced note has a return anchor.
        let labels = std::mem::take(&mut self.footnotes.labels);
        let ids: Vec<(String, String)> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let note = self.unique_id(format!("fn-{label}"));
                let back = if i < referenced {
                    self.unique_id(format!("fnref-{label}"))
                } else {
                    String::new()
                };
                (note, back)
            })
            .collect();
        for (anchor, n) in std::mem::take(&mut self.footnotes.ref_anchors) {
            self.doc.anchors[anchor]
                .0
                .clone_from(&ids[n as usize - 1].1);
        }
        for (link, n) in std::mem::take(&mut self.footnotes.ref_links) {
            self.doc.links[link] = format!("#{}", ids[n as usize - 1].0);
        }
        for (anchor, label) in nested {
            let n = self.footnotes.numbers[&UniCase::new(label)];
            self.doc.anchors[anchor]
                .0
                .clone_from(&ids[n as usize - 1].0);
        }
        if !fn_blocks.is_empty() {
            // List definitions in reference-number order, as GitHub does.
            let ends: Vec<usize> = defs
                .iter()
                .skip(1)
                .map(|d| d.first)
                .chain([fn_blocks.len()])
                .collect();
            let mut groups: Vec<(u32, &FootnoteDef, usize)> = defs
                .iter()
                .zip(ends)
                .map(|(def, end)| {
                    let n = self.footnotes.numbers[&UniCase::new(def.label.clone())];
                    (n, def, end)
                })
                .collect();
            groups.sort_by_key(|&(n, ..)| n);
            // Where each definition block ended up, for anchors that point into them.
            let mut moved = vec![0u32; fn_blocks.len()];
            for (n, def, mut end) in groups {
                let id = n as usize - 1;
                if let Some(back) = &def.back {
                    if id < referenced {
                        let link = push_id(&mut self.doc.links, format!("#{}", ids[id].1));
                        self.doc.spans[back.span].link = link;
                    } else if back.own {
                        end -= 1;
                    } else {
                        fn_blocks[back.block].spans.end -= 1;
                    }
                }
                fn_blocks[def.first].marker = Marker::Footnote(n);
                let block = self.doc.blocks.len() as u32;
                self.doc.anchors.push((ids[id].0.clone(), block));
                for (k, slot) in moved[def.first..end].iter_mut().enumerate() {
                    *slot = block + k as u32;
                }
                self.doc
                    .blocks
                    .extend_from_slice(&fn_blocks[def.first..end]);
            }
            for anchor in std::mem::take(&mut self.footnotes.local) {
                let at = self.doc.anchors[anchor].1 as usize;
                self.doc.anchors[anchor].1 = moved[at];
            }
        }
        self.doc
    }
}

/// Bytes a link or image copies from a reference definition, which count against pulldown-cmark's
/// limit on that.
fn expansion(tag: &Tag) -> usize {
    match tag {
        Tag::Link {
            link_type,
            dest_url,
            title,
            ..
        }
        | Tag::Image {
            link_type,
            dest_url,
            title,
            ..
        } if matches!(
            link_type,
            LinkType::Reference
                | LinkType::Collapsed
                | LinkType::Shortcut
                | LinkType::ReferenceUnknown
                | LinkType::CollapsedUnknown
                | LinkType::ShortcutUnknown
        ) =>
        {
            dest_url.len() + title.len()
        }
        _ => 0,
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

// Cells visited while splitting oversized table rows, on this thread (for tests of its cost).
#[cfg(test)]
thread_local! {
    static ROW_CELL_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A table cell whose text is being cut into pieces: `spans` are its spans, `at..end` the text
/// not placed yet, `next` the first span that may still overlap it.
struct CellRest<'a> {
    col: u32,
    spans: &'a [Span],
    next: usize,
    at: usize,
    end: usize,
}

/// Take the next piece of at most `max` text bytes off `cell` (see [`split_point`] for where it
/// ends) and append its spans to `out`: a span crossing the cut becomes two with the same style,
/// link and image.
fn take_chunk(text: &str, cell: &mut CellRest, max: usize, out: &mut Vec<Span>) {
    let at = cell.at;
    let cut = if cell.end - at > max {
        split_point(text, at, max)
    } else {
        cell.end
    };
    let spans = cell.spans;
    while cell.next < spans.len() && spans[cell.next].end as usize <= at {
        cell.next += 1;
    }
    for span in spans[cell.next..]
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
    cell.at = cut;
}

/// Cut a block's `spans` (contiguous text) into chunks of at most `max` text bytes. Returns the
/// re-cut spans and each chunk's range into them. A cut goes just after the last newline in
/// reach (the newline stays at the end of its chunk, so the line count is unchanged), else
/// after the last whitespace, else at the last char boundary.
fn split_spans(text: &str, spans: &[Span], max: usize) -> (Vec<Span>, Vec<Range<u32>>) {
    let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
        return (Vec::new(), Vec::new());
    };
    let (at, end) = (first.start as usize, last.end as usize);
    let mut out = Vec::with_capacity(spans.len() + end.saturating_sub(at) / max.max(1) + 1);
    let mut chunks = Vec::new();
    let mut cell = CellRest {
        col: 0,
        spans,
        next: 0,
        at,
        end,
    };
    loop {
        let from = out.len() as u32;
        take_chunk(text, &mut cell, max, &mut out);
        chunks.push(from..out.len() as u32);
        if cell.at >= end {
            return (out, chunks);
        }
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
        // The longest entity handled is 10 bytes: look no further than that for the `;`.
        let semi = rest.as_bytes()[..rest.len().min(11)]
            .iter()
            .position(|&b| b == b';');
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
