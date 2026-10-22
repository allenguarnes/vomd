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
#[cfg(feature = "find")]
use winit::dpi::PhysicalSize;
use winit::dpi::{LogicalSize, PhysicalPosition};
#[cfg(feature = "find")]
use winit::event::Ime;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use winit::window::{CursorIcon, Theme, Window, WindowId};

use vomd::cli::{self, Cli, Input};
#[cfg(feature = "clipboard")]
use vomd::clipboard::{Clipboard, Which};
#[cfg(feature = "clipboard")]
use vomd::copy::Content;
use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::nav::{self, BackStack, Target};
use vomd::platform;
use vomd::settings::{self, FontPaths, Prefs, Store};
use vomd::text::fallback::{self, Family, Found};
use vomd::toolbar::Command;
#[cfg(feature = "clipboard")]
use vomd::view::CodeCopy;
use vomd::view::{self, Origin, Outcome, Stamp, View, Wake};

/// On Windows, a failed allocation in an SVG's process ends it with the code that says so (see
/// `image::svg_child`); elsewhere, and in VOMD's own process, the system's allocator as it is.
#[cfg(all(feature = "svg", windows))]
#[global_allocator]
static ALLOCATOR: vomd::image::SvgChildAllocator = vomd::image::SvgChildAllocator;

/// Idle layout runs in slices this long between events.
const IDLE_SLICE: Duration = Duration::from_millis(4);
/// What the empty window says. Dropping a file works on X11 and Windows (winit can't take drops
/// on Wayland), so it is only offered there.
fn empty_message(wayland: bool) -> String {
    let drop = if wayland || !cfg!(any(windows, all(unix, not(target_os = "macos")))) {
        ""
    } else {
        ", or drop one here"
    };
    format!("Press Ctrl+O to open a Markdown file{drop}")
}

/// End the event loop. Also wakes it: on Windows, winit only looks at the request after its next
/// message, and a window that failed to open (or a bench's first frame, painted before the window
/// shows) may never get one.
fn quit(event_loop: &ActiveEventLoop, proxy: &EventLoopProxy<UserEvent>) {
    event_loop.exit();
    let _ = proxy.send_event(UserEvent::Quit);
}

/// Whether the window system is Wayland, by the connection winit made (not by the environment,
/// which can name a Wayland socket that winit then didn't use).
fn is_wayland(display: Option<RawDisplayHandle>) -> bool {
    matches!(display, Some(RawDisplayHandle::Wayland(_)))
}

/// The portal's name for `window`, for its dialog to belong to (`x11:<id in hex>`). Empty on
/// Wayland: a parent there is a handle exported through xdg-foreign, which needs the Wayland
/// protocol crates (`wayland-client`, `wayland-protocols`), which VOMD links for the clipboard
/// only (and not in the smallest build); the dialog then has no parent.
fn portal_parent(window: &Window) -> String {
    parent_name(window.window_handle().map(|h| h.as_raw()).ok())
}

fn parent_name(handle: Option<RawWindowHandle>) -> String {
    match handle {
        Some(RawWindowHandle::Xlib(h)) => format!("x11:{:x}", h.window),
        Some(RawWindowHandle::Xcb(h)) => format!("x11:{:x}", h.window),
        Some(RawWindowHandle::Win32(h)) => format!("win32:{:x}", h.hwnd.get()),
        _ => String::new(),
    }
}

enum UserEvent {
    /// A fallback font scan finished.
    Fonts(Vec<Found>),
    /// `font_sans` / `font_mono` families were looked up: (mono, family, faces if found).
    Families(Vec<(bool, String, Option<Family>)>),
    /// An image was decoded or fetched.
    Images,
    /// The desktop's dark preference from the portal (`None`: none, or no portal).
    #[cfg_attr(
        not(all(unix, not(any(target_os = "macos", target_os = "ios")))),
        allow(dead_code)
    )]
    SystemDark(Option<bool>),
    /// The settings file couldn't be opened in an editor: why, for a toast.
    OpenFailed(String),
    /// Open's dialog closed: the file picked (`None`: nothing), or why there was no dialog.
    Chosen(Result<Option<std::path::PathBuf>, String>),
    /// A paste into the find bar's field was read: the text (`None`: there was none), and the
    /// ticket the paste was asked for with, which says whether the field is still the one.
    #[cfg(all(feature = "clipboard", feature = "find"))]
    Pasted(u32, Option<String>),
    /// Only wakes the loop after `quit` asked it to end (see there).
    Quit,
}

