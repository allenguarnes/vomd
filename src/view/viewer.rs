//! The image viewer in the window: a click on an image opens it over the whole window (see
//! [`crate::viewer`] for its state), and the window's input goes to it until it is closed. The
//! document under it isn't touched: its position, selection, find bar and contents panel are as
//! they were when it closes.
//!
//! Its pixels come from the image threads ([`Images::detail`]) one decode at a time, asked for
//! once the zoom settles; nothing runs and no timer is set once it has settled. An animated image
//! plays in it through the window's animations, in a box of its own size.

use std::time::Instant;

use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::CursorIcon;

use super::{Action, Outcome, View, key_action};
use crate::doc::Want;
use crate::image::{AnimState, Origin};
use crate::layout::ImageSize;
use crate::render::{Frame, Pixels as _};
use crate::text::{Para, TextStyle};
use crate::theme::{Role, Theme};
use crate::toolbar::{self, Rect, icon};
use crate::viewer::{Button, Viewer};

/// The smallest box (each way, in logical pixels) an image has for a click to open it and for
/// stepping to reach it: badges, emoji-sized images and icons don't.
const OPEN_MIN: f32 = 48.0;
/// The close button's size and its margin from the window's corner, the full resolution
/// button's text size, in logical pixels.
const BUTTON: f32 = 32.0;
const EDGE: f32 = 8.0;
const TEXT: f32 = 13.0;
/// What the full resolution button says.
const FULL: &str = "Full resolution";
/// What a tenth of a pinch's magnification counts as in [`View::zoom_wheel`] (`ZOOM_PIXELS`).
const PINCH_TENTH: f64 = super::ZOOM_PIXELS;
/// A wheel notch moves the image this many logical pixels.
const WHEEL: f64 = 120.0;

impl View {
    /// Whether the image viewer is open.
    pub fn viewing(&self) -> bool {
        self.viewer.is_some()
    }

    /// The image at (`x`, `y`) that a click opens in the viewer: one that shows its pixels (not a
    /// placeholder, nor loading, blocked or broken), at least [`OPEN_MIN`] each way
    /// ([`View::viewer_worthy`]).
    pub(super) fn openable_at(&self, x: f64, y: f64) -> Option<u32> {
        if self.message.is_some() || !self.in_area(x, y) {
            return None;
        }
        let id = self.image_at(x, y)?;
        if !matches!(self.layout.image_size(id), Some(ImageSize::Pixels(..)))
            || self.images.blocked(id)
            || !self.images.shown(id)
        {
            return None;
        }
        self.viewer_worthy(id).then_some(id)
    }

    /// Whether image `id` is big enough for the viewer: the box its document asks for (its
    /// `<img>`'s `width` and `height`, else its own size) is at least [`OPEN_MIN`] logical
    /// pixels each way. The one rule for a click and for stepping, from what the document
    /// says and not how it is zoomed or where it is: a badge, an emoji or an icon is neither
    /// opened nor stepped to, and a small animation shown at 120 is both.
    fn viewer_worthy(&self, id: u32) -> bool {
        let Some((nw, nh)) = self.natural_of(id) else {
            return false;
        };
        let want = id
            .checked_sub(1)
            .and_then(|i| self.doc.images.get(i as usize))
            .map_or([Want::Auto; 2], |img| img.want);
        // (A percentage is of the line: wide.)
        let side = |w: Want| match w {
            Want::Auto => None,
            Want::Px(v) => Some(v as f32 / 100.0),
            Want::Pct(_) => Some(f32::INFINITY),
        };
        let (nw, nh) = (nw as f32, nh as f32);
        let (w, h) = match (side(want[0]), side(want[1])) {
            (None, None) => (nw, nh),
            (Some(w), None) => (w, w * nh / nw),
            (None, Some(h)) => (h * nw / nh, h),
            (Some(w), Some(h)) => (w, h),
        };
        w.min(h) >= OPEN_MIN
    }

