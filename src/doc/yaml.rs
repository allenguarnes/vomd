//! The YAML reader for front matter: YAML 1.1 as libyaml reads it (GitHub reads front matter with
//! Ruby's Psych, which is libyaml), to a tree of nodes. Block and flow collections, the plain,
//! quoted and block scalar styles, comments, anchors and tags. An alias is never followed: GitHub
//! doesn't allow them, and the reader only notes that there was one. The common mistakes get
//! libyaml's messages and the position Psych reports (its "context mark", 1-based, from the first
//! line after the opening `---`).
//!
//! Hostile input is bounded: the text is read once, front to back, nesting is at most
//! [`MAX_DEPTH`] collections deep (the reader recurses), and there are at most [`MAX_NODES`] nodes.

/// Collections nested deeper than this are an error.
pub const MAX_DEPTH: usize = 32;
/// More nodes than this are an error.
pub const MAX_NODES: usize = 100_000;
/// Longest implicit key, in bytes (libyaml's 1024 characters, near enough).
const MAX_KEY: usize = 1024;

#[derive(Debug, PartialEq)]
pub enum Node {
    Scalar {
        text: String,
        /// Written plain (not quoted, not a block scalar).
        plain: bool,
        /// The resolved tag: `tag:yaml.org,2002:str` for `!!str`, `!x` for `!x`.
        tag: Option<String>,
    },
    Seq(Vec<Node>),
    /// Entries in the order written, duplicates included.
    Map(Vec<(Node, Node)>),
    /// `*name`: never followed.
    Alias,
}

impl Node {
    fn null(tag: Option<String>) -> Node {
        Node::Scalar {
            text: String::new(),
            plain: true,
            tag,
        }
    }
}

/// What the text is, read as one YAML document: `Ok(None)` when it is empty (blank lines and
/// comments only), else the node and whether an alias appeared in it; or the error message
/// (after GitHub's "Error in user YAML: ").
pub fn read(src: &str) -> Result<Option<(Node, bool)>, String> {
    // libyaml's reader refuses these before anything is parsed.
    if src.chars().any(|c| {
        !matches!(c, '\t' | '\n' | '\r' | ' '..='~' | '\u{85}' | '\u{a0}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}' | '\u{10000}'..)
    }) {
        return Err("(<unknown>): control characters are not allowed at line 1 column 1".into());
    }
    // A `...` line ends the document; what follows it is another document, which isn't read.
    let mut end = src.len();
    let mut at = 0;
    for line in src.split_inclusive('\n') {
        let l = line.trim_end_matches(['\n', '\r']);
        if l.strip_prefix("...")
            .is_some_and(|r| r.is_empty() || r.starts_with([' ', '\t']))
        {
            end = at;
            break;
        }
        at += line.len();
    }
    let src = &src[..end];
    let mut lines = vec![0u32];
    lines.extend(
        src.bytes()
            .enumerate()
            .filter(|&(_, b)| b == b'\n')
            .map(|(i, _)| i as u32 + 1),
    );
    let mut p = P {
        s: src,
        b: src.as_bytes(),
        i: 0,
        lines,
        depth: 0,
        nodes: 0,
        alias: false,
        ws_ok: 0,
    };
    match p.document() {
        Ok(node) => Ok(node.map(|n| (n, p.alias))),
        Err(f) => {
            let (line, col) = p.line_col(f.at);
            let (problem, context) = MSGS[f.e as usize];
            let sep = if context.is_empty() { "" } else { " " };
            Err(format!(
                "(<unknown>): {problem}{sep}{context} at line {line} column {col}"
            ))
        }
    }
}

/// libyaml's errors, as indices into [`MSGS`].
#[derive(Clone, Copy, Debug)]
enum E {
    Key,
    Dash,
    FlowSeq,
    FlowMap,
    MapValues,
    SeqEntries,
    MapKeys,
    SimpleKey,
    Token,
    Tab,
    Eos,
    Escape,
    Hex,
    Unicode,
    BlockBreak,
    Indicator0,
    FlowNode,
    BlockNode,
    Anchor,
    Alias,
    Handle,
    TagGt,
    TagWs,
    TagUri,
    Depth,
    Nodes,
}

