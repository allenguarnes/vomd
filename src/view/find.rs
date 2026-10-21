//! Find in page: a bar below the toolbar with the query, the number of matches and buttons to
//! step through them, and the matches washed behind the text.
//!
//! The bar is docked, never over the document: it is a strip of fixed height that the document
//! area starts below (see [`View::chrome_inset`]), so opening it keeps the reading position as the
//! toolbar's showing does. It is chrome, whose sizes are logical pixels that scale with the display
//! like the toolbar's, not with the document's zoom. There is one input, the query, which has the
//! keyboard while the bar is open (see [`View::find_key`]), until a click on the document takes it
//! away: the bar stays open, and the keys are the document's again.
//!
//! The matches come from the document's text (see [`crate::find`]) and are found again on every
//! edit of the query, on a reload and for a new document. Stepping to a match scrolls it to a
//! third of the way down the document area unless it is wholly in view already; it is looked up
//! by its byte offset in the lines of its block, so a match in a block that has not been shaped
//! yet is reached by shaping the block first.

use std::time::{Duration, Instant};

use winit::keyboard::{Key, ModifiersState, NamedKey};

use self::field::{Field, Motion};
use super::clicks::Clicks;
use super::{Outcome, View};
#[cfg(feature = "clipboard")]
use crate::clipboard::Which;
#[cfg(feature = "clipboard")]
use crate::copy::Content;
use crate::find::{self, Focus, Found, Match};
use crate::layout::{self, Anchor, BlockBox};
use crate::render::{self, Clip, Frame};
use crate::text::{Align, PHit, Para, TextStyle};
use crate::theme::{Role, Theme};
use crate::toolbar::{self, Rect, icon};

mod field;

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
/// How far a press may move, in logical pixels, and still be part of a double click.
const CLICK_SLOP: f64 = 4.0;
/// While a drag in the field is past its end, the text scrolls this many physical pixels per
/// second for each pixel of the pointer's distance past it, within these limits, a step at about
/// the frame rate (see the document's drag, which does the same).
const SCROLL_PER_PX: f64 = 12.0;
const SCROLL_MIN: f64 = 60.0;
const SCROLL_MAX: f64 = 1500.0;
const SCROLL_TICK: Duration = Duration::from_millis(16);
const SCROLL_STEP_MAX: Duration = Duration::from_millis(100);
/// How much of the selection's wash shows while the field doesn't have the keyboard, of 255.
const DIM: u8 = 110;
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
    /// Anywhere else on the bar: the field if it is on that, which takes the focus, and else
    /// nothing, the field keeping the focus it has.
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
    field: Field,
    /// The field has the keyboard: typing goes to it, and it shows a caret. A click on the
    /// document takes the keyboard away without closing the bar.
    focused: bool,
    /// Counts the changes of the field's focus and of the bar's being open, so a paste that was
    /// asked for before one of them is known (see [`View::find_ticket`]).
    epoch: u32,
    /// How far the text is scrolled to the left in the field, in physical pixels.
    scroll: f32,
    /// Where the caret was last put, as a physical x in the window.
    caret: f32,
    /// The field and the room inside it, as last laid out.
    rects: Option<(Rect, Rect)>,
    /// The mouse button is held down on the field.
    drag: Option<Drag>,
    clicks: Clicks,
    /// Shift is held: a click extends the selection.
    shift: bool,
    /// The text a cut took out, for the shell to offer as the clipboard.
    #[cfg_attr(not(feature = "clipboard"), allow(dead_code))]
    cut: Option<String>,
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

/// A press that began on the field, and what the pointer's movements do to the selection.
struct Drag {
    by: By,
    /// Where the pointer is, as a physical x in the window.
    at: f64,
    /// The next step of scrolling, while the pointer is past an end of the field.
    scroll: Option<Instant>,
}

/// What a drag in the field selects by.
#[derive(Clone, Copy)]
enum By {
    Chars,
    /// Words, from the span a double click selected.
    Words(usize, usize),
    /// A triple click selected everything: nothing to extend.
    All,
}

