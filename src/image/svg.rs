//! SVG and svgz, drawn by resvg (vendored, see `vendor/resvg/VOMD-PATCH.md`) straight to the
//! box, so they are sharp at any zoom.
//!
//! The header ([`header`]) reads at most [`SNIFF`] bytes for the root tag's size, as usvg works it
//! out. A job then goes: the bytes counted without allocating ([`scan`]: elements, attributes,
//! path numbers, embedded data, entity expansion, depth), which says what parsing holds before
//! anything is parsed; `roxmltree` with VOMD's options; the style sheet matched to its elements as
//! usvg does it ([`css_costs`]), as that can't be stopped; a pass over its tree that counts what
//! `<use>` and markers make, and the work of shaping its text ([`instances`]); usvg's tree, with resolvers that read no file and fetch nothing;
//! a walk of the tree for the pixmaps rendering holds at once ([`Analysis`]); and the render. A
//! job charged less than what it finds it needs renders nothing and says what it needs
//! ([`Rendered::Need`]): the main thread records it, and asks again with that charge.
//!
//! In VOMD all of that runs in a process of its own ([`draw`], `process`), killed when it takes
//! too long or isn't wanted: the counts keep the work and memory of a job within what it is
//! charged, and the process makes sure that nothing they miss can hang VOMD.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use resvg::{tiny_skia, usvg};
use usvg::roxmltree;

use super::{Bitmap, Format, Header, SvgCost, cancelled};
use crate::fonts::SvgFonts;

/// Bytes of XML an SVG may have (after an svgz is inflated), at most.
pub(super) const MAX_SVG_XML: usize = 4 << 20;
/// Bytes of an SVG's start the header looks at for its root tag, at most.
const SNIFF: usize = 64 << 10;
/// Bytes of an SVG's start the root tag must start within.
const ROOT_WITHIN: usize = 4 << 10;
/// Elements `roxmltree` makes, at most.
const MAX_NODES: u32 = 100_000;
/// Elements deep, at most: usvg and resvg recurse once a level.
const MAX_DEPTH: usize = 256;
/// Bytes of text entity references may expand to, at most.
const MAX_EXPANDED: usize = 1 << 20;
/// Entities an internal subset may declare, at most.
const MAX_ENTITIES: usize = 1024;
/// SVGs inside SVGs (`data:image/svg+xml`), at most this deep.
const MAX_NESTED: usize = 2;
/// What usvg is given for the root's `em` and for text without a size.
const FONT_SIZE: f32 = 16.0;
/// usvg's size for an SVG without one.
const DEFAULT_SIDE: f32 = 100.0;

/// What parsing holds however small the SVG: usvg's and roxmltree's own structures, the
/// resolvers, and the scan's tables.
const PARSE_FIXED: usize = 256 << 10;
/// What parsing holds per element (roxmltree's node, usvg's node and its attributes, transform
/// and bounding boxes), per attribute, and per number of path data, by counting.
const ELEMENT_COST: usize = 1024;
const ATTRIBUTE_COST: usize = 160;
const NUMBER_COST: usize = 20;
/// What parsing holds per byte of text outside tags (each character's glyph outline, a path, when
/// text is drawn: about 580 bytes for Inter, by counting), and per byte entities expand to (which
/// may be markup: as for the densest SVGs measured).
const TEXT_COST: usize = 768;
const EXPANDED_COST: usize = 24;
/// The caps a load forced past the limits ([`Caps::FORCED`]) has instead.
const FORCED_XML: usize = 64 << 20;
const FORCED_NODES: u32 = 1_000_000;
const FORCED_DEPTH: usize = 1024;
const FORCED_EXPANDED: usize = 64 << 20;
const FORCED_ENTITIES: usize = 64 << 10;
/// What usvg's work in a style sheet and in text may come to, at most, which `from_xmltree` can't
/// be stopped in: counted beforehand, in steps of matching a selector ([`css_costs`]) and in units
/// of shaping a byte of text ([`Chunks`]). About a second a cap for an image of the cache; a
/// forced load, which only the next forced loads wait for, takes up to five.
const MAX_CSS_STEPS: usize = 40_000_000;
const MAX_SHAPING: usize = 2_000_000;
const FORCED_CSS_STEPS: usize = 200_000_000;
const FORCED_SHAPING: usize = 10_000_000;
/// What parsing holds for a style sheet: per rule (its place and share of its declarations), per
/// byte (the parts of its selectors), and per declaration.
const RULE_COST: usize = 64;
const SHEET_BYTE_COST: usize = 32;
const DECLARATION_COST: usize = 64;

/// What an SVG may make: the caps of the images the cache holds, or of a forced load (whose
/// memory is its ceiling and whose time has no cap, as it can be cancelled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Caps {
    pub xml: usize,
    nodes: u32,
    depth: usize,
    expanded: usize,
    entities: usize,
    rasters: usize,
    /// Steps of matching selectors, and units of shaping text, at most (see [`MAX_CSS_STEPS`]).
    css: usize,
    shaping: usize,
    /// The work of drawing it is capped ([`MAX_WORK`]).
    work: bool,
    /// Its embedded raster images are decoded within these limits.
    limits: super::Limits,
}

impl Caps {
    pub const NORMAL: Caps = Caps {
        xml: MAX_SVG_XML,
        nodes: MAX_NODES,
        depth: MAX_DEPTH,
        expanded: MAX_EXPANDED,
        entities: MAX_ENTITIES,
        rasters: MAX_RASTERS,
        css: MAX_CSS_STEPS,
        shaping: MAX_SHAPING,
        work: true,
        limits: super::Limits::normal(),
    };

    /// The caps of `limits`.
    pub fn of(limits: &super::Limits) -> Caps {
        if !limits.forced {
            return Caps::NORMAL;
        }
        Caps {
            xml: FORCED_XML,
            nodes: FORCED_NODES,
            depth: FORCED_DEPTH,
            expanded: FORCED_EXPANDED,
            entities: FORCED_ENTITIES,
            rasters: usize::MAX,
            css: FORCED_CSS_STEPS,
            shaping: FORCED_SHAPING,
            work: false,
            limits: *limits,
        }
    }
}

/// What the first job of an SVG is charged per byte of its XML for parsing, before anything is
/// counted, at most [`GUESS_MAX`]: what the densest small files measured (text and `style`
/// attributes, which cost the most for their bytes: badges, logos), so that they are drawn by
/// the first job. Denser files, and the large ones that the cap is under, are found out and
/// asked for again.
const GUESS_PER_BYTE: usize = 64;
const GUESS_MAX: usize = 16 << 20;
/// The layers the first job of an SVG is charged, in canvases (see [`SvgCost::layers`]), and the
/// pixmaps they are, and what drawing its largest path holds: what badges and logos measured.
const GUESS_LAYERS: u32 = 2 * LAYER_UNITS;
const GUESS_PIXMAPS: u32 = 4;
const GUESS_DRAW: usize = 64 << 10;
/// What drawing a path holds for each point of it while tiny-skia fills it (its copy in the
/// canvas's space, and its edges, in lists that double as they grow), and while it strokes it
/// (the outline it makes of it, and that outline's).
const FILL_PER_POINT: usize = 128;
const STROKE_PER_POINT: usize = 512;
/// [`SvgCost::layers`] is in this many parts of a canvas.
pub(super) const LAYER_UNITS: u32 = 16;

// ---- The header ---------------------------------------------------------------------------

/// The [`Header`] of an SVG (or svgz: gzip whose start is an SVG), `start` its first two bytes and
/// `r` just past them: the size usvg gives it, rounded, at least 1 and with no side over
/// [`MAX_SIDE`](super::MAX_SIDE) (scaled down, as it's drawn at any size), and its bytes of XML.
/// `None` for anything else, an SVG usvg has no size for, or one of more XML than a forced load
/// reads ([`FORCED_XML`]; over [`MAX_SVG_XML`] it is refused for VOMD's limits, see
/// `image::refusal`).
pub(super) fn header(start: [u8; 2], r: &mut (impl Read + Seek)) -> Option<Header> {
    let mut buf = [0u8; SNIFF];
    let (head, xml, gz) = if start == [0x1F, 0x8B] {
        // The gzip trailer's last 4 bytes: the inflated size (modulo 2³²).
        let end = r.seek(SeekFrom::End(0)).ok()?;
        r.seek(SeekFrom::End(-4)).ok()?;
        let mut isize = [0u8; 4];
        r.read_exact(&mut isize).ok()?;
        let xml = u32::from_le_bytes(isize) as usize;
        if end < 18 {
            return None;
        }
        r.seek(SeekFrom::Start(0)).ok()?;
        let mut z = flate2::read::GzDecoder::new(r.take(end));
        let n = read_up_to(&mut z, &mut buf);
        (&buf[..n], xml, true)
    } else {
        buf[..2].copy_from_slice(&start);
        let n = 2 + read_up_to(r, &mut buf[2..]);
        let xml = r.seek(SeekFrom::End(0)).ok()? as usize;
        (&buf[..n], xml, false)
    };
    if xml > FORCED_XML {
        return None;
    }
    let (w, h) = natural(root_tag(head)?)?;
    Some(Header {
        format: Format::Svg,
        w,
        h,
        interlaced: false,
        row: 0,
        out_channels: 0,
        coefficients: 0,
        mcu_rows: 0,
        held: 0,
        orientation: 1,
        svg: SvgCost {
            xml: xml as u32,
            gz,
            ..SvgCost::default()
        },
    })
}

/// Reads into `buf` until it is full or `r` ends (or fails): how many bytes.
fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> usize {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    n
}

/// The attributes of the root tag of `head`, if it is an SVG's: within its first
/// [`ROOT_WITHIN`] bytes, after a byte order mark, white space, an XML declaration, comments,
/// processing instructions and a doctype (its internal subset too), the first tag is `<svg`.
fn root_tag(head: &[u8]) -> Option<&[u8]> {
    let mut i = if head.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    loop {
        while head.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if i >= ROOT_WITHIN {
            return None;
        }
        let rest = &head[i..];
        if rest.starts_with(b"<?") {
            i += find(rest, b"?>")? + 2;
        } else if rest.starts_with(b"<!--") {
            i += find(rest, b"-->")? + 3;
        } else if rest.starts_with(b"<!DOCTYPE") {
            i += doctype_end(rest)?;
        } else if rest.starts_with(b"<svg")
            && rest
                .get(4)
                .is_some_and(|&c| c.is_ascii_whitespace() || c == b'>' || c == b'/')
        {
            let end = tag_end(&rest[4..])?;
            return Some(&rest[4..4 + end]);
        } else {
            return None;
        }
    }
}

/// Where `needle` first is in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The length of the doctype `d` starts with, to its closing `>`: past quoted strings and its
/// internal subset (with its own quoted strings and comments).
fn doctype_end(d: &[u8]) -> Option<usize> {
    let mut i = 0;
    let mut subset = false;
    while i < d.len() {
        match d[i] {
            q @ (b'"' | b'\'') => i += 1 + d[i + 1..].iter().position(|&c| c == q)?,
            b'[' if !subset => subset = true,
            b']' if subset => subset = false,
            b'<' if subset && d[i..].starts_with(b"<!--") => i += find(&d[i..], b"-->")? + 2,
            b'>' if !subset => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Where the tag whose attributes `t` starts with ends (its `>`), past quoted values.
fn tag_end(t: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            q @ (b'"' | b'\'') => i += 1 + t[i + 1..].iter().position(|&c| c == q)?,
            b'>' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The attributes of a tag (`t`: what follows its name, to its `>`): name and value.
fn attributes(t: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut i = 0;
    std::iter::from_fn(move || {
        loop {
            while t
                .get(i)
                .is_some_and(|&c| c.is_ascii_whitespace() || c == b'/')
            {
                i += 1;
            }
            let start = i;
            while t
                .get(i)
                .is_some_and(|&c| !c.is_ascii_whitespace() && c != b'=' && c != b'>')
            {
                i += 1;
            }
            if i == start {
                return None;
            }
            let name = &t[start..i];
            while t.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            if t.get(i) != Some(&b'=') {
                continue;
            }
            i += 1;
            while t.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            let q = *t.get(i)?;
            if q != b'"' && q != b'\'' {
                return None;
            }
            let end = i + 1 + t[i + 1..].iter().position(|&c| c == q)?;
            let value = &t[i + 1..end];
            i = end + 1;
            return Some((name, value));
        }
    })
}

/// A length as usvg reads it: a number and a unit, `None` if it isn't one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Length {
    Px(f32),
    Percent(f32),
}

fn length(v: &[u8]) -> Option<Length> {
    let s = std::str::from_utf8(v)
        .ok()?
        .trim_matches(|c: char| c.is_ascii_whitespace());
    // The number: a sign, digits with a point, and an exponent if a digit follows its `e`.
    let b = s.as_bytes();
    let digits = |mut i: usize| {
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    i = digits(i);
    if b.get(i) == Some(&b'.') {
        i = digits(i + 1);
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let j = i + 1 + usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            i = digits(j);
        }
    }
    let n: f32 = s[..i].parse().ok()?;
    let px = match &s[i..] {
        "" | "px" => n,
        "%" => return Some(Length::Percent(n)),
        "em" => n * FONT_SIZE,
        "ex" => n * FONT_SIZE / 2.0,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "pt" => n * 96.0 / 72.0,
        "pc" => n * 96.0 / 6.0,
        _ => return None,
    };
    Some(Length::Px(px))
}

/// A `viewBox`'s width and height, if it is four numbers with both positive.
fn view_box(v: &[u8]) -> Option<(f32, f32)> {
    let s = std::str::from_utf8(v).ok()?;
    let mut n = s
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<f32>().ok());
    let (_, _, w, h) = (n.next()??, n.next()??, n.next()??, n.next()??);
    (n.next().is_none() && w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()).then_some((w, h))
}

/// The size usvg gives an SVG whose root tag has the attributes `tag`, rounded to whole pixels
/// (at least 1), and scaled down to [`MAX_SIDE`](super::MAX_SIDE) a side: a missing or broken
/// `width` or `height` is 100%, a percentage is of the `viewBox` (or of 100 without one, when
/// usvg takes the size of what is drawn, which this can't know); `None` if usvg has no size.
fn natural(tag: &[u8]) -> Option<(u32, u32)> {
    let (mut w, mut h, mut vb) = (None, None, None);
    for (name, value) in attributes(tag) {
        match name {
            b"width" => w = length(value),
            b"height" => h = length(value),
            b"viewBox" => vb = view_box(value),
            _ => {}
        }
    }
    let side = |l: Option<Length>, vb: Option<f32>| match (l.unwrap_or(Length::Percent(100.0)), vb)
    {
        (Length::Px(px), _) => px,
        (Length::Percent(p), Some(v)) => v * p / 100.0,
        (Length::Percent(p), None) => DEFAULT_SIDE * p / 100.0,
    };
    let (w, h) = (side(w, vb.map(|v| v.0)), side(h, vb.map(|v| v.1)));
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return None;
    }
    let max = super::MAX_SIDE as f32;
    let scale = (max / w).min(max / h).min(1.0);
    let round = |v: f32| ((v * scale).round() as u32).clamp(1, super::MAX_SIDE);
    Some((round(w), round(h)))
}

// ---- Counting --------------------------------------------------------------------------------

/// What [`scan`] counts in an SVG's XML.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Scan {
    pub elements: usize,
    pub attributes: usize,
    /// Numbers in path data (`d`) and polygons (`points`).
    pub numbers: usize,
    /// Bytes of `data:` URLs (decoded from base64, at most this many).
    pub data: usize,
    /// Bytes of text outside tags, when there is a text element (else 0).
    pub text: usize,
    /// A text element is there: its glyphs need fonts.
    pub has_text: bool,
    /// Bytes entity references expand to.
    pub expanded: usize,
    /// Properties in style sheets, which may be set on every element.
    pub css: usize,
    /// Rules in style sheets (as many as it has `,` and `{`), and its bytes.
    pub rules: usize,
    pub style: usize,
    /// How deep the elements go.
    pub depth: usize,
}

