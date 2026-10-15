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
//! not fall inside a block that spans blank lines: fenced code, or an HTML block that only ends
//! at its closing tag (CommonMark types 1–5). Those are tracked line by line with the same rules
//! as pulldown-cmark (case-sensitive end tags included, and its whitespace: tab, VT and FF blank
//! a line too). (YAML front matter is not a block pulldown-cmark knows: `Doc` cuts it off the
//! start of the text before parsing, so a `---` here is only ever a rule or a setext underline.)
//!
//! **Ambiguity.** Some lines open one of those blocks only in some contexts: a fence inside a
//! list item or an HTML block. The scan can't always tell, so it follows every reading at once (a handful of states) and cuts only where all of them agree
//! that nothing is open. A fence in a list item, for example, is either a real fence or part of
//! the item (which ends at the next column-0 line); once its closing line is seen both readings
//! agree again and cutting resumes. Only a pile-up of readings (more than [`MAX_STATES`]) makes
//! the scan stop for good.
//!
//! **Tables.** A table has no blank lines, so a giant one has no safe cut. Instead a segment may
//! end between two rows of a table whose rows all start with `|` in column 0; the next segment
//! starts with the table's header and delimiter rows again ([`Segment::head`]), which the builder
//! drops (see `Builder::continue_table`). The header is checked once with pulldown-cmark itself.
//! A table whose short rows would make pulldown-cmark end it partway (it only fills in so many
//! empty cells) is not cut: segments would count afresh and go on. Nor is a table cut before
//! the segment is [`TABLE_HEAD_RATIO`] times its header, so repeating a huge header stays linear.
//!
//! **Definitions.** Link reference definitions and footnote definitions may come after their
//! use, in another segment. They are not guessed from the text: [`Plan::definitions`] parses
//! each segment that could hold one (it has `]:` in it) and takes the definitions from the
//! parser itself, so containers, multi-line labels and titles and every other quirk of the
//! grammar are covered. `Doc::parse_segments` then gives every segment's parser the link
//! definitions (broken-link callback) and a stub footnote definition for each defined label the
//! segment refers to. A label that two segments define differently makes the planner give up
//! (the first definition must win, and a segment's own would shadow it), as does text with lone
//! CR line endings.

use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};
use unicase::UniCase;

/// Most readings of the text the scan follows at once. More than that (a stack of lines that are
/// each ambiguous in a new way) and it stops cutting.
const MAX_STATES: usize = 16;

/// How many cells of short rows pulldown-cmark fills in for one table before it ends the table
/// there (its `MAX_AUTOCOMPLETED_CELLS`).
const MAX_AUTOCOMPLETED_CELLS: usize = 1 << 18;

/// A segment cut inside a table is at least this many times as long as the table's header, which
/// the next segment repeats: a giant header costs at most `1 / TABLE_HEAD_RATIO` of the text in
/// extra parsing, however often it is repeated.
const TABLE_HEAD_RATIO: usize = 4;

/// A part of the text to parse on its own.
pub(super) struct Segment {
    pub range: Range<usize>,
    /// The header and delimiter rows of the table this segment continues, to parse before it.
    pub head: Option<Range<usize>>,
}

pub(super) struct Plan {
    pub segments: Vec<Segment>,
}

/// The definitions of the whole text, from every segment, as pulldown-cmark reads them.
#[derive(Default)]
pub(super) struct Definitions {
    /// Link reference definitions by label (matched like pulldown-cmark does, case-folded):
    /// destination and title of the first one in the text.
    pub links: HashMap<UniCase<String>, (String, String)>,
    /// Footnote labels, as the first definition spells them.
    footnotes: HashMap<UniCase<String>, String>,
}

