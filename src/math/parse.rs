//! Formula source → syntax tree.
//!
//! A recursive descent over the tokens of [`super::lex`], into an arena: the nodes are in one
//! `Vec`, and a node's children are a range of another (`lists`), so the tree is dropped flat
//! and never recursively. Every group, argument, `\left…\right` and environment is one level of
//! [`MAX_DEPTH`], which bounds the recursion; [`MAX_NODES`], [`MAX_CELLS`] and [`MAX_COLS`]
//! bound the tree. There are no macros, so the work is linear in the source. An error says what
//! went wrong and where, as a byte range of the source.
//!
//! The grammar is KaTeX's commonly used set; see the design's list. `\color`, `\colorbox`,
//! `\label`, `\nonumber` and the size switches (`\tiny` … `\Huge`) are read and dropped.

use std::ops::Range;

use super::lex::{Lexer, Tok, Token};
use super::symbols::{self, AccentKind, Class, Font};
use super::{MAX_CELLS, MAX_COLS, MAX_DEPTH, MAX_DIMEN, MAX_NODES, MAX_RULE, MAX_SOURCE};

/// A node of [`Ast::nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// Children: the range `start..end` of [`Ast::lists`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct List {
    pub start: u32,
    pub end: u32,
}

/// `\displaystyle` and the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathStyle {
    Display,
    Text,
    Script,
    ScriptScript,
}

/// Where a big operator's scripts go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limits {
    /// Above and below it in display style, beside it otherwise (`\sum`, `\lim`).
    Display,
    /// Always above and below (`\limits`, `\overbrace`).
    Always,
    /// Always beside (`\int`, `\sin`, `\nolimits`).
    Never,
}

/// The face of `\text` and its kin. Upright and bold are set in the math font; sans and mono
/// in the body text's faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFont {
    Upright,
    Bold,
    Italic,
    Sans,
    Mono,
}

/// Which way a `\mathllap`, `\mathrlap` or `\mathclap` hangs over its neighbours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lap {
    Left,
    Right,
    Centre,
}

/// An environment, or `\substack`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Env {
    Matrix,
    PMatrix,
    BMatrix,
    BraceMatrix,
    VMatrix,
    DoubleVMatrix,
    SmallMatrix,
    Cases,
    DCases,
    RCases,
    Aligned,
    AlignedAt,
    Gathered,
    Split,
    Array,
    Subarray,
    Substack,
    /// `align` and `align*` (display only, never numbered).
    Align,
    Gather,
    Equation,
    /// `eqnarray` (display only): right, centre and left columns, repeated.
    Eqnarray,
}

/// A column of an `array`'s spec: `l c r`, or a rule `|` (`:` dashed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Left,
    Center,
    Right,
    Rule { dashed: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Node {
    /// A character: a letter, digit or symbol, typed or named.
    Char {
        ch: char,
        class: Class,
    },
    /// `{…}`: an ord atom.
    Group(List),
    /// A base, its scripts, and the primes after it (`f''`: 2), which are set at the base's
    /// size, before the superscript.
    Scripts {
        base: Option<NodeId>,
        sup: Option<NodeId>,
        sub: Option<NodeId>,
        primes: u16,
    },
    /// A big operator (`body` a `Char`), an operator name (`body` a `TextRun`), `\mathop`, or
    /// a brace (`body` a `Brace`).
    Op {
        body: NodeId,
        limits: Limits,
    },
    Frac {
        num: NodeId,
        den: NodeId,
        bar: bool,
        /// An explicit rule thickness, in em (`\above 1pt`, `\genfrac`): the font's otherwise.
        thick: Option<f32>,
        style: Option<MathStyle>,
        left: Option<char>,
        right: Option<char>,
        /// `\cfrac`: a continued fraction's parts are set in display style, the numerator
        /// over a strut.
        cfrac: bool,
    },
    Sqrt {
        index: Option<NodeId>,
        body: NodeId,
    },
    /// `\left … \right`, with `Middle`s in the body.
    Delimited {
        left: Option<char>,
        right: Option<char>,
        body: List,
    },
    Middle(Option<char>),
    /// `\big(` and the others: `size` 1 to 4.
    Big {
        delim: Option<char>,
        size: u8,
        class: Class,
    },
    Accent {
        ch: char,
        kind: AccentKind,
        body: NodeId,
    },
    Overline(NodeId),
    Underline(NodeId),
    /// `\overbrace`, `\underbrace`: the body of an `Op` with limits.
    Brace {
        body: NodeId,
        under: bool,
    },
    /// `\overset`, `\underset`, `\stackrel`.
    Stack {
        body: NodeId,
        over: Option<NodeId>,
        under: Option<NodeId>,
        class: Class,
    },
    /// `\xrightarrow[under]{over}` and the others.
    XArrow {
        ch: char,
        over: Option<NodeId>,
        under: Option<NodeId>,
    },
    Font {
        font: Font,
        body: NodeId,
    },
    /// `\text{…}`: `TextRun`s, `Space`s, formulas (`$…$` inside), and nested `Text`.
    Text(List),
    /// Text: `start..end` of [`Ast::text`].
    TextRun {
        start: u32,
        end: u32,
        font: TextFont,
    },
    /// Space in em, or in mu (1/18 em of the style's size).
    Space {
        amount: f32,
        mu: bool,
    },
    Style(MathStyle),
    Class {
        class: Class,
        body: NodeId,
    },
    Phantom {
        body: NodeId,
        width: bool,
        height: bool,
    },
    Smash {
        body: NodeId,
        top: bool,
        bottom: bool,
    },
    Boxed(NodeId),
    /// `\rm`, `\bf` and the other old switches: the alphabet of the rest of the group.
    FontSwitch(Font),
    /// `\mathllap` and its kin: the body set without width, hanging to the left, to the right
    /// or both ways.
    Lap {
        body: NodeId,
        side: Lap,
    },
    /// `\rule[raise]{width}{height}`, in em.
    Rule {
        width: f32,
        height: f32,
        raise: f32,
    },
    /// `\not`: a slash over the next atom.
    Not(NodeId),
    /// `\\` outside an environment, at the formula's top level.
    Newline,
    /// An environment: `cols` is a range of [`Ast::cols`] (empty but for `array`,
    /// `subarray` and `matrix*` with an alignment), `cells` holds `Cell`s, a `RowEnd` after each row but the last, and
    /// `HLine`s where rows start.
    Array {
        env: Env,
        cols: List,
        cells: List,
        /// `alignedat`'s column pairs.
        pairs: u16,
    },
    Cell(List),
    /// `\\`, with the extra space of `\\[…]`, in em.
    RowEnd {
        gap: f32,
    },
    HLine {
        dashed: bool,
    },
}

/// A parsed formula.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ast {
    pub nodes: Vec<Node>,
    pub lists: Vec<NodeId>,
    pub text: String,
    pub cols: Vec<Col>,
    pub root: List,
    /// `\tag{…}` (a `Text` node), and whether it was `\tag*` (no parentheses).
    pub tag: Option<(NodeId, bool)>,
}

impl Ast {
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0 as usize)
    }

    pub fn list(&self, list: List) -> &[NodeId] {
        self.lists
            .get(list.start as usize..list.end as usize)
            .unwrap_or(&[])
    }

    pub fn text_of(&self, start: u32, end: u32) -> &str {
        self.text.get(start as usize..end as usize).unwrap_or("")
    }
}

