//! The copy button on code blocks: where it sits, when it shows, and what a click on it asks for.
//!
//! While the pointer is over a code block, a small button shows in the block's upper right
//! corner, as GitHub's does, and a click copies the block's code. It sits at the block's top, so
//! a block taller than the window scrolls its button away with its top, as GitHub's does. (Pinned
//! to the top of what is visible it would need the block's end as well, which is chunks away in
//! a block of several MB, and it would slide over the code being read.)
//!
//! Nothing is kept for the button but where the pointer is. Where the button is comes from the
//! layout whenever it is needed, so zoom, resize, scrolling and a changed layout leave nothing
//! to undo; a window whose pointer isn't over a code block pays one look-up of the block there.
//!
//! The button is the shell's to act on, like the toolbar's: a click leaves a [`CodeCopy`] for
//! [`View::release_copy`] to return, the shell offers it to the clipboard, and tells the view
//! whether that worked ([`View::code_copied`], or a toast).

use std::time::{Duration, Instant};

use super::View;
use crate::copy::Content;
use crate::doc::BlockKind;
use crate::fonts::Fonts;
use crate::layout::BlockBox;
use crate::render::{Clip, Frame};
use crate::select::block_unit;
use crate::text::cache::GlyphCache;
use crate::theme::{Role, Theme};
use crate::toolbar::{Rect, draw_icon, icon};

/// The button's size and its distance from the block's corner, in logical pixels at 100% zoom
/// (they scale with the block, so a small zoom doesn't leave a button that covers its code).
const SIZE: f32 = 28.0;
const INSET: f32 = 8.0;
const RADIUS: f32 = 6.0;
/// How long the check mark stands in for the copy icon after a copy.
pub(super) const CHECK: Duration = Duration::from_secs(2);

/// What a click on a button asks for: the copy of one code block.
#[derive(Clone, Debug)]
pub struct CodeCopy {
    /// The block's first chunk, which [`View::code_copied`] names.
    pub block: u32,
    pub content: Content,
}

/// The button's state: the pointer, and what it is doing to the button.
#[derive(Debug, Default)]
pub(super) struct CopyButton {
    /// Where the pointer is in the window (window pixels), if it is.
    pointer: Option<(f64, f64)>,
    /// What the pointer was last found to be on, for telling when the picture changes.
    shown: Option<Spot>,
    /// The mouse button went down on the button of the code block starting at this block.
    pressed: Option<u32>,
    /// The code block (its first chunk) that was copied, and when its check mark goes.
    copied: Option<(u32, Instant)>,
}

/// The pointer is over the code block starting at `block`, with its button showing; `hot` when
/// on the button, `down` when the mouse button is held there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spot {
    block: u32,
    hot: bool,
    down: bool,
}

/// A code block (blocks `first..=last`, which are chunks of one) with room for its button.
#[derive(Clone, Copy, Debug)]
struct Found {
    first: u32,
    last: u32,
    button: Rect,
}

/// The rectangle (`x`, `y`, `w`, `h`, in block coordinates) of the background of a code block's
/// chunk: where the code block is.
fn code_rect(content: &BlockBox) -> Option<(f32, f32, f32, f32)> {
    let r = content.rects.iter().find(|r| r.role == Role::CodeBg)?;
    Some((r.x, r.y, r.w, r.h))
}

