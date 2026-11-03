use super::*;

/// The tree as S-expressions: `(frac (a) (b))`, characters bare.
fn dump(ast: &Ast) -> String {
    let mut out = String::new();
    list(ast, ast.root, &mut out);
    if let Some((tag, star)) = ast.tag {
        out.push_str(if star { " (tag* " } else { " (tag " });
        node(ast, tag, &mut out);
        out.push(')');
    }
    out
}

fn list(ast: &Ast, l: List, out: &mut String) {
    for (i, &id) in ast.list(l).iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        node(ast, id, out);
    }
}

fn opt(ast: &Ast, id: Option<NodeId>, out: &mut String) {
    match id {
        Some(id) => node(ast, id, out),
        None => out.push('_'),
    }
}

fn delim(d: Option<char>) -> String {
    d.map_or(".".into(), String::from)
}

fn node(ast: &Ast, id: NodeId, out: &mut String) {
    let Some(&n) = ast.node(id) else {
        out.push_str("<bad>");
        return;
    };
    let mut open = |s: &str| {
        out.push('(');
        out.push_str(s);
    };
    match n {
        Node::Char { ch, class } => {
            out.push(ch);
            if !matches!(class, Class::Ord) {
                out.push_str(&format!(":{class:?}").to_lowercase());
            }
            return;
        }
        Node::Group(l) => {
            open("");
            list(ast, l, out);
        }
        Node::Scripts {
            base,
            sup,
            sub,
            primes,
        } => {
            open("scripts ");
            opt(ast, base, out);
            out.push_str(" ^");
            opt(ast, sup, out);
            out.push_str(" _");
            opt(ast, sub, out);
            if primes > 0 {
                out.push_str(&format!(" '{primes}"));
            }
        }
        Node::Op { body, limits } => {
            open(&format!("op:{limits:?} ").to_lowercase());
            node(ast, body, out);
        }
        Node::Frac {
            num,
            den,
            bar,
            thick,
            style,
            left,
            right,
            cfrac,
        } => {
            open(if cfrac { "cfrac" } else { "frac" });
            if !bar {
                out.push_str(" nobar");
            }
            if let Some(t) = thick {
                out.push_str(&format!(" rule={t}"));
            }
            if let Some(s) = style {
                out.push_str(&format!(" {s:?}"));
            }
            if left.is_some() || right.is_some() {
                out.push_str(&format!(" {}{}", delim(left), delim(right)));
            }
            out.push(' ');
            node(ast, num, out);
            out.push(' ');
            node(ast, den, out);
        }
        Node::Sqrt { index, body } => {
            open("sqrt ");
            opt(ast, index, out);
            out.push(' ');
            node(ast, body, out);
        }
        Node::Delimited { left, right, body } => {
            open(&format!("left{} ", delim(left)));
            list(ast, body, out);
            out.push_str(&format!(" right{}", delim(right)));
        }
        Node::Middle(d) => open(&format!("middle{}", delim(d))),
        Node::Big {
            delim: d,
            size,
            class,
        } => open(&format!("big{size}:{class:?} {}", delim(d)).to_lowercase()),
        Node::Accent { ch, kind, body } => {
            open(&format!("accent:{kind:?} U+{:04X} ", ch as u32).to_lowercase());
            node(ast, body, out);
        }
        Node::Overline(b) => {
            open("overline ");
            node(ast, b, out);
        }
        Node::Underline(b) => {
            open("underline ");
            node(ast, b, out);
        }
        Node::Brace { body, under } => {
            open(if under { "underbrace " } else { "overbrace " });
            node(ast, body, out);
        }
        Node::Stack {
            body,
            over,
            under,
            class,
        } => {
            open(&format!("stack:{class:?} ").to_lowercase());
            node(ast, body, out);
            out.push_str(" ^");
            opt(ast, over, out);
            out.push_str(" _");
            opt(ast, under, out);
        }
        Node::XArrow { ch, over, under } => {
            open(&format!("xarrow {ch} ^"));
            opt(ast, over, out);
            out.push_str(" _");
            opt(ast, under, out);
        }
        Node::Font { font, body } => {
            open(&format!("{font:?} ").to_lowercase());
            node(ast, body, out);
        }
        Node::Text(l) => {
            open("text ");
            list(ast, l, out);
        }
        Node::TextRun { start, end, font } => {
            out.push_str(&format!("{:?}", ast.text_of(start, end)));
            if font != TextFont::Upright {
                out.push_str(&format!(":{font:?}").to_lowercase());
            }
            return;
        }
        Node::Space { amount, mu } => {
            open(&format!("space {amount}{}", if mu { "mu" } else { "em" }))
        }
        Node::Style(s) => open(&format!("style {s:?}").to_lowercase()),
        Node::Class { class, body } => {
            open(&format!("class:{class:?} ").to_lowercase());
            node(ast, body, out);
        }
        Node::Phantom {
            body,
            width,
            height,
        } => {
            open(&format!(
                "phantom{}{} ",
                if width { "w" } else { "" },
                if height { "h" } else { "" }
            ));
            node(ast, body, out);
        }
        Node::Smash { body, top, bottom } => {
            open(&format!(
                "smash{}{} ",
                if top { "t" } else { "" },
                if bottom { "b" } else { "" }
            ));
            node(ast, body, out);
        }
        Node::Boxed(b) => {
            open("boxed ");
            node(ast, b, out);
        }
        Node::Rule {
            width,
            height,
            raise,
        } => open(&format!("rule {width} {height} {raise}")),
        Node::Not(b) => {
            open("not ");
            node(ast, b, out);
        }
        Node::FontSwitch(f) => open(&format!("switch {f:?}").to_lowercase()),
        Node::Lap { body, side } => {
            open(&format!("lap {side:?} ").to_lowercase());
            node(ast, body, out);
        }
        Node::Newline => open("newline"),
        Node::Array {
            env,
            cols,
            cells,
            pairs,
        } => {
            open(&format!("{env:?}").to_lowercase());
            let spec = ast
                .cols
                .get(cols.start as usize..cols.end as usize)
                .unwrap_or(&[]);
            if !spec.is_empty() {
                out.push_str(&format!(" {spec:?}"));
            }
            if pairs > 0 {
                out.push_str(&format!(" pairs={pairs}"));
            }
            out.push(' ');
            list(ast, cells, out);
        }
        Node::Cell(l) => {
            open("cell ");
            list(ast, l, out);
        }
        Node::RowEnd { gap } => open(&format!("row {gap}")),
        Node::HLine { dashed } => open(if dashed { "hdashline" } else { "hline" }),
    }
    out.push(')');
}