/// Why an SVG is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Broken: not XML, not an SVG, not UTF-8, broken svgz.
    Broken,
    /// Over a cap on what it may make: nodes, depth, entities, `<use>`.
    Complex,
    /// Too much work to draw at this size ([`MAX_WORK`]), or drawing took too long.
    Slow,
    /// Its drawing ran out of the memory its process may have (`process`).
    #[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
    Memory,
}

/// Counts what parsing `xml` holds without allocating: one pass over its markup.
#[cfg(test)]
pub(super) fn scan(xml: &[u8]) -> Result<Scan, Refusal> {
    scan_in(xml, &Caps::NORMAL)
}

/// [`scan`] within `caps`.
fn scan_in(xml: &[u8], caps: &Caps) -> Result<Scan, Refusal> {
    let mut s = Scan::default();
    let mut entities = Entities::default();
    let mut depth = 0usize;
    let mut in_style = false;
    let mut i = 0;
    while i < xml.len() {
        if xml[i] != b'<' {
            let end = xml[i..]
                .iter()
                .position(|&c| c == b'<')
                .map_or(xml.len(), |n| i + n);
            let chunk = &xml[i..end];
            if in_style {
                s.style(chunk);
            } else {
                s.text += chunk.iter().filter(|c| !c.is_ascii_whitespace()).count();
            }
            s.expanded = s.expanded.saturating_add(entities.references(chunk)?);
            i = end;
            continue;
        }
        let rest = &xml[i..];
        if rest.starts_with(b"<!--") {
            i += find(rest, b"-->").ok_or(Refusal::Broken)? + 3;
        } else if rest.starts_with(b"<![CDATA[") {
            let n = find(rest, b"]]>").ok_or(Refusal::Broken)?;
            if in_style {
                s.style(&rest[9..n]);
            } else {
                s.text += n;
            }
            i += n + 3;
        } else if rest.starts_with(b"<!DOCTYPE") {
            let n = doctype_end(rest).ok_or(Refusal::Broken)?;
            entities = Entities::declared(&rest[..n], caps)?;
            i += n;
        } else if rest.starts_with(b"<?") || rest.starts_with(b"<!") {
            i += find(rest, b">").ok_or(Refusal::Broken)? + 1;
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            in_style = false;
            i += find(rest, b">").ok_or(Refusal::Broken)? + 1;
        } else {
            let name_end = rest[1..]
                .iter()
                .position(|&c| c.is_ascii_whitespace() || c == b'>' || c == b'/')
                .ok_or(Refusal::Broken)?
                + 1;
            let name = &rest[1..name_end];
            let end = name_end + tag_end(&rest[name_end..]).ok_or(Refusal::Broken)?;
            let tag = &rest[name_end..end];
            s.elements += 1;
            let local = name.rsplit(|&c| c == b':').next().unwrap_or(name);
            if matches!(local, b"text" | b"tspan" | b"textPath") {
                s.has_text = true;
            }
            in_style = local == b"style";
            for (attr, value) in attributes(tag) {
                s.attributes += 1;
                match attr {
                    b"d" | b"points" => s.numbers += numbers(value),
                    // Each property of an element's own style is an attribute of it.
                    b"style" => s.attributes += value.iter().filter(|&&c| c == b':').count(),
                    _ => {}
                }
                if value.starts_with(b"data:") {
                    s.data += value.len();
                }
                s.expanded = s.expanded.saturating_add(entities.references(value)?);
            }
            if tag.last() != Some(&b'/') {
                depth += 1;
                s.depth = s.depth.max(depth);
            }
            i += end + 1;
        }
        if s.expanded > caps.expanded || s.depth > caps.depth {
            return Err(Refusal::Complex);
        }
    }
    if !s.has_text {
        s.text = 0;
    }
    Ok(s)
}

impl Scan {
    /// The text of a `<style>`.
    fn style(&mut self, text: &[u8]) {
        self.style += text.len();
        self.css += text.iter().filter(|&&c| c == b':').count();
        self.rules += text.iter().filter(|&&c| matches!(c, b'{' | b',')).count();
    }
}

/// How many numbers `v` (path data, or a list of points) has.
fn numbers(v: &[u8]) -> usize {
    let mut n = 0;
    let mut prev_digit = false;
    let mut point = false;
    for &c in v {
        let digit = c.is_ascii_digit() || c == b'.';
        // A second point starts another number (`.5.5`).
        if digit && (!prev_digit || (c == b'.' && point)) {
            n += 1;
            point = false;
        }
        point |= c == b'.';
        prev_digit = digit;
    }
    n
}

/// The entities an internal subset declares, and the bytes each expands to (its references
/// expanded too, 10 deep at most, as `roxmltree` does).
#[derive(Default)]
struct Entities<'a> {
    expands: HashMap<&'a [u8], usize>,
    /// [`Caps::expanded`].
    max: usize,
}

impl<'a> Entities<'a> {
    /// The general entities `doctype` declares (`<!ENTITY name "value">`).
    fn declared(doctype: &'a [u8], caps: &Caps) -> Result<Entities<'a>, Refusal> {
        let mut values: HashMap<&[u8], &[u8]> = HashMap::new();
        let mut rest = doctype;
        while let Some(at) = find(rest, b"<!ENTITY") {
            rest = &rest[at + 8..];
            let mut words = rest
                .split(|c| c.is_ascii_whitespace())
                .filter(|w| !w.is_empty());
            let Some(name) = words.next() else { break };
            if name == b"%" {
                continue; // a parameter entity: not in the document's text
            }
            let Some(q) = rest.iter().position(|&c| c == b'"' || c == b'\'') else {
                break;
            };
            let quote = rest[q];
            let Some(len) = rest[q + 1..].iter().position(|&c| c == quote) else {
                break;
            };
            // The first declaration of a name is the one that counts.
            values.entry(name).or_insert(&rest[q + 1..q + 1 + len]);
            if values.len() > caps.entities {
                return Err(Refusal::Complex);
            }
            rest = &rest[q + 1 + len..];
        }
        let mut e = Entities {
            expands: HashMap::with_capacity(values.len()),
            max: caps.expanded,
        };
        for &name in values.keys() {
            expand(name, &values, &mut e.expands, 0, caps.expanded);
        }
        Ok(e)
    }

    /// The bytes the references in `text` expand to.
    fn references(&self, text: &[u8]) -> Result<usize, Refusal> {
        if self.expands.is_empty() {
            return Ok(0);
        }
        let mut total = 0usize;
        let mut rest = text;
        while let Some(at) = rest.iter().position(|&c| c == b'&') {
            rest = &rest[at + 1..];
            let Some(end) = rest.iter().position(|&c| c == b';') else {
                break;
            };
            if let Some(&n) = self.expands.get(&rest[..end]) {
                total = total.saturating_add(n);
                if total > self.max {
                    return Err(Refusal::Complex);
                }
            }
            rest = &rest[end + 1..];
        }
        Ok(total)
    }
}

/// The bytes entity `name` expands to, its own references expanded, each entity worked out
/// once (`known`); more than `max` for a loop or more than 10 deep.
fn expand<'a>(
    name: &'a [u8],
    values: &HashMap<&'a [u8], &'a [u8]>,
    known: &mut HashMap<&'a [u8], usize>,
    depth: usize,
    max: usize,
) -> usize {
    if let Some(&n) = known.get(name) {
        return n;
    }
    let Some(&value) = values.get(name) else {
        return 0;
    };
    if depth > 10 {
        return max.saturating_add(1);
    }
    let mut total = value.len();
    let mut rest = value;
    while let Some(at) = rest.iter().position(|&c| c == b'&') {
        rest = &rest[at + 1..];
        let Some(end) = rest.iter().position(|&c| c == b';') else {
            break;
        };
        total = total.saturating_add(expand(&rest[..end], values, known, depth + 1, max));
        if total > max {
            break;
        }
        rest = &rest[end + 1..];
    }
    known.insert(name, total);
    total
}

/// What parsing an SVG whose XML counted `s` (with `extra` elements that `<use>` makes) holds:
/// see [`PARSE_FIXED`] and the costs after it.
pub(super) fn parse_cost(s: &Scan, extra: usize, xml_copy: usize) -> usize {
    let elements = s.elements.saturating_add(extra);
    // (What a style sheet sets on each element is counted by the rules that match it: `Made`'s
    // `applied`.)
    PARSE_FIXED
        .saturating_add(elements.saturating_mul(ELEMENT_COST))
        .saturating_add(s.attributes.saturating_mul(ATTRIBUTE_COST))
        .saturating_add(s.numbers.saturating_mul(NUMBER_COST))
        .saturating_add(s.data)
        .saturating_add(s.text.saturating_mul(TEXT_COST))
        .saturating_add(s.expanded.saturating_mul(EXPANDED_COST))
        .saturating_add(s.rules.saturating_mul(RULE_COST))
        .saturating_add(s.style.saturating_mul(SHEET_BYTE_COST))
        .saturating_add(s.css.saturating_mul(DECLARATION_COST))
        .saturating_add(xml_copy)
}

/// What the first job of an SVG with `xml` bytes of XML is charged for parsing: a guess.
pub(super) fn guess_parse(xml: usize, gz: bool) -> usize {
    PARSE_FIXED
        .saturating_add(xml.saturating_mul(GUESS_PER_BYTE).min(GUESS_MAX))
        .saturating_add(if gz { xml } else { 0 })
}

/// What a pixmap of a layer of a box of `w` × `h` may hold beyond its share of the canvas
/// ([`SvgCost::layers`]): resvg makes a layer 2 pixels bigger each side than what it holds, and
/// rounds a filter's region out, each side at most 5 canvases long.
fn margin(w: u32, h: u32) -> usize {
    80 * (w as usize + h as usize) + 64
}

/// `svg`, with what the first job is charged for in place of what no job has measured yet.
fn known(svg: &SvgCost) -> SvgCost {
    if svg.parse > 0 {
        return *svg;
    }
    SvgCost {
        parse: guess_parse(svg.xml as usize, svg.gz),
        layers: svg.layers.max(GUESS_LAYERS),
        pixmaps: svg.pixmaps.max(GUESS_PIXMAPS),
        draw: svg.draw.max(GUESS_DRAW),
        ..*svg
    }
}

/// What decoding an SVG to `w` × `h` holds (see `decode_cost`), by what is known of it: parsing,
/// drawing its largest path, its largest embedded raster image, the canvas and the result it is
/// copied to, and the layers rendering holds beside them.
pub(super) fn cost(svg: &SvgCost, w: u32, h: u32) -> usize {
    let svg = known(svg);
    let canvas = w as usize * h as usize * 4;
    svg.parse
        .saturating_add(svg.draw)
        .saturating_add(svg.raster)
        .saturating_add(canvas.saturating_mul(2))
        .saturating_add(canvas.saturating_mul(svg.layers as usize) / LAYER_UNITS as usize)
        .saturating_add(margin(w, h).saturating_mul(svg.pixmaps as usize))
}

/// What decoding an SVG holds for each pixel of its box besides its result twice, and for each
/// pixel of its sides (see `image::pixel_cost`): its layers, and their margins.
pub(super) fn box_costs(svg: &SvgCost) -> (u32, u32) {
    let svg = known(svg);
    (
        (svg.layers.saturating_mul(4)).div_ceil(LAYER_UNITS),
        svg.pixmaps.saturating_mul(80),
    )
}

/// What usvg makes for something besides what the file holds, and holds a copy of for each use of
/// it: its elements, the numbers of its path data (a gradient's stops weigh as many) and the
/// bytes of its text. A copy of a path of 50,000 points is not one element. And what it does, for
/// each copy too, that can't be stopped: matching a style sheet's selectors to it (`css`, in
/// steps: see [`css_costs`]) and shaping its text (`shaping`: see [`Chunks`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Made {
    elements: usize,
    numbers: usize,
    text: usize,
    css: usize,
    shaping: usize,
    /// Declarations of a style sheet set on its elements (each makes an attribute).
    applied: usize,
}

impl Made {
    fn plus(self, o: Made) -> Made {
        Made {
            elements: self.elements.saturating_add(o.elements),
            numbers: self.numbers.saturating_add(o.numbers),
            text: self.text.saturating_add(o.text),
            css: self.css.saturating_add(o.css),
            shaping: self.shaping.saturating_add(o.shaping),
            applied: self.applied.saturating_add(o.applied),
        }
    }

    fn times(self, n: usize) -> Made {
        Made {
            elements: self.elements.saturating_mul(n),
            numbers: self.numbers.saturating_mul(n),
            text: self.text.saturating_mul(n),
            css: self.css.saturating_mul(n),
            shaping: self.shaping.saturating_mul(n),
            applied: self.applied.saturating_mul(n),
        }
    }

    /// The most of each.
    fn most(self, o: Made) -> Made {
        Made {
            elements: self.elements.max(o.elements),
            numbers: self.numbers.max(o.numbers),
            text: self.text.max(o.text),
            css: self.css.max(o.css),
            shaping: self.shaping.max(o.shaping),
            applied: self.applied.max(o.applied),
        }
    }

