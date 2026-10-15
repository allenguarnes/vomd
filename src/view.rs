//! The viewer: one document, its layout and the reading position, driven by the app
//! shell's events. The only window-system types here are winit's key values, so all of it runs
//! headless in tests; `main.rs` turns window events into calls on [`View`].
//!
//! The reading position is an [`Anchor`] (a block and a fraction of its height), not a pixel
//! offset: block heights change as idle layout measures them, and on resize, zoom and reload,
//! and the anchor keeps the same text at the top of the window through all of it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::cli::Input;
use crate::doc::Doc;
use crate::fonts::Fonts;
use crate::layout::{Anchor, BlockBox, Layout, Params};
use crate::render::{self, Clip, Frame, ScrollbarFade, Viewport};
use crate::text::cache::GlyphCache;
use crate::text::fallback::Found;
use crate::text::{Align, Para, TextCtx, TextStyle};
use crate::theme::{DARK, LIGHT, Role, Theme};

/// Zoom limits and step, in percent.
pub const ZOOM_MIN: u16 = 50;
pub const ZOOM_MAX: u16 = 300;
pub const ZOOM_STEP: u16 = 10;
/// Space above and below the document, in em (GitHub pads its pages about this much).
const PAGE_PAD: f32 = 2.0;
/// Lines scrolled per wheel notch.
const WHEEL_LINES: f64 = 3.0;
/// Passes [`View::bottom`] makes: measuring the last screen changes the total height.
const BOTTOM_PASSES: usize = 4;

/// What the user asked for. Keys of later phases (toolbar, links, images, outline)
/// aren't mapped yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Scroll by this many lines (negative = up).
    Lines(i32),
    /// Scroll by this many pages.
    Pages(i32),
    Top,
    Bottom,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Reload,
    CycleTheme,
    Quit,
}

/// What the shell must do after [`View::apply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Redraw,
    /// Re-read the document ([`read_input`]) and pass it to [`View::replace`].
    Reload,
    Quit,
}

/// When the event loop should run again after [`View::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// Straight away: idle layout has more to measure.
    Poll,
    /// At this instant, to hide the scrollbar.
    At(Instant),
    /// Only on the next event.
    Event,
}

/// The result of [`View::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// The window needs a repaint.
    pub redraw: bool,
    pub wake: Wake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    /// Follow the OS.
    System,
    Light,
    Dark,
}

impl ThemeMode {
    /// `T` cycles System → Light → Dark.
    pub fn next(self) -> ThemeMode {
        match self {
            ThemeMode::System => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::System,
        }
    }
}

/// The action for a key press. Plain keys only act without Ctrl, Alt or Cmd, so shortcuts of
/// the desktop (or of later phases) don't scroll by accident.
pub fn key_action(key: &Key, mods: ModifiersState) -> Option<Action> {
    let command = mods.control_key() || mods.super_key();
    if command {
        return match key {
            Key::Character(c) => match c.as_str() {
                "=" | "+" => Some(Action::ZoomIn),
                "-" | "_" => Some(Action::ZoomOut),
                "0" => Some(Action::ZoomReset),
                "w" | "W" => Some(Action::Quit),
                _ => None,
            },
            _ => None,
        };
    }
    if mods.alt_key() {
        return None;
    }
    match key {
        Key::Named(named) => match named {
            NamedKey::ArrowDown => Some(Action::Lines(1)),
            NamedKey::ArrowUp => Some(Action::Lines(-1)),
            NamedKey::PageDown => Some(Action::Pages(1)),
            NamedKey::PageUp => Some(Action::Pages(-1)),
            NamedKey::Space if mods.shift_key() => Some(Action::Pages(-1)),
            NamedKey::Space => Some(Action::Pages(1)),
            NamedKey::Home => Some(Action::Top),
            NamedKey::End => Some(Action::Bottom),
            NamedKey::F5 => Some(Action::Reload),
            NamedKey::Escape => Some(Action::Quit),
            _ => None,
        },
        Key::Character(c) => match c.as_str() {
            "j" | "J" => Some(Action::Lines(1)),
            "k" | "K" => Some(Action::Lines(-1)),
            " " if mods.shift_key() => Some(Action::Pages(-1)),
            " " => Some(Action::Pages(1)),
            // Shift decides, not the case of the character: CapsLock turns `g` into "G", and
            // some layouts report Shift+g as "g" (or as "G" with Shift set).
            "g" | "G" if mods.shift_key() => Some(Action::Bottom),
            "g" | "G" => Some(Action::Top),
            "r" | "R" => Some(Action::Reload),
            "t" | "T" => Some(Action::CycleTheme),
            "q" | "Q" => Some(Action::Quit),
            _ => None,
        },
        _ => None,
    }
}

