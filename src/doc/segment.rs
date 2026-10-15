//! Segmented parsing of large documents.
//!
//! pulldown-cmark builds a tree for its whole input before it emits the first event, so parsing
//! a multi-megabyte document in one go costs time and memory far beyond the Doc IR it produces.
//! A large document is cut into segments of roughly [`super::SEGMENT_BYTES`] that are parsed one
//! after the other into the same builder. The result is identical to one parser over the whole
//! text, because a cut is only made where that provably holds (or not made at all).
//!
//! **Safe cuts.** A cut goes before a line that follows a blank line, starts in column 0 and
//! can't continue a construct before it: nothing but a list marker, `>`, a footnote definition
//! or indentation could continue a list, quote or footnote across the blank line. It must also
//! not fall inside a block that spans blank lines: fenced code, an HTML block that only ends at
//! its closing tag (CommonMark types 1–5), or a YAML metadata block (pulldown-cmark accepts
//! those anywhere). Those are tracked line by line with the same rules as pulldown-cmark
//! (case-sensitive end tags included). Lines that open one of them where it might be inert (a
//! fence inside a list item or after an HTML block, a `---` after a text line) can't be told
//! apart from a real opener, so scanning stops there and the rest of the text is one segment.
//! Everything is conservative: when in doubt, no cut.
//!
//! **Tables.** A table has no blank lines, so a giant one has no safe cut. Instead a segment may
//! end between two rows of a table whose rows all start with `|` in column 0; the next segment
//! starts with the table's header and delimiter rows again ([`Segment::head`]), which the builder
//! drops (see `Builder::continue_table`). The header is checked once with pulldown-cmark itself.
//!
//! **Definitions.** Link reference definitions and footnote definitions may come after their
//! use, in another segment. The scan collects both for the whole text; `Doc::parse_segments`
//! gives every segment's parser the link definitions (broken-link callback) and a stub footnote
//! definition for each label it references. A definition can look like text, or text like a
//! definition (inside an HTML block, after a paragraph line); where the scan can't tell it
//! gives up on segmenting at all, as it does for text with lone CR line endings.

use std::collections::HashSet;
use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag};

/// A part of the text to parse on its own.
pub(super) struct Segment {
    pub range: Range<usize>,
    /// The header and delimiter rows of the table this segment continues, to parse before it.
    pub head: Option<Range<usize>>,
}

pub(super) struct Plan {
    pub segments: Vec<Segment>,
    /// The lines that might be link reference definitions, one group per candidate, as a text
    /// to parse by itself.
    pub definitions: String,
    /// Footnote definition labels (lowercased).
    footnotes: HashSet<String>,
    /// Where leading YAML front matter ends (0 if none).
    pub front_matter_end: usize,
}

impl Plan {
    /// The labels (as spelled in the segment) that the segment references and that some segment
    /// defines. The segment declares them with stub definitions in front of its text, so that
    /// pulldown-cmark sees footnote references, not text, where the definition is in another
    /// segment; the builder drops the stubs' events (they come first). A segment that defines a
    /// label itself gets a stub too, which is harmless.
    pub fn footnote_stubs(&self, src: &str, seg: &Segment) -> Vec<String> {
        let mut stubs: Vec<String> = Vec::new();
        if self.footnotes.is_empty() {
            return stubs;
        }
        let mut seen = HashSet::new();
        let mut rest = &src[seg.range.clone()];
        while let Some(i) = rest.find("[^") {
            rest = &rest[i + 2..];
            let Some(label) = label_at(rest) else {
                continue;
            };
            let key = label.to_lowercase();
            if self.footnotes.contains(&key) && seen.insert(key) {
                stubs.push(label.to_owned());
            }
        }
        stubs
    }
}

/// The footnote label at the start of `s` (up to `]`): letters, digits, `-`, `_` and `.` only,
/// which `Plan::footnote_stubs` can safely write back into a definition.
fn label_at(s: &str) -> Option<&str> {
    let end = s.find(']')?;
    let label = &s[..end];
    let ok = !label.is_empty()
        && label.len() <= 100
        && label
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    ok.then_some(label)
}

