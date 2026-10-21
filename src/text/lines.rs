//! Line breaking (UAX #14 opportunities, greedy fill), per-line bidi reordering (UAX #9 rule
//! L2 at cluster granularity), and glyph positioning.

use std::ops::Range;

use unicode_linebreak::{BreakOpportunity, linebreaks};

use super::shape::{Cluster, Segment, Shaped, flags};
use super::{Align, HIT_RTL, PBox, PGlyph, PHit, PLine, PRect, Para, ParaLayout, RectKind, deco};
use crate::fonts::Fonts;
use crate::theme::Role;

/// Tolerance for float accumulation when comparing widths.
const EPS: f32 = 0.01;

#[derive(Clone, Debug, PartialEq)]
pub struct LineBreak {
    /// Clusters on the line, logical order.
    pub clusters: Range<usize>,
    /// End of the line's content: trailing whitespace and newlines are excluded.
    pub content_end: usize,
    /// Content width.
    pub width: f32,
    /// The line starts after a soft wrap.
    pub cont: bool,
}

fn hangs(flags_: u8) -> bool {
    flags_ & (flags::SPACE | flags::NEWLINE) != 0
}

fn line(shaped: &Shaped, range: Range<usize>, cont: bool) -> LineBreak {
    let cl = &shaped.clusters[range.clone()];
    let content = cl
        .iter()
        .rposition(|c| !hangs(c.flags))
        .map_or(0, |i| i + 1);
    let width = cl[..content].iter().map(|c| c.advance).sum();
    LineBreak {
        content_end: range.start + content,
        clusters: range,
        width,
        cont,
    }
}

/// Width of a cluster range, excluding trailing whitespace.
fn content_width(shaped: &Shaped, range: Range<usize>) -> f32 {
    line(shaped, range, false).width
}

/// Greedy line breaking in logical order. Always returns at least one line.
pub fn break_lines(text: &str, shaped: &Shaped, max_width: f32) -> Vec<LineBreak> {
    let cl = &shaped.clusters;
    let n = cl.len();
    let mut lines = Vec::new();
    let mut start = 0; // first cluster of the current line
    let mut full = 0.0; // width of [start, ci) including trailing whitespace
    let mut cont = false;
    let mut ci = 0;
    for (pos, opp) in linebreaks(text) {
        let mut cj = ci;
        while cj < n && (cl[cj].start as usize) < pos {
            cj += 1;
        }
        let seg_full: f32 = cl[ci..cj].iter().map(|c| c.advance).sum();
        let seg_content = content_width(shaped, ci..cj);
        if start < ci && full + seg_content > max_width + EPS {
            lines.push(line(shaped, start..ci, cont));
            cont = true;
            start = ci;
            full = 0.0;
        }
        if start == ci && seg_content > max_width + EPS {
            // Emergency break: a single unbreakable segment wider than the line.
            let mut w = 0.0;
            for (k, c) in cl.iter().enumerate().take(cj).skip(ci) {
                let a = c.advance;
                if k > start && w + a > max_width + EPS && !hangs(c.flags) {
                    lines.push(line(shaped, start..k, cont));
                    cont = true;
                    start = k;
                    w = 0.0;
                }
                w += a;
            }
            full = w;
        } else {
            full += seg_full;
        }
        ci = cj;
        if opp == BreakOpportunity::Mandatory {
            if start < ci {
                lines.push(line(shaped, start..ci, cont));
            }
            cont = false;
            start = ci;
            full = 0.0;
        }
    }
    if start < n {
        lines.push(line(shaped, start..n, cont));
    }
    if lines.is_empty() {
        lines.push(LineBreak {
            clusters: 0..0,
            content_end: 0,
            width: 0.0,
            cont: false,
        });
    }
    lines
}

/// (minimum, natural) width of a paragraph: widest unbreakable segment, widest hard line.
pub fn measure(para: &Para, shaped: &Shaped) -> (f32, f32) {
    let cl = &shaped.clusters;
    let (mut min, mut max) = (0.0f32, 0.0f32);
    let (mut ci, mut line_start) = (0, 0);
    for (pos, opp) in linebreaks(&para.text) {
        let mut cj = ci;
        while cj < cl.len() && (cl[cj].start as usize) < pos {
            cj += 1;
        }
        min = min.max(content_width(shaped, ci..cj));
        ci = cj;
        if opp == BreakOpportunity::Mandatory {
            max = max.max(content_width(shaped, line_start..ci));
            line_start = ci;
        }
    }
    max = max.max(content_width(shaped, line_start..cl.len()));
    (min, max.max(min))
}