impl View {
    /// The code block under window point (`x`, `y`) and its button (as it is placed on the
    /// block, whether the point is on it or not), if the block is big enough to hold one and has
    /// code to copy. Only blocks near the viewport have content, which covers everything the
    /// pointer can be over.
    fn code_at(&self, x: f64, y: f64) -> Option<Found> {
        if self.message.is_some() || !self.in_area(x, y) {
            return None;
        }
        let doc_y = self.doc_y(y);
        let i = self.layout.range_exact(doc_y, doc_y + 0.001).start;
        let block = self.doc.blocks.get(i)?;
        if block.kind != BlockKind::Code {
            return None;
        }
        let content = self.layout.geoms()[i].content.as_deref()?;
        let (rx, ry, rw, rh) = code_rect(content)?;
        let (col_x, _) = self.layout.column();
        let (lx, ly) = (x as f32 - col_x, (doc_y - self.layout.top(i)) as f32);
        if !(rx..rx + rw).contains(&lx) || !(ry..ry + rh).contains(&ly) {
            return None;
        }
        // The block is all its chunks; its top is the first chunk's, which has content if its
        // corner is on screen (and only then is there a button to show).
        let (lo, hi) = block_unit(&self.doc, i as u32);
        let (first, last) = (lo.block, hi.block);
        if self
            .doc
            .block_spans(&self.doc.blocks[first as usize])
            .is_empty()
        {
            return None;
        }
        let (hx, hy, hw, _) = code_rect(self.layout.geoms()[first as usize].content.as_deref()?)?;
        let scale = f64::from(self.scale());
        let (size, inset) = (f64::from(SIZE) * scale, f64::from(INSET) * scale);
        let top = self.layout.top(first as usize) + f64::from(hy);
        let height = self.layout.top(last as usize + 1) - top;
        let room = size + 2.0 * inset;
        if f64::from(hw) < room || height < room {
            return None;
        }
        let right = f64::from(col_x + hx + hw) - inset;
        let y0 = f64::from(self.top_inset) + top + inset - self.scroll();
        let button = Rect {
            x0: (right - size) as f32,
            y0: y0 as f32,
            x1: right as f32,
            y1: (y0 + size) as f32,
        };
        // A button that has scrolled out of the area isn't there to be seen or pressed.
        let (area_top, area_bottom) = (f64::from(self.top_inset), f64::from(self.height));
        (y0 + size > area_top && y0 < area_bottom).then_some(Found {
            first,
            last,
            button,
        })
    }

    /// The rectangle (window pixels) of the copy button of the code block under (`x`, `y`), if it
    /// has one.
    pub fn copy_button_at(&self, x: f64, y: f64) -> Option<Rect> {
        self.code_at(x, y).map(|f| f.button)
    }

    /// The pointer can reach the document's own things: no drag, selection or toolbar in the way.
    fn pointer_free(&self, x: f64, y: f64) -> bool {
        self.drag.is_none() && !self.selecting() && !self.toolbar.covers(&self.toolbar_ctx(), x, y)
    }

    /// The code block the pointer is over, with its button, and whether the pointer is on the
    /// button.
    fn copy_spot(&self) -> Option<(Found, bool)> {
        let (x, y) = self.copy.pointer?;
        if !self.pointer_free(x, y) {
            return None;
        }
        let found = self.code_at(x, y)?;
        Some((found, found.button.contains(x, y)))
    }

    /// Look at what is under the pointer again. Returns true if the picture changed (the button
    /// came, went, or lit up).
    pub(super) fn copy_look(&mut self) -> bool {
        let spot = self.copy_spot().map(|(found, hot)| Spot {
            block: found.first,
            hot,
            down: hot && self.copy.pressed == Some(found.first),
        });
        std::mem::replace(&mut self.copy.shown, spot) != spot
    }

    /// The pointer moved to (`x`, `y`), or left the window (`None`). Returns true if the
    /// window needs a repaint for the button.
    pub(super) fn copy_pointer(&mut self, at: Option<(f64, f64)>) -> bool {
        self.copy.pointer = at;
        self.copy_look()
    }

    /// The pointer is on a code block's copy button, which shows the hand.
    pub fn over_copy(&self) -> bool {
        self.copy.shown.is_some_and(|s| s.hot)
    }

    /// A press at (`x`, `y`): true if it is on the button of a code block, which takes it (the
    /// press starts no selection and follows no link).
    pub(super) fn press_copy(&mut self, x: f64, y: f64) -> bool {
        self.copy.pressed = None;
        if !self.pointer_free(x, y) {
            return false;
        }
        let Some(found) = self.code_at(x, y).filter(|f| f.button.contains(x, y)) else {
            return false;
        };
        self.copy.pressed = Some(found.first);
        true
    }