impl Plan {
    /// The text to parse for segment `n`: the segment itself, preceded by the header of the table
    /// it continues, and by the footnote `stubs` (labels defined in other segments, which this
    /// one's parser needs to see footnote references where they are, not text). The builder
    /// drops the events of the stubs, which come first. A reference definition ends the last stub (indented text after it would continue
    /// it) and defines nothing anyone refers to.
    pub fn text<'a>(&self, src: &'a str, n: usize, stubs: &[String]) -> Cow<'a, str> {
        let seg = &self.segments[n];
        let body = &src[seg.range.clone()];
        if seg.head.is_none() && stubs.is_empty() {
            return Cow::Borrowed(body);
        }
        let head = seg.head.clone().map_or("", |h| &src[h]);
        let mut text = String::with_capacity(head.len() + body.len() + 16 * stubs.len());
        for label in stubs {
            text.push_str("[^");
            text.push_str(label);
            text.push_str("]: x\n");
        }
        if !stubs.is_empty() {
            text.push_str("\n[\u{1}]: x\n\n");
        }
        text.push_str(head);
        text.push_str(body);
        Cow::Owned(text)
    }

    /// Every link reference definition and footnote definition in the text, in document order
    /// (the first of a label counts), collected by parsing each segment that has `]:` in it
    /// (every definition has) on its own. `None` when a label is defined in several segments
    /// with different destinations or titles: one parser would use the first, but a segment's
    /// own definition shadows what is declared to its parser from outside.
    pub fn definitions(&self, src: &str, opts: Options) -> Option<Definitions> {
        let mut defs = Definitions::default();
        for (n, seg) in self.segments.iter().enumerate() {
            let body = &src[seg.range.clone()];
            let (links, notes) = may_define(body);
            if !links {
                continue;
            }
            let text = self.text(src, n, &[]);
            // Building the parser finds the link definitions; the footnote definitions only show
            // in the events.
            let mut parser = Parser::new_ext(&text, opts);
            if notes {
                let mut open = 0i64;
                for event in parser.by_ref() {
                    match event {
                        Event::Start(Tag::FootnoteDefinition(label)) => {
                            open += 1;
                            let label = label.into_string();
                            defs.footnotes
                                .entry(UniCase::new(label.clone()))
                                .or_insert(label);
                        }
                        Event::Start(_) => open += 1,
                        Event::End(_) => open -= 1,
                        _ => {}
                    }
                }
                // The events ended early (see `Doc::parse_segments`): the definitions seen are
                // not all there are.
                if open != 0 {
                    return None;
                }
            }
            for (label, def) in parser.reference_definitions().iter() {
                let value = (
                    def.dest.to_string(),
                    def.title.as_deref().unwrap_or("").to_owned(),
                );
                match defs.links.entry(UniCase::new(label.to_owned())) {
                    Entry::Occupied(first) if *first.get() != value => return None,
                    Entry::Occupied(_) => {}
                    Entry::Vacant(slot) => {
                        slot.insert(value);
                    }
                }
            }
        }
        Some(defs)
    }
}

impl Definitions {
    /// The defined footnote labels (as their definitions spell them) that `text`, a segment,
    /// may refer to: the stubs to declare in front of it. A segment that defines a label itself
    /// gets a stub for it too, which is harmless.
    pub fn footnote_stubs(&self, text: &str) -> Vec<String> {
        let mut stubs: Vec<String> = Vec::new();
        if self.footnotes.is_empty() {
            return stubs;
        }
        let mut seen = HashSet::new();
        footnote_references(text, |label| {
            if let Some(spelled) = self.footnotes.get(&UniCase::new(label))
                && seen.insert(spelled.as_str())
            {
                stubs.push(spelled.clone());
            }
        });
        stubs
    }
}