/// The query laid out in one line, and the x of each place the caret can be at (a byte offset
/// on a character boundary, with the x in the text's own coordinates). Where the text runs both
/// ways, those are not in order of x.
struct Text {
    content: BlockBox,
    /// The extent of each cluster of the query, from the measured line: it keeps the white space
    /// at the end that the painted one leaves out, and leaves the zero width space out.
    hits: Vec<PHit>,
    /// The right edge of the text as it shows: the greatest x of any of its clusters (the text
    /// starts at 0, whichever way it runs).
    width: f32,
    height: f32,
    xs: Vec<(usize, f32)>,
    /// The byte offsets the clusters start at, in the order of the query.
    clusters: Vec<u32>,
    /// The length of the query: where the last cluster ends.
    len: usize,
}

impl Text {
    /// The x of the caret at byte offset `at`.
    fn x(&self, at: usize) -> f32 {
        let i = self.xs.partition_point(|p| p.0 < at);
        self.xs.get(i).map_or(self.width, |p| p.1)
    }

    /// The caret position nearest to `x`.
    fn nearest(&self, x: f32) -> usize {
        let dist = |p: &&(usize, f32)| (p.1 - x).abs();
        let best = self.xs.iter().min_by(|a, b| dist(a).total_cmp(&dist(b)));
        best.map_or(0, |p| p.0)
    }

    /// The offset of the character `x` is over: the nearest place if it is over none.
    fn over(&self, x: f32) -> usize {
        let hits = self.hits.iter();
        let hit = hits.filter(|h| (h.x0.min(h.x1)..h.x0.max(h.x1)).contains(&x));
        hit.map(|h| h.doc as usize)
            .next()
            .unwrap_or_else(|| self.nearest(x))
    }

    /// Call `rect(x0, x1)` for each stretch of text between the byte offsets `lo` and `hi`: the
    /// clusters they touch, as many stretches as the text runs both ways make of them.
    fn selection(&self, lo: usize, hi: usize, mut rect: impl FnMut(f32, f32)) {
        let touched = |doc: u32| {
            let i = self
                .clusters
                .partition_point(|&c| c <= doc)
                .checked_sub(1)?;
            let start = self.clusters[i] as usize;
            let end = self.clusters.get(i + 1).map_or(self.len, |&c| c as usize);
            (start < hi && end > lo).then_some(())
        };
        layout::runs(&self.hits, touched, |x0, x1, ()| rect(x0, x1));
    }
}

/// The x of every place the caret can be at in `text`, from the clusters' extents: a place inside
/// a cluster (a ligature of two characters) is a share of it.
fn caret_xs(text: &str, hits: &[PHit]) -> Vec<(usize, f32)> {
    let mut clusters: Vec<&PHit> = hits.iter().collect();
    clusters.sort_by_key(|h| h.doc);
    let places = text
        .char_indices()
        .map(|c| c.0)
        .chain(std::iter::once(text.len()));
    places
        .map(|at| {
            let i = clusters.partition_point(|h| h.doc as usize <= at);
            let Some(&h) = i.checked_sub(1).and_then(|i| clusters.get(i)) else {
                return (at, 0.0);
            };
            let start = h.doc as usize;
            let end = clusters.get(i).map_or(text.len(), |n| n.doc as usize);
            let size = text[start..end].chars().count().max(1) as f32;
            let share = text[start..at].chars().count() as f32 / size;
            let x = if h.rtl() {
                h.x1 - (h.x1 - h.x0) * share
            } else {
                h.x0 + (h.x1 - h.x0) * share
            };
            (at, x)
        })
        .collect()
}

impl View {
    /// The bar is open.
    pub fn find_open(&self) -> bool {
        self.find.open
    }

    /// The query, as typed.
    pub fn find_query(&self) -> &str {
        self.find.field.text()
    }

    /// The bar is open and its field has the keyboard.
    pub fn find_focused(&self) -> bool {
        self.find.open && self.find.focused
    }

    /// The selected part of the query, and the caret's byte offset in it.
    pub fn find_selected(&self) -> &str {
        self.find.field.selection()
    }

