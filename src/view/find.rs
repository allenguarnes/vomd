//! Find in page: a bar below the toolbar with the query, the number of matches and buttons to
//! step through them, and the matches washed behind the text.
//!
//! The bar is docked, never over the document: it is a strip of fixed height that the document
//! area starts below (see [`View::chrome_inset`]), so opening it keeps the reading position as the
//! toolbar's showing does. It is chrome, whose sizes are logical pixels that scale with the display
//! like the toolbar's, not with the document's zoom. There is one input, the query, which has the
//! keyboard while the bar is open (see [`View::find_key`]), whatever is clicked.
//!
//! The matches come from the document's text (see [`crate::find`]) and are found again on every
//! edit of the query, on a reload and for a new document. Stepping to a match scrolls it to a
//! third of the way down the document area unless it is wholly in view already; it is looked up
//! by its byte offset in the lines of its block, so a match in a block that has not been shaped
//! yet is reached by shaping the block first.

use std::time::Instant;

use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::{Outcome, View};
use crate::find::{self, Focus, Found, MAX_QUERY, Match};
use crate::layout::{Anchor, BlockBox};
use crate::render::{self, Clip, Frame};
use crate::text::{Align, Para, TextStyle};
use crate::theme::{Role, Theme};
use crate::toolbar::{self, Rect, icon};

/// The bar's height, a button's size, the margin of the buttons at the window's edge, the gap
/// between the field and the count and the buttons, and the text's size, in logical pixels.
const BAR: f32 = 32.0;
const BUTTON: f32 = 32.0;
const EDGE: f32 = 4.0;
const GAP: f32 = 8.0;
const TEXT: f32 = 13.0;
/// The field's margin from the window's left edge and from the bar's top and bottom, and the room
/// between its edge and the text.
const MARGIN: f32 = 8.0;
const FIELD_Y: f32 = 4.0;
const FIELD_PAD: f32 = 8.0;
/// The most passes made to bring a match to its place: measuring the blocks on the way changes
/// where it is.
const PASSES: usize = 5;
/// Where in the document area, as a fraction of its height, a match scrolled to sits.
const PLACE: f64 = 1.0 / 3.0;

/// What the pointer can be on in the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Prev,
    Next,
    Close,
    /// Anywhere else on the bar: the field, which keeps the focus it has.
    Field,
}

impl Spot {
    fn label(self) -> &'static str {
        match self {
            Spot::Prev => "Previous match (Shift+Enter)",
            Spot::Next => "Next match (Enter)",
            Spot::Close => "Close (Esc)",
            Spot::Field => "",
        }
    }

    fn icon(self) -> char {
        match self {
            Spot::Prev => icon::CHEVRON_UP,
            Spot::Next => icon::CHEVRON_DOWN,
            Spot::Close | Spot::Field => icon::CLOSE,
        }
    }
}

/// The bar's state: whether it is open, the query, and what it found.
#[derive(Default)]
pub(super) struct Bar {
    pub open: bool,
    pub query: String,
    /// The query shows selected, so the next character typed replaces it.
    fresh: bool,
    /// Where the caret was last painted, as a physical x.
    caret: f32,
    pub found: Found,
    /// The current match, an index into `found.matches`.
    pub current: Option<usize>,
    /// Where the pointer is while it is over the bar, and what it is on.
    pointer: Option<(f64, f64)>,
    hover: Option<Spot>,
    pressed: Option<Spot>,
    /// When the pointer came to rest on the hovered button (the tooltip's timer).
    hover_since: Option<Instant>,
    tooltip: bool,
}

impl View {
    /// The bar is open.
    pub fn find_open(&self) -> bool {
        self.find.open
    }

    /// The query, as typed.
    pub fn find_query(&self) -> &str {
        &self.find.query
    }

    /// The matches in reading order (none while the bar is closed): all of them, or a window of
    /// at most [`find::WINDOW`] around the current one.
    pub fn find_matches(&self) -> &[Match] {
        &self.find.found.matches
    }

    /// The number of matches in the document, of which [`View::find_matches`] holds a window.
    pub fn find_total(&self) -> usize {
        self.find.found.total
    }

    /// What the search found, of which [`View::find_matches`] is the matches.
    pub fn find_found(&self) -> &Found {
        &self.find.found
    }