/// Visual order of a line's content clusters (trailing whitespace dropped). Implements rule
/// L2: from the highest level down to the lowest odd level, reverse every maximal run of
/// clusters at or above that level. Clusters are reversed as units, never their glyphs.
pub fn visual_order(shaped: &Shaped, line: &LineBreak) -> Vec<usize> {
    let mut order: Vec<usize> = (line.clusters.start..line.content_end).collect();
    let levels: Vec<u8> = order.iter().map(|&i| shaped.clusters[i].level).collect();
    let Some(&max) = levels.iter().max() else {
        return order;
    };
    if max == 0 {
        return order;
    }
    let min_odd = levels.iter().copied().min().unwrap_or(0) | 1;
    let mut lv = levels;
    for level in (min_odd..=max).rev() {
        let mut i = 0;
        while i < order.len() {
            if lv[i] >= level {
                let j = (i..order.len())
                    .find(|&j| lv[j] < level)
                    .unwrap_or(order.len());
                order[i..j].reverse();
                lv[i..j].reverse();
                i = j;
            } else {
                i += 1;
            }
        }
    }
    order
}

/// Line ascent/descent (from the line top to the baseline, and baseline to bottom), with
/// half-leading so each style's line height is `size × line_height`.
fn line_metrics(
    fonts: &Fonts,
    para: &Para,
    shaped: &Shaped,
    range: Range<usize>,
    lh: f32,
) -> (f32, f32) {
    // Extent above and below the baseline for one style, half-leading included.
    let extent = |face, style: &super::TextStyle| {
        let m = fonts.face(face).metrics().scale(style.size);
        let (a, d) = (m.ascent, m.descent.abs());
        let half = ((style.size * lh) - (a + d)).max(0.0) / 2.0;
        (a + half + style.shift, (d + half - style.shift).max(0.0))
    };
    let (mut asc, mut desc) = (0.0f32, 0.0f32);
    let mut any = false;
    for c in &shaped.clusters[range] {
        let style = &para.styles[c.style as usize];
        let (a, d) = if c.flags & flags::BOX != 0 {
            let b = para.items[c.glyphs.start as usize]
                .inline_box
                .expect("box item");
            (b.ascent + style.shift, b.descent - style.shift)
        } else {
            extent(c.face, style)
        };
        asc = asc.max(a);
        desc = desc.max(d);
        any = true;
    }
    if !any && let Some(style) = para.styles.first() {
        (asc, desc) = extent(fonts.primary(style.bold, style.italic, style.mono), style);
    }
    (asc, desc)
}

pub fn position(
    fonts: &Fonts,
    para: &Para,
    shaped: &Shaped,
    max_width: f32,
    align: Align,
    lh: f32,
) -> ParaLayout {
    let mut out = ParaLayout {
        missing: shaped.missing.clone(),
        ..ParaLayout::default()
    };
    let rtl = shaped.base_level % 2 == 1;
    let mut y = 0.0;
    for lb in break_lines(&para.text, shaped, max_width) {
        let (asc, desc) = line_metrics(fonts, para, shaped, lb.clusters.clone(), lh);
        let baseline = y + asc;
        let slack = (max_width - lb.width).max(0.0);
        let mut pen = match align {
            Align::Start if rtl => slack,
            Align::Start => 0.0,
            Align::Center => slack / 2.0,
            Align::Right => slack,
        };
        let hits_start = out.hits.len() as u32;
        for ci in visual_order(shaped, &lb) {
            let c = &shaped.clusters[ci];
            let style = &para.styles[c.style as usize];
            let x0 = pen;
            if c.flags & flags::BOX != 0 {
                let b = para.items[c.glyphs.start as usize]
                    .inline_box
                    .expect("box item");
                out.boxes.push(PBox {
                    x: pen,
                    y: baseline - style.shift - b.ascent,
                    b,
                    #[cfg(feature = "clipboard")]
                    doc: para.doc_offset(c.start),
                });
                pen += b.width;
            } else {
                let synth = fonts.synth_for(c.face, style.bold, style.italic);
                for g in &shaped.glyphs[c.glyphs.start as usize..c.glyphs.end as usize] {
                    out.glyphs.push(PGlyph {
                        x: pen + g.x,
                        y: baseline - g.y - style.shift,
                        id: g.id,
                        face: c.face,
                        style: c.style,
                        synth,
                    });
                    pen += g.advance;
                }
            }
            cluster_hits(para, shaped, c, x0, pen, &mut out.hits);
        }
        let hits = hits_start..out.hits.len() as u32;
        decorations(
            fonts,
            para,
            &out.hits[hits.start as usize..],
            baseline,
            &mut out.rects,
        );
        out.lines.push(PLine {
            top: y,
            baseline,
            bottom: baseline + desc,
            width: lb.width,
            cont: lb.cont,
            hits,
        });
        out.width = out.width.max(lb.width);
        y = baseline + desc;
    }
    out.height = y;
    out
}