    /// Open image `id` in the viewer at `now`, the press that did it at `opened` (its second
    /// click does nothing). Returns false if it can't be shown.
    pub(super) fn open_viewer(
        &mut self,
        id: u32,
        opened: Option<(Instant, (f64, f64))>,
        now: Instant,
    ) -> bool {
        let Some((w, h)) = self.natural_of(id) else {
            return false;
        };
        let (area, hidpi) = ((f64::from(self.width), f64::from(self.height)), self.hidpi);
        let mut v = Viewer::new(id, (w, h), self.images.vector(id), area, hidpi);
        v.origin = self.images.origin(id);
        v.stamp = file_stamp(v.origin.as_ref());
        // A copy: the document's boxes aren't painted while the viewer covers them, and the cache
        // may let them go.
        v.preview = self.images.pixels(id, w, h).cloned();
        v.anim_box = self.anim_box(id, (w, h), v.tf.s);
        if let Some(old) = self.viewer.take() {
            (v.full, v.list) = (old.full, old.list);
        } else {
            v.opened = opened;
            v.list = self.viewer_list();
        }
        v.changed(now);
        self.viewer = Some(Box::new(v));
        self.hover = None;
        self.hover_note = None;
        self.hover_force = false;
        self.pressed = None;
        self.pressed_image = None;
        self.close_menu();
        self.viewer_decode(now);
        true
    }

    /// Open image `id` in the viewer at `now`, as a click on it does (tests and benches).
    #[doc(hidden)]
    pub fn open_image(&mut self, id: u32, now: Instant) -> bool {
        self.open_viewer(id, None, now)
    }

    /// Whether the viewer waits for a decode (tests).
    #[doc(hidden)]
    pub fn viewer_waiting(&self) -> bool {
        self.viewer.as_ref().is_some_and(|v| v.asked.is_some())
    }

    /// Close the viewer: the document shows as it was. Returns true if it was open.
    pub fn close_viewer(&mut self) -> bool {
        if self.viewer.take().is_none() {
            return false;
        }
        self.images.release_detail();
        true
    }

    /// Ask for the decode the viewer wants next, if any.
    fn viewer_decode(&mut self, now: Instant) {
        let Some(v) = self.viewer.as_mut() else {
            return;
        };
        let Some(want) = v.next_decode() else {
            return;
        };
        match self.images.detail(v.id, want.0, want.1) {
            Some(serial) => v.asked_for(want, serial, now),
            None => {
                // (What runs is for an image or a size no longer shown.)
                v.failed = Some(want);
                v.asked = None;
                self.images.cancel_detail();
            }
        }
    }

    /// Image `id`'s natural size: the layout's, or for a block not measured yet, its file's
    /// header (a remote image's is known once it is laid out). `None` for an image that shows
    /// no pixels (broken, refused, blocked or loading).
    fn natural_of(&self, id: u32) -> Option<(u32, u32)> {
        match self.layout.image_size(id) {
            Some(ImageSize::Pixels(w, h)) => Some((w, h)),
            Some(_) => None,
            None => match self.images.origin(id)? {
                Origin::File(path) => crate::image::probe_file(&path),
                Origin::Url(_) => None,
            },
        }
        .filter(|_| !self.images.blocked(id))
    }

    /// The images the viewer steps through: those of the document that show their pixels (or
    /// will, once in view), in order: those [`View::viewer_worthy`] says. An image not measured
    /// yet costs a read of its file's first 4 KiB (as laying it out will), here and when a
    /// reload comes with the viewer open. Blocked, broken and refused ones are left out.
    fn viewer_list(&self) -> Vec<u32> {
        (1..=self.doc.images.len() as u32)
            .filter(|&id| self.viewer_worthy(id))
            .collect()
    }

    /// Step `by` images (negative: back), or to the first or last (`by` = `i32::MIN`, `MAX`):
    /// at the ends it stops. Returns true if another image shows.
    fn viewer_step(&mut self, by: i32, now: Instant) -> bool {
        let Some(v) = self.viewer.as_ref() else {
            return false;
        };
        let (id, list) = (v.id, &v.list);
        let next = match by {
            i32::MIN => list.first().copied(),
            i32::MAX => list.last().copied(),
            n if n < 0 => list.iter().rev().find(|&&i| i < id).copied(),
            _ => list.iter().find(|&&i| i > id).copied(),
        };
        match next {
            Some(n) if n != id => self.open_viewer(n, None, now),
            _ => false,
        }
    }

    /// The viewer's image, as `n` of `of` images to step through.
    pub(super) fn viewer_place(&self) -> Option<(usize, usize)> {
        let v = self.viewer.as_ref()?;
        Some((v.list.iter().position(|&i| i == v.id)? + 1, v.list.len()))
    }

    // ---- Input ----------------------------------------------------------------------------

