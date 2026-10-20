//! The table of contents panel: the document's headings in a column at the window's left, from
//! below the toolbar to the bottom, like the outline of GitHub's or VS Code's.
//!
//! In a window wide enough the panel pushes the document aside: the document is laid out in what
//! is left of the width and painted from the panel's right edge, so the reading position is kept
//! as it is on any resize. In a narrower one it covers the document's left edge instead, with no
//! relayout, and a click on a heading or on the document closes it. Either way nothing of the
//! document is hit through it (see [`View::in_area`]).
//!
//! The panel is chrome: its sizes are logical pixels that scale with the display like the
//! toolbar's, not with the document's zoom. What it keeps is small: whether it is open, its own
//! scroll, the list of the document's headings (found when the panel first needs it, so a closed
//! panel costs nothing), and the lines of text of the rows that were on screen, laid out once.
//! Which heading is the current one is looked up by binary search on every frame it is painted.

use std::collections::HashMap;
use std::ops::Range;

use super::View;
use crate::doc::Doc;
use crate::layout::BlockBox;
use crate::render::{self, Clip, Frame, Viewport};
use crate::text::TextStyle;
use crate::theme::{Role, Theme};

/// The panel's width, and the room the document keeps for itself when the panel pushes it
/// aside, in logical pixels.
const WIDTH: f32 = 260.0;
const MIN_DOC: f32 = 480.0;
/// A row's height, the text's size, the indent per level, and the padding: at the sides of a
/// row's text (more at the right, for the scrollbar) and above the first row and below the last.
const ROW: f32 = 26.0;
const TEXT: f32 = 13.0;
const INDENT: f32 = 12.0;
const PAD_X: f32 = 12.0;
const PAD_RIGHT: f32 = 14.0;
const PAD_Y: f32 = 6.0;
/// The margin of the hover tint and the current row's mark from the panel's edges, and the mark's
/// width.
const MARGIN: f32 = 4.0;
const MARK: f32 = 3.0;
/// How many rows keep their laid-out text: more than any window shows, so scrolling back and
/// forth lays nothing out again, and a long document can't make the cache grow with it.
const ROWS_KEPT: usize = 512;
/// What the panel says of a document without headings.
const EMPTY: &str = "No headings";

/// The panel's width in a window `window` pixels wide (at most the window's own), at display
/// scale `hidpi`.
pub(super) fn width(window: u32, hidpi: f64) -> u32 {
    ((f64::from(WIDTH) * hidpi).round() as u32).min(window)
}

/// Whether the panel pushes the document aside in a window `window` pixels wide: it does when at
/// least [`MIN_DOC`] is left for the document.
pub(super) fn pushes(window: u32, hidpi: f64) -> bool {
    let panel = (f64::from(WIDTH) * hidpi).round();
    f64::from(window) >= panel + (f64::from(MIN_DOC) * hidpi).round()
}

/// A heading of the main flow, as the panel lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    /// The heading's block (its first chunk, for a heading cut into several).
    pub block: u32,
    pub level: u8,
    /// Its index in [`Doc::headings`], where its slug is.
    pub heading: u32,
}

/// The document's headings of the main flow, in order, and the shallowest level among them (the
/// level the rows are indented from).
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct List {
    pub entries: Vec<Entry>,
    pub shallowest: u8,
}

impl List {
    /// The headings of `doc`, without those inside footnote definitions, which are appended to
    /// [`Doc::headings`] after the others and listed in the footnotes' own order.
    pub fn of(doc: &Doc) -> List {
        let entries: Vec<Entry> = doc
            .headings
            .iter()
            .take(doc.main_headings)
            .enumerate()
            .map(|(i, h)| Entry {
                block: h.block,
                level: h.level,
                heading: i as u32,
            })
            .collect();
        let shallowest = entries.iter().map(|e| e.level).min().unwrap_or(1);
        List {
            entries,
            shallowest,
        }
    }

    /// How many logical pixels row `i` is indented.
    pub fn indent(&self, i: usize) -> f32 {
        let level = self.entries.get(i).map_or(self.shallowest, |e| e.level);
        f32::from(level.saturating_sub(self.shallowest)) * INDENT
    }
}

/// How many characters of a heading's text the panel takes: far more than a row ever shows (it
/// holds about a hundred, however it is scaled), so a longer heading is still cut with an
/// ellipsis, and one of megabytes costs no more than this to read.
const HEADING_CHARS: usize = 256;

