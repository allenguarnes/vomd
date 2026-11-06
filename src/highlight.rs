//! Syntax highlighting of fenced code blocks: one table-driven lexer and a string of words per
//! language, after GitHub's colour classes.
//!
//! A fence's info string picks a table ([`find`]); [`lex`] cuts a block's text into runs of one
//! class each ([`Tok`]), which layout gives the matching theme colour. Nothing is stored: a block
//! is lexed each time it is laid out, from the state its chunk starts in (an oversized block is
//! cut into chunks, and the state carries a comment or string open across a cut).
//!
//! The lexer is linear in its input, allocates nothing and never panics: every step consumes at
//! least a byte or changes the state, every lookahead is bounded or done once per line, and every
//! run boundary is next to an ASCII byte, so a run is always a slice on character boundaries.

/// A token's class, after GitHub's (Primer's "prettylights") colour roles. `Plain` is text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tok {
    Plain,
    /// Comments, and a console block's prompts.
    Comment,
    /// Keywords, primitive types, lifetimes, `@media`, `!important`, `#include`.
    Keyword,
    /// Strings and characters; YAML plain scalars, Markdown code.
    Str,
    /// Numbers, literals (`true`, `null`, `None`), builtins; Markdown and diff headings.
    Constant,
    /// Function and type names where they are defined or called; decorators, attributes,
    /// INI sections, CSS classes and ids.
    Entity,
    /// HTML and XML tag names, JSON, YAML and TOML keys, CSS element selectors.
    Tag,
    /// Variables (`$x`, `@x`), YAML anchors, Markdown list markers, diff `!` lines.
    Var,
    /// Diff `+` lines.
    Inserted,
    /// Diff `-` lines.
    Deleted,
    /// Diff hunk headers (`@@`).
    Meta,
}

/// How a table's text is read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A programming language, as its flags (`f`) say.
    Code,
    /// HTML and XML.
    Markup,
    Css,
    Yaml,
    /// INI and TOML.
    Ini,
    Diff,
    /// Prompts, commands (lexed as the shell) and their output (plain).
    Console,
    Markdown,
}

/// The flags of the code mode.
mod f {
    /// `//` line comments.
    pub const C2: u32 = 1 << 0;
    /// `#` line comments.
    pub const HASH: u32 = 1 << 1;
    /// `--` line comments.
    pub const DASH2: u32 = 1 << 2;
    /// `/* */` block comments.
    pub const BLOCK: u32 = 1 << 3;
    /// Block comments nest.
    pub const NEST: u32 = 1 << 4;
    /// `'…'` strings.
    pub const SQ: u32 = 1 << 5;
    /// `` `…` `` strings, which may span lines.
    pub const BT: u32 = 1 << 6;
    /// `"""…"""` strings (and `'''…'''` with `SQ`).
    pub const TRIPLE: u32 = 1 << 7;
    /// `'…'` and `"…"` may span lines.
    pub const ML: u32 = 1 << 8;
    /// Keywords and constants match whatever their case (the lists are lowercase).
    pub const CI: u32 = 1 << 9;
    /// `$var`, `${…}` and `$(…)` are variables.
    pub const DOLLAR: u32 = 1 << 10;
    /// `$` is a name character (JavaScript's `$el`).
    pub const DOLLAR_ID: u32 = 1 << 11;
    /// `name(` is a function.
    pub const CALL: u32 = 1 << 12;
    /// `@name` is a decorator (an entity).
    pub const AT: u32 = 1 << 13;
    /// `@name` and `@@name` are variables.
    pub const ATVAR: u32 = 1 << 14;
    /// Rust: lifetimes, chars, raw strings, `b""`, `name!` macros.
    pub const RUST: u32 = 1 << 15;
    /// Python's string prefixes: `r""`, `f""`, `b""`.
    pub const PY: u32 = 1 << 16;
    /// `#include` and other preprocessor lines.
    pub const PRE: u32 = 1 << 17;
    /// The shell: `#` starts a comment only after a space, and `'…'` has no escapes.
    pub const SHELL: u32 = 1 << 18;
    /// No backslash escapes in strings (SQL).
    pub const NOESC: u32 = 1 << 19;
    /// Lua's long brackets: `--[[ ]]` comments and `[[ ]]` strings.
    pub const LUA: u32 = 1 << 20;
    /// PowerShell's `<# #>` block comments.
    pub const PSBLK: u32 = 1 << 21;
    /// `name:` at the start of a line is an entity (a make target).
    pub const TARGET: u32 = 1 << 22;
    /// `-` inside names (shell commands' options, PowerShell's `-eq`).
    pub const DASHID: u32 = 1 << 23;
    /// A string followed by `:` is a key (JSON).
    pub const KEYS: u32 = 1 << 24;
    /// `:symbol` is a constant (Ruby).
    pub const SYM: u32 = 1 << 25;
    /// C#'s `@"…"` and `$"…"`.
    pub const CSHARP: u32 = 1 << 26;
    /// PowerShell: a backtick escapes in `"…"`, and `'…'` has no escapes.
    pub const PS: u32 = 1 << 27;
    /// JavaScript: `/…/flags` regex literals.
    pub const REGEX: u32 = 1 << 28;
}

use f::*;

/// The flags most C-like languages share.
const CLIKE: u32 = C2 | BLOCK | SQ | CALL;

/// A language's table.
pub struct Lang {
    /// `names|keywords|constants`, each space-separated. The names are what a fence's info
    /// string may say (case-insensitive). The keywords and the constants are sorted bytewise,
    /// with no duplicates; a keyword ending in `*` (`fn*`) makes the name after it an entity.
    spec: &'static str,
    flags: u32,
    mode: Mode,
}

