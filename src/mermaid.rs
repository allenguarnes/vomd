//! Mermaid diagrams: a ```` ```mermaid ```` block shown as a diagram, as GitHub shows it.
//!
//! The diagram's SVG is made by the vendored `mermaid-svg` (its graphs laid out by `dugong`,
//! Dagre's port), in a process of its own as SVGs are drawn (`image::svg::process`): young code,
//! and a panic or a hang there must cost one diagram, not the window. Where no such process is
//! started (tests), it is made on the maker's thread.
//!
//! The document keeps a diagram as a code block whose first span names an image
//! ([`crate::doc::ImageRef`]): its `src` is a name of this module's (`vomd-mermaid:…`, one for
//! each theme: a `<picture>`'s pair, which [`crate::doc::Doc::set_dark`] switches), and its SVG,
//! once made, is kept here, in memory, where the image code reads it as it reads a file
//! ([`virtual_path`], [`bytes`], [`stamp`]). Until then the image is loading; if it can't be
//! made, the block is shown as the code it is, with a line that says why ([`note`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The largest source made a diagram (larger: its code, and a note).
pub const MAX_SOURCE: usize = 50 << 10;
/// The work one diagram's layouts may do (dugong's units): far past a few hundred nodes, well
/// short of a second.
pub const BUDGET: usize = 20_000_000;
/// How long the process that makes a diagram may take.
const DEADLINE: Duration = Duration::from_secs(3);
/// What it may hold beyond its input (`image::svg::process` adds its stack and headroom).
const MEMORY: usize = 256 << 20;
/// The SVGs kept, all told; the oldest go first past it.
/// (The tests' is smaller: a few diagrams are past it.)
const STORE_BYTES: usize = if cfg!(test) { 4 << 20 } else { 32 << 20 };
/// The largest SVG kept (a source at most [`MAX_SOURCE`] makes far less).
const MAX_SVG: usize = 8 << 20;
/// The most diagrams one document makes; those after show as code, with a note.
pub const MAX_DIAGRAMS: usize = 100;
/// The most SVG one document's diagrams keep, all told; one past it shows as code, with a note
/// (the tests' is smaller).
const PINNED_BYTES: usize = if cfg!(test) { 8 << 20 } else { 64 << 20 };
const WHY_TOGETHER: &str = "the document's diagrams are too large together";

/// Why a diagram wasn't made, for the reasons that may not hold the next time (the machine
/// was busy, a process couldn't be started): not kept past a reload or a theme switch.
const WHY_SLOW: &str = "the diagram took too long to make";
const WHY_BROKEN: &str = "the diagram couldn't be made";

const PREFIX: &str = "vomd-mermaid:";

// ---- Names ---------------------------------------------------------------------------------

/// 64-bit FNV-1a: the same in every process and build.
fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// What a diagram of `source` in the `dark` (or light) theme is kept under.
pub fn key(source: &str, dark: bool) -> u64 {
    fnv(source.bytes().chain([u8::from(dark)]))
}

/// The light and the dark theme's image names of a diagram of `source`. A source over
/// [`MAX_SOURCE`] has one name, which says so.
pub fn srcs(source: &str) -> [String; 2] {
    srcs_nth(source, 0)
}

/// [`srcs`] for the document's `nth` diagram (from 0): past [`MAX_DIAGRAMS`], one name that
/// says so.
pub fn srcs_nth(source: &str, nth: usize) -> [String; 2] {
    if nth >= MAX_DIAGRAMS {
        let many = format!("{PREFIX}many/{MAX_DIAGRAMS}");
        return [many.clone(), many];
    }
    if source.len() > MAX_SOURCE {
        let big = format!("{PREFIX}big/{}", source.len());
        return [big.clone(), big];
    }
    [false, true].map(|dark| {
        let theme = if dark { "dark" } else { "light" };
        format!("{PREFIX}{theme}/{:016x}", key(source, dark))
    })
}

enum Name {
    Diagram(u64),
    Big(usize),
    Many(usize),
}

fn name(src: &str) -> Option<Name> {
    let rest = src.strip_prefix(PREFIX)?;
    if let Some(n) = rest.strip_prefix("big/") {
        return n.parse().ok().map(Name::Big);
    }
    if let Some(n) = rest.strip_prefix("many/") {
        return n.parse().ok().map(Name::Many);
    }
    let hex = rest.strip_prefix("light/").or(rest.strip_prefix("dark/"))?;
    u64::from_str_radix(hex, 16).ok().map(Name::Diagram)
}

/// Whether `src` names a diagram.
pub fn is_diagram(src: &str) -> bool {
    name(src).is_some()
}

/// The path the image code reads a diagram's SVG at: no file's (it starts with a NUL, which
/// no file system takes).
pub fn virtual_path(src: &str) -> Option<PathBuf> {
    name(src.trim())?;
    Some(PathBuf::from(format!("\0{}", src.trim())))
}

fn path_name(path: &Path) -> Option<Name> {
    name(path.to_str()?.strip_prefix('\0')?)
}

// ---- What is kept --------------------------------------------------------------------------