    pub fn find_caret(&self) -> usize {
        self.find.field.caret()
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
            _ if self.find.field.text().is_empty() => String::new(),
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

    /// Show or hide the bar. Opening takes a short selection for the query, which shows selected
    /// in the field, and goes to the first match from the reading position; closing clears the
    /// matches but keeps the query. The document area gives up or reclaims the bar's height,
    /// keeping the reading position. Ctrl+F and the toolbar's button open the bar when it is
    /// closed and close it when it is open, whoever has the keyboard.
    pub fn set_find(&mut self, open: bool) {
        if std::mem::replace(&mut self.find.open, open) == open {
            return;
        }
        self.find.pointer = None;
        self.find.hover = None;
        self.find.pressed = None;
        self.find.hover_since = None;
        self.find.tooltip = false;
        self.find.drag = None;
        self.find.clicks.reset();
        self.find.rects = None;
        self.find.focused = open;
        self.find.epoch = self.find.epoch.wrapping_add(1);
        self.find.field.select_all();
        self.find.scroll = 0.0;
        self.set_top_inset(self.chrome_inset());
        if open {
            // Opening takes the toolbar's menu away (the bar would sit under it); closing leaves
            // it, one Escape being one layer.
            self.close_menu();
            self.find_take_selection();
            self.find_refresh(None, true);
            self.find_fit();
        } else {
            self.find.found = Found::default();
            self.find.current = None;
        }
    }

    /// Escape closes the bar, after the toolbar's menu above it. Returns true if it did.
    pub(super) fn find_escape(&mut self) -> bool {
        let was = self.find.open;
        self.set_find(false);
        was
    }

    /// The field has the keyboard (a click on it).
    fn find_focus(&mut self) {
        if !std::mem::replace(&mut self.find.focused, true) {
            self.find.epoch = self.find.epoch.wrapping_add(1);
        }
    }

    /// A click on the document takes the keyboard from the field: the bar stays open, with its
    /// matches, and the keys are the document's again. Returns true if the field had the keyboard.
    pub(super) fn find_blur(&mut self) -> bool {
        let had = self.find.open && std::mem::replace(&mut self.find.focused, false);
        if had {
            self.find.epoch = self.find.epoch.wrapping_add(1);
            self.find.drag = None;
            self.find.clicks.reset();
        }
        had
    }

    /// What a paste asked for now must still be true of when its text comes: the bar not closed
    /// or the field not left since.
    pub fn find_ticket(&self) -> u32 {
        self.find.epoch
    }

    /// The selected text, if it is one line of at most [`find::MAX_QUERY`] characters, becomes the
    /// query, selected. Returns true if the query changed.
    #[cfg(feature = "clipboard")]
    fn find_take_selection(&mut self) -> bool {
        let Some(Content::Text(text)) = self.copy_content() else {
            return false;
        };
        let text = text.trim_end_matches(find::is_line_break);
        if text.is_empty()
            || text.chars().count() > find::MAX_QUERY
            || text.contains(find::is_line_break)
        {
            return false;
        }
        let changed = self.find.field.text() != text;
        self.find.field.set(text);
        changed
    }

    #[cfg(not(feature = "clipboard"))]
    fn find_take_selection(&mut self) -> bool {
        false
    }

    // ---- Typing ----------------------------------------------------------------------------

    /// A key press while the bar is open: Enter (while the field has the keyboard), F3 and Ctrl+G
    /// step through the matches, and Shift with them goes the other way. With the keyboard the
    /// field takes the keys of a text field: text is typed (it is `text` of the key event, so
    /// layouts and dead keys work), the arrows, Home and End move the caret and with Shift
    /// select, Ctrl makes the arrows move by words and Backspace and Delete delete them, Ctrl+A
    /// selects all, and with the clipboard Ctrl+C, Ctrl+X and Ctrl+V (or Ctrl+Insert, Shift+Insert)
    /// copy, cut and paste. The keys that are no text or editing (Up, Down, the page keys) and
    /// the other shortcuts with Ctrl or Cmd (Ctrl+Home and Ctrl+End too) go on to the document,
    /// which is what `None` says; Escape does as well, for [`View::apply`] to close the bar first,
    /// and so does Ctrl+F, which closes it. Without the keyboard the field takes only the steps,
    /// and everything else is the document's.
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
        let (shift, alt) = (mods.shift_key(), mods.alt_key());
        let steps = match key {
            Key::Named(NamedKey::F3) => true,
            Key::Named(NamedKey::Enter) => self.find.focused,
            Key::Character(c) => command && matches!(c.as_str(), "g" | "G"),
            _ => false,
        };
        if steps {
            self.find_step(if shift { -1 } else { 1 }, now);
            return Some(Outcome::Redraw);
        }
        if !self.find.focused {
            return None;
        }
        let outcome = match key {
            Key::Named(named) if !alt => self.find_named_key(*named, command, shift),
            Key::Character(c) if command && !alt => self.find_command_key(c.as_str()),
            _ => None,
        };
        if outcome.is_some() || command || alt {
            return outcome;
        }
        let typed = find::typed_query(text?);
        if typed.is_empty() {
            return None;
        }
        self.find_type(&typed);
        Some(Outcome::Redraw)
    }