    /// What it adds to what parsing holds ([`parse_cost`] counts the elements, with `s`): its
    /// work holds nothing.
    fn cost(&self, s: &Scan) -> usize {
        (parse_cost(s, self.elements, 0) - parse_cost(s, 0, 0))
            .saturating_add(self.applied.saturating_mul(ATTRIBUTE_COST))
            .saturating_add(self.numbers.saturating_mul(NUMBER_COST))
            .saturating_add(self.text.saturating_mul(TEXT_COST))
    }
}

/// What [`instances`] may count, at most: in proportion to the file, as the tree is, but never
/// less than [`FLOOR_ELEMENTS`], [`FLOOR_NUMBERS`] and [`FLOOR_TEXT`] (a small file that reuses a
/// symbol many times is ordinary: what it makes is charged against the budget all the same).
#[derive(Debug)]
struct Limit {
    elements: usize,
    numbers: usize,
    text: usize,
    /// What is left of the work that can't be stopped ([`Caps::css`], [`Caps::shaping`]) for
    /// what is counted: what the file's own elements take is taken off first.
    css: Cell<usize>,
    shaping: Cell<usize>,
    /// It was passed: what is counted would not fit a forced load's room.
    stopped: Cell<bool>,
}

/// What [`Limit::of`] lets any file make, whatever its size: what costs 50 MB or so (see
/// [`ELEMENT_COST`], [`NUMBER_COST`] and [`TEXT_COST`]) and is built in a fraction of a second.
const FLOOR_ELEMENTS: usize = 50_000;
const FLOOR_NUMBERS: usize = 1_000_000;
const FLOOR_TEXT: usize = 20_000;

impl Limit {
    /// No limit.
    #[cfg(test)]
    fn none() -> Limit {
        Limit::new(usize::MAX, usize::MAX, usize::MAX, usize::MAX, usize::MAX)
    }

    fn new(elements: usize, numbers: usize, text: usize, css: usize, shaping: usize) -> Limit {
        Limit {
            elements,
            numbers,
            text,
            css: Cell::new(css),
            shaping: Cell::new(shaping),
            stopped: Cell::new(false),
        }
    }

    /// The limit of an SVG of `xml` bytes within `caps`, with `room` bytes left to charge what is
    /// counted against: a forced load's is what would fit it, so that counting stops where
    /// parsing would not be let go on anyway. `css` and `shaping` are what is left of the work.
    fn of(xml: usize, caps: &Caps, room: usize, css: usize, shaping: usize) -> Limit {
        if !caps.work {
            return Limit::new(
                (room / ELEMENT_COST).min(caps.nodes as usize),
                room / NUMBER_COST,
                room / TEXT_COST,
                css,
                shaping,
            );
        }
        Limit::new(
            (1_000 + xml / 8)
                .max(FLOOR_ELEMENTS)
                .min(caps.nodes as usize),
            (10_000 + xml).max(FLOOR_NUMBERS),
            (1_000 + xml / 4).max(FLOOR_TEXT),
            css,
            shaping,
        )
    }

    /// Takes `m`'s work off what is left: `false` if it was more than there was.
    fn spend(&self, m: &Made) -> bool {
        let (css, shaping) = (self.css.get(), self.shaping.get());
        self.css.set(css.saturating_sub(m.css));
        self.shaping.set(shaping.saturating_sub(m.shaping));
        m.css <= css && m.shaping <= shaping
    }

    fn passed_by(&self, m: &Made) -> bool {
        let passed = m.elements > self.elements || m.numbers > self.numbers || m.text > self.text;
        if passed {
            self.stopped.set(true);
        }
        passed || m.css > self.css.get() || m.shaping > self.shaping.get()
    }
}

/// What usvg makes in `doc` besides what is in the file, by `<use>` (its target's elements, with
/// their path data and text, counted once for each: an element used twice makes its own twice), by
/// markers (the marker's, at each vertex of each path that has one), and by what an element names
/// that usvg converts again for it: a filter's `feImage`s, a clip path or mask (unless both its
/// units are user space), a pattern's elements and a gradient's stops (cloned for each shape that
/// has one), and a `textPath`'s path. `Err` past `limit`, or for a reference loop that doesn't end
/// within [`MAX_NESTED_USE`] levels, or once `cancel` is set (counting is a pass over what the
/// file makes, which may be a lot).
fn instances(
    doc: &roxmltree::Document,
    limit: &Limit,
    css: &Matched,
    cancel: &AtomicBool,
) -> Result<Made, Refusal> {
    let mut ids: HashMap<&str, roxmltree::NodeId> = HashMap::new();
    let mut servers = false;
    // What each element is in, by what its parent is in (it comes before it): a walk up the
    // elements for each would be as long as they are deep.
    let mut flags: HashMap<roxmltree::NodeId, u8> = HashMap::new();
    for n in doc.descendants().filter(roxmltree::Node::is_element) {
        if let Some(id) = n.attribute("id") {
            ids.insert(id, n.id());
        }
        servers |= PAINT_SERVERS.iter().any(|t| n.has_tag_name((SVG_NS, *t)));
        let mut f = n
            .parent()
            .and_then(|p| flags.get(&p.id()))
            .copied()
            .unwrap_or(0);
        for (bit, tags) in WITHIN {
            if tags.iter().any(|t| n.has_tag_name((SVG_NS, *t))) {
                f |= bit;
            }
        }
        flags.insert(n.id(), f);
    }
    let mut count = Instances {
        doc,
        ids,
        servers,
        flags,
        inherits: HashMap::new(),
        direct: HashMap::new(),
        urls: HashMap::new(),
        vertices: HashMap::new(),
        own: HashMap::new(),
        memo: HashMap::new(),
        active: Vec::new(),
        fallback: [Made::default(); 3],
        sheet: Made::default(),
        once: Made::default(),
        charged: HashSet::new(),
        painted: Made::default(),
        css,
        limit,
        cancel,
    };
    // What usvg does for the elements of the file itself comes first: no more work than there is
    // is left for their copies.
    let mut base = Made::default();
    for n in doc.descendants().filter(roxmltree::Node::is_element) {
        count.tick()?;
        let own = count.own(n);
        base = base.plus(Made {
            css: own.css,
            shaping: own.shaping,
            applied: own.applied,
            ..Made::default()
        });
    }
    if !limit.spend(&base) {
        return Err(Refusal::Complex);
    }
    count.fallback = count.fallback()?;
    count.sheet_and_painted()?;
    let mut extra = count.once;
    // A `<use>` in `<defs>` or a `<symbol>` makes its elements where what holds it is used:
    // counted there, by `made`. A marker's shapes are drawn at the vertices of the shape that has
    // it (`marker_size`), and a clip path's have none.
    for n in doc.descendants().filter(roxmltree::Node::is_element) {
        count.tick()?;
        let in_marker = count.within(n, IN_MARKER);
        if n.has_tag_name((SVG_NS, "use")) && !count.within(n, IN_DEFS) {
            extra = extra.plus(count.made(n, 0)?);
        } else if !in_marker {
            extra = extra.plus(count.markers(n, 0)?);
        }
        if !in_marker {
            extra = extra.plus(count.references(n, 0, false)?);
        }
        if limit.passed_by(&extra) {
            return Err(Refusal::Complex);
        }
    }
    Ok(extra.plus(base))
}

/// What shaping text takes, by counting its bytes (one is a unit): the characters of the nested
/// `tspan`s that are counted this many to a unit, and the glyph and segment pairs of a `textPath`.
const COUNT_PER_UNIT: usize = 32;
const PATH_PER_UNIT: usize = 1024;

/// The text of one `<text>` as usvg makes it chunks: the spans (a text node each) and bytes of the
/// chunk being read, and the work of those done.
#[derive(Default)]
struct Chunks {
    spans: usize,
    bytes: usize,
    /// The next text starts a chunk.
    split: bool,
    /// Bytes of text so far, in all chunks.
    bytes_seen: usize,
    work: usize,
}

impl Chunks {
    /// A text node of `len` bytes.
    fn text(&mut self, len: usize) {
        if len == 0 {
            return;
        }
        if self.split || self.spans == 0 {
            self.flush();
            self.split = false;
        }
        self.spans += 1;
        self.bytes = self.bytes.saturating_add(len);
        self.bytes_seen = self.bytes_seen.saturating_add(len);
    }

    /// The chunk is done: shaped whole for each span, and its glyphs found among each other's
    /// (when it has more than one span).
    fn flush(&mut self) {
        let overwrite = if self.spans > 1 {
            self.bytes.saturating_mul(self.bytes) / OVERWRITE_PER_UNIT
        } else {
            0
        };
        self.work = self
            .work
            .saturating_add(self.spans.saturating_mul(self.bytes))
            .saturating_add(overwrite);
        self.spans = 0;
        self.bytes = 0;
    }
}

/// The glyph pairs of the spans of a chunk that are one unit.
const OVERWRITE_PER_UNIT: usize = 256;

/// The SVG namespace.
const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// `<use>`s within `<use>`s, and markers within markers, at most this deep (usvg stops at
/// reference loops).
const MAX_NESTED_USE: usize = 32;

/// How many vertices a rectangle, circle or ellipse has, at most (markers are put on those too).
const SHAPE_VERTICES: usize = 12;

/// The properties that name a shape's markers.
const MARKER_PROPERTIES: [&str; 3] = ["marker-start", "marker-mid", "marker-end"];

/// The elements a shape's `fill` and `stroke` may name.
const PAINT_SERVERS: [&str; 3] = ["pattern", "linearGradient", "radialGradient"];

/// The properties (not inherited, but for the paint) that name what is converted again for the
/// elements that have them, and the elements they may name.
const REFERENCES: [(&str, &[&str]); 5] = [
    ("filter", &["filter"]),
    ("clip-path", &["clipPath"]),
    ("mask", &["mask"]),
    ("fill", &PAINT_SERVERS),
    ("stroke", &PAINT_SERVERS),
];

/// What an element may be in (or be), for [`Instances::within`]: a `<defs>` or a `<symbol>`, a
/// `<marker>`, a `<clipPath>`.
const IN_DEFS: u8 = 1;
const IN_MARKER: u8 = 2;
const IN_CLIP: u8 = 4;
const WITHIN: [(u8, &[&str]); 3] = [
    (IN_DEFS, &["defs", "symbol"]),
    (IN_MARKER, &["marker"]),
    (IN_CLIP, &["clipPath"]),
];