/// Each error's problem and context (empty when it has none).
const MSGS: [(&str, &str); 26] = [
    ("did not find expected key", "while parsing a block mapping"),
    (
        "did not find expected '-' indicator",
        "while parsing a block collection",
    ),
    (
        "did not find expected ',' or ']'",
        "while parsing a flow sequence",
    ),
    (
        "did not find expected ',' or '}'",
        "while parsing a flow mapping",
    ),
    ("mapping values are not allowed in this context", ""),
    ("block sequence entries are not allowed in this context", ""),
    ("mapping keys are not allowed in this context", ""),
    ("could not find expected ':'", "while scanning a simple key"),
    (
        "found character that cannot start any token",
        "while scanning for the next token",
    ),
    (
        "found a tab character that violates indentation",
        "while scanning a plain scalar",
    ),
    (
        "found unexpected end of stream",
        "while scanning a quoted scalar",
    ),
    (
        "found unknown escape character",
        "while parsing a quoted scalar",
    ),
    (
        "did not find expected hexdecimal number",
        "while parsing a quoted scalar",
    ),
    (
        "found invalid Unicode character escape code",
        "while parsing a quoted scalar",
    ),
    (
        "did not find expected comment or line break",
        "while scanning a block scalar",
    ),
    (
        "found an indentation indicator equal to 0",
        "while scanning a block scalar",
    ),
    (
        "did not find expected node content",
        "while parsing a flow node",
    ),
    (
        "did not find expected node content",
        "while parsing a block node",
    ),
    (
        "did not find expected alphabetic or numeric character",
        "while scanning an anchor",
    ),
    (
        "did not find expected alphabetic or numeric character",
        "while scanning an alias",
    ),
    ("found undefined tag handle", "while parsing a node"),
    ("did not find the expected '>'", "while scanning a tag"),
    (
        "did not find expected whitespace or line break",
        "while scanning a tag",
    ),
    ("did not find expected tag URI", "while parsing a tag"),
    ("exceeded max depth of 32", ""),
    ("exceeded max node count of 100000", ""),
];

/// An error and the position Psych reports for it (a byte offset).
#[derive(Debug)]
struct Fail {
    at: usize,
    e: E,
}

type R<T> = Result<T, Fail>;

fn fail<T>(at: usize, e: E) -> R<T> {
    Err(Fail { at, e })
}

/// Where a node is, which decides what may start there.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// At the document's start.
    Top,
    /// After a mapping's `:`, on its line: no `- `, `? ` or `key:` here.
    Value,
    /// After a sequence's `- `, on its line.
    Item,
    /// After `? `, on its line.
    Key,
    /// At the start of a later line than its indicator's.
    Below,
}

struct P<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    /// Byte offset of every line's start.
    lines: Vec<u32>,
    depth: usize,
    nodes: usize,
    alias: bool,
    /// A plain scalar read the whitespace up to here, tabs included: they were legal there.
    ws_ok: usize,
}

const FLOW_IND: &[u8] = b",[]{}";