/// Where a table started, for cuts between its rows.
struct TableScan {
    /// Header and delimiter lines.
    head: Range<usize>,
    /// Start of the first body line.
    body: usize,
    /// Whether pulldown-cmark really reads `head` as a table header (checked once, lazily).
    valid: Option<bool>,
}

/// Plan the segments of `src`, each about `budget` bytes. `None` when there is nothing to cut.
pub(super) fn plan(src: &str, budget: usize) -> Option<Plan> {
    // A lone CR is a line ending for pulldown-cmark, but not for this scanner.
    let bytes = src.as_bytes();
    if bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
    {
        return None;
    }
    let mut scan = Scan::new(src, budget);
    scan.run();
    scan.finish()
}

struct Scan<'a> {
    src: &'a str,
    budget: usize,
    segments: Vec<Segment>,
    start: usize,
    head: Option<Range<usize>>,
    definitions: String,
    footnotes: HashSet<String>,
    front_matter_end: usize,
    /// Start of the open metadata block.
    meta_start: usize,
    // Line state.
    /// Start of the line after the current one.
    next: usize,
    prev_blank: bool,
    /// Open fenced code block: fence character and length.
    fence: Option<(u8, usize)>,
    /// HTML blocks (types 1–5) that may be open, as the text that ends each. A line that looks
    /// like an opener might be content of a block that is open or of a list item, so every
    /// opener counts until its end text shows up (always an over-estimate).
    html: Vec<&'static str>,
    /// Inside a metadata block.
    meta: bool,
    /// A search for the closing line of a metadata block failed: there are none further on.
    no_meta_close: bool,
    /// Something we can't follow: no more cuts.
    stopped: bool,
    /// A list, quote or footnote definition may be open.
    containers: bool,
    /// A line of the current run of non-blank lines began with `<` (maybe an HTML block).
    html_run: bool,
    /// The previous line ended a fenced or metadata block, so this one starts a run.
    fresh: bool,
    /// The previous line ended an HTML block.
    html_ended: bool,
    table: Option<TableScan>,
    // Link reference definition candidates.
    prev_def: bool,
    def_extra: u8,
    /// The definition's destination is on the next line.
    def_open: bool,
    /// The definition's label isn't closed yet.
    label_open: bool,
    /// The previous line ended its block (a heading, a rule).
    prev_heading: bool,
    /// The previous line was ordinary paragraph text.
    prev_text: bool,
    /// The previous line was indented code.
    indented_code: bool,
    /// A definition appeared where we can't tell if it counts (see `definition_line`).
    unsure_definitions: bool,
}

impl<'a> Scan<'a> {
    fn new(src: &'a str, budget: usize) -> Self {
        Scan {
            src,
            budget,
            segments: Vec::new(),
            start: 0,
            head: None,
            definitions: String::new(),
            footnotes: HashSet::new(),
            front_matter_end: 0,
            meta_start: 0,
            next: 0,
            prev_blank: true,
            fence: None,
            html: Vec::new(),
            meta: false,
            no_meta_close: false,
            stopped: false,
            containers: false,
            html_run: false,
            fresh: false,
            html_ended: false,
            table: None,
            prev_def: false,
            def_extra: 0,
            def_open: false,
            label_open: false,
            prev_heading: false,
            prev_text: false,
            indented_code: false,
            unsure_definitions: false,
        }
    }

    fn run(&mut self) {
        let src = self.src;
        let mut pos = 0;
        while pos < src.len() {
            let end = src[pos..].find('\n').map_or(src.len(), |i| pos + i);
            let next = (end + 1).min(src.len());
            let line = src[pos..end].strip_suffix('\r').unwrap_or(&src[pos..end]);
            self.next = next;
            self.line(pos, line.as_bytes());
            pos = next;
        }
    }