/// What went wrong, and where: a byte range of the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: &'static str,
    pub span: Range<usize>,
}

type Result<T> = std::result::Result<T, Error>;

/// Parses a formula: `display` for `$$…$$` and ```` ```math ````, where the display-only
/// environments (`align`, …) and `\tag` are allowed.
pub fn parse(src: &str, display: bool) -> Result<Ast> {
    if src.len() > MAX_SOURCE {
        return Err(Error {
            message: "formula too long",
            span: 0..src.len(),
        });
    }
    parse_any(src, display)
}

/// [`parse`] of a source of any length (the tests reach the other limits with it).
fn parse_any(src: &str, display: bool) -> Result<Ast> {
    let mut p = Parser {
        lx: Lexer::new(src),
        ast: Ast::default(),
        depth: 0,
        lefts: 0,
        display,
    };
    let root = p.list(Stop::Top)?;
    let t = p.lx.peek_solid();
    if t.tok != Tok::Eof {
        return Err(p.unexpected(t));
    }
    p.ast.root = root;
    Ok(p.ast)
}

/// Where a list ends, besides at `}`, `&`, `\\`, `\right` and `\end` (which end every list:
/// the caller checks it got the one it wants).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// The formula's top level: `\\` is a `Newline`, and doesn't end it.
    Top,
    Plain,
    /// An optional argument: `]` ends it too.
    Bracket,
    /// A formula in `\text`: `$` ends it.
    Dollar,
}

struct Parser<'a> {
    lx: Lexer<'a>,
    ast: Ast,
    depth: usize,
    /// `\left`s open: `\middle` is allowed.
    lefts: usize,
    display: bool,
}

fn err<T>(message: &'static str, t: Token) -> Result<T> {
    Err(Error {
        message,
        span: t.start..t.end,
    })
}

impl<'a> Parser<'a> {
    fn push(&mut self, node: Node, at: Token) -> Result<NodeId> {
        if self.ast.nodes.len() >= MAX_NODES {
            return err("formula too complex", at);
        }
        self.ast.nodes.push(node);
        Ok(NodeId(self.ast.nodes.len() as u32 - 1))
    }

    fn make_list(&mut self, items: &[NodeId]) -> List {
        let start = self.ast.lists.len() as u32;
        self.ast.lists.extend_from_slice(items);
        List {
            start,
            end: self.ast.lists.len() as u32,
        }
    }

    fn enter(&mut self, at: Token) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err("formula nests too deep", at);
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn unexpected(&self, t: Token) -> Error {
        let message = match t.tok {
            Tok::Char('}') => "unmatched }",
            Tok::Char('&') => "& outside an environment",
            Tok::Char('$') => "unexpected $",
            Tok::Sym('\\') | Tok::Word("cr") => "\\\\ outside an environment",
            Tok::Word("right") => "\\right without \\left",
            Tok::Word("end") => "\\end without \\begin",
            Tok::Eof => "unexpected end of formula",
            _ => "unexpected token",
        };
        Error {
            message,
            span: t.start..t.end,
        }
    }

    /// Reads `expected` (after any space), or fails.
    fn expect(&mut self, expected: Tok, message: &'static str) -> Result<Token<'a>> {
        self.lx.skip_space();
        let t = self.lx.token();
        if t.tok != expected {
            return err(message, t);
        }
        Ok(t)
    }

    /// A list of atoms, up to (not including) what ends it.
    fn list(&mut self, stop: Stop) -> Result<List> {
        let mut items: Vec<NodeId> = Vec::new();
        // `\over` and its kin: the items before it, and which one it was.
        let mut infix: Option<(Vec<NodeId>, Token<'a>, bool, Option<f32>)> = None;
        loop {
            self.lx.skip_space();
            let t = self.lx.peek();
            match t.tok {
                Tok::Eof | Tok::Char('}' | '&') | Tok::Word("right" | "end") => break,
                Tok::Char(']') if stop == Stop::Bracket => break,
                Tok::Char('$') if stop == Stop::Dollar => break,
                Tok::Sym('\\') | Tok::Word("cr") if stop == Stop::Top => {
                    self.lx.token();
                    self.optional_dimen()?;
                    items.push(self.push(Node::Newline, t)?);
                }
                Tok::Sym('\\') | Tok::Word("cr") => break,
                Tok::Word("over" | "choose" | "atop" | "brace" | "brack" | "above") => {
                    self.lx.token();
                    if infix.is_some() {
                        return err("ambiguous fraction: use \\frac", t);
                    }
                    // `\above 1pt`: a bar that thick, none if it is 0.
                    let (bar, thick) = if t.tok == Tok::Word("above") {
                        rule_thickness(self.dimen()?.0)
                    } else {
                        (t.tok == Tok::Word("over"), None)
                    };
                    infix = Some((std::mem::take(&mut items), t, bar, thick));
                }
                _ => {
                    if let Some(id) = self.atom_with_scripts()? {
                        items.push(id);
                    }
                }
            }
        }
        if let Some((num, t, bar, thick)) = infix {
            // An old font switch before `\over` covers the denominator too, as in TeX.
            let switch = num
                .iter()
                .rev()
                .find(|&&id| matches!(self.ast.node(id), Some(Node::FontSwitch(_))))
                .copied();
            if let Some(switch) = switch {
                items.insert(0, switch);
            }
            let (num, den) = (self.make_list(&num), self.make_list(&items));
            let num = self.push(Node::Group(num), t)?;
            let den = self.push(Node::Group(den), t)?;
            let (left, right) = match t.tok {
                Tok::Word("choose") => (Some('('), Some(')')),
                Tok::Word("brace") => (Some('{'), Some('}')),
                Tok::Word("brack") => (Some('['), Some(']')),
                _ => (None, None),
            };
            let frac = Node::Frac {
                num,
                den,
                bar,
                thick,
                style: None,
                left,
                right,
                cfrac: false,
            };
            items = vec![self.push(frac, t)?];
        }
        Ok(self.make_list(&items))
    }

    /// An atom and its scripts and primes.
    fn atom_with_scripts(&mut self) -> Result<Option<NodeId>> {
        let first = self.lx.peek();
        let base = match first.tok {
            Tok::Char('^' | '_' | '\'') => None,
            _ => match self.atom()? {
                Some(id) => Some(id),
                None => return Ok(None),
            },
        };
        let (mut sup, mut sub, mut primes) = (None, None, 0u16);
        loop {
            self.lx.skip_space();
            let t = self.lx.peek();
            match t.tok {
                Tok::Char('^') => {
                    self.lx.token();
                    if sup.is_some() {
                        return err("double superscript", t);
                    }
                    sup = Some(self.arg(t)?);
                }
                Tok::Char('_') => {
                    self.lx.token();
                    if sub.is_some() {
                        return err("double subscript", t);
                    }
                    sub = Some(self.arg(t)?);
                }
                Tok::Char('\'') => {
                    if sup.is_some() {
                        return err("double superscript", t);
                    }
                    if primes > 0 {
                        return err("double superscript", t);
                    }
                    while self.lx.peek().tok == Tok::Char('\'') {
                        self.lx.token();
                        primes = primes.saturating_add(1);
                    }
                    // `x'^2`: the superscript comes after the primes.
                    self.lx.skip_space();
                    let caret = self.lx.peek();
                    if caret.tok == Tok::Char('^') {
                        self.lx.token();
                        sup = Some(self.arg(caret)?);
                    }
                }
                Tok::Word(w @ ("limits" | "nolimits")) => {
                    self.lx.token();
                    let op = base.and_then(|b| match self.ast.nodes.get_mut(b.0 as usize) {
                        Some(Node::Op { limits, .. }) => Some(limits),
                        _ => None,
                    });
                    match op {
                        Some(limits) => {
                            *limits = if w == "limits" {
                                Limits::Always
                            } else {
                                Limits::Never
                            };
                        }
                        None => return err("\\limits must follow an operator", t),
                    }
                }
                _ => break,
            }
        }
        if sup.is_none() && sub.is_none() && primes == 0 {
            return Ok(base);
        }
        let scripts = Node::Scripts {
            base,
            sup,
            sub,
            primes,
        };
        Ok(Some(self.push(scripts, first)?))
    }

    /// A required argument: a group, or one token (a command with its own arguments counts
    /// as one).
    fn arg(&mut self, at: Token) -> Result<NodeId> {
        self.enter(at)?;
        self.lx.skip_space();
        let t = self.lx.peek();
        let id = match t.tok {
            Tok::Char('{') => {
                self.lx.token();
                let list = self.list(Stop::Plain)?;
                self.close_brace()?;
                self.push(Node::Group(list), t)?
            }
            Tok::Char('}' | '^' | '_' | '&' | '\'' | '$')
            | Tok::Eof
            | Tok::Sym('\\')
            | Tok::Word("right" | "end" | "over" | "choose" | "atop" | "above" | "cr") => {
                return err("missing argument", t);
            }
            _ => match self.atom()? {
                Some(id) => id,
                None => return err("missing argument", t),
            },
        };
        self.leave();
        Ok(id)
    }

    fn close_brace(&mut self) -> Result<()> {
        self.lx.skip_space();
        let t = self.lx.token();
        match t.tok {
            Tok::Char('}') => Ok(()),
            Tok::Eof => err("missing }", t),
            _ => Err(self.unexpected(t)),
        }
    }

    /// `[…]` if it comes next: its contents as a group.
    fn optional_arg(&mut self) -> Result<Option<NodeId>> {
        let t = self.lx.peek_solid();
        if t.tok != Tok::Char('[') {
            return Ok(None);
        }
        self.lx.skip_space();
        self.lx.token();
        self.enter(t)?;
        let list = self.list(Stop::Bracket)?;
        self.expect(Tok::Char(']'), "missing ]")?;
        self.leave();
        Ok(Some(self.push(Node::Group(list), t)?))
    }

    /// The source between `{` and its `}`, as it is (a color, a label, an environment's name).
    fn raw_arg(&mut self) -> Result<&'a str> {
        let open = self.expect(Tok::Char('{'), "missing {")?;
        let start = self.lx.pos();
        let mut level = 1usize;
        loop {
            let t = self.lx.token();
            match t.tok {
                Tok::Char('{') => level += 1,
                Tok::Char('}') => {
                    level -= 1;
                    if level == 0 {
                        return Ok(self.src(start, t.start));
                    }
                }
                Tok::Eof | Tok::Bad => return err("missing }", open),
                _ => {}
            }
        }
    }

    fn src(&self, start: usize, end: usize) -> &'a str {
        let lx = self.lx;
        lx.source().get(start..end).unwrap_or("")
    }