impl P<'_> {
    fn peek(&self, k: usize) -> u8 {
        self.b.get(self.i + k).copied().unwrap_or(0)
    }

    fn eof(&self) -> bool {
        self.i >= self.b.len()
    }

    /// Whether the byte `k` ahead is a blank, a line break or the end.
    fn blankz(&self, k: usize) -> bool {
        self.i + k >= self.b.len() || matches!(self.b[self.i + k], b' ' | b'\t' | b'\n' | b'\r')
    }

    fn at_eol(&self) -> bool {
        self.eof() || matches!(self.peek(0), b'\n' | b'\r')
    }

    fn line_of(&self, at: usize) -> usize {
        self.lines.partition_point(|&l| l as usize <= at) - 1
    }

    /// Byte column (indentation is ASCII spaces, so this is the column for structure).
    fn col(&self, at: usize) -> usize {
        at - self.lines[self.line_of(at)] as usize
    }

    /// 1-based line and character column, as Psych reports them.
    fn line_col(&self, at: usize) -> (usize, usize) {
        let at = at.min(self.s.len());
        let line = self.line_of(at);
        let start = self.lines[line] as usize;
        (line + 1, self.s[start..at].chars().count() + 1)
    }

    fn same_line(&self, from: usize, to: usize) -> bool {
        !self.b[from..to].contains(&b'\n')
    }

    fn count(&mut self, at: usize) -> R<()> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return fail(at, E::Nodes);
        }
        Ok(())
    }

    /// An empty node that the text implies (an empty value, item or key): a node like any other
    /// for the node limit.
    fn null(&mut self, tag: Option<String>) -> R<Node> {
        self.count(self.i)?;
        Ok(Node::null(tag))
    }

    fn enter(&mut self, at: usize) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return fail(at, E::Depth);
        }
        Ok(())
    }

    fn skip_comment(&mut self) {
        while !self.at_eol() {
            self.i += 1;
        }
    }

    /// Spaces, tabs and a comment, up to the line's end.
    fn skip_inline(&mut self) {
        while matches!(self.peek(0), b' ' | b'\t') {
            self.i += 1;
        }
        if self.peek(0) == b'#' {
            self.skip_comment();
        }
    }

    /// Whitespace, comments and line breaks, up to the next token. In block context a tab in a
    /// line's leading whitespace is libyaml's "cannot start any token", even on a blank or
    /// comment line (unless a plain scalar before it read it, see [`P::plain`]).
    fn skip_ws(&mut self, flow: bool) -> R<()> {
        loop {
            match self.peek(0) {
                b' ' | b'\r' | b'\n' => self.i += 1,
                b'\t' => {
                    if !flow
                        && self.i >= self.ws_ok
                        && self.b[self.lines[self.line_of(self.i)] as usize..self.i]
                            .iter()
                            .all(|&b| b == b' ')
                    {
                        return fail(self.i, E::Token);
                    }
                    self.i += 1;
                }
                b'#' => self.skip_comment(),
                _ => return Ok(()),
            }
        }
    }

    /// The document: its root node. libyaml ends the document where the root ends, and Psych
    /// reads no further, so what follows is only scanned as far as its first token.
    fn document(&mut self) -> R<Option<Node>> {
        self.skip_ws(false)?;
        if self.eof() {
            return Ok(None);
        }
        let node = self.node(-1, Ctx::Top)?;
        self.skip_ws(false)?;
        match self.peek(0) {
            b'@' | b'`' => return fail(self.i, E::Token),
            b'"' | b'\'' => {
                self.quoted()?;
            }
            _ => {}
        }
        Ok(Some(node))
    }

    /// The node after an indicator (`-`, `?`, `:`) of a block collection at indentation `n`
    /// (-1 at the top), from anywhere on the indicator's line: on that line, or on later lines
    /// indented more than `n` (or at `n` for a mapping value's sequence).
    fn node(&mut self, n: i32, mut ctx: Ctx) -> R<Node> {
        let mut tag = None;
        // Where the tag was: one on an earlier line than a mapping's first key is the mapping's.
        let mut tag_at = 0;
        loop {
            self.skip_inline();
            if self.at_eol() {
                self.skip_ws(false)?;
                if self.eof() {
                    return self.null(tag);
                }
                let c = self.col(self.i) as i32;
                let compact = c == n && ctx == Ctx::Value && self.peek(0) == b'-' && self.blankz(1);
                if c <= n && !compact {
                    return self.null(tag);
                }
                ctx = Ctx::Below;
            }
            if matches!(self.peek(0), b'&' | b'!') {
                let at = self.i;
                if let Some(t) = self.props(false)? {
                    tag = Some(t);
                    tag_at = at;
                }
                continue;
            }
            break;
        }
        let start = self.i;
        let c = self.col(start);
        let (b0, blank) = (self.peek(0), self.blankz(1));
        if b0 == b'-' && blank {
            if ctx == Ctx::Value {
                return fail(start, E::SeqEntries);
            }
            return self.block_seq(c, c as i32 == n);
        }
        if b0 == b'?' && blank {
            if ctx == Ctx::Value {
                return fail(start, E::MapKeys);
            }
            return self.block_map(c, start, None);
        }
        if b0 == b':' && blank {
            if ctx == Ctx::Value {
                return fail(start, E::MapValues);
            }
            return fail(start, E::Key);
        }
        if b0 == b'|' || b0 == b'>' {
            self.count(start)?;
            let text = self.block_scalar(n)?;
            return Ok(Node::Scalar {
                text,
                plain: false,
                tag,
            });
        }
        let own_line = tag.is_some() && !self.same_line(tag_at, start);
        let node = self.inline(n, false, tag)?;
        let end = self.i;
        if self.colon() {
            if ctx == Ctx::Value || !self.simple_key(start, end) {
                return fail(self.i, E::MapValues);
            }
            let node = match node {
                Node::Scalar { text, plain, .. } if own_line => Node::Scalar {
                    text,
                    plain,
                    tag: None,
                },
                node => node,
            };
            return self.block_map(c, start, Some(node));
        }
        self.i = end;
        Ok(node)
    }

    /// After a block `-` or `?`, where a simple key may start: libyaml skips spaces but not tabs,
    /// so a tab before the content is "cannot start any token".
    fn no_tab(&mut self) -> R<()> {
        while self.peek(0) == b' ' {
            self.i += 1;
        }
        if self.peek(0) == b'\t' {
            return fail(self.i, E::Token);
        }
        Ok(())
    }

    /// Past blanks, whether a `:` that ends a key follows on the line.
    fn colon(&mut self) -> bool {
        while matches!(self.peek(0), b' ' | b'\t') {
            self.i += 1;
        }
        self.peek(0) == b':' && self.blankz(1)
    }

    /// Whether the text from `start` to `end` can be an implicit key: one line, not too long.
    fn simple_key(&self, start: usize, end: usize) -> bool {
        self.same_line(start, end) && end - start <= MAX_KEY
    }

    /// A block mapping whose keys are at column `m`; its first key is at `start`, already read
    /// (`first`, with `self.i` before its `:`) or not.
    fn block_map(&mut self, m: usize, start: usize, mut first: Option<Node>) -> R<Node> {
        self.enter(start)?;
        self.count(start)?;
        let mut entries = Vec::new();
        loop {
            let key = match first.take() {
                Some(k) => Some(k),
                None if self.peek(0) == b'?' && self.blankz(1) => {
                    self.i += 1;
                    self.no_tab()?;
                    let k = self.node(m as i32, Ctx::Key)?;
                    self.skip_ws(false)?;
                    if self.peek(0) == b':' && self.blankz(1) && self.col(self.i) == m {
                        Some(k)
                    } else {
                        entries.push((k, self.null(None)?));
                        None
                    }
                }
                None => {
                    if (matches!(self.peek(0), b':' | b'-') && self.blankz(1))
                        || matches!(self.peek(0), b'|' | b'>')
                    {
                        return fail(start, E::Key);
                    }
                    let kstart = self.i;
                    let tag = self.props(false)?;
                    if self.at_eol() {
                        return fail(start, E::Key);
                    }
                    let k = self.inline(m as i32, false, tag)?;
                    let end = self.i;
                    if !self.colon() {
                        // A key at the mapping's column must find its `:` on its line (libyaml's
                        // required simple key).
                        let e = if self.at_eol()
                            || self.peek(0) == b'#'
                            || !self.same_line(kstart, end)
                        {
                            E::SimpleKey
                        } else {
                            E::Key
                        };
                        return fail(if matches!(e, E::Key) { start } else { kstart }, e);
                    }
                    if !self.simple_key(kstart, end) {
                        return fail(kstart, E::SimpleKey);
                    }
                    Some(k)
                }
            };
            if let Some(k) = key {
                self.i += 1; // ':'
                let v = self.node(m as i32, Ctx::Value)?;
                entries.push((k, v));
                self.skip_ws(false)?;
            }
            if self.eof() {
                break;
            }
            let c = self.col(self.i);
            if c < m {
                break;
            }
            if c > m {
                return fail(start, E::Key);
            }
        }
        self.depth -= 1;
        Ok(Node::Map(entries))
    }

    /// A block sequence whose `-` are at column `s`; `compact` when it is a mapping value at the
    /// mapping's own indentation (it ends at the first line that isn't an entry).
    fn block_seq(&mut self, s: usize, compact: bool) -> R<Node> {
        let start = self.i;
        self.enter(start)?;
        self.count(start)?;
        let mut items = Vec::new();
        loop {
            self.i += 1; // '-'
            self.no_tab()?;
            items.push(self.node(s as i32, Ctx::Item)?);
            self.skip_ws(false)?;
            if self.eof() {
                break;
            }
            let c = self.col(self.i);
            let entry = self.peek(0) == b'-' && self.blankz(1);
            if c < s || (c == s && !entry && compact) {
                break;
            }
            if c == s
                && !entry
                && (matches!(self.peek(0), b'"' | b'\'' | b'!' | b'&') || self.plain_start(false))
            {
                // At the sequence's column, a scalar without its `:` is a required simple key.
                let at = self.i;
                let tag = self.props(false)?;
                if self.at_eol() || {
                    self.inline(s as i32, false, tag)?;
                    !self.colon()
                } {
                    return fail(at, E::SimpleKey);
                }
            }
            if c != s || !entry {
                return fail(start, E::Dash);
            }
        }
        self.depth -= 1;
        Ok(Node::Seq(items))
    }

    /// Anchors and a tag before a node, and the blanks after them (not line breaks). The tag, if
    /// any.
    fn props(&mut self, flow: bool) -> R<Option<String>> {
        let mut tag = None;
        loop {
            match self.peek(0) {
                b'&' => {
                    self.name(E::Anchor)?;
                }
                b'!' => tag = Some(self.tag(flow)?),
                _ => return Ok(tag),
            }
            while matches!(self.peek(0), b' ' | b'\t') {
                self.i += 1;
            }
        }
    }

    /// An anchor's or alias's name, after its `&` or `*`.
    fn name(&mut self, e: E) -> R<()> {
        let start = self.i;
        self.i += 1;
        let from = self.i;
        while self.peek(0).is_ascii_alphanumeric() || matches!(self.peek(0), b'_' | b'-') {
            self.i += 1;
        }
        if self.i == from || !(self.blankz(0) || b"?:,]}%@`".contains(&self.peek(0))) {
            return fail(start, e);
        }
        Ok(())
    }

    /// A tag, resolved: `!<uri>` is the URI, `!!x` is `tag:yaml.org,2002:x`, `!x` and `!` stay.
    fn tag(&mut self, flow: bool) -> R<String> {
        let start = self.i;
        self.i += 1;
        let uri = |b: u8| {
            b.is_ascii_alphanumeric()
                || b"-;/?:@&=+$.!~*'()%_".contains(&b)
                || (!flow && b"[],".contains(&b))
        };
        let tag = if self.peek(0) == b'<' {
            self.i += 1;
            let from = self.i;
            while uri(self.peek(0)) && self.peek(0) != b'>' {
                self.i += 1;
            }
            if self.peek(0) != b'>' || self.i == from {
                return fail(start, E::TagGt);
            }
            self.i += 1;
            self.s[from..self.i - 1].to_owned()
        } else {
            let from = self.i;
            while self.peek(0).is_ascii_alphanumeric() || matches!(self.peek(0), b'-' | b'_') {
                self.i += 1;
            }
            let handle = if self.peek(0) == b'!' {
                self.i += 1;
                &self.s[from - 1..self.i]
            } else {
                self.i = from;
                "!"
            };
            let suffix = self.i;
            while uri(self.peek(0)) {
                self.i += 1;
            }
            let suffix = &self.s[suffix..self.i];
            match handle {
                "!!" if suffix.is_empty() => return fail(start, E::TagUri),
                "!!" => format!("tag:yaml.org,2002:{suffix}"),
                "!" => format!("!{suffix}"),
                _ => {
                    return fail(start, E::Handle);
                }
            }
        };
        if !(self.blankz(0) || (flow && self.peek(0) == b',')) {
            return fail(start, E::TagWs);
        }
        Ok(tag)
    }

    /// A node that fits in a line or a flow: a flow collection, a quoted or plain scalar, or an
    /// alias. A plain scalar in block context goes on over the lines indented more than `n`.
    fn inline(&mut self, n: i32, flow: bool, tag: Option<String>) -> R<Node> {
        let start = self.i;
        let b0 = self.peek(0);
        match b0 {
            b'[' => self.flow(b']'),
            b'{' => self.flow(b'}'),
            b'"' | b'\'' => {
                self.count(start)?;
                let text = self.quoted()?;
                Ok(Node::Scalar {
                    text,
                    plain: false,
                    tag,
                })
            }
            b'*' => {
                self.count(start)?;
                self.name(E::Alias)?;
                self.alias = true;
                Ok(Node::Alias)
            }
            _ if self.plain_start(flow) => {
                self.count(start)?;
                let text = self.plain(n, flow, start)?;
                Ok(Node::Scalar {
                    text,
                    plain: true,
                    tag,
                })
            }
            b'@' | b'`' | b'%' | b'|' | b'>' | b'\t' => fail(start, E::Token),
            _ if tag.is_some() && (self.blankz(0) || (flow && FLOW_IND.contains(&b0))) => {
                self.count(start)?;
                Ok(Node::null(tag))
            }
            _ => fail(start, if flow { E::FlowNode } else { E::BlockNode }),
        }
    }

    fn plain_start(&self, flow: bool) -> bool {
        let b0 = self.peek(0);
        if self.blankz(0) {
            return false;
        }
        if b"-?:,[]{}#&*!|>'\"%@`".contains(&b0) {
            return (b0 == b'-' && !self.blankz(1))
                || (!flow && matches!(b0, b'?' | b':') && !self.blankz(1));
        }
        true
    }

    /// A plain scalar (starting at `start`), folded: its line breaks are spaces, its blank lines
    /// line breaks. Looking for its next line, it reads the whitespace after it: in block
    /// context a tab there, in a line's indentation, left of the scalar's block (column `n` + 1),
    /// is libyaml's "tab character that violates indentation", at the scalar.
    fn plain(&mut self, n: i32, flow: bool, start: usize) -> R<String> {
        let mut out = String::new();
        loop {
            let from = self.i;
            let mut end = self.i;
            loop {
                let c = self.peek(0);
                if self.at_eol()
                    || (c == b':'
                        && (self.blankz(1) || (flow && b",?[]{}".contains(&self.peek(1)))))
                    || (flow && FLOW_IND.contains(&c))
                    || (matches!(c, b' ' | b'\t') && self.peek(1) == b'#')
                {
                    break;
                }
                self.i += 1;
                if !matches!(c, b' ' | b'\t') {
                    end = self.i;
                }
            }
            if end == from {
                break;
            }
            out.push_str(&self.s[from..end]);
            self.i = end;
            // Go on to the next line?
            let mut j = end;
            while matches!(self.b.get(j), Some(b' ' | b'\t' | b'\r')) {
                j += 1;
            }
            if self.b.get(j) != Some(&b'\n') {
                break;
            }
            let mut breaks = 0;
            while self.b.get(j) == Some(&b'\n') {
                breaks += 1;
                j += 1;
                let line = j;
                while let Some(&c @ (b' ' | b'\t' | b'\r')) = self.b.get(j) {
                    if c == b'\t' && !flow && ((j - line) as i32) < n + 1 {
                        return fail(start, E::Tab);
                    }
                    j += 1;
                }
            }
            self.ws_ok = j;
            let line = self.lines[self.line_of(j)] as usize;
            let indent = self.b[line..j].iter().take_while(|&&b| b == b' ').count() as i32;
            if j >= self.b.len()
                || self.b[j] == b'#'
                || (!flow && indent <= n)
                || (j == line
                    && self.b[j..].starts_with(b"---")
                    && self.b.get(j + 3).is_none_or(|b| b.is_ascii_whitespace()))
            {
                break;
            }
            let save = self.i;
            self.i = j;
            // A line that starts with what ends a scalar adds nothing: the scalar ends before
            // its break.
            let c = self.peek(0);
            if (c == b':' && (self.blankz(1) || (flow && b",?[]{}".contains(&self.peek(1)))))
                || (flow && FLOW_IND.contains(&c))
            {
                self.i = save;
                break;
            }
            if breaks == 1 {
                out.push(' ');
            } else {
                out.extend(std::iter::repeat_n('\n', breaks - 1));
            }
        }
        Ok(out)
    }

    /// A single- or double-quoted scalar, folded and unescaped.
    fn quoted(&mut self) -> R<String> {
        let start = self.i;
        let q = self.peek(0);
        self.i += 1;
        let mut out = String::new();
        loop {
            if self.eof() {
                return fail(start, E::Eos);
            }
            let c = self.peek(0);
            if q == b'\'' && c == b'\'' {
                if self.peek(1) == b'\'' {
                    out.push('\'');
                    self.i += 2;
                    continue;
                }
                self.i += 1;
                return Ok(out);
            }
            if q == b'"' && c == b'"' {
                self.i += 1;
                return Ok(out);
            }
            if q == b'"' && c == b'\\' {
                let e = self.peek(1);
                self.i += 2;
                let ch = match e {
                    b'\n' | b'\r' => {
                        // An escaped line break: the lines join with nothing between, but the
                        // blank lines after it stay line breaks (as libyaml has it).
                        if e == b'\r' && self.peek(0) == b'\n' {
                            self.i += 1;
                        }
                        loop {
                            while matches!(self.peek(0), b' ' | b'\t' | b'\r') {
                                self.i += 1;
                            }
                            if self.peek(0) != b'\n' {
                                break;
                            }
                            out.push('\n');
                            self.i += 1;
                        }
                        continue;
                    }
                    b'0' => '\0',
                    b'a' => '\x07',
                    b'b' => '\x08',
                    b't' | b'\t' => '\t',
                    b'n' => '\n',
                    b'v' => '\x0b',
                    b'f' => '\x0c',
                    b'r' => '\r',
                    b'e' => '\x1b',
                    b' ' => ' ',
                    b'"' => '"',
                    b'/' => '/',
                    b'\\' => '\\',
                    b'N' => '\u{85}',
                    b'_' => '\u{a0}',
                    b'L' => '\u{2028}',
                    b'P' => '\u{2029}',
                    b'x' | b'u' | b'U' => {
                        let len = match e {
                            b'x' => 2,
                            b'u' => 4,
                            _ => 8,
                        };
                        let hex = self.s.get(self.i..self.i + len).unwrap_or("");
                        if hex.len() != len || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return fail(start, E::Hex);
                        }
                        self.i += len;
                        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                            Some(ch) => ch,
                            None => {
                                return fail(start, E::Unicode);
                            }
                        }
                    }
                    _ => return fail(start, E::Escape),
                };
                out.push(ch);
                continue;
            }
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                let from = self.i;
                let mut breaks = 0;
                while matches!(self.peek(0), b' ' | b'\t' | b'\n' | b'\r') {
                    breaks += (self.peek(0) == b'\n') as usize;
                    self.i += 1;
                }
                match breaks {
                    0 => out.push_str(&self.s[from..self.i]),
                    1 => out.push(' '),
                    _ => out.extend(std::iter::repeat_n('\n', breaks - 1)),
                }
                continue;
            }
            let ch = self.s[self.i..].chars().next().unwrap_or(' ');
            out.push(ch);
            self.i += ch.len_utf8();
        }
    }

    /// A literal (`|`) or folded (`>`) block scalar, in a block collection at indentation `n`.
    fn block_scalar(&mut self, n: i32) -> R<String> {
        let start = self.i;
        let folded = self.peek(0) == b'>';
        self.i += 1;
        let (mut chomp, mut inc) = (0i8, 0usize);
        for _ in 0..2 {
            match self.peek(0) {
                b'+' | b'-' if chomp == 0 => chomp = if self.peek(0) == b'+' { 1 } else { -1 },
                b'0' => return fail(start, E::Indicator0),
                d @ b'1'..=b'9' if inc == 0 => inc = (d - b'0') as usize,
                _ => break,
            }
            self.i += 1;
        }
        while matches!(self.peek(0), b' ' | b'\t') {
            self.i += 1;
        }
        if self.peek(0) == b'#' {
            self.skip_comment();
        }
        if !self.at_eol() {
            return fail(start, E::BlockBreak);
        }
        while matches!(self.peek(0), b'\r') {
            self.i += 1;
        }
        if self.eof() {
            return Ok(String::new());
        }
        self.i += 1;
        let floor = (n + 1).max(1) as usize;
        let mut indent = if inc > 0 { n.max(0) as usize + inc } else { 0 };
        if indent == 0 {
            // The first line with text sets it (or a longer blank line before it).
            let (mut j, mut max) = (self.i, 0);
            loop {
                let k = self.b[j..].iter().take_while(|&&b| b == b' ').count();
                max = max.max(k);
                match self.b[j + k..].iter().position(|&b| b == b'\n') {
                    Some(e) if self.b[j + k..j + k + e].iter().all(|&b| b == b'\r') => {
                        j += k + e + 1;
                    }
                    _ => break,
                }
            }
            indent = max.max(floor);
        }
        let mut lines: Vec<&str> = Vec::new();
        while !self.eof() {
            let k = self.b[self.i..].iter().take_while(|&&b| b == b' ').count();
            let eol = self.b[self.i..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(self.b.len(), |p| self.i + p);
            let text = self.s[self.i + k.min(indent)..eol].trim_end_matches('\r');
            let blank = self.b[self.i + k..eol].iter().all(|&b| b == b'\r');
            if !blank && k < indent {
                break;
            }
            lines.push(if blank { "" } else { text });
            self.i = (eol + 1).min(self.b.len());
        }
        let trailing = lines.iter().rev().take_while(|l| l.is_empty()).count();
        let body = &lines[..lines.len() - trailing];
        let mut out = String::new();
        // Folding: a break between two lines of text is a space, and the break before blank
        // lines goes (they stand for themselves); more-indented lines keep their breaks.
        let text = |l: &str| !l.is_empty() && !l.starts_with([' ', '\t']);
        for (k, line) in body.iter().enumerate() {
            if k > 0 {
                let prev = body[k - 1];
                match (folded && text(prev), line.is_empty(), text(line)) {
                    (true, _, true) => out.push(' '),
                    (true, true, _) => {}
                    _ => out.push('\n'),
                }
            }
            out.push_str(line);
        }
        match chomp {
            -1 => {}
            1 => out.extend(std::iter::repeat_n(
                '\n',
                trailing + !body.is_empty() as usize,
            )),
            _ if !body.is_empty() => out.push('\n'),
            _ => {}
        }
        Ok(out)
    }

    /// A flow sequence (`close` is `]`) or mapping (`}`); a sequence's `k: v` item is a map of
    /// one entry.
    fn flow(&mut self, close: u8) -> R<Node> {
        let start = self.i;
        self.enter(start)?;
        self.count(start)?;
        self.i += 1;
        let seq = close == b']';
        let e = if seq { E::FlowSeq } else { E::FlowMap };
        let (mut items, mut entries) = (Vec::new(), Vec::new());
        loop {
            self.skip_ws(true)?;
            if self.peek(0) == close {
                self.i += 1;
                break;
            }
            if self.eof() {
                return fail(start, e);
            }
            let (k, v) = if self.peek(0) == b'?' && self.blankz(1) {
                self.i += 1;
                let (k, v) = self.flow_pair(close)?;
                (k, Some(v))
            } else {
                let from = self.i;
                let k = self.flow_node()?;
                self.skip_ws(true)?;
                if self.peek(0) == b':' {
                    if !self.simple_key(from, self.i) {
                        return fail(start, e);
                    }
                    self.i += 1;
                    (k, Some(self.flow_value(close)?))
                } else {
                    (k, if seq { None } else { Some(self.null(None)?) })
                }
            };
            match v {
                None => items.push(k),
                Some(v) if seq => {
                    self.count(start)?;
                    items.push(Node::Map(vec![(k, v)]));
                }
                Some(v) => entries.push((k, v)),
            }
            self.skip_ws(true)?;
            match self.peek(0) {
                b',' => self.i += 1,
                c if c == close => {}
                _ => return fail(start, e),
            }
        }
        self.depth -= 1;
        Ok(if seq {
            Node::Seq(items)
        } else {
            Node::Map(entries)
        })
    }

    /// After `? ` in a flow: the key, and the value after a `:`, either of them empty.
    fn flow_pair(&mut self, close: u8) -> R<(Node, Node)> {
        self.skip_ws(true)?;
        let k = if matches!(self.peek(0), b':' | b',') || self.peek(0) == close {
            self.null(None)?
        } else {
            self.flow_node()?
        };
        self.skip_ws(true)?;
        let v = if self.peek(0) == b':' {
            self.i += 1;
            self.flow_value(close)?
        } else {
            self.null(None)?
        };
        Ok((k, v))
    }

    /// The value after a `:` in a flow (empty before a `,` or the closing bracket).
    fn flow_value(&mut self, close: u8) -> R<Node> {
        self.skip_ws(true)?;
        if self.peek(0) == b',' || self.peek(0) == close {
            return self.null(None);
        }
        self.flow_node()
    }

    fn flow_node(&mut self) -> R<Node> {
        self.skip_ws(true)?;
        let tag = self.props(true)?;
        if tag.is_some() {
            self.skip_ws(true)?;
        }
        self.inline(-1, true, tag)
    }
}

#[cfg(test)]
mod tests;
