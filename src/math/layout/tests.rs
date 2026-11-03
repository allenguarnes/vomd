use super::*;
use crate::fonts::Fonts;
use crate::layout::BlockBox;
use crate::math::emit::{Emitted, emit};
use crate::render::{Frame, paint_block};
use crate::text::cache::GlyphCache;
use crate::theme::{LIGHT, Role, Theme};

/// Body text size in the tests' images, in logical pixels, and the math size over it.
const BODY: f32 = 16.0;
const MATH_SCALE: f32 = 1.15;

fn fonts() -> (Fonts, FaceId) {
    let mut fonts = Fonts::new().expect("fonts");
    let math = fonts.load_math().expect("math face");
    (fonts, math)
}

fn lay_with(fonts: &Fonts, math: FaceId, src: &str, display: bool) -> Formula {
    Engine::new()
        .layout(fonts, math, src, display)
        .unwrap_or_else(|e| panic!("{src}: {} at {:?}", e.message, e.span))
}

/// A formula painted by VOMD's own painter and glyph cache, on `theme`'s background, `scale`
/// times the body size: its pixels (`0xRRGGBB`), width and height.
pub(crate) fn render(
    fonts: &Fonts,
    f: &Formula,
    scale: f32,
    theme: &Theme,
) -> (Vec<u32>, usize, usize) {
    let em = BODY * MATH_SCALE * scale;
    let pad = (8.0 * scale).ceil();
    // A tag goes 2 em to the right, as a block would set it flush right.
    let tag_w = f.tag.as_ref().map_or(0.0, |t| 2.0 + t.width);
    let w = ((f.width.max(0.0) + tag_w) * em + 2.0 * pad).ceil() as usize;
    let h = ((f.height + f.depth) * em + 2.0 * pad).ceil() as usize;
    let mut out = Emitted::default();
    emit(f, em, pad, pad + f.height * em, Role::Text, &mut out);
    if let Some(t) = &f.tag {
        let x = pad + (f.width + 2.0) * em;
        emit(t, em, x, pad + f.height * em, Role::Text, &mut out);
    }
    let block = BlockBox {
        glyphs: out.glyphs,
        styles: out.styles,
        ink: out.ink,
        cuts: out.cuts,
        ..BlockBox::default()
    };
    let mut buf = vec![theme.bg; w * h];
    let mut frame = Frame::new(&mut buf, w, h, w);
    let mut cache = GlyphCache::default();
    paint_block(
        &mut frame,
        &block,
        0.0,
        0.0,
        fonts,
        &mut cache,
        theme,
        #[cfg(feature = "clipboard")]
        None,
    );
    (buf, w, h)
}

/// Writes `render`'s pixels as an RGB PNG.
pub(crate) fn write_png(path: &std::path::Path, (buf, w, h): (Vec<u32>, usize, usize)) {
    let rgb: Vec<u8> = buf
        .iter()
        .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
        .collect();
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("png file"));
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(&rgb).expect("png data");
}

