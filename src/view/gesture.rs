//! Selecting text with the mouse: the press that starts it, the drag that extends it, the clicks
//! that widen it to a word or a block, and the scrolling that follows a drag out of the window.
//! The selection itself is made of document positions (see [`crate::select`]), so nothing here
//! has to be undone when the layout changes; only where the pointer is gets looked up.

use std::time::{Duration, Instant};

use super::View;
use crate::copy::Content;
use crate::select::{Pos, Selection};

/// How far a press may move, in logical pixels (not scaled by zoom), and still be a click.
const DRAG_THRESHOLD: f64 = 4.0;
/// Presses within this time, and the drag threshold of one another, are a double or triple
/// click. winit gives no click count, and neither X11 nor Wayland tells a client the desktop's
/// interval; this is the Windows default, at or above the GTK and KDE ones.
const MULTI_CLICK: Duration = Duration::from_millis(500);
/// A pointer this near the edge of the document area, inside it, also scrolls (a maximized
/// window's edges are the screen's, so the pointer can't go past them), in logical pixels.
const EDGE_BAND: f64 = 2.0;
/// Scrolling while a drag is outside the area: this many physical pixels per second for each
/// pixel of distance past the edge (12 per logical pixel at any scale), within these limits.
const SCROLL_PER_PX: f64 = 12.0;
const SCROLL_MIN: f64 = 60.0;
const SCROLL_MAX: f64 = 4000.0;
/// The scrolling advances at about the frame rate, and never by more than this much time, so a
/// slow frame doesn't throw the document far.
const SCROLL_TICK: Duration = Duration::from_millis(16);
const SCROLL_STEP_MAX: Duration = Duration::from_millis(100);

/// The selection and the gesture making it.
#[derive(Debug, Default)]
pub(super) struct Select {
    /// The selected text: made when a press moves past the drag threshold, by a double or triple
    /// click, or by select all.
    sel: Option<Selection>,
    press: Option<Press>,
    /// The press that started the last gesture, for counting clicks.
    last: Option<Click>,
    /// Shift is held: a click extends the selection.
    shift: bool,
}

/// A mouse button held down on the document.
#[derive(Debug)]
struct Press {
    /// Where it went down, and where the pointer is now (window pixels).
    origin: (f64, f64),
    at: (f64, f64),
    /// The caret position the gesture started at.
    anchor: Pos,
    /// The pointer's movements extend the selection.
    dragging: bool,
    /// The press made or changed the selection, so PRIMARY is claimed when it is released.
    made: bool,
    /// The next step of scrolling, while the pointer is outside the area.
    scroll: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
struct Click {
    at: (f64, f64),
    when: Instant,
    count: u8,
}

impl Select {
    /// A press that starts nothing (the toolbar's, the scrollbar's): the gesture ends, and it
    /// isn't part of a double click.
    fn cancel(&mut self) {
        self.press = None;
        self.last = None;
    }

    /// The count of this press: 1, 2 or 3 for a click, double click or triple click; a fourth
    /// begins again.
    fn count(&mut self, now: Instant, at: (f64, f64), slop: f64) -> u8 {
        let n = match self.last {
            Some(c)
                if now.saturating_duration_since(c.when) <= MULTI_CLICK
                    && (at.0 - c.at.0).hypot(at.1 - c.at.1) <= slop
                    && c.count < 3 =>
            {
                c.count + 1
            }
            _ => 1,
        };
        self.last = Some(Click {
            at,
            when: now,
            count: n,
        });
        n
    }
}

impl View {
    /// The selected span, if anything is selected.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        self.select.sel.filter(|s| !s.is_empty()).map(|s| s.range())
    }

    /// What a copy of the selection is, if anything is selected: its text if that is short,
    /// else the document and the range, which the text is made from when a program pastes it.
    pub fn copy_content(&self) -> Option<Content> {
        let range = self.selection()?;
        Some(Content::of(&self.doc, range))
    }

    /// A drag is making a selection.
    pub fn selecting(&self) -> bool {
        self.select.press.as_ref().is_some_and(|p| p.dragging)
    }

    /// Shift is held down (a click then extends the selection).
    pub fn set_shift(&mut self, shift: bool) {
        self.select.shift = shift;
    }

