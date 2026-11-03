//! A formula's glyphs and rules at a size in pixels: the glyphs and rectangles the painter
//! draws (the glyph cache rasterizes the math face like any other).

use super::layout::Formula;
use crate::text::{PCut, PGlyph, PRect, RectKind, TextStyle};
use crate::theme::Role;

/// Where emitted glyphs and rules go: a block's (or a test's) lists. `ink` is painted over
/// the selection and find washes, as the glyphs are; `cuts` are sorted by glyph.
#[derive(Debug, Default)]
pub struct Emitted {
    pub glyphs: Vec<PGlyph>,
    pub styles: Vec<TextStyle>,
    pub ink: Vec<PRect>,
    pub cuts: Vec<PCut>,
}

/// Appends `f` at `em` pixels per em, its baseline's left end at (`x`, `y`) (pixels, y down),
/// in the color of `role`. One text style is added for each size the glyphs are drawn at.
/// Rules are at least a pixel thick, and on whole pixels, so bars stay crisp.
pub fn emit(f: &Formula, em: f32, x: f32, y: f32, role: Role, out: &mut Emitted) {
    let base = out.styles.len();
    for g in &f.glyphs {
        let size = g.size * em;
        let style = match out.styles[base..].iter().position(|s| s.size == size) {
            Some(i) => base + i,
            None => {
                out.styles.push(TextStyle {
                    bold: false,
                    italic: false,
                    mono: false,
                    size,
                    role,
                    deco: 0,
                    shift: 0.0,
                    link: 0,
                });
                out.styles.len() - 1
            }
        };
        if let Some(c) = g.cut {
            // y goes down in pixels: the band turns over.
            let (lo, hi) = if c.vertical {
                (y - c.hi * em, y - c.lo * em)
            } else {
                (x + c.lo * em, x + c.hi * em)
            };
            out.cuts.push(PCut {
                glyph: out.glyphs.len() as u32,
                vertical: c.vertical,
                lo,
                hi,
            });
        }
        out.glyphs.push(PGlyph {
            x: x + g.x * em,
            y: y - g.y * em,
            id: g.id,
            face: g.face,
            style: style as u32,
            synth: 0,
        });
    }
    for r in &f.rules {
        let h = (r.h * em).round().max(1.0);
        let top = (y - (r.y + r.h) * em).round();
        out.ink.push(PRect {
            x: x + r.x * em,
            y: top,
            w: r.w * em,
            h,
            role,
            radius: 0.0,
            kind: RectKind::Fill,
        });
    }
}