    /// The mouse button came up at (`x`, `y`). A release on the button the press went down on is
    /// a click: the copy of the block's code, which the shell offers to the clipboard.
    pub fn release_copy(&mut self, x: f64, y: f64) -> Option<CodeCopy> {
        let pressed = self.copy.pressed.take()?;
        self.ensure_content();
        let found = self
            .code_at(x, y)
            .filter(|f| f.first == pressed && f.button.contains(x, y))?;
        Some(CodeCopy {
            block: found.first,
            content: Content::code(&self.doc, found.first, found.last),
        })
    }

    /// The press on the button is over without a release to see (the window lost the focus).
    pub fn cancel_copy(&mut self) {
        self.copy.pressed = None;
    }

    /// The code block starting at `block` was copied: its button shows the check mark for a
    /// moment. The caller repaints.
    pub fn code_copied(&mut self, block: u32, now: Instant) {
        self.copy.copied = Some((block, now + CHECK));
    }

    /// When the check mark goes, for the event loop's wake-up.
    pub(super) fn copy_deadline(&self) -> Option<Instant> {
        self.copy.copied.map(|c| c.1)
    }

    /// Take the check mark away if its time is up. Returns true if it went.
    pub(super) fn copy_expire(&mut self, now: Instant) -> bool {
        let up = self.copy.copied.is_some_and(|c| now >= c.1);
        if up {
            self.copy.copied = None;
        }
        up
    }

    /// A new document: the blocks the button's state names are gone. (The pointer stays.)
    pub(super) fn forget_copy(&mut self) {
        let pointer = self.copy.pointer;
        self.copy = CopyButton {
            pointer,
            ..CopyButton::default()
        };
    }

    /// Paint the button of the code block under the pointer, if there is one, over the document
    /// and under what comes after it (the scrollbar, the toolbar).
    pub(super) fn paint_copy_button(
        &mut self,
        frame: &mut Frame,
        area: Clip,
        theme: &Theme,
        now: Instant,
    ) {
        self.copy_look();
        let Some((found, hot)) = self.copy_spot() else {
            return;
        };
        let down = hot && self.copy.pressed == Some(found.first);
        let checked = self
            .copy
            .copied
            .is_some_and(|(block, until)| block == found.first && now < until);
        let scale = self.scale();
        let prev = frame.set_clip(area);
        paint_button(
            frame,
            &self.fonts,
            &mut self.cache,
            theme,
            scale,
            found.button,
            Look { hot, down, checked },
        );
        frame.set_clip(prev);
    }
}

/// How the button looks.
#[derive(Clone, Copy, Debug)]
struct Look {
    /// The pointer is on it.
    hot: bool,
    /// The mouse button is held on it.
    down: bool,
    /// The code was just copied: a check mark in place of the copy icon.
    checked: bool,
}

/// Draw the button in `rect`: the page's color with an outline (so it reads over code of any
/// color), lit as the toolbar's buttons are when the pointer is on it, then its icon.
fn paint_button(
    frame: &mut Frame,
    fonts: &Fonts,
    cache: &mut GlyphCache,
    theme: &Theme,
    scale: f32,
    rect: Rect,
    look: Look,
) {
    let r = (RADIUS * scale).round();
    let edge = scale.round().max(1.0);
    frame.rounded_rect(rect.px(), r, None, theme.bg, 255);
    if look.hot {
        let alpha = if look.down { 200 } else { 110 };
        frame.rounded_rect(rect.px(), r, None, theme.border, alpha);
    }
    frame.rounded_rect(rect.px(), r, Some(edge), theme.border, 255);
    let (glyph, color) = if look.checked {
        // The green of the tip alert: it says "done" without a word.
        (icon::CHECK, theme.color(Role::AlertTip))
    } else if look.hot {
        (icon::COPY, theme.text)
    } else {
        (icon::COPY, theme.muted)
    };
    draw_icon(frame, fonts, cache, glyph, rect, scale, color);
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