/// What [`instances`] counts, in one document.
struct Instances<'a> {
    doc: &'a roxmltree::Document<'a>,
    ids: HashMap<&'a str, roxmltree::NodeId>,
    /// The file has a pattern or gradient.
    servers: bool,
    /// What each element is in (or is): the `IN_` flags.
    flags: HashMap<roxmltree::NodeId, u8>,
    /// What each property an element inherits is set to for it, as it is asked for.
    inherits: HashMap<(roxmltree::NodeId, &'static str), Option<&'a str>>,
    /// What each property an element sets itself is set to, and what `url(#id)`s each value has
    /// (found once for each, whatever the copies of what has it), and how many vertices each shape
    /// has: scanning data is for each element that holds it, not each of its copies.
    direct: HashMap<(roxmltree::NodeId, &'static str), Option<&'a str>>,
    urls: HashMap<(usize, usize), Rc<[&'a str]>>,
    vertices: HashMap<roxmltree::NodeId, Option<usize>>,
    /// What each element makes by itself, as it is counted.
    own: HashMap<roxmltree::NodeId, Made>,
    /// What each `<use>` makes, and each marker draws, and each element that is named makes once,
    /// as it is counted.
    memo: HashMap<roxmltree::NodeId, Made>,
    /// What is being counted: a marker (or a clip path, or a filter) with itself in it makes
    /// nothing there, as in usvg.
    active: Vec<roxmltree::NodeId>,
    /// The elements of the dearest marker a style sheet or a `<use>` may set on a shape, for its
    /// `marker-start`, `marker-mid` and `marker-end` (those are counted for each shape, as they
    /// are for no reference to them): see [`fallback`].
    fallback: [Made; 3],
    /// What the dearest element a style sheet names, with `url(#id)`, makes: it may be set on
    /// any element.
    sheet: Made,
    /// What the elements a style sheet names that usvg converts once make.
    once: Made,
    /// The elements that usvg converts once, whose content is counted.
    charged: HashSet<roxmltree::NodeId>,
    /// What the dearest pattern or gradient a `<use>` names for its `fill` or `stroke` makes: its
    /// copy's shapes inherit it.
    painted: Made,
    /// What matching a style sheet's rules to each element takes and sets, by node
    /// ([`css_costs`]).
    css: &'a Matched,
    limit: &'a Limit,
    cancel: &'a AtomicBool,
}

impl<'a> Instances<'a> {
    /// Element `n` is (or is in) what `flag` says.
    fn within(&self, n: roxmltree::Node, flag: u8) -> bool {
        self.flags.get(&n.id()).is_some_and(|f| f & flag != 0)
    }

    /// Counting is stopped: `cancel` is set.
    fn tick(&self) -> Result<(), Refusal> {
        if cancelled(self.cancel) {
            return Err(Refusal::Complex);
        }
        Ok(())
    }

    /// What `property` is set to for `n` itself ([`property_of`]), found once.
    fn direct(&mut self, n: roxmltree::Node<'a, 'a>, property: &'static str) -> Option<&'a str> {
        *self
            .direct
            .entry((n.id(), property))
            .or_insert_with(|| property_of(n, property))
    }

    /// The ids in the `url(#id)`s of `value` (a value of the file), found once for the value.
    fn urls(&mut self, value: &'a str) -> Rc<[&'a str]> {
        self.urls
            .entry((value.as_ptr() as usize, value.len()))
            .or_insert_with(|| url_ids(value).collect())
            .clone()
    }

    /// How many vertices `n` has ([`vertices`]), counted once.
    fn vertices(&mut self, n: roxmltree::Node) -> Option<usize> {
        *self.vertices.entry(n.id()).or_insert_with(|| vertices(n))
    }

    /// What the property `name`, which elements inherit, is set to for `n`: the nearest value of
    /// it and the elements it is in, in `style` or as an attribute (see [`property_of`]).
    fn inherited(&mut self, n: roxmltree::Node<'a, 'a>, name: &'static str) -> Option<&'a str> {
        let mut path = Vec::new();
        let mut found = None;
        let mut at = Some(n);
        while let Some(a) = at {
            if let Some(&v) = self.inherits.get(&(a.id(), name)) {
                found = v;
                break;
            }
            path.push(a.id());
            if let Some(v) = self.direct(a, name) {
                found = Some(v);
                break;
            }
            at = a.parent();
        }
        for id in path {
            self.inherits.insert((id, name), found);
        }
        found
    }

    /// What a shape with no marker of its own may be given, for each of the properties, by a style
    /// sheet that sets it (a selector may match any shape) or by a `<use>` in whose content it is
    /// drawn (usvg gives the `<use>`'s properties to its copy): the elements of the dearest marker
    /// the file has that these could name, drawn on its own.
    fn fallback(&mut self) -> Result<[Made; 3], Refusal> {
        let mut sheet = [false; 3];
        for text in self
            .doc
            .descendants()
            .filter(|n| n.has_tag_name((SVG_NS, "style")))
            .flat_map(|n| n.descendants())
            .filter_map(|t| t.text())
        {
            for (set, has) in sheet.iter_mut().zip(sets_markers(text)) {
                *set |= has;
            }
        }
        let doc = self.doc;
        let markers: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name((SVG_NS, "marker")))
            .map(|n| n.id())
            .collect();
        let mut fallback = [Made::default(); 3];
        for (i, property) in MARKER_PROPERTIES.into_iter().enumerate() {
            let mut named = if sheet[i] {
                markers.clone()
            } else {
                Vec::new()
            };
            for n in doc
                .descendants()
                .filter(|n| n.has_tag_name((SVG_NS, "use")))
            {
                named.extend(self.marker_of(n, property));
            }
            for m in named {
                let node = doc.get_node(m).ok_or(Refusal::Broken)?;
                // Its own elements only: a marker of its markers is a shape's.
                let flat = node
                    .descendants()
                    .filter(roxmltree::Node::is_element)
                    .fold(Made::default(), |t, d| t.plus(self.own(d)));
                fallback[i] = fallback[i].most(flat);
                if self.limit.passed_by(&fallback[i]) {
                    return Err(Refusal::Complex);
                }
            }
        }
        Ok(fallback)
    }

    /// [`Instances::sheet`] and [`Instances::painted`]: what a style sheet may name for any
    /// element, and what a `<use>` names for its copy's shapes.
    fn sheet_and_painted(&mut self) -> Result<(), Refusal> {
        let doc = self.doc;
        let mut named: Vec<&str> = Vec::new();
        for text in doc
            .descendants()
            .filter(|n| n.has_tag_name((SVG_NS, "style")))
            .flat_map(|n| n.descendants())
            .filter_map(|t| t.text())
        {
            named.extend(url_ids(text));
        }
        for id in named {
            let tags = [
                "filter",
                "clipPath",
                "mask",
                "pattern",
                "linearGradient",
                "radialGradient",
            ];
            let made = self.named(id, &tags, 0)?;
            match self.ids.get(id).and_then(|&e| doc.get_node(e)) {
                Some(e) if shared(e) => {
                    if self.charged.insert(e.id()) {
                        self.once = self.once.plus(made);
                    }
                }
                _ => self.sheet = self.sheet.most(made),
            }
        }
        for n in doc
            .descendants()
            .filter(|n| n.has_tag_name((SVG_NS, "use")))
        {
            for property in ["fill", "stroke"] {
                let ids = self.inherited(n, property).map(|v| self.urls(v));
                for &id in ids.iter().flat_map(|ids| ids.iter()) {
                    let made = self.named(id, &PAINT_SERVERS, 0)?;
                    self.painted = self.painted.most(made);
                }
            }
        }
        Ok(())
    }

    /// What `n` makes by itself: itself, the numbers of its path data, and its text (if it is
    /// text), with the work of matching the style sheet to it and of shaping its text.
    fn own(&mut self, n: roxmltree::Node<'a, 'a>) -> Made {
        if let Some(&m) = self.own.get(&n.id()) {
            return m;
        }
        let data = |name| n.attribute(name).map_or(0, |v| numbers(v.as_bytes()));
        let text = if ["text", "tspan", "textPath"]
            .iter()
            .any(|t| n.has_tag_name((SVG_NS, *t)))
        {
            n.children()
                .filter_map(|c| c.text())
                .map(|t| t.bytes().filter(|b| !b.is_ascii_whitespace()).count())
                .sum()
        } else {
            0
        };
        let shaping = if n.has_tag_name((SVG_NS, "text")) {
            self.shaping(n)
        } else {
            0
        };
        let made = Made {
            elements: 1,
            numbers: data("d") + data("points"),
            text,
            css: self
                .css
                .steps
                .get(n.id().get_usize())
                .map_or(0, |&c| c as usize),
            shaping,
            applied: self
                .css
                .applied
                .get(n.id().get_usize())
                .map_or(0, |&c| c as usize),
        };
        self.own.insert(n.id(), made);
        made
    }

    /// What shaping the text element `text` takes, in units of a byte shaped once: usvg shapes the
    /// whole of a text chunk (the text from one absolute position, or one `textPath`, to the next)
    /// for each of its spans, finds the glyphs of each in the chunk's, and sets glyphs along a
    /// `textPath` one by one in a walk of its segments.
    fn shaping(&mut self, text: roxmltree::Node<'a, 'a>) -> usize {
        let mut chunks = Chunks::default();
        self.chunks(text, &mut chunks);
        chunks.flush();
        chunks.work
    }

    fn chunks(&mut self, n: roxmltree::Node<'a, 'a>, c: &mut Chunks) {
        for child in n.children() {
            if child.is_text() {
                c.text(child.text().map_or(0, str::len));
                continue;
            }
            if !child.is_element() {
                continue;
            }
            let path = child.has_tag_name((SVG_NS, "textPath"));
            let span = child.has_tag_name((SVG_NS, "tspan"));
            // An absolute position starts a chunk (usvg would start several, for a list of them).
            let placed = |name| child.attribute(name).is_some_and(|v| !v.trim().is_empty());
            c.split |= path || (span && (placed("x") || placed("y")));
            let before = c.bytes_seen;
            self.chunks(child, c);
            let inside = c.bytes_seen - before;
            if span {
                // usvg counts the characters of each `tspan`, nested ones again for each.
                c.work = c.work.saturating_add(inside / COUNT_PER_UNIT);
            }
            if path {
                c.split = true;
                let doc = self.doc;
                if let Some(&p) = href(child).and_then(|id| self.ids.get(id))
                    && let Some(segments) = doc.get_node(p).and_then(|p| self.vertices(p))
                {
                    c.work = c
                        .work
                        .saturating_add(segments.saturating_mul(inside) / PATH_PER_UNIT);
                }
            }
        }
    }

    /// What the content of `root` makes (all but `skip`, a `<use>` that is it): each element's
    /// own, and what its `<use>`s, markers and references make.
    fn content(
        &mut self,
        root: roxmltree::Node<'a, 'a>,
        skip: Option<roxmltree::NodeId>,
        depth: usize,
    ) -> Result<Made, Refusal> {
        let mut total = Made::default();
        for d in root.descendants().filter(roxmltree::Node::is_element) {
            self.tick()?;
            total = total.plus(self.own(d));
            if d.has_tag_name((SVG_NS, "use")) && Some(d.id()) != skip {
                total = total.plus(self.made(d, depth + 1)?);
            }
            total = total.plus(self.markers(d, depth)?);
            total = total.plus(self.references(d, depth, true)?);
            if self.limit.passed_by(&total) {
                return Err(Refusal::Complex);
            }
        }
        Ok(total)
    }

    /// What a `<use>` (`n`) makes: its target's content.
    fn made(&mut self, n: roxmltree::Node<'a, 'a>, depth: usize) -> Result<Made, Refusal> {
        if let Some(&m) = self.memo.get(&n.id()) {
            return Ok(m);
        }
        if depth > MAX_NESTED_USE {
            return Err(Refusal::Complex);
        }
        let Some(target) = href(n).and_then(|id| self.ids.get(id)) else {
            return Ok(Made::default());
        };
        let target = self.doc.get_node(*target).ok_or(Refusal::Broken)?;
        let total = self.content(target, Some(n.id()), depth)?;
        self.memo.insert(n.id(), total);
        Ok(total)
    }

    /// What the markers of `n`, if it is a shape, draw: a marker at its start and at its end, and
    /// one at every other vertex ([`vertices`]).
    fn markers(&mut self, n: roxmltree::Node<'a, 'a>, depth: usize) -> Result<Made, Refusal> {
        let Some(vertices) = self.vertices(n) else {
            return Ok(Made::default());
        };
        // usvg draws none on a shape in a clip path.
        if self.within(n, IN_CLIP) {
            return Ok(Made::default());
        }
        let mut total = Made::default();
        for ((property, times), fallback) in MARKER_PROPERTIES
            .into_iter()
            .zip([1, vertices, 1])
            .zip(self.fallback)
        {
            let own = match self.marker_of(n, property) {
                Some(m) => self.marker_size(m, depth + 1)?,
                None => Made::default(),
            };
            let each = own.most(fallback);
            total = total.plus(each.times(times));
            if self.limit.passed_by(&total) {
                return Err(Refusal::Complex);
            }
        }
        Ok(total)
    }

    /// What one drawing of marker `m` makes: its own, and what its `<use>`s and its shapes'
    /// markers and references make.
    fn marker_size(&mut self, m: roxmltree::NodeId, depth: usize) -> Result<Made, Refusal> {
        if let Some(&n) = self.memo.get(&m) {
            return Ok(n);
        }
        if self.active.contains(&m) {
            return Ok(Made::default());
        }
        if depth > MAX_NESTED_USE {
            return Err(Refusal::Complex);
        }
        let node = self.doc.get_node(m).ok_or(Refusal::Broken)?;
        self.active.push(m);
        let mut total = Made::default();
        for d in node.descendants().filter(roxmltree::Node::is_element) {
            total = total.plus(self.own(d));
            let more = (|| {
                self.tick()?;
                let mut more = Made::default();
                if d.has_tag_name((SVG_NS, "use")) {
                    more = more.plus(self.made(d, depth)?);
                }
                if d != node {
                    more = more.plus(self.markers(d, depth)?);
                }
                Ok(more.plus(self.references(d, depth, true)?))
            })();
            match more {
                Ok(more) => total = total.plus(more),
                Err(why) => {
                    self.active.pop();
                    return Err(why);
                }
            }
            if self.limit.passed_by(&total) {
                self.active.pop();
                return Err(Refusal::Complex);
            }
        }
        self.active.pop();
        self.memo.insert(m, total);
        Ok(total)
    }

    /// What `n` makes by the elements it names that usvg converts for it: the `filter`,
    /// `clip-path` and `mask` it has, its `fill` and `stroke` (inherited, and only if it is a
    /// shape or text) and a `textPath`'s path. `in_copy`: `n` is in what a `<use>` or a marker
    /// makes, where what a style sheet or the `<use>` gives is drawn on it.
    fn references(
        &mut self,
        n: roxmltree::Node<'a, 'a>,
        depth: usize,
        in_copy: bool,
    ) -> Result<Made, Refusal> {
        let mut total = self.sheet;
        let painted = self.vertices(n).is_some()
            || ["text", "tspan"]
                .iter()
                .any(|t| n.has_tag_name((SVG_NS, *t)));
        if in_copy && painted {
            total = total.plus(self.painted);
        }
        for (property, tags) in REFERENCES {
            let paint = tags == PAINT_SERVERS;
            if paint && !(self.servers && painted) {
                continue;
            }
            let value = if paint {
                self.inherited(n, property)
            } else {
                self.direct(n, property)
            };
            let Some(value) = value else {
                continue;
            };
            for &id in self.urls(value).iter() {
                let made = self.named(id, tags, depth)?;
                // What usvg converts once is counted once.
                let once = self
                    .ids
                    .get(id)
                    .and_then(|&e| self.doc.get_node(e))
                    .is_some_and(|e| shared(e) && !self.charged.insert(e.id()));
                if !once {
                    total = total.plus(made);
                }
            }
        }
        if let Some(&path) = href(n)
            .filter(|_| n.has_tag_name((SVG_NS, "textPath")))
            .and_then(|id| self.ids.get(id))
        {
            let path = self.doc.get_node(path).ok_or(Refusal::Broken)?;
            total.numbers = total.numbers.saturating_add(self.own(path).numbers);
        }
        if self.limit.passed_by(&total) {
            return Err(Refusal::Complex);
        }
        Ok(total)
    }

    /// What the element `id`, if it is one of `tags`, makes for each element that names it: a
    /// filter's `feImage`s' targets (usvg converts those again for each element that has the
    /// filter, unless it is in user space), a clip path's or mask's content, a pattern's
    /// elements (its path data is shared), a gradient's stops.
    fn named(&mut self, id: &str, tags: &[&str], depth: usize) -> Result<Made, Refusal> {
        let Some(&e) = self.ids.get(id) else {
            return Ok(Made::default());
        };
        let node = self.doc.get_node(e).ok_or(Refusal::Broken)?;
        if !tags.iter().any(|t| node.has_tag_name((SVG_NS, *t))) {
            return Ok(Made::default());
        }
        if let Some(&m) = self.memo.get(&e) {
            return Ok(m);
        }
        if self.active.contains(&e) {
            return Ok(Made::default());
        }
        if depth + 1 > MAX_NESTED_USE {
            return Err(Refusal::Complex);
        }
        self.active.push(e);
        let made = self.named_content(node, depth + 1);
        self.active.pop();
        let made = made?;
        self.memo.insert(e, made);
        Ok(made)
    }

    fn named_content(
        &mut self,
        node: roxmltree::Node<'a, 'a>,
        depth: usize,
    ) -> Result<Made, Refusal> {
        // A pattern, gradient or filter with nothing of its own is what it links to.
        let mut node = node;
        for _ in 0..MAX_NESTED_USE {
            let next = href(node)
                .and_then(|id| self.ids.get(id))
                .and_then(|&id| self.doc.get_node(id));
            match next {
                Some(next)
                    if !node.children().any(|c| c.is_element())
                        && next.tag_name() == node.tag_name() =>
                {
                    node = next
                }
                _ => break,
            }
        }
        let tag = node.tag_name().name();
        Ok(match tag {
            "clipPath" | "mask" => self.content(node, None, depth)?,
            "pattern" => Made {
                numbers: 0,
                ..self.content(node, None, depth)?
            },
            "filter" => {
                let mut total = Made::default();
                for fe in node
                    .children()
                    .filter(|c| c.has_tag_name((SVG_NS, "feImage")))
                {
                    if let Some(&t) = href(fe).and_then(|id| self.ids.get(id)) {
                        let t = self.doc.get_node(t).ok_or(Refusal::Broken)?;
                        total = total.plus(self.content(t, None, depth)?);
                    }
                    if self.limit.passed_by(&total) {
                        return Err(Refusal::Complex);
                    }
                }
                total
            }
            _ => Made {
                numbers: node.children().filter(|c| c.is_element()).count(),
                ..Made::default()
            },
        })
    }

    /// The `<marker>` that `property` names for shape `n`: the nearest of it and the elements it
    /// is in that sets it (it is inherited), in `style` or as an attribute (`marker` in `style`
    /// sets all three).
    fn marker_of(
        &mut self,
        n: roxmltree::Node<'a, 'a>,
        property: &'static str,
    ) -> Option<roxmltree::NodeId> {
        let value = self.inherited(n, property)?;
        let id = *self.urls(value).first()?;
        let marker = self.doc.get_node(*self.ids.get(id)?)?;
        marker.has_tag_name((SVG_NS, "marker")).then(|| marker.id())
    }
}

/// A clip path, mask or filter that usvg converts once, whatever the elements that have it: all
/// its units are user space (a clip path's content, and a mask's, are by default).
fn shared(n: roxmltree::Node) -> bool {
    let units = |name| n.attribute(name);
    let user = |name, default: bool| units(name).map_or(default, |v| v == "userSpaceOnUse");
    match n.tag_name().name() {
        "clipPath" => user("clipPathUnits", true),
        "mask" => user("maskUnits", false) && user("maskContentUnits", true),
        "filter" => user("filterUnits", false) && user("primitiveUnits", true),
        _ => false,
    }
}

/// What an element's `href` (or `xlink:href`) links to inside the file: the id.
fn href<'a>(n: roxmltree::Node<'a, '_>) -> Option<&'a str> {
    n.attribute(("http://www.w3.org/1999/xlink", "href"))
        .or_else(|| n.attribute("href"))?
        .strip_prefix('#')
}

/// What `property` is set to for `n` itself: in its `style` or as an attribute.
fn property_of<'a>(n: roxmltree::Node<'a, '_>, property: &str) -> Option<&'a str> {
    style_property(n.attribute("style").unwrap_or(""), property).or_else(|| n.attribute(property))
}

/// The ids in the `url(#id)`s of a value.
fn url_ids(value: &str) -> impl Iterator<Item = &str> {
    value.split("url(").skip(1).filter_map(|rest| {
        rest.split(')')
            .next()?
            .trim()
            .trim_matches(['"', '\''])
            .trim()
            .strip_prefix('#')
    })
}

/// The last value `property` (or `marker`, which sets the three marker properties) is given in
/// the declarations of a `style` attribute.
fn style_property<'a>(style: &'a str, property: &str) -> Option<&'a str> {
    style
        .split(';')
        .filter_map(|d| d.split_once(':'))
        .filter(|(name, _)| {
            let name = name.trim();
            name.eq_ignore_ascii_case(property)
                || (property.starts_with("marker-") && name.eq_ignore_ascii_case("marker"))
        })
        .map(|(_, value)| value.trim().trim_end_matches("!important").trim_end())
        .next_back()
}

/// How many vertices shape `n` has, at most, by its data; `None` if it isn't a shape usvg draws
/// markers on.
fn vertices(n: roxmltree::Node) -> Option<usize> {
    if n.tag_name().namespace() != Some(SVG_NS) {
        return None;
    }
    let data = |name| n.attribute(name).unwrap_or("").as_bytes();
    Some(match n.tag_name().name() {
        "path" => path_vertices(data("d")),
        "polyline" | "polygon" => numbers(data("points")) / 2 + 1,
        "line" => 2,
        "rect" | "circle" | "ellipse" => SHAPE_VERTICES,
        _ => return None,
    })
}

/// How many vertices path data `d` has, at most: each segment (a command and its numbers, as many
/// segments as its numbers make, a curve of six having one at its end) and a vertex for each `z`.
fn path_vertices(d: &[u8]) -> usize {
    // The numbers of each segment of the command that starts at `from`.
    let segments = |command: u8, from: usize, to: usize| {
        let n = numbers(&d[from..to]);
        match command.to_ascii_lowercase() {
            b'z' => 1,
            // No numbers, no segment; an odd one over is part of the last.
            b'm' | b'l' | b't' => n.div_ceil(2),
            b'h' | b'v' => n,
            b'c' => n.div_ceil(6),
            b's' | b'q' => n.div_ceil(4),
            // (Its flags may be written without a space after them, and count as part of the next.)
            b'a' => n.div_ceil(4),
            // Numbers before any command, or what isn't path data: one for each, at most.
            _ => n,
        }
    };
    // `e` and `E` are exponents, not commands.
    let command = |c: &u8| c.is_ascii_alphabetic() && !matches!(c, b'e' | b'E');
    let mut total = 0;
    let mut start = (b'?', 0);
    for (i, c) in d.iter().enumerate().filter(|(_, c)| command(c)) {
        total += segments(start.0, start.1, i);
        start = (*c, i + 1);
    }
    total + segments(start.0, start.1, d.len())
}

/// Which of `marker-start`, `marker-mid` and `marker-end` the style sheet `css` sets (`marker`
/// sets all three): a declaration of them (after a `{` or a `;`), not a class with the name.
fn sets_markers(css: &str) -> [bool; 3] {
    let css = css.to_ascii_lowercase();
    let b = css.as_bytes();
    let mut sets = [false; 3];
    for (at, _) in css.match_indices("marker") {
        let after = &css[at + "marker".len()..];
        let name = ["-start", "-mid", "-end", ""]
            .iter()
            .position(|suffix| after.starts_with(suffix))
            .unwrap_or(3);
        let declared = after[[6, 4, 4, 0][name]..].trim_start().starts_with(':');
        let first = b[..at]
            .iter()
            .rev()
            .find(|c| !c.is_ascii_whitespace())
            .is_some_and(|c| matches!(c, b'{' | b';' | b'/'));
        if declared && first {
            match name {
                3 => sets = [true; 3],
                i => sets[i] = true,
            }
        }
    }
    sets
}

// ---- Style sheets ----------------------------------------------------------------------------

/// The style sheet of `doc` as usvg reads it: its `<style>` elements' text (of type `text/css`,
/// or with none), rule by rule.
fn style_sheet<'a>(doc: &'a roxmltree::Document<'a>) -> simplecss::StyleSheet<'a> {
    let mut sheet = simplecss::StyleSheet::new();
    for n in doc.descendants().filter(|n| n.has_tag_name("style")) {
        if n.attribute("type").is_some_and(|t| t != "text/css") {
            continue;
        }
        if let Some(text) = n.text() {
            sheet.parse_more(text);
        }
    }
    sheet
}

/// A step of matching a selector: what usvg does to answer what it asks of an element.
const MATCH_STEP: u64 = 1;
/// The steps of what a matched rule's declaration costs usvg (its name looked up, its value
/// parsed, and the element's attributes searched for the one it sets).
const DECLARATION_STEPS: u64 = 12;

/// An element as usvg gives it to simplecss (`parse.rs`'s `XmlNode`), counting what it is asked
/// for: a descendant combinator walks up the elements and backtracks, as many ways as it has
/// ancestors to choose each part from. Once it has asked for more than `cap`, it has no parent
/// and no sibling, and the matching ends at once: as it does when told to stop (looked at every
/// [`CHECK_STEPS`] steps).
struct Probe<'a, 'b> {
    node: roxmltree::Node<'a, 'a>,
    steps: &'b Cell<u64>,
    cap: u64,
    cancel: &'b AtomicBool,
    /// How many elements of the selector's it was asked from: matching recurses for each (a
    /// selector of 20,000 adjacent siblings overflows the stack).
    depth: usize,
}

/// How deep matching may go, at most: the components a selector may match, as it recurses for
/// each. More is refused (a style sheet never has so long a selector).
const MAX_MATCH_DEPTH: usize = 256;

/// Steps between a [`Probe`]'s looks at whether it is told to stop.
const CHECK_STEPS: u64 = 1 << 16;

impl<'a, 'b> Probe<'a, 'b> {
    /// Takes `n` steps; `false` when past the cap.
    fn step(&self, n: u64) -> bool {
        let before = self.steps.get();
        let mut steps = before.saturating_add(n);
        if steps / CHECK_STEPS != before / CHECK_STEPS && cancelled(self.cancel) {
            steps = u64::MAX;
        }
        self.steps.set(steps);
        steps <= self.cap
    }

    fn at(&self, node: roxmltree::Node<'a, 'a>) -> Probe<'a, 'b> {
        Probe {
            node,
            steps: self.steps,
            cap: self.cap,
            cancel: self.cancel,
            depth: self.depth + 1,
        }
    }

    /// Asked for an element to go on: `false`, and the work is past the cap, when that is too deep.
    fn deeper(&self) -> bool {
        if self.depth >= MAX_MATCH_DEPTH {
            self.steps.set(u64::MAX);
        }
        self.depth < MAX_MATCH_DEPTH
    }
}

impl simplecss::Element for Probe<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        if !self.deeper() || !self.step(MATCH_STEP) {
            return None;
        }
        self.node.parent_element().map(|n| self.at(n))
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        if !self.deeper() {
            return None;
        }
        // Past the nodes between (comments, instructions, text) that it skips.
        let mut skipped = 0;
        let prev = self.node.prev_siblings().skip(1).find(|n| {
            skipped += 1;
            n.is_element()
        });
        if !self.step(MATCH_STEP + skipped) {
            return None;
        }
        prev.map(|n| self.at(n))
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        self.step(MATCH_STEP) && self.node.tag_name().name() == local_name
    }

    fn attribute_matches(&self, local_name: &str, operator: simplecss::AttributeOperator) -> bool {
        // Its attributes are searched one by one, and a value for a word in a list.
        let value = self.node.attribute(local_name);
        let long = value.map_or(0, |v| v.len() as u64 / 8);
        self.step(MATCH_STEP + self.node.attributes().len() as u64 + long)
            && value.is_some_and(|v| operator.matches(v))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass) -> bool {
        match class {
            simplecss::PseudoClass::FirstChild => {
                simplecss::Element::prev_sibling_element(self).is_none()
            }
            _ => {
                self.step(MATCH_STEP);
                false
            }
        }
    }
}