/// Calls `found` with the label of every `[^label]` in `text` that might be a footnote
/// reference, as pulldown-cmark would normalize it: the part between the brackets, with runs of
/// whitespace collapsed to one space and trimmed. In a table cell `\|` stands for `|`, so a label
/// with one is reported both ways. This only picks which definitions a segment needs declared;
/// extra candidates are harmless. Returns the bytes looked at (more than `text.len()` only if
/// the scan were not linear, which tests check).
pub(super) fn footnote_references(text: &str, mut found: impl FnMut(String)) -> usize {
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut scanned = 0;
    while let Some(i) = text[at..].find("[^") {
        let start = at + i + 2;
        // The label ends at the first unescaped `]`. A `[` or a line break ends the search: the
        // text is no label, and a `[` may start the next one, so this never looks at a byte twice.
        let mut j = start;
        let closed = loop {
            match bytes.get(j) {
                None | Some(b'[' | b'\n' | b'\r') => break false,
                Some(b']') => break true,
                Some(b'\\') if bytes.get(j + 1).is_some_and(u8::is_ascii_punctuation) => j += 2,
                _ => j += 1,
            }
        };
        scanned += j.min(bytes.len()) - at;
        at = if closed { j + 1 } else { j.min(bytes.len()) };
        if closed && j > start {
            let raw = &text[start..j];
            found(normalize_label(raw));
            if raw.contains("\\|") {
                found(normalize_label(&raw.replace("\\|", "|")));
            }
        }
    }
    scanned
}

/// pulldown-cmark's label normalization: runs of whitespace become one space, both ends are
/// trimmed.
fn normalize_label(raw: &str) -> String {
    let mut label = String::with_capacity(raw.len());
    for word in raw.split([' ', '\t', '\u{b}', '\u{c}']) {
        if !word.is_empty() {
            if !label.is_empty() {
                label.push(' ');
            }
            label.push_str(word);
        }
    }
    label
}

/// Where a table started, for cuts between its rows.
#[derive(Clone, PartialEq, Eq)]
struct TableScan {
    /// Header and delimiter lines.
    head: Range<usize>,
    /// Start of the first body line.
    body: usize,
    /// The table's rows don't leave so many cells empty that pulldown-cmark gives up on it
    /// partway (see [`MAX_AUTOCOMPLETED_CELLS`]), which segments would do at other rows.
    cuttable: bool,
}

/// A fenced code block the scan believes may be open.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Fence {
    /// Fence character and length.
    c: u8,
    n: usize,
    /// For a fence that might be inside a list item or quote: the indentation of its opening
    /// line. Such a fence ends with the item, at a line that is indented less.
    container: Option<usize>,
}

/// One reading of the text so far: what is open, if the ambiguous lines were taken this way.
#[derive(Clone, PartialEq, Eq)]
struct State {
    fence: Option<Fence>,
    /// HTML blocks (types 1–5) that may be open, as the text that ends each. A line that looks
    /// like an opener might be content of a block that is open or of a list item, so every
    /// opener counts until its end text shows up (always an over-estimate).
    html: Vec<&'static str>,
    /// A list, quote or footnote definition may be open.
    containers: bool,
    /// A line of the current run of non-blank lines began with `<` (maybe an HTML block).
    html_run: bool,
    /// A line of the current run has `]:` in it: it may hold a link reference definition.
    def_run: bool,
    /// The previous line ended a fenced block, so this one starts a run.
    fresh: bool,
    table: Option<TableScan>,
}

impl State {
    /// A line inside a fence ended the current run of plain lines.
    fn leave_run(&mut self) {
        self.html_run = false;
        self.def_run = false;
        self.fresh = true;
        self.table = None;
    }
}

/// What the line being read means for cutting, over all readings.
struct Verdict {
    /// No reading has anything open before the line.
    plain: bool,
    /// Every reading is in the same table, which pulldown-cmark can start from its header.
    table: Option<Range<usize>>,
    table_ok: bool,
}

impl Verdict {
    fn block(&mut self) {
        self.plain = false;
        self.table_ok = false;
    }

    fn table(&mut self, head: Option<Range<usize>>) {
        match (head, &self.table) {
            (Some(head), None) if self.table_ok => self.table = Some(head),
            (Some(head), Some(first)) if *first == head => {}
            _ => self.table_ok = false,
        }
    }
}

