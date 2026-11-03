//! What the names and characters of a formula stand for: the symbol table (`\alpha`, `\leq`,
//! `\sum`), the class each character takes in spacing, the accents, and the math alphabets
//! (`\mathbf`, `\mathbb`, …), which map letters into Unicode's Mathematical Alphanumeric
//! Symbols.
//!
//! The symbols and their classes are KaTeX's (`src/symbols.ts`, MIT, © Khan Academy and other
//! contributors), less the ones that need KaTeX's own fonts, plus amsmath's names KaTeX makes
//! with macros (`\neq`, `\notin`, `\cdots`, `\implies`, …) and the slanted capitals
//! (`\varGamma`). Every character here is in the embedded font; `assets/fonts/math-codepoints.txt`
//! is made from [`font_chars`] (a test keeps the three in step).

use std::sync::OnceLock;

/// The class of an atom, which decides the space around it (TeX's eight).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
}

/// A symbol of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub ch: char,
    pub class: Class,
    /// A big operator whose scripts go above and below it in display style (`\sum`, not
    /// `\int`).
    pub limits: bool,
}

/// One symbol a line, sorted by name (bytewise): the name without its backslash, a space, a
/// letter for the class (`o` ord, `L` operator with limits, `O` operator without, `b` bin,
/// `r` rel, `(` open, `)` close, `p` punct, `i` inner), and the character.
static SYMBOLS: &str = "\
# o#
$ o$
% o%
& o&
Alpha oA
And b&
Bbbk o𝕜
Beta oB
Box o□
Bumpeq r≎
Cap b⋒
Chi oX
Cup b⋓
Delta oΔ
Diamond o◊
Doteq r≑
Downarrow r⇓
Epsilon oE
Eta oH
Finv oℲ
Game o⅁
Gamma oΓ
Im oℑ
Iota oI
Join r⋈
Kappa oK
Lambda oΛ
Leftarrow r⇐
Leftrightarrow r⇔
Lleftarrow r⇚
Longleftarrow r⟸
Longleftrightarrow r⟺
Longrightarrow r⟹
Lsh r↰
Mu oM
Nu oN
Omega oΩ
Omicron oO
P o¶
Phi oΦ
Pi oΠ
Psi oΨ
Re oℜ
Rho oP
Rightarrow r⇒
Rrightarrow r⇛
Rsh r↱
S o§
Sigma oΣ
Subset r⋐
Supset r⋑
Tau oT
Theta oΘ
Uparrow r⇑
Updownarrow r⇕
Upsilon oΥ
Vdash r⊩
Vert o‖
Vvdash r⊪
Xi oΞ
Zeta oZ
_ o_
aleph oℵ
alpha oα
amalg b⨿
angle o∠
approx r≈
approxeq r≊
ast b∗
asymp r≍
backepsilon r∍
backprime o‵
backsim r∽
backsimeq r⋍
backslash o\\
barwedge b⊼
because r∵
beta oβ
beth oℶ
between r≬
bigcap L⋂
bigcirc b◯
bigcup L⋃
bigodot L⨀
bigoplus L⨁
bigotimes L⨂
bigsqcup L⨆
bigstar o★
bigtimes L⨉
bigtriangledown b▽
bigtriangleup b△
biguplus L⨄
bigvee L⋁
bigwedge L⋀
blacklozenge o⧫
blacksquare o■
blacktriangle o▲
blacktriangledown o▼
blacktriangleleft r◀
blacktriangleright r▶
bot o⊥
bowtie r⋈
boxdot b⊡
boxminus b⊟
boxplus b⊞
boxtimes b⊠
bullet b∙
bumpeq r≏
cap b∩
cdot b⋅
cdotp p⋅
cdots i⋯
centerdot b⋅
checkmark o✓
chi oχ
circ b∘
circeq r≗
circlearrowleft r↺
circlearrowright r↻
circledR o®
circledS oⓈ
circledast b⊛
circledcirc b⊚
circleddash b⊝
clubsuit o♣
colon p:
coloneqq r≔
complement o∁
cong r≅
coprod L∐
cup b∪
curlyeqprec r⋞
curlyeqsucc r⋟
curlyvee b⋎
curlywedge b⋏
curvearrowleft r↶
curvearrowright r↷
dag o†
dagger b†
daleth oℸ
dashleftarrow r⇠
dashrightarrow r⇢
dashv r⊣
ddag o‡
ddagger b‡
ddots i⋱
degree o°
delta oδ
diagdown o╲
diagup o╱
diamond b⋄
diamondsuit o♢
digamma oϝ
div b÷
divideontimes b⋇
doteq r≐
doteqdot r≑
dotplus b∔
dots i…
dotsb i⋯
dotsc i…
dotsi i⋯
dotsm i⋯
dotso i…
doublebarwedge b⩞
doublecap b⋒
doublecup b⋓
downarrow r↓
downdownarrows r⇊
downharpoonleft r⇃
downharpoonright r⇂
ell oℓ
emptyset o∅
epsilon oϵ
eqcirc r≖
eqqcolon r≕
eqsim r≂
eqslantgtr r⪖
eqslantless r⪕
equiv r≡
eta oη
eth oð
exists o∃
fallingdotseq r≒
flat o♭
forall o∀
frown r⌢
gamma oγ
ge r≥
geq r≥
geqq r≧
geqslant r⩾
gets r←
gg r≫
ggg r⋙
gggtr r⋙
gimel oℷ
gnapprox r⪊
gneq r⪈
gneqq r≩
gnsim r⋧
gt r>
gtrapprox r⪆
gtrdot b⋗
gtreqless r⋛
gtreqqless r⪌
gtrless r≷
gtrsim r≳
hbar oℏ
heartsuit o♡
hookleftarrow r↩
hookrightarrow r↪
hslash oℏ
iddots i⋰
iff r⟺
iiint O∭
iint O∬
imageof r⊷
imath oı
impliedby r⟸
implies r⟹
in r∈
infty o∞
int O∫
intercal b⊺
intop O∫
iota oι
jmath oȷ
kappa oκ
lVert (‖
lambda oλ
land b∧
lang (⟨
langle (⟨
lbrace ({
lbrack ([
lceil (⌈
ldotp p.
ldots i…
le r≤
leadsto r⇝
leftarrow r←
leftarrowtail r↢
leftharpoondown r↽
leftharpoonup r↼
leftleftarrows r⇇
leftrightarrow r↔
leftrightarrows r⇆
leftrightharpoons r⇋
leftrightsquigarrow r↭
leftthreetimes b⋋
leq r≤
leqq r≦
leqslant r⩽
lessapprox r⪅
lessdot b⋖
lesseqgtr r⋚
lesseqqgtr r⪋
lessgtr r≶
lesssim r≲
lfloor (⌊
lgroup (⟮
lhd b⊲
ll r≪
llbracket (⟦
llcorner (⌞
lll r⋘
llless r⋘
lmoustache (⎰
lnapprox r⪉
lneq r⪇
lneqq r≨
lnot o¬
lnsim r⋦
longleftarrow r⟵
longleftrightarrow r⟷
longmapsto r⟼
longrightarrow r⟶
looparrowleft r↫
looparrowright r↬
lor b∨
lozenge o◊
lparen ((
lrcorner )⌟
lt r<
ltimes b⋉
lvert (|
maltese o✠
mapsto r↦
mathellipsis i…
mathsterling o£
measuredangle o∡
mho o℧
mid r∣
models r⊨
mp b∓
mu oμ
multimap r⊸
nLeftarrow r⇍
nLeftrightarrow r⇎
nRightarrow r⇏
nVDash r⊯
nVdash r⊮
nabla o∇
natural o♮
ncong r≆
ne r≠
nearrow r↗
neg o¬
neq r≠
nexists o∄
ngeq r≱
ngtr r≯
ni r∋
nleftarrow r↚
nleftrightarrow r↮
nleq r≰
nless r≮
nmid r∤
notin r∉
nparallel r∦
nprec r⊀
npreceq r⋠
nrightarrow r↛
nsim r≁
nsubseteq r⊈
nsucc r⊁
nsucceq r⋡
nsupseteq r⊉
ntriangleleft r⋪
ntrianglelefteq r⋬
ntriangleright r⋫
ntrianglerighteq r⋭
nu oν
nvDash r⊭
nvdash r⊬
nwarrow r↖
odot b⊙
oiiint O∰
oiint O∯
oint O∮
omega oω
omicron oο
ominus b⊖
oplus b⊕
origof r⊶
oslash b⊘
otimes b⊗
owns r∋
parallel r∥
partial o∂
perp r⊥
phi oϕ
pi oπ
pitchfork r⋔
pm b±
pounds o£
prec r≺
precapprox r⪷
preccurlyeq r≼
preceq r⪯
precnapprox r⪹
precneqq r⪵
precnsim r⋨
precsim r≾
prime o′
prod L∏
propto r∝
psi oψ
rVert )‖
rang )⟩
rangle )⟩
rbrace )}
rbrack )]
rceil )⌉
restriction r↾
rfloor )⌋
rgroup )⟯
rhd b⊳
rho oρ
rightarrow r→
rightarrowtail r↣
rightharpoondown r⇁
rightharpoonup r⇀
rightleftarrows r⇄
rightleftharpoons r⇌
rightrightarrows r⇉
rightsquigarrow r⇝
rightthreetimes b⋌
risingdotseq r≓
rmoustache )⎱
rparen ))
rrbracket )⟧
rtimes b⋊
rvert )|
searrow r↘
setminus b∖
sharp o♯
shortmid r∣
shortparallel r∥
sigma oσ
sim r∼
simeq r≃
smallfrown r⌢
smallint O∫
smallsetminus b∖
smallsmile r⌣
smile r⌣
spadesuit o♠
sphericalangle o∢
sqcap b⊓
sqcup b⊔
sqsubset r⊏
sqsubseteq r⊑
sqsupset r⊐
sqsupseteq r⊒
square o□
star b⋆
subset r⊂
subseteq r⊆
subseteqq r⫅
subsetneq r⊊
subsetneqq r⫋
succ r≻
succapprox r⪸
succcurlyeq r≽
succeq r⪰
succnapprox r⪺
succneqq r⪶
succnsim r⋩
succsim r≿
sum L∑
supset r⊃
supseteq r⊇
supseteqq r⫆
supsetneq r⊋
supsetneqq r⫌
surd o√
swarrow r↙
tau oτ
therefore r∴
theta oθ
thickapprox r≈
thicksim r∼
times b×
to r→
top o⊤
triangle o△
triangledown o▽
triangleleft b◃
trianglelefteq r⊴
triangleq r≜
triangleright b▹
trianglerighteq r⊵
twoheadleftarrow r↞
twoheadrightarrow r↠
ulcorner (⌜
unlhd b⊴
unrhd b⊵
uparrow r↑
updownarrow r↕
upharpoonleft r↿
upharpoonright r↾
uplus b⊎
upsilon oυ
upuparrows r⇈
urcorner )⌝
vDash r⊨
varDelta o𝛥
varGamma o𝛤
varLambda o𝛬
varOmega o𝛺
varPhi o𝛷
varPi o𝛱
varPsi o𝛹
varSigma o𝛴
varTheta o𝛩
varUpsilon o𝛶
varXi o𝛯
varepsilon oε
varkappa oϰ
varnothing o⌀
varphi oφ
varpi oϖ
varpropto r∝
varrho oϱ
varsigma oς
vartheta oϑ
vartriangle r△
vartriangleleft r⊲
vartriangleright r⊳
varvdots o⋮
vdash r⊢
vdots o⋮
vee b∨
veebar b⊻
vert o|
wedge b∧
wp o℘
wr b≀
xi oξ
yen o¥
zeta oζ
{ ({
| o‖
} )}";