    /// One atom, without its scripts; `None` for what makes nothing (`\label{…}`).
    fn atom(&mut self) -> Result<Option<NodeId>> {
        self.lx.skip_space();
        let t = self.lx.token();
        let node = match t.tok {
            Tok::Char('{') => {
                self.enter(t)?;
                let list = self.list(Stop::Plain)?;
                self.close_brace()?;
                self.leave();
                Node::Group(list)
            }
            Tok::Char('~') => Node::Space {
                amount: 0.25,
                mu: false,
            },
            Tok::Char('}' | '&' | '^' | '_' | '#' | '$' | '\'') => return Err(self.unexpected(t)),
            Tok::Char(c) => Node::Char {
                ch: if c == '-' { '\u{2212}' } else { c },
                class: symbols::class_of(c),
            },
            Tok::Sym(c) => return self.control_symbol(c, t),
            Tok::Word(w) => return self.command(w, t),
            Tok::Bad => return err("lone backslash", t),
            Tok::Space | Tok::Eof => return Err(self.unexpected(t)),
        };
        Ok(Some(self.push(node, t)?))
    }

    fn control_symbol(&mut self, c: char, t: Token<'a>) -> Result<Option<NodeId>> {
        let mu = |amount: f32| Node::Space { amount, mu: true };
        let node = match c {
            ',' => mu(3.0),
            ':' | '>' => mu(4.0),
            ';' => mu(5.0),
            '!' => mu(-3.0),
            ' ' | '\n' | '\t' => Node::Space {
                amount: 0.25,
                mu: false,
            },
            _ => match symbols::symbol(c.encode_utf8(&mut [0; 4])) {
                Some(s) => Node::Char {
                    ch: s.ch,
                    class: s.class,
                },
                None => return err("unknown command", t),
            },
        };
        Ok(Some(self.push(node, t)?))
    }