/// A file's modification time and size when it was read, for auto-reload on focus.
pub type Stamp = (Option<SystemTime>, u64);

/// The stamp of `path` now (`None` if it can't be read).
pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

/// Read the document text. Invalid UTF-8 is replaced, never fatal. Errors come back
/// as the message to show in the window.
pub fn read_input(input: &Input) -> Result<(String, Option<Stamp>), String> {
    let lossy = |bytes: Vec<u8>| match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    match input {
        Input::None => Ok((String::new(), None)),
        Input::Stdin => {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut bytes)
                .map_err(|e| format!("Cannot read standard input: {e}"))?;
            Ok((lossy(bytes), None))
        }
        Input::File(path) => {
            // Stamp first: a write landing between the two makes the next focus reload again
            // rather than miss the change.
            let st = stamp(path);
            let bytes =
                std::fs::read(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
            Ok((lossy(bytes), st))
        }
    }
}

/// The window title for an input: the file name, or `vomd`.
pub fn title(input: &Input) -> String {
    match input {
        Input::File(path) => path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        Input::Stdin => "vomd — stdin".into(),
        Input::None => "vomd".into(),
    }
}

/// A scrollbar drag in progress: where on the thumb it was grabbed.
#[derive(Clone, Copy, Debug)]
struct Drag {
    grab: f64,
}

pub struct View {
    doc: Doc,
    layout: Layout,
    fonts: Fonts,
    cache: GlyphCache,
    anchor: Anchor,
    /// Shown centered instead of the document (read errors, the empty window).
    message: Option<String>,
    msg_ctx: TextCtx,
    width: u32,
    height: u32,
    /// Height of the strip above the document area (the toolbar of P9), in physical pixels.
    top_inset: u32,
    /// Physical pixels per logical pixel from the window system.
    hidpi: f64,
    zoom: u16,
    theme: ThemeMode,
    system_dark: bool,
    fade: ScrollbarFade,
    drag: Option<Drag>,
    /// Characters already handed to a fallback scan, over all documents shown (the
    /// system fonts are scanned once per character, not once per reload).
    tried: HashSet<char>,
}

