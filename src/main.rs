//! VOMD app shell: command line, the startup sequence, and the winit
//! event loop. Everything it drives lives in the library ([`vomd::view::View`]); this file only
//! translates window events.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::cell::Cell;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Theme, Window, WindowId};

use vomd::cli::{self, Cli, Input};
use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::nav::{self, BackStack, Target};
use vomd::platform;
use vomd::settings::{self, FontPaths, Prefs, Store};
use vomd::text::fallback::{self, Family, Found};
use vomd::toolbar::Command;
use vomd::view::{self, Origin, Outcome, Stamp, View, Wake};

/// Idle layout runs in slices this long between events.
const IDLE_SLICE: Duration = Duration::from_millis(4);
const EMPTY_MESSAGE: &str = "Drop a Markdown file here";

enum UserEvent {
    /// A fallback font scan finished.
    Fonts(Vec<Found>),
    /// `font_sans` / `font_mono` families were looked up: (mono, family, faces if found).
    Families(Vec<(bool, String, Option<Family>)>),
    /// An image was decoded or fetched.
    Images,
}

/// The document as read and parsed off the main thread.
struct Loaded {
    doc: Doc,
    message: Option<String>,
    stamp: Option<Stamp>,
    /// The large-file notice.
    notice: Option<String>,
}

fn load(input: &Input, origin: Origin) -> Loaded {
    match view::read_input(input, origin) {
        Ok((text, stamp)) => Loaded {
            notice: view::large_notice(text.len()),
            doc: Doc::parse(&text),
            message: (*input == Input::None).then(|| EMPTY_MESSAGE.to_owned()),
            stamp,
        },
        Err(message) => {
            if platform::stderr_is_tty() {
                eprintln!("vomd: {message}");
            }
            Loaded {
                doc: Doc::default(),
                message: Some(message),
                stamp: None,
                notice: None,
            }
        }
    }
}

/// What `VOMD_BENCH` asks for (budget measurement).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bench {
    /// Print the time to the first frame and peak RSS, and exit (any value but `idle`).
    FirstFrame,
    /// `idle`: once the document is measured, its fonts are applied and nothing is left to do,
    /// print the time and RSS and keep running, for the budget harness to watch the CPU and end
    /// the process. Fallback font scans run as in a real window (they're part of its idle RSS);
    /// [`Bench::FirstFrame`] skips them.
    Idle,
}

struct Gfx {
    window: Arc<Window>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
}

struct App {
    start: Instant,
    bench: Option<Bench>,
    /// [`Bench::Idle`] reported the idle state.
    idle_reported: bool,
    input: Input,
    parse: Option<JoinHandle<Loaded>>,
    view: Option<View>,
    gfx: Option<Gfx>,
    proxy: EventLoopProxy<UserEvent>,
    /// A fallback font scan is running.
    scanning: bool,
    /// A redraw was asked for and not yet painted.
    redraw_pending: Cell<bool>,
    stamp: Option<Stamp>,
    store: Store,
    /// The `font_sans` and `font_mono` families applied to the view (`None`: embedded faces).
    families: [Option<String>; 2],
    /// Families (by `mono`) waiting for a lookup of their files.
    unresolved: Vec<bool>,
    resolving: bool,
    mods: ModifiersState,
    cursor: PhysicalPosition<f64>,
    /// The pointer is over the window (hover tracking).
    cursor_in: bool,
    /// The pointer cursor shows a hand (over a link).
    pointer: bool,
    back: BackStack,
    /// The toolbar's visibility as last applied from or saved to the settings.
    toolbar_shown: bool,
    shown: bool,
    exit_code: i32,
}

impl App {
    fn redraw(&self) {
        if let Some(g) = &self.gfx {
            self.redraw_pending.set(true);
            g.window.request_redraw();
        }
    }