/// The tables. A fence's language is a 1-based index into them (0 is none).
pub static LANGS: &[Lang] = &[
    Lang {
        spec: "rust rs|as async await bool break char const continue crate dyn else enum* extern f32 f64 fn* for i128 i16 i32 i64 i8 if impl in isize let loop macro_rules match mod* move mut pub ref return static str struct* super trait* type* u128 u16 u32 u64 u8 union unsafe use usize where while yield|Err None Ok Self Some false self true",
        flags: C2 | BLOCK | NEST | CALL | ML | RUST,
        mode: Mode::Code,
    },
    Lang {
        spec: "python py python3 py3 gyp|and as assert async await break case class* continue def* del elif else except finally for from global if import in is lambda match nonlocal not or pass raise return try while with yield|Exception False KeyError None True TypeError ValueError all any bool cls dict enumerate filter float int isinstance len list map max min object open print range repr self set sorted str sum super tuple type zip",
        flags: HASH | SQ | TRIPLE | CALL | AT | PY,
        mode: Mode::Code,
    },
    Lang {
        spec: "javascript js jsx mjs cjs node typescript ts tsx mts cts|abstract any as async await boolean break case catch class* const continue debugger declare default delete do else enum export extends finally for from function* get if implements import in instanceof interface* keyof let namespace never new number object of private protected public readonly return satisfies set static string super switch symbol this throw try type* typeof unknown var void while with yield|Infinity NaN console document exports false module null process require true undefined window",
        flags: CLIKE | BT | AT | DOLLAR_ID | REGEX,
        mode: Mode::Code,
    },
    Lang {
        spec: "json jsonc json5 geojson jsonl webmanifest||false null true",
        flags: C2 | BLOCK | KEYS,
        mode: Mode::Code,
    },
    Lang {
        spec: "yaml yml||",
        flags: 0,
        mode: Mode::Yaml,
    },
    Lang {
        spec: "toml ini cfg dosini properties editorconfig gitconfig conf||false no off on true yes",
        flags: 0,
        mode: Mode::Ini,
    },
    Lang {
        spec: "sh bash shell zsh ksh shellscript fish ash dash|alias and begin break case continue coproc declare do done elif else end esac eval exec exit export fi for function* if in local not or readonly return select set shift source switch then time trap typeset unset until while|apt awk cargo cat cd chmod chown cp curl docker echo false git grep kill ls make mkdir mv npm pip printf pwd read rm sed sudo test true wget",
        flags: HASH | SQ | ML | DOLLAR | SHELL | DASHID,
        mode: Mode::Code,
    },
    Lang {
        spec: "console shell-session sh-session shellsession bash-session terminal||",
        flags: 0,
        mode: Mode::Console,
    },
    Lang {
        spec: "c h cpp c++ cxx cc hpp hh hxx ino cuda cu objc objective-c objectivec|alignas alignof auto bool break case catch char class* co_await co_return co_yield concept const const_cast consteval constexpr constinit continue decltype default delete do double dynamic_cast else enum* explicit export extern final float for friend goto if inline int int16_t int32_t int64_t int8_t long mutable namespace* new noexcept operator override private protected public register reinterpret_cast requires restrict return short signed size_t sizeof static static_assert static_cast struct* switch template this thread_local throw try typedef typeid typename uint16_t uint32_t uint64_t uint8_t union* unsigned using virtual void volatile while|NULL false nullptr std true",
        flags: CLIKE | PRE,
        mode: Mode::Code,
    },
    Lang {
        spec: "go golang|any bool break byte case chan complex128 complex64 const continue default defer else error fallthrough float32 float64 for func* go goto if import int int16 int32 int64 int8 interface map package range return rune select string struct switch type* uint uint16 uint32 uint64 uint8 uintptr var|append cap close copy delete false iota len make new nil panic print println recover true",
        flags: CLIKE | BT,
        mode: Mode::Code,
    },
    Lang {
        spec: "java|abstract assert boolean break byte case catch char class* const continue default do double else enum* extends final finally float for goto if implements import instanceof int interface* long native new package permits private protected public record* return sealed short static strictfp super switch synchronized this throw throws transient try var void volatile while yield|false null true",
        flags: CLIKE | TRIPLE | AT,
        mode: Mode::Code,
    },
    Lang {
        spec: "kotlin kt kts|abstract annotation as break by catch class* companion const constructor continue crossinline data do else enum expect external final finally for fun* get if import in infix init inline inner interface* internal is lateinit noinline object* open operator out override package private protected public reified return sealed set super suspend tailrec this throw try typealias* val var vararg when where while|false it null println true",
        flags: CLIKE | NEST | TRIPLE | AT | DOLLAR,
        mode: Mode::Code,
    },
    Lang {
        spec: "csharp cs c# dotnet|abstract as async await base bool break byte case catch char checked class* const continue decimal default delegate do double else enum* event explicit extern finally fixed float for foreach get goto if implicit in init int interface* internal is lock long namespace* new object operator out override params private protected public readonly record* ref return sbyte sealed set short sizeof stackalloc static string struct* switch this throw try typeof uint ulong unchecked unsafe ushort using value var virtual void volatile when where while yield|false null true",
        flags: CLIKE | CSHARP | PRE,
        mode: Mode::Code,
    },
    Lang {
        spec: "swift|actor any as associatedtype async await break case catch class* continue default defer deinit do else enum* extension* fallthrough fileprivate for func* guard if import in init inout internal is let mutating nonisolated open operator private protocol* public repeat rethrows return some static struct* subscript super switch throw throws try typealias* var where while|Self false nil print self true",
        flags: CLIKE | NEST | TRIPLE | AT,
        mode: Mode::Code,
    },
    Lang {
        spec: "html htm xhtml xml svg xsd xsl xslt plist rss atom vue-html||",
        flags: 0,
        mode: Mode::Markup,
    },
    Lang {
        spec: "css||",
        flags: BLOCK | SQ,
        mode: Mode::Css,
    },
    Lang {
        spec: "scss less sass||",
        flags: C2 | BLOCK | SQ,
        mode: Mode::Css,
    },
    Lang {
        spec: "sql mysql postgresql postgres psql plsql plpgsql sqlite tsql pgsql|add all alter and as asc begin between bigint boolean by cascade case char check column commit constraint create cross database date decimal declare default delete desc distinct double drop else end exists float foreign from full function grant group having if in index inner insert int integer into is join key left like limit not null numeric offset on or order outer primary procedure real references replace returning returns revoke right rollback select serial set smallint table text then time timestamp to transaction trigger truncate union unique update using values varchar view when where with|avg coalesce count false max min now sum true",
        flags: DASH2 | BLOCK | SQ | CI | NOESC | CALL,
        mode: Mode::Code,
    },
    Lang {
        spec: "diff patch udiff||",
        flags: 0,
        mode: Mode::Diff,
    },
    Lang {
        spec: "markdown md mkd mdown gfm||",
        flags: 0,
        mode: Mode::Markdown,
    },
    Lang {
        spec: "dockerfile docker containerfile|ADD ARG AS CMD COPY ENTRYPOINT ENV EXPOSE FROM HEALTHCHECK LABEL MAINTAINER ONBUILD RUN SHELL STOPSIGNAL USER VOLUME WORKDIR|",
        flags: HASH | SQ | DOLLAR | SHELL,
        mode: Mode::Code,
    },
    Lang {
        spec: "makefile make mk mak bsdmake|define else endef endif export ifdef ifeq ifndef ifneq include override private unexport vpath|",
        flags: HASH | SQ | DOLLAR | SHELL | TARGET,
        mode: Mode::Code,
    },
    Lang {
        spec: "ruby rb jruby rake gemspec podspec|alias and attr_accessor attr_reader begin break case class* def* defined? do else elsif end ensure for if in module* next not or private protected public redo require require_relative rescue retry return super then undef unless until when while yield|false nil puts self true",
        flags: HASH | SQ | ML | CALL | ATVAR | SYM,
        mode: Mode::Code,
    },
    Lang {
        spec: "php|abstract and array as break callable case catch class* clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile enum extends final finally fn for foreach function* global goto if implements include include_once instanceof insteadof interface* isset list match namespace new or php print private protected public readonly require require_once return static switch throw trait* try unset use var while xor yield|FALSE NULL TRUE false null true",
        flags: CLIKE | HASH | ML | DOLLAR,
        mode: Mode::Code,
    },
    Lang {
        spec: "lua|and break do else elseif end for function* goto if in local not or repeat return then until while|false ipairs nil pairs print require self true",
        flags: DASH2 | SQ | CALL | LUA,
        mode: Mode::Code,
    },
    Lang {
        spec: "powershell ps1 pwsh posh psm1|-and -eq -ge -gt -le -like -lt -match -ne -not -notlike -notmatch -or begin break catch class* continue data do dynamicparam else elseif end exit filter finally for foreach from function* if in param process return switch throw trap try until using var while|$false $null $true",
        flags: HASH | SQ | CI | DOLLAR | PSBLK | DASHID | CALL | PS,
        mode: Mode::Code,
    },
];

/// The shell's table (0-based), which a console block lexes its commands with.
const SHELL_LANG: usize = 6;

/// The longest keyword or constant, in bytes; a longer name is neither.
const MAX_WORD: usize = 24;

/// The language a fenced block's info string names (`rust`, `Rust`, `rust,ignore`, `{.python}`,
/// `py title="x"`), as a 1-based index into [`LANGS`]: its first word, by the names and aliases
/// GitHub knows, whatever its case.
pub fn find(info: &str) -> Option<u8> {
    let word = info
        .trim_start_matches(|c: char| c == '{' || c == '.' || c.is_whitespace())
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '{' | '}'))
        .next()?;
    if word.is_empty() || word.len() > MAX_WORD {
        return None;
    }
    let i = LANGS.iter().position(|l| {
        l.spec
            .split('|')
            .next()
            .is_some_and(|names| names.split(' ').any(|n| n.eq_ignore_ascii_case(word)))
    })?;
    Some(i as u8 + 1)
}

/// Lex `text` with language `lang` (1-based, from [`find`]) from `state` (0 at a block's start),
/// calling `emit` with consecutive `(start, end, class)` runs that cover all of it, adjacent runs
/// of one class merged. Returns the state at its end, which the text after it starts in. A
/// `lang` that names no table makes one plain run.
pub fn lex(lang: u8, text: &str, state: u32, emit: &mut dyn FnMut(usize, usize, Tok)) -> u32 {
    let Some(lang) = LANGS.get(usize::from(lang).wrapping_sub(1)) else {
        if !text.is_empty() {
            emit(0, text.len(), Tok::Plain);
        }
        return 0;
    };
    let (kw, cs) = lists(lang);
    // Only the lexer makes states, but one whose quote isn't ASCII could end a string inside a
    // character.
    let quoted = matches!(state & 0xff, S_STR | S_TAG);
    let state = if quoted && (state >> 8) & 0x80 != 0 {
        0
    } else {
        state
    };
    let mut lx = Lexer {
        lang,
        kw,
        cs,
        b: text.as_bytes(),
        i: 0,
        state: state & !DEF_BIT,
        tok_start: 0,
        tok: Tok::Plain,
        emit,
        def_next: state & DEF_BIT != 0,
        str_open: 0,
        bloom: bloom(kw, cs),
        pending: None,
        sh_bloom: None,
        css_upto: 0,
        expr_from: 0,
        expr_end: 0,
        css_sel: false,
    };
    lx.run();
    lx.flush();
    // A heredoc opened on the last line: the text after this starts in its body.
    if let Some((h, at)) = lx.pending
        && lx.state & 0xff == S_NONE
        && at == text.len()
    {
        lx.state = here_state(h);
    }
    lx.state | if lx.def_next { DEF_BIT } else { 0 }
}

// The state: bits 0..8 its kind, 8..16 a parameter, 16..24 an extra (CSS's brace depth), and
// the two flags of a string above.
const S_NONE: u32 = 0;
/// A block comment; the parameter is its depth.
const S_COMMENT: u32 = 1;
/// A string; the parameter is its quote byte.
const S_STR: u32 = 2;
/// A Rust raw string; the parameter is its number of `#`.
const S_RAW: u32 = 3;
/// Inside an HTML or XML tag; the parameter is the quote of the attribute value it is in, if
/// any.
const S_TAG: u32 = 4;
/// An HTML comment, `<!--`.
const S_HTMLC: u32 = 5;
/// A Lua long bracket; the parameter is its level, and `TRIPLE_BIT` makes it a comment.
const S_LONG: u32 = 6;
/// A PowerShell `<#` comment.
const S_PSC: u32 = 7;
/// A YAML block scalar; the parameter is its key's indent.
const S_BLOCK: u32 = 8;
/// A fenced block in Markdown; the parameter is its fence's length.
const S_FENCE: u32 = 9;
/// A console command continued after a backslash.
const S_CONT: u32 = 10;
/// A shell heredoc's body; the parameters (8..16 and 16..24) are a hash of its terminator.
const S_HERE: u32 = 11;
/// A triple-quoted string (or a Lua long comment).
const TRIPLE_BIT: u32 = 1 << 24;
/// A string without backslash escapes.
const NOESC_BIT: u32 = 1 << 25;
/// The last keyword names what follows (`fn` at a line's end), kept in the state between
/// calls only.
const DEF_BIT: u32 = 1 << 26;
/// A C# verbatim string: no escapes, `""` is a quote, and it spans lines.
const VERB_BIT: u32 = 1 << 27;

