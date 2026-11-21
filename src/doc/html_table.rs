//! HTML tables (`<table>`), the builder's side: rows become table-row blocks and cells cells, as
//! for a Markdown table, so the layout, find, selection and copy treat both alike. The tags are
//! read the way an HTML parser's "in table" rules have them, simplified: `<td>` without a row
//! opens one, `<tr>` and `</tr>` close the open cell and row, `</table>` everything, and a table
//! nobody closes ends with the container it opened in (or the document).
//!
//! Markdown blocks in a cell (after a blank line) don't open blocks of their own: they go on in
//! the cell, a paragraph after a blank line, an item after its bullet or number, a code block as
//! code, a heading in bold. A table in a cell is flattened into it, a line per row, its cells
//! apart by ` · `. Text in a table but outside its cells is shown after it (GitHub shows it
//! before; the rows of a table are consecutive blocks).

use super::*;

/// The most columns an HTML table has: a cell that would start past them goes on in the last
/// cell of its row. (`colspan` would otherwise make a thousand columns of a few bytes.)
pub const MAX_HTML_COLS: usize = 1000;
/// The most cells spanning rows a table keeps: `rowspan` on one more is 1.
const MAX_ROWSPANS: usize = 1024;
/// The most rows a cell spans (HTML's limit).
const MAX_ROWSPAN: u32 = 65534;

#[derive(Default)]
pub(super) struct HtmlTable {
    /// `Builder::depth` when it opened.
    pub(super) depth: usize,
    /// Its index in `Doc::tables` and its first block, from its first cell on (a table with no
    /// cell leaves no trace).
    table: Option<(usize, usize)>,
    center: bool,
    /// In `<thead>`: the rows are header rows.
    head: bool,
    /// A row's block is open (from its first cell to the row's end).
    pub(super) block: bool,
    /// The open cell (its spans end when it closes).
    pub(super) cell: Option<Cell>,
    /// The next column of the open row.
    col: usize,
    /// The first block of each row so far, from the table's first block.
    rows: Vec<u32>,
    /// Per column: the row a cell spanning rows over it ends before.
    covered: Vec<u32>,
    /// The cells spanning rows: column, columns, first row, rows.
    spans: Vec<[u32; 4]>,
    /// Text outside the cells, shown after the table.
    stray: String,
    /// Tables open in the open cell, and whether their row has had a cell yet.
    nested: u32,
    nested_cell: bool,
    /// Line breaks owed to the open cell's next text (1, or 2 for a blank line).
    sep: u8,
    /// Markdown blocks open in the cell: lists (with the next number of an ordered one), a code
    /// block, and a heading (where its text starts).
    lists: Vec<Option<u64>>,
    code: bool,
    heading: Option<usize>,
    /// Nothing came yet after an item's bullet: a paragraph there (a loose list's) starts on
    /// the bullet's line, as a tight list's text does.
    fresh_item: bool,
}

impl Builder {
    fn table_mut(&mut self) -> Option<&mut HtmlTable> {
        self.html_table.as_deref_mut()
    }

    /// An HTML table has its first cell: what isn't in a cell now is out of place.
    pub(super) fn table_started(&self) -> bool {
        self.html_table.as_ref().is_some_and(|t| t.table.is_some())
    }

    /// Text now is a code block's in a cell.
    pub(super) fn cell_code(&self) -> bool {
        self.html_table
            .as_ref()
            .is_some_and(|t| t.code && t.cell.is_some())
    }

    /// Text now starts a cell, or a line of one.
    pub(super) fn cell_line_start(&self) -> bool {
        let Some(t) = self.html_table.as_deref() else {
            return false;
        };
        t.cell
            .as_ref()
            .is_some_and(|c| t.sep > 0 || c.spans.start as usize == self.doc.spans.len())
    }

    /// Owe the open cell's next text `n` line breaks.
    pub(super) fn cell_break(&mut self, n: u8) {
        if let Some(t) = self.table_mut().filter(|t| t.cell.is_some()) {
            t.sep = t.sep.max(n);
            self.html_inline.gap = None;
        }
    }