fn dumps(cases: &[&str], display: bool) -> String {
    let mut out = String::new();
    for src in cases {
        out.push_str(&format!("== {src}\n"));
        match parse(src, display) {
            Ok(ast) => out.push_str(&dump(&ast)),
            Err(e) => out.push_str(&format!(
                "error: {} at {:?} {:?}",
                e.message,
                e.span,
                src.get(e.span.clone()).unwrap_or("?")
            )),
        }
        out.push('\n');
    }
    out
}

#[test]
fn atoms_and_scripts() {
    insta::assert_snapshot!(dumps(
        &[
            "x+1=y",
            "a-b",
            r"\alpha\leq\beta, x\in A",
            "x^2_i",
            "x_i^2",
            "x^{y^z}",
            "^2 x",
            "{}_1^2",
            "f'(x) + f''",
            "x'^2_i",
            r"\sum_{i=0}^n \sum\limits_i \int\nolimits_0 \prod",
            r"\sin x \lim_{n\to\infty} \operatorname{tr} A \operatorname*{argmax}_x",
            r"\mathop{x}_1 \mathbin{\star} \mathrel{R}",
            r"\not= \not\in",
            "x % comment\n + y",
            "中 → ≤",
        ],
        false
    ));
}

#[test]
fn fractions_roots_delimiters() {
    insta::assert_snapshot!(dumps(
        &[
            r"\frac{a}{b} \frac12 \dfrac ab \tfrac{1}{2}",
            r"\cfrac[l]{1}{2+\cfrac{1}{3}}",
            r"\binom{n}{k} \dbinom nk \tbinom{a}{b}",
            r"a \over b",
            r"{n \choose k} {a \atop b} {a \brace b} {a \brack b}",
            r"\sqrt{2} \sqrt[3]{x} \sqrt[n+1]{\frac{a}{b}}",
            r"\left( x \right) \left. x \right| \left\{ x \middle| y \right\}",
            r"\left\langle x \right\rangle \left\lfloor x \right\rceil \left< x \right> \left\| x \right\Vert",
            r"\big( \Big[ \bigg\{ \Bigg\langle \bigl( \bigr) \bigm| \Biggr\rbrace",
            r"\left/ x \right\backslash \left\uparrow x \right\Downarrow",
        ],
        false
    ));
}