/// The plain text of the heading that starts at `block`, up to [`HEADING_CHARS`] characters: its
/// spans' text on one line, with the white space (a break in a heading is one, and so is the
/// cut between the chunks of a heading too long for one block) folded to single spaces.
pub(super) fn heading_text(doc: &Doc, block: u32) -> String {
    let mut text = String::new();
    let (mut chars, mut gap) = (0, false);
    let chunks = doc.blocks[block as usize..]
        .iter()
        .enumerate()
        .take_while(|&(i, b)| i == 0 || b.joined)
        .map(|(_, b)| b);
    for span in chunks.flat_map(|b| doc.block_spans(b)) {
        for c in doc.span_text(span).chars() {
            if c.is_whitespace() {
                gap = !text.is_empty();
            } else if chars < HEADING_CHARS {
                if std::mem::take(&mut gap) {
                    text.push(' ');
                    chars += 1;
                }
                text.push(c);
                chars += 1;
            } else {
                return text;
            }
        }
    }
    text
}

/// The index of the last of `n` entries whose top (`top(i)`, ascending) is at or above `at`, if
/// any: the heading the reading position is in the section of.
pub(super) fn current(n: usize, top: impl Fn(usize) -> f64, at: f64) -> Option<usize> {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if top(mid) <= at {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo.checked_sub(1)
}

/// Where the panel is in the window and what that makes of its rows (physical pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Geom {
    /// The panel's right edge, which is its width.
    pub width: f64,
    /// Its top and bottom: below the toolbar, down to the window's end.
    pub y0: f64,
    pub y1: f64,
    /// The display's scale.
    pub scale: f64,
}

impl Geom {
    /// A row's height.
    pub fn row(&self) -> f64 {
        (f64::from(ROW) * self.scale).round().max(1.0)
    }

    fn pad(&self) -> f64 {
        (f64::from(PAD_Y) * self.scale).round()
    }

    /// The height the panel shows rows in.
    pub fn height(&self) -> f64 {
        (self.y1 - self.y0).max(0.0)
    }

    /// The height of `rows` rows with their padding.
    pub fn content(&self, rows: usize) -> f64 {
        2.0 * self.pad() + rows as f64 * self.row()
    }

    /// The most the panel scrolls: its content less what it shows.
    pub fn max_scroll(&self, rows: usize) -> f64 {
        (self.content(rows) - self.height()).max(0.0)
    }

    /// Window y of the top of row `i`, with the panel scrolled by `scroll`.
    pub fn top(&self, i: usize, scroll: f64) -> f64 {
        self.y0 + self.pad() + i as f64 * self.row() - scroll
    }

    /// The row at window `y` among `rows`, if there is one (not the padding, nor below the
    /// last).
    pub fn row_at(&self, y: f64, scroll: f64, rows: usize) -> Option<usize> {
        if !(self.y0..self.y1).contains(&y) {
            return None;
        }
        let at = y - self.y0 - self.pad() + scroll;
        if at < 0.0 {
            return None;
        }
        let i = (at / self.row()) as usize;
        (i < rows).then_some(i)
    }

    /// The rows that show at all, partly or wholly.
    pub fn visible(&self, scroll: f64, rows: usize) -> Range<usize> {
        let first = ((scroll - self.pad()) / self.row()).floor().max(0.0) as usize;
        let last = ((scroll + self.height() - self.pad()) / self.row())
            .ceil()
            .max(0.0) as usize;
        first.min(rows)..last.min(rows)
    }

    /// The scroll nearest `scroll` that shows row `i` of `rows` whole: the same if it shows
    /// already. The first row takes the padding above it along, the last the padding below.
    pub fn show(&self, scroll: f64, i: usize, rows: usize) -> f64 {
        let top = if i == 0 {
            0.0
        } else {
            self.pad() + i as f64 * self.row()
        };
        let bottom = if i + 1 >= rows {
            self.content(rows)
        } else {
            self.pad() + (i + 1) as f64 * self.row()
        };
        let wanted = if top < scroll {
            top
        } else if bottom > scroll + self.height() {
            bottom - self.height()
        } else {
            scroll
        };
        wanted.clamp(0.0, self.max_scroll(rows))
    }
}

/// A row's text, laid out for the width it was laid out at.
struct Row {
    content: BlockBox,
    /// The text's height, for centering it in the row.
    height: f32,
}