    /// Write the line breaks owed to the open cell, if it has text.
    pub(super) fn cell_sep(&mut self) {
        let len = self.doc.spans.len();
        let Some(t) = self.table_mut() else {
            return;
        };
        let Some(cell) = &t.cell else {
            return;
        };
        t.fresh_item = false;
        let n = std::mem::take(&mut t.sep);
        if n > 0 && len > cell.spans.start as usize {
            self.push_text(&"\n\n"[..n.min(2) as usize], 0, 0, 0);
        }
    }

    /// Text outside the cells of a table that has started: kept for after it. True if `s` was.
    pub(super) fn stray(&mut self, s: &str) -> bool {
        let Some(t) = self.table_mut() else {
            return false;
        };
        if t.table.is_none() || t.cell.is_some() {
            return false;
        }
        // (A run of whitespace is a space, and there is none at the start.)
        for c in s.chars() {
            if !c.is_ascii_whitespace() {
                t.stray.push(c);
            } else if !t.stray.is_empty() && !t.stray.ends_with(' ') {
                t.stray.push(' ');
            }
        }
        true
    }

    /// An HTML tag that is the table's: true if it was. A `<table>` starts one (not in a
    /// Markdown table's row, which holds a line of text).
    pub(super) fn html_table_tag(&mut self, name: &str, tag: &str, closing: bool) -> bool {
        // A Markdown table's row (not one of the HTML table's) holds a line of text: the tags in
        // it are no table's, even with an HTML table open around it.
        let ours = self.html_table.as_ref().is_some_and(|t| t.block);
        if self.row.is_some() && !ours {
            return false;
        }
        let Some(t) = self.table_mut() else {
            if name == "table" && !closing {
                self.start_html_table(tag);
                return true;
            }
            return false;
        };
        let in_cell = t.cell.is_some();
        if t.nested > 0 {
            match (name, closing) {
                ("table", false) => t.nested += 1,
                ("table", true) => {
                    t.nested -= 1;
                    t.sep = t.sep.max(1);
                }
                ("tr", false) => {
                    t.nested_cell = false;
                    t.sep = t.sep.max(1);
                }
                ("td" | "th", false) => {
                    if std::mem::replace(&mut t.nested_cell, true) {
                        self.push_text(" · ", 0, 0, 0);
                    }
                }
                (
                    "tr" | "td" | "th" | "thead" | "tbody" | "tfoot" | "caption" | "colgroup"
                    | "col",
                    _,
                ) => {}
                _ => return self.cell_tag(name),
            }
            return true;
        }
        match (name, closing) {
            ("table", false) if in_cell => {
                t.nested = 1;
                t.nested_cell = false;
                t.sep = t.sep.max(1);
            }
            // (A table can't start between the rows of another: it ends that one.)
            ("table", false) => {
                self.end_html_table();
                self.start_html_table(tag);
            }
            ("table", true) => self.end_html_table(),
            ("tr", _) => self.end_html_row(),
            ("td" | "th", false) => self.start_html_cell(name == "th", tag),
            ("td" | "th", true) => self.end_html_cell(),
            ("thead" | "tbody" | "tfoot", _) => {
                self.end_html_row();
                if let Some(t) = self.table_mut() {
                    t.head = name == "thead" && !closing;
                }
            }
            ("caption" | "colgroup" | "col", _) => {}
            _ => return self.cell_tag(name),
        }
        true
    }

    /// In a cell, a block tag is a line break (`<p>`, `<div>`, `<li>`, ...): true if it was. A
    /// `<details>` or `<summary>` anywhere in a table is no more than that (a section in a cell
    /// is shown open).
    fn cell_tag(&mut self, name: &str) -> bool {
        let section = matches!(name, "details" | "summary");
        let block = section || block_tag(name);
        let in_cell = self.html_table.as_ref().is_some_and(|t| t.cell.is_some());
        if block && in_cell {
            self.cell_break(1);
        }
        section || (block && in_cell)
    }

    fn start_html_table(&mut self, tag: &str) {
        self.close();
        let center = attr(tag, "align")
            .is_some_and(|a| decode_entities(&a).trim().eq_ignore_ascii_case("center"));
        self.html_table = Some(Box::new(HtmlTable {
            depth: self.depth,
            center,
            ..HtmlTable::default()
        }));
    }