fn decode(entry: &str) -> Option<Symbol> {
    let mut chars = entry.chars();
    let (class, limits) = match chars.next()? {
        'o' => (Class::Ord, false),
        'L' => (Class::Op, true),
        'O' => (Class::Op, false),
        'b' => (Class::Bin, false),
        'r' => (Class::Rel, false),
        '(' => (Class::Open, false),
        ')' => (Class::Close, false),
        'p' => (Class::Punct, false),
        _ => (Class::Inner, false),
    };
    Some(Symbol {
        ch: chars.next()?,
        class,
        limits,
    })
}

/// The table's lines.
fn entries() -> impl Iterator<Item = (&'static str, Symbol)> {
    SYMBOLS.lines().filter_map(|line| {
        let (name, rest) = line.split_once(' ')?;
        Some((name, decode(rest)?))
    })
}

/// The symbol `\name` stands for (`name` without the backslash).
pub fn symbol(name: &str) -> Option<Symbol> {
    // A binary search over the lines: halve the bytes, and step back to the line's start.
    // (Bytes: `mid` may fall inside a character; a line starts and ends at a boundary.)
    let t = SYMBOLS.as_bytes();
    let (mut lo, mut hi) = (0, t.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let start = t
            .get(..mid)?
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        let end = t
            .get(start..)?
            .iter()
            .position(|&b| b == b'\n')
            .map_or(t.len(), |i| start + i);
        let (n, rest) = SYMBOLS.get(start..end)?.split_once(' ')?;
        match n.cmp(name) {
            std::cmp::Ordering::Less => lo = end + 1,
            std::cmp::Ordering::Greater => hi = start,
            std::cmp::Ordering::Equal => return decode(rest),
        }
    }
    None
}