    /// A named key that is the field's: a motion, a deletion, or a copy, cut or paste key.
    fn find_named_key(&mut self, key: NamedKey, command: bool, shift: bool) -> Option<Outcome> {
        let word = |left, right, command| if command { left } else { right };
        Some(match key {
            NamedKey::ArrowLeft => {
                self.find_go(word(Motion::WordLeft, Motion::Left, command), shift)
            }
            NamedKey::ArrowRight => {
                self.find_go(word(Motion::WordRight, Motion::Right, command), shift)
            }
            NamedKey::Home if !command => self.find_go(Motion::Home, shift),
            NamedKey::End if !command => self.find_go(Motion::End, shift),
            NamedKey::Backspace => self.find_edit(|f| f.backspace(command)),
            NamedKey::Delete => self.find_edit(|f| f.delete(command)),
            #[cfg(feature = "clipboard")]
            NamedKey::Insert if command => self.find_copy(),
            #[cfg(feature = "clipboard")]
            NamedKey::Insert if shift => Outcome::Paste(Which::Clipboard),
            #[cfg(feature = "clipboard")]
            NamedKey::Copy => self.find_copy(),
            #[cfg(feature = "clipboard")]
            NamedKey::Cut => self.find_cut(),
            #[cfg(feature = "clipboard")]
            NamedKey::Paste => Outcome::Paste(Which::Clipboard),
            _ => return None,
        })
    }

    /// A letter with Ctrl that is the field's: select all, and with the clipboard copy, cut and
    /// paste.
    fn find_command_key(&mut self, key: &str) -> Option<Outcome> {
        Some(match key {
            "a" | "A" => {
                self.find.field.select_all();
                self.find_fit();
                selected()
            }
            #[cfg(feature = "clipboard")]
            "c" | "C" => self.find_copy(),
            #[cfg(feature = "clipboard")]
            "x" | "X" => self.find_cut(),
            #[cfg(feature = "clipboard")]
            "v" | "V" => Outcome::Paste(Which::Clipboard),
            _ => return None,
        })
    }

    /// Move the caret, extending the selection with `extend`. What is selected is offered as the
    /// primary selection.
    fn find_go(&mut self, motion: Motion, extend: bool) -> Outcome {
        self.find.field.go(motion, extend);
        self.find_fit();
        if extend { selected() } else { Outcome::Redraw }
    }

    /// Edit the query with `edit`, which says whether it changed the text; the matches are found
    /// again if it did, and the caret is kept in view.
    fn find_edit(&mut self, edit: impl FnOnce(&mut Field) -> bool) -> Outcome {
        let whole = self.find.field.all_selected();
        if edit(&mut self.find.field) {
            self.find_edited(whole);
        }
        self.find_fit();
        Outcome::Redraw
    }

    /// Ctrl+C in the field: the selected text goes to the clipboard, if there is any.
    #[cfg(feature = "clipboard")]
    fn find_copy(&self) -> Outcome {
        if self.find.field.has_selection() {
            Outcome::Copy
        } else {
            Outcome::Nothing
        }
    }