    /// Forget a press that something else took (the toolbar's, the scrollbar's).
    pub(super) fn cancel_select(&mut self) {
        self.select.cancel();
    }

    /// The drag threshold in physical pixels.
    fn slop(&self) -> f64 {
        DRAG_THRESHOLD * self.hidpi
    }

    /// The point is in the document area.
    fn in_area(&self, x: f64, y: f64) -> bool {
        (0.0..f64::from(self.width)).contains(&x)
            && (f64::from(self.top_inset)..f64::from(self.height)).contains(&y)
    }

    /// Document y of window y, for a point in the area.
    fn doc_y(&self, y: f64) -> f64 {
        self.scroll() + (y - f64::from(self.top_inset))
    }

    /// The caret position nearest window point (`x`, `y`): a point outside the area counts as at
    /// its nearest edge.
    fn caret_at(&self, x: f64, y: f64) -> Pos {
        let x = x.clamp(0.0, (f64::from(self.width) - 1.0).max(0.0));
        let top = f64::from(self.top_inset);
        let y = y.clamp(top, (f64::from(self.height) - 1.0).max(top));
        self.layout.locate(x as f32, self.doc_y(y) as f32)
    }

    /// The position of the character under window point (`x`, `y`), if there is one.
    fn char_at(&self, x: f64, y: f64) -> Option<Pos> {
        if !self.in_area(x, y) {
            return None;
        }
        let hit = self.layout.hit(x as f32, self.doc_y(y) as f32)?;
        Some(Pos::new(hit.block, hit.doc))
    }

    /// Whether the pointer is over text, which shows the text cursor: not over the toolbar or
    /// the scrollbar, and not over a link, whose cursor is the hand.
    pub fn text_cursor(&self, x: f64, y: f64) -> bool {
        if self.selecting() {
            return true;
        }
        self.message.is_none()
            && self.drag.is_none()
            && !self.over_bar
            && self.hover.is_none()
            && !self.toolbar.covers(&self.toolbar_ctx(), x, y)
            && self.char_at(x, y).is_some()
    }

    /// A press on the document at (`x`, `y`) that nothing else took. A plain press clears the
    /// selection and may start one; a double or triple press selects a word or a block; with
    /// Shift held a press extends the selection. Returns true if the selection changed (repaint).
    pub(super) fn press_text(&mut self, x: f64, y: f64, now: Instant) -> bool {
        self.select.press = None;
        if self.message.is_some() || self.doc.blocks.is_empty() || !self.in_area(x, y) {
            self.select.last = None;
            return false;
        }
        let count = self.select.count(now, (x, y), self.slop());
        let at = self.caret_at(x, y);
        let had = self.select.sel.is_some();
        let mut press = Press {
            origin: (x, y),
            at: (x, y),
            anchor: at,
            dragging: false,
            made: false,
            scroll: None,
        };
        match count {
            1 if self.select.shift && had => {
                if let Some(sel) = self.select.sel.as_mut() {
                    sel.extend(&self.doc, at);
                }
                (press.dragging, press.made) = (true, true);
                // Extending the selection isn't a click on a link under the pointer.
                self.pressed = None;
            }
            1 => self.select.sel = None,
            // The first click of a double click already followed a link (as it does today); the
            // second doesn't select a word of it, or follow it again.
            _ if self.link_at(x, y).is_some() => {
                self.pressed = None;
                return false;
            }
            2 => {
                let under = self.char_at(x, y).unwrap_or(at);
                self.select.sel = Some(Selection::word(&self.doc, under));
                (press.dragging, press.made) = (true, true);
            }
            _ => {
                self.select.sel = Some(Selection::block(&self.doc, at));
                (press.dragging, press.made) = (true, true);
            }
        }
        self.select.press = Some(press);
        had || press_made(&self.select)
    }