/// The class of a character typed as itself: `+`, `=`, `(`, or `≤` and the other characters
/// of the table. Letters, digits and anything else are ord.
pub fn class_of(ch: char) -> Class {
    match ch {
        '+' | '-' | '*' | '\u{2212}' => Class::Bin,
        '=' | '<' | '>' | ':' => Class::Rel,
        ',' | ';' => Class::Punct,
        '(' | '[' => Class::Open,
        ')' | ']' | '!' | '?' => Class::Close,
        c if c.is_ascii() => Class::Ord,
        c => {
            static BY_CHAR: OnceLock<Vec<(char, Class)>> = OnceLock::new();
            let table = BY_CHAR.get_or_init(|| {
                let mut v: Vec<(char, Class)> = entries().map(|(_, s)| (s.ch, s.class)).collect();
                // One class a character: the first name's, but an ord gives way to the
                // others (`∣` is `\mid`, a relation, before it is `\vert`).
                v.sort_by_key(|&(c, class)| (c, class == Class::Ord));
                v.dedup_by_key(|e| e.0);
                v
            });
            table
                .binary_search_by_key(&c, |e| e.0)
                .ok()
                .and_then(|i| table.get(i))
                .map_or(Class::Ord, |e| e.1)
        }
    }
}

/// How an accent sits on its base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccentKind {
    /// The glyph as it is (`\hat`).
    Plain,
    /// Grown to the base's width (`\widehat`).
    Wide,
    /// An arrow over the base, grown to its width (`\overrightarrow`).
    Arrow,
    /// The same, under the base.
    UnderArrow,
}