    /// `<td>` or `<th>` (`th`): the open cell ends, and a row opens if none is.
    fn start_html_cell(&mut self, th: bool, tag: &str) {
        self.end_html_cell();
        if !self.table_started() {
            // The table starts with its first cell: what came before it is a paragraph before it.
            self.close();
            let center = self.html_table.as_ref().is_some_and(|t| t.center);
            self.doc.tables.push(Table {
                center,
                html: true,
                ..Table::default()
            });
            let first = (self.doc.tables.len() - 1, self.block_count());
            if let Some(t) = self.table_mut() {
                t.table = Some(first);
            }
        }
        let cells = self.doc.cells.len() as u32;
        let Some(t) = self.table_mut() else {
            return;
        };
        let Some((table, _)) = t.table else {
            return;
        };
        let opened = !std::mem::replace(&mut t.block, true);
        let head = t.head;
        if opened {
            t.col = 0;
            self.row = Some((head, cells));
            self.begin(BlockKind::TableRow {
                table: table as u32,
                header: head,
                first_cell: 0,
                cells: 0,
            });
        }
        self.end_inline_html();
        let Some(t) = self.html_table.as_deref_mut() else {
            return;
        };
        t.sep = 0;
        t.nested = 0;
        t.fresh_item = false;
        t.lists.clear();
        // The column: the next one no cell above spans into.
        let row = t.rows.len() as u32;
        let mut col = t.col;
        while t.covered.get(col).is_some_and(|&until| until > row) {
            col += 1;
        }
        if col >= MAX_HTML_COLS {
            // Past the last column: the text goes on in the row's last cell, after a space.
            let first = self.row.map_or(0, |r| r.1) as usize;
            if self.doc.cells.len() > first
                && let Some(last) = self.doc.cells.pop()
            {
                self.merge_floor = last.spans.start as usize;
                t.cell = Some(last);
                self.push_text(" ", 0, 0, 0);
                return;
            }
            col = MAX_HTML_COLS - 1;
        }
        // The columns past its first come out of the document's allowance (see
        // `Builder::spare_cols`), so a few bytes of `colspan` can't make a thousand columns.
        let wanted = integer(tag, "colspan").filter(|&n| n > 0).unwrap_or(1) as usize;
        let more = (wanted.min(MAX_HTML_COLS - col) - 1).min(self.spare_cols);
        self.spare_cols -= more;
        let span = 1 + more;
        let rows = match integer(tag, "rowspan") {
            // (0: to the end of the table.)
            Some(0) => MAX_ROWSPAN,
            Some(n) => n.min(MAX_ROWSPAN),
            None => 1,
        };
        if rows > 1 && t.spans.len() < MAX_ROWSPANS {
            t.spans.push([col as u32, span as u32, row, rows]);
            if t.covered.len() < col + span {
                t.covered.resize(col + span, 0);
            }
            t.covered[col..col + span].fill(row.saturating_add(rows));
        }
        t.col = col + span;
        let value = |name| attr(tag, name).map(|v| decode_entities(&v).trim().to_ascii_lowercase());
        let align = match value("align").as_deref() {
            Some("left") => Align::Left,
            Some("center") => Align::Center,
            Some("right") => Align::Right,
            _ => Align::None,
        };
        let flags = match value("valign").as_deref() {
            Some("top" | "baseline") => Cell::TOP,
            Some("bottom") => Cell::BOTTOM,
            _ => 0,
        } | if th { Cell::TH } else { 0 };
        let width = attr(tag, "width")
            .filter(|_| span == 1)
            .map_or(Want::Auto, |w| Want::parse(&decode_entities(&w), true));
        let tbl = &mut self.doc.tables[table];
        if tbl.aligns.len() < col + span {
            tbl.aligns.resize(col + span, Align::None);
        }
        if width != Want::Auto {
            if tbl.widths.len() <= col {
                tbl.widths.resize(col + 1, Want::Auto);
            }
            // The largest a column's cells ask for.
            let w = &mut tbl.widths[col];
            *w = match (*w, width) {
                (Want::Px(a), Want::Px(b)) => Want::Px(a.max(b)),
                (Want::Pct(a), Want::Pct(b)) => Want::Pct(a.max(b)),
                (Want::Auto, b) => b,
                (a, _) => a,
            };
        }
        let at = self.doc.spans.len() as u32;
        self.merge_floor = at as usize;
        if let Some(t) = self.table_mut() {
            t.cell = Some(Cell {
                col: col as u32,
                spans: at..at,
                span: span as u16,
                align,
                flags,
            });
        }
    }