/// The bits of the state kept across a construct: CSS's brace depth.
const EXTRA: u32 = 0x00ff_0000;

/// A table's keywords and constants.
fn lists(lang: &Lang) -> (&'static str, &'static str) {
    let mut parts = lang.spec.split('|');
    parts.next();
    (parts.next().unwrap_or(""), parts.next().unwrap_or(""))
}

/// A word's bit in the filter: its length and its first and last bytes, whatever their case.
fn hash(w: &[u8]) -> usize {
    let (Some(f), Some(l)) = (w.first(), w.last()) else {
        return 0;
    };
    let (f, l) = (
        f.to_ascii_lowercase() as usize,
        l.to_ascii_lowercase() as usize,
    );
    (w.len() * 37 + f * 7 + l * 3) & 255
}

/// A 256-bit filter of the words of the two lists: most names are neither, and are let through
/// without a search.
fn bloom(kw: &str, cs: &str) -> [u64; 4] {
    let mut m = [0u64; 4];
    for k in kw.split(' ').chain(cs.split(' ')) {
        let k = k.trim_end_matches('*').as_bytes();
        if !k.is_empty() {
            let h = hash(k);
            m[h >> 6] |= 1 << (h & 63);
        }
    }
    m
}

fn is_id(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

#[cfg(test)]
thread_local! {
    /// The steps lexed on this thread, for the tests of linear work.
    static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The bytes the lookaheads (and look-behinds) touched, for the same tests.
    static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count `n` bytes scanned by a lookahead, in the tests.
#[inline(always)]
fn scanned(_n: usize) {
    #[cfg(test)]
    SCANNED.with(|s| s.set(s.get() + _n));
}

/// How far a `/` looks for the end of a regex literal.
const REGEX_SPAN: usize = 256;

struct Lexer<'a> {
    lang: &'static Lang,
    kw: &'static str,
    cs: &'static str,
    b: &'a [u8],
    i: usize,
    state: u32,
    /// The run being built: where it starts, and its class.
    tok_start: usize,
    tok: Tok,
    emit: &'a mut dyn FnMut(usize, usize, Tok),
    /// The last keyword was one that names what follows (`fn`).
    def_next: bool,
    /// Where the string being lexed opened (a JSON key is retagged from there).
    str_open: usize,
    bloom: [u64; 4],
    /// A heredoc opened on this line: the hash of its terminator, and where its body starts (the
    /// next line). A token that runs over that point (a string) drops it.
    pending: Option<(u32, usize)>,
    /// The shell's filter, made at the first command of a console block.
    sh_bloom: Option<[u64; 4]>,
    /// CSS: whether the names before `css_upto` are a selector's.
    css_upto: usize,
    css_sel: bool,
    /// YAML: no `}}` from this byte to `expr_end` (a `${{` there is not looked into again).
    expr_from: usize,
    expr_end: usize,
}

impl Lexer<'_> {
    fn has(&self, fl: u32) -> bool {
        self.lang.flags & fl != 0
    }

    fn at(&self, k: usize) -> u8 {
        self.b.get(self.i + k).copied().unwrap_or(0)
    }

    fn starts(&self, s: &[u8]) -> bool {
        self.b[self.i..].starts_with(s)
    }

    /// Mark `start..end` as `tok`, merging it with the run before it.
    fn put(&mut self, start: usize, end: usize, tok: Tok) {
        if end <= start {
            return;
        }
        if tok != self.tok {
            if start > self.tok_start {
                (self.emit)(self.tok_start, start, self.tok);
            }
            self.tok_start = start;
            self.tok = tok;
        }
    }

    fn flush(&mut self) {
        if self.b.len() > self.tok_start {
            (self.emit)(self.tok_start, self.b.len(), self.tok);
        }
        self.tok_start = self.b.len();
    }

    /// Advance to `end`, all of it `tok`.
    fn take(&mut self, end: usize, tok: Tok) {
        let end = end.min(self.b.len());
        if end > self.i {
            self.put(self.i, end, tok);
            self.i = end;
        }
    }

    fn line_end(&self) -> usize {
        let end = self.b[self.i..]
            .spos(|&c| c == b'\n')
            .map_or(self.b.len(), |p| self.i + p);
        scanned(end - self.i);
        end
    }

    /// Only blanks before this byte on its line.
    fn line_start(&self) -> bool {
        let before = &self.b[..self.i];
        let p = before.srpos(|&c| c != b' ' && c != b'\t');
        scanned(before.len() - p.map_or(0, |p| p + 1) + 1);
        p.is_none_or(|p| before[p] == b'\n')
    }

    fn run(&mut self) {
        while self.i < self.b.len() {
            #[cfg(test)]
            STEPS.with(|s| s.set(s.get() + 1));
            let (i, state) = (self.i, self.state);
            self.step();
            // A safety net: every step consumes something or changes the state.
            if self.i == i && self.state == state {
                self.take(i + 1, Tok::Plain);
                while self.i < self.b.len() && self.b[self.i] & 0xc0 == 0x80 {
                    self.i += 1;
                }
            }
        }
    }

    fn step(&mut self) {
        if self.state & 0xff != S_NONE {
            self.pending = None;
            return self.resume();
        }
        if let Some((h, at)) = self.pending
            && self.i >= at
        {
            self.pending = None;
            if self.i == at {
                self.state = here_state(h);
                return self.resume();
            }
        }
        match self.lang.mode {
            Mode::Code => self.code(),
            Mode::Markup => self.markup(),
            Mode::Css => self.css(),
            Mode::Yaml => self.yaml_line(),
            Mode::Ini => self.ini_line(),
            Mode::Diff => self.diff_line(),
            Mode::Console => self.console_line(),
            Mode::Markdown => self.md_line(),
        }
    }

    /// Continue the construct the state says is open.
    fn resume(&mut self) {
        let param = (self.state >> 8) & 0xff;
        match self.state & 0xff {
            S_COMMENT => self.block_comment(param),
            S_STR => self.string(param as u8),
            S_RAW => self.raw_string(param as usize),
            S_TAG => self.in_tag(param as u8),
            S_HTMLC => self.until(b"-->", Tok::Comment),
            S_PSC => self.until(b"#>", Tok::Comment),
            S_LONG => self.long_bracket(param as usize),
            S_BLOCK => self.yaml_block(param as usize),
            S_HERE => self.heredoc(param | ((self.state >> 16) & 0xff) << 8),
            S_FENCE => self.md_fence(param as usize),
            S_CONT => {
                self.state = S_NONE;
                self.command_line();
            }
            _ => self.state = S_NONE,
        }
    }

    /// Set the state's kind, keeping its extra.
    fn keep_extra(&mut self, kind: u32) {
        self.state = (self.state & EXTRA) | kind;
    }

    /// Up to and including `end`, as `tok`; still open at the text's end without it.
    fn until(&mut self, end: &[u8], tok: Tok) {
        let kind = self.state & 0xff;
        match find_bytes(&self.b[self.i..], end) {
            Some(p) => {
                self.take(self.i + p + end.len(), tok);
                self.keep_extra(S_NONE);
            }
            None => {
                self.take(self.b.len(), tok);
                self.keep_extra(kind);
            }
        }
    }

    /// A block comment whose opening is behind us, `depth` deep.
    fn block_comment(&mut self, mut depth: u32) {
        let start = self.i;
        let nest = self.has(NEST);
        let mut j = self.i;
        while j < self.b.len() {
            if self.b[j] == b'*' && self.b.get(j + 1) == Some(&b'/') {
                j += 2;
                if depth <= 1 {
                    self.put(start, j, Tok::Comment);
                    self.i = j;
                    self.keep_extra(S_NONE);
                    return;
                }
                depth -= 1;
            } else if nest && self.b[j] == b'/' && self.b.get(j + 1) == Some(&b'*') {
                j += 2;
                depth = (depth + 1).min(255);
            } else {
                j += 1;
            }
        }
        self.put(start, j, Tok::Comment);
        self.i = j;
        self.keep_extra(S_COMMENT | depth.max(1) << 8);
    }

    /// A string whose opening quote `q` is behind us.
    fn string(&mut self, q: u8) {
        let triple = self.state & TRIPLE_BIT != 0;
        let verb = self.state & VERB_BIT != 0;
        let noesc = self.state & NOESC_BIT != 0 || verb;
        let esc = if self.has(PS) { b'`' } else { b'\\' };
        // A console block's strings are the shell's, which span lines.
        let ml = triple || verb || q == b'`' || self.has(ML) || self.lang.mode == Mode::Console;
        let start = self.i;
        let mut j = self.i;
        let n = self.b.len();
        while j < n {
            let c = self.b[j];
            // (A string that can't span lines ends at one, escaped or not.)
            if c == esc && !noesc && (ml || self.b.get(j + 1) != Some(&b'\n')) {
                j += 2;
                continue;
            }
            if verb && c == q && self.b.get(j + 1) == Some(&q) {
                j += 2;
                continue;
            }
            if c == q
                && (!triple || (self.b.get(j + 1) == Some(&q) && self.b.get(j + 2) == Some(&q)))
            {
                return self.close_str(start, j + if triple { 3 } else { 1 });
            }
            if c == b'\n' && !ml {
                return self.close_str(start, j);
            }
            j += 1;
        }
        self.put(start, n, Tok::Str);
        self.i = n;
    }

    fn close_str(&mut self, start: usize, end: usize) {
        self.put(start, end, Tok::Str);
        self.i = end;
        self.keep_extra(S_NONE);
        // A JSON key.
        if self.has(KEYS) {
            let mut k = end;
            while k < self.b.len() && (self.b[k] == b' ' || self.b[k] == b'\t') {
                k += 1;
            }
            if self.b.get(k) == Some(&b':') {
                self.put(self.str_open, end, Tok::Tag);
            }
        }
    }

    /// A string opening at `from`, its quote (`quote_len` bytes of `q`) at the current byte.
    fn open_str(&mut self, from: usize, quote_len: usize, q: u8, triple: bool) {
        self.open_str_with(from, quote_len, q, triple, 0);
    }

    /// `open_str`, the string's state also having `bits`.
    fn open_str_with(&mut self, from: usize, quote_len: usize, q: u8, triple: bool, bits: u32) {
        self.str_open = from;
        self.put(from, self.i + quote_len, Tok::Str);
        self.i += quote_len;
        let mut st = S_STR | u32::from(q) << 8 | (self.state & EXTRA) | bits;
        if triple {
            st |= TRIPLE_BIT;
        }
        // Go's raw strings have no escapes (JavaScript's template strings do).
        if self.has(NOESC)
            || (self.has(SHELL) && q == b'\'')
            || (self.has(PS) && q == b'\'')
            || (self.has(BT) && !self.has(DOLLAR_ID) && q == b'`')
        {
            st |= NOESC_BIT;
        }
        self.state = st;
        self.string(q);
    }

    /// A Rust raw string whose opening is behind us, closed by `"` and `hashes` `#`s.
    fn raw_string(&mut self, hashes: usize) {
        let start = self.i;
        let mut j = self.i;
        while j < self.b.len() {
            if self.b[j] == b'"' {
                let mut k = 0;
                while k < hashes && self.b.get(j + 1 + k) == Some(&b'#') {
                    k += 1;
                }
                if k == hashes {
                    j += 1 + hashes;
                    self.put(start, j, Tok::Str);
                    self.i = j;
                    self.keep_extra(S_NONE);
                    return;
                }
            }
            j += 1;
        }
        self.put(start, j, Tok::Str);
        self.i = j;
    }

    /// A Lua long bracket whose opening (of `level` `=`s) is behind us.
    fn long_bracket(&mut self, level: usize) {
        let tok = if self.state & TRIPLE_BIT != 0 {
            Tok::Comment
        } else {
            Tok::Str
        };
        let start = self.i;
        let mut j = self.i;
        while j < self.b.len() {
            if self.b[j] == b']' {
                let mut k = 0;
                while k < level && self.b.get(j + 1 + k) == Some(&b'=') {
                    k += 1;
                }
                if k == level && self.b.get(j + 1 + k) == Some(&b']') {
                    j += 2 + level;
                    self.put(start, j, tok);
                    self.i = j;
                    self.keep_extra(S_NONE);
                    return;
                }
            }
            j += 1;
        }
        self.put(start, j, tok);
        self.i = j;
    }

    /// `[[`, `[=[`, …: the level, if a Lua long bracket opens at `at`.
    fn long_open(&self, at: usize) -> Option<usize> {
        if self.b.get(at) != Some(&b'[') {
            return None;
        }
        let mut k = 0;
        while self.b.get(at + 1 + k) == Some(&b'=') && k < 255 {
            k += 1;
        }
        (self.b.get(at + 1 + k) == Some(&b'[')).then_some(k)
    }

    fn word_end(&self, from: usize) -> usize {
        let dash = self.has(DASHID);
        let dollar = self.has(DOLLAR_ID);
        let mut j = from;
        while let Some(&c) = self.b.get(j) {
            if !(is_id(c) || (dash && c == b'-' && j > from) || (dollar && c == b'$')) {
                break;
            }
            j += 1;
        }
        j
    }

    /// Whether `w` is in `list` (`Some(true)` for a keyword that names what follows).
    fn lookup(&self, list: &str, w: &[u8]) -> Option<bool> {
        if w.is_empty() || w.len() > MAX_WORD || list.is_empty() {
            return None;
        }
        let h = hash(w);
        if self.bloom[h >> 6] & 1 << (h & 63) == 0 {
            return None;
        }
        let mut low = [0u8; MAX_WORD];
        let w = if self.has(CI) {
            let low = &mut low[..w.len()];
            low.copy_from_slice(w);
            low.make_ascii_lowercase();
            low
        } else {
            w
        };
        search(list.as_bytes(), w)
    }

    fn number(&mut self) {
        let mut j = self.i + 1;
        let hex = self.b[self.i] == b'0' && matches!(self.at(1), b'x' | b'X');
        while let Some(&c) = self.b.get(j) {
            let more = c.is_ascii_alphanumeric()
                || c == b'_'
                || (c == b'.' && self.b.get(j + 1).is_some_and(u8::is_ascii_digit))
                || ((c == b'+' || c == b'-') && !hex && matches!(self.b[j - 1], b'e' | b'E'));
            if !more {
                break;
            }
            j += 1;
        }
        self.take(j, Tok::Constant);
    }

    /// One token of a programming language.
    fn code(&mut self) {
        let c = self.b[self.i];
        let fl = self.lang.flags;
        // Comments.
        if fl & C2 != 0 && self.starts(b"//") {
            return self.take(self.line_end(), Tok::Comment);
        }
        if fl & BLOCK != 0 && self.starts(b"/*") {
            self.put(self.i, self.i + 2, Tok::Comment);
            self.i += 2;
            return self.block_comment(1);
        }
        if fl & LUA != 0
            && self.starts(b"--")
            && let Some(level) = self.long_open(self.i + 2)
        {
            self.put(self.i, self.i + 4 + level, Tok::Comment);
            self.i += 4 + level;
            self.state = S_LONG | (level as u32) << 8 | TRIPLE_BIT;
            return self.long_bracket(level);
        }
        if fl & DASH2 != 0 && self.starts(b"--") {
            return self.take(self.line_end(), Tok::Comment);
        }
        if fl & PSBLK != 0 && self.starts(b"<#") {
            self.state = S_PSC;
            return self.until(b"#>", Tok::Comment);
        }
        if c == b'#' {
            if fl & PRE != 0 && self.line_start() {
                return self.preprocessor();
            }
            let after_space =
                self.i == 0 || matches!(self.b[self.i - 1], b' ' | b'\t' | b'\n' | b';');
            if fl & HASH != 0 && (fl & SHELL == 0 || after_space) {
                return self.take(self.line_end(), Tok::Comment);
            }
        }
        // Strings.
        if c == b'"' {
            let triple = fl & TRIPLE != 0 && self.starts(b"\"\"\"");
            return self.open_str(self.i, if triple { 3 } else { 1 }, b'"', triple);
        }
        if c == b'\'' {
            if fl & RUST != 0 {
                return self.rust_quote();
            }
            if fl & SQ != 0 {
                let triple = fl & TRIPLE != 0 && self.starts(b"'''");
                return self.open_str(self.i, if triple { 3 } else { 1 }, b'\'', triple);
            }
        }
        if c == b'`' && fl & BT != 0 {
            return self.open_str(self.i, 1, b'`', false);
        }
        if fl & LUA != 0
            && let Some(level) = self.long_open(self.i)
        {
            self.put(self.i, self.i + 2 + level, Tok::Str);
            self.i += 2 + level;
            self.state = S_LONG | (level as u32) << 8;
            return self.long_bracket(level);
        }
        if fl & CSHARP != 0 && (c == b'@' || c == b'$') {
            // `@"…"` (verbatim), `$"…"`, `$@"…"` and `@$"…"`.
            let (n, verbatim) = match (c, self.at(1), self.at(2)) {
                (b'@', b'"', _) => (1, true),
                (b'$', b'"', _) => (1, false),
                (_, b'@' | b'$', b'"') if self.at(1) != c => (2, true),
                _ => (0, false),
            };
            if n > 0 {
                let from = self.i;
                self.i += n;
                let bits = if verbatim { VERB_BIT } else { 0 };
                return self.open_str_with(from, 1, b'"', false, bits);
            }
        }
        if fl & REGEX != 0 && c == b'/' && self.regex_start() {
            let e = self.regex_end();
            if e > self.i {
                self.def_next = false;
                return self.take(e, Tok::Str);
            }
        }
        if c.is_ascii_digit() {
            return self.number();
        }
        // Variables and decorators.
        if c == b'$' && fl & DOLLAR != 0 {
            return self.dollar();
        }
        if c == b'@' && fl & (AT | ATVAR) != 0 {
            // Ruby's class variables: `@@name`.
            let at = if fl & ATVAR != 0 && self.at(1) == b'@' {
                2
            } else {
                1
            };
            if is_id(self.at(at)) {
                let e = self.word_end(self.i + at);
                return self.take(e, if fl & AT != 0 { Tok::Entity } else { Tok::Var });
            }
        }
        if c == b':'
            && fl & SYM != 0
            && is_id(self.at(1))
            && !self.at(1).is_ascii_digit()
            && (self.i == 0 || !(self.b[self.i - 1] == b':' || is_id(self.b[self.i - 1])))
        {
            let e = self.word_end(self.i + 1);
            return self.take(e, Tok::Constant);
        }
        // Names.
        if is_id(c)
            || (c == b'-' && fl & DASHID != 0 && self.at(1).is_ascii_alphabetic())
            || (c == b'$' && fl & DOLLAR_ID != 0)
        {
            return self.name();
        }
        // Spaces and punctuation that open nothing, in one run, to the end of the line (where a
        // console command ends).
        if c == b'<' && fl & SHELL != 0 {
            self.heredoc_open();
        }
        let shell = fl & SHELL != 0;
        let mut e = self.i + 1;
        if shell && c == b'\\' {
            // A backslash escapes the next character, a quote among them.
            e += self.b.get(e).map_or(0, |&n| match n {
                0xF0.. => 4,
                0xE0.. => 3,
                0xC0.. => 2,
                _ => 1,
            });
            e = e.min(self.b.len());
        }
        while e < self.b.len()
            && self.b[e - 1] != b'\n'
            && self.inert(self.b[e])
            && !(shell && self.b[e] == b'<' && self.b.get(e + 1) == Some(&b'<'))
        {
            e += 1;
        }
        // Punctuation ends a definition's wait for its name (`func (s *T) Name`).
        if self.b[self.i..e].sany(|c| !c.is_ascii_whitespace()) {
            self.def_next = false;
        }
        self.take(e, Tok::Plain);
    }

    /// Whether a `/` at the cursor may start a regex literal: where a value is expected.
    fn regex_start(&self) -> bool {
        let before = &self.b[..self.i];
        let Some(p) = before.srpos(|&c| c != b' ' && c != b'\t') else {
            return true;
        };
        scanned(before.len() - p);
        // No `}` here: it ends a block, or a JSX expression (`{n}/{total}`).
        match before[p] {
            b'\n' | b'(' | b',' | b'=' | b':' | b'[' | b'!' | b'&' | b'|' | b'?' | b'{' | b';'
            | b'+' | b'-' | b'*' | b'%' | b'~' | b'^' => true,
            b'>' => p > 0 && before[p - 1] == b'=',
            c if c.is_ascii_alphabetic() => {
                let from = before[..=p]
                    .srpos(|&c| !c.is_ascii_alphabetic())
                    .map_or(0, |q| q + 1);
                scanned(p + 1 - from);
                matches!(
                    &before[from..=p],
                    b"return"
                        | b"typeof"
                        | b"instanceof"
                        | b"in"
                        | b"of"
                        | b"void"
                        | b"delete"
                        | b"throw"
                        | b"new"
                        | b"case"
                        | b"do"
                        | b"else"
                        | b"yield"
                        | b"await"
                )
            }
            _ => false,
        }
    }

    /// The end of the regex literal opening at the cursor, with its flags; the cursor if it
    /// doesn't close on this line, or within `REGEX_SPAN` bytes (it is a division then).
    fn regex_end(&self) -> usize {
        let mut j = self.i + 1;
        let mut class = false;
        while let Some(&c) = self.b.get(j) {
            if j > self.i + REGEX_SPAN {
                break;
            }
            scanned(1);
            match c {
                b'\n' => break,
                b'\\' if self.b.get(j + 1).is_some_and(|&n| n != b'\n') => j += 1,
                b'[' => class = true,
                b']' => class = false,
                b'/' if !class => {
                    j += 1;
                    while self.b.get(j).is_some_and(u8::is_ascii_alphabetic) {
                        j += 1;
                    }
                    return j;
                }
                _ => {}
            }
            j += 1;
        }
        self.i
    }

    /// Whether the cursor is inside `((…` on its line, looking back 128 bytes at most (`$((a << b))`).
    fn in_arith(&self) -> bool {
        let from = self.i.saturating_sub(128);
        let before = &self.b[from..self.i];
        let line = before.srpos(|&c| c == b'\n').map_or(0, |p| p + 1);
        let before = &before[line..];
        scanned(before.len());
        before
            .windows(2)
            .rposition(|w| w == b"((")
            .is_some_and(|p| !before[p + 2..].windows(2).any(|w| w == b"))"))
    }

    /// At a `<`: note a heredoc (`<<EOF`, `<<-EOF`, `<< EOF`, `<<'EOF'`) opening here, for its
    /// body.
    fn heredoc_open(&mut self) {
        if !self.starts(b"<<")
            || self.at(2) == b'<'
            || (self.i > 0 && (self.b[self.i - 1] == b'<' || self.b[self.i - 1].is_ascii_digit()))
        {
            return;
        }
        let mut k = self.i + 2;
        if self.b.get(k) == Some(&b'-') {
            k += 1;
        }
        // A space before the word (`cat << EOF`): the word is an identifier or a quoted one, and
        // the `<<` is not arithmetic.
        let spaced = matches!(self.b.get(k), Some(b' ' | b'\t'));
        if spaced {
            let blanks = self.b[k..]
                .iter()
                .take(16)
                .take_while(|&&c| c == b' ' || c == b'\t')
                .count();
            scanned(blanks);
            let word = self.b.get(k + blanks).copied().unwrap_or(0);
            let before = self.b[..self.i]
                .iter()
                .rev()
                .take(16)
                .find(|&&c| c != b' ' && c != b'\t');
            if !(is_id(word) || word == b'\'' || word == b'"')
                || before.is_some_and(u8::is_ascii_digit)
                || self.in_arith()
            {
                return;
            }
            k += blanks;
        } else if self.in_arith() {
            // A shift without blanks (`(( v = x<<n ))`).
            return;
        }
        let quote = self.b.get(k).copied().filter(|&q| q == b'\'' || q == b'"');
        if quote.is_some() {
            k += 1;
        }
        let mut e = k;
        while e < self.b.len()
            && e < k + 64
            && match quote {
                Some(q) => self.b[e] != q && self.b[e] != b'\n',
                None => is_id(self.b[e]),
            }
        {
            e += 1;
        }
        let closed = quote.is_none_or(|q| self.b.get(e) == Some(&q));
        if e > k && closed && (quote.is_some() || !self.b[k].is_ascii_digit()) {
            // The body starts on the line after this one: another opener on the same line shares
            // it, so the line's end is looked up once.
            let at = match self.pending {
                Some((_, at)) if at > self.i => at,
                _ => self.line_end() + 1,
            };
            self.pending = Some((word_hash(&self.b[k..e]), at));
        }
    }

    /// A line of a heredoc's body: a string, up to the line of its terminator.
    fn heredoc(&mut self, hash: u32) {
        let end = self.line_end();
        let mut line = self.b[self.i..end].trim_ascii();
        // In a console, the body's lines carry the `> ` of a continuation prompt.
        if self.lang.mode == Mode::Console
            && let Some(rest) = line.strip_prefix(b">")
        {
            line = rest.trim_ascii_start();
        }
        let closes = word_hash(line) == hash;
        self.take(end + usize::from(end < self.b.len()), Tok::Str);
        if closes {
            self.state = S_NONE;
        }
    }

    /// `#include <…>` and other directives, at a line's start.
    fn preprocessor(&mut self) {
        let e = self.word_end(self.i + 1);
        let include = &self.b[self.i + 1..e] == b"include";
        self.take(e, Tok::Keyword);
        if !include {
            return;
        }
        while self.at(0) == b' ' {
            self.take(self.i + 1, Tok::Plain);
        }
        if self.at(0) == b'<' {
            let end = self.b[self.i..]
                .spos(|&c| c == b'>' || c == b'\n')
                .map_or(self.b.len(), |p| self.i + p + 1);
            self.take(end, Tok::Str);
        }
    }

    /// A byte that starts no token in this language.
    fn inert(&self, c: u8) -> bool {
        match c {
            b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'{' | b'}' | b',' | b';' | b'='
            | b'+' | b'*' | b'&' | b'|' | b'!' | b'?' | b'^' | b'~' | b'%' | b'.' | b'>' | b']' => {
                true
            }
            b'<' => !self.has(PSBLK),
            b'[' => !self.has(LUA),
            b':' => !self.has(SYM),
            _ => false,
        }
    }

    /// A name: a keyword, a constant, an entity or plain; or a string's prefix.
    fn name(&mut self) {
        let start = self.i;
        let mut e = self.word_end(start + 1);
        let fl = self.lang.flags;
        let next = self.b.get(e).copied().unwrap_or(0);
        let w = &self.b[start..e];
        // String prefixes: `r"…"`, `f'…'`, `b"…"`, Rust's `b'x'`.
        if fl & (PY | RUST) != 0
            && w.len() <= 2
            && (next == b'"' || next == b'\'')
            && w.sall(|c| b"rRbBfFuUc".contains(c))
        {
            if fl & RUST != 0 && next == b'\'' {
                if w == b"b" {
                    self.put(start, e, Tok::Str);
                    self.i = e;
                    return self.rust_quote();
                }
            } else if !(fl & RUST != 0 && w.contains(&b'r')) {
                self.i = e;
                let triple = fl & TRIPLE != 0 && self.b[e..].starts_with(&[next; 3]);
                return self.open_str(start, if triple { 3 } else { 1 }, next, triple);
            }
        }
        if fl & RUST != 0 && matches!(w, b"r" | b"br" | b"cr") && (next == b'#' || next == b'"') {
            let mut h = 0;
            while self.b.get(e + h) == Some(&b'#') && h < 255 {
                h += 1;
            }
            if self.b.get(e + h) == Some(&b'"') {
                self.put(start, e + h + 1, Tok::Str);
                self.i = e + h + 1;
                self.state = S_RAW | (h as u32) << 8;
                return self.raw_string(h);
            }
        }
        // Ruby's `defined?`, `save!`.
        if fl & SYM != 0 && (next == b'?' || next == b'!') {
            e += 1;
        }
        let w = &self.b[start..e];
        if let Some(def) = self.lookup(self.kw, w) {
            self.def_next = def;
            return self.take(e, Tok::Keyword);
        }
        let tok = if self.lookup(self.cs, w).is_some() {
            Tok::Constant
        } else if self.def_next
            || (fl & TARGET != 0
                && next == b':'
                && self.b.get(e + 1) != Some(&b'=')
                && self.line_start())
            || (fl & CALL != 0 && next == b'(')
            || (fl & RUST != 0 && next == b'!' && self.b.get(e + 1) != Some(&b'='))
        {
            Tok::Entity
        } else {
            Tok::Plain
        };
        self.def_next = false;
        self.take(e, tok);
    }

    /// Rust: `'a'` and `'\n'` are chars, `'a` a lifetime.
    fn rust_quote(&mut self) {
        let s = self.i;
        let b = self.b;
        if b.get(s + 1) == Some(&b'\\') {
            // An escape: up to the closing quote on this line, at most 12 bytes on. The byte
            // after the backslash is skipped (`'\''`), unless it ends the line.
            let from = if b.get(s + 2) == Some(&b'\n') {
                s + 2
            } else {
                s + 3
            };
            let end = b
                .get(from..)
                .unwrap_or(&[])
                .iter()
                .take(12)
                .position(|&c| c == b'\'' || c == b'\n')
                .map_or(s + 2, |p| from + p + 1);
            return self.take(end, Tok::Str);
        }
        // One char (of 1 to 4 bytes) and a quote.
        let first = b.get(s + 1).copied().unwrap_or(0);
        let len = match first {
            0..0x80 => 1,
            0xc0..0xe0 => 2,
            0xe0..0xf0 => 3,
            _ => 4,
        };
        if first != b'\'' && first != b'\n' && b.get(s + 1 + len) == Some(&b'\'') {
            return self.take(s + 2 + len, Tok::Str);
        }
        if first.is_ascii_alphanumeric() || first == b'_' {
            let e = self.word_end(s + 1);
            return self.take(e, Tok::Keyword);
        }
        self.take(s + 1, Tok::Plain);
    }

    /// `$name`, `${…}`, `$(…)`, `$1`, `$?`.
    fn dollar(&mut self) {
        let s = self.i;
        let n1 = self.at(1);
        let end = if n1 == b'{' || n1 == b'(' {
            let close = if n1 == b'{' { b'}' } else { b')' };
            self.b[s + 2..]
                .spos(|&c| c == close || c == b'\n')
                .map_or(self.b.len(), |p| s + 2 + p + 1)
        } else if is_id(n1) && !n1.is_ascii_digit() {
            let mut j = s + 1;
            while j < self.b.len() && is_id(self.b[j]) {
                j += 1;
            }
            j
        } else if n1.is_ascii_digit()
            || matches!(n1, b'@' | b'?' | b'$' | b'#' | b'*' | b'!' | b'-')
        {
            s + 2
        } else {
            s + 1
        };
        // PowerShell's `$true` is a constant.
        let tok = if end == s + 1 {
            Tok::Plain
        } else if self.lookup(self.cs, &self.b[s..end]).is_some() {
            Tok::Constant
        } else {
            Tok::Var
        };
        self.take(end, tok);
    }

    // HTML and XML.

    fn markup(&mut self) {
        let c = self.b[self.i];
        if self.starts(b"<!--") {
            self.state = S_HTMLC;
            return self.until(b"-->", Tok::Comment);
        }
        if c == b'<'
            && (self.at(1).is_ascii_alphabetic() || matches!(self.at(1), b'/' | b'!' | b'?'))
        {
            let skip = if matches!(self.at(1), b'/' | b'!' | b'?') {
                2
            } else {
                1
            };
            self.take(self.i + skip, Tok::Plain);
            let e = self.tag_name_end();
            self.take(e, Tok::Tag);
            self.state = S_TAG;
            return;
        }
        if c == b'&' {
            // An entity: `&amp;`, `&#38;`.
            let e = self.b[self.i + 1..]
                .iter()
                .take(32)
                .position(|&c| !(c.is_ascii_alphanumeric() || c == b'#'))
                .map(|p| self.i + 1 + p);
            if let Some(e) = e
                && e > self.i + 1
                && self.b.get(e) == Some(&b';')
            {
                return self.take(e + 1, Tok::Constant);
            }
        }
        // Text up to the next `<` or `&`.
        let e = self.b[self.i + 1..]
            .spos(|&c| c == b'<' || c == b'&')
            .map_or(self.b.len(), |p| self.i + 1 + p);
        self.take(e, Tok::Plain);
    }

    fn tag_name_end(&self) -> usize {
        let mut e = self.i;
        while e < self.b.len() && (is_id(self.b[e]) || matches!(self.b[e], b'-' | b':' | b'.')) {
            e += 1;
        }
        e
    }

    /// A tag's attributes, up to its `>`.
    fn in_tag(&mut self, quote: u8) {
        if quote != 0 {
            return self.attr_value(quote);
        }
        let c = self.b[self.i];
        match c {
            b'>' => {
                self.take(self.i + 1, Tok::Plain);
                self.state = S_NONE;
            }
            b'"' | b'\'' => {
                self.take(self.i + 1, Tok::Str);
                self.attr_value(c);
            }
            _ if is_id(c) => {
                let e = self.tag_name_end();
                self.take(e, Tok::Entity);
            }
            _ => self.take(self.i + 1, Tok::Plain),
        }
    }

    /// An attribute's value, whose opening `quote` is behind us (it may span lines).
    fn attr_value(&mut self, quote: u8) {
        match self.b[self.i..].spos(|&c| c == quote) {
            Some(p) => {
                self.take(self.i + p + 1, Tok::Str);
                self.state = S_TAG;
            }
            None => {
                self.take(self.b.len(), Tok::Str);
                self.state = S_TAG | u32::from(quote) << 8;
            }
        }
    }

    // CSS.

    fn css(&mut self) {
        let c = self.b[self.i];
        let depth = (self.state >> 16) & 0xff;
        if self.has(C2) && self.starts(b"//") {
            return self.take(self.line_end(), Tok::Comment);
        }
        if self.starts(b"/*") {
            self.put(self.i, self.i + 2, Tok::Comment);
            self.i += 2;
            return self.block_comment(1);
        }
        match c {
            b'"' | b'\'' => self.open_str(self.i, 1, c, false),
            b'{' => {
                self.take(self.i + 1, Tok::Plain);
                self.state = (depth + 1).min(255) << 16;
            }
            b'}' => {
                self.take(self.i + 1, Tok::Plain);
                self.state = depth.saturating_sub(1) << 16;
            }
            b'@' => {
                let e = self.word_end(self.i + 1);
                self.take(e, Tok::Keyword);
            }
            b'!' if self.b[self.i + 1..].starts_with(b"important") => {
                self.take(self.i + 10, Tok::Keyword);
            }
            b'#' | b'.' if depth == 0 && is_id(self.at(1)) => {
                let e = self.css_word(self.i + 1);
                self.take(e, Tok::Entity);
            }
            // A colour.
            b'#' if is_id(self.at(1)) => {
                let e = self.css_word(self.i + 1);
                self.take(e, Tok::Constant);
            }
            b'0'..=b'9' => self.number(),
            b'.' if self.at(1).is_ascii_digit() => self.number(),
            _ if is_id(c) || c == b'-' => {
                let e = self.css_word(self.i);
                // Inside a rule, properties and values; a selector's element names outside one,
                // or a nested rule's (a `{` comes before a `;` or `}`). The answer holds up to
                // the byte that decided it.
                if e >= self.css_upto {
                    let lim = (e + 256).min(self.b.len());
                    let stop = self.b[e..lim].spos(|&c| matches!(c, b'{' | b';' | b'}' | b'\n'));
                    self.css_upto = stop.map_or(lim, |p| e + p);
                    self.css_sel = stop.is_some_and(|p| self.b[e + p] == b'{');
                }
                let selector = depth == 0 || self.css_sel;
                self.take(e, if selector { Tok::Tag } else { Tok::Constant });
            }
            _ => self.take(self.i + 1, Tok::Plain),
        }
    }

    fn css_word(&self, from: usize) -> usize {
        let mut j = from;
        while j < self.b.len() && (is_id(self.b[j]) || self.b[j] == b'-') {
            j += 1;
        }
        j.max(from + 1).min(self.b.len())
    }

    // The line-oriented modes.

    fn indent(&self) -> usize {
        let n = self.b[self.i..]
            .iter()
            .take_while(|&&c| c == b' ' || c == b'\t')
            .count();
        scanned(n);
        n
    }

    /// The rest of the line, to `end`, as values: strings, numbers, constants, comments
    /// (starting with a byte of `comment` after a blank) and bare words (`unquoted`).
    fn values(&mut self, end: usize, comment: &[u8], unquoted: Tok) {
        // Inside an inline table's braces (TOML), a bare word before `=` is a key.
        let mut braces = 0u32;
        while self.i < end {
            let c = self.b[self.i];
            if comment.contains(&c) && (self.i == 0 || matches!(self.b[self.i - 1], b' ' | b'\t')) {
                return self.take(end, Tok::Comment);
            }
            match c {
                b'"' | b'\'' => {
                    let triple = self.b[self.i..].starts_with(&[c; 3]);
                    let q = if triple { 3 } else { 1 };
                    // Double-quoted strings have `\"`; single-quoted (TOML's literal) have none.
                    let close = if q == 1 && c == b'"' {
                        let from = self.i + 1;
                        let mut j = from;
                        while j < end && self.b[j] != b'"' {
                            j += if self.b[j] == b'\\' { 2 } else { 1 };
                        }
                        scanned(j.min(end) - from);
                        (j < end).then(|| j - from)
                    } else {
                        find_bytes(&self.b[self.i + q..end], &[c; 3][..q])
                    };
                    match close {
                        Some(p) => self.take(self.i + q + p + q, Tok::Str),
                        // A TOML multi-line string.
                        None if triple => return self.open_str(self.i, 3, c, true),
                        None => self.take(end, Tok::Str),
                    }
                }
                b' ' | b'\t' | b',' | b'[' | b']' | b'{' | b'}' | b'=' => {
                    match c {
                        b'{' => braces += 1,
                        b'}' => braces = braces.saturating_sub(1),
                        _ => {}
                    }
                    self.take(self.i + 1, Tok::Plain);
                }
                _ => self.scalar(end, comment, unquoted, braces > 0),
            }
        }
    }

    /// A bare value, up to a separator or a comment.
    fn scalar(&mut self, end: usize, comment: &[u8], unquoted: Tok, inline: bool) {
        let ini = self.lang.mode == Mode::Ini;
        let mut e = self.i;
        // GitHub Actions' `${{ … }}` is part of the value.
        let mut expr = self.lang.mode == Mode::Yaml;
        while e < end
            && !matches!(self.b[e], b',' | b']' | b'}' | b'\n')
            && !(inline && ini && matches!(self.b[e], b'=' | b'"' | b'\''))
            && !(comment.contains(&self.b[e])
                && e > self.i
                && matches!(self.b[e - 1], b' ' | b'\t'))
        {
            if expr && self.b[e..end].starts_with(b"${{") {
                // A search that failed covers every later `${{` up to its end: searching again
                // from each one, a line of `${{,` was quadratic.
                if e >= self.expr_from && end <= self.expr_end {
                    expr = false;
                } else {
                    match find_bytes(&self.b[e + 3..end], b"}}") {
                        Some(p) => {
                            e += p + 5;
                            continue;
                        }
                        None => {
                            (self.expr_from, self.expr_end) = (e, end);
                            expr = false;
                        }
                    }
                }
            }
            e += 1;
        }
        scanned(e - self.i);
        let mut t = e;
        while t > self.i && matches!(self.b[t - 1], b' ' | b'\t' | b'\r') {
            t -= 1;
        }
        scanned(e - t);
        let w = &self.b[self.i..t];
        if inline && ini && t > self.i {
            let after = self.b[t..end].sfind(|&c| c != b' ' && c != b'\t');
            if after == Some(&b'=') {
                return self.take(t, Tok::Tag);
            }
        }
        let num = match w {
            [d, ..] if d.is_ascii_digit() => true,
            [b'-' | b'+' | b'.', d, ..] => d.is_ascii_digit(),
            _ => false,
        } && w.sall(|c| c.is_ascii_alphanumeric() || b"._-+:".contains(c));
        let lit = self.lookup(self.cs, w).is_some()
            || matches!(
                w,
                b"true" | b"false" | b"null" | b"~" | b"True" | b"False" | b"Null" | b"NULL"
            );
        self.take(
            t.max(self.i + 1),
            if num || lit { Tok::Constant } else { unquoted },
        );
        // What was scanned is consumed: the blanks and `\r` after the word are plain.
        self.take(e, Tok::Plain);
    }

    fn eol(&mut self) {
        if self.at(0) == b'\n' {
            self.take(self.i + 1, Tok::Plain);
        }
    }

    fn yaml_line(&mut self) {
        let line_start = self.i;
        let end = self.line_end();
        let ind = self.indent();
        self.take(self.i + ind, Tok::Plain);
        if self.i >= end {
            return self.eol();
        }
        if self.starts(b"#") || self.starts(b"---") || self.starts(b"...") {
            self.take(end, Tok::Comment);
            return self.eol();
        }
        // Sequence items.
        while self.i < end && self.at(0) == b'-' && (self.at(1) == b' ' || self.i + 1 == end) {
            let sp = self.b[self.i + 1..end]
                .iter()
                .take_while(|&&c| c == b' ')
                .count();
            self.take(self.i + 1 + sp, Tok::Plain);
        }
        if self.i >= end {
            return self.eol();
        }
        // A key: up to `: ` or a `:` at the end, outside quotes.
        let col = self.i - line_start;
        let line = &self.b[self.i..end];
        let key = if let Some(&q @ (b'"' | b'\'')) = line.first() {
            line[1..]
                .spos(|&c| c == q)
                .map(|p| p + 2)
                .filter(|&k| line.get(k) == Some(&b':'))
        } else {
            let mut k = None;
            scanned(line.len());
            for (p, &c) in line.iter().enumerate() {
                if c == b'#' && p > 0 && line[p - 1] == b' ' {
                    break;
                }
                if c == b':' && matches!(line.get(p + 1), None | Some(b' ' | b'\t' | b'\r')) {
                    k = Some(p);
                    break;
                }
            }
            k
        };
        if let Some(k) = key {
            self.take(self.i + k, Tok::Tag);
            self.take(self.i + 1, Tok::Plain);
        }
        // Anchors, aliases and tags.
        loop {
            let sp = self.b[self.i..end]
                .iter()
                .take_while(|&&c| c == b' ')
                .count();
            self.take(self.i + sp, Tok::Plain);
            if self.i >= end {
                break;
            }
            match self.at(0) {
                b'&' | b'*' if is_id(self.at(1)) => {
                    let e = self.word_end(self.i + 1);
                    self.take(e, Tok::Var);
                }
                b'!' => {
                    let e = self.b[self.i..end]
                        .spos(|&c| c == b' ')
                        .map_or(end, |p| self.i + p);
                    self.take(e, Tok::Keyword);
                }
                _ => break,
            }
        }
        // A block scalar: `|`, `>-`, `|2`.
        let header = if self.i < end && matches!(self.at(0), b'|' | b'>') {
            // Indicators and blanks, then maybe a comment.
            let rest = &self.b[self.i + 1..end];
            let m = rest
                .iter()
                .take_while(|c| b"+-0123456789 \r".contains(c))
                .count();
            scanned(m);
            match rest.get(m) {
                None => Some(end),
                Some(b'#') if m > 0 && rest[m - 1] == b' ' => Some(self.i + 1 + m),
                _ => None,
            }
        } else {
            None
        };
        if let Some(h) = header {
            self.take(h, Tok::Keyword);
            self.take(end, Tok::Comment);
            self.eol();
            // The block is indented past its key's column (after a `- `), or a bare item's dash.
            let key_col = if key.is_some() { col } else { ind };
            self.state = S_BLOCK | (key_col.min(255) as u32) << 8;
            return;
        }
        self.values(end, b"#", Tok::Str);
        self.eol();
    }

    /// A line of a block scalar whose key is indented `key_ind`: text while it is indented more.
    fn yaml_block(&mut self, key_ind: usize) {
        let end = self.line_end();
        let ind = self.indent();
        let blank = self.i + ind >= end || self.b[self.i + ind] == b'\r';
        if !blank && ind <= key_ind {
            self.state = S_NONE;
            return self.yaml_line();
        }
        self.take(end, Tok::Str);
        self.eol();
    }

    fn ini_line(&mut self) {
        let end = self.line_end();
        let ind = self.indent();
        self.take(self.i + ind, Tok::Plain);
        if self.i >= end {
            return self.eol();
        }
        match self.at(0) {
            b'#' | b';' => self.take(end, Tok::Comment),
            b'[' => {
                let close = self.b[self.i..end]
                    .srpos(|&c| c == b']')
                    .map_or(end, |p| self.i + p + 1);
                self.take(close, Tok::Entity);
                self.values(end, b"#;", Tok::Plain);
            }
            _ => {
                // `key = value` or `key: value`; a line without one is a value (in an array).
                let line = &self.b[self.i..end];
                let key = line
                    .spos(|&c| matches!(c, b'=' | b':' | b'"' | b'\'' | b'[' | b'{' | b'#'))
                    .filter(|&p| p > 0 && matches!(line[p], b'=' | b':'));
                if let Some(k) = key {
                    let name = line[..k]
                        .srpos(|&c| c != b' ' && c != b'\t')
                        .map_or(0, |p| p + 1);
                    self.take(self.i + name, Tok::Tag);
                    self.take(self.i + k - name + 1, Tok::Plain);
                }
                self.values(end, b"#;", Tok::Plain);
            }
        }
        self.eol();
    }

    fn diff_line(&mut self) {
        let tok = match self.at(0) {
            b'+' if self.starts(b"+++ ") => Tok::Constant,
            b'-' if self.starts(b"--- ") => Tok::Constant,
            b'+' | b'>' => Tok::Inserted,
            b'-' | b'<' => Tok::Deleted,
            b'!' => Tok::Var,
            b'@' if self.starts(b"@@") => Tok::Meta,
            b'd' if self.starts(b"diff ") => Tok::Constant,
            b'i' if self.starts(b"index ") => Tok::Constant,
            _ => Tok::Plain,
        };
        self.take(self.line_end(), tok);
        self.eol();
    }

    fn console_line(&mut self) {
        let end = self.line_end();
        let line = &self.b[self.i..end];
        let prompt = [&b"$ "[..], b"% ", b"> ", "❯ ".as_bytes()]
            .iter()
            .find(|p| line.starts_with(p))
            .map(|p| p.len())
            .or_else(|| {
                // `PS C:\x> `, `user@host:~$ `.
                let p = find_bytes(line, b"> ").or_else(|| find_bytes(line, b"$ "))?;
                (p < 64 && (line.starts_with(b"PS ") || line[..p].contains(&b'@'))).then_some(p + 2)
            });
        match prompt {
            Some(p) => {
                self.take(self.i + p, Tok::Comment);
                self.command_line();
            }
            None => {
                self.take(end, Tok::Plain);
                self.eol();
            }
        }
    }

    /// A shell command, to the end of its line (and the next, after a backslash).
    fn command_line(&mut self) {
        let end = self.line_end();
        let cont = end > self.i && self.b[end - 1] == b'\\';
        let saved = (self.lang, self.kw, self.cs, self.bloom);
        self.lang = &LANGS[SHELL_LANG];
        let (kw, cs) = lists(self.lang);
        (self.kw, self.cs) = (kw, cs);
        self.bloom = *self.sh_bloom.get_or_insert_with(|| bloom(kw, cs));
        while self.i < end && self.state & 0xff == S_NONE {
            let i = self.i;
            self.code();
            if self.i == i {
                self.take(i + 1, Tok::Plain);
            }
        }
        (self.lang, self.kw, self.cs, self.bloom) = saved;
        self.def_next = false;
        // A string that ran on past the line had the backslash, and ended the command.
        if self.state & 0xff == S_NONE && self.i <= end + 1 {
            if self.i == end {
                self.eol();
            }
            if cont {
                self.state = S_CONT;
            }
        }
    }

    fn md_line(&mut self) {
        let end = self.line_end();
        let ind = self.indent();
        self.take(self.i + ind, Tok::Plain);
        if self.i >= end {
            return self.eol();
        }
        let line = &self.b[self.i..end];
        let run = |b: u8| line.iter().take_while(|&&c| c == b).count();
        let fence = run(b'`').max(run(b'~'));
        if fence >= 3 {
            self.take(end, Tok::Str);
            self.eol();
            self.state = S_FENCE | (fence.min(255) as u32) << 8;
            return;
        }
        let hashes = run(b'#');
        if (1..=6).contains(&hashes) && matches!(line.get(hashes), None | Some(b' ')) {
            self.take(end, Tok::Constant);
            return self.eol();
        }
        // A list or quote marker.
        let digits = line.iter().take_while(|c| c.is_ascii_digit()).count();
        let marker = if matches!(line.first(), Some(b'-' | b'*' | b'+' | b'>'))
            && matches!(line.get(1), None | Some(b' '))
        {
            1
        } else if (1..10).contains(&digits)
            && matches!(line.get(digits), Some(b'.' | b')'))
            && matches!(line.get(digits + 1), None | Some(b' '))
        {
            digits + 1
        } else {
            0
        };
        self.take(self.i + marker, Tok::Var);
        // Code spans, link targets and comments.
        while self.i < end {
            match self.at(0) {
                b'`' => {
                    let n = self.b[self.i..end]
                        .iter()
                        .take_while(|&&c| c == b'`')
                        .count();
                    let close = if n <= 8 {
                        find_bytes(&self.b[self.i + n..end], &[b'`'; 8][..n])
                    } else {
                        None
                    };
                    match close {
                        Some(p) => self.take(self.i + n + p + n, Tok::Str),
                        None => self.take(self.i + n, Tok::Plain),
                    }
                }
                b']' if self.at(1) == b'(' => {
                    self.take(self.i + 1, Tok::Plain);
                    let e = self.b[self.i..end]
                        .spos(|&c| c == b')')
                        .map_or(end, |p| self.i + p + 1);
                    self.take(e, Tok::Str);
                }
                b'<' if self.starts(b"<!--") => {
                    // Closed on this line, the rest of the line is inline text: not a line
                    // start, and not scanned again from here (that was quadratic).
                    self.state = S_HTMLC;
                    self.until(b"-->", Tok::Comment);
                    if self.state & 0xff != S_NONE || self.i > end {
                        return;
                    }
                }
                _ => {
                    let e = self.b[self.i + 1..end]
                        .spos(|&c| matches!(c, b'`' | b']' | b'<'))
                        .map_or(end, |p| self.i + 1 + p);
                    self.take(e, Tok::Plain);
                }
            }
        }
        self.eol();
    }

    /// A line inside a fenced block of a fence `n` long: a string, up to a closing fence.
    fn md_fence(&mut self, n: usize) {
        let end = self.line_end();
        let line = &self.b[self.i + self.indent()..end];
        let run = |b: u8| line.iter().take_while(|&&c| c == b).count();
        let closes = line.len() >= n
            && run(b'`').max(run(b'~')) >= n
            && line
                .iter()
                .skip(n)
                .all(|&c| matches!(c, b'`' | b'~' | b' ' | b'\r'));
        self.take(end, Tok::Str);
        self.eol();
        if closes {
            self.state = S_NONE;
        }
    }
}