/// The part of the steps of a style sheet one element may take, at most.
const ELEMENT_SHARE: u64 = 64;

/// What matching a style sheet takes and does, by element (indexed by node): steps, and
/// declarations that apply.
#[derive(Default)]
struct Matched {
    steps: Vec<u32>,
    applied: Vec<u32>,
}

/// The steps of what usvg does matching `sheet`'s rules to each element of `doc` (indexed by
/// node), by doing it as it does with a [`Probe`]; their sum at most `cap`: `Err` past it, or
/// when told to stop. A rule that matches costs its declarations as well. usvg does the same for
/// each copy of an element that `<use>` makes (its ancestors, which the selectors are matched on,
/// are the original's).
fn css_costs(
    doc: &roxmltree::Document,
    sheet: &simplecss::StyleSheet,
    cap: usize,
    cancel: &AtomicBool,
) -> Result<Matched, Refusal> {
    if sheet.rules.is_empty() {
        return Ok(Matched::default());
    }
    let nodes = doc.descendants().count();
    let mut costs = Matched {
        steps: vec![0u32; nodes],
        applied: vec![0u32; nodes],
    };
    let cap = cap as u64;
    let steps = Cell::new(0u64);
    for n in doc.descendants().filter(roxmltree::Node::is_element) {
        if cancelled(cancel) {
            return Err(Refusal::Complex);
        }
        let before = steps.get();
        // No one element may take more than a share: a selector with many descendant
        // combinators takes all it is let on the first deep element, and that is at once.
        let probe = Probe {
            node: n,
            steps: &steps,
            cap: cap.min(before.saturating_add(cap / ELEMENT_SHARE)),
            cancel,
            depth: 0,
        };
        for rule in &sheet.rules {
            if !probe.step(MATCH_STEP) {
                return Err(Refusal::Complex);
            }
            if rule.selector.matches(&probe) {
                probe.step(DECLARATION_STEPS * rule.declarations.len() as u64);
                let applied = &mut costs.applied[n.id().get_usize()];
                *applied = applied.saturating_add(rule.declarations.len() as u32);
            }
            if steps.get() > probe.cap {
                return Err(Refusal::Complex);
            }
        }
        costs.steps[n.id().get_usize()] = (steps.get() - before).min(u32::MAX as u64) as u32;
    }
    Ok(costs)
}

// ---- Parsing --------------------------------------------------------------------------------

/// What a job is let hold, and what parsing has counted so far (nested SVGs add theirs).
struct Budget {
    /// Bytes the job may hold for parsing and rendering besides the canvas and result.
    room: usize,
    /// What parsing holds, as counted so far.
    parse: usize,
    /// Parsing needs more than `room` (a nested SVG wasn't parsed for it).
    over: bool,
    /// Refused for a cap, not for room.
    refused: Option<Refusal>,
    /// Raster images in it so far ([`MAX_RASTERS`]), and what decoding the dearest holds whatever
    /// the box ([`raster_cost`]).
    rasters: usize,
    raster: usize,
    /// A raster image was left out for VOMD's limits.
    skipped: bool,
    /// The work that can't be stopped, counted so far ([`Caps::css`], [`Caps::shaping`]).
    css: usize,
    shaping: usize,
    caps: Caps,
}

impl Budget {
    /// `room` for parsing, `parse` of it taken.
    fn new(room: usize, parse: usize, caps: Caps) -> Mutex<Budget> {
        Mutex::new(Budget {
            caps,
            room,
            parse,
            over: false,
            refused: None,
            rasters: 0,
            raster: 0,
            skipped: false,
            css: 0,
            shaping: 0,
        })
    }
}

/// The XML of an SVG's bytes: the bytes themselves, or an svgz inflated (`header`'s size, at
/// most `max`).
fn xml_of<'a>(
    bytes: &'a [u8],
    header: &SvgCost,
    buf: &'a mut Vec<u8>,
    max: usize,
) -> Option<&'a [u8]> {
    if !header.gz {
        return (bytes.len() <= max).then_some(bytes);
    }
    let want = header.xml as usize;
    if want > max {
        return None;
    }
    buf.reserve_exact(want);
    let mut z = flate2::read::GzDecoder::new(bytes).take(want as u64 + 1);
    z.read_to_end(buf).ok()?;
    (buf.len() == want).then_some(&buf[..])
}

