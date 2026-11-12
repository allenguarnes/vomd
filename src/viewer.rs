//! The image viewer: one image of the document over the whole window, zoomed and moved about.
//! This is its state and its geometry, with no window, fonts or decoding: the view drives it
//! (`view/viewer.rs`), paints it and hands it the pixels the image threads make.
//!
//! Sizes are physical pixels. The image's place is a [`Transform`]: its top left corner in the
//! window and `s`, window pixels per image pixel, `s` = the display's scale being 100% (the
//! image's natural size in logical pixels, as the document sizes it at zoom 100%). What is shown
//! is the best of the pixels it has, enlarged nearest-pixel by the paint: first the document's
//! own bitmap, then a decode at the size the zoom needs (its *detail*, at most [`DETAIL_PIXELS`]
//! and never more than the image has), and with *full resolution* on, the part on screen
//! decoded at the image's own resolution (its *crop*), which holds no more than the window.

use std::time::{Duration, Instant};

use crate::image::{Bitmap, Crop, Origin};

#[cfg(test)]
mod tests;

/// The most pixels the detail has (64 MB): a zoom that wants more enlarges it.
pub(crate) const DETAIL_PIXELS: f64 = 16_777_216.0;
/// The largest side of a decode ([`crate::image::MAX_SIDE`]).
const MAX_SIDE: f64 = crate::image::MAX_SIDE as f64;
/// The room around the image at Fit, in logical pixels.
const MARGIN: f64 = 16.0;
/// The most a zoom enlarges, of 100% (or of Fit, when that is more).
const MAX_ZOOM: f64 = 16.0;
/// A wheel notch's or a key's zoom step.
pub(crate) const STEP: f64 = 1.25;
/// A touchpad's zoom: by e for this many pixels of its scroll (a pinch is smooth).
const PIXELS_PER_E: f64 = 400.0;
/// An arrow key moves the image by this many logical pixels.
const LINE: f64 = 40.0;
/// How far a press may move, in logical pixels, and still be a click.
const SLOP: f64 = 4.0;
/// A press this soon after the one that opened the viewer, and near it, is the second click of
/// a double click on the image: it does nothing. (The document's double click, `MULTI_CLICK`.)
const DOUBLE: Duration = Duration::from_millis(500);
/// How long after the zoom or the image's place last changed a new decode is asked for.
pub(crate) const SETTLE: Duration = Duration::from_millis(150);
/// How long the zoom label shows after it changed.
pub(crate) const LABEL: Duration = Duration::from_millis(1200);
/// A decode that takes longer than this says so in the label until it is done.
pub(crate) const SLOW: Duration = Duration::from_millis(400);

/// Where the image is: its top left corner in the window, and window pixels per image pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transform {
    pub s: f64,
    pub x: f64,
    pub y: f64,
}

/// What a resize does: fit the image again, or keep its zoom and the point at the center.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Fit,
    Free,
}

/// A decode asked for and not back yet: its number, its size, its part, when, and whether the
/// label says "Loading…" for it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Asked {
    pub serial: u64,
    pub size: (u32, u32),
    pub crop: Option<Crop>,
    pub at: Instant,
    pub slow: bool,
}

/// A button of the viewer's: close, and full resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Button {
    Close,
    Full,
}

/// The mouse button is down: where it went down, where the image was then, whether it moved
/// past the click's slop (a drag moves the image), and whether it is the second click of the
/// double click that opened the viewer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Press {
    pub at: (f64, f64),
    pub from: (f64, f64),
    pub moved: bool,
    pub ignored: bool,
}