/// One of each construct, for a look (`display`, name, source).
const GALLERY: &[(bool, &str, &str)] = &[
    (
        false,
        "inline-mix",
        r"E = mc^2, \quad a_i + b^{j} - c_{k}^{2}",
    ),
    (
        true,
        "scripts-nested",
        r"x^{y^{z}} + x_{i_{j}} + x_i^2 + {}^{14}_{6}C + f'(x) + f''",
    ),
    (
        true,
        "spacing-classes",
        r"a+b=c, \; -x \le y \times z, \quad f(x)\colon A \to B",
    ),
    (
        true,
        "frac",
        r"\frac{a+b}{c-d} + \frac{1}{2} + \tfrac{1}{2} + \dfrac{x}{y}",
    ),
    (
        false,
        "frac-inline",
        r"\frac{a}{b} \text{ and } \dfrac{a}{b} \text{ inline}",
    ),
    (true, "frac-nested", r"\frac{1}{1+\frac{1}{1+\frac{1}{x}}}"),
    (
        true,
        "cfrac",
        r"x = a_0 + \cfrac{1}{a_1 + \cfrac{1}{a_2 + \cfrac{1}{a_3}}}",
    ),
    (
        true,
        "binom",
        r"\binom{n}{k} = \frac{n!}{k!\,(n-k)!}, \quad {n \choose 2}, \tbinom{a}{b}",
    ),
    (
        true,
        "sqrt",
        r"\sqrt{2} + \sqrt{x^2+y^2} + \sqrt[3]{\frac{a}{b}} + \sqrt[n]{1+x}",
    ),
    (true, "sqrt-tall", r"\sqrt{\frac{\frac{a}{b}}{\frac{c}{d}}}"),
    (
        true,
        "sum-limits",
        r"\sum_{i=1}^{n} i^2 = \frac{n(n+1)(2n+1)}{6}",
    ),
    (
        false,
        "sum-inline",
        r"\sum_{i=1}^{n} x_i \text{ and } \prod_{k} a_k \text{ inline}",
    ),
    (
        true,
        "integrals",
        r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt\pi}{2}, \quad \oint_C f, \iint_D g",
    ),
    (
        true,
        "bigops",
        r"\bigcup_{i\in I} A_i \cap \bigoplus_{k=1}^{m} V_k \quad \coprod \bigwedge \bigvee",
    ),
    (
        true,
        "limits-names",
        r"\lim_{x\to 0} \frac{\sin x}{x} = 1, \quad \max_{i} a_i, \quad \log n",
    ),
    (
        true,
        "limits-override",
        r"\sum\nolimits_{i} a_i + \int\limits_{0}^{1} f + \operatorname*{arg\,max}_{x} g",
    ),
    (
        true,
        "left-right",
        r"\left( \frac{a}{b} \right) \left[ \sum_{i} x_i \right] \left\{ x \middle| x > 0 \right\}",
    ),
    (
        true,
        "left-right-tall",
        r"\left( \frac{\frac{\frac{1}{2}}{3}}{4} \right) \left\langle \frac{a}{b} \right\rangle \left. \frac{df}{dx} \right|_{x=0}",
    ),
    (
        true,
        "delims-assembly",
        r"\left( \vphantom{\rule{1pt}{6em}} x \right) \left\lbrace \vphantom{\rule{1pt}{8em}} y \right. \left\| \vphantom{\rule{1pt}{5em}} z \right\| \left| \vphantom{\rule{1pt}{4em}} w \right| \left[ \vphantom{\rule{1pt}{7em}} v \right\rceil \left\langle \vphantom{\rule{1pt}{3em}} u \right\rbrace",
    ),
    (
        true,
        "big-sizes",
        r"\big( \Big( \bigg( \Bigg( x \Bigg) \bigg) \Big) \big) \quad \bigl[ a \bigr] \bigm| \Bigl\langle b \Bigr\rangle",
    ),
    (
        true,
        "delim-kinds",
        r"\left\lfloor x \right\rfloor \left\lceil y \right\rceil \left| z \right| \left\Vert w \right\Vert \left/ q \right\backslash",
    ),
    (
        true,
        "accents",
        r"\hat{a}\ \check{c}\ \tilde{n}\ \acute{e}\ \grave{o}\ \dot{x}\ \ddot{y}\ \breve{u}\ \bar{z}\ \vec{v}\ \mathring{A}",
    ),
    (
        true,
        "wide-accents",
        r"\widehat{xyz}\ \widetilde{abc}\ \overline{x+y}\ \underline{a+b}\ \overrightarrow{AB}\ \overleftarrow{CD}\ \overleftrightarrow{EF}",
    ),
    (
        true,
        "braces",
        r"\overbrace{a+b+\cdots+z}^{26} \quad \underbrace{1+1+\cdots+1}_{n\text{ times}}",
    ),
    (
        true,
        "stack-arrows",
        r"A \xrightarrow{f} B \xleftarrow[g]{\text{long label}} C, \quad \overset{!}{=} \quad \underset{x}{\arg} \quad \stackrel{def}{=}",
    ),
    (
        true,
        "text",
        r"\text{if } x > 0 \text{ and } \textbf{bold} \textit{ italic } \textsf{sans} \texttt{ mono} \text{ — café}",
    ),
    (
        true,
        "text-fallback",
        r"\text{Привет, 世界} = \text{a $x^2$ b}",
    ),
    (
        true,
        "fonts",
        r"\mathrm{d}x\ \mathbf{v}\ \mathit{ff}\ \mathbb{R}\ \mathbb{N}\ \mathcal{L}\ \mathscr{H}\ \mathfrak{g}\ \mathsf{sans}\ \mathtt{mono}\ \boldsymbol{\alpha\beta}",
    ),
    (
        true,
        "greek",
        r"\alpha\beta\gamma\delta\epsilon\varepsilon\zeta\eta\theta\vartheta\lambda\mu\pi\varphi\omega \; \Gamma\Delta\Theta\Lambda\Sigma\Phi\Psi\Omega \; \varGamma\varOmega",
    ),
    (
        true,
        "symbols",
        r"\forall x \in A, \exists y \notin B: x \neq y \Rightarrow \nabla \cdot \vec{E} = \frac{\rho}{\varepsilon_0}, \infty, \partial, \hbar, \emptyset",
    ),
    (
        true,
        "relations-arrows",
        r"a \leq b \geq c \approx d \equiv e \sim f \subset g \supseteq h \to i \mapsto j \Leftrightarrow k \hookrightarrow l",
    ),
    (true, "not", r"a \not= b, \not\in, \not\subset, \not\equiv"),
    (
        true,
        "dots",
        r"1, 2, \ldots, n \quad a_1 + \cdots + a_n \quad \vdots \quad \ddots",
    ),
    (
        true,
        "phantoms",
        r"\boxed{x}\ \boxed{\frac{a}{b}}\ a\phantom{xyz}b\ \sqrt{\vphantom{\frac12}x}\ \sqrt{\smash{\frac12}}\ a\hphantom{\frac12}b",
    ),
    (
        true,
        "spaces",
        r"a\,b\:c\;d\!e\ f\quad g\qquad h\hspace{2em}i\kern1em j\mkern18mu k\enspace l",
    ),
    (
        true,
        "rule-style",
        r"\rule{2em}{0.5em} \rule[-0.5em]{0.2em}{1.5em} \quad \displaystyle\sum_i \textstyle\sum_i \scriptstyle\sum_i \scriptscriptstyle\sum_i",
    ),
    (
        true,
        "classes",
        r"a\mathbin{\star}b \mathrel{R} c \mathop{\mathrm{Tr}}_{x} d \mathpunct{.} e",
    ),
    (
        true,
        "physics",
        r"i\hbar\frac{\partial}{\partial t}\Psi(\mathbf{r},t) = \left[-\frac{\hbar^2}{2m}\nabla^2 + V(\mathbf{r},t)\right]\Psi(\mathbf{r},t)",
    ),
    (
        true,
        "gauss",
        r"\oint_{\partial\Omega} \mathbf{E}\cdot d\mathbf{A} = \frac{1}{\varepsilon_0}\iiint_\Omega \rho\, dV",
    ),
    (
        true,
        "euler",
        r"e^{i\pi} + 1 = 0, \qquad \zeta(s) = \sum_{n=1}^{\infty} \frac{1}{n^s} = \prod_{p} \frac{1}{1-p^{-s}}",
    ),
];