enum State {
    Pending,
    /// The SVG, and the [`Metrics::key`] of the faces it was measured with: made again for
    /// other faces.
    Done(Arc<Vec<u8>>, u64),
    /// Why not, and whether the reason may not hold the next time ([`WHY_SLOW`],
    /// [`WHY_BROKEN`]).
    Failed(Arc<str>, bool),
}

#[derive(Default)]
struct Store {
    map: HashMap<u64, (State, u64)>,
    bytes: usize,
    serial: u64,
    /// The diagrams each maker's document shows ([`Maker::request`]): never evicted, and a
    /// job for one no longer shown is dropped.
    shown: HashMap<usize, HashSet<u64>>,
    /// Diagrams made but over VOMD's image limits ([`over`]): shown as their code, their SVG
    /// kept for a click that draws them anyway.
    over: HashSet<u64>,
}

impl Store {
    fn is_shown(&self, key: u64) -> bool {
        self.shown.values().any(|s| s.contains(&key))
    }

    fn remove(&mut self, key: u64) {
        self.over.remove(&key);
        if let Some((State::Done(b, _), _)) = self.map.remove(&key) {
            self.bytes -= b.len();
        }
    }

    fn put(&mut self, key: u64, state: State) {
        self.serial += 1;
        self.remove(key);
        if let State::Done(b, _) = &state {
            self.bytes += b.len();
        }
        self.map.insert(key, (state, self.serial));
        // The oldest go: never the one just put, nor one a document shows (a document's own
        // diagrams may be more than the budget: it is the page's memory, as its text is; the
        // number a document makes is capped, `MAX_DIAGRAMS`).
        while self.bytes > STORE_BYTES {
            let Some(old) = self
                .map
                .iter()
                .filter(|(k, (s, _))| {
                    **k != key && matches!(s, State::Done(..)) && !self.is_shown(**k)
                })
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| *k)
            else {
                break;
            };
            self.remove(old);
        }
        // Failures are small; keep their number bounded too (the same exceptions).
        if self.map.len() > 4096 {
            let mut olds: Vec<(u64, u64)> = self
                .map
                .iter()
                .filter(|(k, (s, _))| {
                    **k != key && !matches!(s, State::Pending) && !self.is_shown(**k)
                })
                .map(|(k, (_, at))| (*k, *at))
                .collect();
            olds.sort_by_key(|o| o.1);
            for (k, _) in olds.into_iter().take(1024) {
                self.remove(k);
            }
        }
    }
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));
/// Each maker's shared part, by its id: a result wakes every maker whose document shows it.
static MAKERS_SHARED: LazyLock<Mutex<HashMap<usize, std::sync::Weak<Shared>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn store() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The SVG made for the diagram at `path` ([`virtual_path`]), if it is made.
pub fn bytes(path: &Path) -> Option<Arc<Vec<u8>>> {
    let Name::Diagram(key) = path_name(path)? else {
        return None;
    };
    match &store().map.get(&key)?.0 {
        State::Done(b, _) => Some(b.clone()),
        _ => None,
    }
}

/// Whether `path` names a diagram ([`virtual_path`]).
pub fn is_virtual(path: &Path) -> bool {
    path_name(path).is_some()
}

/// The stamp of a made diagram's SVG: the same as long as it is kept, so a reload or a theme
/// that comes back keeps its pixels.
pub fn stamp(path: &Path) -> Option<crate::view::Stamp> {
    let b = bytes(path)?;
    let Name::Diagram(key) = path_name(path)? else {
        return None;
    };
    Some((None, key ^ b.len() as u64))
}

/// Whether the diagram `src` names is being made (asked for, and not done or failed). A name
/// nobody asked for (an image written by hand) isn't: it shows its alt text.
pub fn pending(src: &str) -> bool {
    match name(src) {
        Some(Name::Diagram(key)) => matches!(store().map.get(&key), Some((State::Pending, _))),
        _ => false,
    }
}

/// Why the diagram `src` names is shown as its code: the line under the code. `None` while it
/// is made, or once it is.
pub fn note(src: &str) -> Option<String> {
    match name(src)? {
        Name::Big(n) => Some(format!(
            "Mermaid: the diagram is too large ({} KB, at most {} KB)",
            n.div_ceil(1024),
            MAX_SOURCE >> 10
        )),
        Name::Many(n) => Some(format!(
            "Mermaid: only a document's first {n} diagrams are drawn"
        )),
        Name::Diagram(key) => {
            let s = store();
            match &s.map.get(&key)?.0 {
                State::Failed(why, _) => Some(format!("Mermaid: {why}")),
                State::Done(..) if s.over.contains(&key) => Some(OVER_NOTE.into()),
                _ => None,
            }
        }
    }
}

/// Whether the diagram `src` names is shown as its code: it can't be made, or drawn
/// ([`note`]).
pub fn shown_as_code(src: &str) -> bool {
    note(src).is_some()
}