/// The accent `\name` puts over its argument.
pub fn accent(name: &str) -> Option<(char, AccentKind)> {
    use AccentKind::*;
    Some(match name {
        "hat" => ('\u{302}', Plain),
        "check" => ('\u{30C}', Plain),
        "tilde" => ('\u{303}', Plain),
        "acute" => ('\u{301}', Plain),
        "grave" => ('\u{300}', Plain),
        "dot" => ('\u{307}', Plain),
        "ddot" => ('\u{308}', Plain),
        "dddot" => ('\u{20DB}', Plain),
        "breve" => ('\u{306}', Plain),
        "bar" => ('\u{304}', Plain),
        "vec" => ('\u{20D7}', Plain),
        "mathring" => ('\u{30A}', Plain),
        "widehat" => ('\u{302}', Wide),
        "widetilde" => ('\u{303}', Wide),
        "widecheck" => ('\u{30C}', Wide),
        "overrightarrow" => ('\u{2192}', Arrow),
        "overleftarrow" => ('\u{2190}', Arrow),
        "overleftrightarrow" => ('\u{2194}', Arrow),
        "Overrightarrow" => ('\u{21D2}', Arrow),
        "underrightarrow" => ('\u{2192}', UnderArrow),
        "underleftarrow" => ('\u{2190}', UnderArrow),
        "underleftrightarrow" => ('\u{2194}', UnderArrow),
        _ => return None,
    })
}