/// Environments, `\tag` and rows of the top level (display unless noted).
const GALLERY_C: &[(bool, &str, &str)] = &[
    (
        true,
        "matrices",
        r"\begin{matrix} a & b \\ c & d \end{matrix} \begin{pmatrix} 1 & 0 \\ 0 & 1 \end{pmatrix} \begin{bmatrix} x_{11} & x_{12} \\ x_{21} & x_{22} \end{bmatrix} \begin{vmatrix} a & b \\ c & d \end{vmatrix} \begin{Vmatrix} u \\ v \end{Vmatrix} \begin{Bmatrix} 1 \end{Bmatrix}",
    ),
    (
        false,
        "smallmatrix",
        r"\left(\begin{smallmatrix} a & b \\ c & d \end{smallmatrix}\right) \text{ inline}",
    ),
    (
        true,
        "big-matrix",
        r"A = \begin{pmatrix} a_{11} & a_{12} & \cdots & a_{1n} \\ a_{21} & a_{22} & \cdots & a_{2n} \\ \vdots & \vdots & \ddots & \vdots \\ a_{m1} & a_{m2} & \cdots & a_{mn} \end{pmatrix}",
    ),
    (
        true,
        "cases",
        r"|x| = \begin{cases} x & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases} \quad f(n) = \begin{dcases} \frac{n}{2} & n \text{ even} \\ 3n+1 & n \text{ odd} \end{dcases} \quad \begin{rcases} a \\ b \end{rcases} = c",
    ),
    (
        true,
        "aligned",
        r"\begin{aligned} f(x) &= (x+1)^2 \\ &= x^2 + 2x + 1 \\ g &= \int_0^1 f(t)\,dt & h &= 2 \end{aligned}",
    ),
    (
        true,
        "gathered-split",
        r"\begin{gathered} a = b \\ c + d = e + f \end{gathered} \qquad \begin{split} x &= y \\ &\quad + z \end{split} \qquad \begin{alignedat}{2} a &= b &\quad c &= d \end{alignedat}",
    ),
    (
        true,
        "array",
        r"\left[\begin{array}{c|cc:r} 1 & 2 & 3 & 4 \\ \hline 5 & 6 & 7 & 8 \\ \hdashline 9 & 10 & 11 & 12 \end{array}\right]",
    ),
    (
        true,
        "substack",
        r"\sum_{\substack{0 < i < m \\ 0 < j < n}} P(i, j) \qquad \sum_{\begin{subarray}{l} i \in I \\ j \in J \end{subarray}} x_{ij}",
    ),
    (true, "tag", r"E = mc^2 \tag{1.1}"),
    (true, "tag-star", r"a^2 + b^2 = c^2 \tag*{(Pythagoras)}"),
    (true, "newlines", r"a + b = c \\ d = e + f + g \\ x"),
    (
        true,
        "align",
        r"\begin{align} a &= b + c \\ d &= e \end{align}",
    ),
];

/// Every construct lays out, and paints. With `VOMD_MATH_PNG=<dir>`, each is written there
/// as a PNG (light theme, 2×) to look at.
#[test]
fn gallery() {
    let (mut fonts, math) = fonts();
    let dir = std::env::var_os("VOMD_MATH_PNG").map(std::path::PathBuf::from);
    if let Some(dir) = &dir {
        std::fs::create_dir_all(dir).expect("png dir");
    }
    for &(display, name, src) in GALLERY.iter().chain(GALLERY_C) {
        let mut f = lay_with(&fonts, math, src, display);
        if dir.is_some() && !f.missing.is_empty() {
            // As the view does: add fallback faces for what is missing, and lay out again.
            let skip = |p: &std::path::Path, i: u32| fonts.has_source(p, i);
            let found = crate::text::fallback::find_fallbacks(&f.missing, &skip);
            for found in found {
                fonts.add_fallback(&found.path, found.index);
            }
            f = lay_with(&fonts, math, src, display);
        }
        assert!(f.width > 0.0 && !f.glyphs.is_empty(), "{name}");
        let pixels = render(&fonts, &f, 2.0, &LIGHT);
        assert!(pixels.0.iter().any(|&p| p != LIGHT.bg), "{name}: no ink");
        if let Some(dir) = &dir {
            write_png(&dir.join(format!("{name}.png")), pixels);
        }
    }
}

const EPS: f32 = 1e-4;

#[test]
fn a_fraction_is_taller_and_deeper_than_its_parts() {
    let (fonts, math) = fonts();
    let a = lay_with(&fonts, math, "a", true);
    let f = lay_with(&fonts, math, r"\frac{a}{a}", true);
    assert!(
        f.height > a.height + 0.3 && f.depth > a.depth + 0.3,
        "{f:?}"
    );
    assert_eq!(f.rules.len(), 1, "the bar");
    let bar = f.rules[0];
    // The bar is on the axis, between the numerator and the denominator.
    let axis = 258.0 / 1000.0;
    assert!((bar.y + bar.h / 2.0 - axis).abs() < EPS, "{bar:?}");
    assert!(f.glyphs[0].y > bar.y + bar.h && f.glyphs[1].y < bar.y);
    // Text style sets the parts smaller and closer.
    let t = lay_with(&fonts, math, r"\frac{a}{a}", false);
    assert!(t.height < f.height && t.glyphs[0].size < 1.0);
    // No bar in a binomial, and delimiters around it.
    let b = lay_with(&fonts, math, r"\binom{a}{b}", true);
    assert!(b.rules.is_empty() && b.glyphs.len() == 4);
}

#[test]
fn an_explicit_fraction_rule_sets_its_thickness_and_clearances() {
    let (fonts, math) = fonts();
    let at = |src: &str| lay_with(&fonts, math, src, true);
    let thin = at(r"\genfrac{}{}{1pt}{}{a}{b}");
    let thick = at(r"\genfrac{}{}{10pt}{}{a}{b}");
    assert!((thick.rules[0].h / thin.rules[0].h - 10.0).abs() < 0.001);
    assert!((at(r"{a \above 10pt b}").rules[0].h - thick.rules[0].h).abs() < EPS);
    // The parts keep clear of a thick rule.
    assert!(thick.height > thin.height && thick.depth > thin.depth);
    // Without one, the font's thickness: the same as `\frac`, not as 1pt or 10pt.
    let font = at(r"\genfrac{}{}{}{}{a}{b}");
    assert!((font.rules[0].h - at(r"\frac{a}{b}").rules[0].h).abs() < EPS);
    assert!(at(r"\genfrac{}{}{0pt}{}{a}{b}").rules.is_empty());
    assert!(at(r"{a \above 0pt b}").rules.is_empty());
    // Absolute: a script-style fraction's rule is as thick.
    let script = at(r"x^{\genfrac{}{}{1pt}{}{a}{b}}");
    assert!((script.rules[0].h - thin.rules[0].h).abs() < EPS);
}