    fn line(&mut self, pos: usize, line: &[u8]) {
        if self.stopped {
            // Nothing the scan tracks can be trusted any more, so every line that looks like a
            // definition is a guess (see `definition_line`).
            let indent = leading_spaces(line);
            let rest = &line[indent.min(line.len())..];
            self.unsure_definitions |= footnote_label(line).is_some()
                || (indent <= 3
                    && rest.first() == Some(&b'[')
                    && rest.get(1) != Some(&b'^')
                    && contains(rest, b"]:"));
            return;
        }
        // Only spaces make a line blank here: pulldown-cmark mishandles a tab-only line after a
        // link reference definition (it leaves a soft break), so nothing is cut after one.
        let blank = line.iter().all(|&b| b == b' ');
        let was_blank = std::mem::replace(&mut self.prev_blank, blank);
        if self.meta {
            self.meta = !is_meta_close(line);
            if !self.meta && self.meta_start == 0 {
                self.front_matter_end = self.next;
            }
            return self.leave_run();
        }
        if let Some((c, n)) = self.fence {
            let indent = leading_spaces(line);
            if indent <= 3 && fence_close(&line[indent..], c, n) {
                self.fence = None;
            }
            return self.leave_run();
        }
        // A cut is decided on the state at the start of the line: a line that ends an HTML block
        // belongs to it.
        let in_html = !self.html.is_empty();
        let cuttable = !in_html && !self.stopped;
        self.html.retain(|end| !contains(line, end.as_bytes()));
        if blank {
            self.table = None;
            self.prev_def = false;
            self.def_extra = 0;
            self.fresh = false;
            return;
        }
        let indent = leading_spaces(line);
        let top = indent <= 3 && line.get(indent) != Some(&b'\t');
        let rest = if top { &line[indent..] } else { &line[..0] };
        // The first line of a run of plain lines: after a blank line, or a block that ended.
        let first = was_blank || std::mem::take(&mut self.fresh);
        if first {
            self.html_run = false;
        }
        // The line after the end of an HTML block starts a block, unless the block wasn't one.
        let after_html = std::mem::replace(&mut self.html_ended, in_html && self.html.is_empty());
        // An HTML block of type 6 or 7 can start wherever a block does (after a heading, a rule,
        // ...), and runs until the next blank line.
        let html_start = top && rest.first() == Some(&b'<');
        self.html_run |= html_start;

        // A YAML metadata block starts with `---` in column 0 (pulldown-cmark accepts it at any
        // block start) and may contain anything, blank lines included. After a blank line, outside
        // HTML blocks, that is certainly a block start; anywhere else the line may be a setext
        // underline or HTML content, and we can't tell.
        if indent == 0 && is_meta_open(line) && self.meta_closes() {
            if was_blank && self.html.is_empty() {
                self.meta = true;
                self.meta_start = pos;
            } else {
                self.stopped = true;
            }
            return self.leave_run();
        }

        let l_start = was_blank && indent == 0 && cut_may_start(line);
        if cuttable && self.size(pos) >= self.budget {
            if l_start {
                self.cut(pos, None);
            } else if indent == 0 && rest.first() == Some(&b'|') {
                self.table_cut(pos);
            }
        }
        if top && starts_container(rest) {
            self.containers = true;
        } else if l_start {
            self.containers = false;
        }

        self.table_line(pos, line, was_blank);
        // Inside an HTML block a definition is just content, but whether we are inside one is a
        // guess: type 6 and 7 blocks (and lazy continuation lines) look alike.
        let unsure =
            !self.prev_def && (in_html || (self.html_run && !first) || (after_html && !first));
        self.definition_line(line, top, rest, first, unsure);
        self.prev_text = top && is_plain_text(rest);
        // Also indented code: an indented line that starts a block or continues code, with no
        // list item possibly open (there it would be a continuation line).
        let code = !top && (first || self.indented_code || self.prev_heading) && !self.containers;
        self.prev_heading = if top {
            is_block_end(rest, indent, self.containers)
        } else {
            code
        };
        self.indented_code = code;

        if !top {
            return;
        }
        if let Some((c, n)) = fence_open(rest) {
            // Whether this line really opens a fence depends on its context: inside a list item,
            // an HTML block or a metadata block it is just content, and a list item's fence
            // ends with the item, so the closing line may as well open the next fence.
            let certain =
                (indent == 0 || !self.containers) && !self.html_run && self.html.is_empty();
            if certain {
                self.fence = Some((c, n));
            } else {
                self.stopped = true;
            }
        } else if rest.first() == Some(&b'<')
            && let Some(end) = html_end_tag(&rest[1..])
            && !contains(rest, end.as_bytes())
            && !self.html.contains(&end)
        {
            self.html.push(end);
        }
    }