#[test]
fn accents_text_fonts_spaces() {
    insta::assert_snapshot!(dumps(
        &[
            r"\hat a \vec{v} \widehat{xyz} \overrightarrow{AB} \underleftarrow{x}",
            r"\overline{x} \underline{y} \overbrace{a+b}^{2} \underbrace{c}_{d}",
            r"\overset{!}{=} \underset{x}{\lim} \stackrel{def}{=} \overset{a}{b}",
            r"\xrightarrow{f} \xleftarrow[g]{h} \xmapsto{}",
            r"\text{if  $x>0$, {and} \textbf{bold}~\$5 \% \{\} \ldots}",
            r"\textit{it} \textsf{sf} \texttt{tt} \mbox{m} \text a",
            r"\mathrm{d}x \mathbf{v} \mathbb{R} \mathcal{L} \mathscr{H} \mathfrak{g} \mathsf{s} \mathtt{t} \boldsymbol\alpha \bm{x}",
            r"\displaystyle x \textstyle y \scriptstyle z \scriptscriptstyle w",
            r"a\,b\:c\;d\!e\ f~g\quad h\qquad i\enspace j",
            r"\hspace{1em} \hspace*{2pt} \kern-1.5em \mkern18mu \mskip 9mu \kern 1in \hspace{1,5cm}",
            r"\phantom{x} \hphantom{y} \vphantom{z} \smash{a} \smash[t]{b} \smash[b]{c}",
            r"\boxed{x} \rule{1em}{2em} \rule[-1ex]{2pt}{1pt}",
            r"\color{red} x \textcolor{blue}{y} \label{eq} \nonumber \notag",
            r"\kern1000em \rule{9999em}{-9999em}",
        ],
        false
    ));
}

#[test]
fn environments() {
    insta::assert_snapshot!(dumps(
        &[
            r"\begin{matrix} a & b \\ c & d \end{matrix}",
            r"\begin{pmatrix} 1 \end{pmatrix} \begin{bmatrix}1\end{bmatrix} \begin{Bmatrix}1\end{Bmatrix}",
            r"\begin{vmatrix}1\end{vmatrix} \begin{Vmatrix}1\end{Vmatrix} \begin{smallmatrix}a&b\end{smallmatrix}",
            r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}",
            r"\begin{dcases}a\end{dcases} \begin{rcases}a\end{rcases}",
            r"\begin{aligned} a &= b \\[2pt] &= c \end{aligned}",
            r"\begin{alignedat}{2} a &= b & c &= d \end{alignedat}",
            r"\begin{gathered} a \\ b \end{gathered} \begin{split} a &= b \end{split}",
            r"\begin{array}{l|c:r} \hline a & b & c \\ \hdashline d & e & f \\ \hline \end{array}",
            r"\sum_{\substack{0<i<m \\ 0<j<n}} P(i,j) \begin{subarray}{l} a \\ b \end{subarray}",
            r"\begin{align} a &= b \\ c &= d \end{align} \begin{align*} x \end{align*}",
            r"\begin{gather} a \end{gather} \begin{equation*} E = mc^2 \end{equation*}",
            r"a \\ b \\[1em] c \tag{1.2}",
            r"x \tag*{A}",
        ],
        true
    ));
}