/// The character a delimiter name or character stands for after `\left`, `\big` and the
/// others; `Some(None)` for the null delimiter `.`.
pub fn delimiter(name: &str) -> Option<Option<char>> {
    Some(Some(match name {
        "." => return Some(None),
        "(" | ")" | "[" | "]" | "|" | "/" => name.chars().next()?,
        "<" | "langle" => '\u{27E8}',
        ">" | "rangle" => '\u{27E9}',
        "\\" | "backslash" => '\\',
        "uparrow" | "downarrow" | "updownarrow" | "Uparrow" | "Downarrow" | "Updownarrow" => {
            symbol(name)?.ch
        }
        _ => {
            let s = symbol(name)?;
            if !matches!(s.class, Class::Open | Class::Close) && !matches!(s.ch, '|' | '‖') {
                return None;
            }
            s.ch
        }
    }))
}

/// A math alphabet: what `\mathbf` and the others do to letters and digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// Letters and lowercase Greek slanted, the rest upright: TeX's default.
    Normal,
    Roman,
    Italic,
    Bold,
    /// Bold and slanted (`\boldsymbol`).
    BoldItalic,
    Script,
    Fraktur,
    DoubleStruck,
    /// Set in the body text's faces, which have these: the math font doesn't (`\mathsf`).
    Sans,
    Mono,
}

/// `ch` in the alphabet `font`, from the Mathematical Alphanumeric Symbols block and its
/// holes in Letterlike Symbols (`ℎ ℬ ℭ ℂ …`). Characters the alphabet has no form of stay as
/// they are.
pub fn styled(ch: char, font: Font) -> char {
    let latin = match ch {
        'A'..='Z' => Some(ch as u32 - 'A' as u32),
        'a'..='z' => Some(ch as u32 - 'a' as u32 + 26),
        _ => None,
    };
    let greek = greek_index(ch);
    let from = |base: u32, i: u32| char::from_u32(base + i).unwrap_or(ch);
    match font {
        Font::Normal => match (latin, greek, ch) {
            (_, _, 'ı') => '\u{1D6A4}',
            (_, _, 'ȷ') => '\u{1D6A5}',
            (Some(i), ..) => italic(i).unwrap_or_else(|| from(0x1D434, i)),
            // Lowercase Greek and its variants; capitals stay upright.
            (_, Some(i), _) if i >= 26 && i != 51 => from(0x1D6E2, i),
            _ => ch,
        },
        Font::Roman | Font::Sans | Font::Mono => ch,
        Font::Italic => match (latin, greek) {
            (Some(i), _) => italic(i).unwrap_or_else(|| from(0x1D434, i)),
            (_, Some(i)) => from(0x1D6E2, i),
            _ => ch,
        },
        Font::Bold => match (latin, greek) {
            (Some(i), _) => from(0x1D400, i),
            (_, Some(i)) => from(0x1D6A8, i),
            _ if ch.is_ascii_digit() => from(0x1D7CE, ch as u32 - '0' as u32),
            _ => ch,
        },
        Font::BoldItalic => match (latin, greek) {
            (Some(i), _) => from(0x1D468, i),
            (_, Some(i)) => from(0x1D71C, i),
            _ if ch.is_ascii_digit() => from(0x1D7CE, ch as u32 - '0' as u32),
            _ => ch,
        },
        Font::Script => match latin {
            Some(i) => hole(i, SCRIPT_HOLES).unwrap_or_else(|| from(0x1D49C, i)),
            None => ch,
        },
        Font::Fraktur => match latin {
            Some(i) => hole(i, FRAKTUR_HOLES).unwrap_or_else(|| from(0x1D504, i)),
            None => ch,
        },
        Font::DoubleStruck => match latin {
            Some(i) => hole(i, DOUBLE_STRUCK_HOLES).unwrap_or_else(|| from(0x1D538, i)),
            None if ch.is_ascii_digit() => from(0x1D7D8, ch as u32 - '0' as u32),
            None => ch,
        },
    }
}