    /// Ctrl+X: the selected text is taken out of the query, and waits in [`View::take_cut`] to
    /// be offered as the clipboard.
    #[cfg(feature = "clipboard")]
    fn find_cut(&mut self) -> Outcome {
        let whole = self.find.field.all_selected();
        let Some(text) = self.find.field.cut() else {
            return Outcome::Nothing;
        };
        self.find.cut = Some(text);
        self.find_edited(whole);
        self.find_fit();
        Outcome::Cut
    }

    /// The text of the last cut, for the clipboard.
    #[cfg(feature = "clipboard")]
    pub fn take_cut(&mut self) -> Option<Content> {
        self.find.cut.take().map(|text| Content::Text(text.into()))
    }

    /// What a copy offers while the field has the keyboard: the text selected in it.
    #[cfg(feature = "clipboard")]
    pub(super) fn find_content(&self) -> Option<Content> {
        let field = &self.find.field;
        field
            .has_selection()
            .then(|| Content::Text(field.selection().into()))
    }

    /// Type `text` (a key's, or what an input method committed) into the query at the caret, over
    /// what is selected, up to [`find::MAX_QUERY`] characters in all. It goes to the field only while
    /// the field has the keyboard.
    pub fn find_type(&mut self, text: &str) {
        let text = find::typed_query(text);
        if !self.find_focused() || text.is_empty() {
            return;
        }
        self.find_edit(|f| f.insert(&text));
    }

    /// Text pasted from a selection (asked for with [`View::find_ticket`]) goes into the query
    /// like typed text, but for what a paste may hold: the first line only, tabs as spaces, no
    /// other control characters. Returns true if the bar needs a repaint. It is dropped if the bar
    /// was closed or the field left since the ticket was taken.
    pub fn find_paste(&mut self, ticket: u32, text: &str) -> bool {
        if !self.find_focused() || ticket != self.find.epoch {
            return false;
        }
        let text = find::pasted_query(text);
        if text.is_empty() {
            return false;
        }
        self.find_edit(|f| f.insert(&text));
        true
    }

    /// The query changed: look again from where the current match was, so typing on keeps it, as
    /// long as it is what the reader is looking at. A new query (`replaced`: the whole of the old
    /// one was selected), or a current match that has been scrolled out of view, is looked for from
    /// the reading position.
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
        let found = find::search_from(&self.doc, self.find.field.text(), Focus::At(block, off));
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
            let found = find::search_from(&self.doc, self.find.field.text(), Focus::Index(next));
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
    /// click. A press on the field takes the keyboard for it and puts the caret there (Shift
    /// extends the selection to it), a double click selects a word and a triple click everything,
    /// and the pointer's movements with the button down select on. Anywhere else on the bar it
    /// does nothing (the field keeps the keyboard it has).
    pub(super) fn press_find(&mut self, x: f64, y: f64, now: Instant) -> bool {
        self.find.pressed = self.find_spot_at(x, y);
        self.find.tooltip = false;
        self.find.hover_since = None;
        self.find.drag = None;
        let mut on_field = false;
        if self.find.pressed == Some(Spot::Field) {
            let (field, _) = self.find_geometry();
            on_field = field.contains(x, y);
            if on_field {
                self.find_press_field(x, now);
            }
        }
        if !on_field {
            self.find_clicks_end();
        }
        self.find.pressed.is_some()
    }

    /// A press that was not the bar's: it ends a double click on the field.
    pub(super) fn find_clicks_end(&mut self) {
        self.find.clicks.reset();
    }

    fn find_press_field(&mut self, x: f64, now: Instant) {
        self.find_focus();
        let slop = CLICK_SLOP * self.hidpi;
        let count = self.find.clicks.count(now, (x, 0.0), slop);
        let text = self.find_text();
        let tx = self.find_text_x(x);
        let by = match count {
            1 => {
                self.find.field.place(text.nearest(tx), self.find.shift);
                By::Chars
            }
            2 => {
                let (lo, hi) = self.find.field.word_at(text.over(tx));
                self.find.field.select(lo, hi);
                By::Words(lo, hi)
            }
            _ => {
                self.find.field.select_all();
                By::All
            }
        };
        self.find.drag = Some(Drag {
            by,
            at: x,
            scroll: None,
        });
        self.find_fit();
    }