    /// The open cell ends (`</td>`, or another cell, row or the table's end).
    pub(super) fn end_html_cell(&mut self) {
        let len = self.doc.spans.len();
        let Some(t) = self.table_mut() else {
            return;
        };
        let Some(mut cell) = t.cell.take() else {
            return;
        };
        t.nested = 0;
        t.code = false;
        // A heading the cell ended in the middle of is bold no more.
        let heading = t.heading.take().is_some();
        if heading {
            self.bold = self.bold.saturating_sub(1);
        }
        cell.spans.end = len as u32;
        self.doc.cells.push(cell);
        self.merge_floor = len;
        // An unclosed tag in a cell doesn't go on after it.
        self.end_inline_html();
    }

    /// The open row ends (`<tr>`, `</tr>`, a row group, the table's end): its block is pushed.
    fn end_html_row(&mut self) {
        self.end_html_cell();
        let at = self.block_count();
        let Some(t) = self.table_mut() else {
            return;
        };
        t.col = 0;
        if !std::mem::take(&mut t.block) {
            return;
        }
        let first = t.table.map_or(0, |(_, first)| first);
        t.rows.push((at - first) as u32);
        self.close();
    }

    /// The table ends: its cells spanning rows are kept as blocks, and its stray text is a
    /// paragraph after it.
    pub(super) fn end_html_table(&mut self) {
        self.end_html_row();
        let Some(t) = self.html_table.take() else {
            return;
        };
        if let Some((table, first)) = t.table {
            let total = (self.block_count() - first) as u32;
            let rowspans = t.spans.iter().filter_map(|&[col, cols, row, n]| {
                let first = *t.rows.get(row as usize)?;
                let end = (t.rows.get(row.saturating_add(n) as usize)).map_or(total, |&e| e);
                Some(RowSpan {
                    col,
                    cols,
                    first,
                    end,
                })
            });
            self.doc.tables[table].rowspans = rowspans.collect();
        }
        let stray = t.stray.trim_end();
        if !stray.is_empty() {
            self.close();
            self.begin(BlockKind::Paragraph);
            self.push_text(stray, 0, 0, 0);
            self.close();
        }
        // Lists, items and quotes a cell took that are still open go on as Markdown's (as the
        // HTML parser leaves the rest of a list whose start was in a cell), an ordered one at the
        // number the cell came to.
        let mut numbers = t.lists.iter();
        for i in 0..self.taken.len() {
            match self.taken[i] {
                Taken::List(start) => self.lists.push(numbers.next().map_or(start, |&n| n)),
                Taken::Item => self.depth += 1,
                Taken::Quote => {
                    self.quote = self.quote.saturating_add(1);
                    let depth = self.lists.len().min(u8::MAX as usize) as u8;
                    self.quote_lists.push(depth);
                }
                _ => continue,
            }
            self.taken[i] = Taken::No;
        }
    }