    /// A line inside a fence or block ended the current run of plain lines.
    fn leave_run(&mut self) {
        self.html_run = false;
        self.prev_text = false;
        self.indented_code = false;
        self.fresh = true;
        self.prev_heading = false;
        self.table = None;
        self.prev_def = false;
        self.def_extra = 0;
    }

    fn size(&self, pos: usize) -> usize {
        pos - self.start
    }

    fn cut(&mut self, at: usize, head: Option<Range<usize>>) {
        let head = std::mem::replace(&mut self.head, head);
        self.segments.push(Segment {
            range: self.start..at,
            head,
        });
        self.start = at;
    }

    /// Cut before the table row at `pos`, if it is in a table that pulldown-cmark reads from
    /// its header, after at least one body row.
    fn table_cut(&mut self, pos: usize) {
        let src = self.src;
        let Some(table) = self.table.as_mut() else {
            return;
        };
        if pos <= table.body {
            return;
        }
        if *table
            .valid
            .get_or_insert_with(|| valid_head(src, &table.head))
        {
            let head = table.head.clone();
            self.cut(pos, Some(head));
        }
    }

    /// Whether the metadata block opened by the `---` line just read is closed somewhere:
    /// pulldown-cmark ignores the opener otherwise, and when the first line after it is blank or
    /// a closing line.
    fn meta_closes(&mut self) -> bool {
        if self.no_meta_close {
            return false;
        }
        let src = self.src;
        let mut pos = self.next;
        let mut first = true;
        while pos < src.len() {
            let end = src[pos..].find('\n').map_or(src.len(), |i| pos + i);
            let line = src[pos..end].strip_suffix('\r').unwrap_or(&src[pos..end]);
            let close = is_meta_close(line.as_bytes());
            if first && (close || line.bytes().all(|b| matches!(b, b' ' | b'\t'))) {
                return false;
            }
            first = false;
            if close {
                return true;
            }
            pos = end + 1;
        }
        self.no_meta_close = true;
        false
    }

    /// Track a table whose rows all start with `|` in column 0 (so it is not inside a list
    /// item): the header must follow a blank line and be followed by a delimiter row.
    fn table_line(&mut self, pos: usize, line: &[u8], was_blank: bool) {
        let row = line.first() == Some(&b'|');
        if let Some(table) = self.table.as_mut() {
            if pos < table.body {
                // The delimiter row.
            } else if !row {
                self.table = None;
            }
            return;
        }
        if !(was_blank && row && self.html.is_empty()) {
            return;
        }
        let (src, next) = (self.src, self.next);
        let end = src[next..].find('\n').map_or(src.len(), |i| next + i);
        let delim = src[next..end].strip_suffix('\r').unwrap_or(&src[next..end]);
        if is_delimiter_row(delim.as_bytes()) {
            let body = (end + 1).min(src.len());
            self.table = Some(TableScan {
                head: pos..body,
                body,
                valid: None,
            });
        }
    }

    /// Collect lines that may be link reference definitions (`[label]: destination "title"`, the
    /// destination and title possibly on the next lines) and footnote definitions.
    fn definition_line(
        &mut self,
        line: &[u8],
        top: bool,
        rest: &[u8],
        was_blank: bool,
        unsure: bool,
    ) {
        // Nearly every line has no bracket and can't be (part of) a definition.
        if !self.prev_def && !line.contains(&b'[') {
            return;
        }
        let ref_shaped = top && rest.first() == Some(&b'[') && rest.get(1) != Some(&b'^');
        if unsure {
            // A definition here may or may not be one. Segments before it could depend on
            // it, so the text can't be cut at all.
            self.unsure_definitions |=
                footnote_label(line).is_some() || (ref_shaped && contains(rest, b"]:"));
            self.prev_def = false;
            return;
        }
        // A footnote definition can interrupt a paragraph. A link reference definition counts
        // when it surely starts a block: after a blank line, a heading or another definition;
        // anywhere else it may just continue a paragraph.
        if let Some((label, _)) = footnote_label(line) {
            self.footnotes.insert(label.to_lowercase());
        }
        let starts = was_blank || self.prev_def || self.prev_heading;
        // The label may run over several lines, so a line that opens one without closing it
        // counts too.
        let shaped = ref_shaped && (contains(rest, b"]:") || !rest.contains(&b']'));
        let candidate = shaped && starts;
        // Right after paragraph text it continues the paragraph; right after anything else that
        // doesn't end a block, it may or may not be a definition.
        self.unsure_definitions |=
            shaped && !starts && !self.prev_text && !self.prev_def && contains(rest, b"]:");
        let text = std::str::from_utf8(line).unwrap_or("");
        let appended = if candidate {
            if !self.prev_def {
                self.definitions.push('\n');
            }
            self.def_extra = 2;
            self.label_open = !rest.contains(&b']');
            true
        } else if self.prev_def
            && self.def_extra > 0
            && (self.def_open
                || self.label_open
                || matches!(line.trim_ascii_start().first(), Some(b'"' | b'\'' | b'(')))
        {
            self.def_extra -= 1;
            self.label_open &= !line.contains(&b']');
            true
        } else {
            false
        };
        self.prev_def = appended;
        if appended {
            self.definitions.push_str(text);
            self.definitions.push('\n');
            // Without a destination yet, the next line is it.
            self.def_open = !self.label_open && line.trim_ascii_end().ends_with(b":");
        }
    }