    fn command(&mut self, w: &'a str, t: Token<'a>) -> Result<Option<NodeId>> {
        let space = |amount: f32, mu: bool| Some(Node::Space { amount, mu });
        let node = match w {
            "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" => {
                if w == "cfrac" {
                    // The alignment, `[l]` or `[r]`, is not kept.
                    self.optional_arg()?;
                }
                let num = self.arg(t)?;
                let den = self.arg(t)?;
                let binom = w.ends_with("binom");
                let style = match w.as_bytes().first() {
                    Some(b'd') | Some(b'c') => Some(MathStyle::Display),
                    Some(b't') => Some(MathStyle::Text),
                    _ => None,
                };
                Some(Node::Frac {
                    num,
                    den,
                    bar: !binom,
                    thick: None,
                    style,
                    left: binom.then_some('('),
                    right: binom.then_some(')'),
                    cfrac: w == "cfrac",
                })
            }
            "sqrt" => {
                let index = self.optional_arg()?;
                let body = self.arg(t)?;
                Some(Node::Sqrt { index, body })
            }
            "left" => return self.delimited(t).map(Some),
            "middle" => {
                if self.lefts == 0 {
                    return err("\\middle without \\left", t);
                }
                Some(Node::Middle(self.delim_arg(t)?))
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "Bigl" | "biggl" | "Biggl" | "bigr"
            | "Bigr" | "biggr" | "Biggr" | "bigm" | "Bigm" | "biggm" | "Biggm" => {
                let size = match w.trim_end_matches(['l', 'r', 'm']) {
                    "big" => 1,
                    "Big" => 2,
                    "bigg" => 3,
                    _ => 4,
                };
                let class = match w.as_bytes().last() {
                    Some(b'l') => Class::Open,
                    Some(b'r') => Class::Close,
                    Some(b'm') => Class::Rel,
                    _ => Class::Ord,
                };
                let delim = self.delim_arg(t)?;
                Some(Node::Big { delim, size, class })
            }
            "begin" => return self.environment(t).map(Some),
            "substack" => {
                self.expect(Tok::Char('{'), "missing {")?;
                self.enter(t)?;
                let cells = self.cells(Env::Substack, t)?;
                self.close_brace()?;
                self.leave();
                Some(Node::Array {
                    env: Env::Substack,
                    cols: List::default(),
                    cells,
                    pairs: 0,
                })
            }
            "overline" | "underline" => {
                let body = self.arg(t)?;
                Some(if w == "overline" {
                    Node::Overline(body)
                } else {
                    Node::Underline(body)
                })
            }
            "overbrace" | "underbrace" => {
                let body = self.arg(t)?;
                let under = w == "underbrace";
                let brace = self.push(Node::Brace { body, under }, t)?;
                Some(Node::Op {
                    body: brace,
                    limits: Limits::Always,
                })
            }
            "overset" | "underset" | "stackrel" => {
                let script = self.arg(t)?;
                let body = self.arg(t)?;
                // `\overset{!}{=}` is a relation, as amsmath makes it.
                let mut inner = body;
                if let Some(Node::Group(l)) = self.ast.node(body)
                    && let [only] = self.ast.list(*l)
                {
                    inner = *only;
                }
                let class = match (w, self.ast.node(inner)) {
                    ("stackrel", _) => Class::Rel,
                    (_, Some(Node::Char { class, .. })) => *class,
                    _ => Class::Ord,
                };
                let (over, under) = if w == "underset" {
                    (None, Some(script))
                } else {
                    (Some(script), None)
                };
                Some(Node::Stack {
                    body,
                    over,
                    under,
                    class,
                })
            }
            "xrightarrow" | "xleftarrow" | "xleftrightarrow" | "xRightarrow" | "xLeftarrow"
            | "xLeftrightarrow" | "xmapsto" | "xhookleftarrow" | "xhookrightarrow"
            | "xrightleftharpoons" | "xlongequal" => {
                let ch = match w {
                    "xrightarrow" => '→',
                    "xleftarrow" => '←',
                    "xleftrightarrow" => '↔',
                    "xRightarrow" => '⇒',
                    "xLeftarrow" => '⇐',
                    "xLeftrightarrow" => '⇔',
                    "xmapsto" => '↦',
                    "xhookleftarrow" => '↩',
                    "xhookrightarrow" => '↪',
                    "xlongequal" => '=',
                    _ => '⇌',
                };
                let under = self.optional_arg()?;
                let over = self.arg(t)?;
                Some(Node::XArrow {
                    ch,
                    over: Some(over),
                    under,
                })
            }
            "text" | "textrm" | "textnormal" | "textup" | "mbox" | "textbf" | "textit"
            | "textsf" | "texttt" | "emph" => {
                let font = text_font(w);
                return self.text_arg(font, t).map(Some);
            }
            "operatorname" => {
                let star = self.star();
                let name = self.arg(t)?;
                let body = self.push(
                    Node::Font {
                        font: Font::Roman,
                        body: name,
                    },
                    t,
                )?;
                Some(Node::Op {
                    body,
                    limits: if star { Limits::Display } else { Limits::Never },
                })
            }
            "mathop" => {
                let body = self.arg(t)?;
                Some(Node::Op {
                    body,
                    limits: Limits::Display,
                })
            }
            "mathord" | "mathbin" | "mathrel" | "mathopen" | "mathclose" | "mathpunct"
            | "mathinner" => {
                let class = match w {
                    "mathord" => Class::Ord,
                    "mathbin" => Class::Bin,
                    "mathrel" => Class::Rel,
                    "mathopen" => Class::Open,
                    "mathclose" => Class::Close,
                    "mathpunct" => Class::Punct,
                    _ => Class::Inner,
                };
                let body = self.arg(t)?;
                Some(Node::Class { class, body })
            }
            "displaystyle" => Some(Node::Style(MathStyle::Display)),
            "textstyle" => Some(Node::Style(MathStyle::Text)),
            "scriptstyle" => Some(Node::Style(MathStyle::Script)),
            "scriptscriptstyle" => Some(Node::Style(MathStyle::ScriptScript)),
            "quad" => space(1.0, false),
            "qquad" => space(2.0, false),
            "enspace" | "enskip" => space(0.5, false),
            "thinspace" => space(3.0, true),
            "medspace" => space(4.0, true),
            "thickspace" => space(5.0, true),
            "negthinspace" => space(-3.0, true),
            "negmedspace" => space(-4.0, true),
            "negthickspace" => space(-5.0, true),
            "space" | "nobreakspace" => space(0.25, false),
            "hspace" => {
                self.star();
                let (amount, mu) = self.braced_dimen()?;
                space(amount, mu)
            }
            "kern" | "hskip" | "mkern" | "mskip" => {
                let (amount, mu) = self.dimen()?;
                if mu != w.starts_with('m') {
                    return err("wrong unit", t);
                }
                space(amount, mu)
            }
            "phantom" | "hphantom" | "vphantom" => {
                let body = self.arg(t)?;
                Some(Node::Phantom {
                    body,
                    width: w != "vphantom",
                    height: w != "hphantom",
                })
            }
            "smash" => {
                let (mut top, mut bottom) = (true, true);
                if self.lx.peek_solid().tok == Tok::Char('[') {
                    self.lx.skip_space();
                    self.lx.token();
                    match self.raw_until(']')?.trim() {
                        "t" => bottom = false,
                        "b" => top = false,
                        "tb" | "bt" | "" => {}
                        _ => return err("\\smash takes [t] or [b]", t),
                    }
                }
                let body = self.arg(t)?;
                Some(Node::Smash { body, top, bottom })
            }
            "boxed" => Some(Node::Boxed(self.arg(t)?)),
            "rule" => {
                let raise = if self.lx.peek_solid().tok == Tok::Char('[') {
                    self.lx.skip_space();
                    self.lx.token();
                    let (r, _) = self.dimen()?;
                    self.expect(Tok::Char(']'), "missing ]")?;
                    r
                } else {
                    0.0
                };
                let (width, _) = self.braced_dimen()?;
                let (height, _) = self.braced_dimen()?;
                Some(Node::Rule {
                    width,
                    height,
                    raise,
                })
            }
            "not" => {
                let next = self.lx.peek_solid();
                // Its operand is a level, as an argument's is: `\not\not\not…` is bounded.
                self.enter(t)?;
                let body = self.atom()?;
                self.leave();
                match body {
                    Some(body) => Some(Node::Not(body)),
                    None => return err("missing argument", next),
                }
            }
            "tag" => {
                let star = self.star();
                if !self.display {
                    return err("\\tag outside display math", t);
                }
                if self.ast.tag.is_some() {
                    return err("multiple \\tag", t);
                }
                let tag = self.text_arg(TextFont::Upright, t)?;
                self.ast.tag = Some((tag, star));
                None
            }
            "label" => {
                self.raw_arg()?;
                None
            }
            "color" => {
                self.raw_arg()?;
                None
            }
            "textcolor" => {
                self.raw_arg()?;
                return self.arg(t).map(Some);
            }
            "bmod" => {
                let body = self.named_run("mod", t)?;
                Some(Node::Class {
                    class: Class::Bin,
                    body,
                })
            }
            "mod" | "pmod" => {
                // As KaTeX has them: 18 mu of space before in display style, 8 (12 for
                // `\mod`) otherwise; `\pmod` in parentheses.
                let paren = w == "pmod";
                let arg = self.arg(t)?;
                let mu = |amount: f32| Node::Space { amount, mu: true };
                let gap = match (self.display, paren) {
                    (true, _) => 18.0,
                    (false, true) => 8.0,
                    (false, false) => 12.0,
                };
                let mut items = vec![self.push(mu(gap), t)?];
                if paren {
                    let open = Node::Char {
                        ch: '(',
                        class: Class::Open,
                    };
                    items.push(self.push(open, t)?);
                }
                items.push(self.named_run("mod", t)?);
                items.push(self.push(mu(6.0), t)?);
                items.push(arg);
                if paren {
                    let close = Node::Char {
                        ch: ')',
                        class: Class::Close,
                    };
                    items.push(self.push(close, t)?);
                }
                Some(Node::Group(self.make_list(&items)))
            }
            "rm" | "bf" | "it" | "cal" | "sf" | "tt" | "mit" | "frak" => {
                Some(Node::FontSwitch(match w {
                    "rm" => Font::Roman,
                    "bf" => Font::Bold,
                    "it" => Font::Italic,
                    "cal" => Font::Script,
                    "sf" => Font::Sans,
                    "tt" => Font::Mono,
                    "mit" => Font::Normal,
                    _ => Font::Fraktur,
                }))
            }
            // The size switches are accepted and do nothing: glyphs are never set larger than
            // the body's math size, and there is no smaller text size to go to.
            "tiny" | "scriptsize" | "footnotesize" | "small" | "normalsize" | "large" | "Large"
            | "LARGE" | "huge" | "Huge" => None,
            "mathllap" | "mathrlap" | "mathclap" | "llap" | "rlap" | "clap" => {
                let body = if w.starts_with("math") {
                    self.arg(t)?
                } else {
                    self.text_arg(TextFont::Upright, t)?
                };
                let side = match w {
                    "mathllap" | "llap" => Lap::Left,
                    "mathrlap" | "rlap" => Lap::Right,
                    _ => Lap::Centre,
                };
                Some(Node::Lap { body, side })
            }
            "fbox" => Some(Node::Boxed(self.text_arg(TextFont::Upright, t)?)),
            "colorbox" => {
                self.raw_arg()?;
                return self.text_arg(TextFont::Upright, t).map(Some);
            }
            "fcolorbox" => {
                self.raw_arg()?;
                self.raw_arg()?;
                Some(Node::Boxed(self.text_arg(TextFont::Upright, t)?))
            }
            "strut" => Some(Node::Rule {
                width: 0.0,
                height: 1.2,
                raise: -0.36,
            }),
            "mathstrut" => Some(Node::Rule {
                width: 0.0,
                height: 1.0,
                raise: -0.25,
            }),
            "genfrac" => {
                let left = self.genfrac_delim(t)?;
                let right = self.genfrac_delim(t)?;
                // `{}` is the font's thickness, `{0pt}` none.
                self.expect(Tok::Char('{'), "missing {")?;
                let (bar, thick) = if self.lx.peek_solid().tok == Tok::Char('}') {
                    (true, None)
                } else {
                    rule_thickness(self.dimen()?.0)
                };
                self.expect(Tok::Char('}'), "missing }")?;
                let style = match self.raw_arg()?.trim() {
                    "" => None,
                    "0" => Some(MathStyle::Display),
                    "1" => Some(MathStyle::Text),
                    "2" => Some(MathStyle::Script),
                    "3" => Some(MathStyle::ScriptScript),
                    _ => return err("\\genfrac takes a style from 0 to 3", t),
                };
                let num = self.arg(t)?;
                let den = self.arg(t)?;
                Some(Node::Frac {
                    num,
                    den,
                    bar,
                    thick,
                    style,
                    left,
                    right,
                    cfrac: false,
                })
            }
            "utilde" => {
                let body = self.arg(t)?;
                Some(Node::Accent {
                    ch: '\u{303}',
                    kind: AccentKind::UnderArrow,
                    body,
                })
            }
            "varliminf" | "varlimsup" => {
                let lim = self.named_run("lim", t)?;
                let deco = if w == "varliminf" {
                    Node::Underline(lim)
                } else {
                    Node::Overline(lim)
                };
                let body = self.push(deco, t)?;
                Some(Node::Op {
                    body,
                    limits: Limits::Display,
                })
            }
            "ref" | "eqref" => {
                // Nothing is numbered here, so a reference shows its label.
                let label = self.raw_arg()?.trim();
                let start = self.ast.text.len() as u32;
                if w == "eqref" {
                    self.ast.text.push('(');
                }
                self.ast.text.push_str(label);
                if w == "eqref" {
                    self.ast.text.push(')');
                }
                Some(Node::TextRun {
                    start,
                    end: self.ast.text.len() as u32,
                    font: TextFont::Upright,
                })
            }
            "nonumber" | "notag" | "allowbreak" | "nobreak" | "relax" => None,
            _ => {
                if let Some(font) = math_font(w) {
                    let body = self.arg(t)?;
                    Some(Node::Font { font, body })
                } else if let Some((ch, kind)) = symbols::accent(w) {
                    let body = self.arg(t)?;
                    Some(Node::Accent { ch, kind, body })
                } else if let Some(name) = operator_name(w) {
                    return self.operator_name(name, t).map(Some);
                } else if let Some(s) = symbols::symbol(w) {
                    let ch = self.push(
                        Node::Char {
                            ch: s.ch,
                            class: s.class,
                        },
                        t,
                    )?;
                    if s.class == Class::Op {
                        let limits = if s.limits {
                            Limits::Display
                        } else {
                            Limits::Never
                        };
                        Some(Node::Op { body: ch, limits })
                    } else {
                        return Ok(Some(ch));
                    }
                } else {
                    return err("unknown command", t);
                }
            }
        };
        match node {
            Some(node) => Ok(Some(self.push(node, t)?)),
            None => Ok(None),
        }
    }

    /// A `*` right after a command name.
    fn star(&mut self) -> bool {
        let starred = self.lx.peek().tok == Tok::Char('*');
        if starred {
            self.lx.token();
        }
        starred
    }

    /// `name` in upright letters: a `TextRun` of the text.
    fn named_run(&mut self, name: &str, t: Token) -> Result<NodeId> {
        let start = self.ast.text.len() as u32;
        self.ast.text.push_str(name);
        let run = Node::TextRun {
            start,
            end: self.ast.text.len() as u32,
            font: TextFont::Upright,
        };
        self.push(run, t)
    }

    /// `\sin`, `\lim`: the name in upright letters, as an operator.
    fn operator_name(&mut self, (name, limits): (&str, bool), t: Token) -> Result<NodeId> {
        let body = self.named_run(name, t)?;
        let limits = if limits {
            Limits::Display
        } else {
            Limits::Never
        };
        self.push(Node::Op { body, limits }, t)
    }

    /// One of `\genfrac`'s delimiters, in braces or bare: a character, a command, or nothing.
    fn genfrac_delim(&mut self, at: Token) -> Result<Option<char>> {
        self.lx.skip_space();
        let found = match self.lx.peek().tok {
            Tok::Char('{') => {
                let raw = self.raw_arg()?.trim();
                match raw.strip_prefix('\\') {
                    Some(name) if name.chars().count() == 1 => {
                        symbols::symbol(name).map(|s| Some(s.ch))
                    }
                    Some(name) => symbols::delimiter(name),
                    None if raw.is_empty() => Some(None),
                    None => symbols::delimiter(raw),
                }
            }
            Tok::Char(c) => {
                self.lx.token();
                symbols::delimiter(c.encode_utf8(&mut [0; 4]))
            }
            Tok::Sym(c) => {
                self.lx.token();
                symbols::symbol(c.encode_utf8(&mut [0; 4])).map(|s| Some(s.ch))
            }
            Tok::Word(w) => {
                self.lx.token();
                symbols::delimiter(w)
            }
            _ => None,
        };
        match found {
            Some(d) => Ok(d),
            None => err("not a delimiter", at),
        }
    }

    /// A delimiter after `\left`, `\big` and the others.
    fn delim_arg(&mut self, at: Token) -> Result<Option<char>> {
        self.lx.skip_space();
        let t = self.lx.token();
        let mut buf = [0; 4];
        let name = match t.tok {
            Tok::Char(c) => &*c.encode_utf8(&mut buf),
            Tok::Sym(c) => match c {
                '{' | '}' | '|' | '\\' => &*c.encode_utf8(&mut buf),
                _ => return err("not a delimiter", t),
            },
            Tok::Word(w) => w,
            _ => return err("missing delimiter", at),
        };
        let found = match (t.tok, name) {
            (Tok::Sym(_), "\\") => None,
            (Tok::Sym(_), "|") => Some(Some('‖')),
            (Tok::Sym(_), n) => symbols::symbol(n).map(|s| Some(s.ch)),
            (Tok::Char(c), _) if !c.is_ascii() => symbols::delimiter(name).or_else(|| {
                matches!(symbols::class_of(c), Class::Open | Class::Close).then_some(Some(c))
            }),
            _ => symbols::delimiter(name),
        };
        match found {
            Some(d) => Ok(d),
            None => err("not a delimiter", t),
        }
    }

    fn delimited(&mut self, t: Token<'a>) -> Result<NodeId> {
        let left = self.delim_arg(t)?;
        self.enter(t)?;
        self.lefts += 1;
        let body = self.list(Stop::Plain)?;
        self.lefts -= 1;
        self.lx.skip_space();
        let r = self.lx.token();
        if r.tok != Tok::Word("right") {
            return err("\\left without \\right", t);
        }
        let right = self.delim_arg(r)?;
        self.leave();
        self.push(Node::Delimited { left, right, body }, t)
    }

    /// `\begin{name}…\end{name}`.
    fn environment(&mut self, t: Token<'a>) -> Result<NodeId> {
        let name = self.raw_arg()?.trim();
        let env = match name {
            "matrix" => Env::Matrix,
            "pmatrix" => Env::PMatrix,
            "bmatrix" => Env::BMatrix,
            "Bmatrix" => Env::BraceMatrix,
            "vmatrix" => Env::VMatrix,
            "Vmatrix" => Env::DoubleVMatrix,
            "smallmatrix" => Env::SmallMatrix,
            "cases" => Env::Cases,
            "dcases" => Env::DCases,
            "rcases" => Env::RCases,
            "aligned" => Env::Aligned,
            "alignedat" => Env::AlignedAt,
            "gathered" => Env::Gathered,
            "split" => Env::Split,
            "array" => Env::Array,
            "subarray" => Env::Subarray,
            "align" | "align*" => Env::Align,
            "gather" | "gather*" => Env::Gather,
            "equation" | "equation*" => Env::Equation,
            "multline" | "multline*" => Env::Gather,
            "eqnarray" | "eqnarray*" => Env::Eqnarray,
            "matrix*" => Env::Matrix,
            "pmatrix*" => Env::PMatrix,
            "bmatrix*" => Env::BMatrix,
            "Bmatrix*" => Env::BraceMatrix,
            "vmatrix*" => Env::VMatrix,
            "Vmatrix*" => Env::DoubleVMatrix,
            "smallmatrix*" => Env::SmallMatrix,
            _ => return err("unknown environment", t),
        };
        if matches!(
            env,
            Env::Align | Env::Gather | Env::Equation | Env::Eqnarray
        ) && !self.display
        {
            return err("environment only allowed in display math", t);
        }
        self.enter(t)?;
        let mut cols = List::default();
        let mut pairs = 0;
        match env {
            Env::Array | Env::Subarray => {
                let spec = self.raw_arg()?;
                let start = self.ast.cols.len() as u32;
                for c in spec.chars() {
                    let col = match c {
                        'l' => Col::Left,
                        'c' => Col::Center,
                        'r' => Col::Right,
                        '|' => Col::Rule { dashed: false },
                        ':' => Col::Rule { dashed: true },
                        c if c.is_whitespace() => continue,
                        _ => return err("bad column spec", t),
                    };
                    if self.ast.cols.len() - start as usize >= MAX_COLS {
                        return err("too many columns", t);
                    }
                    self.ast.cols.push(col);
                }
                cols = List {
                    start,
                    end: self.ast.cols.len() as u32,
                };
            }
            // `matrix*` and kin: `[r]` sets the alignment of every column.
            _ if name.ends_with("matrix*") => {
                if self.lx.peek_solid().tok == Tok::Char('[') {
                    self.lx.skip_space();
                    self.lx.token();
                    let col = match self.raw_until(']')?.trim() {
                        "l" => Col::Left,
                        "c" => Col::Center,
                        "r" => Col::Right,
                        _ => return err("bad column spec", t),
                    };
                    let start = self.ast.cols.len() as u32;
                    self.ast.cols.push(col);
                    cols = List {
                        start,
                        end: start + 1,
                    };
                }
            }
            Env::AlignedAt => {
                let n = self.raw_arg()?.trim();
                pairs = match n.parse::<u16>() {
                    Ok(n) if (1..=MAX_COLS as u16 / 2).contains(&n) => n,
                    _ => return err("bad column count", t),
                };
            }
            _ => {}
        }
        let cells = self.cells(env, t)?;
        self.lx.skip_space();
        let end = self.lx.token();
        if end.tok != Tok::Word("end") {
            return err("\\begin without \\end", t);
        }
        if self.raw_arg()?.trim() != name {
            return err("\\end doesn't match \\begin", end);
        }
        self.leave();
        self.push(
            Node::Array {
                env,
                cols,
                cells,
                pairs,
            },
            t,
        )
    }

    /// The rows and cells of an environment, up to its `\end` (or `}` for `\substack`).
    fn cells(&mut self, env: Env, t: Token<'a>) -> Result<List> {
        let mut items: Vec<NodeId> = Vec::new();
        let (mut cells, mut col) = (0usize, 0usize);
        loop {
            if col == 0 {
                loop {
                    let h = self.lx.peek_solid();
                    let dashed = match h.tok {
                        Tok::Word("hline") => false,
                        Tok::Word("hdashline") => true,
                        _ => break,
                    };
                    self.lx.skip_space();
                    self.lx.token();
                    items.push(self.push(Node::HLine { dashed }, h)?);
                }
                // `\\` before the end makes no row.
                let next = self.lx.peek_solid().tok;
                let end = if env == Env::Substack {
                    next == Tok::Char('}')
                } else {
                    next == Tok::Word("end")
                };
                if end && cells > 0 {
                    break;
                }
            }
            let body = self.list(Stop::Plain)?;
            cells += 1;
            col += 1;
            if cells > MAX_CELLS || col > MAX_COLS {
                return err("too many cells", t);
            }
            items.push(self.push(Node::Cell(body), t)?);
            let next = self.lx.peek_solid();
            match next.tok {
                Tok::Char('&') if env != Env::Substack => {
                    self.lx.skip_space();
                    self.lx.token();
                }
                Tok::Sym('\\') | Tok::Word("cr") => {
                    self.lx.skip_space();
                    self.lx.token();
                    let gap = self.optional_dimen()?;
                    items.push(self.push(Node::RowEnd { gap }, next)?);
                    col = 0;
                }
                Tok::Word("end") if env != Env::Substack => break,
                Tok::Char('}') if env == Env::Substack => break,
                Tok::Eof => return err("\\begin without \\end", t),
                _ => return Err(self.unexpected(next)),
            }
        }
        Ok(self.make_list(&items))
    }

    /// `\text{…}`: characters as they are, white space collapsed, with `$…$` formulas and
    /// nested `\textbf{…}` and the like.
    fn text_arg(&mut self, mut font: TextFont, at: Token<'a>) -> Result<NodeId> {
        self.enter(at)?;
        self.lx.skip_space();
        let open = self.lx.token();
        let mut items: Vec<NodeId> = Vec::new();
        let mut run = self.ast.text.len();
        if open.tok != Tok::Char('{') {
            // One token: `\text a`.
            match open.tok {
                Tok::Char(c) if !matches!(c, '}' | '$' | '^' | '_' | '&') => self.ast.text.push(c),
                _ => return err("missing argument", open),
            }
        } else {
            let mut level = 1usize;
            // The style outside each open `{`, which a switch inside it doesn't outlast.
            let mut outer: Vec<TextFont> = Vec::new();
            loop {
                let pos = self.lx.pos();
                let Some(c) = self.lx.peek_char() else {
                    return err("missing }", open);
                };
                let here = Token {
                    tok: Tok::Char(c),
                    start: pos,
                    end: pos + c.len_utf8(),
                };
                match c {
                    '{' => {
                        self.lx.bump();
                        level += 1;
                        outer.push(font);
                        // The argument's own brace is the level `enter` counted.
                        if self.depth + level - 1 > MAX_DEPTH {
                            return err("formula nests too deep", here);
                        }
                    }
                    '}' => {
                        self.lx.bump();
                        level -= 1;
                        if level == 0 {
                            break;
                        }
                        if let Some(before) = outer.pop().filter(|&before| before != font) {
                            self.flush_run(&mut items, &mut run, font, here)?;
                            font = before;
                        }
                    }
                    '%' => {
                        while self.lx.peek_char().is_some_and(|c| c != '\n') {
                            self.lx.bump();
                        }
                    }
                    '$' => {
                        self.lx.bump();
                        self.flush_run(&mut items, &mut run, font, here)?;
                        self.enter(here)?;
                        let list = self.list(Stop::Dollar)?;
                        self.expect(Tok::Char('$'), "missing $")?;
                        self.leave();
                        items.push(self.push(Node::Group(list), here)?);
                        run = self.ast.text.len();
                    }
                    '\\' => {
                        let t = self.lx.token();
                        match t.tok {
                            // `\rm`, `\bf`, `\it`, `\sf` and `\tt` change the style for the rest
                            // of the group.
                            Tok::Word(w @ ("rm" | "bf" | "it" | "sf" | "tt")) => {
                                self.flush_run(&mut items, &mut run, font, here)?;
                                font = match w {
                                    "bf" => TextFont::Bold,
                                    "it" => TextFont::Italic,
                                    "sf" => TextFont::Sans,
                                    "tt" => TextFont::Mono,
                                    _ => TextFont::Upright,
                                };
                                while self.lx.peek_char().is_some_and(char::is_whitespace) {
                                    self.lx.bump();
                                }
                            }
                            Tok::Word(w) if is_text_command(w) => {
                                self.flush_run(&mut items, &mut run, font, here)?;
                                let inner = match w {
                                    "text" | "textnormal" | "mbox" => font,
                                    _ => text_font(w),
                                };
                                items.push(self.text_arg(inner, t)?);
                                run = self.ast.text.len();
                            }
                            Tok::Sym(c @ ('$' | '%' | '&' | '#' | '_' | '{' | '}')) => {
                                self.ast.text.push(c)
                            }
                            Tok::Sym(' ' | '\n' | '\t') | Tok::Word("space") => {
                                self.ast.text.push(' ')
                            }
                            Tok::Sym(c @ (',' | ':' | ';' | '!')) => {
                                self.flush_run(&mut items, &mut run, font, here)?;
                                let amount = match c {
                                    ',' => 3.0,
                                    ':' => 4.0,
                                    ';' => 5.0,
                                    _ => -3.0,
                                };
                                items.push(self.push(Node::Space { amount, mu: true }, t)?);
                                run = self.ast.text.len();
                            }
                            Tok::Word("quad") | Tok::Word("qquad") => {
                                self.flush_run(&mut items, &mut run, font, here)?;
                                let amount = if t.tok == Tok::Word("quad") { 1.0 } else { 2.0 };
                                items.push(self.push(Node::Space { amount, mu: false }, t)?);
                                run = self.ast.text.len();
                            }
                            Tok::Word(w) => match text_symbol(w) {
                                Some(c) => {
                                    self.ast.text.push(c);
                                    // A control word eats the space after it.
                                    while self.lx.peek_char().is_some_and(char::is_whitespace) {
                                        self.lx.bump();
                                    }
                                }
                                None => return err("unknown command in text", t),
                            },
                            Tok::Bad => return err("lone backslash", t),
                            _ => return err("unknown command in text", t),
                        }
                    }
                    '~' => {
                        self.lx.bump();
                        self.ast.text.push('\u{A0}');
                    }
                    c if c.is_whitespace() => {
                        self.lx.bump();
                        if !self.ast.text.ends_with(' ') || self.ast.text.len() == run {
                            self.ast.text.push(' ');
                        }
                    }
                    c => {
                        self.lx.bump();
                        self.ast.text.push(c);
                    }
                }
                if self.ast.text.len() > MAX_SOURCE * 2 {
                    return err("formula too long", here);
                }
            }
        }
        self.flush_run(&mut items, &mut run, font, open)?;
        self.leave();
        let list = self.make_list(&items);
        self.push(Node::Text(list), at)
    }

    /// Ends the text run that started at `run`, if it has anything.
    fn flush_run(
        &mut self,
        items: &mut Vec<NodeId>,
        run: &mut usize,
        font: TextFont,
        at: Token,
    ) -> Result<()> {
        let end = self.ast.text.len();
        if end > *run {
            let node = Node::TextRun {
                start: *run as u32,
                end: end as u32,
                font,
            };
            items.push(self.push(node, at)?);
        }
        *run = end;
        Ok(())
    }

    /// The source up to `close`, as it is, consumed with it.
    fn raw_until(&mut self, close: char) -> Result<&'a str> {
        let start = self.lx.pos();
        loop {
            match self.lx.peek_char() {
                Some(c) if c == close => {
                    let s = self.src(start, self.lx.pos());
                    self.lx.bump();
                    return Ok(s);
                }
                Some(_) => self.lx.bump(),
                None => {
                    let at = self.lx.pos();
                    return Err(Error {
                        message: "missing ]",
                        span: start..at,
                    });
                }
            }
        }
    }