    /// The current match, an index into [`View::find_matches`].
    pub fn find_current(&self) -> Option<usize> {
        self.find.current
    }

    /// What the bar says about the matches: "3 / 17", "No matches", nothing for an empty query.
    pub fn find_count(&self) -> String {
        let f = &self.find.found;
        match self.find.current {
            _ if self.find.query.is_empty() => String::new(),
            Some(i) => format!("{} / {}", f.first + i + 1, f.total),
            None => "No matches".into(),
        }
    }

    /// The bar's height in physical pixels: 0 while it is closed.
    pub(super) fn find_height(&self) -> u32 {
        if self.find.open {
            (BAR * self.hidpi as f32).round() as u32
        } else {
            0
        }
    }

    /// Where the bar is: its top is the toolbar's bottom.
    fn find_rect(&self) -> Rect {
        let top = self.toolbar.height(self.hidpi as f32) as f32;
        Rect {
            x0: 0.0,
            y0: top,
            x1: self.width as f32,
            y1: top + self.find_height() as f32,
        }
    }

    /// The buttons of the bar, from the right edge.
    fn find_buttons(&self) -> [(Spot, Rect); 3] {
        let (s, bar) = (self.hidpi as f32, self.find_rect());
        let button = |n: f32| Rect {
            x0: bar.x1 - (EDGE + n * BUTTON) * s,
            x1: bar.x1 - (EDGE + (n - 1.0) * BUTTON) * s,
            ..bar
        };
        [
            (Spot::Close, button(1.0)),
            (Spot::Next, button(2.0)),
            (Spot::Prev, button(3.0)),
        ]
    }

    fn find_spot_at(&self, x: f64, y: f64) -> Option<Spot> {
        if !self.find_covers(x, y) {
            return None;
        }
        let hit = self
            .find_buttons()
            .into_iter()
            .find(|(_, r)| r.contains(x, y));
        Some(hit.map_or(Spot::Field, |(spot, _)| spot))
    }

    /// The point is on the bar, which takes it.
    pub(super) fn find_covers(&self, x: f64, y: f64) -> bool {
        self.find.open && self.find_rect().contains(x, y)
    }

    // ---- Opening and closing ---------------------------------------------------------------

    /// Show or hide the bar. Opening takes a short selection for the query and goes to the first
    /// match from the reading position; closing clears the matches but keeps the query. The
    /// document area gives up or reclaims the bar's height, keeping the reading position.
    pub fn set_find(&mut self, open: bool) {
        if std::mem::replace(&mut self.find.open, open) == open {
            return;
        }
        self.find.pointer = None;
        self.find.hover = None;
        self.find.pressed = None;
        self.find.hover_since = None;
        self.find.tooltip = false;
        self.find.fresh = true;
        self.set_top_inset(self.chrome_inset());
        if open {
            // Opening takes the toolbar's menu away (the bar would sit under it); closing leaves
            // it, one Escape being one layer.
            self.close_menu();
            self.find_take_selection();
            self.find_refresh(None, true);
        } else {
            self.find.found = Found::default();
            self.find.current = None;
        }
    }

    /// Ctrl+F: open the bar, or when it is open put the focus back on the query, which shows
    /// selected (and is the selected text, if there is a short selection).
    pub(super) fn find_focus(&mut self) {
        if !self.find.open {
            self.set_find(true);
            return;
        }
        self.find.fresh = true;
        if self.find_take_selection() {
            self.find_refresh(None, true);
        }
    }

    /// Escape closes the bar, after the toolbar's menu above it. Returns true if it did.
    pub(super) fn find_escape(&mut self) -> bool {
        let was = self.find.open;
        self.set_find(false);
        was
    }

    /// The selected text, if it is one line of at most [`MAX_QUERY`] characters, becomes the
    /// query. Returns true if the query changed.
    #[cfg(feature = "clipboard")]
    fn find_take_selection(&mut self) -> bool {
        let Some(crate::copy::Content::Text(text)) = self.copy_content() else {
            return false;
        };
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() || text.chars().count() > MAX_QUERY || text.contains(['\r', '\n']) {
            return false;
        }
        self.find.fresh = true;
        std::mem::replace(&mut self.find.query, text.to_owned()) != text
    }

    #[cfg(not(feature = "clipboard"))]
    fn find_take_selection(&mut self) -> bool {
        false
    }