    /// The button went down at (`x`, `y`) with the viewer open.
    pub(super) fn viewer_press(&mut self, x: f64, y: f64, now: Instant) -> bool {
        let Some(v) = self.viewer.as_mut() else {
            return false;
        };
        v.press(x, y, now);
        false
    }

    /// The pointer moved to (`x`, `y`) with the button down: a drag moves the image. Returns true
    /// if it moved.
    pub fn viewer_drag(&mut self, x: f64, y: f64, now: Instant) -> bool {
        let Some(v) = self.viewer.as_mut() else {
            return false;
        };
        let moved = v.drag(x, y);
        if moved {
            v.moved(now);
        }
        moved
    }

    /// The window lost the keyboard: a drag of the image ends there (its release isn't seen).
    pub fn viewer_unpress(&mut self) {
        if let Some(v) = self.viewer.as_mut() {
            v.press = None;
        }
    }

    /// The button came up at (`x`, `y`) after a press in the viewer: a click on the image
    /// switches between Fit and 100%, on a button does what it says, beside the image closes.
    /// Nothing for a drag, or for the second click of the double click that opened it.
    pub(super) fn viewer_click(&mut self, x: f64, y: f64, now: Instant) {
        let Some(v) = self.viewer.as_mut() else {
            return;
        };
        let Some(press) = v.press.take() else {
            return;
        };
        if press.moved || press.ignored {
            return;
        }
        match self.viewer_button(x, y) {
            Some(Button::Close) => {
                self.close_viewer();
            }
            Some(Button::Full) => self.viewer_full(now),
            None => {
                let Some(v) = self.viewer.as_mut() else {
                    return;
                };
                if !v.on_image(x, y) || !v.toggle((x, y)) {
                    self.close_viewer();
                } else {
                    v.changed(now);
                }
            }
        }
    }

    /// Full resolution on or off.
    fn viewer_full(&mut self, now: Instant) {
        let Some(v) = self.viewer.as_mut() else {
            return;
        };
        if !v.over_cap() {
            return;
        }
        v.set_full(!v.full);
        if !v.full && v.asked.is_some_and(|a| a.crop.is_some()) {
            v.asked = None;
            self.images.cancel_detail();
        }
        self.viewer_decode(now);
    }