    /// `[dimen]` after `\\`, if it comes next: the dimension in em (0 for none).
    fn optional_dimen(&mut self) -> Result<f32> {
        // Not past space: `\\ [x]` is a row end and then a bracket.
        if self.lx.peek().tok != Tok::Char('[') {
            return Ok(0.0);
        }
        self.lx.token();
        let (d, _) = self.dimen()?;
        self.expect(Tok::Char(']'), "missing ]")?;
        Ok(d)
    }

    /// `{dimen}`.
    fn braced_dimen(&mut self) -> Result<(f32, bool)> {
        self.expect(Tok::Char('{'), "missing {")?;
        let d = self.dimen()?;
        self.expect(Tok::Char('}'), "missing }")?;
        Ok(d)
    }

    /// A dimension, `-1.5em`: its value in em, or in mu if its unit is `mu` (then `true`).
    /// Clamped to ±[`MAX_DIMEN`] em.
    fn dimen(&mut self) -> Result<(f32, bool)> {
        self.lx.skip_space();
        let start = self.lx.pos();
        let mut negative = false;
        while let Some(c @ ('-' | '+')) = self.lx.peek_char() {
            negative ^= c == '-';
            self.lx.bump();
            self.lx.skip_space();
        }
        let (mut value, mut scale, mut digits, mut point) = (0f64, 1f64, 0usize, false);
        while let Some(c) = self.lx.peek_char() {
            match c {
                '0'..='9' => {
                    let d = f64::from(c as u8 - b'0');
                    if point {
                        scale /= 10.0;
                        value += d * scale;
                    } else {
                        value = (value * 10.0 + d).min(1e9);
                    }
                    digits += 1;
                }
                '.' | ',' if !point => point = true,
                _ => break,
            }
            self.lx.bump();
        }
        self.lx.skip_space();
        let unit_at = self.lx.pos();
        let mut unit = [0u8; 2];
        for u in &mut unit {
            match self.lx.peek_char() {
                Some(c) if c.is_ascii_alphabetic() => {
                    *u = c.to_ascii_lowercase() as u8;
                    self.lx.bump();
                }
                _ => break,
            }
        }
        // In em; TeX's units at a 10 pt font, as KaTeX takes them.
        let (per_unit, mu) = match &unit {
            b"em" => (1.0, false),
            b"ex" => (0.431, false),
            b"mu" => (1.0, true),
            b"pt" => (0.1, false),
            b"pc" => (1.2, false),
            b"bp" => (0.1 * 72.27 / 72.0, false),
            b"dd" => (0.1 * 1238.0 / 1157.0, false),
            b"cc" => (1.2 * 1238.0 / 1157.0, false),
            b"in" => (7.227, false),
            b"cm" => (7.227 / 2.54, false),
            b"mm" => (0.7227 / 2.54, false),
            b"sp" => (0.1 / 65536.0, false),
            b"px" => (0.1, false),
            _ => {
                return Err(Error {
                    message: "bad dimension",
                    span: start..unit_at.max(start),
                });
            }
        };
        if digits == 0 {
            return Err(Error {
                message: "bad dimension",
                span: start..unit_at.max(start),
            });
        }
        let limit = if mu { MAX_DIMEN * 18.0 } else { MAX_DIMEN };
        let v = (value * per_unit).min(f64::from(limit)) as f32;
        Ok((if negative { -v } else { v }, mu))
    }
}

