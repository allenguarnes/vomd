//! VOMD app shell: command line, the startup sequence, and the winit
//! event loop. Everything it drives lives in the library ([`vomd::view::View`]); this file only
//! translates window events.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::num::NonZeroU32;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Theme, Window, WindowId};

use vomd::cli::{self, Cli, Input};
use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::platform;
use vomd::text::fallback::{self, Found};
use vomd::view::{self, Outcome, Stamp, View, Wake};

/// Default window size in logical pixels (P6 makes it the last-used size).
const WINDOW_SIZE: (f64, f64) = (900.0, 1000.0);
/// Idle layout runs in slices this long between events.
const IDLE_SLICE: Duration = Duration::from_millis(4);
/// Ctrl + trackpad scroll zooms one step per this many pixels.
const ZOOM_PIXELS: f64 = 50.0;
const EMPTY_MESSAGE: &str = "Drop a Markdown file here";

enum UserEvent {
    /// A fallback font scan finished.
    Fonts(Vec<Found>),
}

/// The document as read and parsed off the main thread.
struct Loaded {
    doc: Doc,
    message: Option<String>,
    stamp: Option<Stamp>,
}

fn load(input: &Input) -> Loaded {
    match view::read_input(input) {
        Ok((text, stamp)) => Loaded {
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
            }
        }
    }
}

struct Gfx {
    window: Arc<Window>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
}

struct App {
    start: Instant,
    bench: bool,
    input: Input,
    parse: Option<JoinHandle<Loaded>>,
    view: Option<View>,
    gfx: Option<Gfx>,
    proxy: EventLoopProxy<UserEvent>,
    /// A fallback font scan is running.
    scanning: bool,
    stamp: Option<Stamp>,
    mods: ModifiersState,
    cursor: PhysicalPosition<f64>,
    /// Ctrl + trackpad scroll not yet turned into zoom steps.
    zoom_pixels: f64,
    shown: bool,
    exit_code: i32,
}

impl App {
    fn redraw(&self) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn paint(&mut self, event_loop: &ActiveEventLoop) {
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
            if self.bench {
                eprintln!(
                    "vomd-bench first_frame_ms={:.2} rss_kb={}",
                    self.start.elapsed().as_secs_f64() * 1e3,
                    platform::peak_rss_kb()
                );
                event_loop.exit();
            }
        }
        self.scan_fonts();
    }

    /// Start a fallback font scan for characters no loaded face covers (one at a time, off the
    /// main thread). Characters found missing meanwhile wait for the next scan.
    fn scan_fonts(&mut self) {
        if self.scanning || self.bench {
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
        if !matches!(self.input, Input::File(_)) {
            return;
        }
        let loaded = load(&self.input);
        self.stamp = loaded.stamp;
        if let Some(view) = self.view.as_mut() {
            view.replace(loaded.doc, loaded.message);
        }
        self.redraw();
    }

    /// Open a dropped file in place of the current document.
    fn open(&mut self, input: Input) {
        let loaded = load(&input);
        self.input = input;
        self.stamp = loaded.stamp;
        if let Some(view) = self.view.as_mut() {
            view.open(loaded.doc, loaded.message);
        }
        if let Some(g) = &self.gfx {
            g.window.set_title(&view::title(&self.input));
        }
        self.redraw();
    }

    fn outcome(&mut self, outcome: Outcome, event_loop: &ActiveEventLoop) {
        match outcome {
            Outcome::Nothing => {}
            Outcome::Redraw => self.redraw(),
            Outcome::Reload => self.reload(),
            Outcome::Quit => event_loop.exit(),
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let mut size = LogicalSize::new(WINDOW_SIZE.0, WINDOW_SIZE.1);
        // Clamp to the monitor where the system reports one (Wayland doesn't).
        if let Some(m) = event_loop.primary_monitor() {
            let avail = m.size().to_logical::<f64>(m.scale_factor());
            size.width = size.width.min(avail.width * 0.95);
            size.height = size.height.min(avail.height * 0.9);
        }
        let attrs = Window::default_attributes()
            .with_title(view::title(&self.input))
            .with_inner_size(size)
            .with_visible(false);
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
            },
        };
        self.stamp = loaded.stamp;
        let px = window.inner_size();
        let mut view = View::new(
            fonts,
            loaded.doc,
            loaded.message,
            (px.width, px.height),
            window.scale_factor(),
        );
        view.set_system_dark(match window.theme() {
            Some(t) => t == Theme::Dark,
            None => platform::fallback_dark(),
        });
        self.view = Some(view);
        self.gfx = Some(Gfx { window, surface });
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
        }
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
            WindowEvent::Focused(true) => {
                if let Input::File(path) = &self.input
                    && view::stamp(path) != self.stamp
                {
                    self.reload();
                }
            }
            WindowEvent::DroppedFile(path) => self.open(Input::File(path)),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let Some(action) = view::key_action(&event.logical_key, self.mods) {
                    let outcome = view.apply(action, now);
                    self.outcome(outcome, event_loop);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let zoom = self.mods.control_key() || self.mods.super_key();
                match (delta, zoom) {
                    (MouseScrollDelta::LineDelta(_, y), false) => {
                        view.wheel_lines(-f64::from(y), now)
                    }
                    (MouseScrollDelta::PixelDelta(p), false) => view.scroll_by(-p.y, now),
                    (delta, true) => {
                        self.zoom_pixels += match delta {
                            MouseScrollDelta::LineDelta(_, y) => f64::from(y) * ZOOM_PIXELS,
                            MouseScrollDelta::PixelDelta(p) => p.y,
                        };
                        while self.zoom_pixels.abs() >= ZOOM_PIXELS {
                            let up = self.zoom_pixels > 0.0;
                            self.zoom_pixels -= ZOOM_PIXELS.copysign(self.zoom_pixels);
                            let action = if up {
                                view::Action::ZoomIn
                            } else {
                                view::Action::ZoomOut
                            };
                            view.apply(action, now);
                        }
                    }
                }
                self.redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                if view.drag_to(position.y, now) {
                    self.redraw();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let changed = match state {
                    ElementState::Pressed => view.press(self.cursor.x, self.cursor.y, now),
                    ElementState::Released => view.release(now),
                };
                if changed {
                    self.redraw();
                }
            }
            _ => {}
        }
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
        event_loop.set_control_flow(match tick.wake {
            Wake::Poll => ControlFlow::Poll,
            Wake::At(t) => ControlFlow::WaitUntil(t),
            Wake::Event => ControlFlow::Wait,
        });
    }
}

fn main() {
    let start = Instant::now();
    platform::attach_console();
    let input = match cli::parse(std::env::args_os().skip(1)) {
        Ok(Cli::Help) => {
            print!("{}", cli::USAGE);
            return;
        }
        Ok(Cli::Version) => {
            println!("vomd {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        // `--no-settings` matters once there are settings (P6).
        Ok(Cli::Run { input, .. }) => input,
        Err(e) => {
            eprintln!("vomd: {e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };
    let bench = std::env::var_os("VOMD_BENCH").is_some();
    // Read and parse in parallel with creating the window.
    let parse = {
        let input = input.clone();
        std::thread::spawn(move || load(&input))
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
        input,
        parse: Some(parse),
        view: None,
        gfx: None,
        proxy: event_loop.create_proxy(),
        scanning: false,
        stamp: None,
        mods: ModifiersState::empty(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        zoom_pixels: 0.0,
        shown: false,
        exit_code: 0,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("vomd: {e}");
        std::process::exit(1);
    }
    std::process::exit(app.exit_code);
}