    /// A Markdown block starting while an HTML table is open: in a cell it goes on in the cell;
    /// between rows a paragraph is stray text, and anything else ends the table. `None` when
    /// it was taken care of.
    pub(super) fn table_start<'a>(&mut self, tag: Tag<'a>) -> Option<Tag<'a>> {
        let Some(t) = self.html_table.as_deref_mut() else {
            return Some(tag);
        };
        let passes = matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::Superscript
                | Tag::Subscript
                | Tag::HtmlBlock
                | Tag::FootnoteDefinition(_)
        );
        if passes || t.table.is_none() {
            return Some(tag);
        }
        if t.cell.is_none() {
            if tag == Tag::Paragraph {
                return None;
            }
            self.end_html_table();
            return Some(tag);
        }
        match tag {
            Tag::Paragraph if t.fresh_item => {}
            Tag::Paragraph | Tag::BlockQuote(_) => t.sep = t.sep.max(2),
            Tag::CodeBlock(_) => {
                t.code = true;
                t.sep = t.sep.max(2);
            }
            Tag::Heading { .. } => {
                t.sep = t.sep.max(2);
                self.cell_sep();
                self.bold += 1;
                let at = self.doc.spans.len();
                if let Some(t) = self.table_mut() {
                    t.heading = Some(at);
                }
            }
            Tag::List(start) => {
                // (A list is a block apart, a nested one a line of its item.)
                t.sep = t.sep.max(if t.lists.is_empty() { 2 } else { 1 });
                t.lists.push(start);
            }
            Tag::Item => {
                t.sep = t.sep.max(1);
                // Three no-break spaces (two bytes each) per level of nesting.
                const NBSP: &str =
                    "\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}";
                let indent = &NBSP[..t.lists.len().saturating_sub(1).min(4) * 6];
                let mark = match t.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{indent}{}. ", *n - 1)
                    }
                    _ => format!("{indent}• "),
                };
                self.push_text(&mark, 0, 0, 0);
                if let Some(t) = self.table_mut() {
                    t.fresh_item = true;
                }
            }
            Tag::Table(_) => {
                // A repeat of a header that the segments cut (see `Doc::segments`).
                if std::mem::take(&mut self.continue_table) {
                    self.skip_head = true;
                }
                t.nested += 1;
                t.nested_cell = false;
                t.sep = t.sep.max(1);
            }
            Tag::TableHead | Tag::TableRow => {
                t.nested_cell = false;
                t.sep = t.sep.max(1);
            }
            Tag::TableCell if std::mem::replace(&mut t.nested_cell, true) => {
                self.push_text(" · ", 0, 0, 0);
            }
            _ => {}
        }
        None
    }

    /// The end of a Markdown block a cell took (see `table_start`): what it does to the cell, if
    /// the cell is still open (`</td>` may have come inside the block).
    pub(super) fn cell_end(&mut self, tag: TagEnd) {
        let Some(t) = self.html_table.as_deref_mut().filter(|t| t.cell.is_some()) else {
            return;
        };
        match tag {
            TagEnd::Heading(_) => {
                let Some(start) = t.heading.take() else {
                    return;
                };
                t.sep = t.sep.max(2);
                self.bold = self.bold.saturating_sub(1);
                // Its anchor is the row's.
                let (slug, _) = self.slug(start as u32..self.doc.spans.len() as u32);
                if !slug.is_empty() {
                    if self.footnotes.active {
                        self.footnotes.local.push(self.doc.anchors.len());
                    }
                    let at = self.block_count() as u32;
                    self.doc.anchors.push((slug, at));
                }
            }
            TagEnd::CodeBlock => {
                t.code = false;
                t.sep = t.sep.max(2);
                let start = t.cell.as_ref().map_or(0, |c| c.spans.start as usize);
                self.trim_trailing_newline(start);
            }
            TagEnd::BlockQuote(_) => t.sep = t.sep.max(2),
            TagEnd::List(_) => {
                t.lists.pop();
                t.sep = t.sep.max(1);
            }
            TagEnd::Table => {
                t.nested = t.nested.saturating_sub(1);
                t.sep = t.sep.max(1);
            }
            _ => {}
        }
    }
}

/// A Markdown block a cell may take, and whether it did (see `Builder::taken`): the containers
/// are told apart, as they go on as Markdown's if the table ends inside them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Taken {
    No,
    Block,
    List(Option<u64>),
    Item,
    Quote,
    Table,
}

impl Taken {
    /// What taking `tag` would be; `None` for a tag no cell takes.
    pub(super) fn of(tag: &Tag) -> Option<Taken> {
        Some(match tag {
            Tag::List(start) => Taken::List(*start),
            Tag::Item => Taken::Item,
            Tag::BlockQuote(_) => Taken::Quote,
            Tag::Table(_) => Taken::Table,
            Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::CodeBlock(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell => Taken::Block,
            _ => return None,
        })
    }
}

/// The ends of [`takeable`] blocks.
pub(super) fn takeable_end(tag: &TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
    )
}

/// An attribute's value as HTML reads a non-negative integer: the digits it starts with.
fn integer(tag: &str, name: &str) -> Option<u32> {
    let v = attr(tag, name)?;
    let v = v.trim_ascii_start();
    let digits = v.bytes().take_while(u8::is_ascii_digit).count();
    let n: u64 = v[..digits.min(18)].parse().ok()?;
    Some(n.min(u32::MAX as u64) as u32)
}
