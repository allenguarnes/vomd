//! Itemization and shaping. Runs split on face (per-character fallback), font size, script and
//! bidi level, and at inline boxes; each run is shaped by swash in logical order. Neighboring
//! items that differ only in color, link or decoration share one run, so shaping sees the text
//! whole (Arabic letters keep joining across a link boundary, say) and each cluster takes the
//! style of the item it starts in. RTL reordering happens per line
//! in `lines.rs`, at cluster granularity, as swash requires.

use std::ops::Range;

use swash::shape::{Direction, ShapeContext};
use swash::text::cluster::SourceRange;
use swash::text::{Category, Codepoint, Script};
use unicode_bidi::{BidiClass, BidiInfo, bidi_class};

use super::{Para, ParaItem};
use crate::fonts::{FaceId, Fonts};

pub mod flags {
    /// All characters are whitespace (hangs at line end, not counted in width).
    pub const SPACE: u8 = 1 << 0;
    /// Contains a newline (hard break).
    pub const NEWLINE: u8 = 1 << 1;
    /// Inline box; `Cluster::glyphs.start` is the item index.
    pub const BOX: u8 = 1 << 2;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    /// Byte range in `Para::text`.
    pub start: u32,
    pub end: u32,
    /// Range into `Shaped::glyphs` (for BOX clusters, `start` is the item index).
    pub glyphs: Range<u32>,
    pub advance: f32,
    pub face: FaceId,
    /// Style of the item the cluster starts in; the glyphs are drawn in it. A cluster that
    /// spans several items (a ligature across a link boundary) is one glyph, so it has one
    /// color: the first segment's.
    pub style: u32,
    /// Range into `Shaped::segments` when the cluster spans items of different styles; empty
    /// otherwise (the common case, which allocates nothing).
    pub segs: Range<u32>,
    /// Bidi embedding level (odd = RTL).
    pub level: u8,
    pub flags: u8,
}

/// The part of a multi-item cluster that belongs to one item style. Hit-testing and
/// decorations treat each segment as its own span of the cluster's advance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    /// Start in `Para::text`; a segment runs to the next one's start (the last, to the
    /// cluster's end).
    pub start: u32,
    pub style: u32,
    /// Share of the cluster's advance: ligature components (or characters) in the segment.
    pub weight: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaped {
    /// Clusters in logical order.
    pub clusters: Vec<Cluster>,
    pub glyphs: Vec<Glyph>,
    /// Style segments of the clusters that span items of different styles.
    pub segments: Vec<Segment>,
    /// Paragraph base level (0 = LTR, 1 = RTL).
    pub base_level: u8,
    /// Characters no face covers (sorted, deduplicated).
    pub missing: Vec<char>,
    /// Shaping runs, boxes included.
    #[cfg(test)]
    pub runs: usize,
}

struct Run {
    range: Range<usize>,
    /// The item the run starts in; a text run covers every item up to the one holding its end.
    item: usize,
    /// An inline box.
    boxed: bool,
    face: FaceId,
    size: f32,
    script: Script,
    level: u8,
}

/// Splits a cluster that reaches past its first item into one segment per item style, pushing
/// them onto `segments` (nothing when every item has the same style). `items` starts at the
/// item holding the cluster's start; `span` is the cluster's byte range in `text`. A segment's
/// weight is the number of ligature components starting in it (`components` are relative to
/// `base`) when every segment has some, else its characters.
fn split_cluster(
    segments: &mut Vec<Segment>,
    text: &str,
    items: &[ParaItem],
    span: Range<u32>,
    base: u32,
    components: &[SourceRange],
) {
    let seg0 = segments.len();
    for (k, it) in items.iter().enumerate() {
        if it.range.start >= span.end {
            break;
        }
        if segments.len() == seg0 || segments.last().is_some_and(|s| s.style != it.style) {
            segments.push(Segment {
                start: if k == 0 { span.start } else { it.range.start },
                style: it.style,
                weight: 0,
            });
        }
    }
    if segments.len() - seg0 < 2 {
        segments.truncate(seg0);
        return;
    }
    let mut ends: Vec<u32> = segments[seg0 + 1..].iter().map(|s| s.start).collect();
    ends.push(span.end);
    let mut by_components = !components.is_empty();
    for (seg, &end) in segments[seg0..].iter_mut().zip(&ends) {
        let n = components
            .iter()
            .filter(|c| (seg.start..end).contains(&(base + c.start)))
            .count();
        seg.weight = n as u32;
        by_components &= n > 0;
    }
    if !by_components {
        for (seg, &end) in segments[seg0..].iter_mut().zip(&ends) {
            seg.weight = text[seg.start as usize..end as usize].chars().count() as u32;
        }
    }
}