#[test]
fn more_commands_and_environments() {
    insta::assert_snapshot!(dumps(
        &[
            r"a \bmod b",
            r"a \equiv b \pmod{n}",
            r"a \mod n",
            r"{\rm d}x {\bf x}\it y",
            r"\tiny x \Huge y",
            r"\mathllap{a} \mathrlap{b} \clap{c}",
            r"\fbox{a} \colorbox{red}{b} \fcolorbox{a}{b}{c} \strut \mathstrut",
            r"\Bbb R \bold{x} \pmb{y}",
            r"\genfrac(){0pt}{}{a}{b} \genfrac{[}{]}{1pt}{0}{a}{b} {a \above 1pt b}",
            r"\llbracket \rrbracket \iddots \lang \rang",
            r"\varliminf \varlimsup \xlongequal{a} \utilde{x}",
            r"\eqref{e} \ref{r} \argmax \injlim \sh",
            r"\begin{matrix*}[r] a \end{matrix*} \begin{pmatrix*}b\end{pmatrix*}",
            r"\begin{multline} a \\ b \end{multline}",
            r"\begin{eqnarray} a & = & b \end{eqnarray}",
        ],
        true
    ));
}

#[test]
fn old_switches_cover_both_halves_of_an_infix_fraction() {
    insta::assert_snapshot!(dumps(
        &[
            r"{\rm a \over b}",
            r"{x \bf a \choose b} c",
            r"{a \over b \it c}",
        ],
        false
    ));
}

#[test]
fn boxes_and_laps_read_text() {
    insta::assert_snapshot!(dumps(
        &[
            r"\fbox{Note $x$} \colorbox{yellow}{a b} \fcolorbox{r}{g}{hi}",
            r"\llap{a b} \rlap{c} \clap{d} \mathllap{e f}",
        ],
        false
    ));
}

#[test]
fn old_switches_work_inside_text() {
    insta::assert_snapshot!(dumps(
        &[
            r"\text{a \rm b \bf c \it d \sf e \tt f}",
            r"\text{\bf Note: $x$ {\it y} z}",
            r"\fbox{\bf hi}",
            r"x \tag{\rm (a)}",
            r"\text{\cal a}",
        ],
        true
    ));
}

#[test]
fn unsupported_commands_are_errors() {
    insta::assert_snapshot!(dumps(
        &[
            r"\href{x}{y}",
            r"\ce{H2O}",
            r"\verb|x|",
            r"\cancel{x}",
            r"\newcommand{\x}{y}",
            r"\begin{CD} A \end{CD}",
            r"\genfrac{x}{)}{}{}{a}{b}",
            r"\begin{matrix*}[x] a \end{matrix*}",
            r"\begin{multline} a \end{multline}",
        ],
        false
    ));
}

#[test]
fn errors_say_where() {
    insta::assert_snapshot!(dumps(
        &[
            r"\foo",
            r"x \frac{a}",
            "{x",
            "x}",
            "x^",
            "x^1^2",
            "x_1_2",
            "x'^1^2",
            r"\left( x",
            r"x \right)",
            r"\middle|",
            r"\left\alpha x \right)",
            r"\begin{foo} \end{foo}",
            r"\begin{matrix} a \end{pmatrix}",
            r"\begin{matrix} a",
            r"\end{matrix}",
            "a & b",
            r"{a \\ b}",
            r"a \over b \over c",
            r"\limits",
            r"x\limits",
            r"\sqrt[3",
            r"\text{abc",
            r"\text{\alpha}",
            r"\kern 3",
            r"\kern 3mu",
            r"\mkern 3em",
            r"\hspace{x}",
            r"\begin{array}{lq} a \end{array}",
            r"\begin{align} a \end{align}",
            r"\tag{1}",
            r"\newcommand{\x}{y}",
            r"\def\x{y}",
            r"\cancel{x}",
            "\\",
            r"x \smash[q]{y}",
        ],
        false
    ));
}

/// Display-only constructs and a second `\tag`.
#[test]
fn display_errors() {
    insta::assert_snapshot!(dumps(&[r"a \tag{1} \tag{2}", r"\text{$x$ \\ }"], true));
}

fn nested(open: &str, close: &str, n: usize) -> String {
    format!("{}x{}", open.repeat(n), close.repeat(n))
}