#[test]
fn scripts_go_up_and_down() {
    let (fonts, math) = fonts();
    let f = lay_with(&fonts, math, "x_i", false);
    assert!(
        f.glyphs[1].y < 0.0 && f.glyphs[1].size < 1.0,
        "below the baseline, smaller"
    );
    let f = lay_with(&fonts, math, "x^2", false);
    assert!(f.glyphs[1].y > 0.3);
    // Both: the superscript higher than alone? never lower; the subscript never higher.
    let both = lay_with(&fonts, math, "x^2_i", false);
    assert!(both.glyphs[1].y >= f.glyphs[1].y - EPS);
    assert!(both.glyphs[2].y <= lay_with(&fonts, math, "x_i", false).glyphs[1].y + EPS);
    // Cramped (in a denominator), superscripts sit lower.
    let den = lay_with(&fonts, math, r"\frac{1}{x^2}", true);
    let num = lay_with(&fonts, math, r"\frac{x^2}{1}", true);
    let rise = |f: &Formula, i: usize, j: usize| f.glyphs[j].y - f.glyphs[i].y;
    assert!(rise(&den, 1, 2) < rise(&num, 0, 1) - EPS);
    // Scripts of scripts: scriptscript size.
    let f = lay_with(&fonts, math, "x^{y^z}", false);
    assert_eq!(
        f.glyphs.iter().map(|g| g.size).collect::<Vec<_>>(),
        [1.0, 0.7, 0.55]
    );
}

#[test]
fn limits_above_and_below_in_display_beside_in_text() {
    let (fonts, math) = fonts();
    let d = lay_with(&fonts, math, r"\sum_{i}^{n}", true);
    let (op, n, i) = (d.glyphs[0], d.glyphs[1], d.glyphs[2]);
    let op_w = 1.112; // the display sum's advance, in em
    assert!(
        n.x > op.x && n.x < op.x + op_w && n.y > 0.9,
        "n above: {d:?}"
    );
    assert!(
        i.x > op.x && i.x < op.x + op_w && i.y < -0.7,
        "i below: {d:?}"
    );
    let t = lay_with(&fonts, math, r"\sum_{i}^{n}", false);
    let (op, n, i) = (t.glyphs[0], t.glyphs[1], t.glyphs[2]);
    assert!(n.x > op.x + 0.9 && i.x > op.x + 0.9, "beside: {t:?}");
    // The display sum is the bigger variant.
    assert_ne!(d.glyphs[0].id, t.glyphs[0].id);
    // \limits and \nolimits override.
    let l = lay_with(&fonts, math, r"\int\limits_0^1", false);
    assert!(l.glyphs[1].x < 0.5 && l.glyphs[1].y > 0.5);
    let nl = lay_with(&fonts, math, r"\sum\nolimits_i", true);
    assert!(nl.glyphs[1].x > 1.0);
    // Operator names too: `\lim` takes limits, `\sin` doesn't.
    let lim = lay_with(&fonts, math, r"\lim_{x}", true);
    assert!(lim.glyphs[3].y < -0.3 && lim.glyphs[3].x < 1.0);
    let sin = lay_with(&fonts, math, r"\sin_{x}", true);
    assert!(sin.glyphs[3].x > 1.0);
}

#[test]
fn delimiters_grow_with_their_body() {
    let (fonts, math) = fonts();
    let small = lay_with(&fonts, math, r"\left( x \right)", true);
    let mid = lay_with(&fonts, math, r"\left( \frac{a}{b} \right)", true);
    let tall = lay_with(
        &fonts,
        math,
        r"\left( \frac{\frac{a}{b}}{\frac{c}{d}} \right)",
        true,
    );
    let huge = lay_with(&fonts, math, r"\left( \rule{1pt}{20em} \right)", true);
    let total = |f: &Formula| f.height + f.depth;
    assert!(
        total(&small) < total(&mid) && total(&mid) < total(&tall) && total(&tall) < total(&huge)
    );
    assert_ne!(small.glyphs[1].id, mid.glyphs[1].id, "a bigger variant");
    // Past the variants: an assembly of parts, top, extenders and bottom.
    assert!(huge.glyphs.len() > 10, "{}", huge.glyphs.len());
    let paren = huge.glyphs.iter().filter(|g| g.x < 0.5).collect::<Vec<_>>();
    let span = paren.iter().map(|g| g.y).fold(f32::MIN, f32::max)
        - paren.iter().map(|g| g.y).fold(f32::MAX, f32::min);
    assert!(span > 15.0, "{span}");
    // Centred on the axis, so around a body as deep as it is high, as deep as high.
    let even = lay_with(
        &fonts,
        math,
        r"\left( \rule[-10em]{1pt}{20em} \right)",
        true,
    );
    assert!(
        even.height > 10.0 && even.depth > 9.5,
        "{} {}",
        even.height,
        even.depth
    );
    // `\big` and the others: fixed sizes, growing.
    let sizes: Vec<f32> = [r"\big(", r"\Big(", r"\bigg(", r"\Bigg("]
        .iter()
        .map(|s| total(&lay_with(&fonts, math, s, false)))
        .collect();
    assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{sizes:?}");
    // `\middle` is as tall as `\left`.
    let m = lay_with(&fonts, math, r"\left( \frac{a}{b} \middle| c \right)", true);
    let ids: Vec<u16> = m.glyphs.iter().map(|g| g.id).collect();
    // (STIX's bar has no bigger sizes: it is assembled of itself.)
    let bar = fonts.face(math).glyph('|');
    assert!(ids.iter().filter(|&&id| id == bar).count() >= 2, "{ids:?}");
}

#[test]
fn spacing_follows_the_classes() {
    let (fonts, math) = fonts();
    let w = |s: &str| lay_with(&fonts, math, s, false).width;
    let (a, b, plus, eq) = (w("a"), w("b"), w("+"), w("="));
    let thin = 3.0 / 18.0;
    let (med, thick) = (4.0 / 18.0, 5.0 / 18.0);
    assert!(
        (w("a+b") - (a + plus + b + 2.0 * med)).abs() < EPS,
        "bin: medium"
    );
    assert!(
        (w("a=b") - (a + eq + b + 2.0 * thick)).abs() < EPS,
        "rel: thick"
    );
    assert!(
        (w("+a") - (plus + a)).abs() < EPS,
        "a leading bin is an ord"
    );
    assert!((w("a+") - (a + plus)).abs() < EPS, "and a trailing one");
    assert!(
        (w("a=+b") - (a + eq + plus + b + 2.0 * thick)).abs() < EPS,
        "bin after rel"
    );
    let comma = w(",");
    assert!(
        (w("a,b") - (a + comma + b + thin)).abs() < EPS,
        "punct: thin after"
    );
    let sin = w(r"\sin");
    assert!(
        (w(r"\sin a") - (sin + thin + a)).abs() < EPS,
        "op ord: thin"
    );
    // None of the medium or thick spaces in scripts.
    let sup = lay_with(&fonts, math, "x^{a+b}", false);
    let sup_plain = lay_with(&fonts, math, "x^{a}", false);
    let s = 0.7;
    let grow = sup.width - sup_plain.width;
    assert!((grow - s * (plus + b)).abs() < EPS, "{grow}");
    // Explicit spaces: in mu of the style, and em.
    assert!((w(r"a\,b") - (a + b + thin)).abs() < EPS);
    assert!((w(r"a\quad b") - (a + b + 1.0)).abs() < EPS);
    assert!((w(r"a\!b") - (a + b - thin)).abs() < EPS);
    assert!((w(r"a\mkern18mu b") - (a + b + 1.0)).abs() < EPS);
}