/// Hit spans for a cluster covering `x0..x1`: one, or one per style segment for a cluster that
/// spans items of different styles (a ligature across a link boundary). Segments split the
/// advance by weight, laid out in visual order (the reverse of logical in RTL clusters).
fn cluster_hits(para: &Para, shaped: &Shaped, c: &Cluster, x0: f32, x1: f32, hits: &mut Vec<PHit>) {
    let segs = &shaped.segments[c.segs.start as usize..c.segs.end as usize];
    let dir = if c.level % 2 == 1 { HIT_RTL } else { 0 };
    if segs.is_empty() {
        hits.push(PHit {
            x0,
            x1,
            doc: para.doc_offset(c.start),
            style: c.style | dir,
        });
        return;
    }
    let total: u32 = segs.iter().map(|s| s.weight).sum::<u32>().max(1);
    let (mut done, mut x) = (0, x0);
    let mut emit = |s: &Segment| {
        done += s.weight;
        // Land exactly on `x1` after the last segment, whatever the rounding.
        let end = if done >= total {
            x1
        } else {
            x0 + (x1 - x0) * done as f32 / total as f32
        };
        hits.push(PHit {
            x0: x,
            x1: end,
            doc: para.doc_offset(s.start),
            style: s.style | dir,
        });
        x = end;
    };
    if c.level % 2 == 1 {
        segs.iter().rev().for_each(&mut emit);
    } else {
        segs.iter().for_each(&mut emit);
    }
}