/// The line being read.
struct Line<'l> {
    pos: usize,
    bytes: &'l [u8],
    blank: bool,
    was_blank: bool,
    indent: usize,
    /// Whitespace that only some scanners of pulldown-cmark take for blank (see `is_blank_ws`):
    /// whether it separates blocks depends on what comes before it.
    odd_blank: bool,
    /// Indented at most 3 spaces: may start a block at the top level.
    top: bool,
    /// The line without its indentation, if `top`.
    rest: &'l [u8],
    /// A table row: starts with `|` in column 0 and has a cell (a lone `|` ends a table).
    row: bool,
    /// A segment may start here: after a blank line, in column 0, with a line that can't
    /// continue a construct before it.
    l_start: bool,
    /// The segment is big enough to cut.
    want_cut: bool,
}

/// Plan the segments of `src`, each about `budget` bytes. `None` when there is nothing to cut.
pub(super) fn plan(src: &str, budget: usize) -> Option<Plan> {
    // A lone CR is a line ending for pulldown-cmark, but not for this scanner.
    let bytes = src.as_bytes();
    if bytes.contains(&b'\r')
        && bytes
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
    // Line state.
    /// Start of the line after the current one.
    next: usize,
    prev_blank: bool,
    /// Every reading of the text so far (never empty until the scan stops).
    states: Vec<State>,
    scratch: Vec<State>,
    /// Too many readings to follow: no more cuts.
    stopped: bool,
    /// Whether the header at this start offset is a table for pulldown-cmark (checked lazily).
    valid_head: Option<(usize, bool)>,
}

impl<'a> Scan<'a> {
    fn new(src: &'a str, budget: usize) -> Self {
        Scan {
            src,
            budget,
            segments: Vec::new(),
            start: 0,
            head: None,
            next: 0,
            prev_blank: true,
            states: vec![State {
                fence: None,
                html: Vec::new(),
                containers: false,
                html_run: false,
                def_run: false,
                fresh: false,
                table: None,
            }],
            scratch: Vec::new(),
            stopped: false,
            valid_head: None,
        }
    }

    fn run(&mut self) {
        let src = self.src;
        let mut pos = 0;
        while pos < src.len() && !self.stopped {
            let end = src[pos..].find('\n').map_or(src.len(), |i| pos + i);
            let next = (end + 1).min(src.len());
            let line = src[pos..end].strip_suffix('\r').unwrap_or(&src[pos..end]);
            self.next = next;
            self.line(pos, line.as_bytes());
            pos = next;
        }
    }

    fn line(&mut self, pos: usize, bytes: &[u8]) {
        let blank = is_blank(bytes);
        let was_blank = std::mem::replace(&mut self.prev_blank, blank);
        let indent = leading_spaces(bytes);
        let top = indent <= 3 && bytes.get(indent) != Some(&b'\t');
        let line = Line {
            pos,
            bytes,
            blank,
            was_blank,
            indent,
            odd_blank: !blank && is_blank_ws(bytes),
            top,
            rest: if top { &bytes[indent..] } else { &bytes[..0] },
            row: bytes.first() == Some(&b'|') && !is_blank_ws(&bytes[1..]),
            l_start: !blank && was_blank && indent == 0 && cut_may_start(bytes),
            want_cut: pos - self.start >= self.budget,
        };
        let mut verdict = Verdict {
            plain: true,
            table: None,
            table_ok: true,
        };
        let mut states = std::mem::take(&mut self.states);
        let mut next = std::mem::take(&mut self.scratch);
        next.clear();
        for state in states.drain(..) {
            self.step(state, &line, &mut next, &mut verdict);
        }
        // Readings that came out the same are one.
        let mut i = 1;
        while i < next.len() {
            if next[..i].contains(&next[i]) {
                next.swap_remove(i);
            } else {
                i += 1;
            }
        }
        if next.len() > MAX_STATES {
            self.stopped = true;
        }
        self.states = next;
        self.scratch = states;
        if blank || self.stopped || !line.want_cut {
            return;
        }
        if line.l_start && verdict.plain {
            self.cut(pos, None);
        } else if let Some(head) = verdict
            .table
            .filter(|h| verdict.table_ok && pos - self.start >= TABLE_HEAD_RATIO * h.len())
        {
            self.cut(pos, Some(head));
        }
    }