impl View {
    /// A view of `doc`, or of `message` in place of a document, in a `width` × `height` window.
    pub fn new(
        fonts: Fonts,
        doc: Doc,
        message: Option<String>,
        (width, height): (u32, u32),
        hidpi: f64,
    ) -> View {
        let hidpi = sane_scale(hidpi);
        let params = params(width, hidpi, 100);
        View {
            layout: Layout::new(&doc, params),
            doc,
            fonts,
            cache: GlyphCache::default(),
            anchor: Anchor::TOP,
            message,
            msg_ctx: TextCtx::default(),
            width,
            height,
            top_inset: 0,
            hidpi,
            zoom: 100,
            theme: ThemeMode::System,
            system_dark: false,
            fade: ScrollbarFade::default(),
            drag: None,
            tried: HashSet::new(),
        }
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    pub fn zoom(&self) -> u16 {
        self.zoom
    }

    pub fn theme_mode(&self) -> ThemeMode {
        self.theme
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Physical pixels per logical pixel, zoom included.
    pub fn scale(&self) -> f32 {
        (self.hidpi * f64::from(self.zoom) / 100.0) as f32
    }

    pub fn dark(&self) -> bool {
        match self.theme {
            ThemeMode::System => self.system_dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        }
    }

    fn palette(&self) -> &'static Theme {
        if self.dark() { &DARK } else { &LIGHT }
    }

    /// The OS theme changed (or was first read). Returns true if the colors on screen change.
    pub fn set_system_dark(&mut self, dark: bool) -> bool {
        let before = self.dark();
        self.system_dark = dark;
        self.dark() != before
    }

    pub fn set_theme_mode(&mut self, mode: ThemeMode) {
        self.theme = mode;
    }

    pub fn set_zoom(&mut self, zoom: u16) {
        self.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.relayout();
    }

    /// The window size the view lays out and paints for (physical pixels).
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reserve `px` pixels at the top of the window for something else (the toolbar, P9): the
    /// document area, its scrolling, pages and scrollbar start below it.
    pub fn set_top_inset(&mut self, px: u32) {
        self.top_inset = px.min(self.height);
        self.clamp_scroll();
    }

    pub fn top_inset(&self) -> u32 {
        self.top_inset
    }

    /// The document area's height.
    fn area_height(&self) -> u32 {
        self.height.saturating_sub(self.top_inset)
    }

    /// The window was resized (physical pixels).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.top_inset = self.top_inset.min(height);
        self.relayout();
        self.clamp_scroll();
    }

    pub fn set_hidpi(&mut self, hidpi: f64) {
        self.hidpi = sane_scale(hidpi);
        self.relayout();
    }

    /// New layout parameters; the anchor keeps the reading position.
    fn relayout(&mut self) {
        self.layout
            .set_params(params(self.width, self.hidpi, self.zoom));
    }

    // ---- Scrolling ------------------------------------------------------------------------

    /// Document y at the top of the window.
    pub fn scroll(&self) -> f64 {
        self.layout.y_exact(self.anchor)
    }

    pub fn max_scroll(&self) -> f64 {
        (self.layout.top(self.doc.blocks.len()) - f64::from(self.area_height())).max(0.0)
    }

    pub fn set_scroll(&mut self, y: f64) {
        let y = if y.is_nan() { 0.0 } else { y };
        self.anchor = self.layout.anchor_exact(y.clamp(0.0, self.max_scroll()));
    }

    fn clamp_scroll(&mut self) {
        if self.scroll() > self.max_scroll() {
            self.set_scroll(self.max_scroll());
        }
    }

    pub fn scroll_by(&mut self, dy: f64, now: Instant) {
        self.set_scroll(self.scroll() + dy);
        self.fade.poke(now);
    }

    /// One line of body text, in physical pixels.
    fn line(&self) -> f64 {
        let p = self.layout.params();
        f64::from(p.font_size * p.scale) * 1.5
    }

    /// One page: the window less a line, so the last line stays in view.
    fn page(&self) -> f64 {
        (f64::from(self.area_height()) - self.line()).max(self.line())
    }

    /// Wheel notches (line deltas, positive = down).
    pub fn wheel_lines(&mut self, lines: f64, now: Instant) {
        self.scroll_by(lines * WHEEL_LINES * self.line(), now);
    }

    /// Jump to the end: measuring the last screen changes the total, so settle in a few passes.
    fn bottom(&mut self) {
        for _ in 0..BOTTOM_PASSES {
            self.set_scroll(f64::INFINITY);
            let before = self.max_scroll();
            self.ensure();
            if (self.max_scroll() - before).abs() < 0.5 {
                break;
            }
        }
        self.set_scroll(f64::INFINITY);
    }