/// The panel's state, apart from where it is (which comes from the window).
#[derive(Default)]
pub(super) struct Panel {
    /// Whether the panel is wanted: it shows only with a document.
    pub open: bool,
    /// The panel's own scroll, in physical pixels.
    pub scroll: f64,
    /// The headings, found when first needed for this document.
    pub list: Option<List>,
    /// The text of the rows shown lately, by entry, and what it was laid out for: the panel's
    /// width and the display's scale.
    rows: HashMap<u32, Row>,
    rows_for: (u32, u32),
    /// Where the pointer is in the window, while it can reach the panel.
    pub pointer: Option<(f64, f64)>,
    /// The row under it.
    pub hover: Option<usize>,
    /// The row the mouse button went down on.
    pub pressed: Option<usize>,
    /// The current heading as of the last time the panel followed it (`None` before it did): the
    /// panel scrolls to the current heading when this changes, and not otherwise, so a panel the
    /// reader scrolled is left where it is.
    pub seen: Option<Option<usize>>,
}

impl Panel {
    /// The laid-out text is out of date (another font face was found): lay it out again.
    pub fn forget_rows(&mut self) {
        self.rows.clear();
    }
}

impl View {
    /// The panel is open (the `toc` setting, `C`), whether or not there is a document to list.
    pub fn toc_open(&self) -> bool {
        self.toc.open
    }

    /// Open or close the panel (the `toc` setting). A document that is pushed aside is laid out
    /// again in the room left, keeping the reading position.
    pub fn set_toc(&mut self, open: bool) {
        if std::mem::replace(&mut self.toc.open, open) == open {
            return;
        }
        self.toc.pressed = None;
        self.toc.seen = None;
        self.toc_look();
        self.relayout();
    }

    /// The panel shows: it is open and there is a document, not a message in its place.
    pub(super) fn toc_showing(&self) -> bool {
        self.toc.open && self.message.is_none()
    }

    /// The panel's width in physical pixels: 0 while it doesn't show.
    pub(super) fn toc_width(&self) -> u32 {
        if self.toc_showing() {
            width(self.width, self.hidpi)
        } else {
            0
        }
    }

    /// The panel shows and pushes the document aside, rather than covering its edge.
    pub(super) fn toc_pushes(&self) -> bool {
        self.toc_showing() && pushes(self.width, self.hidpi)
    }

    pub(super) fn toc_geom(&self) -> Geom {
        Geom {
            width: f64::from(self.toc_width()),
            y0: f64::from(self.top_inset),
            y1: f64::from(self.height),
            scale: self.hidpi,
        }
    }

    /// The point is on the panel, which then takes it.
    pub(super) fn toc_covers(&self, x: f64, y: f64) -> bool {
        self.toc_showing() && (0.0..f64::from(self.toc_width())).contains(&x) && {
            let g = self.toc_geom();
            (g.y0..g.y1).contains(&y)
        }
    }

    /// Find the headings if that is not done for this document yet.
    fn toc_ready(&mut self) {
        if self.toc.list.is_none() {
            self.toc.list = Some(List::of(&self.doc));
        }
    }

    pub(super) fn toc_rows(&self) -> usize {
        self.toc.list.as_ref().map_or(0, |l| l.entries.len())
    }

    /// The row at window point (`x`, `y`), if it is on one.
    pub(super) fn toc_row_at(&self, x: f64, y: f64) -> Option<usize> {
        if !self.toc_covers(x, y) {
            return None;
        }
        self.toc_geom().row_at(y, self.toc.scroll, self.toc_rows())
    }

    /// Look at the row under the pointer again. Returns true if the picture changed.
    fn toc_look(&mut self) -> bool {
        if self.toc_showing() {
            self.toc_ready();
        }
        let hover = self.toc.pointer.and_then(|(x, y)| self.toc_row_at(x, y));
        std::mem::replace(&mut self.toc.hover, hover) != hover
    }

    /// The pointer moved to `at`, or left the window (`None`); `free` when nothing else (a drag,
    /// the toolbar's dropdown) has it. Returns true if the window needs a repaint.
    pub(super) fn toc_pointer(&mut self, at: Option<(f64, f64)>, free: bool) -> bool {
        self.toc.pointer = at.filter(|_| free);
        self.toc_look()
    }

    /// The pointer is on a row of the panel, which shows the hand.
    pub fn over_toc(&self) -> bool {
        self.toc.hover.is_some()
    }