/// Where a Greek letter is in the math alphabets' Greek order: the capitals with `ϴ` and
/// `∇`, then the small letters, then `∂ ϵ ϑ ϰ ϕ ϱ ϖ`.
fn greek_index(ch: char) -> Option<u32> {
    let c = ch as u32;
    Some(match ch {
        'Α'..='Ρ' => c - 0x391,
        'ϴ' => 17,
        'Σ'..='Ω' => c - 0x391,
        '∇' => 25,
        'α'..='ω' => c - 0x3B1 + 26,
        '∂' => 51,
        'ϵ' => 52,
        'ϑ' => 53,
        'ϰ' => 54,
        'ϕ' => 55,
        'ϱ' => 56,
        'ϖ' => 57,
        _ => return None,
    })
}

/// Math italic's hole: `h` is Planck's constant, `ℎ`.
fn italic(i: u32) -> Option<char> {
    (i == 26 + 7).then_some('ℎ')
}

const SCRIPT_HOLES: &[(u8, char)] = &[
    (1, 'ℬ'),
    (4, 'ℰ'),
    (5, 'ℱ'),
    (7, 'ℋ'),
    (8, 'ℐ'),
    (11, 'ℒ'),
    (12, 'ℳ'),
    (17, 'ℛ'),
    (26 + 4, 'ℯ'),
    (26 + 6, 'ℊ'),
    (26 + 14, 'ℴ'),
];
const FRAKTUR_HOLES: &[(u8, char)] = &[(2, 'ℭ'), (7, 'ℌ'), (8, 'ℑ'), (17, 'ℜ'), (25, 'ℨ')];
const DOUBLE_STRUCK_HOLES: &[(u8, char)] = &[
    (2, 'ℂ'),
    (7, 'ℍ'),
    (13, 'ℕ'),
    (15, 'ℙ'),
    (16, 'ℚ'),
    (17, 'ℝ'),
    (25, 'ℤ'),
];

fn hole(i: u32, holes: &[(u8, char)]) -> Option<char> {
    holes.iter().find(|h| u32::from(h.0) == i).map(|h| h.1)
}

/// Characters the layout uses that no name gives: the `\not` slash, primes, the radical,
/// braces, the arrows of `\xrightarrow` and the others, and punctuation of `\text`.
const LAYOUT_CHARS: &str = "\u{338}′″‴√⏞⏟⎴⎵−∗→←↔⇒⇐⇔↦↩↪⇌⇀↽–—‘’“”…";

/// Every character the embedded math font must have: the table's, the accents', the
/// layout's, and the alphabets' (ASCII, Latin-1, Greek, and their math forms). Sorted, without
/// repeats. `assets/fonts/math-codepoints.txt` lists them.
pub fn font_chars() -> Vec<char> {
    let mut v: Vec<char> = entries().map(|(_, s)| s.ch).collect();
    v.extend(LAYOUT_CHARS.chars());
    let names = [
        "hat",
        "check",
        "tilde",
        "acute",
        "grave",
        "dot",
        "ddot",
        "dddot",
        "breve",
        "bar",
        "vec",
        "mathring",
        "overrightarrow",
        "overleftarrow",
        "overleftrightarrow",
        "Overrightarrow",
    ];
    v.extend(names.iter().filter_map(|n| accent(n)).map(|a| a.0));
    let base = (' '..='~')
        .chain('\u{A0}'..='\u{FF}')
        .chain(['ı', 'ȷ'])
        .chain('Α'..='Ρ')
        .chain('Σ'..='Ω')
        .chain('α'..='ω')
        .chain(['ϴ', '∇', '∂', 'ϵ', 'ϑ', 'ϰ', 'ϕ', 'ϱ', 'ϖ']);
    use Font::*;
    for ch in base {
        for font in [
            Roman,
            Normal,
            Italic,
            Bold,
            BoldItalic,
            Script,
            Fraktur,
            DoubleStruck,
        ] {
            v.push(styled(ch, font));
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}