    pub fn apply(&mut self, action: Action, now: Instant) -> Outcome {
        match action {
            Action::Lines(n) => self.scroll_by(f64::from(n) * self.line(), now),
            Action::Pages(n) => self.scroll_by(f64::from(n) * self.page(), now),
            Action::Top => {
                self.anchor = Anchor::TOP;
                self.fade.poke(now);
            }
            Action::Bottom => {
                self.bottom();
                self.fade.poke(now);
            }
            Action::ZoomIn => self.set_zoom(self.zoom.saturating_add(ZOOM_STEP)),
            Action::ZoomOut => self.set_zoom(self.zoom.saturating_sub(ZOOM_STEP)),
            Action::ZoomReset => self.set_zoom(100),
            Action::CycleTheme => self.theme = self.theme.next(),
            Action::Reload => return Outcome::Reload,
            Action::Quit => return Outcome::Quit,
        }
        Outcome::Redraw
    }

    // ---- Scrollbar drag -------------------------------------------------------------------

    fn viewport(&self) -> Viewport {
        Viewport {
            area: Clip {
                x0: 0,
                y0: self.top_inset.min(i32::MAX as u32) as i32,
                x1: self.width.min(i32::MAX as u32) as i32,
                y1: self.height.min(i32::MAX as u32) as i32,
            },
            scroll: self.scroll(),
        }
    }

    /// Thumb geometry at document y `scroll`.
    fn thumb_at(&self, scroll: f64) -> Option<render::Thumb> {
        let vp = Viewport {
            scroll,
            ..self.viewport()
        };
        render::thumb(vp, self.layout.total_height(), self.scale())
    }

    /// Mouse button pressed at (`x`, `y`). On the thumb it starts a drag, visible or not; on the
    /// rest of the scrollbar's track it centers the thumb there first, but only while the
    /// scrollbar is showing, so links and other things under a hidden one stay clickable.
    /// Returns true if it took the press.
    pub fn press(&mut self, x: f64, y: f64, now: Instant) -> bool {
        let Some(t) = self.thumb_at(self.scroll()) else {
            return false;
        };
        // The column is the thumb and its margin to the right edge, a few pixels wider to the
        // left so the thin thumb is easy to hit.
        let slack = f64::from(t.x1 - t.x0);
        if x < f64::from(t.x0) - slack || y < f64::from(self.top_inset) {
            return false;
        }
        let (y0, y1) = (f64::from(t.y0), f64::from(t.y1));
        let on_thumb = (y0..y1).contains(&y);
        if !on_thumb && !self.fade.visible(now) {
            return false;
        }
        let grab = if on_thumb { y - y0 } else { (y1 - y0) / 2.0 };
        self.drag = Some(Drag { grab });
        self.drag_to(y, now);
        true
    }

    /// The pointer moved to `y` during a drag. Returns true if the view scrolled.
    pub fn drag_to(&mut self, y: f64, now: Instant) -> bool {
        let Some(drag) = self.drag else {
            return false;
        };
        let max = self.max_scroll();
        let (Some(first), Some(last)) = (self.thumb_at(0.0), self.thumb_at(max)) else {
            return false;
        };
        let travel = f64::from(last.y0 - first.y0);
        let frac = if travel > 0.0 {
            (y - drag.grab - f64::from(first.y0)) / travel
        } else {
            0.0
        };
        let before = self.anchor;
        self.set_scroll(frac.clamp(0.0, 1.0) * max);
        self.fade.poke(now);
        self.anchor != before
    }

