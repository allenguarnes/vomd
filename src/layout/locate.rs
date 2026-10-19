//! Pointer → caret position, for selecting text (see `select.rs`).

use super::Layout;
use crate::select::{END, Pos};
use crate::text::{PHit, PLine};

impl Layout {
    /// The caret position nearest to (`x`, `y`), document-area coordinates: before or after the
    /// cluster under it, by which half it is in (and which way the cluster runs). Anywhere in the
    /// document has an answer: above a block's text it is the block's start, below it the end,
    /// and beside a line the nearest cluster on it. A block with no content (far from the
    /// viewport) gives its start. Only blocks near the viewport have the hits to look through,
    /// which covers everything on screen.
    pub fn locate(&self, x: f32, y: f32) -> Pos {
        if self.geoms.is_empty() {
            return Pos::start(0);
        }
        let i = self.block_at(y);
        let block = i as u32;
        let Some(content) = self.geoms[i].content.as_deref() else {
            return Pos::start(block);
        };
        let (lx, ly) = (x - self.col_x, y - self.y(i));
        let lines = &content.lines;
        if lines.is_empty() {
            // A rule, or an alert's title: the nearer end.
            return if ly < self.geoms[i].h / 2.0 {
                Pos::start(block)
            } else {
                Pos::end(block)
            };
        }
        let top = lines.iter().map(|l| l.top).fold(f32::INFINITY, f32::min);
        let bottom = lines
            .iter()
            .map(|l| l.bottom)
            .fold(f32::NEG_INFINITY, f32::max);
        if ly < top {
            return Pos::start(block);
        }
        if ly >= bottom {
            return Pos::end(block);
        }
        // The line holding `ly`, or the nearest one; the cells of a table row have lines side by
        // side, so there may be several.
        let away = |l: &PLine| (l.top - ly).max(ly - l.bottom).max(0.0);
        let near = lines.iter().map(away).fold(f32::INFINITY, f32::min);
        let on_row = |l: &&PLine| away(l) <= near;
        let hits_of = |l: &PLine| &content.hits[l.hits.start as usize..l.hits.end as usize];
        let nearest = lines
            .iter()
            .filter(on_row)
            .flat_map(hits_of)
            .map(|h| (h, (h.x0.min(h.x1) - lx).max(lx - h.x0.max(h.x1)).max(0.0)))
            .reduce(|best, c| if c.1 < best.1 { c } else { best });
        let off = match nearest {
            Some((h, _)) => caret(content.hits.as_slice(), h, lx),
            // A line without text (an empty one in a code block): where the next line's begins.
            None => {
                let k = lines.iter().position(|l| on_row(&l)).unwrap_or(0);
                lines[k + 1..]
                    .iter()
                    .find_map(|l| hits_of(l).iter().map(|h| h.doc).min())
                    .unwrap_or(END)
            }
        };
        Pos::new(block, off)
    }
}

/// The offset of the caret next to hit `h` that `lx` is nearer to: before the cluster if the
/// point is in the half it begins, else before the next cluster in the text (or the end).
fn caret(hits: &[PHit], h: &PHit, lx: f32) -> u32 {
    let leading = (lx < (h.x0 + h.x1) / 2.0) != h.rtl();
    if leading {
        return h.doc;
    }
    hits.iter()
        .map(|o| o.doc)
        .filter(|&d| d > h.doc)
        .min()
        .unwrap_or(END)
}