#[test]
fn radicals_accents_and_rules() {
    let (fonts, math) = fonts();
    let x = lay_with(&fonts, math, "x", false);
    let s = lay_with(&fonts, math, r"\sqrt{x}", false);
    assert!(s.height > x.height && s.rules.len() == 1, "{s:?}");
    let bar = s.rules[0];
    assert!(bar.y > x.height && (bar.x + bar.w - s.width).abs() < 0.01);
    let tall = lay_with(&fonts, math, r"\sqrt{\frac{a}{b}}", true);
    assert_ne!(
        tall.glyphs.last().map(|g| g.id),
        s.glyphs.last().map(|g| g.id)
    );
    // The index sits left of the radical, above its middle.
    let i = lay_with(&fonts, math, r"\sqrt[3]{x}", false);
    let three = i.glyphs.iter().find(|g| g.size < 1.0).expect("index");
    assert!(three.x < 0.3 && three.y > 0.0);
    // Accents over the base's top accent point, above it.
    let hat = lay_with(&fonts, math, r"\hat{x}", false);
    assert!((hat.width - x.width).abs() < EPS && hat.height > x.height);
    let tall_hat = lay_with(&fonts, math, r"\hat{X}", false);
    assert!(
        tall_hat.glyphs[1].y > hat.glyphs[1].y,
        "raised over a capital"
    );
    // A wide accent grows to the base.
    let wide = lay_with(&fonts, math, r"\widehat{xxxx}", false);
    let narrow = lay_with(&fonts, math, r"\hat{xxxx}", false);
    assert_ne!(wide.glyphs[4].id, narrow.glyphs[4].id);
    // Overline, underline, boxed: rules.
    assert_eq!(
        lay_with(&fonts, math, r"\overline{x}", false).rules.len(),
        1
    );
    let u = lay_with(&fonts, math, r"\underline{x}", false);
    assert!(u.rules[0].y < 0.0 && u.depth > x.depth);
    assert_eq!(lay_with(&fonts, math, r"\boxed{x}", false).rules.len(), 4);
}

#[test]
fn phantoms_and_smash() {
    let (fonts, math) = fonts();
    let f = lay_with(&fonts, math, r"a\phantom{\frac{x}{y}}b", true);
    assert_eq!(f.glyphs.len(), 2, "nothing drawn");
    assert!(f.height > 1.0, "but as tall");
    let h = lay_with(&fonts, math, r"\hphantom{\frac{x}{y}}", true);
    assert!(h.width > 0.3 && h.height == 0.0);
    let v = lay_with(&fonts, math, r"\vphantom{\frac{x}{y}}", true);
    assert!(v.width == 0.0 && v.height > 1.0);
    let s = lay_with(&fonts, math, r"\smash{\frac{x}{y}}", true);
    assert!(s.height == 0.0 && s.depth == 0.0 && s.glyphs.len() == 2);
}

#[test]
fn text_and_fonts() {
    let (fonts, math) = fonts();
    let f = lay_with(&fonts, math, r"\text{ab}", false);
    let face = fonts.face(math);
    assert_eq!(
        f.glyphs.iter().map(|g| g.id).collect::<Vec<_>>(),
        [face.glyph('a'), face.glyph('b')]
    );
    let b = lay_with(&fonts, math, r"\mathbb{R}", false);
    assert_eq!(b.glyphs[0].id, face.glyph('ℝ'));
    let sf = lay_with(&fonts, math, r"\mathsf{x}\texttt{y}", false);
    assert_eq!(sf.glyphs[0].face, fonts.slot(Slot::Regular));
    assert_eq!(sf.glyphs[1].face, fonts.slot(Slot::Mono));
    // What no face has is listed, for fallback faces to be found.
    let missing = lay_with(&fonts, math, "\u{E000}", false);
    assert_eq!(missing.missing, ['\u{E000}']);
}