    /// The button at (`x`, `y`), as last painted.
    fn viewer_button(&self, x: f64, y: f64) -> Option<Button> {
        let v = self.viewer.as_ref()?;
        v.buttons
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(b, _)| *b)
    }

    /// The pointer moved to (`x`, `y`) with the viewer open. Returns true if a button lit up
    /// or went out.
    pub(super) fn viewer_hover(&mut self, x: f64, y: f64) -> bool {
        let hot = self.viewer_button(x, y);
        let Some(v) = self.viewer.as_mut() else {
            return false;
        };
        v.pointer = Some((x, y));
        std::mem::replace(&mut v.hot, hot) != hot
    }

    /// The pointer left the window.
    pub(super) fn viewer_leave(&mut self) -> bool {
        let Some(v) = self.viewer.as_mut() else {
            return false;
        };
        v.pointer = None;
        v.hot.take().is_some()
    }

    /// The cursor the viewer shows (`None`: it is closed): the closed hand while the image is
    /// dragged, the open one over an image that a drag moves, the hand over a button and over
    /// an image a click zooms.
    pub fn viewer_cursor(&self) -> Option<CursorIcon> {
        let v = self.viewer.as_ref()?;
        Some(if v.dragging() {
            CursorIcon::Grabbing
        } else if v.hot.is_some() {
            CursorIcon::Pointer
        } else if v.pointer.is_some_and(|(x, y)| v.on_image(x, y)) {
            if v.pannable() {
                CursorIcon::Grab
            } else {
                CursorIcon::Pointer
            }
        } else {
            CursorIcon::Default
        })
    }

    /// The wheel or a touchpad with the viewer open: `x`, `y` as winit gives them (positive:
    /// the content comes left or down), in notches (`lines`) or pixels. With `zoom` (Ctrl or ⌘,
    /// a Windows touchpad's pinch) it zooms about the pointer; else it moves the image, across
    /// with Shift. Returns true if it changed.
    pub fn viewer_wheel(
        &mut self,
        (x, y): (f64, f64),
        lines: bool,
        zoom: bool,
        shift: bool,
        now: Instant,
    ) -> bool {
        let Some(v) = self.viewer.as_mut() else {
            return false;
        };
        let changed = if zoom {
            let p = v.pointer.unwrap_or((v.area.0 / 2.0, v.area.1 / 2.0));
            if lines {
                v.zoom_about(p, crate::viewer::STEP.powf(y))
            } else {
                v.zoom_pixels(p, y)
            }
        } else {
            let k = if lines { WHEEL * v.hidpi } else { 1.0 };
            let (dx, dy) = if shift { (y, x) } else { (x, y) };
            v.pan(dx * k, dy * k)
        };
        if changed && zoom {
            v.changed(now);
        } else if changed {
            v.moved(now);
        }
        changed
    }

    /// A pinch on macOS, as the document's zoom counts it (`pixels`: [`PINCH_TENTH`] for every
    /// tenth of magnification, positive: in), about the pointer: the image grows by what the
    /// fingers did, e^(magnification).
    pub(super) fn viewer_zoom_pixels(&mut self, pixels: f64, now: Instant) {
        if let Some(v) = self.viewer.as_mut() {
            let p = v.pointer.unwrap_or((v.area.0 / 2.0, v.area.1 / 2.0));
            if v.zoom_about(p, (pixels / (10.0 * PINCH_TENTH)).exp()) {
                v.changed(now);
            }
        }
    }

    /// A key with the viewer open: what it does there (`None`: the viewer is closed). Every key
    /// is the viewer's while it is: those that mean nothing in it do nothing.
    pub fn viewer_key(&mut self, key: &Key, mods: ModifiersState, now: Instant) -> Option<Outcome> {
        let v = self.viewer.as_mut()?;
        let command = mods.control_key() || mods.super_key();
        let plain = !command && !mods.alt_key();
        let animated = v.anim_box.is_some();
        // The viewer's own keys first: those `key_action` has no meaning for, or another one.
        let own = match key {
            Key::Character(c) if !mods.alt_key() => match c.as_str() {
                "+" | "=" => Some(Action::ZoomIn),
                "-" | "_" => Some(Action::ZoomOut),
                "0" => Some(Action::ZoomReset),
                "f" | "F" if plain => {
                    self.viewer_full(now);
                    return Some(Outcome::Redraw);
                }
                " " if plain && animated && !mods.shift_key() => {
                    self.viewer_play();
                    return Some(Outcome::Redraw);
                }
                _ => None,
            },
            Key::Named(NamedKey::Space) if plain && animated && !mods.shift_key() => {
                self.viewer_play();
                return Some(Outcome::Redraw);
            }
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) if plain => {
                let n = if matches!(key, Key::Named(NamedKey::ArrowLeft)) {
                    -1
                } else {
                    1
                };
                if v.wider() {
                    if v.pan_lines(f64::from(n), 0.0) {
                        v.moved(now);
                    }
                } else {
                    self.viewer_step(n, now);
                }
                return Some(Outcome::Redraw);
            }
            _ => None,
        };
        let Some(action) = own.or_else(|| key_action(key, mods)) else {
            return Some(Outcome::Nothing);
        };
        let v = self.viewer.as_mut()?;
        match action {
            Action::Lines(n) => {
                if v.pan_lines(0.0, f64::from(n)) {
                    v.moved(now);
                }
            }
            Action::Pages(n) => {
                self.viewer_step(n, now);
            }
            Action::Top => {
                self.viewer_step(i32::MIN, now);
            }
            Action::Bottom => {
                self.viewer_step(i32::MAX, now);
            }
            Action::ZoomIn | Action::ZoomOut => {
                if v.zoom_steps(if action == Action::ZoomIn { 1.0 } else { -1.0 }) {
                    v.changed(now);
                }
            }
            Action::ZoomReset => {
                if v.toggle((v.area.0 / 2.0, v.area.1 / 2.0)) {
                    v.changed(now);
                }
            }
            Action::Back | Action::Escape => {
                self.close_viewer();
            }
            Action::CycleTheme
            | Action::Reload
            | Action::Open
            | Action::ToggleAnimations
            | Action::Quit => return Some(self.apply(action, now)),
            #[cfg(target_os = "macos")]
            Action::Settings | Action::Minimize => return Some(self.apply(action, now)),
            _ => return Some(Outcome::Nothing),
        }
        Some(Outcome::Redraw)
    }

    /// `Space` on an animated image: it plays, or stops where it is.
    fn viewer_play(&mut self) {
        let Some(id) = self.viewer.as_ref().map(|v| v.id) else {
            return;
        };
        if self.images.anim_state(id) == Some(AnimState::Playing) {
            self.images.pause(id);
        } else {
            self.images.play(id);
        }
    }

    // ---- What changes under it --------------------------------------------------------------

    /// The window's size or scale changed.
    pub(super) fn viewer_area(&mut self, now: Instant) {
        let (area, hidpi) = ((f64::from(self.width), f64::from(self.height)), self.hidpi);
        let Some(v) = self.viewer.as_mut() else {
            return;
        };
        let rescaled = v.hidpi != hidpi;
        v.set_area(area, hidpi);
        v.changed(now);
        // A new display scale makes an animation's frames again, at its box for that scale (a
        // resize alone keeps the box: frames aren't made again while the window is dragged).
        if rescaled {
            let (id, natural, s) = (v.id, v.natural, v.tf.s);
            let anim_box = self.anim_box(id, natural, s);
            if let Some(v) = self.viewer.as_mut() {
                v.anim_box = anim_box;
            }
        }
    }

    /// The box an animated image's frames are made at in the viewer, at scale `s`: its own size,
    /// or smaller when it is shown smaller. `None` for an image that isn't animated.
    fn anim_box(&self, id: u32, (w, h): (u32, u32), s: f64) -> Option<(u32, u32)> {
        self.images.animated(id)?;
        let s = s.min(1.0);
        let side = |n: u32| (f64::from(n) * s).round().max(1.0) as u32;
        Some((side(w), side(h)))
    }

    /// The document was replaced (a reload): the viewer follows its image by where it comes
    /// from. If it is still there it is that image again, decoded again if its file changed
    /// (its zoom and place kept if its size is the same); if it is gone (or has no place to
    /// find it by, a data URI), the viewer closes: its id means another image now, and nothing
    /// it asks for may be that one.
    pub(super) fn viewer_reload(&mut self, now: Instant) {
        let Some(v) = self.viewer.as_ref() else {
            return;
        };
        let n = self.doc.images.len() as u32;
        // Of several copies of the image, the one nearest where it was: the same one when
        // nothing before it changed.
        let found = v.origin.clone().and_then(|origin| {
            (1..=n)
                .filter(|&id| self.images.origin(id).as_ref() == Some(&origin))
                .min_by_key(|&id| id.abs_diff(v.id))
        });
        let Some(id) = found else {
            self.close_viewer();
            return;
        };
        let list = self.viewer_list();
        if let Some(v) = self.viewer.as_mut() {
            v.list = list;
        }
        self.viewer_repoint(id, now);
    }

    /// The theme changed what a `<picture>` shows: the viewer shows what the document does.
    pub(super) fn viewer_theme(&mut self, now: Instant) {
        let Some(v) = self.viewer.as_ref() else {
            return;
        };
        let id = v.id;
        if self.images.origin(id) != v.origin {
            self.viewer_repoint(id, now);
        }
    }

    /// The viewer's image is image `id` now, from where it came, or from elsewhere (a theme's
    /// other `<picture>`): if its file or its source changed it is decoded again, at the same
    /// zoom and place if its size is the same, else fitted.
    fn viewer_repoint(&mut self, id: u32, now: Instant) {
        let size = self.natural_of(id);
        let origin = self.images.origin(id);
        let stamp = file_stamp(origin.as_ref());
        let vector = self.images.vector(id);
        let Some(v) = self.viewer.as_mut() else {
            return;
        };
        v.id = id;
        // (A reload stopped what was decoding.)
        v.asked = None;
        v.failed = None;
        if origin == v.origin && stamp == v.stamp {
            self.viewer_decode(now);
            return;
        }
        (v.origin, v.stamp, v.vector, v.fresh) = (origin, stamp, vector, false);
        v.crop = None;
        if let Some((w, h)) = size
            && (w, h) != v.natural
        {
            v.natural = (w, h);
            v.detail = None;
            v.preview = self.images.pixels(id, w, h).cloned();
            v.open_zoom();
        }
        // (It may have become an animation, stopped being one, or changed its size.)
        let (natural, s) = (v.natural, v.tf.s);
        let anim_box = self.anim_box(id, natural, s);
        if let Some(v) = self.viewer.as_mut() {
            v.anim_box = anim_box;
        }
        self.viewer_decode(now);
    }

    // ---- Time -----------------------------------------------------------------------------

    /// When the viewer next needs the event loop.
    pub(super) fn viewer_deadline(&self) -> Option<Instant> {
        self.viewer.as_ref()?.deadline()
    }

    /// What is due by `now`: the label hides, a decode is asked for. Returns true to repaint.
    pub(super) fn viewer_expire(&mut self, now: Instant) -> bool {
        let Some(v) = self.viewer.as_mut() else {
            return false;
        };
        let (repaint, settle) = v.expire(now);
        if settle {
            self.viewer_decode(now);
        }
        repaint
    }

    /// The image threads' poll: the viewer's decode, if it is done, shows, and the next one
    /// (the part on screen, once the whole image is there) is asked for. Returns true to
    /// repaint.
    pub(super) fn viewer_poll(&mut self, now: Instant) -> bool {
        let Some((serial, bitmap)) = self.images.take_detail() else {
            return false;
        };
        let Some(v) = self.viewer.as_mut() else {
            return false;
        };
        let shown = v.took(serial, bitmap);
        self.viewer_decode(now);
        shown
    }

    // ---- Painting -------------------------------------------------------------------------

    /// Paint the viewer over the whole window: the document's background, the image, the close
    /// button, the full resolution button for an image that has more pixels than the detail,
    /// the zoom label and the alt text while they show.
    pub(super) fn paint_viewer(&mut self, frame: &mut Frame, now: Instant, whole: bool) {
        let theme = self.palette();
        let (w, h) = (frame.width() as i32, frame.height() as i32);
        frame.fill_rect(0, 0, w, h, theme.bg);
        if whole {
            self.images.begin_frame();
        }
        let Some(v) = self.viewer.as_mut() else {
            return;
        };
        let (x, y, bw, bh) = v.rect();
        // An animation shows its frame (made at its own box, enlarged), while it has one.
        let frame_of = match v.anim_box {
            Some((aw, ah)) => {
                let r = (
                    x.max(0),
                    y.max(0),
                    x.saturating_add(bw as i32).min(w),
                    y.saturating_add(bh as i32).min(h),
                );
                self.images.placed(v.id, r);
                self.images.anim_frame(v.id, aw, ah)
            }
            None => None,
        };
        if let Some(bm) = frame_of {
            frame.blit_bitmap(x, y, bw, bh, bm);
        } else {
            if let Some(bm) = v.detail.as_ref().or(v.preview.as_ref()) {
                frame.blit_bitmap(x, y, bw, bh, bm);
            }
            // The part decoded at full resolution, where it is in the image at its full size.
            if let Some((bm, c)) = &v.crop {
                let (kx, ky) = (
                    f64::from(bw) / f64::from(c.full.0.max(1)),
                    f64::from(bh) / f64::from(c.full.1.max(1)),
                );
                let at =
                    |o: i32, n: u32, k: f64| o.saturating_add((f64::from(n) * k).round() as i32);
                let (cx, cy) = (at(x, c.at.0, kx), at(y, c.at.1, ky));
                let (cx1, cy1) = (at(x, c.at.0 + bm.width, kx), at(y, c.at.1 + bm.height, ky));
                let size = |a: i32, b: i32| (b.saturating_sub(a)).max(1) as u32;
                frame.blit_bitmap(cx, cy, size(cx, cx1), size(cy, cy1), bm);
            }
        }
        self.images.painted();
        self.paint_viewer_chrome(frame, theme, now);
    }

    /// The viewer's buttons, label and alt text.
    fn paint_viewer_chrome(&mut self, frame: &mut Frame, theme: &Theme, now: Instant) {
        let s = self.hidpi as f32;
        let w = frame.width() as f32;
        let mut buttons = Vec::new();
        // ✕ at the top right, on a pill of its own: it may be over the image.
        let close = Rect {
            x0: w - (EDGE + BUTTON) * s,
            y0: EDGE * s,
            x1: w - EDGE * s,
            y1: (EDGE + BUTTON) * s,
        };
        let hot = self.viewer.as_ref().and_then(|v| v.hot);
        self.viewer_pill(frame, close, theme, hot == Some(Button::Close), false);
        let color = if hot == Some(Button::Close) {
            theme.text
        } else {
            theme.muted
        };
        toolbar::draw_icon(
            frame,
            &self.fonts,
            &mut self.cache,
            icon::CLOSE,
            close,
            s,
            color,
        );
        buttons.push((Button::Close, close));
        // Full resolution at the top left, for an image with more pixels than the detail.
        let (over, full) = self
            .viewer
            .as_ref()
            .map_or((false, false), |v| (v.over_cap(), v.full));
        if over {
            let (content, tw, th) =
                self.viewer_text(FULL, if full { Role::Link } else { Role::Text });
            let (pad, top) = (12.0 * s, EDGE * s);
            let r = Rect {
                x0: EDGE * s,
                y0: top,
                x1: EDGE * s + tw + 2.0 * pad,
                y1: top + BUTTON * s,
            };
            self.viewer_pill(frame, r, theme, hot == Some(Button::Full), full);
            let ty = r.y0 + (BUTTON * s - th) / 2.0;
            crate::render::paint_block(
                frame,
                &content,
                r.x0 + pad,
                ty,
                &self.fonts,
                &mut self.cache,
                theme,
                #[cfg(feature = "clipboard")]
                None,
            );
            buttons.push((Button::Full, r));
        }
        if let Some(v) = self.viewer.as_mut() {
            v.buttons = buttons;
        }
        // The label and the alt text, while they show.
        let place = self.viewer_place();
        let Some(v) = self.viewer.as_ref() else {
            return;
        };
        if !v.label_shows(now) {
            return;
        }
        let id = v.id;
        let note = v
            .anim_box
            .and_then(|_| self.images.anim_state(id))
            .map(anim_note);
        let label = v.label(now, place, note);
        self.paint_toast(frame, &label, theme, false);
        if let Some(alt) = self.viewer_alt(id) {
            self.paint_preview(frame, &alt, theme, true);
        }
    }

    /// A button's pill: lit while hovered, or while on.
    fn viewer_pill(&self, frame: &mut Frame, r: Rect, theme: &Theme, hot: bool, on: bool) {
        let s = self.hidpi as f32;
        let rect = r.px();
        let radius = (6.0 * s).round();
        frame.rounded_rect(rect, radius, None, theme.code_bg, 255);
        if hot || on {
            frame.rounded_rect(rect, radius, None, theme.border, if hot { 160 } else { 90 });
        }
        frame.rounded_rect(rect, radius, Some(s.round().max(1.0)), theme.border, 255);
    }

    /// One line of the viewer's text laid out: its box, width and height.
    fn viewer_text(&mut self, text: &str, role: Role) -> (crate::layout::BlockBox, f32, f32) {
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: TEXT * self.hidpi as f32,
            role,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let mut para = Para::default();
        para.push_text(text, 0, style);
        let pl = self
            .msg_ctx
            .layout(&self.fonts, &para, 1.0e6, crate::text::Align::Start, 1.4);
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let (w, h) = (pl.width, pl.height);
        (crate::layout::BlockBox::from_para(pl, &para.styles), w, h)
    }

    /// Image `id`'s alt text, if it says more than the file's name.
    fn viewer_alt(&self, id: u32) -> Option<String> {
        let img = self.doc.images.get(id.checked_sub(1)? as usize)?;
        let mut alt = String::new();
        for block in &self.doc.blocks {
            let cells = self.doc.row_cells(block).map(|(_, spans)| spans);
            for span in std::iter::once(self.doc.block_spans(block))
                .chain(cells)
                .flatten()
                .filter(|s| s.image == id)
            {
                alt.push_str(self.doc.span_text(span));
            }
            if !alt.is_empty() {
                break;
            }
        }
        let alt = alt.trim();
        let name = img.src.rsplit(['/', '\\']).next().unwrap_or("");
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        (!alt.is_empty() && alt != name && alt != stem).then(|| alt.to_string())
    }
}

/// What the label says of an animation: its state, and what `Space` does about it.
pub(super) fn anim_note(state: AnimState) -> &'static str {
    match state {
        AnimState::Playing => "Space pauses",
        AnimState::Paused => "Space plays",
        AnimState::Waiting => "Space plays now",
        AnimState::Over => "Over VOMD's animation limits · Space plays anyway",
        AnimState::Failed => "Couldn't load the animation · Space tries again",
    }
}

/// The stamp of a local image's file (`None` for a remote one).
fn file_stamp(origin: Option<&Origin>) -> Option<super::Stamp> {
    match origin? {
        Origin::File(path) => super::stamp(path),
        Origin::Url(_) => None,
    }
}