#[test]
fn depth_limit() {
    // A group, an argument, a `\left`: a level each.
    assert!(parse(&nested("{", "}", MAX_DEPTH), false).is_ok());
    let e = parse(&nested("{", "}", MAX_DEPTH + 1), false).unwrap_err();
    assert_eq!(e.message, "formula nests too deep");
    assert_eq!(e.span, MAX_DEPTH..MAX_DEPTH + 1, "at the brace too many");
    assert!(parse(&nested(r"\left(", r"\right)", MAX_DEPTH), false).is_ok());
    assert!(parse(&nested(r"\left(", r"\right)", MAX_DEPTH + 1), false).is_err());
    // `\sqrt{…}`: the argument is one level, its group no other.
    assert!(parse(&nested(r"\sqrt{", "}", MAX_DEPTH), false).is_ok());
    assert!(parse(&nested(r"\sqrt{", "}", MAX_DEPTH + 1), false).is_err());
    assert!(parse(&nested("x^{", "}", MAX_DEPTH), false).is_ok());
    assert!(parse(&nested("x^{", "}", MAX_DEPTH + 1), false).is_err());
    // Text braces count with the formula's.
    let text = format!(r"\text{{{}}}", nested("{", "}", MAX_DEPTH - 1));
    assert!(parse(&text, false).is_ok());
    let text = format!(r"\text{{{}}}", nested("{", "}", MAX_DEPTH));
    assert!(parse(&text, false).is_err());
    // Deep enough to overflow a small stack if the limit didn't hold.
    assert!(parse(&nested("{", "}", 4000), false).is_err());
    assert!(parse(&"x^".repeat(4000), false).is_err());
}