    /// The x in the text's own coordinates of window x `x`, which is clamped to the room the text
    /// has in the field.
    fn find_text_x(&self, x: f64) -> f32 {
        let Some((_, inner)) = self.find.rects else {
            return 0.0;
        };
        let x = (x as f32).clamp(inner.x0, inner.x1);
        x - inner.x0 + self.find.scroll
    }

    /// A selection is being dragged in the field.
    pub fn find_dragging(&self) -> bool {
        self.find.drag.is_some()
    }

    /// The pointer moved to window x `x` with the button down on the field: the selection follows
    /// it, and past an end of the field the text scrolls on its own (see `View::find_expire`).
    /// Returns true if the bar needs a repaint.
    pub fn find_drag_to(&mut self, x: f64, now: Instant) -> bool {
        let Some(drag) = self.find.drag.as_mut() else {
            return false;
        };
        drag.at = x;
        let (_, inner) = self.find_geometry();
        let outside = x < f64::from(inner.x0) || x > f64::from(inner.x1);
        if let Some(drag) = self.find.drag.as_mut() {
            drag.scroll = if outside {
                drag.scroll.or(Some(now))
            } else {
                None
            };
        }
        self.find_drag_select()
    }

    /// Put the end of the selection under the pointer again. Returns true if it moved.
    fn find_drag_select(&mut self) -> bool {
        let Some(&Drag { by, at, .. }) = self.find.drag.as_ref() else {
            return false;
        };
        let before = (self.find.field.range(), self.find.field.caret());
        let text = self.find_text();
        let tx = self.find_text_x(at);
        let field = &mut self.find.field;
        match by {
            By::Chars => field.place(text.nearest(tx), true),
            By::Words(lo, hi) => {
                let (from, to) = field.word_at(text.over(tx));
                if from < lo {
                    field.select(hi, from);
                } else {
                    field.select(lo, to);
                }
            }
            By::All => {}
        }
        (field.range(), field.caret()) != before
    }

    /// The button came up after a press on the field. Returns true if what it selected is worth
    /// offering as the primary selection.
    pub fn release_find_drag(&mut self) -> bool {
        self.find.drag.take().is_some() && self.find.field.has_selection()
    }

    /// Shift is held down (a click then extends the selection).
    pub(super) fn find_shift(&mut self, shift: bool) {
        self.find.shift = shift;
    }

    /// A middle click at window point (`x`, `y`): on the field it puts the caret there and takes
    /// the keyboard for it, and the ticket for the paste of the primary selection that follows
    /// is returned.
    #[cfg(feature = "clipboard")]
    pub fn find_middle(&mut self, x: f64, y: f64) -> Option<u32> {
        if !self.find.open {
            return None;
        }
        let (field, _) = self.find_geometry();
        if !field.contains(x, y) || self.find_menu_covers(x, y) {
            return None;
        }
        self.find_focus();
        let text = self.find_text();
        self.find
            .field
            .place(text.nearest(self.find_text_x(x)), false);
        self.find_fit();
        Some(self.find.epoch)
    }

    /// The point is on the field, where the pointer is a text cursor: not under the toolbar's
    /// dropdown, which hangs over the bar. It goes by where the field was last laid out.
    pub fn find_over_field(&self, x: f64, y: f64) -> bool {
        self.find.open
            && self
                .find
                .rects
                .is_some_and(|(field, _)| field.contains(x, y))
            && !self.find_menu_covers(x, y)
    }