/// The usvg tree of the SVG `xml`, nested `depth` SVGs deep, counting what it holds into `budget`.
fn parse(
    xml: &[u8],
    budget: &Mutex<Budget>,
    depth: usize,
    cancel: &AtomicBool,
    fonts: Option<&Built>,
) -> Option<usvg::Tree> {
    let text = std::str::from_utf8(xml).ok();
    let refuse = |why| {
        let mut b = budget.lock().unwrap_or_else(|e| e.into_inner());
        // An SVG in an SVG that can't be drawn is left out, and the rest is drawn: for a cap of
        // VOMD's, a click draws it with it (as for a raster image), for broken data nothing does.
        match (depth, why) {
            (0, why) => {
                b.refused.get_or_insert(why);
            }
            (_, Refusal::Broken) => {}
            _ => b.skipped = true,
        }
        None
    };
    let Some(text) = text else {
        return refuse(Refusal::Broken);
    };
    let caps = budget.lock().unwrap_or_else(|e| e.into_inner()).caps;
    let s = match scan_in(xml, &caps) {
        Ok(s) => s,
        Err(why) => return refuse(why),
    };
    if s.elements > caps.nodes as usize {
        return refuse(Refusal::Complex);
    }
    {
        let mut b = budget.lock().unwrap_or_else(|e| e.into_inner());
        b.parse = b.parse.saturating_add(parse_cost(&s, 0, 0));
        if b.parse > b.room {
            b.over = true;
            return None;
        }
    }
    if cancelled(cancel) {
        return None;
    }
    let opt = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: caps.nodes,
        ..Default::default()
    };
    let doc = match roxmltree::Document::parse_with_options(text, opt) {
        Ok(doc) => doc,
        Err(roxmltree::Error::NodesLimitReached) => return refuse(Refusal::Complex),
        Err(_) => return refuse(Refusal::Broken),
    };
    // What usvg does matching the style sheet's selectors can't be stopped: it is counted by doing
    // as usvg does, and ended where it passes what is let (a forced load's too).
    let (css_left, shaping_left) = {
        let b = budget.lock().unwrap_or_else(|e| e.into_inner());
        (
            caps.css.saturating_sub(b.css),
            caps.shaping.saturating_sub(b.shaping),
        )
    };
    let costs = match css_costs(&doc, &style_sheet(&doc), css_left, cancel) {
        Ok(costs) => costs,
        Err(why) => return refuse(why),
    };
    // What `<use>`, markers and references make is in proportion to the file, so the tree is too
    // (a forced load's, to what fits its room).
    let limit = {
        let b = budget.lock().unwrap_or_else(|e| e.into_inner());
        Limit::of(
            xml.len(),
            &caps,
            b.room.saturating_sub(b.parse),
            css_left,
            shaping_left,
        )
    };
    let extra = match instances(&doc, &limit, &costs, cancel) {
        Ok(n) => n,
        Err(_) if limit.stopped.get() && !caps.work => {
            // It makes more than there is room for: counted no further.
            let mut b = budget.lock().unwrap_or_else(|e| e.into_inner());
            b.parse = b.room.saturating_add(1);
            b.over = true;
            return None;
        }
        Err(why) => return refuse(why),
    };
    {
        let mut b = budget.lock().unwrap_or_else(|e| e.into_inner());
        b.css = b.css.saturating_add(extra.css);
        b.shaping = b.shaping.saturating_add(extra.shaping);
        b.parse = b.parse.saturating_add(extra.cost(&s));
        if b.parse > b.room {
            b.over = true;
            return None;
        }
    }
    if cancelled(cancel) {
        return None;
    }
    let opt = options(budget, depth, cancel, fonts);
    usvg::Tree::from_xmltree(&doc, &opt).ok()
}

/// How many `href`s that aren't `data:` URLs were asked for (and refused), for the tests.
#[cfg(test)]
static RESOLVED_STRINGS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// usvg's options: no file is read and nothing fetched (`href`s that aren't `data:` URLs are
/// dropped, there is no resources directory or style sheet), and an SVG in a `data:` URL is
/// parsed as this one is, within the same budget, at most [`MAX_NESTED`] deep.
fn options<'a>(
    budget: &'a Mutex<Budget>,
    depth: usize,
    cancel: &'a AtomicBool,
    fonts: Option<&'a Built>,
) -> usvg::Options<'a> {
    let resolve_data = move |mime: &str, data: Arc<Vec<u8>>, _: &usvg::Options| {
        let svg = mime == "image/svg+xml" || root_tag(&data[..data.len().min(SNIFF)]).is_some();
        if !svg {
            return raster(data, budget);
        }
        if depth + 1 > MAX_NESTED {
            budget.lock().unwrap_or_else(|e| e.into_inner()).skipped = true;
            return None;
        }
        parse(&data, budget, depth + 1, cancel, fonts).map(usvg::ImageKind::SVG)
    };
    // Text is set in VOMD's faces (none: it isn't drawn).
    let (fontdb, font_family) = match fonts {
        Some(f) => (f.db.clone(), f.sans.clone()),
        None => Default::default(),
    };
    usvg::Options {
        resources_dir: None,
        font_size: FONT_SIZE,
        font_family,
        fontdb,
        font_resolver: usvg::FontResolver {
            select_font: Box::new(select_font),
            select_fallback: usvg::FontResolver::default_fallback_selector(),
        },
        default_size: usvg::Size::from_wh(DEFAULT_SIDE, DEFAULT_SIDE).unwrap(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(resolve_data),
            resolve_string: Box::new(|_, _| {
                #[cfg(test)]
                RESOLVED_STRINGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                None
            }),
        },
        style_sheet: None,
        ..usvg::Options::default()
    }
}

// ---- Raster images ---------------------------------------------------------------------------

/// Raster images an SVG (with the SVGs in it) draws, at most.
const MAX_RASTERS: usize = 64;

/// The raster image in a `data:` URL, if it is one VOMD decodes (by its bytes, whatever the
/// URL's type says) within its limits, and the SVG has fewer than [`MAX_RASTERS`] so far:
/// counted into `budget`. Bytes that aren't an image draw nothing; an image over the limits
/// draws nothing and is noted ([`Budget::skipped`]).
fn raster(data: Arc<Vec<u8>>, budget: &Mutex<Budget>) -> Option<usvg::ImageKind> {
    let h = super::info_of(&data)?;
    let mut b = budget.lock().unwrap_or_else(|e| e.into_inner());
    if b.rasters >= b.caps.rasters || super::refusal(&h, &b.caps.limits).is_some() {
        b.skipped = true;
        return None;
    }
    b.rasters += 1;
    b.raster = b.raster.max(raster_cost(&h));
    Some(match h.format {
        Format::Png => usvg::ImageKind::PNG(data),
        Format::Jpeg => usvg::ImageKind::JPEG(data),
        Format::Gif => usvg::ImageKind::GIF(data),
        Format::Webp => usvg::ImageKind::WEBP(data),
        Format::Svg => return None,
    })
}

/// What decoding a raster image in an SVG holds whatever its box (see `decode_cost`): its
/// decoder, and the resampler's tables but for the box's share (no more than the margin of a
/// pixmap: see `margin`). Its pixels, at most the canvas's, are [`RASTER_LAYERS`]; one drawn
/// whole has all it holds in [`whole_cost`], whatever the box.
fn raster_cost(h: &Header) -> usize {
    super::header_cost(h)
        .saturating_add(4 * (h.w as usize + h.h as usize))
        .saturating_add(64)
}

/// What drawing raster images holds per pixel of the canvas, in [`LAYER_UNITS`]: the decoded
/// image (at most the canvas), its copy turned by its EXIF orientation, and the pixmap resvg is
/// given; and for each pixel of the canvas's sides, the resampler's tables (in the margin of one
/// more pixmap).
const RASTER_LAYERS: u32 = 3 * LAYER_UNITS;

/// Whether resvg draws a raster image of `rendering` by nearest neighbor.
fn is_nearest(rendering: usvg::ImageRendering) -> bool {
    matches!(
        rendering,
        usvg::ImageRendering::OptimizeSpeed
            | usvg::ImageRendering::CrispEdges
            | usvg::ImageRendering::Pixelated
    )
}

/// Whether the raster image of `h`, drawn as `rendering`, is decoded whole (see [`draw_raster`]),
/// not reduced: one drawn by nearest neighbor, as stored (its orientation not one that swaps the
/// sides), its pixmap limited as a decode is ([`super::clamp_box`], [`super::MAX_PIXELS`]).
/// Accepted limitation: pixel-sharp sampling isn't exact for embedded rasters turned by EXIF 5–8
/// or with a side over 16,384 px (they are reduced, to fit the box usvg made for them; their
/// pixels are sampled at the pixel grid, not where resvg would pick them).
fn drawn_whole(h: &Header, rendering: usvg::ImageRendering) -> bool {
    is_nearest(rendering)
        && !h.transposed()
        && h.w.max(h.h) <= super::MAX_SIDE
        && u64::from(h.w) * u64::from(h.h) <= super::MAX_PIXELS
}

/// What drawing a raster image whole ([`drawn_whole`]) holds, whatever the box: decoding it at
/// its own size (the resampler's tables and rows included: `decode_cost`), and the pixmap beside
/// the decoded image.
fn whole_cost(header: &Header) -> usize {
    super::decode_cost(header, header.w, header.h)
        .saturating_add(header.w as usize * header.h as usize * 4)
}

/// The pixels of the raster image `data` for resvg to draw under `ts` (from the image's own
/// pixels, as usvg sized it: as stored, its EXIF orientation not applied) on a canvas of
/// `canvas`: decoded at the size it is drawn (no bigger than the canvas, or than the image
/// itself; its pixels can't be more than the canvas's), reduced by averaging, and turned upright.
/// An orientation that swaps the sides is fitted inside the box usvg made for it, a pixmap of the
/// box's size on the canvas: the upright image is fitted in it exactly (its aspect kept, centered,
/// its edges at the pixels nearest to its own) and decoded at that size, so the fit is where it
/// is drawn, not rounded at the stored size and magnified by resvg. One drawn by nearest neighbor
/// is not reduced: resvg picks the pixels under the centers of the ones it draws, where it
/// places them, so any pixels picked here (under the centers of a grid that is not placed) can
/// be ones it doesn't want. It is decoded whole, as large as the image is ([`drawn_whole`]), and
/// that is what [`Analysis::nearest`] charges. `refused` is set if the decode is refused for
/// `limits` (a click draws it); broken data, or a decode cancelled, draws nothing and sets
/// nothing.
fn draw_raster(
    data: &[u8],
    ts: tiny_skia::Transform,
    rendering: usvg::ImageRendering,
    canvas: (u32, u32),
    limits: &super::Limits,
    cancel: &AtomicBool,
    refused: &Cell<bool>,
) -> Option<tiny_skia::Pixmap> {
    let h = super::info_of(data)?;
    let (sw, sh) = (h.w as f32, h.h as f32);
    let sampling = if is_nearest(rendering) {
        super::Sampling::Nearest
    } else {
        super::Sampling::Area
    };
    let ((tw, th), (bw, bh), (x0, y0)) = if drawn_whole(&h, rendering) {
        ((h.w, h.h), (h.w, h.h), (0, 0))
    } else {
        // Its size on the canvas, along its own sides (turned or skewed as it may be).
        let (sx, sy) = ts.get_scale();
        let (dw, dh) = (sw * sx, sh * sy);
        let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
        let s = ((cw * ch) / (dw * dh))
            .sqrt()
            .min(cw.max(ch) / dw.max(dh))
            .min(1.0);
        // An image that is only reduced isn't made bigger than itself; one turned to fit its box
        // is drawn at the size it is shown, so that the fit isn't rounded at the size it is
        // stored in, and magnified.
        let s = if h.transposed() { s } else { s.min(sw / dw) };
        if !(s > 0.0 && s.is_finite()) {
            return None;
        }
        let max = super::MAX_SIDE as f32;
        if h.transposed() {
            let (tw, th) = (
                (dw * s).round().clamp(1.0, max),
                (dh * s).round().clamp(1.0, max),
            );
            // Shown, it is `sh` × `sw`: fitted in the box exactly, its aspect kept, and centered;
            // the edges are the pixels' nearest to those it has.
            let f = f64::from(tw / sh).min(f64::from(th / sw));
            let edges = |side: f32, shown: f32| {
                let (lo, len) = (
                    (f64::from(side) - f64::from(shown) * f) / 2.0,
                    f64::from(shown) * f,
                );
                let a = lo.round().clamp(0.0, f64::from(side) - 1.0);
                let b = (lo + len).round().clamp(a + 1.0, f64::from(side));
                (a as u32, (b - a) as u32)
            };
            let ((x0, bw), (y0, bh)) = (edges(tw, sh), edges(th, sw));
            ((tw as u32, th as u32), (bw, bh), (x0, y0))
        } else {
            let side = |v: f32| (v * s).ceil().clamp(1.0, max) as u32;
            let (tw, th) = (side(dw), side(dh));
            ((tw, th), (tw, th), (0, 0))
        }
    };
    #[cfg(test)]
    DECODED_AT.with(|d| d.set((bw, bh)));
    let bitmap = match super::decode_why(
        data,
        bw,
        bh,
        sampling,
        limits,
        cancel,
        &mut Fonts::default(),
    ) {
        Ok(bitmap) => bitmap,
        Err(why) => {
            refused.set(refused.get() || why.is_some());
            return None;
        }
    };
    let mut pixmap = tiny_skia::Pixmap::new(tw, th)?;
    let out = pixmap.data_mut();
    for (y, row) in bitmap.data.chunks_exact(bitmap.width as usize).enumerate() {
        let at = ((y0 as usize + y) * tw as usize + x0 as usize) * 4;
        for (p, o) in row.iter().zip(out[at..].as_chunks_mut::<4>().0) {
            let [a, r, g, b] = p.to_be_bytes();
            *o = [r, g, b, a];
        }
    }
    Some(pixmap)
}