/// The diagram `src` names was made, but can't be drawn (VOMD's image limits): it is kept as a
/// failure that says `why` (tried again on a reload or a theme switch), so it is shown, copied
/// and found as code. Returns true if it was a made diagram.
pub fn refused(src: &str, why: &str) -> bool {
    let Some(Name::Diagram(key)) = name(src) else {
        return false;
    };
    let mut s = store();
    if !matches!(s.map.get(&key), Some((State::Done(..), _))) {
        return false;
    }
    s.put(key, State::Failed(why.into(), true));
    true
}

/// The line under a diagram over VOMD's image limits ([`over`]), as an image's placeholder
/// says it.
pub const OVER_NOTE: &str =
    "Mermaid: the diagram is over VOMD's image limits · click to draw it anyway";

/// The diagram `src` names was made, but is over VOMD's image limits for its size (a refusal
/// a click overrides, as an image's): it is shown as its code with a line that offers to draw
/// it anyway ([`OVER_NOTE`]), and its SVG is kept for that. Until [`force`], a reload or a
/// theme switch. Returns true if it is now (it wasn't: a made diagram).
pub fn over(src: &str) -> bool {
    let Some(Name::Diagram(key)) = name(src) else {
        return false;
    };
    let mut s = store();
    if !matches!(s.map.get(&key), Some((State::Done(..), _))) {
        return false;
    }
    s.over.insert(key)
}

/// Whether the diagram `src` names is over VOMD's image limits ([`over`]).
pub fn is_over(src: &str) -> bool {
    match name(src) {
        Some(Name::Diagram(key)) => store().over.contains(&key),
        _ => false,
    }
}

/// The diagram `src` names is drawn past VOMD's image limits (a click on its line): it is a
/// diagram again. Returns true if it was over them.
pub fn force(src: &str) -> bool {
    match name(src) {
        Some(Name::Diagram(key)) => store().over.remove(&key),
        _ => false,
    }
}

// ---- Text widths ---------------------------------------------------------------------------

/// The advances of the face diagram text is drawn in (body text's bold face: no label is
/// wider in it than measured), in em: Latin and its punctuation; and for one diagram, those of
/// the other characters and sequences its source has (an emoji with its modifiers and joiners
/// is one advance) in the face that draws them ([`scan`]). Others are estimated.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    pub adv: Vec<f32>,
    pub fallback: f32,
    /// A diagram's sequences and their advances, sorted, at most [`MAX_SEQ`].
    pub seq: Vec<(String, f32)>,
}

/// A diagram's measured sequences, at most; and the bytes of one, at most.
const MAX_SEQ: usize = 4096;
const MAX_SEQ_BYTES: usize = 64;

/// The characters [`Metrics::adv`] has: U+0000 to U+024F, then U+2000 to U+206F.
const LATIN: u32 = 0x250;
const PUNCT: std::ops::Range<u32> = 0x2000..0x2070;

impl Default for Metrics {
    /// The crate's own estimate (7.5 px a character at 14 px).
    fn default() -> Metrics {
        Metrics {
            adv: Vec::new(),
            fallback: 7.5 / 14.0,
            seq: Vec::new(),
        }
    }
}

impl Metrics {
    /// The bold body face's advances.
    pub fn of(fonts: &crate::fonts::Fonts) -> Metrics {
        let face = fonts.face(fonts.slot(crate::fonts::Slot::Bold));
        let font = face.font();
        let upem = f32::from(font.metrics(&[]).units_per_em).max(1.0);
        let gm = font.glyph_metrics(&[]);
        let adv_of = |c: u32| {
            let g = char::from_u32(c).map_or(0, |c| face.glyph(c));
            (g != 0).then(|| gm.advance_width(g) / upem)
        };
        let fallback = adv_of('n' as u32).map_or(0.6, |n| n * 1.15);
        let adv = (0..LATIN)
            .chain(PUNCT)
            .map(|c| adv_of(c).unwrap_or(fallback))
            .collect();
        Metrics {
            adv,
            fallback,
            seq: Vec::new(),
        }
    }

    /// Whether [`Metrics::adv`] has `c`'s advance.
    fn in_table(c: char) -> bool {
        (c as u32) < LATIN || PUNCT.contains(&(c as u32))
    }

    fn advance(&self, c: char) -> f32 {
        let u = c as u32;
        let i = if u < LATIN {
            Some(u as usize)
        } else if PUNCT.contains(&u) {
            Some((LATIN + u - PUNCT.start) as usize)
        } else {
            None
        };
        if let Some(a) = i.and_then(|i| self.adv.get(i)) {
            return *a;
        }
        let wide = matches!(u,
            0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD);
        if wide { 1.0 } else { self.fallback }
    }

    /// What tells these metrics from others (a diagram made with other metrics is made again).
    pub fn key(&self) -> u64 {
        let seq = self.seq.iter().flat_map(|(s, a)| {
            (s.len() as u32)
                .to_le_bytes()
                .into_iter()
                .chain(s.bytes())
                .chain(a.to_bits().to_le_bytes())
        });
        fnv(self
            .adv
            .iter()
            .chain([&self.fallback])
            .flat_map(|a| a.to_bits().to_le_bytes())
            .chain(seq))
    }