pub(crate) struct Viewer {
    /// The image (the document's id, which a reload changes) and where it comes from (which it
    /// doesn't: the viewer finds its image again by it), and its file's stamp when it was read.
    pub id: u32,
    pub origin: Option<Origin>,
    pub stamp: Option<crate::view::Stamp>,
    /// Its size as shown (EXIF orientation applied), and whether it is an SVG.
    pub natural: (u32, u32),
    pub vector: bool,
    /// The window and its scale.
    pub area: (f64, f64),
    pub hidpi: f64,
    pub mode: Mode,
    pub tf: Transform,
    /// The document's bitmap of the image, shown until the detail comes.
    pub preview: Option<Bitmap>,
    /// The whole image decoded for the viewer, and whether it is of the image as it is now
    /// (a reload that changed the file makes it stale: shown until the new one comes).
    pub detail: Option<Bitmap>,
    pub fresh: bool,
    /// Full resolution is on, and the part it decoded.
    pub full: bool,
    pub crop: Option<(Bitmap, Crop)>,
    pub asked: Option<Asked>,
    /// The last decode that made nothing (refused, or failed): not asked for again.
    pub failed: Option<((u32, u32), Option<Crop>)>,
    /// When the next decode is to be considered, and until when the label shows.
    pub settle_at: Option<Instant>,
    pub label_until: Option<Instant>,
    pub press: Option<Press>,
    /// The press that opened the viewer: when and where.
    pub opened: Option<(Instant, (f64, f64))>,
    /// The box the animation is made at, if the image is one: its own size (enlarged by the
    /// paint), or the size it opened at when the window is smaller.
    pub anim_box: Option<(u32, u32)>,
    /// The document's images it steps through, in order.
    pub list: Vec<u32>,
    /// Where the pointer is (`None`: not in the window), the button under it, and the buttons
    /// where they were last painted.
    pub pointer: Option<(f64, f64)>,
    pub hot: Option<Button>,
    pub buttons: Vec<(Button, crate::toolbar::Rect)>,
}

impl Viewer {
    /// The viewer of image `id`, of `natural` size, in a window of `area` at `hidpi`, at its
    /// opening zoom: Fit for an SVG, which stays sharp enlarged, else Fit or 100%, whichever is
    /// smaller (a small image isn't blown up).
    pub fn new(id: u32, natural: (u32, u32), vector: bool, area: (f64, f64), hidpi: f64) -> Viewer {
        let mut v = Viewer {
            id,
            origin: None,
            stamp: None,
            natural: (natural.0.max(1), natural.1.max(1)),
            vector,
            area: (area.0.max(1.0), area.1.max(1.0)),
            hidpi: sane(hidpi, 1.0),
            mode: Mode::Fit,
            tf: Transform {
                s: 1.0,
                x: 0.0,
                y: 0.0,
            },
            preview: None,
            detail: None,
            fresh: true,
            full: false,
            crop: None,
            asked: None,
            failed: None,
            settle_at: None,
            label_until: None,
            press: None,
            opened: None,
            anim_box: None,
            list: Vec::new(),
            pointer: None,
            hot: None,
            buttons: Vec::new(),
        };
        v.open_zoom();
        v
    }

    /// Back to the opening zoom, Fit, centered.
    pub fn open_zoom(&mut self) {
        self.tf.s = self.fit_zoom();
        self.mode = Mode::Fit;
        self.clamp();
    }

    /// `s` at 100%.
    pub fn hundred(&self) -> f64 {
        self.hidpi
    }

    /// `s` at Fit: the image as large as the window, less its margin, holds.
    pub fn fit(&self) -> f64 {
        let m = 2.0 * MARGIN * self.hidpi;
        let (aw, ah) = ((self.area.0 - m).max(1.0), (self.area.1 - m).max(1.0));
        let s = (aw / f64::from(self.natural.0)).min(ah / f64::from(self.natural.1));
        sane(s, 1.0)
    }

    /// The viewer's Fit, which it opens at: [`Viewer::fit`] for an SVG, which stays sharp
    /// enlarged, else no more than 100% (a small image isn't blown up).
    pub fn fit_zoom(&self) -> f64 {
        if self.vector {
            self.fit()
        } else {
            self.fit().min(self.hundred())
        }
    }

    /// The zooms there are: from the smaller of Fit and 100% to the larger of Fit and 16 × 100%.
    fn range(&self) -> (f64, f64) {
        let fit = self.fit();
        (fit.min(self.hundred()), fit.max(MAX_ZOOM * self.hundred()))
    }