#[test]
fn old_switches_laps_and_the_rest() {
    let (fonts, math) = fonts();
    let face = fonts.face(math);
    // `\rm` ends with its group.
    let f = lay_with(&fonts, math, r"{\rm d}d", false);
    assert_eq!(f.glyphs[0].id, face.glyph('d'));
    assert_eq!(f.glyphs[1].id, face.glyph('\u{1D451}'));
    let f = lay_with(&fonts, math, r"{\bf x}", false);
    assert_eq!(f.glyphs[0].id, face.glyph('\u{1D431}'));
    // A lap is zero wide, and its body hangs to the left or the right of the spot.
    let ll = lay_with(&fonts, math, r"\mathllap{xx}", false);
    let rl = lay_with(&fonts, math, r"\mathrlap{xx}", false);
    let cl = lay_with(&fonts, math, r"\mathclap{xx}", false);
    assert_eq!((ll.width, rl.width, cl.width), (0.0, 0.0, 0.0));
    let x = |f: &Formula| f.glyphs.iter().map(|g| g.x).fold(f32::MAX, f32::min);
    assert!(x(&ll) < -0.5 && x(&rl) == 0.0 && x(&cl) < -0.2 && x(&cl) > x(&ll));
    // The switch before `\over` covers the denominator too.
    let f = lay_with(&fonts, math, r"{\rm a \over b}", false);
    assert!(f.glyphs.iter().any(|g| g.id == face.glyph('a')));
    assert!(f.glyphs.iter().any(|g| g.id == face.glyph('b')));
    // Boxes and laps take text: upright, and the space stays.
    let f = lay_with(&fonts, math, r"\fbox{Na}", false);
    assert!(f.glyphs.iter().any(|g| g.id == face.glyph('N')));
    let f = lay_with(&fonts, math, r"\llap{a}", false);
    assert!(f.glyphs.iter().any(|g| g.id == face.glyph('a')));
    let f = lay_with(&fonts, math, r"\text{a \bf b}", false);
    assert!(f.glyphs.iter().any(|g| g.id == face.glyph('\u{1D41B}')));
    // `\pmod` has its parentheses and a gap before them.
    let p = lay_with(&fonts, math, r"a \pmod{n}", false);
    let plain = lay_with(&fonts, math, "a (mod n)", false);
    assert_eq!(p.glyphs.len(), plain.glyphs.len());
    assert!(p.width > plain.width, "the gap before it");
    // A strut has height and depth and nothing to draw.
    let st = lay_with(&fonts, math, r"\strut", false);
    assert!(st.glyphs.is_empty() && st.rules.is_empty());
    assert!((st.height - 0.84).abs() < 1e-3 && (st.depth - 0.36).abs() < 1e-3);
    // `\xlongequal` is two rules as long as its label; `\fbox` frames; `\eqref` has its label.
    let eq = lay_with(&fonts, math, r"\xlongequal{abcdefgh}", false);
    assert_eq!(eq.rules.len(), 2);
    assert!(eq.rules[0].w >= eq.width - 1e-3);
    assert_eq!(lay_with(&fonts, math, r"\fbox{a}", false).rules.len(), 4);
    let r = lay_with(&fonts, math, r"\eqref{a1}", false);
    assert_eq!(r.glyphs.len(), 4);
    // `matrix*` aligns its columns as told.
    let m = lay_with(
        &fonts,
        math,
        r"\begin{matrix*}[r] 1 & 2 \\ 333 & 4 \end{matrix*}",
        false,
    );
    let ones = x_of(&fonts, math, &m, '1');
    let threes = x_of(&fonts, math, &m, '3');
    assert!(ones[0].0 > threes[0].0 + 0.5, "{ones:?} {threes:?}");
    // The rest lays out, with every glyph in the font.
    for src in [
        r"\begin{eqnarray} a & = & b \\ cc & = & d \end{eqnarray}",
        r"\begin{multline} a \\ b \end{multline}",
        r"\genfrac(){0pt}{}{a}{b} {a \above 1pt b} \llbracket x \rrbracket \iddots",
        r"\varliminf_n \varlimsup_n \utilde{abc}",
    ] {
        let f = lay_with(&fonts, math, src, true);
        check(&f);
        assert!(f.missing.is_empty(), "{src}: {:?}", f.missing);
    }
}

/// `∫` in display style is set larger than its text size, whatever the font's minimum says.
#[test]
fn a_display_integral_grows() {
    let (fonts, math) = fonts();
    let small = lay_with(&fonts, math, r"\int", false);
    let big = lay_with(&fonts, math, r"\int", true);
    assert!(big.height + big.depth > (small.height + small.depth) * 1.4);
}

/// A cut-off window shows what the whole paint shows: the tall size variants of radicals and
/// brackets (up to 2.6 em above their baseline) aren't skipped while any of them is in view.
#[test]
fn glyphs_are_not_culled_before_they_leave_the_window() {
    let (fonts, math) = fonts();
    let f = lay_with(
        &fonts,
        math,
        r"\sqrt{\dfrac{\dfrac{a}{b}}{\dfrac{c}{d}}} \quad \left\langle \rule{0pt}{2.6em} \right\rangle \quad \sqrt{\rule{0pt}{2.2em}}",
        true,
    );
    let (em, pad) = (BODY * MATH_SCALE, 8.0);
    let mut out = Emitted::default();
    emit(&f, em, pad, pad + f.height * em, Role::Text, &mut out);
    let block = BlockBox {
        glyphs: out.glyphs,
        styles: out.styles,
        ink: out.ink,
        cuts: out.cuts,
        ..BlockBox::default()
    };
    let w = ((f.width * em) + 2.0 * pad).ceil() as usize;
    let h = ((f.height + f.depth) * em + 2.0 * pad).ceil() as usize;
    let mut cache = GlyphCache::default();
    // The block painted at `oy` into a frame `rows` high.
    let mut paint = |rows: usize, oy: f32| {
        let mut buf = vec![LIGHT.bg; w * rows];
        let mut frame = Frame::new(&mut buf, w, rows, w);
        paint_block(
            &mut frame,
            &block,
            0.0,
            oy,
            &fonts,
            &mut cache,
            &LIGHT,
            #[cfg(feature = "clipboard")]
            None,
        );
        buf
    };
    let full = paint(h, 0.0);
    for cut in 1..h {
        // The window's bottom edge at `cut`, and its top edge at `cut`.
        let bottom = paint(cut, 0.0);
        assert!(bottom[..] == full[..cut * w], "bottom edge at {cut}");
        let top = paint(h - cut, -(cut as f32));
        assert!(top[..] == full[cut * w..], "top edge at {cut}");
    }
}

#[test]
fn limits_and_errors() {
    let (fonts, math) = fonts();
    let mut e = Engine::new();
    let err = |src: &str, e: &mut Engine| e.layout(&fonts, math, src, true).unwrap_err().message;
    assert_eq!(err(r"\frac{", &mut e), "missing }");
    // Too wide: 100 em at most a kern, 1000 em at most a formula.
    let wide = r"\kern100em".repeat(11);
    assert_eq!(err(&wide, &mut e), "formula too large");
    assert!(
        e.layout(&fonts, math, &r"\kern100em".repeat(9), true)
            .is_ok()
    );
    // Too many glyphs: `\left(` around a tall rule takes up to 64 extenders; many of them.
    let many = r"\left(\rule{1pt}{100em}\right)".repeat(200);
    assert!(many.len() <= crate::math::MAX_SOURCE);
    assert_eq!(err(&many, &mut e), "formula too big");
    // Deep nesting stays within the extent check, finite.
    let deep = format!("{}x{}", r"\sqrt{".repeat(40), "}".repeat(40));
    let f = e.layout(&fonts, math, &deep, true).expect("deep");
    assert!(f.glyphs.iter().all(|g| g.x.is_finite() && g.y.is_finite()));
}