#[cfg(test)]
thread_local! {
    /// The size the last raster image was decoded at, for the tests.
    pub(super) static DECODED_AT: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

// ---- Fonts ----------------------------------------------------------------------------------

/// What a face of the font database costs to keep, at most: its names and record.
pub(crate) const FONTDB_COST: usize = 2 << 10;

/// The font database text is set in, made when the first SVG with text needs it, and again when
/// the faces change ([`Fonts::set`]).
#[derive(Default)]
pub(crate) struct Fonts {
    sources: Option<Arc<SvgFonts>>,
    built: Option<(Arc<SvgFonts>, Arc<Built>)>,
}

/// A font database, and the family of body text.
pub(crate) struct Built {
    db: Arc<usvg::fontdb::Database>,
    sans: String,
}

/// How many font databases were made, for the tests.
#[cfg(test)]
pub(super) static BUILT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Fonts {
    /// The faces to set text in from now on.
    pub(crate) fn set(&mut self, sources: Option<Arc<SvgFonts>>) {
        self.sources = sources;
    }

    /// The database of the faces, made if it isn't.
    fn get(&mut self) -> Option<Arc<Built>> {
        let sources = self.sources.as_ref()?;
        if let Some((of, built)) = &self.built
            && Arc::ptr_eq(of, sources)
        {
            return Some(built.clone());
        }
        // The old one goes first.
        self.built = None;
        #[cfg(test)]
        BUILT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut db = usvg::fontdb::Database::new();
        let (mut sans, mut mono) = (String::new(), String::new());
        for (i, (data, index)) in sources.sources.iter().enumerate() {
            let ids = db.load_font_source(usvg::fontdb::Source::Binary(data.clone()));
            let family = ids
                .get(*index as usize)
                .and_then(|&id| db.face(id))
                .and_then(|f| f.families.first())
                .map(|f| f.0.clone());
            if let Some(family) = family {
                if i == sources.sans {
                    sans = family.clone();
                }
                if i == sources.mono {
                    mono = family;
                }
            }
        }
        // The generic families are VOMD's: there is no serif face, or a cursive one.
        db.set_serif_family(sans.clone());
        db.set_sans_serif_family(sans.clone());
        db.set_cursive_family(sans.clone());
        db.set_fantasy_family(sans.clone());
        db.set_monospace_family(mono);
        let built = Arc::new(Built {
            db: Arc::new(db),
            sans,
        });
        self.built = Some((sources.clone(), built.clone()));
        Some(built)
    }
}

/// The face for `font`: the first of its families the database has; if none, code's for a
/// name that looks like a monospace family (`Courier`, `Consolas`, `... Mono`, `... Code`) and
/// body text's for any other (`Arial`, `Verdana`), so that a badge's text is drawn.
fn select_font(
    font: &usvg::Font,
    db: &mut Arc<usvg::fontdb::Database>,
) -> Option<usvg::fontdb::ID> {
    use usvg::fontdb::{Family, Query, Stretch, Style};
    let mut monoish = false;
    let mut families: Vec<Family> = font
        .families()
        .iter()
        .map(|f| match f {
            usvg::FontFamily::Serif => Family::Serif,
            usvg::FontFamily::SansSerif => Family::SansSerif,
            usvg::FontFamily::Cursive => Family::Cursive,
            usvg::FontFamily::Fantasy => Family::Fantasy,
            usvg::FontFamily::Monospace => Family::Monospace,
            usvg::FontFamily::Named(s) => {
                let lower = s.to_ascii_lowercase();
                monoish |= ["mono", "courier", "consol", "code"]
                    .iter()
                    .any(|m| lower.contains(m));
                Family::Name(s)
            }
        })
        .collect();
    families.push(if monoish {
        Family::Monospace
    } else {
        Family::SansSerif
    });
    let stretch = match font.stretch() {
        usvg::FontStretch::UltraCondensed => Stretch::UltraCondensed,
        usvg::FontStretch::ExtraCondensed => Stretch::ExtraCondensed,
        usvg::FontStretch::Condensed => Stretch::Condensed,
        usvg::FontStretch::SemiCondensed => Stretch::SemiCondensed,
        usvg::FontStretch::Normal => Stretch::Normal,
        usvg::FontStretch::SemiExpanded => Stretch::SemiExpanded,
        usvg::FontStretch::Expanded => Stretch::Expanded,
        usvg::FontStretch::ExtraExpanded => Stretch::ExtraExpanded,
        usvg::FontStretch::UltraExpanded => Stretch::UltraExpanded,
    };
    let style = match font.style() {
        usvg::FontStyle::Normal => Style::Normal,
        usvg::FontStyle::Italic => Style::Italic,
        usvg::FontStyle::Oblique => Style::Oblique,
    };
    db.query(&Query {
        families: &families,
        weight: usvg::fontdb::Weight(font.weight()),
        stretch,
        style,
    })
}

// ---- Rendering ------------------------------------------------------------------------------

/// What a job of an SVG made.
pub(crate) enum Rendered {
    /// Drawn; `true` if raster images in it were left out for VOMD's limits.
    Pixels(Bitmap, bool),
    /// It needs more than it was charged: this, recorded, is what it is charged next.
    Need(SvgCost),
    Refused(Refusal),
    /// Given up: told to stop.
    Cancelled,
}

/// The SVG `bytes` (`svg` its header's cost) drawn to `w` × `h`, fitted uniformly and
/// centered, by a job charged `charge` bytes (what [`cost`] says), all told.
pub(super) fn render(
    bytes: &[u8],
    svg: &SvgCost,
    size: (u32, u32),
    charge: usize,
    cancel: &AtomicBool,
    fonts: &mut Fonts,
) -> Rendered {
    render_in(bytes, svg, size, charge, &Caps::NORMAL, cancel, fonts)
}

/// [`render_in`] within the caps of `limits`, in a process of its own if SVGs are drawn apart
/// ([`process::set_program`]): one that takes too long, or isn't wanted any more (`cancel`), is
/// killed. `inline` is set while the drawing is VOMD's own process's after all (a child couldn't
/// be started), and cleared otherwise.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    bytes: &[u8],
    svg: &SvgCost,
    size: (u32, u32),
    charge: usize,
    limits: &super::Limits,
    cancel: &AtomicBool,
    inline: &AtomicBool,
    fonts: &mut Fonts,
) -> Rendered {
    #[cfg(any(target_os = "linux", windows))]
    if let Some(program) = process::program() {
        return process::draw(
            program, bytes, svg, size, charge, limits, cancel, inline, fonts,
        );
    }
    let _ = inline;
    render_in(bytes, svg, size, charge, &Caps::of(limits), cancel, fonts)
}

/// Whether SVGs are drawn in processes of their own, which are killed when they are given up.
pub(super) fn apart() -> bool {
    #[cfg(any(target_os = "linux", windows))]
    return process::program().is_some();
    #[cfg(not(any(target_os = "linux", windows)))]
    false
}

/// [`render`] within `caps`.
pub(super) fn render_in(
    bytes: &[u8],
    svg: &SvgCost,
    (w, h): (u32, u32),
    charge: usize,
    caps: &Caps,
    cancel: &AtomicBool,
    fonts: &mut Fonts,
) -> Rendered {
    let svg = known(svg);
    // What parsing may hold: the charge, less the rest of it.
    let rest = cost(&svg, w, h) - svg.parse;
    let mut inflated = Vec::new();
    let Some(xml) = xml_of(bytes, &svg, &mut inflated, caps.xml) else {
        return Rendered::Refused(Refusal::Broken);
    };
    let copy = if svg.gz { svg.xml as usize } else { 0 };
    let budget = Budget::new(charge.saturating_sub(rest), copy, *caps);
    // The font database is made for text only (an SVG in this one may have some).
    let text = scan_in(xml, caps).is_ok_and(|s| s.has_text) || find(xml, b"svg+xml").is_some();
    let fonts = if text { fonts.get() } else { None };
    let tree = parse(xml, &budget, 0, cancel, fonts.as_deref());
    let b = budget.into_inner().unwrap_or_else(|e| e.into_inner());
    if cancelled(cancel) {
        return Rendered::Cancelled;
    }
    if let Some(why) = b.refused {
        return Rendered::Refused(why);
    }
    let mut need = SvgCost {
        parse: b.parse.max(svg.parse),
        raster: b.raster.max(svg.raster),
        ..svg
    };
    if b.over {
        return Rendered::Need(need);
    }
    let Some(tree) = tree else {
        return Rendered::Refused(Refusal::Broken);
    };
    let transform = fit(&tree, w, h);
    let canvas = w as usize * h as usize * 4;
    let mut a = Analysis::of(&tree, transform, w, h);
    if b.rasters > 0 {
        // A raster image's decode and pixmap, beside the layers it is drawn in (what one drawn
        // whole holds, whatever the box, is a fixed charge: not a number of layers).
        a.peak = a
            .peak
            .saturating_add(canvas.saturating_mul(RASTER_LAYERS as usize) / LAYER_UNITS as usize);
        a.pixmaps += 1;
        need.raster = need.raster.max(a.nearest);
    }
    let margins = margin(w, h).saturating_mul(a.pixmaps);
    let layers = (a.peak.saturating_sub(margins))
        .saturating_mul(LAYER_UNITS as usize)
        .div_ceil(canvas);
    if caps.work && a.work > MAX_WORK {
        return Rendered::Refused(Refusal::Slow);
    }
    need.draw = need.draw.max(a.draw);
    need.layers = need.layers.max(layers.min(u32::MAX as usize) as u32);
    need.pixmaps = need.pixmaps.max(a.pixmaps.min(u32::MAX as usize) as u32);
    if cost(&need, w, h) > charge {
        return Rendered::Need(need);
    }
    let Some(mut pixmap) = tiny_skia::Pixmap::new(w, h) else {
        return Rendered::Refused(Refusal::Broken);
    };
    // A raster image refused when it came to be decoded (it passed the checks of its header).
    let refused = Cell::new(false);
    let raster = |kind: &usvg::ImageKind,
                  ts: tiny_skia::Transform,
                  rendering: usvg::ImageRendering| match kind {
        usvg::ImageKind::PNG(d)
        | usvg::ImageKind::JPEG(d)
        | usvg::ImageKind::GIF(d)
        | usvg::ImageKind::WEBP(d) => {
            draw_raster(d, ts, rendering, (w, h), &caps.limits, cancel, &refused)
        }
        usvg::ImageKind::SVG(_) => None,
    };
    let hooks = resvg::Hooks {
        raster: &raster,
        cancel,
    };
    resvg::render_with(&tree, transform, &mut pixmap.as_mut(), &hooks);
    drop(tree);
    if cancelled(cancel) {
        return Rendered::Cancelled;
    }
    // Premultiplied RGBA to `0xAARRGGBB`.
    let data = pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[r, g, b, a]| u32::from_be_bytes([a, r, g, b]))
        .collect();
    let bitmap = Bitmap {
        width: w,
        height: h,
        data,
    };
    Rendered::Pixels(bitmap, b.skipped || refused.get())
}

/// The transform that draws `tree` in a box of `w` × `h`: scaled uniformly to fit, and centered
/// (its size is the box's but where usvg and the header disagree).
fn fit(tree: &usvg::Tree, w: u32, h: u32) -> tiny_skia::Transform {
    let size = tree.size();
    let (bw, bh) = (w as f32, h as f32);
    let s = (bw / size.width()).min(bh / size.height());
    tiny_skia::Transform::from_row(
        s,
        0.0,
        0.0,
        s,
        (bw - size.width() * s) / 2.0,
        (bh - size.height() * s) / 2.0,
    )
}

/// What rendering a tree holds at once besides its canvas, from a walk that follows resvg's:
/// each isolated group's layer (its bounding box on the canvas, 2 pixels more each side, within
/// resvg's limit of 5 canvases a side) while its children render into it, and beside it what
/// its filters, clip path and mask make; patterns' tiles; nested SVGs' canvases; and apart from
/// those, the largest path's drawing.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Analysis {
    /// Bytes of pixmaps held at once, at most.
    pub peak: usize,
    /// How many pixmaps those are.
    pub pixmaps: usize,
    /// What drawing the largest path holds ([`FILL_PER_POINT`], [`STROKE_PER_POINT`]).
    pub draw: usize,
    /// The work of drawing, about a nanosecond a unit (see [`MAX_WORK`]).
    pub work: f64,
    /// What drawing the costliest raster image drawn whole ([`drawn_whole`]) holds
    /// ([`whole_cost`]): a charge of the image, not of the canvas (it is part of
    /// [`SvgCost::raster`]).
    pub nearest: usize,
}

/// Pixmaps held at once: bytes, and how many.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Peak(usize, usize);

impl Peak {
    fn plus(self, o: Peak) -> Peak {
        Peak(self.0.saturating_add(o.0), self.1.saturating_add(o.1))
    }
}

/// A pixmap of `w` × `h`.
fn pixmap(w: u32, h: u32) -> Peak {
    Peak(w as usize * h as usize * 4, 1)
}

/// The walk of a tree for [`Analysis`].
struct Walk {
    /// The rectangle resvg keeps layers within: the canvas, 2 canvases more each side.
    max: tiny_skia::IntRect,
    draw: usize,
    /// The work of drawing ([`Analysis::work`]).
    work: f64,
    /// [`Analysis::nearest`].
    nearest: usize,
}

impl Analysis {
    pub(super) fn of(
        tree: &usvg::Tree,
        transform: tiny_skia::Transform,
        w: u32,
        h: u32,
    ) -> Analysis {
        let max =
            tiny_skia::IntRect::from_xywh(-(w as i32) * 2, -(h as i32) * 2, w * 5, h * 5).unwrap();
        let mut walk = Walk {
            max,
            draw: 0,
            work: 0.0,
            nearest: 0,
        };
        let Peak(peak, pixmaps) = walk.group(tree.root(), transform, (w, h));
        Analysis {
            peak,
            pixmaps,
            draw: walk.draw,
            work: walk.work,
            nearest: walk.nearest,
        }
    }
}

impl Walk {
    /// What rendering the children of `g` holds at once, drawn under `transform` into a pixmap
    /// of `target`.
    fn children(
        &mut self,
        g: &usvg::Group,
        transform: tiny_skia::Transform,
        target: (u32, u32),
    ) -> Peak {
        let mut most = Peak::default();
        for n in g.children() {
            most = most.max(self.node(n, transform, target));
        }
        most
    }

    fn node(
        &mut self,
        n: &usvg::Node,
        transform: tiny_skia::Transform,
        target: (u32, u32),
    ) -> Peak {
        match n {
            usvg::Node::Group(g) => self.group(g, transform, target),
            usvg::Node::Path(p) => {
                self.path(p, transform, target);
                let fill = p.fill().map(usvg::Fill::paint);
                let stroke = p.stroke().map(usvg::Stroke::paint);
                let mut most = Peak::default();
                for paint in [fill, stroke].into_iter().flatten() {
                    most = most.max(self.paint(paint, transform));
                }
                most
            }
            usvg::Node::Image(i) => match i.kind() {
                // Decoded, and drawn on what it covers.
                usvg::ImageKind::PNG(d)
                | usvg::ImageKind::JPEG(d)
                | usvg::ImageKind::GIF(d)
                | usvg::ImageKind::WEBP(d) => {
                    let size = i.size();
                    let source = f64::from(size.width()) * f64::from(size.height());
                    let drawn = size
                        .to_non_zero_rect(0.0, 0.0)
                        .transform(transform)
                        .map_or(0.0, |r| area_in(r.to_rect(), target));
                    self.work += 2.0 * source.min(MAX_PIXELS_F) + 4.0 * drawn;
                    if let Some(h) = super::info_of(d)
                        && drawn_whole(&h, i.rendering_mode())
                    {
                        self.nearest = self.nearest.max(whole_cost(&h));
                    }
                    Peak::default()
                }
                // A nested SVG renders to a canvas of its own, the target's size.
                usvg::ImageKind::SVG(t) => {
                    let inner = self.group(t.root(), transform, target);
                    pixmap(target.0, target.1).plus(inner)
                }
            },
            usvg::Node::Text(t) => self.group(t.flattened(), transform, target),
        }
    }