    /// Whether the zoom is `s`.
    fn at(&self, s: f64) -> bool {
        (self.tf.s - s).abs() <= 1e-9 * s
    }

    /// Whether the image is at Fit.
    pub fn at_fit(&self) -> bool {
        self.at(self.fit_zoom())
    }

    /// The image's size in the window.
    pub fn shown(&self) -> (f64, f64) {
        (
            f64::from(self.natural.0) * self.tf.s,
            f64::from(self.natural.1) * self.tf.s,
        )
    }

    /// The image's box in the window: left, top, width, height.
    pub fn rect(&self) -> (i32, i32, u32, u32) {
        let (w, h) = self.shown();
        let side = |v: f64| v.round().clamp(1.0, f64::from(u32::MAX)) as u32;
        let at = |v: f64| v.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        (at(self.tf.x), at(self.tf.y), side(w), side(h))
    }

    /// Whether window point (`x`, `y`) is on the image.
    pub fn on_image(&self, x: f64, y: f64) -> bool {
        let (w, h) = self.shown();
        (self.tf.x..self.tf.x + w).contains(&x) && (self.tf.y..self.tf.y + h).contains(&y)
    }

    /// The image is wider than the window (`←` `→` move it rather than step to another image).
    pub fn wider(&self) -> bool {
        self.shown().0 > self.area.0 + 0.5
    }

    /// The image is larger than the window one way or the other: a drag moves it.
    pub fn pannable(&self) -> bool {
        let (w, h) = self.shown();
        w > self.area.0 + 0.5 || h > self.area.1 + 0.5
    }

    /// Keep the zoom within its range and the image in the window: centered along a side where
    /// it is smaller than the window, and with no gutter along one where it is larger.
    pub fn clamp(&mut self) {
        let (lo, hi) = self.range();
        self.tf.s = sane(self.tf.s, lo).clamp(lo, hi);
        let (w, h) = self.shown();
        let place = |pos: f64, size: f64, area: f64| {
            if size <= area {
                (area - size) / 2.0
            } else {
                sane(pos, 0.0).clamp(area - size, 0.0)
            }
        };
        self.tf.x = place(self.tf.x, w, self.area.0);
        self.tf.y = place(self.tf.y, h, self.area.1);
    }

    /// Zoom by `factor` about window point `p`: the image point under it stays under it.
    /// Returns true if the zoom changed.
    pub fn zoom_about(&mut self, p: (f64, f64), factor: f64) -> bool {
        if !factor.is_finite() || factor <= 0.0 || !p.0.is_finite() || !p.1.is_finite() {
            return false;
        }
        let (lo, hi) = self.range();
        let s = (self.tf.s * factor).clamp(lo, hi);
        if s == self.tf.s {
            return false;
        }
        let k = s / self.tf.s;
        self.tf = Transform {
            s,
            x: p.0 - (p.0 - self.tf.x) * k,
            y: p.1 - (p.1 - self.tf.y) * k,
        };
        self.mode = Mode::Free;
        self.clamp();
        true
    }

    /// Zoom by a touchpad's scroll of `pixels` (positive: in) about `p`.
    pub fn zoom_pixels(&mut self, p: (f64, f64), pixels: f64) -> bool {
        self.zoom_about(p, (pixels / PIXELS_PER_E).exp())
    }

    /// Zoom in or out a step (`steps` of them, negative: out) about the window's center.
    pub fn zoom_steps(&mut self, steps: f64) -> bool {
        let c = (self.area.0 / 2.0, self.area.1 / 2.0);
        self.zoom_about(c, STEP.powf(steps))
    }

    /// Fit ↔ 100%, about `p`: 100% from Fit, else back to Fit. Returns false at Fit if Fit is
    /// 100% (a raster no larger than the window): there is nowhere to go.
    pub fn toggle(&mut self, p: (f64, f64)) -> bool {
        let (fit, hundred) = (self.fit_zoom(), self.hundred());
        if !self.at_fit() {
            self.tf.s = fit;
            self.mode = Mode::Fit;
            self.clamp();
            return true;
        }
        if (fit - hundred).abs() <= 1e-9 * hundred {
            return false;
        }
        self.zoom_about(p, hundred / self.tf.s);
        true
    }

