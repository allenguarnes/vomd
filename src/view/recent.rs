//! The empty window's list of the files opened lately ([`crate::recent`]), under its invitation
//! to open one: a click on a row, or its number, opens the file; the arrow keys (or `j`/`k`) pick
//! a row for Enter to open and Delete to take off the list; "Clear list" empties it.
//!
//! It is part of the page, not chrome: its sizes are logical pixels times the document's scale,
//! so zoom applies, as it does to the invitation. Rows that don't fit the window are left out
//! from the bottom ("Clear list" before the last one). Where the rows are is known once they are
//! painted, which every change that moves them (a resize, zoom, the toolbar) is followed by; hit
//! tests use that.

use std::path::{Path, PathBuf};

use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::{Action, Outcome, View};
use crate::layout::BlockBox;
use crate::render::{self, Frame};
use crate::text::{Align, Para, ParaLayout, TextStyle};
use crate::theme::{Role, Theme};

/// Sizes in logical pixels at 100% zoom: the invitation's text (as `paint_message`'s), the space
/// under it, the "Recent" heading's text and line, a row's text and height, the number column,
/// the space between a file's name and its folder, the line of "Clear list" and the space above
/// it, the list's widest, its least margin to the window's sides and to its top and bottom, and
/// how far a row's tint reaches past the text at the sides.
const MESSAGE: f32 = 16.0;
const GAP: f32 = 24.0;
const HEAD_TEXT: f32 = 14.0;
const HEAD: f32 = 26.0;
const TEXT: f32 = 16.0;
const ROW: f32 = 30.0;
const NUMBER: f32 = 28.0;
const SPACE: f32 = 10.0;
const CLEAR: f32 = 28.0;
const CLEAR_GAP: f32 = 6.0;
const WIDEST: f32 = 640.0;
const SIDE: f32 = 32.0;
const MARGIN: f32 = 16.0;
const TINT: f32 = 8.0;

const HEADING: &str = "Recent";
const CLEAR_LIST: &str = "Clear list";

/// A file of the list, with the words its row shows.
struct Entry {
    path: PathBuf,
    name: String,
    folder: String,
}

/// What a point of the list is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    Row(usize),
    Clear,
}

/// The list's state: the files to show (given by the shell), the row the keys picked, what is
/// under the pointer and pressed, and where the rows were painted.
#[derive(Default)]
pub(super) struct Screen {
    entries: Vec<Entry>,
    pub focus: Option<usize>,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    pointer: Option<(f64, f64)>,
    /// What shows, and where, in window pixels: `(x0, y0, x1, y1)`, as last painted (`painted`).
    spots: Vec<(Hit, (i32, i32, i32, i32))>,
    painted: bool,
}

/// Where the parts go, in window pixels: the invitation's top, the heading's, the first row's and
/// that of "Clear list", the list's sides, and how many rows show and whether "Clear list" does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Plan {
    pub message_y: f32,
    pub head_y: f32,
    pub rows_y: f32,
    pub clear_y: f32,
    pub x0: f32,
    pub x1: f32,
    pub rows: usize,
    pub clear: bool,
}

/// The plan for `n` files in a window `width` wide whose page starts at `top` and is `height`
/// tall, at scale `s`, under an invitation `message_h` tall. The block is centered; rows that
/// don't fit go from the bottom, "Clear list" before the last of them, and the invitation always
/// stays.
pub(super) fn plan(width: f32, top: f32, height: f32, s: f32, message_h: f32, n: usize) -> Plan {
    let need = |rows: usize, clear: bool| {
        let list = match rows {
            0 => 0.0,
            _ => GAP + HEAD + rows as f32 * ROW + if clear { CLEAR_GAP + CLEAR } else { 0.0 },
        };
        message_h + list * s
    };
    let room = height - 2.0 * MARGIN * s;
    let (rows, clear) = (1..=n)
        .rev()
        .find(|&r| need(r, true) <= room)
        .map(|r| (r, true))
        .or_else(|| (n > 0 && need(1, false) <= room).then_some((1, false)))
        .unwrap_or((0, false));
    let message_y = (top + ((height - need(rows, clear)) / 2.0).max(0.0)).round();
    let head_y = message_y + message_h + GAP * s;
    let rows_y = head_y + HEAD * s;
    let clear_y = rows_y + rows as f32 * ROW * s + CLEAR_GAP * s;
    let w = (width - 2.0 * SIDE * s).min(WIDEST * s).max(0.0);
    let x0 = ((width - w) / 2.0).round();
    Plan {
        message_y,
        head_y,
        rows_y,
        clear_y,
        x0,
        x1: x0 + w,
        rows,
        clear,
    }
}