#[test]
fn a_tall_assembly_stops_at_the_part_limit() {
    let (fonts, math) = fonts();
    let f = lay_with(&fonts, math, r"\left(\rule{1pt}{100em}\right.", true);
    let parts = f.glyphs.len();
    assert!(parts <= crate::math::MAX_PARTS + 4, "{parts}");
}

/// Negative space makes no formula narrower than nothing (fuzz finding: `\!\!` was a box
/// −1/3 em wide; fuzz/seeds/math/negative-space-width.bin).
#[test]
fn negative_space_makes_no_negative_width() {
    let (fonts, math) = fonts();
    for (src, display) in [
        (r"\!\!", false),
        (r"\kern-50em x", true),
        (r"\hspace{-3em}", false),
        (r"x \tag{\!\!\!}", true),
    ] {
        let f = lay_with(&fonts, math, src, display);
        assert!(f.width >= 0.0, "{src}: {}", f.width);
        assert!(f.tag.as_ref().is_none_or(|t| t.width >= 0.0), "{src}");
    }
}

/// What every formula laid out must be: finite, inside the extent, within the glyph limit.
fn check(f: &Formula) {
    let ok = |v: f32| v.is_finite() && v.abs() <= crate::math::MAX_EXTENT;
    assert!(ok(f.width) && ok(f.height) && ok(f.depth));
    assert!(f.width >= 0.0 && f.height >= 0.0 && f.depth >= 0.0);
    assert!(f.glyphs.len() + f.rules.len() <= crate::math::MAX_GLYPHS);
    assert!(f.glyphs.iter().all(|g| ok(g.x) && ok(g.y) && g.size > 0.0));
    assert!(
        f.rules
            .iter()
            .all(|r| ok(r.x) && ok(r.y) && ok(r.w) && ok(r.h))
    );
}

/// Every input of `fuzz/seeds/math/` (samples, and past findings) lays out or fails cleanly,
/// and is fitted into the column the `math` fuzz target picks for it, as that target does.
#[test]
fn fuzz_seeds() {
    let (fonts, math) = fonts();
    let mut engine = Engine::new();
    let dir = format!("{}/fuzz/seeds/math", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for entry in std::fs::read_dir(dir).expect("seeds") {
        let data = std::fs::read(entry.expect("entry").path()).expect("seed");
        let Some((&b, rest)) = data.split_first() else {
            continue;
        };
        let src = String::from_utf8_lossy(rest);
        if let Ok(f) = engine.layout(&fonts, math, &src, b & 1 != 0) {
            check(&f);
            if let Some(t) = &f.tag {
                check(t);
            }
            let avail = 1.0 + f32::from(b >> 1) * 0.5;
            let (g, s) = crate::math::fit::fit(&f, avail);
            assert!(g.width * s <= avail && s > 0.0 && s <= 1.0, "{src}");
            assert!(g.height + g.depth <= crate::math::MAX_EXTENT, "{src}");
        }
        n += 1;
    }
    assert!(n >= 40, "{n}");
}

/// Random soups of tokens: parse and layout never panic, and what lays out is in bounds.
#[test]
fn token_soup() {
    const TOKENS: &[&str] = &[
        r"\frac",
        r"\over",
        r"\sqrt",
        r"\left(",
        r"\right)",
        r"\middle|",
        r"\big(",
        r"\sum",
        r"\int",
        r"\lim",
        r"\limits",
        r"\hat",
        r"\widehat",
        r"\overbrace",
        r"\underset",
        r"\xrightarrow",
        r"\text",
        r"\mathbb",
        r"\not",
        r"\phantom",
        r"\smash",
        r"\boxed",
        r"\rule",
        r"\kern",
        "{",
        "}",
        "^",
        "_",
        "'",
        "x",
        "1",
        "+",
        "=",
        "(",
        ")",
        "[",
        "]",
        r"\alpha",
        r"\,",
        r"\quad",
        r"\displaystyle",
        r"\scriptstyle",
        "$",
        "&",
        r"\\",
        "-9em",
        "2em",
        r"\binom",
        r"\operatorname*",
        r"\color{red}",
        r"\tag{1}",
        r"\vec",
        r"\left.",
    ];
    let (fonts, math) = fonts();
    let mut engine = Engine::new();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let (mut ok, mut failed) = (0, 0);
    for _ in 0..4000 {
        let mut src = String::new();
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        for _ in 0..(seed % 16) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            src.push_str(TOKENS[(seed % TOKENS.len() as u64) as usize]);
            if seed & 0x100 != 0 {
                src.push(' ');
            }
        }
        match engine.layout(&fonts, math, &src, seed & 0x200 != 0) {
            Ok(f) => {
                check(&f);
                ok += 1;
            }
            Err(e) => {
                assert!(e.span.end <= src.len(), "{src}: {e:?}");
                failed += 1;
            }
        }
    }
    assert!(ok > 500 && failed > 500, "{ok} laid out, {failed} failed");
}

/// The x of the glyph of `ch` in `f` (the first one), in the math face.
fn x_of(fonts: &Fonts, math: FaceId, f: &Formula, ch: char) -> Vec<(f32, f32)> {
    let id = fonts.face(math).glyph(ch);
    f.glyphs
        .iter()
        .filter(|g| g.id == id)
        .map(|g| (g.x, g.y))
        .collect()
}

#[test]
fn matrices_set_rows_down_and_columns_across_centred_on_the_axis() {
    let (fonts, math) = fonts();
    let f = lay_with(
        &fonts,
        math,
        r"\begin{matrix} 1 & 2 \\ 3 & 4 \end{matrix}",
        true,
    );
    let at = |ch| x_of(&fonts, math, &f, ch)[0];
    let (one, two, three, four) = (at('1'), at('2'), at('3'), at('4'));
    assert!(
        one.1 == two.1 && three.1 == four.1 && one.1 > three.1,
        "rows"
    );
    assert!(
        (one.0 - three.0).abs() < EPS && two.0 > one.0 + 0.9,
        "columns, a quad apart"
    );
    // Centred on the axis.
    let axis = 258.0 / 1000.0;
    assert!(((f.height - f.depth) / 2.0 - axis).abs() < 1e-3, "{f:?}");
    // In delimiters as tall as it.
    let p = lay_with(
        &fonts,
        math,
        r"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}",
        true,
    );
    assert!(p.width > f.width && p.height >= f.height && p.depth >= f.depth);
    // A row's extra space, and rows of fewer cells.
    let g = lay_with(
        &fonts,
        math,
        r"\begin{matrix} 1 & 2 \\[1em] 3 \end{matrix}",
        true,
    );
    assert!(g.height + g.depth > f.height + f.depth + 0.9);
    // A big one stays finite and bounded.
    let big = format!(
        r"\begin{{matrix}}{}\end{{matrix}}",
        vec![vec!["x"; 60].join("&"); 60].join(r"\\")
    );
    let b = lay_with(&fonts, math, &big, true);
    assert_eq!(b.glyphs.len(), 3600);
}