/// Characters that must stay in the face of the character before them: joiners, variation
/// selectors, emoji modifiers and tags, combining marks.
fn attaches_to_previous(c: char) -> bool {
    matches!(c,
        '\u{200C}' | '\u{200D}' | '\u{20E3}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{1F3FB}'..='\u{1F3FF}'
        | '\u{E0020}'..='\u{E007F}'
        | '\u{E0100}'..='\u{E01EF}')
        || is_combining_mark(c)
}

/// Visible combining marks: they stay in their base's face, so that face must cover them too.
pub fn is_combining_mark(c: char) -> bool {
    !is_default_ignorable(c)
        && matches!(
            c.category(),
            Category::NonspacingMark | Category::SpacingMark | Category::EnclosingMark
        )
}

/// Default-ignorable code points (formatting characters, bidi controls, variation selectors):
/// never drawn, so they never count as missing and take the face of their neighbors.
fn is_default_ignorable(c: char) -> bool {
    // Unicode's Default_Ignorable_Code_Point, all of it: swash leaves out the glyphs of the
    // same characters, unless they were turned into others (see `shape`).
    matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}'..='\u{1160}'
        | '\u{17B4}'..='\u{17B5}'
        | '\u{180B}'..='\u{180F}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}'
        | '\u{3164}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{FEFF}'
        | '\u{FFA0}'
        | '\u{FFF0}'..='\u{FFF8}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0000}'..='\u{E0FFF}')
}

/// The combining marks after a base, and where they end.
struct Marks {
    /// Directly after the base: they may compose with it.
    composable: Vec<char>,
    /// After an ignorable character (ZWJ, variation selector, CGJ, ZWSP, ...): canonical
    /// composition doesn't reach across it, so the mark needs a glyph of its own.
    loose: Vec<char>,
    /// End of everything that stays with the base.
    end: usize,
}

impl Marks {
    fn is_empty(&self) -> bool {
        self.composable.is_empty() && self.loose.is_empty()
    }

    /// Marks no face draws at all (composition aside), to report as missing.
    fn uncovered<'a>(
        &'a self,
        fonts: &'a Fonts,
        primary: FaceId,
    ) -> impl Iterator<Item = char> + 'a {
        let all = self.composable.iter().chain(&self.loose).copied();
        all.filter(move |&m| fonts.select(primary, m).is_none())
    }
}