    /// The width of `s` in em: at each place the longest sequence measured that starts there,
    /// or the character's advance.
    pub fn measure(&self, s: &str) -> f64 {
        if self.seq.is_empty() {
            return s.chars().map(|c| f64::from(self.advance(c))).sum();
        }
        let mut width = 0.0;
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            let (len, advance) = self
                .longest(rest, c)
                .unwrap_or((c.len_utf8(), self.advance(c)));
            width += f64::from(advance);
            rest = &rest[len..];
        }
        width
    }

    /// The longest sequence measured that `rest` (its first character `c`) starts with: its
    /// length and advance.
    fn longest(&self, rest: &str, c: char) -> Option<(usize, f32)> {
        let first = &rest[..c.len_utf8()];
        let from = self.seq.partition_point(|(k, _)| k.as_str() < first);
        self.seq[from..]
            .iter()
            .take_while(|(k, _)| k.starts_with(first))
            .filter(|(k, _)| rest.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(k, a)| (k.len(), *a))
    }

    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.adv.len() as u32).to_le_bytes());
        for a in &self.adv {
            out.extend_from_slice(&a.to_le_bytes());
        }
        out.extend_from_slice(&self.fallback.to_le_bytes());
        let seq = &self.seq[..self.seq.len().min(MAX_SEQ)];
        out.extend_from_slice(&(seq.len() as u32).to_le_bytes());
        for (s, a) in seq {
            out.push(s.len().min(MAX_SEQ_BYTES) as u8);
            out.extend_from_slice(&s.as_bytes()[..s.len().min(MAX_SEQ_BYTES)]);
            out.extend_from_slice(&a.to_le_bytes());
        }
    }

    fn get(r: &mut &[u8]) -> Option<Metrics> {
        let n = u32::from_le_bytes(take(r)?) as usize;
        if n > 4096 {
            return None;
        }
        let mut adv = Vec::with_capacity(n);
        for _ in 0..n {
            adv.push(sane(f32::from_le_bytes(take(r)?)));
        }
        let fallback = sane(f32::from_le_bytes(take(r)?));
        let n = u32::from_le_bytes(take(r)?) as usize;
        if n > MAX_SEQ {
            return None;
        }
        let mut seq: Vec<(String, f32)> = Vec::with_capacity(n);
        for _ in 0..n {
            let len = usize::from(take::<1>(r)?[0]);
            if len == 0 || len > MAX_SEQ_BYTES || r.len() < len {
                return None;
            }
            let (s, rest) = r.split_at(len);
            *r = rest;
            let s = std::str::from_utf8(s).ok()?.to_owned();
            seq.push((s, sane(f32::from_le_bytes(take(r)?))));
        }
        // (Sorted, for the search: a job that isn't is refused.)
        if !seq.is_sorted_by(|a, b| a.0 < b.0) {
            return None;
        }
        Some(Metrics { adv, fallback, seq })
    }
}

/// What the text of a diagram's `source` has outside the table of [`Metrics::adv`], measured in
/// `fonts`: each sequence (a character with what attaches to it: marks, variation selectors,
/// skin tones, joiners and the emoji after them, a flag's second letter) and its advance in em,
/// shaped as body text's bold is, in the faces it would be drawn in (sorted, at most
/// [`MAX_SEQ`]); and the characters that no face has, for a fallback scan (their sequences are
/// estimated).
pub fn scan(source: &str, fonts: &crate::fonts::Fonts) -> (Vec<(String, f32)>, Vec<char>) {
    use crate::text::{Para, TextCtx, TextStyle, attaches_to_previous};
    /// The size sequences are shaped at, in pixels.
    const SIZE: f32 = 64.0;
    if source.is_ascii() {
        return (Vec::new(), Vec::new());
    }
    let ri = |c: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
    let style = TextStyle {
        bold: true,
        italic: false,
        mono: false,
        size: SIZE,
        role: crate::theme::Role::Text,
        deco: 0,
        shift: 0.0,
        link: 0,
    };
    // (Kept sorted as it is made: a sequence is measured once, and found by a binary search.)
    let mut seq: Vec<(String, f32)> = Vec::new();
    let mut missing: Vec<char> = Vec::new();
    let mut ctx = TextCtx::default();
    let mut para = Para::default();
    let mut chars = source.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let mut end = at + c.len_utf8();
        let mut last = c;
        while let Some(&(i, n)) = chars.peek() {
            let joined = last == '\u{200D}' && !n.is_whitespace() && !n.is_control();
            let flag = ri(c) && ri(n) && i == at + c.len_utf8();
            if !(attaches_to_previous(n) || joined || flag) {
                break;
            }
            end = i + n.len_utf8();
            last = n;
            chars.next();
        }
        let s = &source[at..end];
        if s.chars().all(Metrics::in_table) || s.len() > MAX_SEQ_BYTES || seq.len() >= MAX_SEQ {
            continue;
        }
        let Err(place) = seq.binary_search_by(|(k, _)| k.as_str().cmp(s)) else {
            continue;
        };
        para.clear();
        para.push_text(s, 0, style);
        let shaped = ctx.shape(fonts, &para);
        if shaped.missing.is_empty() {
            let advance: f32 = shaped.clusters.iter().map(|c| c.advance).sum();
            seq.insert(place, (s.to_owned(), sane(advance / SIZE)));
        } else {
            missing.extend(shaped.missing);
        }
    }
    missing.sort_unstable();
    missing.dedup();
    (seq, missing)
}