    /// Move the image by `dx`, `dy` window pixels. Returns true if it moved.
    pub fn pan(&mut self, dx: f64, dy: f64) -> bool {
        let before = self.tf;
        self.tf.x += sane(dx, 0.0);
        self.tf.y += sane(dy, 0.0);
        self.clamp();
        self.tf != before
    }

    /// Move the image by `lines` arrow-key steps across (`x`) and down (`y`): the view moves
    /// that way, so the image moves the other.
    pub fn pan_lines(&mut self, x: f64, y: f64) -> bool {
        let line = LINE * self.hidpi;
        self.pan(-x * line, -y * line)
    }

    /// The window is now `area` at `hidpi`. At Fit the image fits again; otherwise it keeps its
    /// zoom (relative to 100%: a new scale changes `s` with it) and the image point at the
    /// window's center.
    pub fn set_area(&mut self, area: (f64, f64), hidpi: f64) {
        let (area, hidpi) = ((area.0.max(1.0), area.1.max(1.0)), sane(hidpi, self.hidpi));
        let c = (
            (self.area.0 / 2.0 - self.tf.x) / self.tf.s,
            (self.area.1 / 2.0 - self.tf.y) / self.tf.s,
        );
        let s = self.tf.s * hidpi / self.hidpi;
        (self.area, self.hidpi) = (area, hidpi);
        if self.mode == Mode::Fit {
            self.tf.s = self.fit_zoom();
        } else {
            self.tf = Transform {
                s,
                x: area.0 / 2.0 - c.0 * s,
                y: area.1 / 2.0 - c.1 * s,
            };
        }
        self.clamp();
    }

    // ---- Decodes ------------------------------------------------------------------------

    /// The image's size at the zoom, as decoded: never more than its own (a raster's), its
    /// sides within a decode's. (So a forced image over [`MAX_SIDE`] a side is shown at most
    /// at that: a decode's box is clamped to it ([`crate::image::clamp_box`]), and a part is
    /// inside the box. Making it truly full would take a decode that isn't bound by it.)
    fn full_size(&self) -> (f64, f64) {
        let (w, h) = self.shown();
        let k = if self.vector {
            1.0
        } else {
            (f64::from(self.natural.0) / w).min(1.0)
        };
        let k = k.min(MAX_SIDE / w.max(1.0)).min(MAX_SIDE / h.max(1.0));
        ((w * k).max(1.0), (h * k).max(1.0))
    }

    /// The detail to decode at the zoom: [`Viewer::full_size`], within [`DETAIL_PIXELS`].
    pub fn target(&self) -> (u32, u32) {
        let (w, h) = self.full_size();
        let k = (DETAIL_PIXELS / (w * h)).sqrt().min(1.0);
        let side = |v: f64| (v * k).round().max(1.0) as u32;
        (side(w), side(h))
    }

    /// Whether a detail of `have` should be decoded again at `want`: it would be enlarged by
    /// more than a tenth, or shrunk by more than half as much again.
    pub fn redo(have: (u32, u32), want: (u32, u32)) -> bool {
        let side = |s: (u32, u32)| f64::from(s.0.max(s.1).max(1));
        let r = side(want) / side(have);
        !(1.0 / 1.5..=1.1).contains(&r)
    }

    /// Whether the image has more pixels than the detail holds (its zoomed size, for an SVG):
    /// the viewer offers its full resolution.
    pub fn over_cap(&self) -> bool {
        let (w, h) = if self.vector {
            self.shown()
        } else {
            (f64::from(self.natural.0), f64::from(self.natural.1))
        };
        w * h > DETAIL_PIXELS || w.max(h) > MAX_SIDE
    }