    /// A press at (`x`, `y`): true if the panel takes it. On the panel it is the start of a
    /// click on a row (and never of a selection); on the document, with the panel covering it,
    /// it closes the panel and does nothing else.
    pub(super) fn press_toc(&mut self, x: f64, y: f64) -> bool {
        self.toc.pressed = None;
        if !self.toc_showing() {
            return false;
        }
        if self.toc_covers(x, y) {
            self.toc_ready();
            self.toc.pressed = self.toc_row_at(x, y);
            return true;
        }
        if !self.toc_pushes() && self.in_area(x, y) {
            self.set_toc(false);
            return true;
        }
        false
    }

    /// The mouse button came up at (`x`, `y`) after a press on row `row`: the anchor of its
    /// heading if it is still on that row, for the shell to follow as it follows a link of the
    /// document. A panel over the document closes.
    pub(super) fn click_toc(&mut self, row: usize, x: f64, y: f64) -> Option<String> {
        if self.toc_row_at(x, y) != Some(row) {
            return None;
        }
        let entry = *self.toc.list.as_ref()?.entries.get(row)?;
        let href = format!("#{}", self.doc.headings[entry.heading as usize].slug);
        if !self.toc_pushes() {
            self.set_toc(false);
        }
        Some(href)
    }

    /// The wheel or the trackpad scrolled by `dy` pixels: the panel scrolls, and the document
    /// doesn't, when the pointer is over it. Returns true if the panel took it.
    pub(super) fn toc_wheel(&mut self, dy: f64) -> bool {
        let Some((x, y)) = self.toc.pointer else {
            return false;
        };
        if !self.toc_covers(x, y) {
            return false;
        }
        self.toc_ready();
        let max = self.toc_geom().max_scroll(self.toc_rows());
        self.toc.scroll = (self.toc.scroll + dy).clamp(0.0, max);
        self.toc_look();
        true
    }

    /// Wheel notches (`lines`, positive = down) over the panel: it scrolls by rows of its own,
    /// so a notch is the same however far the document is zoomed. Returns true if the panel
    /// took it.
    pub(super) fn toc_wheel_lines(&mut self, lines: f64) -> bool {
        let step = super::WHEEL_LINES * self.toc_geom().row();
        self.toc_wheel(lines * step)
    }

    /// Escape closes the panel when it covers the document. Returns true if it did.
    pub(super) fn toc_escape(&mut self) -> bool {
        if self.toc_showing() && !self.toc_pushes() {
            self.set_toc(false);
            return true;
        }
        false
    }

    /// Another document: its headings are not the old ones, and the panel starts at the top.
    pub(super) fn forget_toc(&mut self) {
        self.toc.list = None;
        self.toc.forget_rows();
        self.toc.scroll = 0.0;
        self.toc.pressed = None;
        self.toc.seen = None;
        self.toc_look();
    }

    /// The same document again, as the file is now (a reload): the panel stays where it is,
    /// within what its new list allows.
    pub(super) fn refresh_toc(&mut self) {
        self.toc.list = None;
        self.toc.forget_rows();
        self.toc.pressed = None;
        self.toc_look();
    }

    /// The heading the reading position is in the section of: the last at or above the top of
    /// the window, a half line down, so a heading just scrolled to counts as the current one.
    /// `None` before the first heading, or before the panel has the list.
    pub(super) fn toc_current(&self) -> Option<usize> {
        let list = self.toc.list.as_ref()?;
        let at = self.scroll() + 0.5 * self.line();
        current(
            list.entries.len(),
            |i| self.layout.top(list.entries[i].block as usize),
            at,
        )
    }

    /// Keep the panel's scroll within its content, and when the current heading is another than
    /// last time, scroll it just enough to show it.
    pub(super) fn toc_sync(&mut self) {
        if !self.toc_showing() {
            return;
        }
        self.toc_ready();
        let (rows, g) = (self.toc_rows(), self.toc_geom());
        self.toc.scroll = self.toc.scroll.clamp(0.0, g.max_scroll(rows));
        let now = self.toc_current();
        if self.toc.seen != Some(now) {
            self.toc.seen = Some(now);
            if let Some(i) = now {
                self.toc.scroll = g.show(self.toc.scroll, i, rows);
            }
        }
    }

    /// Lay out the text of row `i` if that isn't done yet.
    fn toc_layout_row(&mut self, i: usize, g: Geom) {
        let Some(list) = self.toc.list.as_ref() else {
            return;
        };
        let Some(&entry) = list.entries.get(i) else {
            return;
        };
        if self.toc.rows.contains_key(&(i as u32)) {
            return;
        }
        let s = g.scale as f32;
        let room = g.width as f32 - (PAD_X + list.indent(i) + PAD_RIGHT) * s;
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: TEXT * s,
            role: Role::Text,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let text = heading_text(&self.doc, entry.block);
        let (para, pl) = self.fit_line(&text, style, room.max(TEXT * s), false);
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let height = pl.height;
        let content = BlockBox::from_para(pl, &para.styles);
        self.toc.rows.insert(i as u32, Row { content, height });
    }