/// Binary search of a sorted, space-separated word list; `Some(true)` for a word marked `*`.
fn search(list: &[u8], w: &[u8]) -> Option<bool> {
    let (mut lo, mut hi) = (0, list.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let mut s = mid;
        while s > lo && list[s - 1] != b' ' {
            s -= 1;
        }
        let mut e = s;
        while e < hi && list[e] != b' ' {
            e += 1;
        }
        let def = e > s && list[e - 1] == b'*';
        let k = &list[s..e - usize::from(def)];
        match k.cmp(w) {
            std::cmp::Ordering::Equal => return Some(def),
            std::cmp::Ordering::Less => lo = e + 1,
            std::cmp::Ordering::Greater => hi = s.saturating_sub(1).max(lo),
        }
    }
    None
}

/// A heredoc terminator's hash, of 16 bits.
fn word_hash(w: &[u8]) -> u32 {
    let h = w.iter().fold(0x811c_9dc5u32, |h, &c| {
        (h ^ u32::from(c)).wrapping_mul(0x0100_0193)
    });
    (h ^ h >> 16) & 0xffff
}

/// The state in a heredoc's body, its terminator hashed `h`.
fn here_state(h: u32) -> u32 {
    S_HERE | (h & 0xff) << 8 | (h >> 8) << 16
}