#[test]
fn not_counts_against_the_depth_limit() {
    // `\not` takes the next atom, so it nests without braces.
    let chain = |n: usize| format!("{}=", r"\not".repeat(n));
    assert!(parse(&chain(MAX_DEPTH), false).is_ok());
    let e = parse(&chain(MAX_DEPTH + 1), false).unwrap_err();
    assert_eq!(e.message, "formula nests too deep");
    // The same at 4 x the limit, in the middle of other levels, and as the largest source
    // there is, on a stack of 256 KiB.
    assert!(parse(&format!("{{{}}}", chain(MAX_DEPTH - 1)), false).is_ok());
    assert!(parse(&format!("{{{}}}", chain(MAX_DEPTH)), false).is_err());
    assert!(parse(&format!(r"x^{{{}}}", chain(MAX_DEPTH - 1)), false).is_ok());
    assert!(parse(&format!(r"x^{{{}}}", chain(MAX_DEPTH)), false).is_err());
    let huge = chain((MAX_SOURCE - 1) / r"\not".len());
    assert!(huge.len() <= MAX_SOURCE);
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            assert!(parse(&huge, true).is_err());
            for src in [
                r"\not".repeat(2000),
                r"\frac".repeat(1300),
                r"\sqrt[".repeat(1300),
                r"\left(".repeat(1300),
                r"\text{$".repeat(1000),
                r"\begin{matrix}".repeat(500),
                r"x^".repeat(2000),
                r"\mathrm".repeat(1000),
                r"\not\mathrm\sqrt".repeat(400),
            ] {
                assert!(parse(&src, true).is_err(), "{src}");
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn limits_follow_an_operator_and_its_scripts() {
    let op = |src: &str| dump(&parse(src, false).unwrap());
    assert!(op(r"\sum_{i=1}^n\limits").starts_with("(scripts (op:always"));
    assert!(op(r"\int_0^1\nolimits").starts_with("(scripts (op:never"));
    assert!(op(r"\sum'\nolimits").starts_with("(scripts (op:never"));
    assert!(op(r"\sum\limits_1^2").starts_with("(scripts (op:always"));
    for src in [
        r"x_1\limits",
        r"x^2\nolimits",
        r"{\sum}_1\limits",
        r"x'\limits",
    ] {
        let e = parse(src, false).unwrap_err();
        assert_eq!(e.message, "\\limits must follow an operator", "{src}");
    }
}

#[test]
fn fraction_rules_keep_their_thickness() {
    let rule = |src: &str| dump(&parse(src, false).unwrap());
    assert_eq!(rule(r"a \above 1pt b"), "(frac rule=0.1 (a) (b))");
    assert_eq!(rule(r"a \above 0pt b"), "(frac nobar (a) (b))");
    assert_eq!(
        rule(r"\genfrac{}{}{2pt}{}{a}{b}"),
        "(frac rule=0.2 (a) (b))"
    );
    assert_eq!(rule(r"\genfrac{}{}{}{}{a}{b}"), "(frac (a) (b))");
    assert_eq!(rule(r"\genfrac{}{}{0pt}{}{a}{b}"), "(frac nobar (a) (b))");
    // Clamped to MAX_RULE em; a negative thickness is no rule.
    assert_eq!(
        rule(r"\genfrac{}{}{1000pt}{}{a}{b}"),
        "(frac rule=2 (a) (b))"
    );
    assert_eq!(rule(r"\genfrac{}{}{-1pt}{}{a}{b}"), "(frac nobar (a) (b))");
    assert!(parse(r"\genfrac{}{}{x}{}{a}{b}", false).is_err());
    assert!(parse(r"\genfrac{}{}{1pt{}{a}{b}", false).is_err());
}

#[test]
fn node_limit() {
    // One node a character.
    let just = "x".repeat(MAX_NODES);
    assert_eq!(parse(&just, false).map(|a| a.nodes.len()), Ok(MAX_NODES));
    // (Past the source limit: no 8 KiB source makes more nodes than it has bytes.)
    let over = "x".repeat(MAX_NODES + 1);
    let e = parse_any(&over, false).unwrap_err();
    assert_eq!(
        (e.message, e.span),
        ("formula too complex", MAX_NODES..MAX_NODES + 1)
    );
}

#[test]
fn source_limit() {
    assert!(parse(&"x ".repeat(MAX_SOURCE / 2), false).is_ok());
    let e = parse(&"x ".repeat(MAX_SOURCE / 2 + 1), false).unwrap_err();
    assert_eq!(e.message, "formula too long");
}

#[test]
fn cell_and_column_limits() {
    let row = |n: usize| format!(r"\begin{{matrix}}{}\end{{matrix}}", "&".repeat(n - 1));
    assert!(parse(&row(MAX_COLS), false).is_ok());
    assert_eq!(
        parse(&row(MAX_COLS + 1), false).unwrap_err().message,
        "too many cells"
    );
    // Full rows, then one cell more.
    let rows = MAX_CELLS / MAX_COLS;
    let full = vec!["&".repeat(MAX_COLS - 1); rows].join(r"\\");
    let cells = |extra: &str| format!(r"\begin{{matrix}}{full}{extra}\end{{matrix}}");
    assert!(parse(&cells(""), false).is_ok());
    assert_eq!(
        parse(&cells(r"\\x"), false).unwrap_err().message,
        "too many cells"
    );
    let spec = |n: usize| format!(r"\begin{{array}}{{{}}}x\end{{array}}", "c".repeat(n));
    assert!(parse(&spec(MAX_COLS), false).is_ok());
    assert_eq!(
        parse(&spec(MAX_COLS + 1), false).unwrap_err().message,
        "too many columns"
    );
}

#[test]
fn dimensions_are_clamped() {
    let space = |src: &str| match parse(src, false)
        .ok()
        .and_then(|a| a.nodes.first().copied())
    {
        Some(Node::Space { amount, mu }) => (amount, mu),
        other => panic!("{src}: {other:?}"),
    };
    assert_eq!(space(r"\kern 1em"), (1.0, false));
    assert_eq!(space(r"\kern -10pt"), (-1.0, false));
    assert_eq!(space(r"\kern 99999999999999999999em"), (MAX_DIMEN, false));
    assert_eq!(space(r"\kern --+-2em"), (-2.0, false));
    assert_eq!(space(r"\mkern-99999mu"), (-MAX_DIMEN * 18.0, true));
    assert!(parse(r"\kern 1e", false).is_err());
}

/// A dimension cut off by the end of the formula: the error is inside the source
/// (fuzz/seeds/math/kern-at-the-end.bin).
#[test]
fn a_dimension_at_the_end() {
    for src in [
        r"\kern ",
        r"\kern",
        r"\kern -",
        r"\hspace{",
        r"\rule[",
        r"\\[",
    ] {
        let e = parse(src, true).unwrap_err();
        assert!(
            e.span.start <= e.span.end && e.span.end <= src.len(),
            "{src}: {e:?}"
        );
    }
}