/// Rects for decorated runs: consecutive clusters sharing a decorated style.
fn decorations(fonts: &Fonts, para: &Para, hits: &[PHit], baseline: f32, rects: &mut Vec<PRect>) {
    let mut i = 0;
    while i < hits.len() {
        let style = &para.styles[hits[i].style_index()];
        let j = (i..hits.len())
            .find(|&j| hits[j].style_index() != hits[i].style_index())
            .unwrap_or(hits.len());
        if style.deco != 0 {
            let (x0, x1) = (
                hits[i].x0.min(hits[j - 1].x0),
                hits[i].x1.max(hits[j - 1].x1),
            );
            let m = fonts
                .face(fonts.primary(style.bold, style.italic, style.mono))
                .metrics()
                .scale(style.size);
            let base = baseline - style.shift;
            let pad = style.size * 0.15;
            if style.deco & deco::CODE_BG != 0 {
                rects.push(PRect {
                    x: x0 - pad,
                    y: base - m.ascent - pad * 0.5,
                    w: x1 - x0 + 2.0 * pad,
                    h: m.ascent + m.descent.abs() + pad,
                    role: Role::CodeBg,
                    radius: style.size * 0.25,
                    kind: RectKind::Fill,
                });
            }
            if style.deco & deco::IMAGE_BOX != 0 {
                rects.push(PRect {
                    x: x0 - pad,
                    y: base - m.ascent - pad * 0.5,
                    w: x1 - x0 + 2.0 * pad,
                    h: m.ascent + m.descent.abs() + pad,
                    role: Role::Border,
                    radius: style.size * 0.2,
                    kind: RectKind::Stroke(1.0f32.max(style.size / 16.0).round()),
                });
            }
            if style.deco & deco::STRIKE != 0 {
                let t = m.stroke_size.max(1.0);
                rects.push(PRect {
                    x: x0,
                    y: base - m.strikeout_offset - t / 2.0,
                    w: x1 - x0,
                    h: t,
                    role: style.role,
                    radius: 0.0,
                    kind: RectKind::Fill,
                });
            }
        }
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextStyle;

    fn style(link: u32) -> TextStyle {
        TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: 16.0,
            role: Role::Link,
            deco: 0,
            shift: 0.0,
            link,
        }
    }

    /// Two items ("ab" doc 10, "cd" doc 50) and a synthetic cluster over all of "abcd".
    fn spanning(level: u8, weights: [u32; 2]) -> (Para, Shaped) {
        let mut para = Para::default();
        para.push_text("ab", 10, style(1));
        para.push_text("cd", 50, style(2));
        let cluster = Cluster {
            start: 0,
            end: 4,
            glyphs: 0..0,
            advance: 30.0,
            face: crate::fonts::FaceId(0),
            style: 0,
            segs: 0..2,
            level,
            flags: 0,
        };
        let seg = |start, style, weight| Segment {
            start,
            style,
            weight,
        };
        let shaped = Shaped {
            clusters: vec![cluster],
            segments: vec![seg(0, 0, weights[0]), seg(2, 1, weights[1])],
            base_level: level,
            ..Shaped::default()
        };
        (para, shaped)
    }

    fn hits(para: &Para, shaped: &Shaped) -> Vec<PHit> {
        let mut out = Vec::new();
        cluster_hits(para, shaped, &shaped.clusters[0], 100.0, 130.0, &mut out);
        out
    }

    #[test]
    fn a_cluster_over_two_items_gets_one_hit_each() {
        let (para, shaped) = spanning(0, [1, 1]);
        let h = hits(&para, &shaped);
        let links: Vec<u32> = h
            .iter()
            .map(|h| para.styles[h.style_index()].link)
            .collect();
        assert_eq!(links, [1, 2]);
        assert_eq!(
            (h[0].x0, h[0].x1, h[1].x0, h[1].x1),
            (100.0, 115.0, 115.0, 130.0)
        );
        assert_eq!((h[0].doc, h[1].doc), (10, 50));
        // Weights divide the advance.
        let (para, shaped) = spanning(0, [1, 2]);
        let h = hits(&para, &shaped);
        assert_eq!((h[0].x1, h[1].x1), (110.0, 130.0));
    }

    #[test]
    fn rtl_clusters_lay_their_segments_out_in_reverse() {
        let (para, shaped) = spanning(1, [1, 2]);
        let h = hits(&para, &shaped);
        let links: Vec<u32> = h
            .iter()
            .map(|h| para.styles[h.style_index()].link)
            .collect();
        assert_eq!(links, [2, 1]);
        assert_eq!(
            (h[0].x0, h[0].x1, h[1].x0, h[1].x1),
            (100.0, 120.0, 120.0, 130.0)
        );
        assert_eq!((h[0].doc, h[1].doc), (50, 10));
        #[cfg(any(feature = "clipboard", feature = "find"))]
        assert!(h.iter().all(PHit::rtl));
    }

    /// The direction is only a bit of the style index: the index reads the same.
    #[cfg(any(feature = "clipboard", feature = "find"))]
    #[test]
    fn the_direction_bit_leaves_the_style_index_alone() {
        let (para, shaped) = spanning(0, [1, 1]);
        assert!(hits(&para, &shaped).iter().all(|h| !h.rtl()));
        let (para, shaped) = spanning(1, [1, 1]);
        let h = hits(&para, &shaped);
        assert_eq!(h.iter().map(PHit::style_index).collect::<Vec<_>>(), [1, 0]);
        assert!(h.iter().all(|h| h.style >> 31 == 1));
    }

    #[test]
    fn a_cluster_without_segments_is_one_hit() {
        let (para, mut shaped) = spanning(0, [1, 1]);
        shaped.clusters[0].segs = 0..0;
        let h = hits(&para, &shaped);
        assert_eq!(h.len(), 1);
        assert_eq!(
            (h[0].x0, h[0].x1, h[0].doc, h[0].style),
            (100.0, 130.0, 10, 0)
        );
    }
}