/// Everything after the base ending at `from` (up to `end`) that stays in the base's face:
/// attaching characters and ignorables, with the visible combining marks among them.
fn marks_after(text: &str, from: usize, end: usize) -> Marks {
    let mut seq = Marks {
        composable: Vec::new(),
        loose: Vec::new(),
        end: from,
    };
    let mut blocked = false;
    for c in text[from..end].chars() {
        if is_combining_mark(c) {
            let list = if blocked {
                &mut seq.loose
            } else {
                &mut seq.composable
            };
            list.push(c);
        } else if is_default_ignorable(c) {
            // Joiners and variation selectors stay with the base's face, but still stand
            // between it and the marks after them.
            blocked = true;
        } else if !attaches_to_previous(c) {
            break;
        }
        seq.end += c.len_utf8();
    }
    seq
}

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Per-byte bidi levels and the base level, or `None` when the text is entirely LTR (the
/// common case, which skips the bidi algorithm).
fn bidi_levels(text: &str) -> Option<(Vec<u8>, u8)> {
    let rtl = text.chars().any(|c| {
        c >= '\u{0590}'
            && matches!(
                bidi_class(c),
                BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI
            )
    });
    if !rtl {
        return None;
    }
    let info = BidiInfo::new(text, None);
    let base = info.paragraphs.first().map_or(0, |p| p.level.number());
    Some((info.levels.iter().map(|l| l.number()).collect(), base))
}

pub fn shape(ctx: &mut ShapeContext, fonts: &Fonts, para: &Para) -> Shaped {
    let text = para.text.as_str();
    let levels = bidi_levels(text);
    let level_at = |i: usize| levels.as_ref().map_or(0, |(l, _)| l[i]);
    let mut out = Shaped {
        base_level: levels.as_ref().map_or(0, |l| l.1),
        ..Shaped::default()
    };

    // Itemize.
    let mut runs: Vec<Run> = Vec::new();
    for (item_idx, item) in para.items.iter().enumerate() {
        let range = item.range.start as usize..item.range.end as usize;
        if item.inline_box.is_some() {
            runs.push(Run {
                range,
                item: item_idx,
                boxed: true,
                face: FaceId(0),
                size: 0.0,
                script: Script::Common,
                level: level_at(item.range.start as usize),
            });
            continue;
        }
        let style = &para.styles[item.style as usize];
        let primary = fonts.primary(style.bold, style.italic, style.mono);
        let mut prev: Option<char> = None;
        // Marks before this offset were already checked together with their base.
        let mut checked = 0;
        for (off, c) in text[range.clone()].char_indices() {
            let at = range.start + off;
            let level = level_at(at);
            // The run holding the character before this one, within the item.
            let last = runs.last().filter(|_| off > 0);
            let inherit = prev.is_some()
                && (attaches_to_previous(c)
                    || prev.is_some_and(|p| is_regional_indicator(p) && is_regional_indicator(c)));
            let face = match (inherit, last) {
                (true, Some(r)) if is_combining_mark(c) && at >= checked => {
                    // No base vouched for this mark (it follows a control character or an
                    // ignorable one): the face it would inherit must draw it.
                    let covered = |id| fonts.face(id).glyph(c) != 0;
                    if covered(r.face) {
                        r.face
                    } else {
                        fonts.select(primary, c).unwrap_or_else(|| {
                            out.missing.push(c);
                            r.face
                        })
                    }
                }
                (true, Some(r)) => r.face,
                _ if c.is_control() || c.is_whitespace() || is_default_ignorable(c) => {
                    // Spaces stay in the run before them (an Arabic or CJK fallback keeps one
                    // shaping run), except after emoji: color fonts have very wide spaces.
                    let keep = last
                        .map(|r| r.face)
                        .filter(|&f| !fonts.face(f).color)
                        .unwrap_or(primary);
                    if c.is_control() || !c.is_whitespace() {
                        keep
                    } else {
                        // A space can carry marks too: its face must draw them.
                        let seq = marks_after(text, at + c.len_utf8(), range.end);
                        checked = seq.end;
                        let face = (!seq.is_empty())
                            .then(|| fonts.select_sequence(primary, c, &seq.composable, &seq.loose))
                            .flatten();
                        face.unwrap_or_else(|| {
                            out.missing.extend(seq.uncovered(fonts, primary));
                            keep
                        })
                    }
                }
                _ => {
                    // The marks that follow stay on this face, so it must draw all of them.
                    let seq = marks_after(text, at + c.len_utf8(), range.end);
                    checked = seq.end;
                    let whole = (!seq.is_empty())
                        .then(|| fonts.select_sequence(primary, c, &seq.composable, &seq.loose))
                        .flatten();
                    whole.unwrap_or_else(|| {
                        // No face draws the whole sequence: the base keeps its own face, and
                        // marks nothing draws are reported so a fallback gets searched.
                        out.missing.extend(seq.uncovered(fonts, primary));
                        fonts.select(primary, c).unwrap_or_else(|| {
                            out.missing.push(c);
                            primary
                        })
                    })
                }
            };
            // Characters with no script of their own go with the text before them, across items.
            let script = match c.script() {
                Script::Common | Script::Inherited | Script::Unknown => runs
                    .last()
                    .filter(|r| !r.boxed)
                    .map_or(Script::Latin, |r| r.script),
                s => s,
            };
            // A run goes on over an item boundary unless something that shapes differs (face,
            // size, script, level) or the text is synthesized (it maps to the document apart).
            let across =
                off > 0 || item_idx == 0 || !(item.fixed_doc || para.items[item_idx - 1].fixed_doc);
            match runs.last_mut() {
                Some(r)
                    if !r.boxed
                        && across
                        && r.face == face
                        && r.size.to_bits() == style.size.to_bits()
                        && r.level == level
                        && r.script == script =>
                {
                    r.range.end = at + c.len_utf8();
                }
                _ => runs.push(Run {
                    range: at..at + c.len_utf8(),
                    item: item_idx,
                    boxed: false,
                    face,
                    size: style.size,
                    script,
                    level,
                }),
            }
            prev = Some(c);
        }
    }
    out.missing.sort_unstable();
    out.missing.dedup();

    // Shape.
    #[cfg(test)]
    {
        out.runs = runs.len();
    }
    for run in &runs {
        let item = &para.items[run.item];
        if let Some(b) = item.inline_box {
            out.clusters.push(Cluster {
                start: run.range.start as u32,
                end: run.range.end as u32,
                glyphs: run.item as u32..run.item as u32,
                advance: b.width,
                face: run.face,
                style: item.style,
                segs: 0..0,
                level: run.level,
                flags: flags::BOX,
            });
            continue;
        }
        // The items the run covers, to find each cluster's style in.
        let items = &para.items[run.item..];
        let font = fonts.face(run.face).font();
        let dir = if run.level % 2 == 1 {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        };
        // Code shows its characters as typed: no ligatures or contextual alternates (JetBrains
        // Mono would draw `://`, `->` or `!=` as joined shapes, unlike GitHub's code font).
        let mono = para.styles[para.items[run.item].style as usize].mono;
        let off: &[(&str, u16)] = if mono {
            &[("liga", 0), ("calt", 0)]
        } else {
            &[]
        };
        // swash would drop every glyph that stands for an invisible character, also one a
        // substitution made of it and the next ones: Segoe UI Emoji makes each member of a family
        // (👨‍👩‍👧) from a zero-width joiner and the person after it. Such glyphs are kept here,
        // and the ones that are still the invisible characters' own are dropped below.
        let mut shaper = ctx
            .builder(font)
            .size(run.size)
            .script(run.script)
            .direction(dir)
            .features(off.iter().copied())
            .retain_ignorables(true)
            .build();
        shaper.add_str(&text[run.range.clone()]);
        let base = run.range.start as u32;
        let glyphs = &mut out.glyphs;
        let clusters = &mut out.clusters;
        let segments = &mut out.segments;
        shaper.shape_with(|c| {
            let start = base + c.source.start;
            let end = base + c.source.end;
            let src = &text[start as usize..end as usize];
            let first = items.partition_point(|it| it.range.end <= start);
            let style = items[first].style;
            let seg0 = segments.len() as u32;
            if items[first].range.end < end {
                split_cluster(
                    segments,
                    text,
                    &items[first..],
                    start..end,
                    base,
                    c.components,
                );
            }
            // Invisible formatting characters render nothing, even when the face lacks them:
            // a cluster that is all of them drops every `.notdef`, one that carries them next
            // to a base (`e` + variation selector + mark) drops one per such character.
            let face = fonts.face(run.face);
            let invisible = src.chars().all(is_default_ignorable);
            let ignorables = src.chars().any(is_default_ignorable);
            let mut unseen = src
                .chars()
                .filter(|&ch| is_default_ignorable(ch) && face.glyph(ch) == 0)
                .count();
            // An invisible character's own glyph (a soft hyphen's hyphen, a joiner's empty
            // glyph), as swash would have dropped it; a glyph substituted for it stays.
            let own = |id: u16| {
                ignorables
                    && id != 0
                    && src
                        .chars()
                        .any(|ch| is_default_ignorable(ch) && face.glyph(ch) == id)
            };
            let g0 = glyphs.len() as u32;
            let mut advance = 0.0;
            for g in c.glyphs.iter().filter(|g| {
                let drop = g.id == 0 && (invisible || unseen > 0);
                unseen -= (drop && unseen > 0) as usize;
                !drop && !own(g.id)
            }) {
                glyphs.push(Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                    advance: g.advance,
                });
                advance += g.advance;
            }
            let mut f = 0;
            if !src.is_empty() && src.chars().all(char::is_whitespace) {
                f |= flags::SPACE;
            }
            if src.contains('\n') {
                f |= flags::NEWLINE;
                advance = 0.0;
            }
            clusters.push(Cluster {
                start,
                end,
                glyphs: g0..glyphs.len() as u32,
                advance,
                face: run.face,
                style,
                segs: seg0..segments.len() as u32,
                level: run.level,
                flags: f,
            });
        });
    }
    // swash emits clusters in logical order within each run; runs are in logical order.
    debug_assert!(out.clusters.windows(2).all(|w| w[0].start <= w[1].start));
    out
}