    /// Read `line` in one reading of the text, pushing the readings it leads to (several when
    /// the line is ambiguous) to `out` and what it says about cutting to `verdict`.
    fn step(&mut self, mut st: State, line: &Line, out: &mut Vec<State>, verdict: &mut Verdict) {
        if let Some(fence) = st.fence {
            match fence.container {
                None => {
                    if line.indent <= 3 && fence_close(&line.bytes[line.indent..], fence.c, fence.n)
                    {
                        st.fence = None;
                    }
                    verdict.block();
                    st.leave_run();
                    return out.push(st);
                }
                Some(open) => {
                    // The fence ends with its container, at a line indented less than the
                    // container's content. That is certain for a line indented under 2 (or more
                    // than 3 less than the fence), and certainly not for one indented as much as
                    // the fence; in between, both readings go on.
                    let width = indent_width(line.bytes);
                    let alive = line.blank || (width >= 2 && width + 3 >= open);
                    let certain = line.blank || width >= open;
                    if alive {
                        let mut kept = st.clone();
                        if !line.blank && fence_close(trim_indent(line.bytes), fence.c, fence.n) {
                            kept.fence = None;
                        }
                        verdict.block();
                        kept.leave_run();
                        out.push(kept);
                        if certain {
                            return;
                        }
                    }
                    st.fence = None;
                }
            }
        }
        if line.odd_blank {
            // Such a line is blank at the start of a block and in an HTML block of type 6 or 7,
            // but text within a paragraph: no telling which, so no more cuts.
            self.stopped = true;
        }
        let in_html = !st.html.is_empty();
        st.html.retain(|end| !contains(line.bytes, end.as_bytes()));
        if line.blank {
            // Pulldown-cmark mishandles a line of only whitespace right after a link reference
            // definition: it is part of the paragraph (a break), and in a list item it can cut
            // the rest of the text off. Nothing can be said about what follows, so no more cuts.
            if st.def_run && !line.bytes.is_empty() {
                self.stopped = true;
            }
            st.def_run = false;
            st.table = None;
            st.fresh = false;
            return out.push(st);
        }
        st.def_run |= has_ref_end(line.bytes);
        if in_html {
            verdict.block();
        }
        // The first line of a run of plain lines: after a blank line, or a block that ended.
        let first = line.was_blank || std::mem::take(&mut st.fresh);
        if first {
            st.html_run = false;
        }
        // An HTML block of type 6 or 7 can start wherever a block does (after a heading, a rule,
        // ...), and runs until the next blank line.
        st.html_run |= line.top && line.rest.first() == Some(&b'<');

        if line.want_cut && line.row {
            let head = (!in_html).then(|| self.table_head(&st, line.pos)).flatten();
            verdict.table(head);
        }
        if line.top && starts_container(line.rest) {
            st.containers = true;
        } else if line.l_start {
            st.containers = false;
        }

        self.table_line(&mut st, line);
        if !line.top {
            return out.push(st);
        }
        if let Some((c, n)) = fence_open(line.rest) {
            // Whether this line really opens a fence depends on its context. Inside a list item
            // it opens one that ends with the item; inside an HTML block it is just content;
            // otherwise it is a fence of the top level. Each reading that is possible goes on.
            let in_item = line.indent > 0 && st.containers;
            let in_html = st.html_run || !st.html.is_empty();
            if in_item {
                let mut item = st.clone();
                item.fence = Some(Fence {
                    c,
                    n,
                    container: Some(line.indent),
                });
                out.push(item);
            }
            if in_html {
                out.push(st.clone());
            }
            st.fence = Some(Fence {
                c,
                n,
                container: None,
            });
        } else if line.rest.first() == Some(&b'<')
            && let Some(end) = html_end_tag(&line.rest[1..])
            && !contains(line.rest, end.as_bytes())
            && !st.html.contains(&end)
        {
            st.html.push(end);
        }
        out.push(st);
    }