/// A text style of the list at size `size`, in `role`.
fn style(size: f32, role: Role) -> TextStyle {
    TextStyle {
        bold: false,
        italic: false,
        mono: false,
        size,
        role,
        deco: 0,
        shift: 0.0,
        link: 0,
    }
}

impl View {
    /// Show `list` (files that are there, newest first) under the empty window's invitation, with
    /// `home` as `~`. A row picked with the keys stays picked, as far as the list still reaches.
    pub fn set_recent(&mut self, list: Vec<PathBuf>, home: Option<&Path>) {
        let r = &mut self.recent;
        // The same again (the window got focus, maybe for a click on a row): left as it is.
        let n = list.len().min(crate::recent::SHOWN);
        if r.entries.len() == n && r.entries.iter().zip(&list).all(|(e, p)| e.path == *p) {
            return;
        }
        let picked = r
            .focus
            .and_then(|f| r.entries.get(f))
            .map(|e| e.path.clone());
        r.entries = list
            .into_iter()
            .take(crate::recent::SHOWN)
            .map(|path| {
                let (name, folder) = crate::recent::label(&path, home);
                // Invisible characters (bidi controls) are shown, not acted on, as in a link's
                // preview; the name's are escaped when it is fitted.
                let folder = folder.chars().fold(String::new(), |mut s, c| {
                    if crate::nav::is_invisible(c) {
                        s.push_str(&format!("\\u{{{:04X}}}", c as u32));
                    } else {
                        s.push(c);
                    }
                    s
                });
                Entry { path, name, folder }
            })
            .collect();
        // The picked file stays picked wherever it went; gone (Delete), the row that took its
        // place is (seen once painted: keys wait for that).
        let n = r.entries.len();
        let clamped = r.focus.filter(|_| n > 0).map(|f| f.min(n - 1));
        let moved = picked.and_then(|p| r.entries.iter().position(|e| e.path == p));
        r.focus = moved.or(clamped);
        r.pressed = None;
        r.spots.clear();
        r.painted = false;
        r.hover = None;
    }

    /// The file of row `i`, if the list has one.
    pub fn recent_path(&self, i: u8) -> Option<PathBuf> {
        let e = self.recent.entries.get(usize::from(i))?;
        Some(e.path.clone())
    }

    /// The rows the list showed when it was last painted (fewer than its files in a small
    /// window; none before its first paint).
    pub fn recent_rows(&self) -> usize {
        if !self.recent_showing() {
            return 0;
        }
        let rows = |&&(hit, _): &&(Hit, _)| matches!(hit, Hit::Row(_));
        self.recent.spots.iter().filter(rows).count()
    }

    /// The list shows: the window has no document (an invitation, not an error, which forgets
    /// the list: see `open_doc`) and the list has files.
    pub(super) fn recent_showing(&self) -> bool {
        self.message.is_some() && !self.recent.entries.is_empty()
    }

    /// Another document, or none: the list is the shell's to give again.
    pub(super) fn forget_recent(&mut self) {
        self.recent = Screen::default();
    }

    /// What window point (`x`, `y`) is on.
    fn recent_at(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.recent_showing() {
            return None;
        }
        let inside = |&(x0, y0, x1, y1): &(i32, i32, i32, i32)| {
            (f64::from(x0)..f64::from(x1)).contains(&x)
                && (f64::from(y0)..f64::from(y1)).contains(&y)
        };
        let spot = self.recent.spots.iter().find(|(_, r)| inside(r));
        spot.map(|&(hit, _)| hit)
    }

    /// Look at what is under the pointer again. Returns true if that changed.
    fn recent_look(&mut self) -> bool {
        let hover = self.recent.pointer.and_then(|(x, y)| self.recent_at(x, y));
        std::mem::replace(&mut self.recent.hover, hover) != hover
    }

    /// The pointer moved to `at` (`None`: it left, or something else has it). Returns true if
    /// the window needs a repaint.
    pub(super) fn recent_pointer(&mut self, at: Option<(f64, f64)>) -> bool {
        self.recent.pointer = at;
        self.recent_look()
    }

    /// The pointer is on a row or on "Clear list", which shows the hand.
    pub fn over_recent(&self) -> bool {
        self.recent.hover.is_some()
    }