fn sane(a: f32) -> f32 {
    if a.is_finite() {
        a.clamp(0.0, 4.0)
    } else {
        0.6
    }
}

fn take<const N: usize>(r: &mut &[u8]) -> Option<[u8; N]> {
    let (head, rest) = r.split_first_chunk::<N>()?;
    *r = rest;
    Some(*head)
}

// ---- Making one ----------------------------------------------------------------------------

/// The SVG of a diagram of `source`, in the `dark` (or light) theme unless it names its own,
/// its text measured by `metrics`; or why not, as the note says it. Bounded: the source is at
/// most [`MAX_SOURCE`], and the layouts' work [`BUDGET`].
pub fn make(source: &str, dark: bool, metrics: &Metrics) -> Result<String, String> {
    if source.len() > MAX_SOURCE {
        return Err("the diagram is too large".into());
    }
    let (d, meta) = mermaid_svg::parse_with_meta(source).map_err(|e| match e {
        mermaid_svg::ParseError::Syntax { line, message, .. } => {
            format!("line {line}: {message}")
        }
        mermaid_svg::ParseError::UnknownDiagramType(t) => {
            let t: String = t.chars().take(40).collect();
            format!("unknown diagram type “{t}”")
        }
        // The diagram types mermaid-svg leaves out (see its VOMD-PATCH.md).
        mermaid_svg::ParseError::Unsupported(kind) => format!("{kind} diagrams aren't supported"),
        _ => "the diagram is empty".into(),
    })?;
    let m = metrics.clone();
    mermaid_svg::vomd::set_measure(Some(Box::new(move |s| m.measure(s))));
    let svg = mermaid_svg::vomd::render(&d, &meta, dark, BUDGET);
    mermaid_svg::vomd::set_measure(None);
    svg.map_err(|_| "the diagram is too complex".into())
}

/// [`make`] where a panic is a failure (tests and builds without a drawing process; a release
/// build aborts on a panic, but makes diagrams apart).
fn make_here(source: &str, dark: bool, metrics: &Metrics) -> Result<String, String> {
    guarded(|| make(source, dark, metrics))
}

/// `f`, a panic in it a failure.
fn guarded(
    f: impl FnOnce() -> Result<String, String> + std::panic::UnwindSafe,
) -> Result<String, String> {
    std::panic::catch_unwind(f).unwrap_or_else(|_| Err(WHY_BROKEN.into()))
}

/// What a making process does instead of its job, for tests: spin forever, allocate until its
/// limit stops it (`image::svg::process`'s modes), or panic.
const TEST_SPIN: u8 = 1;
const TEST_EAT: u8 = 2;
#[doc(hidden)]
pub const TEST_PANIC: u8 = 3;

/// The job a making process reads: `VMM2`, a test mode, the theme, the metrics (with the
/// diagram's sequences), the source.
fn put_job(source: &str, dark: bool, metrics: &Metrics, test: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(source.len() + 4096);
    out.extend_from_slice(b"VMM2");
    out.push(test);
    out.push(u8::from(dark));
    metrics.put(&mut out);
    out.extend_from_slice(&(source.len() as u64).to_le_bytes());
    out.extend_from_slice(source.as_bytes());
    out
}

fn get_job(mut r: &[u8]) -> Option<(String, bool, Metrics, u8)> {
    if take::<4>(&mut r)? != *b"VMM2" {
        return None;
    }
    let test = take::<1>(&mut r)?[0];
    let dark = take::<1>(&mut r)?[0] != 0;
    let metrics = Metrics::get(&mut r)?;
    let n = u64::from_le_bytes(take(&mut r)?) as usize;
    if n > MAX_SOURCE || r.len() != n {
        return None;
    }
    Some((String::from_utf8(r.to_vec()).ok()?, dark, metrics, test))
}

/// A job's bytes besides its source, at most: the table, and the sequences.
const MAX_JOB_EXTRA: usize = (64 << 10) + MAX_SEQ * (MAX_SEQ_BYTES + 5);