/// Slice scans that count the bytes they touch, for the tests of linear work: every lookahead
/// that does not consume what it scans goes through these.
trait Scan {
    fn spos(&self, f: impl Fn(&u8) -> bool) -> Option<usize>;
    fn srpos(&self, f: impl Fn(&u8) -> bool) -> Option<usize>;
    fn sfind(&self, f: impl Fn(&u8) -> bool) -> Option<&u8>;
    fn sany(&self, f: impl Fn(&u8) -> bool) -> bool;
    fn sall(&self, f: impl Fn(&u8) -> bool) -> bool;
}

impl Scan for [u8] {
    fn spos(&self, f: impl Fn(&u8) -> bool) -> Option<usize> {
        let p = self.iter().position(f);
        scanned(p.map_or(self.len(), |p| p + 1));
        p
    }
    fn srpos(&self, f: impl Fn(&u8) -> bool) -> Option<usize> {
        let p = self.iter().rposition(f);
        scanned(p.map_or(self.len(), |p| self.len() - p));
        p
    }
    fn sfind(&self, f: impl Fn(&u8) -> bool) -> Option<&u8> {
        self.spos(f).map(|p| &self[p])
    }
    fn sany(&self, f: impl Fn(&u8) -> bool) -> bool {
        self.spos(f).is_some()
    }
    fn sall(&self, f: impl Fn(&u8) -> bool) -> bool {
        self.spos(|c| !f(c)).is_none()
    }
}

/// The first `needle` in `hay`.
fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let (&first, rest) = needle.split_first()?;
    let mut i = 0;
    while let Some(p) = hay.get(i..)?.spos(|&c| c == first) {
        i += p;
        if hay[i + 1..].starts_with(rest) {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests;