/// How long the first window waits for the portal's dark preference before painting with
/// what winit or `GTK_THEME` say (a later answer still applies). It usually takes ~1 ms.
const PORTAL_WAIT: Duration = Duration::from_millis(50);

/// The portal's first answer, set once by its thread (outer `None`: not yet), which then
/// unparks the main thread.
type FirstDark = Arc<Mutex<Option<Option<bool>>>>;

/// Ask the portal for the desktop's dark preference on a thread of its own and follow it: the
/// first answer is kept for the first frame and, like every change after it, sent to the
/// event loop.
fn watch_dark(proxy: EventLoopProxy<UserEvent>) -> Option<FirstDark> {
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
    {
        let first = FirstDark::default();
        let (theirs, main) = (first.clone(), std::thread::current());
        std::thread::spawn(move || {
            vomd::portal::watch(|dark| {
                if let Ok(mut slot) = theirs.lock()
                    && slot.is_none()
                {
                    *slot = Some(dark);
                    main.unpark();
                }
                let _ = proxy.send_event(UserEvent::SystemDark(dark));
            });
        });
        Some(first)
    }
    #[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
    {
        drop(proxy);
        None
    }
}

/// Whether the system theme is dark: the portal's preference if it has one, else
/// winit's theme, else what `GTK_THEME` says (`gtk`).
fn system_dark(portal: Option<bool>, winit: Option<Theme>, gtk: impl FnOnce() -> bool) -> bool {
    portal.unwrap_or_else(|| winit.map_or_else(gtk, |t| t == Theme::Dark))
}