    fn finish(mut self) -> Option<Plan> {
        if self.segments.is_empty() || self.unsure_definitions {
            return None;
        }
        self.segments.push(Segment {
            range: self.start..self.src.len(),
            head: self.head.take(),
        });
        Some(Plan {
            segments: self.segments,
            definitions: self.definitions,
            footnotes: self.footnotes,
            front_matter_end: self.front_matter_end,
        })
    }
}

/// Whether pulldown-cmark reads `head` (a header and delimiter row) as a table.
fn valid_head(src: &str, head: &Range<usize>) -> bool {
    let mut parser = Parser::new_ext(&src[head.clone()], super::options());
    matches!(parser.next(), Some(Event::Start(Tag::Table(_))))
}

fn leading_spaces(line: &[u8]) -> usize {
    line.iter().take_while(|&&b| b == b' ').count()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Whitespace as pulldown-cmark's scanners see it (a superset of what we treat as blank).
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// A line starting at column 0 that no list, quote, footnote or indented block before the blank
/// line above it could continue.
fn cut_may_start(line: &[u8]) -> bool {
    match line[0] {
        b' ' | b'\t' | b'>' | b'-' | b'*' | b'+' => false,
        b'[' => line.get(1) != Some(&b'^'),
        b'0'..=b'9' => !is_ordered_marker(line, false),
        _ => true,
    }
}

/// A list item, quote or footnote definition may start with this (indentation already removed).
fn starts_container(rest: &[u8]) -> bool {
    match rest.first() {
        Some(b'>') => true,
        Some(b'-' | b'*' | b'+') => rest.get(1).is_none_or(|&b| b == b' ' || b == b'\t'),
        Some(b'0'..=b'9') => is_ordered_marker(rest, true),
        Some(b'[') => rest.get(1) == Some(&b'^'),
        _ => false,
    }
}

/// `1.` or `1)`; with `spaced`, followed by a space, a tab or the end of the line.
fn is_ordered_marker(line: &[u8], spaced: bool) -> bool {
    let digits = line.iter().take_while(|b| b.is_ascii_digit()).count();
    matches!(line.get(digits), Some(b'.' | b')'))
        && (!spaced
            || line
                .get(digits + 1)
                .is_none_or(|&b| b == b' ' || b == b'\t'))
}

/// An opening fence: three or more backticks (none in the info string) or tildes.
fn fence_open(rest: &[u8]) -> Option<(u8, usize)> {
    let c = *rest.first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = rest.iter().take_while(|&&b| b == c).count();
    if n < 3 || (c == b'`' && rest[n..].contains(&b'`')) {
        return None;
    }
    Some((c, n))
}

/// A closing fence: at least as long as the opening one, then only spaces.
fn fence_close(rest: &[u8], c: u8, len: usize) -> bool {
    let n = rest.iter().take_while(|&&b| b == c).count();
    n >= len && rest[n..].iter().all(|&b| b == b' ')
}

/// Whether the line (after list and quote markers) is ordinary paragraph text.
fn is_plain_text(mut rest: &[u8]) -> bool {
    loop {
        let marker = match rest.first() {
            Some(b'>') => 1,
            Some(b'-' | b'*' | b'+') if rest.get(1).is_none_or(|&b| b == b' ' || b == b'\t') => 1,
            Some(b'0'..=b'9') if is_ordered_marker(rest, true) => {
                rest.iter().take_while(|b| b.is_ascii_digit()).count() + 1
            }
            _ => break,
        };
        rest = &rest[marker..];
        rest = &rest[rest.iter().take_while(|&&b| b == b' ').count()..];
    }
    rest.first()
        .is_some_and(|b| !b"#>-*+=_|<[`~\\ \t".contains(b))
}

/// A line that ends its block by itself: an ATX heading or a thematic break (at column 0 these
/// also end any list item or quote, and a `---` setext underline ends its paragraph the same
/// way). A `===` underline isn't one: without a paragraph above it is text.
fn is_block_end(rest: &[u8], indent: usize, containers: bool) -> bool {
    let certain = indent == 0 || !containers;
    match rest.first() {
        Some(b'#') => {
            let hashes = rest.iter().take_while(|&&b| b == b'#').count();
            certain && hashes <= 6 && rest.get(hashes).is_none_or(|&b| b == b' ' || b == b'\t')
        }
        Some(&c @ (b'-' | b'*' | b'_')) => {
            let marks = rest.iter().filter(|&&b| b != b' ' && b != b'\t').count();
            certain && marks >= 3 && rest.iter().all(|&b| b == c || b == b' ' || b == b'\t')
        }
        _ => false,
    }
}

/// `---` and nothing else but whitespace.
fn is_meta_open(line: &[u8]) -> bool {
    line.starts_with(b"---") && line[3..].iter().all(|&b| is_ws(b))
}

/// `---` or `...` in column 0, then only spaces.
fn is_meta_close(line: &[u8]) -> bool {
    (line.starts_with(b"---") || line.starts_with(b"...")) && line[3..].iter().all(|&b| b == b' ')
}

/// A table delimiter row in column 0: pipes, dashes, colons and spaces, with at least one dash.
fn is_delimiter_row(line: &[u8]) -> bool {
    line.first() == Some(&b'|')
        && line.contains(&b'-')
        && line
            .iter()
            .all(|&b| matches!(b, b'|' | b'-' | b':' | b' ' | b'\t'))
}

/// What ends the HTML block (types 1–5) that a `<` followed by `after` starts, if any.
fn html_end_tag(after: &[u8]) -> Option<&'static str> {
    for (name, end) in [
        ("pre", "</pre>"),
        ("style", "</style>"),
        ("script", "</script>"),
        ("textarea", "</textarea>"),
    ] {
        if after.len() >= name.len()
            && after[..name.len()].eq_ignore_ascii_case(name.as_bytes())
            && after.get(name.len()).is_none_or(|&b| is_ws(b) || b == b'>')
        {
            return Some(end);
        }
    }
    for (start, end) in [("!--", "-->"), ("?", "?>"), ("![CDATA[", "]]>")] {
        if after.starts_with(start.as_bytes()) {
            return Some(end);
        }
    }
    (after.len() > 1 && after[0] == b'!' && after[1].is_ascii_alphabetic()).then_some(">")
}

/// The label of a footnote definition line (`[^label]:`), possibly inside quotes or list items
/// (then the flag is set).
fn footnote_label(line: &[u8]) -> Option<(&str, bool)> {
    let mut rest = line;
    let mut nested = false;
    for _ in 0..6 {
        let indent = leading_spaces(rest).min(3);
        rest = &rest[indent..];
        match rest.first()? {
            b'>' => {
                rest = &rest[1..];
                nested = true;
            }
            b'-' | b'*' | b'+' if rest.get(1) == Some(&b' ') => {
                rest = &rest[2..];
                nested = true;
            }
            b'0'..=b'9' => {
                let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
                if matches!(rest.get(digits), Some(b'.' | b')'))
                    && rest.get(digits + 1) == Some(&b' ')
                {
                    rest = &rest[digits + 2..];
                    nested = true;
                } else {
                    return None;
                }
            }
            _ => break,
        }
    }
    let rest = std::str::from_utf8(rest.strip_prefix(b"[^")?).ok()?;
    let label = label_at(rest)?;
    rest[label.len()..]
        .starts_with("]:")
        .then_some((label, nested))
}