    /// What drawing path `p` holds ([`Analysis::draw`]) and its work.
    fn path(&mut self, p: &usvg::Path, transform: tiny_skia::Transform, target: (u32, u32)) {
        let points = p.data().points().len();
        let per_point = if p.stroke().is_some() {
            STROKE_PER_POINT
        } else {
            FILL_PER_POINT
        };
        self.draw = self.draw.max(points.saturating_mul(per_point));
        self.work += path_work(p, transform, target);
    }

    /// A pattern's tile, rendered at the transform's scale, and what rendering it holds.
    fn paint(&mut self, paint: &usvg::Paint, transform: tiny_skia::Transform) -> Peak {
        let usvg::Paint::Pattern(p) = paint else {
            return Peak::default();
        };
        let (sx, sy) = transform.pre_concat(p.transform()).get_scale();
        let r = p.rect();
        let (tw, th) = ((r.width() * sx).round(), (r.height() * sy).round());
        if !(tw >= 1.0 && th >= 1.0) {
            return Peak::default();
        }
        let (tw, th) = (
            tw.min(u32::MAX as f32) as u32,
            th.min(u32::MAX as f32) as u32,
        );
        let ts = tiny_skia::Transform::from_scale(sx, sy);
        self.work += LAYER_WORK * f64::from(tw) * f64::from(th);
        let inner = self.group(p.root(), ts, (tw, th));
        pixmap(tw, th).plus(inner)
    }

    fn group(
        &mut self,
        g: &usvg::Group,
        transform: tiny_skia::Transform,
        target: (u32, u32),
    ) -> Peak {
        let transform = transform.pre_concat(g.transform());
        if !g.should_isolate() {
            return self.children(g, transform, target);
        }
        let Some(bbox) = g.layer_bounding_box().transform(transform) else {
            return Peak::default();
        };
        let ibbox = if g.filters().is_empty() {
            tiny_skia::IntRect::from_xywh(
                bbox.x().floor() as i32 - 2,
                bbox.y().floor() as i32 - 2,
                (bbox.width().ceil() as u32).saturating_add(4),
                (bbox.height().ceil() as u32).saturating_add(4),
            )
        } else {
            Some(bbox.to_int_rect())
        };
        let Some(ibbox) = ibbox.and_then(|r| fit_to_rect(r, self.max)) else {
            return Peak::default();
        };
        let (lw, lh) = (ibbox.width(), ibbox.height());
        self.work += LAYER_WORK * f64::from(lw) * f64::from(lh);
        let shift = tiny_skia::Transform::from_translate(-ibbox.x() as f32, -ibbox.y() as f32);
        let inner = shift.pre_concat(transform);
        let mut most = self.children(g, inner, (lw, lh));
        // Filters keep every primitive's result, and the costliest primitives a copy or two more.
        for f in g.filters() {
            let Some(r) = f.rect().transform(inner) else {
                continue;
            };
            let r = r.to_int_rect();
            let (sx, sy) = inner.get_scale();
            // What an `feImage` draws is drawn in a pixmap of the filter's region, scaled as the
            // layer is, each time the filter is applied.
            let mut content = Peak::default();
            for p in f.primitives() {
                let region = p
                    .rect()
                    .transform(inner)
                    .map_or(0.0, |r| area_in(r.to_rect(), (lw, lh)));
                self.work += region * primitive_work(p.kind(), sx, sy);
                if let usvg::filter::Kind::Image(fe) = p.kind() {
                    let ts = tiny_skia::Transform::from_scale(sx, sy);
                    self.work += LAYER_WORK * f64::from(r.width()) * f64::from(r.height());
                    content = content.max(self.children(fe.root(), ts, (r.width(), r.height())));
                }
            }
            let n = filter_pixmaps(f);
            let Peak(bytes, _) = pixmap(r.width(), r.height());
            most = most.max(Peak(bytes.saturating_mul(n), n).plus(content));
        }
        // A clip path: a pixmap and a mask of the layer's size, for each clip path within it, and
        // what its paths hold as they are filled.
        if let Some(c) = g.clip_path() {
            let c = self.clip(c, inner, (lw, lh));
            most = most.max(c);
        }
        // A mask: its pixmap and two masks of a byte a pixel, what its content holds, and its
        // own mask.
        if let Some(m) = g.mask() {
            let m = self.mask(m, inner, (lw, lh));
            most = most.max(m);
        }
        pixmap(lw, lh).plus(most)
    }

    /// What applying clip path `c` to a layer of `target` holds, its paths drawn under
    /// `transform` as resvg does (each one filled, the path's edges made at once), and the work.
    fn clip(
        &mut self,
        c: &usvg::ClipPath,
        transform: tiny_skia::Transform,
        target: (u32, u32),
    ) -> Peak {
        let (w, h) = target;
        let own = Peak(pixmap(w, h).0 * 2 + w as usize * h as usize, 3);
        self.work += LAYER_WORK * f64::from(w) * f64::from(h);
        let content = self.clipped(c.root(), transform.pre_concat(c.transform()), target);
        let nested = match c.clip_path() {
            Some(n) => self.clip(n, transform, target),
            None => Peak::default(),
        };
        own.plus(content.max(nested))
    }

    /// What drawing the children of `g` into a clip path's pixmap holds at once: a path filled, a
    /// group with a clip path of its own on a pixmap of its own.
    fn clipped(
        &mut self,
        g: &usvg::Group,
        transform: tiny_skia::Transform,
        target: (u32, u32),
    ) -> Peak {
        let mut most = Peak::default();
        for n in g.children() {
            match n {
                usvg::Node::Path(p) if p.is_visible() => self.path(p, transform, target),
                usvg::Node::Path(_) => {}
                usvg::Node::Text(t) => {
                    most = most.max(self.clipped(t.flattened(), transform, target))
                }
                usvg::Node::Group(g) => {
                    let transform = transform.pre_concat(g.transform());
                    most = most.max(match g.clip_path() {
                        Some(c) => {
                            let inner = self.clipped(g, transform, target);
                            let clip = self.clip(c, transform, target);
                            pixmap(target.0, target.1).plus(inner.max(clip))
                        }
                        None => self.clipped(g, transform, target),
                    });
                }
                usvg::Node::Image(_) => {}
            }
        }
        most
    }

    fn mask(
        &mut self,
        m: &usvg::Mask,
        transform: tiny_skia::Transform,
        target: (u32, u32),
    ) -> Peak {
        let own = Peak(
            pixmap(target.0, target.1).0 + 2 * target.0 as usize * target.1 as usize,
            3,
        );
        let content = self.group(m.root(), transform, target);
        let nested = match m.mask() {
            Some(m) => self.mask(m, transform, target),
            None => Peak::default(),
        };
        own.plus(content.max(nested))
    }
}

/// A job whose drawing is more work than this ([`Analysis::work`]) is refused: about 0.3 s.
pub(super) const MAX_WORK: f64 = 3e8;
/// What a pixel of a layer or pattern tile costs to make and draw.
const LAYER_WORK: f64 = 8.0;
/// [`MAX_PIXELS`](super::MAX_PIXELS), for the work of raster images.
const MAX_PIXELS_F: f64 = super::MAX_PIXELS as f64;

/// The part of `r` within a pixmap of `target`, in pixels.
fn area_in(r: tiny_skia::Rect, target: (u32, u32)) -> f64 {
    let w = r.right().min(target.0 as f32) - r.left().max(0.0);
    let h = r.bottom().min(target.1 as f32) - r.top().max(0.0);
    f64::from(w.max(0.0)) * f64::from(h.max(0.0))
}

/// The work of filling (and stroking) path `p` drawn under `ts` into a pixmap of `target`, by
/// what tiny-skia does: each pixel it covers, each edge on each row it crosses (4 rows a pixel,
/// anti-aliased), and the edges reordered between rows, which for edges that cross each other
/// is about the square of how many cross a row (a stroke's outline has twice the path's edges,
/// and crosses itself where the path turns back). By timing: a stroked zig-zag of 8,000
/// segments over itself takes 0.33 s, a thin spiral of as many 7 ms.
fn path_work(p: &usvg::Path, ts: tiny_skia::Transform, target: (u32, u32)) -> f64 {
    // The rows each segment crosses, and the path's extent; its length, for dashes.
    let mut e = Extent {
        rows: 0.0,
        top: f32::MAX,
        bottom: f32::MIN,
        length: 0.0,
        at: tiny_skia::Point::zero(),
    };
    let mut start = e.at;
    let map = |mut q: tiny_skia::Point| {
        ts.map_point(&mut q);
        q
    };
    for seg in p.data().segments() {
        use tiny_skia::PathSegment as S;
        match seg {
            S::MoveTo(q) => {
                e.at = map(q);
                start = e.at;
            }
            S::LineTo(q) => e.to(&[map(q)]),
            S::QuadTo(c, q) => e.to(&[map(c), map(q)]),
            S::CubicTo(c1, c2, q) => e.to(&[map(c1), map(c2), map(q)]),
            S::Close => e.to(&[start]),
        }
    }
    let Extent {
        rows,
        top,
        bottom,
        length,
        ..
    } = e;
    if top > bottom || !rows.is_finite() {
        return 0.0;
    }
    let height = f64::from(bottom - top).max(1.0);
    let visible = f64::from(bottom.min(target.1 as f32) - top.max(0.0)).max(0.0);
    let bounds = p
        .data()
        .bounds()
        .transform(ts)
        .map_or(0.0, |r| area_in(r, target));
    // Edges across a row, on average.
    let active = rows / height;
    let mut work = 0.0;
    let mut paint = |active: f64| {
        work += 4.0 * bounds + 40.0 * 4.0 * visible.min(height) * active + 4.0 * active * active;
    };
    if p.fill().is_some() {
        paint(active);
    }
    if let Some(stroke) = p.stroke() {
        paint(2.0 * active);
        // Each dash is a path of its own: tiny-skia gives up past a million of them.
        if let Some(dashes) = stroke.dasharray() {
            let period: f32 = dashes.iter().sum();
            let (sx, sy) = ts.get_scale();
            let period = f64::from(period * sx.max(sy));
            if period > 0.0 {
                work += (length / period * dashes.len() as f64).min(1e6) * 128.0;
            }
        }
    }
    work
}

/// What [`path_work`] measures of a path, segment by segment, in the canvas's space.
struct Extent {
    rows: f64,
    top: f32,
    bottom: f32,
    length: f64,
    at: tiny_skia::Point,
}

impl Extent {
    /// A segment from where the last ended, through `pts`.
    fn to(&mut self, pts: &[tiny_skia::Point]) {
        let mut ys = (self.at.y, self.at.y);
        for &q in pts {
            ys = (ys.0.min(q.y), ys.1.max(q.y));
            let (dx, dy) = (q.x - self.at.x, q.y - self.at.y);
            self.length += f64::from((dx * dx + dy * dy).sqrt());
            self.at = q;
        }
        self.rows += f64::from(ys.1 - ys.0);
        self.top = self.top.min(ys.0);
        self.bottom = self.bottom.max(ys.1);
    }
}

/// The work of a filter primitive for each pixel of its region, `sx` and `sy` the scale of
/// the canvas: resvg's morphology looks at every pixel of the radius, convolution at every one
/// of the matrix, turbulence goes through every octave for each channel.
fn primitive_work(kind: &usvg::filter::Kind, sx: f32, sy: f32) -> f64 {
    use usvg::filter::Kind as K;
    match kind {
        K::GaussianBlur(_) | K::DropShadow(_) => 16.0,
        K::Morphology(m) => {
            let rx = f64::from((m.radius_x().get() * sx).ceil() * 2.0).max(1.0);
            let ry = f64::from((m.radius_y().get() * sy).ceil() * 2.0).max(1.0);
            10.0 * rx * ry
        }
        K::ConvolveMatrix(c) => {
            4.0 * f64::from(c.matrix().columns()) * f64::from(c.matrix().rows())
        }
        K::Turbulence(t) => 160.0 * f64::from(t.num_octaves()),
        K::DiffuseLighting(_) | K::SpecularLighting(_) => 64.0,
        K::DisplacementMap(_) => 16.0,
        _ => 4.0,
    }
}

/// How many pixmaps of its region resvg holds at once for filter `f`, beside the layer it is
/// applied to: every primitive's result is kept until the last (`apply_inner`'s `results`), and
/// a primitive holds a few more as it makes its own: a copy of the source for an input that is
/// one, a copy for what it changes of an input that is a result (that is kept), a pixmap for its
/// output, and for a drop shadow its blurred copy.
fn filter_pixmaps(f: &usvg::filter::Filter) -> usize {
    use usvg::filter::{CompositeOperator, Input, Kind as K};
    let source = |i: &Input| usize::from(!matches!(i, Input::Reference(_)));
    let mut most = 0;
    for (kept, p) in f.primitives().iter().enumerate() {
        let more = match p.kind() {
            K::Flood(_) | K::Image(_) | K::Turbulence(_) => 1,
            // Made in place of what it takes: a copy of the source, or of a result.
            K::GaussianBlur(_)
            | K::ColorMatrix(_)
            | K::ComponentTransfer(_)
            | K::ConvolveMatrix(_)
            | K::Morphology(_) => 1,
            K::Offset(fe) => source(fe.input()) + 1,
            K::Blend(fe) => source(fe.input1()) + source(fe.input2()) + 1,
            K::Composite(fe) => match fe.operator() {
                CompositeOperator::Arithmetic { .. } => 3,
                _ => source(fe.input1()) + source(fe.input2()) + 1,
            },
            K::Merge(fe) => 1 + fe.inputs().iter().map(source).max().unwrap_or(0),
            K::Tile(fe) => source(fe.input()) + 2,
            K::DiffuseLighting(fe) => source(fe.input()) + 1,
            K::SpecularLighting(fe) => source(fe.input()) + 1,
            K::DisplacementMap(_) | K::DropShadow(_) => 3,
        };
        most = most.max(kept + more);
    }
    most
}

/// resvg's `geom::fit_to_rect`: `r` cut to `max`.
fn fit_to_rect(r: tiny_skia::IntRect, max: tiny_skia::IntRect) -> Option<tiny_skia::IntRect> {
    let left = r.left().max(max.left());
    let top = r.top().max(max.top());
    let right = r.right().min(max.right());
    let bottom = r.bottom().min(max.bottom());
    tiny_skia::IntRect::from_ltrb(left, top, right, bottom)
}

#[cfg(any(target_os = "linux", windows))]
pub(super) mod process;
#[cfg(test)]
pub(super) mod tests;