    /// A press at (`x`, `y`): true if it is on a row or on "Clear list" (a click there acts on
    /// release, on the same one).
    pub(super) fn press_recent(&mut self, x: f64, y: f64) -> bool {
        self.recent.pressed = self.recent_at(x, y);
        self.recent.pressed.is_some()
    }

    /// The button came up at (`x`, `y`) after a press on `hit`: a click if it is still on it,
    /// whose action waits for [`View::take_command`] as a toolbar button's does.
    pub(super) fn click_recent(&mut self, hit: Hit, x: f64, y: f64) {
        if self.recent_at(x, y) != Some(hit) {
            return;
        }
        let action = match hit {
            Hit::Row(i) => Action::OpenRecent(i as u8),
            Hit::Clear => Action::ClearRecent,
        };
        self.command = Some(crate::toolbar::Command::Action(action));
    }

    /// A key while the list shows: `1`–`8` open that row's file, the arrows (and `j`/`k`) pick
    /// a row, Enter opens the picked one and Delete (or Backspace, the Mac's delete key) takes it
    /// off the list. `None` for other keys, with a modifier other than Shift, or with no list.
    /// They act only on rows the reader has seen: a list given again and not painted yet takes
    /// them and does nothing.
    pub fn recent_key(&mut self, key: &Key, mods: ModifiersState) -> Option<Outcome> {
        if !self.recent_showing() || mods.control_key() || mods.alt_key() || mods.super_key() {
            return None;
        }
        if !self.recent.painted {
            let ours = match key {
                Key::Named(k) => matches!(
                    k,
                    NamedKey::ArrowDown
                        | NamedKey::ArrowUp
                        | NamedKey::Enter
                        | NamedKey::Delete
                        | NamedKey::Backspace
                ),
                Key::Character(c) => {
                    matches!(c.as_str(), "j" | "J" | "k" | "K")
                        || matches!(c.as_bytes(), [b'1'..=b'8'])
                }
                _ => false,
            };
            return ours.then_some(Outcome::Nothing);
        }
        let rows = self.recent_rows();
        let step = |view: &mut View, by: isize| {
            let last = rows.saturating_sub(1);
            let next = match view.recent.focus {
                None if by > 0 => 0,
                None => last,
                Some(f) => f.saturating_add_signed(by).min(last),
            };
            view.recent.focus = Some(next);
            Some(Outcome::Redraw)
        };
        let focus = self.recent.focus.filter(|&f| f < rows);
        match key {
            Key::Character(c) => match c.as_str() {
                "j" | "J" => step(self, 1),
                "k" | "K" => step(self, -1),
                d => {
                    let n = d.parse::<usize>().ok().filter(|n| (1..=rows).contains(n))?;
                    Some(Outcome::OpenRecent(n as u8 - 1))
                }
            },
            Key::Named(NamedKey::ArrowDown) => step(self, 1),
            Key::Named(NamedKey::ArrowUp) => step(self, -1),
            Key::Named(NamedKey::Enter) => focus.map(|f| Outcome::OpenRecent(f as u8)),
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                focus.map(|f| Outcome::ForgetRecent(f as u8))
            }
            _ => None,
        }
    }

    /// `raw` on one line no wider than `max_w`, its start cut off with a `…` if it is wider: a
    /// folder keeps its most specific part.
    fn fit_start(&mut self, raw: &str, style: TextStyle, max_w: f32) -> (Para, ParaLayout) {
        let lay = |view: &mut View, text: &str| {
            let mut para = Para::default();
            para.push_text(text, 0, style);
            let pl = view
                .msg_ctx
                .layout(&view.fonts, &para, 1.0e6, Align::Start, 1.4);
            (para, pl)
        };
        let starts: Vec<usize> = raw.char_indices().map(|(i, _)| i).collect();
        // `raw` without its first `k` characters, after a `…` (none left out: as it is).
        let cut = |k: usize| match k {
            0 => raw.to_owned(),
            _ => format!("…{}", &raw[starts.get(k).map_or(raw.len(), |&i| i)..]),
        };
        // The fewest left out for the rest to fit. Characters narrower than a fifth of an em are
        // rare, so no more than the last that many are ever laid out: a huge folder costs no more.
        let most = (max_w / (0.2 * style.size)) as usize + 1;
        let (mut lo, mut hi) = (starts.len().saturating_sub(most), starts.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if lay(self, &cut(mid)).1.width <= max_w {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lay(self, &cut(lo))
    }

    /// Paint `(para, pl)` with its top left at (`x`, `y`), in `theme` with its text color
    /// `text`. Returns its width.
    fn paint_line(
        &mut self,
        frame: &mut Frame,
        (para, pl): (Para, ParaLayout),
        x: f32,
        y: f32,
        theme: &Theme,
    ) -> f32 {
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let width = pl.width;
        let content = BlockBox::from_para(pl, &para.styles);
        render::paint_block(
            frame,
            &content,
            x,
            y,
            &self.fonts,
            &mut self.cache,
            theme,
            #[cfg(feature = "clipboard")]
            None,
        );
        width
    }

    /// The empty window with its list: the invitation `text`, then the rows that fit.
    pub(super) fn paint_recent(&mut self, frame: &mut Frame, text: &str, theme: &Theme) {
        let (w, h) = (frame.width() as f32, frame.height() as f32);
        frame.fill_rect(0, 0, w as i32, h as i32, theme.bg);
        let s = self.scale();
        let mut para = Para::default();
        para.push_text(text, 0, style(MESSAGE * s, Role::Muted));
        let max_w = (w - 2.0 * SIDE * s).max(MESSAGE * s);
        let pl = self
            .msg_ctx
            .layout(&self.fonts, &para, max_w, Align::Center, 1.5);
        let top = self.top_inset as f32;
        let p = plan(w, top, h - top, s, pl.height, self.recent.entries.len());
        self.paint_line(frame, (para, pl), (w - max_w) / 2.0, p.message_y, theme);
        // Where everything goes first, for what is under the pointer to be known when painting.
        let px = |v: f32| v.round() as i32;
        let (row, tint) = (ROW * s, TINT * s);
        let spot =
            |y: f32, x1: f32, height: f32| (px(p.x0 - tint), px(y), px(x1 + tint), px(y + height));
        let mut spots: Vec<_> = (0..p.rows)
            .map(|i| (Hit::Row(i), spot(p.rows_y + i as f32 * row, p.x1, row)))
            .collect();
        let clear = self.fit_line(
            CLEAR_LIST,
            style(HEAD_TEXT * s, Role::Muted),
            p.x1 - p.x0,
            false,
        );
        if p.clear {
            let x1 = p.x0 + clear.1.width;
            spots.push((Hit::Clear, spot(p.clear_y, x1, CLEAR * s)));
        }
        self.recent.spots = spots;
        self.recent.painted = true;
        self.recent_look();
        if p.rows == 0 {
            return;
        }
        let r = self.recent.pressed;
        let (hover, focus) = (self.recent.hover, self.recent.focus);
        for &(hit, rect) in &self.recent.spots {
            let alpha = match hit {
                _ if r == Some(hit) && hover == Some(hit) => 200,
                Hit::Row(i) if focus == Some(i) => 170,
                _ if hover == Some(hit) => 110,
                _ => continue,
            };
            frame.rounded_rect(rect, (4.0 * s).round(), None, theme.border, alpha);
        }
        // The heading, and each row: its number, the file's name, its folder in what is left.
        let center = |line_h: f32, height: f32| (line_h - height) / 2.0;
        let head = self.fit_line(
            HEADING,
            style(HEAD_TEXT * s, Role::Muted),
            p.x1 - p.x0,
            false,
        );
        let y = p.head_y + center(HEAD * s, head.1.height);
        self.paint_line(frame, head, p.x0, y, theme);
        for i in 0..p.rows {
            let y = p.rows_y + i as f32 * row;
            let (name, folder) = {
                let e = &self.recent.entries[i];
                (e.name.clone(), e.folder.clone())
            };
            let number = self.fit_line(
                &(i + 1).to_string(),
                style(TEXT * s, Role::Muted),
                1e6,
                false,
            );
            let dy = center(row, number.1.height);
            self.paint_line(frame, number, p.x0, y + dy, theme);
            let x = p.x0 + NUMBER * s;
            let name = self.fit_line(
                &name,
                style(TEXT * s, Role::Link),
                (p.x1 - x).max(0.0),
                true,
            );
            let used = self.paint_line(frame, name, x, y + dy, theme);
            let x = x + used + SPACE * s;
            if p.x1 - x > 2.0 * TEXT * s {
                let folder = self.fit_start(&folder, style(TEXT * s, Role::Muted), p.x1 - x);
                self.paint_line(frame, folder, x, y + dy, theme);
            }
        }
        if p.clear {
            let y = p.clear_y + center(CLEAR * s, clear.1.height);
            self.paint_line(frame, clear, p.x0, y, theme);
        }
    }
}

#[cfg(test)]
mod tests;