/// `\mathbf` and the other alphabets.
/// Whether a fraction of rule thickness `d` em (as written) has a rule, and how thick: none
/// at 0 or less, up to [`MAX_RULE`] em.
fn rule_thickness(d: f32) -> (bool, Option<f32>) {
    if d > 0.0 {
        (true, Some(d.min(MAX_RULE)))
    } else {
        (false, None)
    }
}

fn math_font(w: &str) -> Option<Font> {
    Some(match w {
        "mathrm" | "mathup" => Font::Roman,
        "mathit" => Font::Italic,
        "mathnormal" => Font::Normal,
        "mathbf" | "bold" | "pmb" => Font::Bold,
        "boldsymbol" | "bm" | "mathbfit" => Font::BoldItalic,
        "mathbb" | "Bbb" => Font::DoubleStruck,
        "mathcal" | "mathscr" => Font::Script,
        "mathfrak" => Font::Fraktur,
        "mathsf" => Font::Sans,
        "mathtt" => Font::Mono,
        _ => return None,
    })
}

fn is_text_command(w: &str) -> bool {
    matches!(
        w,
        "text"
            | "textrm"
            | "textnormal"
            | "textup"
            | "mbox"
            | "textbf"
            | "textit"
            | "textsf"
            | "texttt"
            | "emph"
    )
}

