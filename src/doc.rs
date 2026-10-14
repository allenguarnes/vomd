//! Markdown → Doc IR.
//!
//! The document is parsed once into a flat structure: one text arena, a span pool, and a block
//! list. Nesting (lists, quotes) becomes per-block depth counters and markers instead of a tree,
//! so layout and paint stay linear.

use std::collections::HashMap;
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
    pub link: u16,
    /// 1-based index into [`Doc::images`]; 0 = not an image.
    pub image: u16,
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
        table: u16,
        header: bool,
        first_cell: u32,
        cells: u16,
    },
}

/// Decoration drawn before the first line of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    None,
    Bullet,
    Ordered(u32),
    Task(bool),
    Footnote(u16),
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
    pub marker: Marker,
}

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

    /// Spans of each cell in a table row (empty for other blocks).
    pub fn row_cells(&self, block: &Block) -> impl Iterator<Item = &[Span]> {
        let cells = match block.kind {
            BlockKind::TableRow {
                first_cell, cells, ..
            } => &self.cells[first_cell as usize..first_cell as usize + cells as usize],
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
    numbers: HashMap<String, u16>,
    /// Definition blocks, appended after the main flow at the end.
    blocks: Vec<Block>,
    /// One per definition: (number, label, first index into `blocks`).
    defs: Vec<(u16, String, usize)>,
    /// Labels whose first reference already has a `fnref-` anchor.
    referenced: std::collections::HashSet<String>,
    active: bool,
}

struct Builder {
    doc: Doc,
    open: Option<Open>,
    /// Spans at or after this index may be merged with the next text; protects cell boundaries.
    merge_floor: usize,
    bold: u16,
    italic: u16,
    strike: u16,
    links: Vec<u16>,
    /// Next number for each open list (`None` = bullet list).
    lists: Vec<Option<u64>>,
    quote: u8,
    /// Outermost quote depth started since the last block was pushed (0 = none).
    quote_start: u8,
    marker: Marker,
    /// Open image: (image id, alt text collected so far).
    image: Option<(u16, String)>,
    html_block: Option<String>,
    metadata: bool,
    row: Option<(bool, u32)>,
    footnotes: Footnotes,
    slugs: HashMap<String, u32>,
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
            quote_start: 0,
            marker: Marker::None,
            image: None,
            html_block: None,
            metadata: false,
            row: None,
            footnotes: Footnotes::default(),
            slugs: HashMap::new(),
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
                self.push_block(BlockKind::Rule, 0..0);
            }
            Event::TaskListMarker(checked) => self.marker = Marker::Task(checked),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.open(BlockKind::Paragraph),
            Tag::Heading { level, .. } => self.open(BlockKind::Heading(level as u8)),
            Tag::CodeBlock(_) => self.open(BlockKind::Code),
            Tag::HtmlBlock => {
                self.close();
                self.html_block = Some(String::new());
            }
            Tag::BlockQuote(kind) => {
                self.close();
                self.quote = self.quote.saturating_add(1);
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
                let at = self.footnotes.blocks.len();
                self.footnotes.defs.push((n, label.into_string(), at));
                self.marker = Marker::Footnote(n);
            }
            Tag::Table(aligns) => {
                self.close();
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
                let table = self
                    .doc
                    .tables
                    .len()
                    .saturating_sub(1)
                    .min(u16::MAX as usize) as u16;
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
                let id = push_capped(&mut self.doc.links, dest_url.into_string());
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

    fn link(&self) -> u16 {
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
    fn text(&mut self, s: &str, extra_style: u8, image: u16) {
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
            *cells = (self.doc.cells.len() as u32 - first).min(u16::MAX as u32) as u16;
        }
        let spans = open.span_start..self.doc.spans.len() as u32;
        if kind == BlockKind::Paragraph && spans.is_empty() && self.marker == Marker::None {
            return;
        }
        if let BlockKind::Heading(level) = kind
            && !self.footnotes.active
            && let Some(slug) = self.slug(spans.clone())
        {
            let block = self.doc.blocks.len() as u32;
            self.doc.anchors.push((slug.clone(), block));
            self.doc.headings.push(Heading { level, block, slug });
        }
        self.push_block(kind, spans);
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
        let block = Block {
            kind,
            spans,
            list: self.lists.len().min(u8::MAX as usize) as u8,
            quote: self.quote,
            quote_start: std::mem::take(&mut self.quote_start),
            marker: std::mem::replace(&mut self.marker, Marker::None),
        };
        if self.footnotes.active {
            self.footnotes.blocks.push(block);
        } else {
            self.doc.blocks.push(block);
        }
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
    /// become `-`, duplicates get `-1`, `-2`, ... suffixes.
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
        let n = self.slugs.entry(slug.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            slug = format!("{slug}-{}", *n - 1);
            self.slugs.entry(slug.clone()).or_insert(1);
        }
        Some(slug)
    }

    fn push_image(&mut self, src: String) -> u16 {
        let remote = src.starts_with("http://") || src.starts_with("https://");
        push_capped(&mut self.doc.images, ImageRef { src, remote })
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

    fn footnote_number(&mut self, label: &str) -> u16 {
        let next = self.footnotes.numbers.len() as u16 + 1;
        *self
            .footnotes
            .numbers
            .entry(label.to_owned())
            .or_insert(next)
    }

    fn footnote_ref(&mut self, label: &str) {
        let n = self.footnote_number(label);
        if !self.footnotes.active && self.footnotes.referenced.insert(label.to_owned()) {
            let block = self.doc.blocks.len() as u32;
            self.doc.anchors.push((format!("fnref-{label}"), block));
        }
        let link = push_capped(&mut self.doc.links, format!("#fn-{label}"));
        self.links.push(link);
        self.text(&n.to_string(), style::FOOTREF, 0);
        self.links.pop();
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
        let link = push_capped(&mut self.doc.links, target);
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
                    let src = attr(tag, "src").unwrap_or_default();
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

/// Push and return a 1-based id, or 0 once the u16 id space is exhausted.
fn push_capped<T>(v: &mut Vec<T>, item: T) -> u16 {
    if v.len() >= u16::MAX as usize {
        return 0;
    }
    v.push(item);
    v.len() as u16
}

/// Value of an HTML attribute (quoted or unquoted), case-insensitive name match.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let boundary = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !boundary || !rest.starts_with('=') {
            continue;
        }
        let value_at = tag.len() - rest.len() + 1;
        let value = tag[value_at..].trim_start();
        let (quote, body) = match value.chars().next()? {
            q @ ('"' | '\'') => (Some(q), &value[1..]),
            _ => (None, value),
        };
        let end = match quote {
            Some(q) => body.find(q).unwrap_or(body.len()),
            None => body
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(body.len()),
        };
        return Some(body[..end].to_owned());
    }
    None
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