    fn find_menu_covers(&self, x: f64, y: f64) -> bool {
        self.toolbar.menu_covers(&self.toolbar_ctx(), x, y)
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
    fn tooltip_deadline(&self) -> Option<Instant> {
        if self.find.tooltip {
            return None;
        }
        self.find.hover_since.map(|t| t + toolbar::TOOLTIP_DELAY)
    }

    /// When the loop must wake for the bar: for a tooltip, or to scroll the text for a drag that
    /// is past an end of the field.
    pub(super) fn find_deadline(&self) -> Option<Instant> {
        let scroll = self.find.drag.as_ref().and_then(|d| d.scroll);
        let scroll = scroll.map(|t| t + SCROLL_TICK);
        [self.tooltip_deadline(), scroll]
            .into_iter()
            .flatten()
            .min()
    }

    /// Show the tooltip if its time has come, and scroll a step if one is due. Returns true if
    /// the window needs a repaint.
    pub(super) fn find_expire(&mut self, now: Instant) -> bool {
        let tip = self.tooltip_deadline().is_some_and(|t| now >= t);
        if tip {
            self.find.tooltip = true;
        }
        tip | self.find_autoscroll(now)
    }

    /// Scroll the text a step while a drag is past an end of the field, and put the end of the
    /// selection under the pointer again.
    fn find_autoscroll(&mut self, now: Instant) -> bool {
        let Some(&Drag {
            at,
            scroll: Some(last),
            ..
        }) = self.find.drag.as_ref()
        else {
            return false;
        };
        if now < last + SCROLL_TICK {
            return false;
        }
        let (_, inner) = self.find_geometry();
        let past = if at < f64::from(inner.x0) {
            at - f64::from(inner.x0)
        } else {
            (at - f64::from(inner.x1)).max(0.0)
        };
        let moved = if past == 0.0 {
            false
        } else {
            let speed = (past.abs() * SCROLL_PER_PX)
                .clamp(SCROLL_MIN * self.hidpi, SCROLL_MAX * self.hidpi)
                * past.signum();
            let secs = now.saturating_duration_since(last).min(SCROLL_STEP_MAX);
            let before = self.find.scroll;
            let text = self.find_text();
            let to = f64::from(before) + speed * secs.as_secs_f64();
            self.find.scroll = (to as f32).clamp(0.0, self.find_max_scroll(inner, &text));
            self.find.scroll != before
        };
        // At the end of the text there is nothing to wait for until the pointer moves.
        if let Some(drag) = self.find.drag.as_mut() {
            drag.scroll = moved.then_some(now);
        }
        self.find_drag_select() | moved
    }

    /// A new text (a reload, another document): the query stays, and the matches are those of the
    /// new text, the current one the first from the reading position (nothing scrolls).
    pub(super) fn find_again(&mut self) {
        self.find.pointer = None;
        self.find.hover = None;
        self.find.pressed = None;
        self.find.hover_since = None;
        self.find.tooltip = false;
        self.find.drag = None;
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

    /// The field and the room inside it as painted, which depends on how wide the count is, and
    /// is kept for the pointer's sake.
    fn find_geometry(&mut self) -> (Rect, Rect) {
        let count = self.find_count();
        let width = (!count.is_empty()).then(|| self.find_line(&count, Role::Muted).1);
        self.find_laid(width)
    }

    /// The field and its room when the count is `width` wide, kept.
    fn find_laid(&mut self, width: Option<f32>) -> (Rect, Rect) {
        let rects = self.find_field(self.find_count_x(width));
        self.find.rects = Some(rects);
        rects
    }

    /// The query laid out in one line. The layout leaves white space at the end of a line out, so
    /// where the caret can be is measured on the query with a zero width space after it, which
    /// stays and neither kerns, joins nor takes part in bidi runs with what is before it.
    fn find_text(&mut self) -> Text {
        let query = self.find.field.text().to_owned();
        let probe = format!("{query}\u{200B}");
        let (measured, room, _) = self.find_line(&probe, Role::Text);
        // The query as painted is as wide as the measured line, so that a line that runs right to
        // left has the same right edge in both (what hangs at its end is left of it).
        let (content, _, height) = self.find_line_in(&query, Role::Text, room);
        let mut xs = caret_xs(&probe, &measured.hits);
        xs.pop();
        let width = measured
            .hits
            .iter()
            .map(|h| h.x0.max(h.x1))
            .fold(0.0, f32::max);
        let hits: Vec<PHit> = (measured.hits.iter().copied())
            .filter(|h| (h.doc as usize) < query.len())
            .collect();
        let mut clusters: Vec<u32> = hits.iter().map(|h| h.doc).collect();
        clusters.sort_unstable();
        clusters.dedup();
        Text {
            content,
            hits,
            len: query.len(),
            width,
            height,
            xs,
            clusters,
        }
    }

    /// How far the text may be scrolled: until its end, with room for the caret, is at the field's.
    fn find_max_scroll(&self, inner: Rect, text: &Text) -> f32 {
        (text.width + self.find_caret_width() - (inner.x1 - inner.x0)).max(0.0)
    }

    fn find_caret_width(&self) -> f32 {
        (self.hidpi as f32).round().max(1.0)
    }

    /// Scroll the text so the caret is in the field, and note where it is for an input method.
    fn find_fit(&mut self) {
        let (_, inner) = self.find_geometry();
        let text = self.find_text();
        self.find_fit_to(inner, &text);
    }

    fn find_fit_to(&mut self, inner: Rect, text: &Text) {
        let (room, bar) = (inner.x1 - inner.x0, self.find_caret_width());
        // The caret is a bar that starts at the place it is at.
        let place = text.x(self.find.field.caret());
        let mut scroll = self.find.scroll;
        if place + bar - scroll > room {
            scroll = place + bar - room;
        }
        if place < scroll {
            scroll = place;
        }
        self.find.scroll = scroll.clamp(0.0, self.find_max_scroll(inner, text));
        self.find.caret = inner.x0 - self.find.scroll + place;
    }

    /// One line of the bar's text in `style`, laid out (the characters no face has are noted for
    /// the fallback scan).
    fn find_line(&mut self, text: &str, role: Role) -> (BlockBox, f32, f32) {
        self.find_line_in(text, role, 0.0)
    }

    /// `find_line`, where a line that runs right to left has at least `room` to be laid out in.
    fn find_line_in(&mut self, text: &str, role: Role, room: f32) -> (BlockBox, f32, f32) {
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
        let mut pl = self
            .msg_ctx
            .layout(&self.fonts, &para, 1.0e6, Align::Start, 1.4);
        // A line that runs right to left is aligned to the right of all that room: laid out
        // again, as wide as it is, it starts at the left like the rest.
        if pl.hits.iter().any(PHit::rtl) {
            let fit = (pl.width + 0.5).max(room + 0.5);
            pl = self
                .msg_ctx
                .layout(&self.fonts, &para, fit, Align::Start, 1.4);
        }
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
        // The field: the query, scrolled to keep the caret in it, with what is selected washed
        // and the caret a bar.
        let (field, inner) = self.find_laid(counted);
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
        let text = self.find_text();
        self.find_fit_to(inner, &text);
        let (x, y) = (
            inner.x0 - self.find.scroll,
            field.y0 + (field.y1 - field.y0 - text.height) / 2.0,
        );
        if self.find.field.text().is_empty() {
            let (content, _, h) = self.find_line("Find in page", Role::Muted);
            let y = field.y0 + (field.y1 - field.y0 - h) / 2.0;
            self.paint_text(frame, &content, inner.x0, y, theme);
        }
        if self.find.field.has_selection() {
            let (lo, hi) = self.find.field.range();
            let alpha = if self.find.focused { 255 } else { DIM };
            let (top, bottom) = (y.round() as i32, (y + text.height).round() as i32);
            text.selection(lo, hi, |x0, x1| {
                let wash = (
                    (x + x0).round() as i32,
                    top,
                    (x + x1).round() as i32,
                    bottom,
                );
                let color = theme.color(Role::Selection);
                frame.rounded_rect(wash, (2.0 * s).round(), None, color, alpha);
            });
        }
        self.paint_text(frame, &text.content, x, y, theme);
        if self.find.focused {
            let cx = self.find.caret.round() as i32;
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

/// What a selection made in the field asks of the shell: that it be offered as the primary
/// selection, which needs the clipboard.
fn selected() -> Outcome {
    #[cfg(feature = "clipboard")]
    return Outcome::Selected;
    #[cfg(not(feature = "clipboard"))]
    Outcome::Redraw
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