#[test]
fn aligned_lines_its_relations_up() {
    let (fonts, math) = fonts();
    let f = lay_with(
        &fonts,
        math,
        r"\begin{aligned} f(x) &= x + 1 \\ g &= 2 \\ &= y \end{aligned}",
        true,
    );
    let eqs = x_of(&fonts, math, &f, '=');
    assert_eq!(eqs.len(), 3);
    assert!(eqs.iter().all(|e| (e.0 - eqs[0].0).abs() < EPS), "{eqs:?}");
    assert!(eqs[0].1 > eqs[1].1 && eqs[1].1 > eqs[2].1);
    // `&=` is spaced as a relation, as if after an atom: the `=` doesn't start its column.
    let g = x_of(&fonts, math, &f, '𝑔')[0];
    assert!(eqs[1].0 > g.0 + 0.5);
}

#[test]
fn cases_are_left_aligned_in_a_brace() {
    let (fonts, math) = fonts();
    let f = lay_with(
        &fonts,
        math,
        r"\begin{cases} 1 & a \\ 22 & bb \end{cases}",
        true,
    );
    let ones = x_of(&fonts, math, &f, '1');
    let twos = x_of(&fonts, math, &f, '2');
    assert!((ones[0].0 - twos[0].0).abs() < EPS, "left aligned");
    let brace = x_of(&fonts, math, &f, '{');
    let x = brace.first().map_or(0.0, |b| b.0);
    assert!(x < ones[0].0, "the brace first");
}

#[test]
fn array_rules_cross_the_whole_array() {
    let (fonts, math) = fonts();
    let f = lay_with(
        &fonts,
        math,
        r"\begin{array}{c|c} a & b \\ \hline c & d \end{array}",
        true,
    );
    assert_eq!(f.rules.len(), 2);
    let (v, h) = if f.rules[0].h > f.rules[0].w {
        (f.rules[0], f.rules[1])
    } else {
        (f.rules[1], f.rules[0])
    };
    assert!(
        (h.w - f.width).abs() < EPS,
        "the \\hline is as wide as the array"
    );
    assert!(
        (v.h - (f.height + f.depth)).abs() < 1e-3,
        "the | as tall: {v:?} {f:?}"
    );
    // Between the columns.
    let (a, b) = (
        x_of(&fonts, math, &f, '𝑎')[0],
        x_of(&fonts, math, &f, '𝑏')[0],
    );
    assert!(a.0 < v.x && v.x < b.0);
    // Dashed: in pieces.
    let d = lay_with(
        &fonts,
        math,
        r"\begin{array}{c:c} a & b \\ \hdashline c & d \end{array}",
        true,
    );
    assert!(d.rules.len() > 4);
}

#[test]
fn rows_of_the_top_level_and_tags() {
    let (fonts, math) = fonts();
    let one = lay_with(&fonts, math, "a + b", true);
    let two = lay_with(&fonts, math, r"a + b \\ c", true);
    assert!(two.height + two.depth > one.height + one.depth + 1.0);
    assert!(one.tag.is_none() && !one.pieces.is_empty());
    assert!(two.pieces.is_empty(), "rows of its own don't break");
    let t = lay_with(&fonts, math, r"x = 1 \tag{3}", true);
    let tag = t.tag.as_deref().expect("tag");
    assert_eq!(tag.glyphs.len(), 3, "(3)");
    assert!(!t.pieces.is_empty(), "the tag leaves the pieces");
    let star = lay_with(&fonts, math, r"x \tag*{A}", true);
    assert_eq!(star.tag.as_deref().unwrap().glyphs.len(), 1);
}

#[test]
fn pieces_cover_the_top_level_in_order() {
    let (fonts, math) = fonts();
    let f = lay_with(&fonts, math, r"a = \frac{b}{c} + d \le e", true);
    let p = &f.pieces;
    assert!(
        p.windows(2)
            .all(|w| w[0].x <= w[1].x && w[0].glyphs <= w[1].glyphs)
    );
    let breaks: Vec<_> = p.iter().filter_map(|p| p.breaks).collect();
    assert_eq!(breaks, [Break::Rel, Break::Bin, Break::Rel]);
    assert_eq!(p[0].glyphs, 0);
    // A `\middle` lays out out of order: no pieces.
    let m = lay_with(&fonts, math, r"\left( a \middle| b \right) = c", true);
    assert!(!m.pieces.is_empty(), "the \\middle is inside one piece");
}

#[test]
fn assembly_parts_are_cut_where_the_next_takes_over() {
    let (fonts, math) = fonts();
    for (src, vertical) in [
        (r"\left( \rule{1pt}{6em} \right.", true),
        (r"\left\| \rule{1pt}{5em} \right.", true),
        (r"\overbrace{aaaaaaaaaaaa}", false),
    ] {
        let f = lay_with(&fonts, math, src, true);
        let cuts: Vec<_> = f.glyphs.iter().filter_map(|g| g.cut).collect();
        assert!(cuts.len() >= 3, "{src}: an assembly");
        assert!(cuts.iter().all(|c| c.vertical == vertical));
        assert!(cuts[0].lo.is_infinite() && cuts[cuts.len() - 1].hi.is_infinite());
        // Each band starts where the one before ends: no gap, no overlap.
        for w in cuts.windows(2) {
            assert!((w[0].hi - w[1].lo).abs() < 1e-4, "{src}: {w:?}");
            assert!(w[0].lo < w[0].hi);
        }
    }
}