fn text_font(w: &str) -> TextFont {
    match w {
        "textbf" => TextFont::Bold,
        "textit" | "emph" => TextFont::Italic,
        "textsf" => TextFont::Sans,
        "texttt" => TextFont::Mono,
        _ => TextFont::Upright,
    }
}

/// The few named characters text takes.
fn text_symbol(w: &str) -> Option<char> {
    Some(match w {
        "textbackslash" => '\\',
        "textasciitilde" => '~',
        "textasciicircum" => '^',
        "textendash" => '–',
        "textemdash" => '—',
        "textquoteleft" => '‘',
        "textquoteright" => '’',
        "textdollar" => '$',
        "ldots" | "dots" | "textellipsis" => '…',
        "S" => '§',
        "P" => '¶',
        "dag" => '†',
        "ddag" => '‡',
        "copyright" => '©',
        "pounds" => '£',
        "degree" => '°',
        _ => return None,
    })
}

/// An operator name: what it prints, and whether it takes limits in display style.
fn operator_name(w: &str) -> Option<(&'static str, bool)> {
    const NAMES: &[(&str, &str, bool)] = &[
        ("Pr", "Pr", true),
        ("arccos", "arccos", false),
        ("arcctg", "arcctg", false),
        ("arcsin", "arcsin", false),
        ("arctan", "arctan", false),
        ("arctg", "arctg", false),
        ("arg", "arg", false),
        ("argmax", "arg max", true),
        ("argmin", "arg min", true),
        ("ch", "ch", false),
        ("cos", "cos", false),
        ("cosh", "cosh", false),
        ("cot", "cot", false),
        ("coth", "coth", false),
        ("csc", "csc", false),
        ("ctg", "ctg", false),
        ("cth", "cth", false),
        ("deg", "deg", false),
        ("det", "det", true),
        ("dim", "dim", false),
        ("exp", "exp", false),
        ("gcd", "gcd", true),
        ("hom", "hom", false),
        ("inf", "inf", true),
        ("injlim", "inj lim", true),
        ("ker", "ker", false),
        ("lg", "lg", false),
        ("lim", "lim", true),
        ("liminf", "lim inf", true),
        ("limsup", "lim sup", true),
        ("ln", "ln", false),
        ("log", "log", false),
        ("max", "max", true),
        ("min", "min", true),
        ("plim", "plim", true),
        ("projlim", "proj lim", true),
        ("sec", "sec", false),
        ("sh", "sh", false),
        ("sin", "sin", false),
        ("sinh", "sinh", false),
        ("sup", "sup", true),
        ("tan", "tan", false),
        ("tanh", "tanh", false),
        ("tg", "tg", false),
        ("th", "th", false),
    ];
    let i = NAMES.binary_search_by_key(&w, |e| e.0).ok()?;
    NAMES.get(i).map(|e| (e.1, e.2))
}

#[cfg(test)]
mod tests;
