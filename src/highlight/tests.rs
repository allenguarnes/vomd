use super::*;

/// A short sample of each table, by its first name.
const SAMPLES: &[(&str, &str)] = &[
    (
        "rust",
        r##"/// Doc /* nested */ comment
#[derive(Debug)]
pub fn parse<'a>(s: &'a str) -> Option<u32> {
    let c = '\n'; let d = 'x'; let r = r#"raw "q" "#;
    println!("{} {}", 0x1F, 1.5e-3);
    if s.len() > 2 { Some(42) } else { None }
}"##,
    ),
    (
        "python",
        r#"@dataclass
class Foo(Base):
    """Doc
    string"""
    def bar(self, x=1.5):  # comment
        return f"hi {x}" + r'\d' if x else None"#,
    ),
    (
        "javascript",
        "interface User { name: string; age?: number }
export async function load(id: number): Promise<User> {
  const $el = await fetch(`/api/${id}
  `); // fetch
  return res.json() as User;
}",
    ),
    (
        "json",
        r#"{"name": "x", "n": [1, 2.5, -3e2], "ok": true, "z": null}"#,
    ),
    (
        "yaml",
        "# config
name: vomd
version: 1.7
on: [push, \"pull_request\"]
jobs:
  build:
    - run: cargo test # c
      env: &e
        X: true
    script: |
      echo hi
      echo there
    after: *e",
    ),
    (
        "toml",
        "[package]
name = \"vomd\" # c
deps = [
  \"a\", 1, true,
]
doc = \"\"\"
two lines\"\"\"
[[bin]]",
    ),
    (
        "sh",
        "#!/bin/bash
for f in *.md; do echo \"file: $f ${HOME}\" | grep -v '#x'; done # c
apt-get install -y --no-install-recommends curl",
    ),
    (
        "console",
        "$ cargo build --release
   Compiling vomd v1.7.0
user@host:~$ echo \"hi\" \\
  && ls
hi
PS C:\\> Get-Item x",
    ),
    (
        "c",
        "#include <stdio.h>
struct point { int x; };
int main(void) { printf(\"%d\\n\", 0xff); return 0; } /* done */",
    ),
    (
        "go",
        "package main
import \"fmt\"
func main() { fmt.Println(`raw`, 42, nil) }",
    ),
    (
        "java",
        "@Override
public class Foo extends Bar {
    String s = \"\"\"
        text\"\"\"; // c
}",
    ),
    (
        "kotlin",
        "data class User(val name: String)
fun greet(u: User) = println(\"hi ${u.name} $x\") /* a /* b */ c */",
    ),
    (
        "csharp",
        "#region x
namespace App { class Foo { string p = @\"C:\\x\"; var s = $\"{p}\"; } }",
    ),
    (
        "swift",
        "struct Point { var x = 0 }
func area(_ p: Point) -> Int { return p.x * 2 } // c",
    ),
    (
        "html",
        "<!-- c -->
<a href=\"x.html\" class='b'>Hi &amp; bye</a>
<br/>",
    ),
    (
        "css",
        "/* c */
a:hover, .btn #id { color: #fff; margin: 0 1.5em !important; }
@media (max-width: 600px) { body { font: 12px \"Inter\"; } }",
    ),
    (
        "scss",
        "// c
$size: 4px;
.nav { ul { margin: $size; } }",
    ),
    (
        "sql",
        "SELECT id, COUNT(*) FROM users WHERE name = 'it''s' -- c
/* two */ insert into t values (1, null);",
    ),
    (
        "diff",
        "diff --git a/x b/x
--- a/x
+++ b/x
@@ -1,2 +1,2 @@
-old
+new
 same",
    ),
    (
        "markdown",
        "# Title
- item `code` [link](http://x)
```rust
let x = 1;
```
1. two <!-- c -->",
    ),
    (
        "dockerfile",
        "FROM rust:1.89 AS build
RUN cargo build --release # build
COPY --from=build /app/target/release/vomd /usr/local/bin/
ENV PATH=$HOME/bin",
    ),
    (
        "makefile",
        "CC := gcc
all: build # c
build:
\t$(CC) -o app main.c
ifeq ($(OS),Windows_NT)
endif",
    ),
    (
        "ruby",
        "class Foo < Bar
  def hi(x) = puts \"hi #{x}\" unless x.nil? # c
  @count = @@total + :sym.size
end",
    ),
    (
        "php",
        "<?php
function hi($name) { echo \"hi $name\"; } // c
$x = new Foo(); # c",
    ),
    (
        "lua",
        "--[==[ long
comment ]==]
local function f(t) return t[1] .. [[long
string]] end -- c",
    ),
    (
        "powershell",
        "<# block
comment #>
function Get-X { param($Name) if ($Name -eq $null) { Write-Host \"x\" } } # c",
    ),
];

/// Lex `text` whole, checking that the runs are non-empty, contiguous, cover it and fall on
/// character boundaries; the runs and the state at the end.
fn runs(lang: u8, text: &str, state: u32) -> (Vec<(usize, usize, Tok)>, u32) {
    let mut out: Vec<(usize, usize, Tok)> = Vec::new();
    let end = lex(lang, text, state, &mut |s, e, t| {
        let at = out.last().map_or(0, |r| r.1);
        assert_eq!(s, at, "a gap or overlap before {s}");
        assert!(e > s, "an empty run at {s}");
        assert!(text.is_char_boundary(s) && text.is_char_boundary(e));
        assert!(out.last().is_none_or(|r| r.2 != t), "unmerged runs at {s}");
        out.push((s, e, t));
    });
    assert_eq!(out.last().map_or(0, |r| r.1), text.len(), "not covered");
    (out, end)
}

fn lang(name: &str) -> u8 {
    find(name).unwrap_or_else(|| panic!("no table for {name}"))
}

/// One line per run: its class and its text.
fn tokens(lang: u8, text: &str) -> String {
    runs(lang, text, 0)
        .0
        .iter()
        .map(|&(s, e, t)| format!("{t:?}\t{:?}\n", &text[s..e]))
        .collect()
}

#[test]
fn every_table_has_a_sample() {
    let mut named: Vec<u8> = SAMPLES.iter().map(|(n, _)| lang(n)).collect();
    named.sort_unstable();
    named.dedup();
    assert_eq!(named.len(), LANGS.len());
}

#[test]
fn token_snapshots() {
    for (name, text) in SAMPLES {
        insta::assert_snapshot!(*name, tokens(lang(name), text));
    }
}

/// Inputs that end inside something, or are easy to get wrong.
const EDGES: &[(&str, &str)] = &[
    ("rust", "/* open"),
    ("rust", "/* /* */ still */ x"),
    ("rust", "r##\"a \"# still\"## x"),
    ("rust", "r#\"open"),
    ("rust", "'a 'a' '\\u{1F600}' '😀' b'x' '"),
    ("rust", "\"open \\"),
    ("json", "{\"a \\\n\": 1}"),
    ("python", "'''open\n"),
    ("python", "rb'x' f\"{y}\" u"),
    ("lua", "--[==[ x ]] ]=] ]==] y [[ open"),
    ("html", "<!-- open"),
    ("html", "<a b='open"),
    ("html", "&amp &#; &"),
    ("html", "<a b=\"two\nlines\" c>x"),
    ("rust", "struct\nName fn\n\nf"),
    ("console", "$ echo \"a \\\nb\" c\n$ d"),
    ("console", "$ a \\\n  &"),
    ("console", "$ a \\\n\n  b"),
    ("yaml", "a: |\n  b\n\n  c\nd: 1"),
    ("yaml", "- - x\n-\n'k': v"),
    ("toml", "a = \"\"\"\nopen"),
    ("toml", "[open\nk"),
    ("console", "$ a \\\n b \\\n c\nout"),
    ("console", "$ echo \"two\nlines\" x\nout"),
    ("powershell", "<# open"),
    ("markdown", "````\n```\n````\n# h"),
    ("css", "a { b { c: d } } }}} e"),
    ("c", "#include <open\n#"),
    ("csharp", "@\"open"),
    ("ruby", "@@ @ : :1 x?"),
    ("makefile", "a:= b\nc: d"),
    ("sql", "'no \\' escape"),
    ("sh", "${open $( $"),
    ("diff", "+\n-\n@@\n!"),
    (
        "toml",
        "serde = { version = \"1.0\", features = [\"derive\"] }\nx = { a = 'b', c = 1 }",
    ),
    (
        "go",
        "func (s *Server) Start() { x := `C:\\dir\\`; y := `two\nlines` }",
    ),
    // The byte after an escape's backslash is skipped, unless it is the line end: a cut there
    // must not change the runs (found by the fuzzer).
    ("rust", "'\\\n'\\"),
    ("rust", "let q = '\\''; let r = '\\\\'; '\\"),
    (
        "csharp",
        "var s = @\"C:\\temp\\\"; var t = @\"a \"\"b\"\"\nc\"; var u = $@\"x\ny\"; @\"open \"\"",
    ),
    ("powershell", "\"a`\"b\" 'it''s' \"C:\\temp\\\" \"open`"),
    (
        "javascript",
        "x = /\\/*$/g.test(a) / 2; y = /[/]'/; return /open",
    ),
    (
        "sh",
        "cat <<EOF | tr a b\nIt's here\nEOF\necho 'x' <<-'END'\n\tdone\n\tEND\nls <<< 'a' ; echo $((1<<3))\ncat <<EOF\nopen",
    ),
    ("sh", "cat <<EOF\n"),
    // A string that runs over the opening line's end leaves no heredoc (found by the fuzzer).
    ("sh", "cat <<EOF \"a\nb\" c\nd\nEOF\ne"),
    ("sh", "cat <<EOF 'a\nb' c\nd\n"),
    (
        "yaml",
        "- run: |\n    a\n  shell: bash\n- |\n  text\n- b: 1",
    ),
    // Heredocs in a console, with `> ` prompts, and spaced; a regex that doesn't close; JSX.
    (
        "console",
        "$ cat <<EOF\n> it's\n> EOF\n$ ls\n$ cat <<EOF\n> open",
    ),
    ("console", "$ cat << 'EOF'\nbody\nEOF\n$ ls <<EOF"),
    (
        "sh",
        "cat << EOF\nIt's\nEOF\ncat <<- \"E\"\n\tx\n\tE\necho $((a << b)) $(( 1 << 2 ))",
    ),
    ("sh", "x << 3; y 2 << a; ((i << j)); cat << EOF"),
    ("sh", "(( v = x<<n ))\necho \"$v\"\nls"),
    ("sh", "echo \\\"hello\\\"\necho done\n\\é \\"),
    ("toml", "a = \"echo \\\"hi\\\"\" # c\nb = 'x\\' y\nc = \"\\"),
    ("yaml", "run: | # c\n  # d\n  a: b\nx: >-2 # c\n  y\nz: |#c"),
    (
        "yaml",
        "os: ${{ matrix.os }}\nb: x-${{ a, b }}-y ${{ open\nc: ${{ ${{ ${{",
    ),
    ("yaml", "a: x \r \r\r\nb:\r\n- c \r"),
    ("toml", "a = 1 \r\r\nb = [x , \r]\r"),
    ("ini", "a= \r \r\nb={ c=d \r}"),
    ("javascript", "x = (/[ a / b; <p>{n}/{t} done</p>; y = (/[/"),
];

#[test]
fn edge_cases_are_covered_whole() {
    for (name, text) in EDGES {
        runs(lang(name), text, 0);
    }
}

#[test]
fn edge_cases_end_open() {
    let open = |name, text| runs(lang(name), text, 0).1 & 0xff;
    assert_eq!(open("rust", "/* open"), S_COMMENT);
    assert_eq!(open("rust", "/* /* */"), S_COMMENT);
    assert_eq!(open("rust", "/* /* */ */"), S_NONE);
    assert_eq!(open("rust", "r#\"open"), S_RAW);
    assert_eq!(open("python", "'''open\n"), S_STR);
    assert_eq!(open("lua", "[[ open"), S_LONG);
    assert_eq!(open("html", "<!-- open"), S_HTMLC);
    assert_eq!(open("html", "<a b"), S_TAG);
    assert_eq!(open("yaml", "a: |\n  b\n"), S_BLOCK);
    assert_eq!(open("toml", "a = \"\"\"\nopen"), S_STR);
    assert_eq!(open("console", "$ a \\\n"), S_CONT);
    assert_eq!(open("powershell", "<# open"), S_PSC);
    assert_eq!(open("markdown", "```\nx\n"), S_FENCE);
    // A C string doesn't continue past its line.
    assert_eq!(open("c", "\"open\n"), S_NONE);
}

#[test]
fn ruby_class_variables_are_variables() {
    let text = "@@total @count";
    let (r, _) = runs(lang("ruby"), text, 0);
    assert_eq!(r[0], (0, 7, Tok::Var));
    assert_eq!(r[2], (8, 14, Tok::Var));
}

/// Every line end (where VOMD cuts an oversized block) of `text`.
fn line_ends(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.match_indices('\n').map(|(i, _)| i + 1)
}

/// Lexed in two pieces at `cut`, the second from the first's state: the same runs as whole
/// (with a run that the cut splits joined again).
fn split_matches_whole(lang: u8, text: &str, cut: usize) {
    let (whole, end) = runs(lang, text, 0);
    let (mut a, state) = runs(lang, &text[..cut], 0);
    let (b, end_b) = runs(lang, &text[cut..], state);
    for (s, e, t) in b {
        match a.last_mut() {
            Some(last) if last.2 == t && last.1 == cut + s => last.1 = cut + e,
            _ => a.push((cut + s, cut + e, t)),
        }
    }
    assert_eq!(a, whole, "{text:?} cut at {cut}");
    assert_eq!(end_b, end, "{text:?} cut at {cut}");
}

#[test]
fn a_cut_at_a_line_end_changes_nothing() {
    for (name, text) in SAMPLES.iter().chain(EDGES) {
        for cut in line_ends(text) {
            split_matches_whole(lang(name), text, cut);
        }
    }
}

#[test]
fn any_cut_and_any_state_is_safe() {
    // Not the same runs (a cut can split a word), but always well formed.
    let mut seed: u32 = 0x9e37_79b9;
    for (name, text) in SAMPLES.iter().chain(EDGES) {
        let l = lang(name);
        for cut in (0..=text.len()).filter(|&c| text.is_char_boundary(c)) {
            let (_, state) = runs(l, &text[..cut], 0);
            runs(l, &text[cut..], state);
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            runs(l, &text[cut..], seed);
        }
    }
}

#[test]
fn a_state_with_a_quote_past_ascii_is_reset() {
    // A string "quoted" by the lead byte of `é` would end inside the character.
    let state = S_STR | 0xc3 << 8;
    for l in 1..=LANGS.len() as u8 {
        runs(l, "aé b", state);
    }
}

#[test]
fn no_table_is_one_plain_run() {
    for l in [0, LANGS.len() as u8 + 1, u8::MAX] {
        assert_eq!(runs(l, "fn x", 7), (vec![(0, 4, Tok::Plain)], 0));
        assert_eq!(runs(l, "", 7), (vec![], 0));
    }
}

/// Repeated, these make the most runs, or open what never closes.
const ADVERSARIAL: &[&str] = &[
    "\"",
    "'",
    "r#",
    "/*",
    "*/",
    "[[",
    "[=[",
    "<",
    "<a ",
    "&",
    "\\",
    "`",
    "$(",
    "${",
    "- ",
    ": ",
    "# ",
    "@@ ",
    "$ x \\\n",
    "```\n",
    "a(",
    "fn ",
    "'a",
    "0x",
    "1e+",
    "<!--",
    "-- ",
    "\t",
    "a:",
    "#a",
    ".a{",
    "`` ` ",
    "@@a",
    ":a",
    "(/[",
    "(/[a",
    "<<E ",
    "<<'E'",
    "<< E ",
    "<!---->",
    "<!-- a --> ",
    "${{,",
    "a: ${{,",
    "<<-\"E\" ",
    "x /",
    "= /a",
    "{n}/",
    "a/b/",
    "((<< a ",
    "$((a << b ",
    // Ordinary code, on one long line.
    "f(x, /a+/g) + a / b; if (y) { return z; } // c ",
    "cat <<EOF | tr a b; echo \"$x\" ",
    "\r",
    " \r",
    "\r\t",
    "a: \r",
    "a = \r",
    "(( x<<n ",
    "(( x<<n ))\n",
];

/// Whether a lookahead touches no more than a constant (256, a regex's span, and a little) bytes
/// per byte of text.
const SCAN_PER_BYTE: usize = 300;

#[test]
fn work_is_linear() {
    for pat in ADVERSARIAL {
        let text = pat.repeat((1 << 16) / pat.len());
        for l in 1..=LANGS.len() as u8 {
            STEPS.with(|s| s.set(0));
            SCANNED.with(|s| s.set(0));
            runs(l, &text, 0);
            let steps = STEPS.with(std::cell::Cell::get);
            let bytes = SCANNED.with(std::cell::Cell::get);
            assert!(
                steps <= 2 * text.len(),
                "{pat:?} in table {l}: {steps} steps"
            );
            assert!(
                bytes <= SCAN_PER_BYTE * text.len(),
                "{pat:?} in table {l}: {bytes} bytes scanned"
            );
        }
    }
}

#[test]
fn a_shift_without_blanks_is_not_a_heredoc() {
    for t in [
        "(( v = x<<n ))\nls it's",
        "((v=x<<n))\nls it's",
        "(( v <<= n ))\nls it's",
    ] {
        let (_, state) = runs(lang("sh"), t, 0);
        assert_ne!(state & 0xff, S_HERE, "{t:?}");
        assert_ne!(class_of("sh", t, "ls"), Tok::Str, "{t:?}");
    }
    // The same without the parentheses is a heredoc, and so is one after a closed `((…))`.
    let (_, state) = runs(lang("sh"), "x<<n\n", 0);
    assert_eq!(state & 0xff, S_HERE);
    let (_, state) = runs(lang("sh"), "((a)); cat <<EOF\n", 0);
    assert_eq!(state & 0xff, S_HERE);
}

#[test]
fn text_after_a_markdown_comment_is_inline() {
    let t = "<!-- a --> # x\n<!-- b --> - y `z`\n<!-- c\n--> w\n# h";
    assert_eq!(class_of("markdown", t, "a"), Tok::Comment);
    assert_eq!(class_of("markdown", t, "# x"), Tok::Plain);
    assert_eq!(class_of("markdown", t, "- y"), Tok::Plain);
    assert_eq!(class_of("markdown", t, "`z`"), Tok::Str);
    assert_eq!(class_of("markdown", t, "c"), Tok::Comment);
    assert_eq!(class_of("markdown", t, "# h"), Tok::Constant);
    for cut in line_ends(t) {
        split_matches_whole(lang("markdown"), t, cut);
    }
}

#[test]
fn runs_of_cr_in_a_scalar_are_split_safe_and_linear() {
    for (name, t) in [
        ("yaml", "a: x \r \r\r\nb: 1 \r# c\r\n"),
        ("toml", "a = 1 \r\r\nb = [x , \r]\r# c"),
        ("ini", "a= \r \r\nb={ c=d \r}"),
    ] {
        for cut in line_ends(t) {
            split_matches_whole(lang(name), t, cut);
        }
        // The blanks and `\r` after a word stay plain, and a comment after them is one.
        let (r, _) = runs(lang(name), t, 0);
        assert_eq!(r.last().map(|r| r.1), Some(t.len()));
    }
    let t = format!("a:{}", "\r".repeat(1 << 16));
    SCANNED.with(|s| s.set(0));
    runs(lang("yaml"), &t, 0);
    assert!(SCANNED.with(std::cell::Cell::get) <= SCAN_PER_BYTE * t.len());
}

/// The units whose repeats the sweep lexes: every ASCII byte and `é`, and every pair of the
/// lexer's punctuation.
fn sweep_units() -> Vec<String> {
    let mut units: Vec<String> = (0u8..0x80).map(|b| char::from(b).to_string()).collect();
    units.push("é".into());
    let punct: Vec<char> = "\r\n\t \"'/*#<-[{(\\$@:=|`;>)]}&!.,?~%^+_a1"
        .chars()
        .collect();
    for a in &punct {
        for b in &punct {
            units.push(format!("{a}{b}"));
        }
    }
    // Whole constructs, opened and closed: a repeated pair can't build these.
    for c in [
        "/**/",
        "<!---->",
        "<##>",
        "--[[]]",
        "--[==[]==]",
        r##"r#""#"##,
        r#""""a""""#,
        "'''a'''",
        "<<E\nE\n",
        "<<'E'\nE\n",
        "`a`",
        "${a}",
        "$(a)",
        "{{a}}",
        r#"<a b="c">"#,
        "</a>",
        r#"@"a""#,
        "/a/g ",
        "(/a/)",
        "[a](b)",
        "``a``",
        "#a\n",
        "a: |\n  b\n",
        "x = { a = 1 }\n",
        "${{,",
        "a: ${{,",
        "${{ a }},",
        "@@ -1 +1 @@\n",
        "$ a\n",
        "> a\n",
        "<!-- a --> # b\n",
    ] {
        units.push(c.into());
    }
    units
}

/// Slow (every table, about 700 units, 64 KB each): run in release, at the final check.
#[test]
#[ignore = "slow: cargo test --release -- --ignored every_repeated_unit_is_scanned_linearly"]
fn every_repeated_unit_is_scanned_linearly() {
    for unit in sweep_units() {
        let text = unit.repeat((1 << 16) / unit.len());
        for l in 1..=LANGS.len() as u8 {
            STEPS.with(|s| s.set(0));
            SCANNED.with(|s| s.set(0));
            runs(l, &text, 0);
            let steps = STEPS.with(std::cell::Cell::get);
            let bytes = SCANNED.with(std::cell::Cell::get);
            assert!(
                steps <= 2 * text.len(),
                "{unit:?} in table {l}: {steps} steps"
            );
            assert!(
                bytes <= SCAN_PER_BYTE * text.len(),
                "{unit:?} in table {l}: {bytes} bytes scanned"
            );
        }
    }
}

/// A table's names, keywords and constants.
fn parts(l: &Lang) -> [Vec<&'static str>; 3] {
    let mut p = l.spec.split('|');
    let mut next = || {
        p.next()
            .unwrap_or("")
            .split(' ')
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
    };
    [next(), next(), next()]
}

#[test]
fn the_tables_are_sound() {
    let mut names = Vec::new();
    for l in LANGS {
        assert_eq!(l.spec.matches('|').count(), 2, "{}", l.spec);
        let [n, kw, cs] = parts(l);
        assert!(!n.is_empty());
        names.extend(n);
        let kw: Vec<&str> = kw.iter().map(|w| w.trim_end_matches('*')).collect();
        for list in [&kw, &cs] {
            // The binary search needs them sorted, and each word once.
            assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
            assert!(list.iter().all(|w| !w.is_empty() && w.len() <= MAX_WORD));
            if l.flags & CI != 0 {
                assert!(
                    list.iter().all(|w| *w == w.to_ascii_lowercase()),
                    "{list:?}"
                );
            }
        }
        assert!(kw.iter().all(|w| !cs.contains(w)), "{}", l.spec);
    }
    let count = names.len();
    names.sort_unstable_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by_key(|n| n.to_ascii_lowercase());
    assert_eq!(names.len(), count, "a name in two tables");
    assert!(LANGS.len() < usize::from(u8::MAX));
}

#[test]
fn every_keyword_and_constant_is_found() {
    for (i, l) in LANGS.iter().enumerate() {
        let [_, kws, css] = parts(l);
        let (kw, cs) = lists(l);
        for w in kws {
            let def = w.ends_with('*');
            let w = w.trim_end_matches('*');
            assert_eq!(search(kw.as_bytes(), w.as_bytes()), Some(def), "{w}");
            if l.mode == Mode::Code {
                let (r, _) = runs(i as u8 + 1, w, 0);
                assert_eq!(r, [(0, w.len(), Tok::Keyword)], "{w}");
            }
        }
        for w in css {
            assert_eq!(search(cs.as_bytes(), w.as_bytes()), Some(false), "{w}");
        }
    }
}

#[test]
fn console_commands_use_the_shell() {
    assert_eq!(find("sh"), Some(SHELL_LANG as u8 + 1));
    assert!(LANGS[SHELL_LANG].mode == Mode::Code && LANGS[SHELL_LANG].flags & SHELL != 0);
}

#[test]
fn info_strings() {
    let rust = find("rust");
    assert!(rust.is_some());
    for info in [
        "Rust",
        "rs",
        "RS",
        "rust,ignore",
        "{.rust}",
        " rust",
        "rust title=\"x\"",
    ] {
        assert_eq!(find(info), rust, "{info}");
    }
    assert_eq!(find("{.python}"), find("python"));
    assert_eq!(find("py title=\"x\""), find("python"));
    assert_eq!(find("c++"), find("c"));
    assert_eq!(find("c#"), find("csharp"));
    for info in [
        "",
        " ",
        "{}",
        "math",
        "text",
        "plaintext",
        "txt",
        "none",
        "rus",
        "rustt",
    ] {
        assert_eq!(find(info), None, "{info}");
    }
    assert_eq!(find(&"a".repeat(1024)), None);
    assert_eq!(find(&format!("rust {}", "a".repeat(1024))), rust);
}

#[test]
fn a_definition_may_name_on_the_next_line() {
    let (r, state) = runs(lang("rust"), "struct\n", 0);
    assert_eq!(r[0], (0, 6, Tok::Keyword));
    assert_eq!(runs(lang("rust"), "Name", state).0, [(0, 4, Tok::Entity)]);
}

/// The class of every byte of `needle` (its first occurrence in `text`), which must be one.
fn class_of(name: &str, text: &str, needle: &str) -> Tok {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"));
    let (r, _) = runs(lang(name), text, 0);
    let t = r.iter().find(|&&(s, e, _)| s <= at && at < e).unwrap().2;
    for &(s, e, c) in &r {
        if s < at + needle.len() && at < e {
            assert_eq!(c, t, "{needle:?} is more than one class in {text:?}: {r:?}");
        }
    }
    t
}

#[test]
fn toml_inline_tables_are_coloured_like_lines() {
    let line = "serde = { version = \"1.0\", features = [\"derive\"], n = 3, ok = true }";
    for key in ["serde", "version", "features", "n", "ok"] {
        assert_eq!(class_of("toml", line, key), Tok::Tag, "{key}");
    }
    assert_eq!(class_of("toml", line, "\"1.0\""), Tok::Str);
    assert_eq!(class_of("toml", line, "\"derive\""), Tok::Str);
    assert_eq!(class_of("toml", line, "3"), Tok::Constant);
    assert_eq!(class_of("toml", line, "true"), Tok::Constant);
    // A value with an `=` in it, outside braces, is not a key.
    assert_eq!(
        class_of("toml", "u = http://a?b=c", "http://a?b"),
        Tok::Plain
    );
}

#[test]
fn a_definition_ends_at_punctuation() {
    let e = |name, text, w| class_of(name, text, w) == Tok::Entity;
    let go = "func (s *Server) Start(w int) {}";
    assert!(!e("go", go, "s *Server"));
    assert!(e("go", go, "Start"));
    assert!(!e("go", "go func(w int) {}", "w"));
    assert!(!e("javascript", "a.forEach(function (item) {})", "item"));
    assert!(!e("lua", "t(function(a, b) end)", "a"));
    assert!(!e("javascript", "x = { type: 'ADD', payload }", "payload"));
    // Named ones are unaffected.
    assert!(e("javascript", "function load(a) {}", "load"));
    assert!(e("javascript", "class  Foo {}", "Foo"));
    assert!(e("rust", "fn name<T>(a: T) {}", "name"));
    assert!(e("python", "def name(self):", "name"));
}

#[test]
fn a_yaml_block_ends_at_its_items_next_key() {
    let t =
        "steps:\n  - run: |\n      cargo test\n    shell: bash\n    env:\n      A: 1\n  - uses: x";
    assert_eq!(class_of("yaml", t, "cargo test"), Tok::Str);
    assert_eq!(class_of("yaml", t, "shell"), Tok::Tag);
    assert_eq!(class_of("yaml", t, "env"), Tok::Tag);
    assert_eq!(class_of("yaml", t, "A"), Tok::Tag);
    assert_eq!(class_of("yaml", t, "uses"), Tok::Tag);
    // A bare item's block is indented past its dash.
    let t = "- |\n  text\n- k: v";
    assert_eq!(class_of("yaml", t, "text"), Tok::Str);
    assert_eq!(class_of("yaml", t, "k"), Tok::Tag);
}

#[test]
fn raw_and_verbatim_strings_have_their_own_rules() {
    // Go: no escapes, and it may span lines.
    let go = "a := `C:\\dir\\`; b := `x\ny`; c := 1";
    assert_eq!(class_of("go", go, "`C:\\dir\\`"), Tok::Str);
    assert_eq!(class_of("go", go, "`x\ny`"), Tok::Str);
    assert_eq!(class_of("go", go, "1"), Tok::Constant);
    // Rust: `'\''` is one char.
    let rs = "let q = '\\''; let n = 1;";
    assert_eq!(class_of("rust", rs, "'\\''"), Tok::Str);
    assert_eq!(class_of("rust", rs, "1"), Tok::Constant);
    // C#: no escapes, `""` is a quote, lines go on; `$"…"` keeps its escapes.
    let cs = "var a = @\"C:\\x\\\"; var b = @\"p \"\"q\"\"\nr\"; var c = 1;";
    assert_eq!(class_of("csharp", cs, "@\"C:\\x\\\""), Tok::Str);
    assert_eq!(class_of("csharp", cs, "@\"p \"\"q\"\"\nr\""), Tok::Str);
    assert_eq!(class_of("csharp", cs, "1"), Tok::Constant);
    assert_eq!(class_of("csharp", "$\"a\\\"b\" 2", "$\"a\\\"b\""), Tok::Str);
    // PowerShell: a backtick escapes, a backslash doesn't, `'…'` has no escapes.
    let ps = "\"a`\"b\" 1; \"C:\\x\\\" 2; 'a`' 3";
    assert_eq!(class_of("powershell", ps, "\"a`\"b\""), Tok::Str);
    assert_eq!(class_of("powershell", ps, "\"C:\\x\\\""), Tok::Str);
    assert_eq!(class_of("powershell", ps, "'a`'"), Tok::Str);
    assert_eq!(class_of("powershell", ps, "3"), Tok::Constant);
}

#[test]
fn javascript_regex_literals() {
    let t = "x = /\\/*$/g.test(a);\ny = a / b; z = /[/]'/i;\nw = a / 2 / c; return /x/";
    assert_eq!(class_of("javascript", t, "/\\/*$/g"), Tok::Str);
    // The line after isn't a comment, and `/'/` isn't a string.
    assert_eq!(class_of("javascript", t, "y"), Tok::Plain);
    assert_eq!(class_of("javascript", t, "/[/]'/i"), Tok::Str);
    assert_eq!(class_of("javascript", t, "w"), Tok::Plain);
    assert_eq!(class_of("javascript", t, "2"), Tok::Constant);
    assert_eq!(class_of("javascript", t, "/x/"), Tok::Str);
    // Division, and a slash that doesn't close on its line.
    let has_str = |text| {
        runs(lang("javascript"), text, 0)
            .0
            .iter()
            .any(|r| r.2 == Tok::Str)
    };
    assert!(!has_str("(a) / 2 / 3"));
    assert!(!has_str("x = /open\ny = 1"));
    // JSX closing tags aren't regexes.
    assert!(!has_str("<a>x</a> <b>y</b>"));
}

#[test]
fn shell_heredoc_bodies_are_strings() {
    let t = "cat <<EOF | tr a b\nIt's $x done\nEOF\necho 'q' <<-'END'\n\tdon't\n\tEND\nls";
    assert_eq!(class_of("sh", t, "It's $x done\nEOF"), Tok::Str);
    assert_eq!(class_of("sh", t, "don't\n\tEND"), Tok::Str);
    // The command after the body, and the one before it on the opening line, are code.
    assert_ne!(class_of("sh", t, "ls"), Tok::Str);
    assert_ne!(class_of("sh", t, "tr"), Tok::Str);
    // Here-strings and shifts aren't heredocs.
    let t = "cat <<< 'a'\necho $((1<<3))\nls it's";
    assert_ne!(class_of("sh", t, "echo"), Tok::Str);
    assert_ne!(class_of("sh", t, "ls"), Tok::Str);
    // One that opens on the last line carries over a cut.
    let (_, state) = runs(lang("sh"), "cat <<EOF\n", 0);
    assert_eq!(state & 0xff, S_HERE);
    let (r, state) = runs(lang("sh"), "it's\nEOF\nls\n", state);
    assert_eq!(r[0], (0, 9, Tok::Str));
    assert_eq!(state & 0xff, S_NONE);
}

#[test]
fn console_heredocs_end_at_a_prompted_terminator() {
    let t = "$ cat <<EOF\n> it's here\n> EOF\n$ echo 'x' y\n$ cat <<-E\n>\tz\n>\tE\n$ ls";
    assert_eq!(class_of("console", t, "it's here\n> EOF"), Tok::Str);
    assert_eq!(class_of("console", t, "z\n>\tE"), Tok::Str);
    // The prompts after it are prompts again, with commands.
    assert_eq!(class_of("console", t, "echo"), Tok::Constant);
    assert_eq!(class_of("console", t, "'x'"), Tok::Str);
    assert_ne!(class_of("console", t, "ls"), Tok::Str);
    // Without `> ` it ends as before.
    let t = "$ cat <<EOF\nhi\nEOF\n$ ls";
    assert_ne!(class_of("console", t, "ls"), Tok::Str);
    // And across a cut.
    let (_, state) = runs(lang("console"), "$ cat <<EOF\n", 0);
    let (r, state) = runs(lang("console"), "> a\n> EOF\n$ ls\n", state);
    assert_eq!(r[0], (0, 10, Tok::Str));
    assert_eq!(state & 0xff, S_NONE);
}

#[test]
fn spaced_heredocs() {
    let t = "cat << EOF\nIt's here\nEOF\ncat <<- 'E'\n\tdon't\n\tE\nls";
    assert_eq!(class_of("sh", t, "It's here\nEOF"), Tok::Str);
    assert_eq!(class_of("sh", t, "don't\n\tE"), Tok::Str);
    assert_ne!(class_of("sh", t, "ls"), Tok::Str);
    // Arithmetic, a number, a digit before, and a bare operator are not heredocs.
    for t in [
        "echo $((a << b))\nit's\nls it's",
        "(( x << y ))\nit's",
        "x=$(( 1 << 2 ))\nit's",
        "echo 1 << 3\nit's",
        "echo 2 << EOF\nit's",
        "a << \nit's",
        "a << (b)\nit's",
    ] {
        let (_, state) = runs(lang("sh"), t, 0);
        assert_ne!(state & 0xff, S_HERE, "{t:?}");
        assert_ne!(class_of("sh", t, "it"), Tok::Str, "{t:?}");
    }
    // One on the last line carries over a cut.
    let (_, state) = runs(lang("sh"), "cat << EOF\n", 0);
    assert_eq!(state & 0xff, S_HERE);
}

#[test]
fn a_regex_does_not_follow_a_brace() {
    let t = "<p>{count}/{total} done</p>\nx = /a/";
    assert_ne!(class_of("javascript", t, "/{total}"), Tok::Str);
    assert_ne!(class_of("javascript", t, "done"), Tok::Str);
    assert_eq!(class_of("javascript", t, "/a/"), Tok::Str);
}

#[test]
fn a_regex_that_is_too_long_is_division() {
    let long = format!("x = (/{}/g", "a".repeat(400));
    let (r, _) = runs(lang("javascript"), &long, 0);
    assert!(r.iter().all(|r| r.2 != Tok::Str));
    let ok = format!("x = (/{}/g", "a".repeat(200));
    let (r, _) = runs(lang("javascript"), &ok, 0);
    assert!(r.iter().any(|r| r.2 == Tok::Str));
}

#[test]
fn an_escaped_shell_quote_opens_no_string() {
    let t = "echo \\\"hello\\\"\necho done\nfor f in *.md; do cat \"$f\"; done";
    let (_, state) = runs(lang("sh"), t, 0);
    assert_eq!(state & 0xff, S_NONE);
    assert_eq!(class_of("sh", t, "done"), Tok::Keyword);
    assert_eq!(class_of("sh", t, "for"), Tok::Keyword);
    assert_eq!(class_of("sh", t, "\"$f\""), Tok::Str);
    // The same inside a console command, and with a multibyte escaped character.
    let c = "$ echo \\\"a\\\" b\n$ ls";
    assert_eq!(runs(lang("console"), c, 0).1 & 0xff, S_NONE);
    let (_, state) = runs(lang("sh"), "echo \\é \"open", 0);
    assert_ne!(state & 0xff, S_NONE);
}

#[test]
fn toml_basic_strings_have_escapes_and_literal_ones_do_not() {
    let t = "a = \"echo \\\"hello\\\"\" # c\nb = true";
    assert_eq!(class_of("toml", t, "\"echo \\\"hello\\\"\""), Tok::Str);
    assert_eq!(class_of("toml", t, "# c"), Tok::Comment);
    assert_eq!(class_of("toml", t, "true"), Tok::Constant);
    // A backslash ends nothing in a literal string.
    let t = "a = 'C:\\' # c\nb = 1";
    assert_eq!(class_of("toml", t, "'C:\\'"), Tok::Str);
    assert_eq!(class_of("toml", t, "# c"), Tok::Comment);
    // Multi-line strings still close at their own delimiter.
    let t = "a = \"\"\"x\"\"\" # c\nb = \'\'\'y\'\'\' # d";
    assert_eq!(class_of("toml", t, "# c"), Tok::Comment);
    assert_eq!(class_of("toml", t, "# d"), Tok::Comment);
    // A backslash at the end of the line closes nothing and swallows nothing after it.
    let (_, state) = runs(lang("toml"), "a = \"x\\\nb = 1", 0);
    assert_eq!(state & 0xff, S_NONE);
}

#[test]
fn a_comment_after_a_yaml_block_header_still_starts_the_block() {
    for h in ["|", "| # c", ">- # c", "|2 # c", "|+  # c", "> \r"] {
        let t = format!("run: {h}\n  # not a comment\n  a: b\nnext: 1");
        let (_, state) = runs(lang("yaml"), &format!("run: {h}\n"), 0);
        assert_eq!(state & 0xff, S_BLOCK, "{h:?}");
        assert_eq!(class_of("yaml", &t, "# not a comment"), Tok::Str, "{h:?}");
        assert_eq!(class_of("yaml", &t, "a: b"), Tok::Str, "{h:?}");
        assert_eq!(class_of("yaml", &t, "next"), Tok::Tag, "{h:?}");
    }
    let t = "run: | # c\n  x";
    assert_eq!(class_of("yaml", t, "| "), Tok::Keyword);
    assert_eq!(class_of("yaml", t, "# c"), Tok::Comment);
    // Not headers: a `#` without a blank before it, or other text.
    for h in ["|#c", "| x", "|2#c"] {
        let (_, state) = runs(lang("yaml"), &format!("run: {h}\n  y\n"), 0);
        assert_ne!(state & 0xff, S_BLOCK, "{h:?}");
    }
}

#[test]
fn a_yaml_expression_is_one_value() {
    let t = "os: ${{ matrix.os }}\nb: x-${{ a, b }}-y # c";
    assert_eq!(class_of("yaml", t, "${{ matrix.os }}"), Tok::Str);
    assert_eq!(class_of("yaml", t, "x-${{ a, b }}-y"), Tok::Str);
    assert_eq!(class_of("yaml", t, "# c"), Tok::Comment);
    // Unclosed, it is an ordinary value.
    assert_eq!(class_of("yaml", "a: ${{ b", "${{ b"), Tok::Str);
}