    /// The part of the image on screen at its full size, if full resolution is on and the
    /// detail doesn't hold it: the crop and its size. It holds no more pixels than the window.
    pub fn crop_wanted(&self) -> Option<(Crop, (u32, u32))> {
        if !self.full || !self.over_cap() {
            return None;
        }
        let (fw, fh) = self.full_size();
        let (w, h) = self.shown();
        if fw * fh <= DETAIL_PIXELS {
            return None;
        }
        // The window in the image at its full size, cut to the image.
        let span = |pos: f64, shown: f64, area: f64, full: f64| {
            let k = full / shown;
            let a = ((-pos) * k).floor().clamp(0.0, full - 1.0);
            let b = ((area - pos) * k).ceil().clamp(a + 1.0, full);
            (a as u32, (b - a) as u32)
        };
        let (x, cw) = span(self.tf.x, w, self.area.0, fw.round());
        let (y, ch) = span(self.tf.y, h, self.area.1, fh.round());
        let crop = Crop {
            full: (fw.round() as u32, fh.round() as u32),
            at: (x, y),
        };
        Some((crop, (cw, ch)))
    }

    /// What to decode next, if anything: the detail at the zoom's target, or with full
    /// resolution on and the detail there, the part on screen. Nothing while a decode runs for
    /// it already, nor what failed last.
    pub fn next_decode(&mut self) -> Option<((u32, u32), Option<Crop>)> {
        // The part held is dropped once none is wanted (zoomed out, or full resolution off):
        // it would be painted over the detail, and kept in memory.
        if self.crop.is_some() && self.crop_wanted().is_none() {
            self.crop = None;
        }
        let base = self.detail.is_some() || self.asked.is_some_and(|a| a.crop.is_some());
        let want = match self.crop_wanted() {
            Some((crop, size)) if base => {
                if self
                    .crop
                    .as_ref()
                    .is_some_and(|(b, c)| *c == crop && (b.width, b.height) == size)
                {
                    return None;
                }
                (size, Some(crop))
            }
            _ => {
                let target = self.target();
                let have = self.detail.as_ref().map(|b| (b.width, b.height));
                if self.fresh && have.is_some_and(|h| !Viewer::redo(h, target)) {
                    return None;
                }
                // (While the part is asked for, the detail there is good enough.)
                if self
                    .asked
                    .is_some_and(|a| a.crop.is_none() && !Viewer::redo(a.size, target))
                {
                    return None;
                }
                (target, None)
            }
        };
        if self.asked.is_some_and(|a| (a.size, a.crop) == want) || self.failed == Some(want) {
            return None;
        }
        Some(want)
    }

    /// The decode `want` was asked for at `now`, as decode `serial`.
    pub fn asked_for(&mut self, want: ((u32, u32), Option<Crop>), serial: u64, now: Instant) {
        self.asked = Some(Asked {
            serial,
            size: want.0,
            crop: want.1,
            at: now,
            slow: false,
        });
    }

    /// Decode `serial` came back with `bitmap`. Returns true if it is shown (repaint).
    pub fn took(&mut self, serial: u64, bitmap: Option<Bitmap>) -> bool {
        let Some(a) = self.asked.filter(|a| a.serial == serial) else {
            return false;
        };
        self.asked = None;
        let Some(b) = bitmap else {
            self.failed = Some((a.size, a.crop));
            // ("Loading…" goes.)
            return true;
        };
        match a.crop {
            // (A part the zoom no longer wants, because it was decoding as the window zoomed
            // out, is of no use.)
            Some(_) if self.crop_wanted().is_none() => {}
            Some(c) => self.crop = Some((b, c)),
            None => {
                self.detail = Some(b);
                self.fresh = true;
                self.preview = None;
            }
        }
        true
    }

    /// Full resolution on or off: off, its part goes.
    pub fn set_full(&mut self, on: bool) {
        self.full = on;
        if !on {
            self.crop = None;
            self.failed = None;
        }
    }

    // ---- Timing -------------------------------------------------------------------------

    /// The zoom or the image's place changed at `now`: the label shows, and a decode is
    /// considered once it settles.
    pub fn changed(&mut self, now: Instant) {
        self.settle_at = Some(now + SETTLE);
        self.label_until = Some(now + LABEL);
    }

    /// The image moved at `now` (the zoom didn't): only the part on screen depends on that.
    pub fn moved(&mut self, now: Instant) {
        if self.full {
            self.settle_at = Some(now + SETTLE);
        }
    }