    fn cut(&mut self, at: usize, head: Option<Range<usize>>) {
        let head = std::mem::replace(&mut self.head, head);
        self.segments.push(Segment {
            range: self.start..at,
            head,
        });
        self.start = at;
    }

    /// The header to repeat for a cut before the table row at `pos`, if `st` is in a table that
    /// pulldown-cmark reads from its header, after at least one body row.
    fn table_head(&mut self, st: &State, pos: usize) -> Option<Range<usize>> {
        let table = st.table.as_ref()?;
        if pos <= table.body || !table.cuttable {
            return None;
        }
        let valid = match self.valid_head {
            Some((start, valid)) if start == table.head.start => valid,
            _ => {
                let valid = valid_head(self.src, &table.head);
                self.valid_head = Some((table.head.start, valid));
                valid
            }
        };
        valid.then(|| table.head.clone())
    }

    /// Track a table whose rows all start with `|` in column 0 (so it is not inside a list
    /// item): the header must follow a blank line and be followed by a delimiter row.
    fn table_line(&mut self, st: &mut State, line: &Line) {
        if let Some(table) = st.table.as_ref() {
            if line.pos >= table.body && !line.row {
                st.table = None;
            }
            return;
        }
        if !(line.was_blank && line.row && st.html.is_empty()) {
            return;
        }
        let (src, next) = (self.src, self.next);
        let end = src[next..].find('\n').map_or(src.len(), |i| next + i);
        let delim = src[next..end].strip_suffix('\r').unwrap_or(&src[next..end]);
        if is_delimiter_row(delim.as_bytes()) {
            let body = (end + 1).min(src.len());
            st.table = Some(TableScan {
                head: line.pos..body,
                body,
                cuttable: sparse_rows_fit(src, delim.as_bytes(), body),
            });
        }
    }

    fn finish(mut self) -> Option<Plan> {
        if self.segments.is_empty() {
            return None;
        }
        self.segments.push(Segment {
            range: self.start..self.src.len(),
            head: self.head.take(),
        });
        Some(Plan {
            segments: self.segments,
        })
    }
}

/// Whether the rows of the table whose body starts at `from` (rows start with `|`; a lone one
/// ends the table) fill no more than [`MAX_AUTOCOMPLETED_CELLS`] cells in all, going by how many
/// cells each row has at least and the delimiter row `delim` has at most. A row with fewer cells
/// than the header gets the others as empty cells, and once pulldown-cmark has made that many it
/// ends the table, where a segment of the table (which starts counting afresh) would not.
fn sparse_rows_fit(src: &str, delim: &[u8], from: usize) -> bool {
    let cols = cells_at_least(delim, true);
    let mut missing = 0;
    for line in src[from..].split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line).as_bytes();
        if line.first() != Some(&b'|') || is_blank_ws(&line[1..]) {
            break;
        }
        missing += cols.saturating_sub(cells_at_least(line, false));
        if missing > MAX_AUTOCOMPLETED_CELLS {
            return false;
        }
    }
    true
}

/// The cells of a table row that starts with `|`: one per unescaped `|` (not counting one at the
/// end), at least one. With a backtick in the line that is all that is claimed, unless `exact`
/// (the delimiter row, which has no backticks or escapes).
fn cells_at_least(row: &[u8], exact: bool) -> usize {
    if !exact && row.contains(&b'`') {
        return 1;
    }
    // A pipe after a backslash may be escaped; counting it out only lowers the count.
    let escapes = row.contains(&b'\\');
    let pipes = if escapes {
        (0..row.len())
            .filter(|&i| row[i] == b'|' && (i == 0 || row[i - 1] != b'\\'))
            .count()
    } else {
        row.iter().filter(|&&b| b == b'|').count()
    };
    let end = row
        .iter()
        .rposition(|&b| !matches!(b, b' ' | b'\t' | 0x0b | 0x0c))
        .map_or(0, |i| i + 1);
    let trailing =
        end > 0 && row[end - 1] == b'|' && !(escapes && end > 1 && row[end - 2] == b'\\');
    (pipes - usize::from(trailing)).max(1)
}