    /// Paint the panel over the document (and what the document has over it: the scrollbar, the
    /// URL preview), under the toolbar.
    pub(super) fn paint_toc(&mut self, frame: &mut Frame, theme: &Theme) {
        if !self.toc_showing() {
            return;
        }
        self.toc_sync();
        // The pointer may be over another row than it was: the panel scrolled, or the list changed.
        self.toc_look();
        let (g, s) = (self.toc_geom(), self.hidpi as f32);
        let rows = self.toc_rows();
        let (x1, y0, y1) = (g.width as i32, g.y0 as i32, g.y1 as i32);
        let area = Clip { x0: 0, y0, x1, y1 };
        let prev = frame.set_clip(area);
        frame.fill_rect(0, y0, x1, y1, theme.code_bg);
        let line = s.round().max(1.0) as i32;
        frame.fill_rect(x1 - line, y0, x1, y1, theme.border);
        let visible = g.visible(self.toc.scroll, rows);
        // The text laid out for another width or scale, or too much of it, is dropped.
        let key = (g.width as u32, (g.scale * 100.0) as u32);
        if std::mem::replace(&mut self.toc.rows_for, key) != key
            || self.toc.rows.len() + visible.len() > ROWS_KEPT
        {
            self.toc.rows.clear();
        }
        for i in visible.clone() {
            self.toc_layout_row(i, g);
        }
        let current = self.toc_current();
        for i in visible {
            let top = g.top(i, self.toc.scroll) as f32;
            let row = g.row() as f32;
            let tint = (
                (MARGIN * s).round() as i32,
                top.round() as i32,
                x1 - line - (MARGIN * s).round() as i32,
                (top + row).round() as i32,
            );
            if self.toc.hover == Some(i) {
                let alpha = if self.toc.pressed == Some(i) {
                    200
                } else {
                    110
                };
                frame.rounded_rect(tint, (4.0 * s).round(), None, theme.border, alpha);
            }
            let here = current == Some(i);
            if here {
                let (w, inset) = ((MARK * s).round().max(1.0), (5.0 * s).round());
                let x0 = tint.0 as f32;
                let mark = (
                    x0 as i32,
                    (top + inset) as i32,
                    (x0 + w) as i32,
                    (top + row - inset) as i32,
                );
                frame.rounded_rect(mark, w / 2.0, None, theme.link, 255);
            }
            let Some(r) = self.toc.rows.get(&(i as u32)) else {
                continue;
            };
            let indent = self.toc.list.as_ref().map_or(0.0, |l| l.indent(i));
            let x = (PAD_X + indent) * s;
            let y = top + (row - r.height) / 2.0;
            // UI text takes the color of the row whatever role its style names.
            let mut colors = *theme;
            colors.text = if here { theme.link } else { theme.text };
            render::paint_block(
                frame,
                &r.content,
                x,
                y,
                &self.fonts,
                &mut self.cache,
                &colors,
                #[cfg(feature = "clipboard")]
                None,
            );
        }
        if rows == 0 {
            self.paint_toc_empty(frame, g, theme);
        } else {
            let vp = Viewport {
                area,
                scroll: self.toc.scroll,
            };
            render::paint_scrollbar(frame, vp, g.content(rows) as f32, s, theme);
        }
        frame.set_clip(prev);
    }

    /// "No headings", muted, where the first row would be.
    fn paint_toc_empty(&mut self, frame: &mut Frame, g: Geom, theme: &Theme) {
        let s = g.scale as f32;
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: TEXT * s,
            role: Role::Muted,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let room = (g.width as f32 - (PAD_X + PAD_RIGHT) * s).max(TEXT * s);
        let (para, pl) = self.fit_line(EMPTY, style, room, false);
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let height = pl.height;
        let content = BlockBox::from_para(pl, &para.styles);
        let y = g.top(0, 0.0) as f32 + (g.row() as f32 - height) / 2.0;
        render::paint_block(
            frame,
            &content,
            PAD_X * s,
            y,
            &self.fonts,
            &mut self.cache,
            theme,
            #[cfg(feature = "clipboard")]
            None,
        );
    }
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
