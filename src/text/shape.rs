//! Itemization and shaping. Runs split on item, face (per-character fallback), script, and
//! bidi level; each run is shaped by swash in logical order. RTL reordering happens per line
//! in `lines.rs`, at cluster granularity, as swash requires.

use std::ops::Range;

use swash::shape::{Direction, ShapeContext};
use swash::text::{Category, Codepoint, Script};
use unicode_bidi::{BidiClass, BidiInfo, bidi_class};

use super::Para;
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
    pub style: u32,
    /// Bidi embedding level (odd = RTL).
    pub level: u8,
    pub flags: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaped {
    /// Clusters in logical order.
    pub clusters: Vec<Cluster>,
    pub glyphs: Vec<Glyph>,
    /// Paragraph base level (0 = LTR, 1 = RTL).
    pub base_level: u8,
    /// Characters no face covers (sorted, deduplicated).
    pub missing: Vec<char>,
}

struct Run {
    range: Range<usize>,
    item: usize,
    face: FaceId,
    script: Script,
    level: u8,
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
fn is_combining_mark(c: char) -> bool {
    !is_default_ignorable(c)
        && matches!(
            c.category(),
            Category::NonspacingMark | Category::SpacingMark | Category::EnclosingMark
        )
}

/// Default-ignorable code points (formatting characters, bidi controls, variation selectors):
/// never drawn, so they never count as missing and take the face of their neighbors.
fn is_default_ignorable(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{180B}'..='\u{180F}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{FEFF}'
        | '\u{E0000}'..='\u{E0FFF}')
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
                face: FaceId(0),
                script: Script::Common,
                level: level_at(item.range.start as usize),
            });
            continue;
        }
        let style = &para.styles[item.style as usize];
        let primary = fonts.primary(style.bold, style.italic, style.mono);
        let mut prev: Option<char> = None;
        for (off, c) in text[range.clone()].char_indices() {
            let at = range.start + off;
            let level = level_at(at);
            let last = runs.last().filter(|r| r.item == item_idx);
            let inherit = prev.is_some()
                && (attaches_to_previous(c)
                    || prev.is_some_and(|p| is_regional_indicator(p) && is_regional_indicator(c)));
            let face = match (inherit, last) {
                (true, Some(r)) => r.face,
                _ if c.is_control() || c.is_whitespace() || is_default_ignorable(c) => {
                    last.map_or(primary, |r| r.face)
                }
                _ => {
                    // The marks that follow stay on this face, so it must draw all of them.
                    let marks: Vec<char> = text[at + c.len_utf8()..range.end]
                        .chars()
                        .take_while(|&m| attaches_to_previous(m))
                        .filter(|&m| is_combining_mark(m))
                        .collect();
                    let whole = (!marks.is_empty())
                        .then(|| fonts.select_sequence(primary, c, &marks))
                        .flatten();
                    whole.unwrap_or_else(|| {
                        // No face draws the whole sequence: the base keeps its own face, and
                        // marks nothing draws are reported so a fallback gets searched.
                        out.missing.extend(
                            marks
                                .iter()
                                .filter(|&&m| fonts.select(primary, m).is_none()),
                        );
                        fonts.select(primary, c).unwrap_or_else(|| {
                            out.missing.push(c);
                            primary
                        })
                    })
                }
            };
            let script = match c.script() {
                Script::Common | Script::Inherited | Script::Unknown => {
                    last.map_or(Script::Latin, |r| r.script)
                }
                s => s,
            };
            match runs.last_mut() {
                Some(r)
                    if r.item == item_idx
                        && r.face == face
                        && r.level == level
                        && r.script == script =>
                {
                    r.range.end = at + c.len_utf8();
                }
                _ => runs.push(Run {
                    range: at..at + c.len_utf8(),
                    item: item_idx,
                    face,
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
                level: run.level,
                flags: flags::BOX,
            });
            continue;
        }
        let style = &para.styles[item.style as usize];
        let font = fonts.face(run.face).font();
        let dir = if run.level % 2 == 1 {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        };
        let mut shaper = ctx
            .builder(font)
            .size(style.size)
            .script(run.script)
            .direction(dir)
            .build();
        shaper.add_str(&text[run.range.clone()]);
        let base = run.range.start as u32;
        let glyphs = &mut out.glyphs;
        let clusters = &mut out.clusters;
        shaper.shape_with(|c| {
            let start = base + c.source.start;
            let end = base + c.source.end;
            let src = &text[start as usize..end as usize];
            // Invisible formatting characters render nothing, even when the face lacks them.
            let invisible = src.chars().all(is_default_ignorable);
            let g0 = glyphs.len() as u32;
            let mut advance = 0.0;
            for g in c.glyphs.iter().filter(|g| !(invisible && g.id == 0)) {
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
                style: item.style,
                level: run.level,
                flags: f,
            });
        });
    }
    // swash emits clusters in logical order within each run; runs are in logical order.
    debug_assert!(out.clusters.windows(2).all(|w| w[0].start <= w[1].start));
    out
}