    /// A decode runs for longer than [`SLOW`] by `now`: the label says "Loading…".
    pub fn loading(&self, now: Instant) -> bool {
        self.asked.is_some_and(|a| a.slow || now >= a.at + SLOW)
    }

    /// Whether the zoom label shows at `now`.
    pub fn label_shows(&self, now: Instant) -> bool {
        self.label_until.is_some_and(|t| now < t) || self.loading(now)
    }

    /// When the viewer next needs the event loop: a decode to consider, the label to hide, or
    /// "Loading…" to show. `None` once it has settled: no timer.
    pub fn deadline(&self) -> Option<Instant> {
        let slow = self.asked.filter(|a| !a.slow).map(|a| a.at + SLOW);
        [self.settle_at, self.label_until, slow]
            .into_iter()
            .flatten()
            .min()
    }

    /// What is due by `now` is done: the label hides, and whether a decode is to be considered
    /// (`.1`). `.0`: repaint.
    pub fn expire(&mut self, now: Instant) -> (bool, bool) {
        let settle = self.settle_at.is_some_and(|t| now >= t);
        if settle {
            self.settle_at = None;
        }
        let label = self.label_until.is_some_and(|t| now >= t);
        if label {
            self.label_until = None;
        }
        // ("Loading…" shows until the result comes, with no timer.)
        let mut slow = false;
        if let Some(a) = self.asked.as_mut()
            && !a.slow
            && now >= a.at + SLOW
        {
            (a.slow, slow) = (true, true);
        }
        (label || slow, settle)
    }

    /// The label: the zoom ("Fit", "250%"), the image's place among the document's, and for an
    /// animation what `Space` does, or "Loading…".
    pub fn label(&self, now: Instant, place: Option<(usize, usize)>, note: Option<&str>) -> String {
        if self.loading(now) {
            return "Loading…".into();
        }
        let mut label = if self.at(self.hundred()) {
            "100%".to_string()
        } else if self.at_fit() {
            "Fit".to_string()
        } else {
            format!("{}%", (self.tf.s / self.hundred() * 100.0).round())
        };
        if let Some((n, of)) = place.filter(|p| p.1 > 1) {
            label = format!("{label} · {n} / {of}");
        }
        match note {
            Some(note) => format!("{label} · {note}"),
            None => label,
        }
    }

    // ---- The mouse ----------------------------------------------------------------------

    /// The button went down at (`x`, `y`) at `now`.
    pub fn press(&mut self, x: f64, y: f64, now: Instant) {
        let ignored = self.opened.take().is_some_and(|(t, at)| {
            now.saturating_duration_since(t) <= DOUBLE
                && (x - at.0).hypot(y - at.1) <= SLOP * self.hidpi
        });
        self.press = Some(Press {
            at: (x, y),
            from: (self.tf.x, self.tf.y),
            moved: false,
            ignored,
        });
    }

    /// The pointer moved to (`x`, `y`) with the button down: past the slop, the image follows
    /// it. Returns true if the image moved.
    pub fn drag(&mut self, x: f64, y: f64) -> bool {
        let slop = SLOP * self.hidpi;
        let Some(p) = self.press.as_mut() else {
            return false;
        };
        let (dx, dy) = (x - p.at.0, y - p.at.1);
        if !p.moved && dx.hypot(dy) < slop {
            return false;
        }
        p.moved = true;
        let before = self.tf;
        let from = p.from;
        self.tf.x = from.0 + sane(dx, 0.0);
        self.tf.y = from.1 + sane(dy, 0.0);
        self.clamp();
        self.tf != before
    }

    /// Whether the image is being dragged.
    pub fn dragging(&self) -> bool {
        self.press.is_some_and(|p| p.moved)
    }
}

/// `v`, or `or` if it isn't a positive finite number (or, for an offset, finite).
fn sane(v: f64, or: f64) -> f64 {
    if v.is_finite() && (v > 0.0 || or <= 0.0) {
        v
    } else {
        or
    }
}