    /// The mouse button was released. Returns true if a drag ended.
    pub fn release(&mut self, now: Instant) -> bool {
        if self.drag.take().is_some() {
            self.fade.poke(now);
            return true;
        }
        false
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    // ---- Layout and painting --------------------------------------------------------------

    /// Lay out the blocks around the reading position, keeping it inside the document.
    pub fn ensure(&mut self) {
        let h = self.area_height() as f32;
        self.layout.ensure(&self.doc, &self.fonts, self.anchor, h);
        if self.scroll() > self.max_scroll() + 0.5 {
            // Exact heights made the document shorter than the position: settle at the end.
            self.set_scroll(self.max_scroll());
            self.layout.ensure(&self.doc, &self.fonts, self.anchor, h);
        }
    }

    /// Measure more blocks until `deadline`. Returns true when the whole document is measured.
    pub fn idle(&mut self, deadline: Instant) -> bool {
        self.layout.idle(&self.doc, &self.fonts, deadline)
    }

    /// Paint the window into `buf` (`stride` pixels per row).
    pub fn paint(&mut self, buf: &mut [u32], stride: usize, now: Instant) {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut frame = Frame::new(buf, w, h, stride);
        let theme = self.palette();
        if let Some(text) = self.message.clone() {
            self.paint_message(&mut frame, &text, theme);
            return;
        }
        self.ensure();
        let vp = self.viewport();
        // Whatever uses the inset paints over this.
        frame.fill_rect(0, 0, w as i32, vp.area.y0, theme.bg);
        render::paint(
            &mut frame,
            vp,
            &self.layout,
            &self.fonts,
            &mut self.cache,
            theme,
        );
        if self.fade.visible(now) || self.drag.is_some() {
            let total = self.layout.total_height();
            render::paint_scrollbar(&mut frame, vp, total, self.scale(), theme);
        }
    }

    /// One centered paragraph of muted text.
    fn paint_message(&mut self, frame: &mut Frame, text: &str, theme: &Theme) {
        let (w, h) = (frame.width() as f32, frame.height() as f32);
        frame.fill_rect(0, 0, w as i32, h as i32, theme.bg);
        let scale = self.scale();
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: 16.0 * scale,
            role: Role::Muted,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let mut para = Para::default();
        para.push_text(text, 0, style);
        let max_w = (w - 64.0 * scale).max(16.0 * scale);
        let pl = self
            .msg_ctx
            .layout(&self.fonts, &para, max_w, Align::Center, 1.5);
        let oy = ((h - pl.height) / 2.0).max(0.0);
        let content = BlockBox::from_para(pl, &para.styles);
        let ox = (w - max_w) / 2.0;
        render::paint_block(frame, &content, ox, oy, &self.fonts, &mut self.cache, theme);
    }

    // ---- Scrollbar timer ------------------------------------------------------------------

    /// When the event loop must wake for the scrollbar (`None`: never).
    pub fn deadline(&self) -> Option<Instant> {
        if self.drag.is_some() {
            return None;
        }
        self.fade.deadline()
    }

    /// Hide the scrollbar if its time is up. Returns true if the window needs a repaint.
    pub fn expire(&mut self, now: Instant) -> bool {
        self.drag.is_none() && self.fade.expire(now)
    }

    /// What the event loop does each time it runs out of events: hide the scrollbar if its
    /// time is up (whatever woke the loop, and however busy measuring keeps it), then measure
    /// until `measure_until` (`None`: not yet), and say when to run again.
    pub fn tick(&mut self, now: Instant, measure_until: Option<Instant>) -> Tick {
        let redraw = self.expire(now);
        let measured = measure_until.is_none_or(|t| self.idle(t));
        // No timers when there's nothing to do.
        let wake = match (measured, self.deadline()) {
            (false, _) => Wake::Poll,
            (true, Some(t)) => Wake::At(t),
            (true, None) => Wake::Event,
        };
        Tick { redraw, wake }
    }

    // ---- Fonts ----------------------------------------------------------------------------

    /// Characters no loaded face covers that the shell hasn't looked for yet. Each is returned
    /// once per process, however often the document is reloaded: a character that no
    /// installed font has would otherwise rescan them all on every reload. (A character
    /// first seen in a later edit does get a scan of its own.)
    pub fn take_missing(&mut self) -> Vec<char> {
        let mut missing = self.layout.take_missing();
        missing.retain(|&c| self.tried.insert(c));
        missing
    }

    /// Faces already loaded from disk, for a fallback scan to skip.
    pub fn font_sources(&self) -> Vec<(PathBuf, u32)> {
        self.fonts.sources()
    }

    /// Add the faces a fallback scan found and relayout what they cover. Returns true if any
    /// face was added (the window needs a repaint).
    pub fn add_fallbacks(&mut self, found: &[Found]) -> bool {
        let mut added = false;
        for f in found {
            added |= self.fonts.add_fallback(&f.path, f.index).is_some();
        }
        if added {
            self.layout.invalidate_missing(&self.doc);
        }
        added
    }

    // ---- Documents ------------------------------------------------------------------------

    /// Show a different document (or a message), from the top.
    pub fn open(&mut self, doc: Doc, message: Option<String>) {
        self.doc = doc;
        self.message = message;
        self.layout = Layout::new(&self.doc, params(self.width, self.hidpi, self.zoom));
        self.anchor = Anchor::TOP;
        self.drag = None;
    }

    /// Show a new version of the document (reload) or a message in its place. The position
    /// follows the nearest heading at or above it, so edits above it don't move the text being
    /// read; without one (or if the heading is gone) it keeps the block index.
    pub fn replace(&mut self, doc: Doc, message: Option<String>) {
        let place = self.place();
        let old = self.anchor;
        self.doc = doc;
        self.message = message;
        self.layout = Layout::new(&self.doc, params(self.width, self.hidpi, self.zoom));
        self.anchor = Anchor::TOP;
        let n = self.doc.blocks.len();
        if n == 0 {
            return;
        }
        let found = place.and_then(|p| {
            let h = self.doc.headings.iter().find(|h| h.slug == p.slug)?;
            // The same block offset below the heading, not the same pixels: the blocks in
            // between are only estimates until measured, so a pixel offset lands elsewhere
            // even when nothing changed. A section that got shorter clamps to its last block.
            let last = section_end(&self.doc, h.block) - 1;
            Some(Anchor {
                block: (h.block + p.offset).min(last),
                frac: p.frac,
            })
        });
        self.anchor = found.unwrap_or(Anchor {
            block: old.block.min(n as u32 - 1),
            frac: old.frac,
        });
        // Measures around the position, and settles at the end if the new text is shorter.
        self.ensure();
    }

    /// Where the reading position is relative to the heading at or above it.
    fn place(&self) -> Option<Place> {
        if self.message.is_some() {
            return None;
        }
        let at = self.anchor.block;
        let h = self
            .doc
            .headings
            .iter()
            .filter(|h| h.block <= at)
            .max_by_key(|h| h.block)?;
        Some(Place {
            slug: h.slug.clone(),
            offset: at - h.block,
            frac: self.anchor.frac,
        })
    }
}

/// The reading position as a heading (by slug), blocks below it, and a fraction of that block.
struct Place {
    slug: String,
    offset: u32,
    frac: f32,
}

/// The block after the section that starts at heading block `from`: the next heading's, or the
/// end of the document.
fn section_end(doc: &Doc, from: u32) -> u32 {
    doc.headings
        .iter()
        .map(|h| h.block)
        .filter(|&b| b > from)
        .min()
        .unwrap_or(doc.blocks.len() as u32)
}

/// Layout parameters for a window `width` wide at `hidpi` and `zoom` percent.
fn params(width: u32, hidpi: f64, zoom: u16) -> Params {
    Params {
        width: width as f32,
        scale: (hidpi * f64::from(zoom) / 100.0) as f32,
        page_pad: PAGE_PAD,
        ..Params::default()
    }
}

/// The window system's scale factor, guarded: winit promises a positive, finite one.
fn sane_scale(s: f64) -> f64 {
    if s.is_finite() && s > 0.0 {
        s.clamp(0.25, 8.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests;