    /// The pointer moved to (`x`, `y`) with the button down. The first movement past the drag
    /// threshold starts the selection (and the press is no longer a click on a link); later
    /// ones extend it, and a pointer outside the document area scrolls it. Returns true if the
    /// window needs a repaint.
    pub fn select_to(&mut self, x: f64, y: f64, now: Instant) -> bool {
        let slop = self.slop();
        let Some(press) = self.select.press.as_mut() else {
            return false;
        };
        press.at = (x, y);
        let mut changed = false;
        if !press.dragging {
            let (dx, dy) = (x - press.origin.0, y - press.origin.1);
            if dx.hypot(dy) < slop {
                return false;
            }
            (press.dragging, press.made) = (true, true);
            self.select.sel = Some(Selection::caret(press.anchor));
            self.pressed = None;
            changed = true;
        }
        // Armed once; the steps keep their own pace until the pointer comes back.
        let outside = self.edge_speed(y).is_some();
        if let Some(press) = self.select.press.as_mut() {
            press.scroll = if outside {
                press.scroll.or(Some(now))
            } else {
                None
            };
        }
        self.ensure_content();
        changed | self.refocus()
    }

    /// Put the focus of the selection at the pointer again. Returns true if the selected span
    /// changed.
    fn refocus(&mut self) -> bool {
        let Some(press) = self.select.press.as_ref().filter(|p| p.dragging) else {
            return false;
        };
        let pos = self.caret_at(press.at.0, press.at.1);
        let Some(sel) = self.select.sel.as_mut() else {
            return false;
        };
        let before = sel.range();
        sel.extend(&self.doc, pos);
        sel.range() != before
    }

    /// The document moved under a held pointer (the wheel, a key): the selection follows it.
    pub(super) fn follow_pointer(&mut self) {
        if self.selecting() {
            self.ensure_content();
            self.refocus();
        }
    }

    /// Signed pixels per second to scroll for a pointer at window `y`: `None` inside the area,
    /// negative above it.
    fn edge_speed(&self, y: f64) -> Option<f64> {
        let (top, bottom) = (f64::from(self.top_inset), f64::from(self.height));
        let band = EDGE_BAND * self.hidpi;
        let (past, dir) = if y < top + band {
            (top - y, -1.0)
        } else if y >= bottom - band {
            (y - bottom, 1.0)
        } else {
            return None;
        };
        Some(dir * (past.max(0.0) * SCROLL_PER_PX).clamp(SCROLL_MIN, SCROLL_MAX))
    }

    /// When the loop must wake to scroll for a drag outside the area.
    pub(super) fn select_deadline(&self) -> Option<Instant> {
        self.select.press.as_ref()?.scroll.map(|t| t + SCROLL_TICK)
    }

    /// Scroll a step if one is due. Returns true if the window needs a repaint.
    pub(super) fn autoscroll(&mut self, now: Instant) -> bool {
        let Some(press) = self.select.press.as_ref() else {
            return false;
        };
        let (Some(last), y) = (press.scroll, press.at.1) else {
            return false;
        };
        if now < last + SCROLL_TICK {
            return false;
        }
        let Some(speed) = self.edge_speed(y) else {
            if let Some(p) = self.select.press.as_mut() {
                p.scroll = None;
            }
            return false;
        };
        let secs = now.saturating_duration_since(last).min(SCROLL_STEP_MAX);
        let before = self.scroll();
        self.set_scroll(before + speed * secs.as_secs_f64());
        let moved = self.scroll() != before;
        self.fade.poke(now);
        // At the end of the document there is nothing to wait for until the pointer moves.
        if let Some(p) = self.select.press.as_mut() {
            p.scroll = moved.then_some(now);
        }
        self.ensure_content();
        self.refocus() | moved
    }

    /// The mouse button came up. Returns true if the press made a selection, which is then
    /// worth offering as the primary selection.
    pub fn release_select(&mut self) -> bool {
        let made = self.select.press.take().is_some_and(|p| p.made);
        made && self.selection().is_some()
    }

    /// Select the whole document. Returns false when there is none to select.
    pub(super) fn select_all(&mut self) -> bool {
        self.select.press = None;
        if self.message.is_some() {
            return false;
        }
        self.select.sel = Selection::all(&self.doc);
        self.select.sel.is_some()
    }

    /// Drop the selection (Escape, a reload). Returns true if there was one.
    pub(super) fn clear_selection(&mut self) -> bool {
        self.select.press = None;
        self.select.sel.take().is_some()
    }
}

/// Whether the press that was just recorded made or changed a selection.
fn press_made(select: &Select) -> bool {
    select.press.as_ref().is_some_and(|p| p.made)
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