    // ---- Typing ----------------------------------------------------------------------------

    /// A key press while the bar is open goes to the query before it is a shortcut of the
    /// document: text is typed (it is `text` of the key event, so layouts and dead keys work),
    /// Backspace deletes, Enter and F3 step, Ctrl+Backspace deletes a word. Shortcuts with Ctrl
    /// or Cmd and the keys that are no text (arrows, pages) go on to the document, which is what
    /// `None` says; Escape does as well, for [`View::apply`] to close the bar first.
    pub fn find_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
        now: Instant,
    ) -> Option<Outcome> {
        if !self.find.open {
            return None;
        }
        let command = mods.control_key() || mods.super_key();
        let dir = if mods.shift_key() { -1 } else { 1 };
        match key {
            Key::Named(NamedKey::Enter | NamedKey::F3) => {
                self.find_step(dir, now);
                return Some(Outcome::Redraw);
            }
            Key::Character(c) if command && matches!(c.as_str(), "g" | "G") => {
                self.find_step(dir, now);
                return Some(Outcome::Redraw);
            }
            Key::Named(NamedKey::Backspace) if !mods.alt_key() => {
                self.find_backspace(command);
                return Some(Outcome::Redraw);
            }
            _ => {}
        }
        if command || mods.alt_key() {
            return None;
        }
        let typed: String = text?.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return None;
        }
        self.find_type(&typed);
        Some(Outcome::Redraw)
    }

    /// Type `text` (a key's, or what an input method committed) into the query: it replaces a
    /// query that shows selected, else goes at its end, up to [`MAX_QUERY`] characters.
    pub fn find_type(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if !self.find.open || text.is_empty() {
            return;
        }
        let replaced = std::mem::take(&mut self.find.fresh);
        if replaced {
            self.find.query.clear();
        }
        let room = MAX_QUERY.saturating_sub(self.find.query.chars().count());
        self.find.query.extend(text.chars().take(room));
        self.find_edited(replaced);
    }

    /// Delete the last character of the query, or its last word (and the white space after it)
    /// for `word`; a query that shows selected goes whole.
    fn find_backspace(&mut self, word: bool) {
        let q = &mut self.find.query;
        let replaced = std::mem::take(&mut self.find.fresh);
        if replaced {
            q.clear();
        } else if word {
            while q.ends_with(char::is_whitespace) {
                q.pop();
            }
            while q.chars().next_back().is_some_and(|c| !c.is_whitespace()) {
                q.pop();
            }
        } else {
            q.pop();
        }
        self.find_edited(replaced);
    }

    /// The query changed: look again from where the current match was, so typing on keeps it, as
    /// long as it is what the reader is looking at. A new query (`replaced`), or a current match
    /// that has been scrolled out of view, is looked for from the reading position.
    fn find_edited(&mut self, replaced: bool) {
        let from = self
            .find_current_start()
            .filter(|_| !replaced && self.find_current_in_view());
        self.find_refresh(from, true);
    }

    /// Some of the current match is in the document area. A match in a block that has no content
    /// is not.
    fn find_current_in_view(&self) -> bool {
        let found = &self.find.found;
        let Some(&m) = self.find.current.and_then(|i| found.matches.get(i)) else {
            return false;
        };
        let (scroll, height) = (self.scroll(), f64::from(self.area_height()));
        self.find_extent(m)
            .is_some_and(|(top, bottom)| bottom > scroll && top < scroll + height)
    }

    // ---- Matches ---------------------------------------------------------------------------

    /// Where the current match begins.
    fn find_current_start(&self) -> Option<(u32, u32)> {
        let m = self.find.found.matches.get(self.find.current?)?;
        Some((m.block, m.start))
    }

    /// Where the reading position is as a block and byte offset: the start of the first line of
    /// the window's top block that is not wholly above the window, or the block's start when it
    /// is not shaped. A match that starts before it is behind the reader.
    fn find_reading_point(&self) -> (u32, u32) {
        let block = self.anchor.block;
        let content = self.layout.geoms().get(block as usize);
        let Some(content) = content.and_then(|g| g.content.as_deref()) else {
            return (block, 0);
        };
        let (top, scroll) = (self.layout.top(block as usize), self.scroll());
        let line = content.lines.iter().find_map(|line| {
            if top + f64::from(line.bottom) <= scroll {
                return None;
            }
            let hits = &content.hits[line.hits.start as usize..line.hits.end as usize];
            hits.iter().map(|h| h.doc).min()
        });
        line.map_or((block + 1, 0), |off| (block, off))
    }

    /// Search the document again and pick the current match: the first at or after `from` (a
    /// block and byte offset), or the reading position without one, wrapping around. With
    /// `reveal` the view scrolls to it.
    pub(super) fn find_refresh(&mut self, from: Option<(u32, u32)>, reveal: bool) {
        let (block, off) = from.unwrap_or_else(|| self.find_reading_point());
        let found = find::search_from(&self.doc, &self.find.query, Focus::At(block, off));
        self.find.current = (found.total > 0).then(|| found.at - found.first);
        self.find.found = found;
        if reveal {
            self.find_reveal();
        }
    }

    /// Step to the next (`dir` > 0) or previous match, around the ends, and scroll to it. A match
    /// outside the window kept is reached by searching again with the window around it.
    pub fn find_step(&mut self, dir: i32, now: Instant) {
        let f = &self.find.found;
        let Some(current) = self.find.current.filter(|_| f.total > 0) else {
            return;
        };
        let number = f.first + current;
        let next = if dir < 0 {
            (number + f.total - 1) % f.total
        } else {
            (number + 1) % f.total
        };
        if (f.first..f.first + f.matches.len()).contains(&next) {
            self.find.current = Some(next - f.first);
        } else {
            let found = find::search_from(&self.doc, &self.find.query, Focus::Index(next));
            self.find.current = Some(found.at - found.first);
            self.find.found = found;
        }
        self.find_reveal();
        self.fade.poke(now);
    }

    /// The document y of the top of the first line and the bottom of the last that match `m`
    /// occupies, if its blocks are shaped: the lines the highlight is painted on (see
    /// [`find::BlockMarks::rects`]).
    fn find_extent(&self, m: Match) -> Option<(f64, f64)> {
        let marks = find::Marks::one(&m);
        let mut extent: Option<(f64, f64)> = None;
        let mut grow = |top: f64, y0: f32, y1: f32| {
            let (y0, y1) = (top + f64::from(y0), top + f64::from(y1));
            extent = Some(extent.map_or((y0, y1), |(a, b)| (a.min(y0), b.max(y1))));
        };
        let chunks = || {
            let rest = self.doc.blocks.iter().enumerate().skip(m.block as usize);
            rest.take_while(|&(b, block)| b == m.block as usize || block.joined)
        };
        for (b, _) in chunks() {
            let Some(content) = self.layout.geoms()[b].content.as_deref() else {
                if b == m.block as usize {
                    return None;
                }
                continue;
            };
            if let Some(marks) = marks.of_block(&self.doc, b as u32) {
                let top = self.layout.top(b);
                marks.rects(content, |_, y0, _, y1, _| grow(top, y0, y1));
            }
        }
        // A match of whitespace the layout left out shows nothing: it sits on the line of the
        // text near it, or, with none in its block, at the top of the block.
        extent
            .or_else(|| {
                chunks().find_map(|(b, _)| {
                    let content = self.layout.geoms()[b].content.as_deref()?;
                    let (y0, y1) = find::line_of(&self.doc, b as u32, content, m.start)?;
                    let top = self.layout.top(b);
                    Some((top + f64::from(y0), top + f64::from(y1)))
                })
            })
            .or_else(|| {
                let content = self.layout.geoms()[m.block as usize].content.as_deref()?;
                let top = self.layout.top(m.block as usize);
                let (y0, y1) = content
                    .lines
                    .first()
                    .map_or((0.0, 0.0), |l| (l.top, l.bottom));
                Some((top + f64::from(y0), top + f64::from(y1)))
            })
    }

    /// Scroll to the current match unless it is wholly in view: its first line goes a third of
    /// the way down the document area. A match in a block without content has the block shaped
    /// first, and the position is set again as often as measuring what lies between moves it.
    fn find_reveal(&mut self) {
        let Some(&m) = self
            .find
            .current
            .and_then(|i| self.find.found.matches.get(i))
        else {
            return;
        };
        if self.message.is_some() {
            return;
        }
        let height = f64::from(self.area_height());
        let wanted = |view: &View, top: f64| (top - PLACE * height).clamp(0.0, view.max_scroll());
        // Blocks near the window are shaped, so one that is not is out of view.
        let shaped = self.layout.geoms()[m.block as usize].content.is_some();
        for pass in 0..PASSES {
            if self.layout.geoms()[m.block as usize].content.is_none() {
                self.set_anchor(Anchor {
                    block: m.block,
                    frac: 0.0,
                });
            }
            let Some((top, bottom)) = self.find_extent(m) else {
                return;
            };
            let scroll = self.scroll();
            // In view already (judged before the block was brought up to be measured).
            if pass == 0 && shaped && top >= scroll && bottom <= scroll + height {
                return;
            }
            let to = wanted(self, top);
            if (to - scroll).abs() < 0.5 {
                return;
            }
            self.set_scroll(to);
            self.ensure();
        }
    }

    // ---- Mouse -----------------------------------------------------------------------------

    /// A press at (`x`, `y`): true if the bar takes it. A press on a button is the start of a
    /// click, anywhere else on the bar it does nothing (the field keeps the focus).
    pub(super) fn press_find(&mut self, x: f64, y: f64) -> bool {
        self.find.pressed = self.find_spot_at(x, y);
        self.find.tooltip = false;
        self.find.hover_since = None;
        self.find.pressed.is_some()
    }

    /// The button came up at (`x`, `y`) after a press on the bar: a click if it is on the same
    /// button. Returns true if the window needs a repaint.
    pub(super) fn release_find(&mut self, x: f64, y: f64, now: Instant) -> bool {
        let Some(pressed) = self.find.pressed.take() else {
            return false;
        };
        if self.find_spot_at(x, y) == Some(pressed) {
            match pressed {
                Spot::Prev => self.find_step(-1, now),
                Spot::Next => self.find_step(1, now),
                Spot::Close => self.set_find(false),
                Spot::Field => {}
            }
        }
        true
    }

    /// The pointer moved to `at`, or left the window (`None`). The toolbar's dropdown hangs over
    /// the bar: a pointer under it is the dropdown's, so no button lights up. Returns true if the
    /// bar needs a repaint.
    pub(super) fn find_pointer(&mut self, at: Option<(f64, f64)>, now: Instant) -> bool {
        self.find.pointer = at.filter(|&(x, y)| self.find_covers(x, y));
        let ctx = self.toolbar_ctx();
        let spot = self
            .find
            .pointer
            .filter(|&(x, y)| !self.toolbar.menu_covers(&ctx, x, y))
            .and_then(|(x, y)| self.find_spot_at(x, y))
            .filter(|s| *s != Spot::Field);
        if spot == self.find.hover {
            return false;
        }
        let changed = self.find.hover.is_some() || self.find.tooltip || spot.is_some();
        self.find.hover = spot;
        self.find.tooltip = false;
        self.find.hover_since = spot.map(|_| now);
        changed
    }

    /// The wheel over the bar does nothing: true if the pointer is on it. Where the toolbar's
    /// dropdown hangs over the bar the wheel is the dropdown's.
    pub(super) fn find_wheel(&self) -> bool {
        let ctx = self.toolbar_ctx();
        self.find
            .pointer
            .is_some_and(|(x, y)| !self.toolbar.menu_covers(&ctx, x, y))
    }

    /// When the tooltip of the hovered button is due.
    pub(super) fn find_deadline(&self) -> Option<Instant> {
        if self.find.tooltip {
            return None;
        }
        self.find.hover_since.map(|t| t + toolbar::TOOLTIP_DELAY)
    }

    /// Show the tooltip if its time has come. Returns true if the window needs a repaint.
    pub(super) fn find_expire(&mut self, now: Instant) -> bool {
        match self.find_deadline() {
            Some(t) if now >= t => {
                self.find.tooltip = true;
                true
            }
            _ => false,
        }
    }

    /// A new text (a reload, another document): the query stays, and the matches are those of the
    /// new text, the current one the first from the reading position (nothing scrolls).
    pub(super) fn find_again(&mut self) {
        self.find.pointer = None;
        self.find.hover = None;
        self.find.pressed = None;
        self.find.hover_since = None;
        self.find.tooltip = false;
        if self.find.open {
            self.find_refresh(None, false);
        }
    }

    /// Where an input method puts its window: at the caret, at the end of the query as last
    /// painted, in physical pixels.
    pub fn find_ime_area(&self) -> (u32, u32, u32, u32) {
        let (s, bar) = (self.hidpi as f32, self.find_rect());
        let x = self.find.caret.max(bar.x0);
        (x as u32, bar.y0 as u32, (4.0 * s) as u32, (BAR * s) as u32)
    }

    /// The right edge of the field's room: left of the count, which is `width` wide if there is
    /// one, and of the buttons.
    fn find_count_x(&self, width: Option<f32>) -> f32 {
        let s = self.hidpi as f32;
        let x = self.find_buttons()[2].1.x0 - GAP * s;
        width.map_or(x, |w| x - w - GAP * s)
    }

    /// The field, and the room inside it for the text, when the room to its right ends at
    /// `text_x1`.
    fn find_field(&self, text_x1: f32) -> (Rect, Rect) {
        let (s, bar) = (self.hidpi as f32, self.find_rect());
        let line = s.round().max(1.0);
        let field = Rect {
            x0: MARGIN * s,
            y0: bar.y0 + FIELD_Y * s,
            x1: text_x1.max((MARGIN + 2.0 * FIELD_PAD) * s),
            y1: bar.y1 - FIELD_Y * s - line,
        };
        let inner = Rect {
            x0: field.x0 + FIELD_PAD * s,
            x1: field.x1 - FIELD_PAD * s,
            ..field
        };
        (field, inner)
    }

    /// The width the query may take: the caret needs room of its own at the text's end.
    fn find_room(&self, inner: Rect) -> f32 {
        let s = self.hidpi as f32;
        (inner.x1 - inner.x0 - 2.0 * s).max(TEXT * s)
    }

    fn find_query_style(&self) -> TextStyle {
        TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: TEXT * self.hidpi as f32,
            role: Role::Text,
            deco: 0,
            shift: 0.0,
            link: 0,
        }
    }

    // ---- Painting --------------------------------------------------------------------------

    /// `raw` in one line no wider than `max_w`, cut from the left with a leading `…` when it is
    /// wider (the end of a long query is where the caret is), at the longest tail that fits when
    /// measured.
    fn fit_tail(
        &mut self,
        raw: &str,
        style: TextStyle,
        max_w: f32,
    ) -> (Para, crate::text::ParaLayout) {
        let chars: Vec<(usize, char)> = raw.char_indices().collect();
        let tail = |n: usize| -> String {
            let from = chars
                .get(chars.len().saturating_sub(n))
                .map_or(raw.len(), |c| c.0);
            if n < chars.len() {
                format!("…{}", &raw[from..])
            } else {
                raw.to_owned()
            }
        };
        let lay = |view: &mut View, text: &str| {
            let mut para = Para::default();
            para.push_text(text, 0, style);
            let pl = view
                .msg_ctx
                .layout(&view.fonts, &para, 1.0e6, Align::Start, 1.4);
            (para, pl)
        };
        let whole = lay(self, raw);
        if whole.1.width <= max_w {
            return whole;
        }
        // The longest tail that fits (widths grow with it); none still shows `…`.
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if lay(self, &tail(mid)).1.width <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lay(self, &tail(lo))
    }

    /// One line of the bar's text in `style`, laid out (the characters no face has are noted for
    /// the fallback scan).
    fn find_line(&mut self, text: &str, role: Role) -> (BlockBox, f32, f32) {
        let s = self.hidpi as f32;
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: TEXT * s,
            role,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let mut para = Para::default();
        para.push_text(text, 0, style);
        let pl = self
            .msg_ctx
            .layout(&self.fonts, &para, 1.0e6, Align::Start, 1.4);
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let (w, h) = (pl.width, pl.height);
        (BlockBox::from_para(pl, &para.styles), w, h)
    }

    fn paint_text(&mut self, frame: &mut Frame, content: &BlockBox, x: f32, y: f32, theme: &Theme) {
        render::paint_block(
            frame,
            content,
            x,
            y,
            &self.fonts,
            &mut self.cache,
            theme,
            #[cfg(feature = "clipboard")]
            None,
        );
    }

    /// Paint the bar, then the tooltip of the hovered button. The toolbar's dropdown, painted
    /// after, goes over it.
    pub(super) fn paint_find(&mut self, frame: &mut Frame, theme: &Theme) {
        if !self.find.open {
            return;
        }
        let (s, bar) = (self.hidpi as f32, self.find_rect());
        let (x0, y0, x1, y1) = bar.px();
        let line = s.round().max(1.0) as i32;
        let area = Clip { x0, y0, x1, y1 };
        let prev = frame.set_clip(area);
        frame.fill_rect(x0, y0, x1, y1, theme.code_bg);
        frame.fill_rect(x0, y1 - line, x1, y1, theme.border);
        let buttons = self.find_buttons();
        let steps = !self.find.found.matches.is_empty();
        for &(spot, rect) in &buttons {
            let enabled = steps || spot == Spot::Close;
            let hot = enabled && self.find.hover == Some(spot);
            if hot {
                let inset = 3.0 * s;
                let r = Rect {
                    x0: rect.x0 + inset,
                    y0: rect.y0 + inset,
                    x1: rect.x1 - inset,
                    y1: rect.y1 - inset,
                };
                let down = self.find.pressed == Some(spot);
                let alpha = if down { 200 } else { 110 };
                frame.rounded_rect(r.px(), (4.0 * s).round(), None, theme.border, alpha);
            }
            let color = if !enabled {
                theme.border
            } else if hot {
                theme.text
            } else {
                theme.muted
            };
            let (fonts, cache) = (&self.fonts, &mut self.cache);
            toolbar::draw_icon(frame, fonts, cache, spot.icon(), rect, s, color);
        }
        // The count, right-aligned against the buttons.
        let count = self.find_count();
        let mut counted = None;
        if !count.is_empty() {
            let (content, w, h) = self.find_line(&count, Role::Muted);
            let x = self.find_count_x(None) - w;
            self.paint_text(frame, &content, x, bar.y0 + (BAR * s - h) / 2.0, theme);
            counted = Some(w);
        }
        // The field: the query with the caret after it, or shown selected.
        let (field, inner) = self.find_field(self.find_count_x(counted));
        let r = (4.0 * s).round();
        frame.rounded_rect(field.px(), r, None, theme.bg, 255);
        frame.rounded_rect(field.px(), r, Some(line as f32), theme.border, 255);
        let (fx0, fy0, fx1, fy1) = inner.px();
        let inside = frame.set_clip(Clip {
            x0: fx0,
            y0: fy0,
            x1: fx1,
            y1: fy1,
        });
        let room = self.find_room(inner);
        if self.find.query.is_empty() {
            let (content, _, h) = self.find_line("Find in page", Role::Muted);
            let y = field.y0 + (field.y1 - field.y0 - h) / 2.0;
            self.paint_text(frame, &content, inner.x0, y, theme);
        }
        let query = self.find.query.clone();
        let style = self.find_query_style();
        let (para, pl) = self.fit_tail(&query, style, room);
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let (w, h) = (pl.width, pl.height);
        let y = field.y0 + (field.y1 - field.y0 - h) / 2.0;
        if !query.is_empty() {
            let content = BlockBox::from_para(pl, &para.styles);
            if self.find.fresh {
                let pad = 2.0 * s;
                let wash = (
                    (inner.x0 - pad).round() as i32,
                    y.round() as i32,
                    (inner.x0 + w + pad).round() as i32,
                    (y + h).round() as i32,
                );
                frame.rounded_rect(wash, (2.0 * s).round(), None, theme.link, 70);
            }
            self.paint_text(frame, &content, inner.x0, y, theme);
        }
        self.find.caret = inner.x0 + w + s;
        if !self.find.fresh || query.is_empty() {
            let cx = (inner.x0 + w + s).round() as i32;
            let top = (field.y0 + 6.0 * s).round() as i32;
            let bottom = (field.y1 - 6.0 * s).round() as i32;
            frame.fill_rect(cx, top, cx + line.max(1), bottom, theme.text);
        }
        frame.set_clip(inside);
        frame.set_clip(prev);
        if self.find.tooltip
            && let Some(spot) = self.find.hover
            && let Some(&(_, rect)) = buttons.iter().find(|(sp, _)| *sp == spot)
        {
            let ctx = self.toolbar_ctx();
            toolbar::paint_tooltip(
                frame,
                &ctx,
                &self.fonts,
                &mut self.cache,
                &mut self.msg_ctx,
                theme,
                spot.label(),
                rect,
            );
        }
    }
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