/// The document as read and parsed off the main thread.
struct Loaded {
    doc: Doc,
    /// Why there's no document (an error). The empty window's invitation is [`App::message`]'s.
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
            message: None,
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

/// Make the toolbar's Back follow the back stack. The toolbar accepts clicks by this too, so
/// the back stack is only ever changed through [`leave`] and [`pop`], which call it: no frame
/// and no input sees a stale value, whatever event changed the stack (a window event, a
/// dialog's answer, a drop).
fn sync_back(view: &mut View, back: &BackStack) {
    view.set_can_back(!back.is_empty());
}

/// [`BackStack::leave`], then [`sync_back`].
fn leave(view: &mut View, back: &mut BackStack, here: nav::Entry, next: &Input) {
    back.leave(here, next);
    sync_back(view, back);
}

/// [`BackStack::pop`], then [`sync_back`].
fn pop(view: &mut View, back: &mut BackStack) -> Option<nav::Entry> {
    let entry = back.pop();
    sync_back(view, back);
    entry
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
    /// The portal's first dark preference, waited for by the first window.
    portal_first: Option<FirstDark>,
    /// The portal's dark preference, if it has one: it wins over winit's theme.
    portal: Option<bool>,
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
    /// Open's dialog is up (one at a time).
    choosing: bool,
    /// The window system is Wayland (known once the window exists).
    wayland: bool,
    mods: ModifiersState,
    cursor: PhysicalPosition<f64>,
    /// The pointer is over the window (hover tracking).
    cursor_in: bool,
    /// The pointer cursor shows a hand (over a link).
    pointer: bool,
    /// The pointer cursor shows the text cursor (over text, or while selecting).
    #[cfg(any(feature = "clipboard", feature = "find"))]
    text_cursor: bool,
    /// Where copies go (made with the window, which has the display).
    #[cfg(feature = "clipboard")]
    clipboard: Option<Clipboard>,
    /// A selection is offered as the primary selection (`primary_selection`).
    #[cfg(feature = "clipboard")]
    primary: bool,
    back: BackStack,
    /// The toolbar's visibility as last applied from or saved to the settings.
    toolbar_shown: bool,
    /// The wide layout as last applied from or saved to the settings.
    wide_set: bool,
    /// The contents panel as last applied from or saved to the settings.
    #[cfg(feature = "toc")]
    toc_set: bool,
    /// The window takes input method text, as it does while the find bar is open, with the area
    /// last given for its window.
    #[cfg(feature = "find")]
    ime: Option<(u32, u32, u32, u32)>,
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
        vomd::render::opaque(&mut buf);
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
                quit(event_loop, &self.proxy);
            }
        }
        self.scan_fonts();
        self.resolve_families();
        // The caret has moved when the query has.
        #[cfg(feature = "find")]
        self.sync_ime();
    }

    /// Apply preferences from the settings file (at startup and when it changes). Font
    /// families with files cached for them load at once; others are looked up in the
    /// background after the first frame, the defaults standing in meanwhile.
    fn apply(&mut self, prefs: &Prefs) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        view.set_theme_mode(prefs.theme);
        view.set_layout(prefs.zoom, prefs.font_size, prefs.line_width, prefs.wide);
        view.set_remote_policy(prefs.remote_images);
        #[cfg(feature = "clipboard")]
        {
            self.primary = prefs.primary_selection;
        }
        if view.toolbar_visible() != prefs.toolbar {
            view.set_toolbar(prefs.toolbar);
        }
        self.toolbar_shown = prefs.toolbar;
        self.wide_set = prefs.wide;
        #[cfg(feature = "toc")]
        {
            view.set_toc(prefs.toc);
            self.toc_set = prefs.toc;
        }
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
            let wide = view.wide();
            if wide != self.wide_set {
                self.wide_set = wide;
                self.store
                    .set("wide", if wide { "true" } else { "false" }, now);
            }
            #[cfg(feature = "toc")]
            {
                let toc = view.toc_open();
                if toc != self.toc_set {
                    self.toc_set = toc;
                    self.store
                        .set("toc", if toc { "true" } else { "false" }, now);
                }
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
            Command::Settings => self.open_settings(),
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

    /// What the window says in place of `input`'s document: `message` if there is one, else the
    /// invitation to open a file when there is no input.
    fn message(&self, input: &Input, message: Option<String>) -> Option<String> {
        message.or_else(|| (*input == Input::None).then(|| empty_message(self.wayland)))
    }

    /// Re-read the document, keeping the reading position.
    fn reload(&mut self) {
        // A pipe named on the command line was read once: keep what it gave.
        if !view::rereadable(&self.input) {
            return;
        }
        let loaded = load(&self.input, Origin::Navigation);
        self.stamp = loaded.stamp;
        let message = self.message(&self.input, loaded.message);
        if let Some(view) = self.view.as_mut() {
            view.replace(loaded.doc, message);
        }
        self.redraw();
    }

    /// Open a document in place of the current one (the back stack is the caller's business).
    fn open(&mut self, input: Input) {
        let loaded = load(&input, Origin::Navigation);
        let message = self.message(&input, loaded.message);
        self.input = input;
        self.stamp = loaded.stamp;
        if let Some(view) = self.view.as_mut() {
            view.open_in(loaded.doc, message, nav::base_dir(&self.input));
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
        if let Some(view) = self.view.as_mut() {
            let here = nav::Entry {
                input: self.input.clone(),
                anchor: view.anchor(),
            };
            leave(view, &mut self.back, here, &input);
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
                    leave(view, &mut self.back, here, &self.input);
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

    /// Scroll to a heading's block (a row of the contents panel), leaving the place on the back
    /// stack as following a link to its anchor does.
    #[cfg(feature = "toc")]
    fn jump_to(&mut self, block: u32, now: Instant) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let here = nav::Entry {
            input: self.input.clone(),
            anchor: view.anchor(),
        };
        view.jump_to_block(block, now);
        leave(view, &mut self.back, here, &self.input);
        self.redraw();
    }

    /// Return to the last place on the back stack, reopening its file if it's another one.
    /// (Stdin is never on the stack as a place to reopen, see [`BackStack::leave`].)
    fn go_back(&mut self) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let Some(entry) = pop(view, &mut self.back) else {
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

    /// Take text from an input method while the find bar's field has the keyboard, and not
    /// otherwise (so the window has no input method's cursor over the document), with its window
    /// by the caret.
    #[cfg(feature = "find")]
    fn sync_ime(&mut self) {
        let (Some(view), Some(gfx)) = (self.view.as_ref(), self.gfx.as_ref()) else {
            return;
        };
        let area = view.find_focused().then(|| view.find_ime_area());
        if let Some((x, y, w, h)) = area
            && self.ime != area
        {
            gfx.window
                .set_ime_cursor_area(PhysicalPosition::new(x, y), PhysicalSize::new(w, h));
        }
        let (was, open) = (self.ime.is_some(), area.is_some());
        self.ime = area;
        if was != open {
            gfx.window.set_ime_allowed(open);
        }
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
        // The hand is for what a click acts on: a link, a code block's copy button, an image to
        // load past the limits, a row of the contents panel.
        #[cfg(feature = "clipboard")]
        let hand = view.hovered().is_some() || view.over_copy() || view.over_force();
        #[cfg(not(feature = "clipboard"))]
        let hand = view.hovered().is_some() || view.over_force();
        #[cfg(feature = "toc")]
        let hand = hand || view.over_toc();
        let update = view::hover_change(self.pointer, hand, changed);
        #[cfg(any(feature = "clipboard", feature = "find"))]
        let text = self.cursor_in && view.text_cursor(self.cursor.x, self.cursor.y);
        #[cfg(any(feature = "clipboard", feature = "find"))]
        let set_cursor = update.set_cursor || text != self.text_cursor;
        #[cfg(not(any(feature = "clipboard", feature = "find")))]
        let set_cursor = update.set_cursor;
        if set_cursor {
            #[cfg(any(feature = "clipboard", feature = "find"))]
            let text_icon = text && !update.pointer;
            #[cfg(not(any(feature = "clipboard", feature = "find")))]
            let text_icon = false;
            gfx.window.set_cursor(if update.pointer {
                CursorIcon::Pointer
            } else if text_icon {
                CursorIcon::Text
            } else {
                CursorIcon::Default
            });
        }
        self.pointer = update.pointer;
        #[cfg(any(feature = "clipboard", feature = "find"))]
        {
            self.text_cursor = text;
        }
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

    /// The ⚙ action: the settings file (made from the template if missing, with what is pending
    /// saved) opened in the default editor off the main thread. A failure shows as a toast.
    fn open_settings(&mut self) {
        let file = self.store.editor_file();
        if let Some(prefs) = self.store.take_merged() {
            self.apply(&prefs);
        }
        match file {
            Ok(path) => {
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    if let Some(why) = open_in_editor(&path) {
                        let _ = proxy.send_event(UserEvent::OpenFailed(why));
                    }
                });
            }
            Err(e) => self.toast(format!("Can't open the settings file: {e}")),
        }
    }

    /// Open: ask for a file (off the main thread, starting in the document's folder), then go
    /// to it like a dropped one. A second request while the dialog is up says so.
    fn choose_file(&mut self) {
        if std::mem::replace(&mut self.choosing, true) {
            self.toast("A file dialog is already open".into());
            return;
        }
        let folder = nav::base_dir(&self.input).and_then(|d| std::path::absolute(d).ok());
        let parent = self.gfx.as_ref().map(|g| portal_parent(&g.window));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let chosen =
                platform::choose_file(parent.as_deref().unwrap_or_default(), folder.as_deref());
            let _ = proxy.send_event(UserEvent::Chosen(chosen));
        });
    }

    fn toast(&mut self, text: String) {
        if let Some(view) = self.view.as_mut() {
            view.show_toast(text, Instant::now());
            self.redraw();
        }
    }

    /// Offer the selection as the clipboard or the primary selection (which only if the setting
    /// allows). Nothing is copied now: a long selection is made when a program pastes it.
    #[cfg(feature = "clipboard")]
    fn offer(&mut self, which: Which) {
        if which == Which::Primary && !self.primary {
            return;
        }
        let content = self.view.as_ref().and_then(View::offered);
        self.offer_content(which, content);
    }

    /// Offer `content`, if there is any, as the clipboard or the primary selection.
    #[cfg(feature = "clipboard")]
    fn offer_content(&mut self, which: Which, content: Option<Content>) {
        if let (Some(content), Some(clipboard)) = (content, self.clipboard.as_mut()) {
            // (Without a clipboard a copy by key does nothing, as it always has; the copy button
            // says why.)
            let _ = clipboard.set(which, content);
        }
    }

    /// Paste a selection into the find bar's field: read it off the main thread, and the text
    /// comes back as [`UserEvent::Pasted`].
    #[cfg(all(feature = "clipboard", feature = "find"))]
    fn paste(&mut self, which: Which, ticket: u32) {
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        let proxy = self.proxy.clone();
        clipboard.read(which, move |text| {
            let _ = proxy.send_event(UserEvent::Pasted(ticket, text));
        });
    }

    /// A click on a code block's copy button: offer its code as the clipboard (never as PRIMARY,
    /// which is what was selected), and show the check mark. If there is no clipboard to offer
    /// it to, a toast says so instead.
    #[cfg(feature = "clipboard")]
    fn copy_code(&mut self, copy: CodeCopy, now: Instant) {
        let result = match self.clipboard.as_mut() {
            Some(clipboard) => clipboard.set(Which::Clipboard, copy.content),
            None => Err("no window system to copy to".into()),
        };
        match result {
            Ok(()) => {
                if let Some(view) = self.view.as_mut() {
                    view.code_copied(copy.block, now);
                }
                self.redraw();
            }
            Err(why) => self.toast(format!("Can't copy: {why}")),
        }
    }

    fn outcome(&mut self, outcome: Outcome, event_loop: &ActiveEventLoop) {
        match outcome {
            Outcome::Nothing => {}
            Outcome::Redraw => self.redraw(),
            Outcome::Reload => self.reload(),
            Outcome::Back => self.go_back(),
            Outcome::Open => self.choose_file(),
            #[cfg(feature = "clipboard")]
            Outcome::Selected => {
                self.redraw();
                self.offer(Which::Primary);
            }
            #[cfg(feature = "clipboard")]
            Outcome::Copy => self.offer(Which::Clipboard),
            #[cfg(all(feature = "clipboard", feature = "find"))]
            Outcome::Cut => {
                let cut = self.view.as_mut().and_then(View::take_cut);
                self.offer_content(Which::Clipboard, cut);
                self.redraw();
            }
            #[cfg(all(feature = "clipboard", feature = "find"))]
            Outcome::Paste(which) => {
                if let Some(ticket) = self.view.as_ref().map(View::find_ticket) {
                    self.paste(which, ticket);
                }
            }
            Outcome::Quit => quit(event_loop, &self.proxy),
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
        // The executable's icon (build.rs puts it in as resource 1), for the title bar and the
        // taskbar: winit gives a window none of its own.
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
            let icon = winit::window::Icon::from_resource(1, None).ok();
            attrs.with_window_icon(icon.clone()).with_taskbar_icon(icon)
        };
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                platform::fatal(&format!("vomd: cannot create a window: {e}"));
                self.exit_code = 1;
                quit(event_loop, &self.proxy);
                return;
            }
        };
        // Windows places a new window where it likes, which can put its bottom under the
        // taskbar: it is moved (or made smaller) to fit the work area before it is shown.
        #[cfg(windows)]
        if let (Some(area), Ok(pos)) = (platform::work_area(), window.outer_position()) {
            let (outer, inner) = (window.outer_size(), window.inner_size());
            let ((x, y), (w, h)) =
                platform::fit_in(area, (pos.x, pos.y), (outer.width, outer.height));
            if (w, h) != (outer.width, outer.height) {
                let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
                    inner.width.saturating_sub(outer.width - w),
                    inner.height.saturating_sub(outer.height - h),
                ));
            }
            window.set_outer_position(PhysicalPosition::new(x, y));
        }
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|cx| softbuffer::Surface::new(&cx, window.clone()));
        let surface = match surface {
            Ok(s) => s,
            Err(e) => {
                platform::fatal(&format!("vomd: cannot draw to the window: {e}"));
                self.exit_code = 1;
                quit(event_loop, &self.proxy);
                return;
            }
        };
        let fonts = match Fonts::new() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("vomd: {e}");
                self.exit_code = 1;
                quit(event_loop, &self.proxy);
                return;
            }
        };
        self.wayland = is_wayland(window.display_handle().ok().map(|h| h.as_raw()));
        #[cfg(feature = "clipboard")]
        {
            self.clipboard = window
                .display_handle()
                .ok()
                .map(|h| Clipboard::new(h.as_raw()));
        }
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
        let message = self.message(&self.input, loaded.message);
        let px = window.inner_size();
        let mut view = View::with_base(
            fonts,
            loaded.doc,
            message,
            (px.width, px.height),
            window.scale_factor(),
            nav::base_dir(&self.input),
        );
        view.set_notice(loaded.notice);
        sync_back(&mut view, &self.back);
        view.set_settings_available(self.store.path().is_some());
        let proxy = Mutex::new(self.proxy.clone());
        view.set_image_wake(Arc::new(move || {
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(UserEvent::Images);
            }
        }));
        let portal = self.portal_first.take().and_then(|first| {
            let until = Instant::now() + PORTAL_WAIT;
            loop {
                if let Some(dark) = *first.lock().ok()? {
                    break dark;
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break None;
                }
                std::thread::park_timeout(left);
            }
        });
        self.portal = portal;
        view.set_system_dark(system_dark(portal, window.theme(), platform::fallback_dark));
        self.view = Some(view);
        self.gfx = Some(Gfx { window, surface });
        self.apply(&prefs);
        // The window shows once its first frame is painted (see `paint`). Windows never sends a
        // hidden window the paint a redraw asks for, so there the first frame is painted now.
        #[cfg(windows)]
        self.paint(event_loop);
        #[cfg(not(windows))]
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
            UserEvent::Quit => {}
            UserEvent::Images => {
                if let Some(view) = self.view.as_mut()
                    && view.poll_images(Instant::now())
                {
                    self.redraw();
                }
            }
            UserEvent::OpenFailed(why) => self.toast(why),
            UserEvent::Chosen(chosen) => {
                self.choosing = false;
                match chosen {
                    Ok(Some(path)) => self.go_to(Input::File(path)),
                    Ok(None) => {}
                    Err(why) => self.toast(format!("Can't open a file: {why}")),
                }
            }
            #[cfg(all(feature = "clipboard", feature = "find"))]
            UserEvent::Pasted(ticket, text) => {
                let pasted = text.as_deref().zip(self.view.as_mut());
                if pasted.is_some_and(|(text, view)| view.find_paste(ticket, text)) {
                    self.redraw();
                }
                self.sync_ime();
            }
            UserEvent::SystemDark(dark) => {
                self.portal = dark;
                if let (Some(view), Some(gfx)) = (self.view.as_mut(), self.gfx.as_ref())
                    && view.set_system_dark(system_dark(
                        dark,
                        gfx.window.theme(),
                        platform::fallback_dark,
                    ))
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
                quit(event_loop, &self.proxy);
            }
            return;
        };
        match event {
            WindowEvent::CloseRequested => quit(event_loop, &self.proxy),
            WindowEvent::Resized(size) => {
                view.resize(size.width, size.height);
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                view.set_hidpi(scale_factor);
                self.redraw();
            }
            WindowEvent::ThemeChanged(theme) => {
                if view.set_system_dark(system_dark(
                    self.portal,
                    Some(theme),
                    platform::fallback_dark,
                )) {
                    self.redraw();
                }
            }
            // Changes made here are in the file before another window (or an editor) is used.
            WindowEvent::Focused(false) => {
                // A button released elsewhere isn't seen: the drag ends here.
                #[cfg(feature = "clipboard")]
                {
                    view.release_select();
                    view.cancel_copy();
                }
                #[cfg(feature = "find")]
                view.release_find_drag();
                self.save(|s| s.flush());
            }
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
                #[cfg(any(feature = "clipboard", feature = "find"))]
                view.set_shift(self.mods.shift_key());
                view.set_ctrl(self.mods.control_key());
                if !(self.mods.control_key() || self.mods.super_key()) {
                    view.zoom_wheel_end();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // The find bar, when open, has the keys that are text first.
                #[cfg(feature = "find")]
                let typed =
                    view.find_key(&event.logical_key, event.text.as_deref(), self.mods, now);
                #[cfg(not(feature = "find"))]
                let typed: Option<Outcome> = None;
                let outcome = typed.or_else(|| {
                    let action = view::key_action(&event.logical_key, self.mods)?;
                    Some(view.apply(action, now))
                });
                if let Some(outcome) = outcome {
                    self.outcome(outcome, event_loop);
                    self.record(now);
                }
            }
            // What an input method committed goes to the find bar's query.
            #[cfg(feature = "find")]
            WindowEvent::Ime(Ime::Commit(text)) => {
                view.find_type(&text);
                self.redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let zoom = self.mods.control_key() || self.mods.super_key();
                match (delta, zoom) {
                    (MouseScrollDelta::LineDelta(_, y), false) => {
                        view.wheel_lines(-f64::from(y), now)
                    }
                    (MouseScrollDelta::PixelDelta(p), false) => view.wheel_by(-p.y, now),
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
                let moved = view.drag_to(position.y, now);
                #[cfg(feature = "clipboard")]
                let moved = view.select_to(position.x, position.y, now) | moved;
                #[cfg(feature = "find")]
                let moved = view.find_drag_to(position.x, now) | moved;
                if moved {
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
                #[cfg(feature = "clipboard")]
                let mut offer_primary = false;
                #[cfg(feature = "clipboard")]
                let mut code = None;
                let (changed, clicked, command) = match state {
                    ElementState::Pressed => (view.press(x, y, now), None, None),
                    ElementState::Released => {
                        let bar = view.release_toolbar(x, y);
                        #[cfg(feature = "clipboard")]
                        {
                            offer_primary = view.release_select();
                            code = view.release_copy(x, y);
                        }
                        // A selection made in the find bar's field is offered too.
                        #[cfg(all(feature = "clipboard", feature = "find"))]
                        {
                            offer_primary |= view.release_find_drag();
                        }
                        #[cfg(all(feature = "find", not(feature = "clipboard")))]
                        view.release_find_drag();
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
                #[cfg(feature = "toc")]
                if let Some(block) = self.view.as_mut().and_then(View::take_jump) {
                    self.jump_to(block, now);
                }
                if let Some(command) = command {
                    self.command(command, event_loop, now);
                }
                // A selection made with the mouse is done when the button is up.
                #[cfg(feature = "clipboard")]
                if offer_primary {
                    self.offer(Which::Primary);
                }
                #[cfg(feature = "clipboard")]
                if let Some(copy) = code {
                    self.copy_code(copy, now);
                }
                // A click can close the contents panel (a row, or the document it covers).
                self.record(now);
            }
            // A middle click on the find bar's field pastes the primary selection there.
            #[cfg(all(feature = "clipboard", feature = "find"))]
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Middle,
                ..
            } => {
                if let Some(ticket) = view.find_middle(self.cursor.x, self.cursor.y) {
                    self.redraw();
                    self.paste(Which::Primary, ticket);
                }
            }
            _ => {}
        }
        // Scrolling, zooming, resizing or a new document move what's under the pointer.
        self.update_hover();
        #[cfg(feature = "find")]
        self.sync_ime();
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
        #[cfg(feature = "clipboard")]
        if let Some(clipboard) = self.clipboard.as_ref() {
            clipboard.save();
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
    // Started by VOMD to draw an SVG (`image::svg_child`): that, and nothing else.
    #[cfg(all(feature = "svg", any(target_os = "linux", windows)))]
    if let Some(code) = vomd::image::svg_child() {
        std::process::exit(code);
    }
    let start = Instant::now();
    platform::tune_allocator();
    // SVGs are drawn by this same binary, in processes that can be killed.
    #[cfg(all(feature = "svg", target_os = "linux"))]
    vomd::image::set_svg_program("/proc/self/exe".into());
    #[cfg(all(feature = "svg", windows))]
    if let Ok(exe) = std::env::current_exe() {
        vomd::image::set_svg_program(exe);
    }
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
            platform::fatal(&format!("vomd: {e}\n\n{}", cli::USAGE));
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
            platform::fatal(&format!("vomd: cannot connect to the display: {e}"));
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
        portal_first: watch_dark(event_loop.create_proxy()),
        portal: None,
        scanning: false,
        redraw_pending: Cell::new(false),
        stamp: None,
        store,
        families: [None, None],
        unresolved: Vec::new(),
        resolving: false,
        choosing: false,
        wayland: false,
        mods: ModifiersState::empty(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        cursor_in: false,
        pointer: false,
        #[cfg(any(feature = "clipboard", feature = "find"))]
        text_cursor: false,
        #[cfg(feature = "clipboard")]
        clipboard: None,
        #[cfg(feature = "clipboard")]
        primary: false,
        back: BackStack::default(),
        toolbar_shown: true,
        wide_set: false,
        #[cfg(feature = "toc")]
        toc_set: false,
        #[cfg(feature = "find")]
        ime: None,
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
/// Open `path` in the default editor and wait for the launcher (run off the main thread).
/// Returns why it failed, for a toast (see `platform::open_file`).
fn open_in_editor(path: &std::path::Path) -> Option<String> {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    platform::open_file(path)
        .err()
        .map(|e| format!("Can't open {name}: {e}"))
}

fn same_family(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{empty_message, is_wayland, leave, parent_name, pop, same_family, system_dark};
    use std::ptr::NonNull;
    use std::time::Instant;
    use vomd::cli::Input;
    use vomd::doc::Doc;
    use vomd::fonts::Fonts;
    use vomd::nav::{BackStack, Entry};
    use vomd::toolbar::Command;
    use vomd::view::{Action, View};
    use winit::raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
        XcbDisplayHandle, XcbWindowHandle, XlibDisplayHandle, XlibWindowHandle,
    };
    use winit::window::Theme;

    #[test]
    fn the_empty_window_offers_a_drop_where_winit_takes_one() {
        assert_eq!(
            empty_message(false),
            if cfg!(any(windows, all(unix, not(target_os = "macos")))) {
                "Press Ctrl+O to open a Markdown file, or drop one here"
            } else {
                "Press Ctrl+O to open a Markdown file"
            }
        );
        assert_eq!(empty_message(true), "Press Ctrl+O to open a Markdown file");
    }

    /// Wayland is what the display handle says, whatever the environment names.
    #[test]
    fn wayland_is_decided_by_the_display_handle() {
        let wayland = WaylandDisplayHandle::new(NonNull::dangling());
        assert!(is_wayland(Some(RawDisplayHandle::Wayland(wayland))));
        let xlib = RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0));
        assert!(!is_wayland(Some(xlib)));
        let xcb = RawDisplayHandle::Xcb(XcbDisplayHandle::new(None, 0));
        assert!(!is_wayland(Some(xcb)));
        assert!(!is_wayland(None));
    }

    #[test]
    fn the_portal_parent_is_an_x11_window_id_in_hex() {
        let xlib = RawWindowHandle::Xlib(XlibWindowHandle::new(0x4a0_0007));
        assert_eq!(parent_name(Some(xlib)), "x11:4a00007");
        let id = std::num::NonZeroU32::new(0xff).unwrap();
        assert_eq!(
            parent_name(Some(RawWindowHandle::Xcb(XcbWindowHandle::new(id)))),
            "x11:ff"
        );
        // Wayland has no parent to give, and no handle gives none.
        let wayland = WaylandWindowHandle::new(NonNull::dangling());
        assert_eq!(parent_name(Some(RawWindowHandle::Wayland(wayland))), "");
        assert_eq!(parent_name(None), "");
        // A Win32 window is its HWND, also in hex.
        let hwnd = std::num::NonZeroIsize::new(0x1_02a4).unwrap();
        let win32 = RawWindowHandle::Win32(winit::raw_window_handle::Win32WindowHandle::new(hwnd));
        assert_eq!(parent_name(Some(win32)), "win32:102a4");
    }

    /// Open's answer (or a link, or a drop) pushes the old document on the back stack from a
    /// user event, and a Back click may follow before any frame is painted: the toolbar must
    /// accept it, and the next frame shows it enabled. (Likewise disabled once the stack is
    /// empty again.)
    #[test]
    fn back_follows_the_stack_the_moment_it_changes() {
        let (w, h) = (300_u32, 200_u32);
        let mut view = View::new(Fonts::new().unwrap(), Doc::parse("Hi"), None, (w, h), 1.0);
        view.set_toolbar(true);
        let mut buf = vec![0; (w * h) as usize];
        let mut back = BackStack::default();
        let back_asks = |view: &mut View| {
            let now = Instant::now();
            view.press(20.0, 16.0, now);
            view.release(now);
            view.release_toolbar(20.0, 16.0);
            view.take_command() == Some(Command::Action(Action::Back))
        };
        view.paint(&mut buf, w as usize, Instant::now());
        assert!(!back_asks(&mut view), "nothing to go back to");
        let here = Entry {
            input: Input::File("a.md".into()),
            anchor: view.anchor(),
        };
        leave(&mut view, &mut back, here, &Input::File("b.md".into()));
        assert!(
            back_asks(&mut view),
            "navigated: Back is enabled, unpainted"
        );
        assert!(pop(&mut view, &mut back).is_some());
        assert!(!back_asks(&mut view), "stack empty again");
    }

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

    /// The portal's preference wins while it has one and gives way when it is withdrawn
    /// (GNOME's light style is `color-scheme` 0): the theme then follows winit, then `GTK_THEME`.
    #[test]
    fn the_portal_wins_only_while_it_has_a_preference() {
        let gtk_dark = || true;
        let gtk_light = || false;
        // Dark Style on, then off: winit's theme (Wayland: none) and GTK_THEME decide again.
        assert!(system_dark(Some(true), None, gtk_light));
        assert!(!system_dark(None, None, gtk_light));
        assert!(system_dark(None, None, gtk_dark));
        assert!(!system_dark(None, Some(Theme::Light), gtk_dark));
        // Light preference, then dark.
        assert!(!system_dark(Some(false), Some(Theme::Dark), gtk_dark));
        assert!(system_dark(Some(true), Some(Theme::Light), gtk_light));
        // ThemeChanged: ignored with a portal preference, followed without one.
        assert!(!system_dark(Some(false), Some(Theme::Dark), gtk_light));
        assert!(system_dark(None, Some(Theme::Dark), gtk_light));
        assert!(!system_dark(None, Some(Theme::Light), gtk_dark));
    }
}