/// Whether pulldown-cmark reads `head` (a header and delimiter row) as a table.
fn valid_head(src: &str, head: &Range<usize>) -> bool {
    let mut parser = Parser::new_ext(&src[head.clone()], super::options());
    matches!(parser.next(), Some(Event::Start(Tag::Table(_))))
}

fn leading_spaces(line: &[u8]) -> usize {
    line.iter().take_while(|&&b| b == b' ').count()
}

/// Whether the line (its end removed) is blank to every scanner of pulldown-cmark: only spaces
/// and tabs.
fn is_blank(line: &[u8]) -> bool {
    line.iter().all(|&b| matches!(b, b' ' | b'\t'))
}

/// Whether the line is whitespace by `scan_blank_line`, which also takes a vertical tab and a
/// form feed for blank. Only some scanners do: a paragraph goes on over a line of them.
fn is_blank_ws(line: &[u8]) -> bool {
    line.iter()
        .all(|&b| matches!(b, b' ' | b'\t' | 0x0b | 0x0c))
}

/// Width of the leading spaces and tabs, in columns (a tab goes to the next multiple of 4).
fn indent_width(line: &[u8]) -> usize {
    let mut width = 0;
    for &b in line {
        match b {
            b' ' => width += 1,
            b'\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

/// The line without its leading spaces and tabs.
fn trim_indent(line: &[u8]) -> &[u8] {
    let n = line
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    &line[n..]
}

/// What a text may define, going by its shape: a link reference definition, a footnote
/// definition. Every definition is `[label]:` with the `[` first on its line (after the markers
/// of lists and quotes), so only that is looked for: a `]:` and the unescaped `[` before it,
/// with no blank line in between. That finds every definition (and some text that is none),
/// which is all `Plan::definitions` needs to know to skip a segment.
fn may_define(text: &str) -> (bool, bool) {
    let bytes = text.as_bytes();
    let (mut links, mut notes) = (false, false);
    // The scan for a label's `[` goes back no further than the previous `]:`, so the whole text
    // is read about twice.
    let mut floor = 0;
    let mut at = 0;
    while let Some(i) = text[at..].find("]:") {
        let end = at + i;
        at = end + 2;
        let mut k = end;
        // Whether the line being read backwards is blank so far; the one with the `]:` is not.
        let mut blank = false;
        let open = loop {
            if k <= floor {
                break None;
            }
            k -= 1;
            match bytes[k] {
                b'[' => {
                    let escapes = bytes[..k].iter().rev().take_while(|&&b| b == b'\\').count();
                    if escapes % 2 == 0 {
                        break Some(k);
                    }
                    blank = false;
                }
                b'\n' if blank => break None,
                b'\n' => blank = true,
                b' ' | b'\t' | b'\r' => {}
                _ => blank = false,
            }
        };
        floor = end + 2;
        let Some(open) = open else { continue };
        let mut before = bytes[..open].iter().rev().take_while(|&&b| b != b'\n');
        if before.all(|&b| {
            matches!(b, b' ' | b'\t' | b'>' | b'*' | b'+' | b'-' | b'.' | b')')
                || b.is_ascii_digit()
        }) {
            links = true;
            notes |= bytes.get(open + 1) == Some(&b'^') && !bytes[open..end].contains(&b'\n');
            if notes {
                break;
            }
        }
    }
    (links, notes)
}

/// Whether the line has `]:` in it (the fast check for `]` first).
fn has_ref_end(line: &[u8]) -> bool {
    line.contains(&b']') && contains(line, b"]:")
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Whitespace as pulldown-cmark's HTML scanners see it.
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