/// The result a making process writes: 0 and the SVG, or 1 and why not.
fn put_result(r: &Result<String, String>) -> Vec<u8> {
    let (kind, text) = match r {
        Ok(svg) => (0u8, svg),
        Err(why) => (1u8, why),
    };
    let mut out = Vec::with_capacity(text.len() + 9);
    out.push(kind);
    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

fn get_result(mut r: &[u8]) -> Option<Result<String, String>> {
    let kind = take::<1>(&mut r)?[0];
    let n = u64::from_le_bytes(take(&mut r)?) as usize;
    if r.len() != n || n > MAX_SVG {
        return None;
    }
    let text = String::from_utf8(r.to_vec()).ok()?;
    match kind {
        0 => Some(Ok(text)),
        1 => Some(Err(text)),
        _ => None,
    }
}

/// The making process (`image::svg::process`, [`crate::image::MERMAID_ARG`]): read the job on
/// stdin, write the result on stdout. The exit code (`bad` for a job it can't read).
pub fn child_job(bad: i32) -> i32 {
    use std::io::{Read, Write};
    // (A test's: wait before reading the job, for the parent's deadline to hold meanwhile.)
    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    if std::env::var_os(crate::image::MERMAID_STALL).is_some() {
        std::thread::sleep(Duration::from_secs(2));
    }
    let mut input = Vec::new();
    if std::io::stdin()
        .lock()
        .take((MAX_SOURCE + MAX_JOB_EXTRA) as u64)
        .read_to_end(&mut input)
        .is_err()
    {
        return bad;
    }
    let Some((source, dark, metrics, test)) = get_job(&input) else {
        return bad;
    };
    drop(input);
    match test {
        TEST_SPIN => loop {
            std::hint::black_box(());
        },
        TEST_EAT => {
            let mut held: Vec<Vec<u8>> = Vec::new();
            loop {
                held.push(vec![1; 1 << 20]);
                std::hint::black_box(&held);
            }
        }
        TEST_PANIC => panic!("a test's panic"),
        _ => {}
    }
    let result = make(&source, dark, &metrics);
    let mut out = std::io::stdout().lock();
    match out
        .write_all(&put_result(&result))
        .and_then(|()| out.flush())
    {
        Ok(()) => 0,
        Err(_) => bad,
    }
}

/// [`make`] in a process of its own if VOMD draws SVGs apart, else here.
fn make_apart(
    source: &str,
    dark: bool,
    metrics: &Metrics,
    cancel: &AtomicBool,
) -> Result<String, String> {
    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    if let Some(program) = crate::image::svg_program() {
        use crate::image::SvgRefusal;
        let job = put_job(source, dark, metrics, crate::image::test_mode());
        let deadline = Instant::now() + deadline();
        return match crate::image::run_apart(
            program,
            crate::image::MERMAID_ARG,
            &job,
            MEMORY,
            MAX_SVG + 9,
            deadline,
            cancel,
        ) {
            Ok(out) => get_result(&out).unwrap_or_else(|| Err(WHY_BROKEN.into())),
            Err(None) => Err("cancelled".into()),
            Err(Some(SvgRefusal::Slow)) => Err(WHY_SLOW.into()),
            Err(Some(SvgRefusal::Memory)) => Err("the diagram needs too much memory".into()),
            Err(Some(SvgRefusal::Complex)) => Err("the diagram is too large".into()),
            Err(Some(_)) => Err(WHY_BROKEN.into()),
        };
    }
    let _ = cancel;
    // A panic aborts a release build: it never makes a diagram in the window's own process.
    if !cfg!(panic = "unwind") {
        return Err(WHY_BROKEN.into());
    }
    make_here(source, dark, metrics)
}

/// [`make`] as the maker makes it: in a process of its own if VOMD draws SVGs apart.
#[doc(hidden)]
pub fn make_as_maker(source: &str, dark: bool, metrics: &Metrics) -> Result<String, String> {
    make_apart(source, dark, metrics, &AtomicBool::new(false))
}

/// The deadline of a making process ([`DEADLINE`]; the tests' may be shorter).
fn deadline() -> Duration {
    match TEST_DEADLINE_MS.load(Ordering::Relaxed) {
        0 => DEADLINE,
        ms => Duration::from_millis(ms),
    }
}

static TEST_DEADLINE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Give making processes `ms` milliseconds instead of [`DEADLINE`] (0: that), for tests.
#[doc(hidden)]
pub fn set_test_deadline(ms: u64) {
    TEST_DEADLINE_MS.store(ms, Ordering::Relaxed);
}

/// Forget every diagram made, for tests.
#[doc(hidden)]
pub fn forget_all() {
    let mut s = store();
    s.map.clear();
    s.bytes = 0;
}

/// Forget the failures whose reason may not hold any more ([`WHY_SLOW`], [`WHY_BROKEN`]): the
/// document was opened again, or the theme switched.
pub fn forget_transient() {
    let mut s = store();
    s.map
        .retain(|_, (st, _)| !matches!(st, State::Failed(_, true)));
    s.over.clear();
}

// ---- The maker -----------------------------------------------------------------------------

struct Job {
    key: u64,
    source: Arc<str>,
    dark: bool,
    /// The diagram's metrics: the base's and its own sequences.
    metrics: Arc<Metrics>,
    /// The metrics of the faces it was asked for with ([`Shared::metrics`]).
    base: Arc<Metrics>,
}

/// What a maker shares with its thread.
struct Shared {
    /// The wake the thread calls when it has a result: the one set then.
    wake: Mutex<crate::image::Wake>,
    /// The metrics diagrams are made with now: a result made with others is dropped.
    metrics: Mutex<Arc<Metrics>>,
    /// A diagram the maker's document shows was made (or failed) since
    /// [`Maker::take_finished`]: the maker's own, so one window (or test) never takes
    /// another's.
    finished: AtomicBool,
}

impl Shared {
    /// A result is in: say so, and wake the window.
    fn notify(&self) {
        self.finished.store(true, Ordering::SeqCst);
        let wake = lock(&self.wake).clone();
        wake();
    }
}

/// Makes the diagrams a document shows, one at a time, on a thread of its own; each one made
/// is kept ([`bytes`]) and the window woken ([`Maker::take_finished`] then says so).
pub struct Maker {
    id: usize,
    tx: Option<Sender<Job>>,
    shared: Arc<Shared>,
    /// The metrics of each diagram the document shows ([`scan`]), and the faces' generation
    /// they were worked out at ([`crate::fonts::Fonts::generation`]).
    scans: HashMap<u64, (u64, Arc<Metrics>)>,
}

/// Jobs whose source has one of these are held back (tests: each holds its own).
#[cfg(test)]
static HELD: Mutex<Vec<String>> = Mutex::new(Vec::new());

static MAKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Default for Maker {
    fn default() -> Maker {
        let id = MAKERS.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(Shared {
            wake: Mutex::new(Arc::new(|| {})),
            metrics: Mutex::new(Arc::new(Metrics::default())),
            finished: AtomicBool::new(false),
        });
        lock(&MAKERS_SHARED).insert(id, Arc::downgrade(&shared));
        Maker {
            id,
            tx: None,
            shared,
            scans: HashMap::new(),
        }
    }
}

impl Drop for Maker {
    fn drop(&mut self) {
        store().shown.remove(&self.id);
        lock(&MAKERS_SHARED).remove(&self.id);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Maker {
    /// How the maker wakes the window. A result that came before it was set wakes it now.
    pub fn set_wake(&mut self, wake: crate::image::Wake) {
        *lock(&self.shared.wake) = wake.clone();
        if self.shared.finished.load(Ordering::SeqCst) {
            wake();
        }
    }

    /// Whether a diagram this maker's document shows was made (or failed) since this was last
    /// called.
    pub fn take_finished(&self) -> bool {
        self.shared.finished.swap(false, Ordering::SeqCst)
    }

    /// The faces text is drawn in changed: diagrams made from now on are measured with them,
    /// and those being made with the old ones are dropped when they are done. Returns true if
    /// the metrics changed (the diagrams shown should be made again: [`Maker::forget`]).
    pub fn set_metrics(&mut self, metrics: Metrics) -> bool {
        let mut m = lock(&self.shared.metrics);
        if **m == metrics {
            return false;
        }
        *m = Arc::new(metrics);
        self.scans.clear();
        true
    }

    /// Make the diagrams of `doc` that it shows now (each image's current `src`) and that
    /// aren't made or being made, in the document's order. These are kept while the document
    /// shows them; jobs still waiting for diagrams it doesn't show any more are dropped.
    /// Their text is measured with the metrics alone (see [`Maker::request_with`]).
    pub fn request(&mut self, doc: &crate::doc::Doc) {
        self.request_in(doc, None);
    }

    /// [`Maker::request`], each diagram's text outside the metrics' table measured in `fonts`
    /// ([`scan`]): a diagram made with other widths (a fallback added since has some of its
    /// characters) is made again. The characters of the diagrams' text that no face has (for
    /// a fallback scan), and whether a diagram made is made again.
    pub fn request_with(
        &mut self,
        doc: &crate::doc::Doc,
        fonts: &crate::fonts::Fonts,
    ) -> (Vec<char>, bool) {
        self.request_in(doc, Some(fonts))
    }

    fn request_in(
        &mut self,
        doc: &crate::doc::Doc,
        fonts: Option<&crate::fonts::Fonts>,
    ) -> (Vec<char>, bool) {
        let mut wanted: Vec<(u64, &Arc<str>, bool)> = Vec::new();
        let shown: HashSet<u64> = doc
            .images
            .iter()
            .filter_map(|img| {
                let source = img.diagram.as_ref()?;
                let Some(Name::Diagram(key)) = name(&img.src) else {
                    return None;
                };
                let dark = img.src.starts_with(&format!("{PREFIX}dark/"));
                wanted.push((key, source, dark));
                Some(key)
            })
            .collect();
        let base = lock(&self.shared.metrics).clone();
        let mut missing: Vec<char> = Vec::new();
        let mut remade = false;
        // Each diagram's metrics: worked out once for the faces there are.
        self.scans.retain(|k, _| shown.contains(k));
        let mut metrics_of = |key: u64, source: &str| -> Arc<Metrics> {
            let Some(fonts) = fonts else {
                return base.clone();
            };
            let generation = fonts.generation();
            if let Some((g, m)) = self.scans.get(&key)
                && *g == generation
            {
                return m.clone();
            }
            let (seq, uncovered) = scan(source, fonts);
            missing.extend(uncovered);
            let m = if seq.is_empty() {
                base.clone()
            } else {
                Arc::new(Metrics {
                    seq,
                    ..(*base).clone()
                })
            };
            self.scans.insert(key, (generation, m.clone()));
            m
        };
        let mut jobs = Vec::new();
        {
            let mut s = store();
            s.shown.insert(self.id, shown);
            for (key, source, dark) in wanted {
                let metrics = metrics_of(key, source);
                // Made with other faces (before a font change, for another document, before a
                // fallback that has some of its characters): made again.
                if let Some((State::Done(_, made_with), _)) = s.map.get(&key)
                    && *made_with != metrics.key()
                {
                    s.remove(key);
                    remade = true;
                }
                if s.map.contains_key(&key) {
                    continue;
                }
                s.put(key, State::Pending);
                jobs.push(Job {
                    key,
                    source: source.clone(),
                    dark,
                    metrics,
                    base: base.clone(),
                });
            }
        }
        for job in jobs {
            let tx = self
                .tx
                .get_or_insert_with(|| spawn(self.id, self.shared.clone()));
            if let Err(e) = tx.send(job) {
                // The thread is gone: another takes the job.
                let tx = spawn(self.id, self.shared.clone());
                let _ = tx.send(e.0);
                self.tx = Some(tx);
            }
        }
        missing.sort_unstable();
        missing.dedup();
        (missing, remade)
    }

    /// Forget the diagrams `doc` shows, made or being made, so the next [`Maker::request`]
    /// makes them again (the faces changed).
    pub fn forget(&mut self, doc: &crate::doc::Doc) {
        let mut s = store();
        for img in &doc.images {
            for src in std::iter::once(&img.src).chain(img.theme.iter().flat_map(|t| t.iter())) {
                if let Some(Name::Diagram(key)) = name(src) {
                    s.remove(key);
                }
            }
        }
    }
}

/// What maker `id`'s result for `key` does to the store. Made with the current metrics, it is
/// kept (unless the document's diagrams together would pass [`PINNED_BYTES`]: then it shows as
/// code); made with old ones, it is dropped, and so is the entry that said it was being made,
/// so the next request makes it again.
fn finish(
    s: &mut Store,
    id: usize,
    key: u64,
    (current, metrics): (bool, u64),
    result: Result<String, String>,
) {
    if !current {
        if matches!(s.map.get(&key), Some((State::Pending, _))) {
            s.remove(key);
        }
        return;
    }
    let state = match result {
        Ok(svg) if svg.len() <= MAX_SVG => {
            let others: usize = s.shown.get(&id).map_or(0, |keys| {
                keys.iter()
                    .filter(|&&k| k != key)
                    .filter_map(|k| match s.map.get(k) {
                        Some((State::Done(b, _), _)) => Some(b.len()),
                        _ => None,
                    })
                    .sum()
            });
            if others + svg.len() > PINNED_BYTES {
                State::Failed(WHY_TOGETHER.into(), true)
            } else {
                State::Done(Arc::new(svg.into_bytes()), metrics)
            }
        }
        Ok(_) => State::Failed("the diagram is too large".into(), false),
        Err(why) => {
            let transient = why == WHY_SLOW || why == WHY_BROKEN;
            State::Failed(why.into(), transient)
        }
    };
    s.put(key, state);
}

fn spawn(id: usize, shared: Arc<Shared>) -> Sender<Job> {
    let (tx, rx): (Sender<Job>, Receiver<Job>) = channel();
    let _ = std::thread::Builder::new()
        .name("vomd-mermaid".into())
        .stack_size(8 << 20)
        .spawn(move || {
            let never = AtomicBool::new(false);
            while let Ok(job) = rx.recv() {
                // Not shown any more (another document, the other theme): dropped.
                {
                    let mut s = store();
                    if !s.shown.get(&id).is_some_and(|k| k.contains(&job.key)) {
                        if matches!(s.map.get(&job.key), Some((State::Pending, _))) {
                            s.remove(job.key);
                        }
                        continue;
                    }
                }
                // (A test holds the jobs of the sources it names until it lets them go.)
                #[cfg(test)]
                while lock(&HELD).iter().any(|h| job.source.contains(h.as_str())) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let result = make_apart(&job.source, job.dark, &job.metrics, &never);
                let current = Arc::ptr_eq(&job.base, &lock(&shared.metrics));
                finish(
                    &mut store(),
                    id,
                    job.key,
                    (current, job.metrics.key()),
                    result,
                );
                // This maker, and any other whose document shows the diagram (it made none of its
                // own: the store had it pending).
                let others: Vec<usize> = store()
                    .shown
                    .iter()
                    .filter(|(m, keys)| **m != id && keys.contains(&job.key))
                    .map(|(&m, _)| m)
                    .collect();
                shared.notify();
                let makers: Vec<Arc<Shared>> = {
                    let all = lock(&MAKERS_SHARED);
                    others
                        .iter()
                        .filter_map(|m| all.get(m)?.upgrade())
                        .collect()
                };
                for m in makers {
                    m.notify();
                }
            }
        });
    tx
}

#[cfg(test)]
mod tests;