    fn paint(&mut self, event_loop: &ActiveEventLoop) {
        self.redraw_pending.set(false);
        let (Some(gfx), Some(view)) = (self.gfx.as_mut(), self.view.as_mut()) else {
            return;
        };
        let size = gfx.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if gfx.surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = gfx.surface.buffer_mut() else {
            return;
        };
        // The Resized and ScaleFactorChanged handlers keep the view's size; this only covers a
        // frame painted before their event arrives (the buffer must match what is painted).
        if view.size() != (w.get(), h.get()) {
            view.resize(w.get(), h.get());
        }
        view.paint(&mut buf, w.get() as usize, Instant::now());
        if buf.present().is_err() {
            return;
        }
        if !self.shown {
            self.shown = true;
            gfx.window.set_visible(true);
            if self.bench == Some(Bench::FirstFrame) {
                eprintln!(
                    "vomd-bench first_frame_ms={:.2} rss_kb={}",
                    self.start.elapsed().as_secs_f64() * 1e3,
                    platform::peak_rss_kb()
                );
                event_loop.exit();
            }
        }
        self.scan_fonts();
        self.resolve_families();
    }

    /// Apply preferences from the settings file (at startup and when it changes). Font
    /// families with files cached for them load at once; others are looked up in the
    /// background after the first frame, the defaults standing in meanwhile.
    fn apply(&mut self, prefs: &Prefs) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        view.set_theme_mode(prefs.theme);
        view.set_zoom(prefs.zoom);
        view.set_remote_policy(prefs.remote_images);
        if view.toolbar_visible() != prefs.toolbar {
            view.set_toolbar(prefs.toolbar);
        }
        self.toolbar_shown = prefs.toolbar;
        view.set_text(prefs.font_size, prefs.line_width);
        for mono in [false, true] {
            let (family, cached) = if mono {
                (&prefs.font_mono, &prefs.font_mono_path)
            } else {
                (&prefs.font_sans, &prefs.font_sans_path)
            };
            let slot = &mut self.families[mono as usize];
            if same_family(family.as_deref(), slot.as_deref()) {
                continue;
            }
            *slot = family.clone();
            self.unresolved.retain(|&m| m != mono);
            match (family, cached) {
                (None, _) => {
                    view.set_family(mono, None);
                }
                (Some(_), Some(c)) if c.exists() && view.set_family(mono, Some(&c.faces)) => {}
                (Some(_), _) => {
                    // No usable cache: the defaults now, and look the family up.
                    view.set_family(mono, None);
                    self.unresolved.push(mono);
                }
            }
        }
        self.redraw();
    }

    /// Look up the files of `font_sans` / `font_mono` families on a background thread (one
    /// scan for both; never on the startup path).
    fn resolve_families(&mut self) {
        if self.resolving
            || self.bench == Some(Bench::FirstFrame)
            || !self.shown
            || self.unresolved.is_empty()
        {
            return;
        }
        let wanted: Vec<(bool, String)> = std::mem::take(&mut self.unresolved)
            .into_iter()
            .filter_map(|mono| Some((mono, self.families[mono as usize].clone()?)))
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.resolving = true;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let names: Vec<&str> = wanted.iter().map(|(_, f)| f.as_str()).collect();
            let found = fallback::find_families(&names);
            let results = wanted
                .into_iter()
                .zip(found)
                .map(|((mono, family), faces)| (mono, family, faces))
                .collect();
            let _ = proxy.send_event(UserEvent::Families(results));
        });
    }

    /// Save the view's theme and zoom if they changed (debounced).
    fn record(&mut self, now: Instant) {
        if let Some(view) = &self.view {
            self.store.record(view.theme_mode(), view.zoom(), now);
            let shown = view.toolbar_visible();
            if shown != self.toolbar_shown {
                self.toolbar_shown = shown;
                self.store
                    .set("toolbar", if shown { "true" } else { "false" }, now);
            }
        }
    }

    /// Run what a toolbar button asked for: the same actions as the keys.
    fn command(&mut self, command: Command, event_loop: &ActiveEventLoop, now: Instant) {
        match command {
            Command::Action(action) => {
                if let Some(view) = self.view.as_mut() {
                    let outcome = view.apply(action, now);
                    self.outcome(outcome, event_loop);
                }
                self.record(now);
            }
            Command::Settings => self.save(|s| s.open_in_editor()),
        }
    }

    /// Start a fallback font scan for characters no loaded face covers (one at a time, off the
    /// main thread). Characters found missing meanwhile wait for the next scan.
    fn scan_fonts(&mut self) {
        if self.scanning || self.bench == Some(Bench::FirstFrame) {
            return;
        }
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let missing = view.take_missing();
        if missing.is_empty() {
            return;
        }
        let sources = view.font_sources();
        let proxy = self.proxy.clone();
        self.scanning = true;
        std::thread::spawn(move || {
            let skip =
                |p: &std::path::Path, i: u32| sources.iter().any(|(sp, si)| sp == p && *si == i);
            let found = fallback::find_fallbacks(&missing, &skip);
            let _ = proxy.send_event(UserEvent::Fonts(found));
        });
    }

    /// Re-read the document, keeping the reading position.
    fn reload(&mut self) {
        // A pipe named on the command line was read once: keep what it gave.
        if !view::rereadable(&self.input) {
            return;
        }
        let loaded = load(&self.input, Origin::Navigation);
        self.stamp = loaded.stamp;
        if let Some(view) = self.view.as_mut() {
            view.replace(loaded.doc, loaded.message);
        }
        self.redraw();
    }

    /// Open a document in place of the current one (the back stack is the caller's business).
    fn open(&mut self, input: Input) {
        let loaded = load(&input, Origin::Navigation);
        self.input = input;
        self.stamp = loaded.stamp;
        if let Some(view) = self.view.as_mut() {
            view.open_in(loaded.doc, loaded.message, nav::base_dir(&self.input));
            view.set_notice(loaded.notice);
        }
        if let Some(g) = &self.gfx {
            g.window.set_title(&view::title(&self.input));
        }
        self.redraw();
    }

    /// Go to another document, leaving the current place on the back stack (see
    /// [`BackStack::leave`]).
    fn go_to(&mut self, input: Input) {
        if let Some(view) = self.view.as_ref() {
            let here = nav::Entry {
                input: self.input.clone(),
                anchor: view.anchor(),
            };
            self.back.leave(here, &input);
        }
        self.open(input);
    }

    /// Follow a clicked link: `#anchor` scrolls, a relative Markdown file opens in
    /// this window, `http(s)`/`mailto` go to the desktop, anything else does nothing. Places
    /// left are pushed on the back stack.
    fn follow(&mut self, href: &str, now: Instant) {
        let base = nav::base_dir(&self.input);
        match nav::classify(href, base.as_deref()) {
            Target::Anchor(fragment) => {
                let Some(view) = self.view.as_mut() else {
                    return;
                };
                let here = nav::Entry {
                    input: self.input.clone(),
                    anchor: view.anchor(),
                };
                if view.jump_to_anchor(&fragment, now) {
                    self.back.leave(here, &self.input);
                    self.redraw();
                }
            }
            Target::Doc { path, fragment } => {
                self.go_to(Input::File(path));
                if let (Some(view), Some(fragment)) = (self.view.as_mut(), fragment) {
                    view.jump_to_anchor(&fragment, now);
                }
            }
            Target::External(url) => {
                if let Err(e) = platform::open(std::ffi::OsStr::new(&url))
                    && platform::stderr_is_tty()
                {
                    eprintln!("vomd: cannot open {url}: {e}");
                }
            }
            Target::Ignore => {}
        }
    }

    /// Return to the last place on the back stack, reopening its file if it's another one.
    /// (Stdin is never on the stack as a place to reopen, see [`BackStack::leave`].)
    fn go_back(&mut self) {
        let Some(entry) = self.back.pop() else {
            return;
        };
        if entry.input != self.input {
            self.open(entry.input);
        }
        if let Some(view) = self.view.as_mut() {
            view.set_anchor(entry.anchor);
        }
        self.redraw();
    }

    /// Track the link under the pointer: the hand cursor and the URL preview follow it.
    fn update_hover(&mut self) {
        let (Some(view), Some(gfx)) = (self.view.as_mut(), self.gfx.as_ref()) else {
            return;
        };
        let changed = if self.cursor_in {
            view.hover(self.cursor.x, self.cursor.y)
        } else {
            view.hover_end()
        };
        let update = view::hover_change(self.pointer, view.hovered().is_some(), changed);
        if update.set_cursor {
            gfx.window.set_cursor(if update.pointer {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
        }
        self.pointer = update.pointer;
        if update.redraw {
            gfx.window.request_redraw();
        }
    }

    /// Run a settings save; a failure is reported (on a terminal) and otherwise ignored, since
    /// preferences are not worth interrupting reading for. Changes from outside that the save
    /// merged into the file are applied at once.
    fn save(&mut self, op: impl FnOnce(&mut Store) -> std::io::Result<()>) {
        if let Err(e) = op(&mut self.store)
            && platform::stderr_is_tty()
        {
            let path = self.store.path().map(|p| p.display().to_string());
            eprintln!("vomd: cannot save {}: {e}", path.unwrap_or_default());
        }
        if let Some(prefs) = self.store.take_merged() {
            self.apply(&prefs);
        }
    }

    fn outcome(&mut self, outcome: Outcome, event_loop: &ActiveEventLoop) {
        match outcome {
            Outcome::Nothing => {}
            Outcome::Redraw => self.redraw(),
            Outcome::Reload => self.reload(),
            Outcome::Back => self.go_back(),
            Outcome::Quit => event_loop.exit(),
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let prefs = self.store.prefs();
        let mut size = LogicalSize::new(f64::from(prefs.window.0), f64::from(prefs.window.1));
        let bench_size = self.bench.and(bench_size());
        if let Some((w, h)) = bench_size {
            size = LogicalSize::new(w, h);
        }
        // Clamp to the monitor where the system reports one (Wayland doesn't).
        if let Some(m) = event_loop
            .primary_monitor()
            .filter(|_| bench_size.is_none())
        {
            let avail = m.size().to_logical::<f64>(m.scale_factor());
            size.width = size.width.min(avail.width * 0.95);
            size.height = size.height.min(avail.height * 0.9);
        }
        let attrs = Window::default_attributes()
            .with_title(view::title(&self.input))
            .with_inner_size(size)
            .with_visible(false);
        // The Wayland app ID (and X11 WM_CLASS) names vomd.desktop, so the desktop shows its
        // icon for the window: Wayland has no default and would show a generic one.
        #[cfg(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        let attrs =
            winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, "vomd", "vomd");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("vomd: cannot create a window: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|cx| softbuffer::Surface::new(&cx, window.clone()));
        let surface = match surface {
            Ok(s) => s,
            Err(e) => {
                eprintln!("vomd: cannot draw to the window: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        let fonts = match Fonts::new() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("vomd: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        let loaded = match self.parse.take().map(JoinHandle::join) {
            Some(Ok(loaded)) => loaded,
            _ => Loaded {
                doc: Doc::default(),
                message: Some("Cannot read the document".into()),
                stamp: None,
                notice: None,
            },
        };
        self.stamp = loaded.stamp;
        let px = window.inner_size();
        let mut view = View::with_base(
            fonts,
            loaded.doc,
            loaded.message,
            (px.width, px.height),
            window.scale_factor(),
            nav::base_dir(&self.input),
        );
        view.set_notice(loaded.notice);
        view.set_settings_available(self.store.path().is_some());
        let proxy = Mutex::new(self.proxy.clone());
        view.set_image_wake(Arc::new(move || {
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(UserEvent::Images);
            }
        }));
        view.set_system_dark(match window.theme() {
            Some(t) => t == Theme::Dark,
            None => platform::fallback_dark(),
        });
        self.view = Some(view);
        self.gfx = Some(Gfx { window, surface });
        self.apply(&prefs);
        self.redraw();
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Fonts(found) => {
                self.scanning = false;
                if let Some(view) = self.view.as_mut()
                    && view.add_fallbacks(&found)
                {
                    self.redraw();
                }
                self.scan_fonts();
            }
            UserEvent::Families(results) => {
                self.resolving = false;
                let now = Instant::now();
                for (mono, family, faces) in results {
                    // Skip lookups the settings have moved on from.
                    if !same_family(Some(&family), self.families[mono as usize].as_deref()) {
                        continue;
                    }
                    let (Some(view), Some(faces)) = (self.view.as_mut(), faces) else {
                        continue;
                    };
                    let faces = faces.map(|f| f.map(|f| (f.path, f.index)));
                    if view.set_family(mono, Some(&faces)) {
                        self.store
                            .record_fonts(mono, &FontPaths { family, faces }, now);
                    }
                    self.redraw();
                }
                self.resolve_families();
            }
            UserEvent::Images => {
                if let Some(view) = self.view.as_mut()
                    && view.poll_images(Instant::now())
                {
                    self.redraw();
                }
            }
        }
        // New faces change line breaks and heights, and so what's under the pointer.
        self.update_hover();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let now = Instant::now();
        if let WindowEvent::RedrawRequested = event {
            self.paint(event_loop);
            return;
        }
        let Some(view) = self.view.as_mut() else {
            if let WindowEvent::CloseRequested = event {
                event_loop.exit();
            }
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                view.resize(size.width, size.height);
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                view.set_hidpi(scale_factor);
                self.redraw();
            }
            WindowEvent::ThemeChanged(theme) => {
                if view.set_system_dark(theme == Theme::Dark) {
                    self.redraw();
                }
            }
            // Changes made here are in the file before another window (or an editor) is used.
            WindowEvent::Focused(false) => self.save(|s| s.flush()),
            WindowEvent::Focused(true) => {
                // (`reload` keeps a pipe's document: its stamp says nothing.)
                if let Input::File(path) = &self.input
                    && view::stamp(path) != self.stamp
                {
                    self.reload();
                }
                // The same check for the settings file (live apply).
                if let Some(prefs) = self.store.refresh() {
                    self.apply(&prefs);
                }
            }
            WindowEvent::DroppedFile(path) => self.go_to(Input::File(path)),
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                if !(self.mods.control_key() || self.mods.super_key()) {
                    view.zoom_wheel_end();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let Some(action) = view::key_action(&event.logical_key, self.mods) {
                    let outcome = view.apply(action, now);
                    self.outcome(outcome, event_loop);
                    self.record(now);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let zoom = self.mods.control_key() || self.mods.super_key();
                match (delta, zoom) {
                    (MouseScrollDelta::LineDelta(_, y), false) => {
                        view.wheel_lines(-f64::from(y), now)
                    }
                    (MouseScrollDelta::PixelDelta(p), false) => view.scroll_by(-p.y, now),
                    (delta, true) => view.zoom_wheel(
                        match delta {
                            MouseScrollDelta::LineDelta(_, y) => f64::from(y) * view::ZOOM_PIXELS,
                            MouseScrollDelta::PixelDelta(p) => p.y,
                        },
                        now,
                    ),
                }
                self.redraw();
                self.record(now);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                self.cursor_in = true;
                if view.drag_to(position.y, now) {
                    self.redraw();
                }
            }
            WindowEvent::CursorLeft { .. } => self.cursor_in = false,
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = (self.cursor.x, self.cursor.y);
                let (changed, clicked, command) = match state {
                    ElementState::Pressed => (view.press(x, y, now), None, None),
                    ElementState::Released => {
                        let bar = view.release_toolbar(x, y);
                        (
                            view.release(now) | bar,
                            view.click(x, y),
                            view.take_command(),
                        )
                    }
                };
                if changed {
                    self.redraw();
                }
                if let Some(href) = clicked {
                    self.follow(&href, now);
                }
                if let Some(command) = command {
                    self.command(command, event_loop, now);
                }
            }
            _ => {}
        }
        // The toolbar's Back follows the back stack.
        if let Some(view) = self.view.as_mut()
            && view.set_can_back(!self.back.is_empty())
        {
            self.redraw();
        }
        // Scrolling, zooming, resizing or a new document move what's under the pointer.
        self.update_hover();
    }

    /// However the window goes (close button, quit keys), save its size and anything else
    /// waiting.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        // A maximized or fullscreen window's size isn't the one to come back to.
        if let Some(g) = &self.gfx
            && self.shown
            && !g.window.is_maximized()
            && g.window.fullscreen().is_none()
        {
            let size = g
                .window
                .inner_size()
                .to_logical::<f64>(g.window.scale_factor());
            let logical = (size.width.round() as u32, size.height.round() as u32);
            self.store.record_window(logical, Instant::now());
        }
        self.save(|s| s.flush());
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let now = Instant::now();
        // Idle layout starts once the first frame is out. The scrollbar's deadline is checked
        // on every turn, so it hides on time while measuring keeps the loop polling.
        let tick = view.tick(now, self.shown.then(|| now + IDLE_SLICE));
        if tick.redraw {
            self.redraw();
        }
        self.scan_fonts();
        self.save(|s| s.poll(now));
        let save = self.store.deadline();
        if self.bench == Some(Bench::Idle)
            && self.shown
            && !self.idle_reported
            && !matches!(tick.wake, Wake::Poll)
            && !self.scanning
            && !self.resolving
            && self.unresolved.is_empty()
            && !self.redraw_pending.get()
        {
            self.idle_reported = true;
            eprintln!(
                "vomd-bench idle_ms={:.2} rss_kb={} peak_rss_kb={}",
                self.start.elapsed().as_secs_f64() * 1e3,
                platform::rss_kb(),
                platform::peak_rss_kb()
            );
        }
        event_loop.set_control_flow(match (tick.wake, save) {
            (Wake::Poll, _) => ControlFlow::Poll,
            (Wake::At(t), Some(s)) => ControlFlow::WaitUntil(t.min(s)),
            (Wake::At(t), None) | (Wake::Event, Some(t)) => ControlFlow::WaitUntil(t),
            (Wake::Event, None) => ControlFlow::Wait,
        });
    }
}

fn main() {
    let start = Instant::now();
    platform::tune_allocator();
    platform::attach_console();
    let (input, no_settings) = match cli::parse(std::env::args_os().skip(1)) {
        Ok(Cli::Help) => {
            print!("{}", cli::USAGE);
            return;
        }
        Ok(Cli::Version) => {
            println!("vomd {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Ok(Cli::Run { input, no_settings }) => (input, no_settings),
        Err(e) => {
            eprintln!("vomd: {e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };
    let bench = std::env::var_os("VOMD_BENCH").map(|v| {
        if v == "idle" {
            Bench::Idle
        } else {
            Bench::FirstFrame
        }
    });
    // Settings come first: they size the window and set theme and zoom.
    let store = Store::open(if no_settings { None } else { settings::path() });
    // Read and parse in parallel with creating the window.
    let parse = {
        let input = input.clone();
        std::thread::spawn(move || load(&input, Origin::Argument))
    };
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("vomd: cannot connect to the display: {e}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        start,
        bench,
        idle_reported: false,
        input,
        parse: Some(parse),
        view: None,
        gfx: None,
        proxy: event_loop.create_proxy(),
        scanning: false,
        redraw_pending: Cell::new(false),
        stamp: None,
        store,
        families: [None, None],
        unresolved: Vec::new(),
        resolving: false,
        mods: ModifiersState::empty(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        cursor_in: false,
        pointer: false,
        back: BackStack::default(),
        toolbar_shown: true,
        shown: false,
        exit_code: 0,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("vomd: {e}");
        std::process::exit(1);
    }
    std::process::exit(app.exit_code);
}

/// `VOMD_BENCH_SIZE=WxH`: the window's size in logical pixels for a bench run, unclamped
/// (RSS is measured in a 1080p window).
fn bench_size() -> Option<(f64, f64)> {
    let v = std::env::var("VOMD_BENCH_SIZE").ok()?;
    let (w, h) = v.split_once('x')?;
    let (w, h): (f64, f64) = (w.parse().ok()?, h.parse().ok()?);
    (w >= 1.0 && h >= 1.0 && w <= 16384.0 && h <= 16384.0).then_some((w, h))
}

/// Family names in settings compare case-insensitively.
fn same_family(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::same_family;

    /// A family lookup in flight when the setting changes is ignored when it comes back: its
    /// result only counts while the family is still the wanted one (`Families` event).
    #[test]
    fn lookups_for_a_family_no_longer_wanted_are_ignored() {
        assert!(same_family(Some("Noto Sans"), Some(" noto sans ")));
        assert!(
            !same_family(Some("Noto Sans"), Some("Inter")),
            "changed meanwhile"
        );
        assert!(
            !same_family(Some("Noto Sans"), None),
            "back to the default meanwhile"
        );
        assert!(same_family(None, None));
    }
}
