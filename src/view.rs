//! The viewer: one document, its layout and the reading position, driven by the app
//! shell's events. The only window-system types here are winit's key values, so all of it runs
//! headless in tests; `main.rs` turns window events into calls on [`View`].
//!
//! The reading position is an [`Anchor`] (a block and a fraction of its height), not a pixel
//! offset: block heights change as idle layout measures them, and on resize, zoom and reload,
//! and the anchor keeps the same text at the top of the window through all of it.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
#[cfg(feature = "clipboard")]
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use unicode_bidi::{BidiClass, bidi_class};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::cli::Input;
use crate::doc::{Doc, Heading};
use crate::fonts::Fonts;
use crate::image::{Images, Wake as ImageWake};
use crate::layout::{Anchor, BlockBox, Layout, Params};
use crate::platform;
use crate::render::{self, Clip, Frame, ScrollbarFade, Viewport};
use crate::settings::RemoteImages;
use crate::text::cache::GlyphCache;
use crate::text::fallback::Found;
use crate::text::{Align, Para, ParaLayout, TextCtx, TextStyle};
use crate::theme::{DARK, LIGHT, Role, Theme};
use crate::toolbar::{self, Toolbar};

#[cfg(feature = "clipboard")]
mod copybtn;
#[cfg(feature = "find")]
mod find;
#[cfg(feature = "clipboard")]
mod gesture;
#[cfg(feature = "toc")]
mod toc;

#[cfg(feature = "clipboard")]
pub use copybtn::CodeCopy;

/// Zoom limits and step, in percent.
pub const ZOOM_MIN: u16 = 50;
pub const ZOOM_MAX: u16 = 300;
pub const ZOOM_STEP: u16 = 10;
/// Font size of the link preview, in logical pixels.
const PREVIEW_SIZE: f32 = 12.0;
/// Space above and below the document, in em (GitHub pads its pages about this much).
const PAGE_PAD: f32 = 2.0;
/// Ctrl + trackpad scroll zooms one step per this many pixels.
pub const ZOOM_PIXELS: f64 = 50.0;
/// Lines scrolled per wheel notch.
const WHEEL_LINES: f64 = 3.0;
/// How long the toolbar's remote-images button pulses after the first frame, and how long the
/// toast stands in for it while the toolbar is hidden.
const PULSE: Duration = Duration::from_millis(900);
const TOAST: Duration = Duration::from_secs(4);
/// How long the large-file notice shows from the document's first frame.
const NOTICE: Duration = Duration::from_secs(6);
/// Documents larger than this render with a one-line notice.
pub const LARGE_FILE: usize = 10 << 20;
/// What hovering a blocked remote image's placeholder says.
pub const BLOCKED_NOTE: &str = "Remote image blocked · press I to load";
/// Passes [`View::bottom`] makes: measuring the last screen changes the total height.
const BOTTOM_PASSES: usize = 4;

/// The document as the view holds it: shared with the clipboard, whose copy of a large selection
/// reads the text from it when another program asks (and so outlives a reload).
#[cfg(feature = "clipboard")]
type Shared = Arc<Doc>;
#[cfg(not(feature = "clipboard"))]
type Shared = Doc;

fn share(doc: Doc) -> Shared {
    #[cfg(feature = "clipboard")]
    {
        Arc::new(doc)
    }
    #[cfg(not(feature = "clipboard"))]
    {
        doc
    }
}

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
    /// Jump to the previous / next heading (`[` / `]`).
    PrevHeading,
    NextHeading,
    /// Go back after following a link.
    Back,
    /// Show or hide the toolbar (`B`).
    ToggleToolbar,
    /// Wide layout on or off (`W`): the text spans the window or stops at `line_width`.
    ToggleWide,
    /// Show or hide the table of contents panel (`C`).
    #[cfg(feature = "toc")]
    ToggleToc,
    /// Open the find bar, or focus it again (`Ctrl+F`).
    #[cfg(feature = "find")]
    Find,
    /// Show or hide the find bar (the toolbar's button).
    #[cfg(feature = "find")]
    ToggleFind,
    /// Escape: closes the find bar if it's open, else the toolbar's dropdown if it's open, else
    /// the contents panel if it covers the document, else clears the selection, else quits.
    Escape,
    /// Select the whole document (`Ctrl+A`).
    #[cfg(feature = "clipboard")]
    SelectAll,
    /// Copy the selection to the clipboard (`Ctrl+C`).
    #[cfg(feature = "clipboard")]
    Copy,
    /// Load remote images for this document, or hide them again (`I`).
    ToggleRemote,
    /// Choose a file to open (`Ctrl+O`).
    Open,
    Quit,
}

/// What the shell must do after [`View::apply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Redraw,
    /// Re-read the document ([`read_input`]) and pass it to [`View::replace`].
    Reload,
    /// Return to the last place on the back stack (the shell owns it: it may be another file).
    Back,
    /// Ask the user for a file and open it (the shell shows the dialog).
    Open,
    /// Everything was selected: repaint, and offer it as the primary selection.
    #[cfg(feature = "clipboard")]
    Selected,
    /// Copy the selection ([`View::copy_content`]) to the clipboard.
    #[cfg(feature = "clipboard")]
    Copy,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    /// Follow the OS.
    #[default]
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
                "o" | "O" => Some(Action::Open),
                #[cfg(feature = "find")]
                "f" | "F" => Some(Action::Find),
                #[cfg(feature = "clipboard")]
                "a" | "A" => Some(Action::SelectAll),
                // Ctrl+Shift+C is a terminal's habit, and costs nothing.
                #[cfg(feature = "clipboard")]
                "c" | "C" => Some(Action::Copy),
                // Cmd+[ is the macOS back key; Ctrl+[ goes back on Linux too.
                "[" => Some(Action::Back),
                _ => None,
            },
            // Ctrl+Insert is the old copy, and the Copy key is the keyboard's own.
            #[cfg(feature = "clipboard")]
            Key::Named(NamedKey::Insert) if mods.control_key() => Some(Action::Copy),
            #[cfg(feature = "clipboard")]
            Key::Named(NamedKey::Copy) => Some(Action::Copy),
            _ => None,
        };
    }
    if mods.alt_key() {
        return match key {
            Key::Named(NamedKey::ArrowLeft) => Some(Action::Back),
            _ => None,
        };
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
            NamedKey::Backspace => Some(Action::Back),
            NamedKey::Escape => Some(Action::Escape),
            #[cfg(feature = "clipboard")]
            NamedKey::Copy => Some(Action::Copy),
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
            "[" => Some(Action::PrevHeading),
            "]" => Some(Action::NextHeading),
            "b" | "B" => Some(Action::ToggleToolbar),
            "w" | "W" => Some(Action::ToggleWide),
            #[cfg(feature = "toc")]
            "c" | "C" => Some(Action::ToggleToc),
            "i" | "I" if cfg!(feature = "remote-images") => Some(Action::ToggleRemote),
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

/// What the shell does after a hover update: see [`hover_change`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoverChange {
    /// Whether the cursor is the hand from now on.
    pub pointer: bool,
    /// The window's cursor must be set (to the hand if `pointer`, else the default).
    pub set_cursor: bool,
    /// The window must be repainted (the URL preview came, went or changed).
    pub redraw: bool,
}

/// Decide the cursor and the repaint after the link under the pointer was looked up again:
/// `pointer` is whether the window shows the hand, `hovered` whether a link is under the
/// pointer now, `changed` what [`View::hover`] / [`View::hover_end`] returned. The cursor is
/// compared with the link every time, not only when the hover changed: a new document resets
/// the hover without the shell's cursor knowing (following a link, Back, a reload, a drop),
/// and the hand would stay over plain text.
pub fn hover_change(pointer: bool, hovered: bool, changed: bool) -> HoverChange {
    HoverChange {
        pointer: hovered,
        set_cursor: hovered != pointer,
        redraw: changed,
    }
}

/// The notice for a document of `len` bytes: `None` up to [`LARGE_FILE`].
pub fn large_notice(len: usize) -> Option<String> {
    (len > LARGE_FILE).then(|| format!("This file is {} MB, so it may be slow", len >> 20))
}

/// The most a document may be: files over 10 MB render with a notice, but a link
/// can point at a device or something endless, and reading it must end.
pub const MAX_BYTES: u64 = 512 << 20;

/// Read all of `reader`, or `None` if it holds more than `limit` bytes.
fn read_limited(
    reader: impl std::io::Read,
    hint: u64,
    limit: u64,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::with_capacity(hint.min(limit) as usize);
    std::io::Read::read_to_end(&mut reader.take(limit + 1), &mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// How the reader came to a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Named by the reader on the command line (any file the OS can read, including
    /// a pipe such as `<(cmd)` or `/dev/stdin`; it is read once and the open may block until
    /// the writer is there, as for any other program).
    Argument,
    /// Reached from another document (a link, Back, a drop): only regular files, so a link
    /// can't hang the window on a FIFO or read a device.
    Navigation,
}

/// Whether `input` can be read again (reload, Back): not stdin, not the empty window, and not
/// a pipe or device named on the command line, which a second read would find empty or block
/// on. A file that can't be inspected (gone) counts: reading it again reports why.
pub fn rereadable(input: &Input) -> bool {
    match input {
        Input::File(path) => std::fs::metadata(path).map_or(true, |m| m.is_file()),
        Input::Stdin | Input::None => false,
    }
}

/// [`read_input`] with a chosen size limit.
fn read_input_limited(
    input: &Input,
    origin: Origin,
    limit: u64,
) -> Result<(String, Option<Stamp>), String> {
    let lossy = |bytes: Vec<u8>| match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    let too_big = |what: &str| format!("{what} is larger than {} MB", limit >> 20);
    match input {
        Input::None => Ok((String::new(), None)),
        Input::Stdin => {
            let stdin = std::io::stdin().lock();
            let bytes = read_limited(stdin, 0, limit)
                .map_err(|e| format!("Cannot read standard input: {e}"))?
                .ok_or_else(|| too_big("Standard input"))?;
            Ok((lossy(bytes), None))
        }
        Input::File(path) => {
            let shown = path.display();
            // Stamp first: a write landing between the two makes the next focus reload again
            // rather than miss the change.
            let before = stamp(path);
            // A document reached from another one must be a regular file. `metadata` follows
            // symlinks and opens nothing, so every target refuses a FIFO or device here without
            // blocking on it or touching it. The open is still non-blocking and the opened file
            // is judged again, so a path swapped for a FIFO after that look is refused too on
            // targets where `platform::open_nonblocking` has the flag (elsewhere that rare race
            // can still block the open). The command-line argument opens plainly: a pipe there
            // waits for its writer.
            if origin == Origin::Navigation
                && !std::fs::metadata(path).map_or(true, |m| m.is_file())
            {
                return Err(format!("Cannot open {shown}: not a regular file"));
            }
            let opened = match origin {
                Origin::Navigation => platform::open_nonblocking(path),
                Origin::Argument => std::fs::File::open(path),
            };
            let file = opened.map_err(|e| format!("Cannot open {shown}: {e}"))?;
            let meta = file
                .metadata()
                .map_err(|e| format!("Cannot open {shown}: {e}"))?;
            let regular = meta.is_file();
            if !regular && origin == Origin::Navigation {
                return Err(format!("Cannot open {shown}: not a regular file"));
            }
            // A pipe or device has no state to compare.
            let st = if regular { before } else { None };
            let bytes = read_limited(file, meta.len(), limit)
                .map_err(|e| format!("Cannot open {shown}: {e}"))?
                .ok_or_else(|| too_big(&shown.to_string()))?;
            Ok((lossy(bytes), st))
        }
    }
}

/// Read the document text. Invalid UTF-8 is replaced, never fatal. At most
/// [`MAX_BYTES`] are read, and a document reached by [`Origin::Navigation`] must be a regular
/// file (so a link can't hang or exhaust memory). Errors come back as the message to show in
/// the window.
pub fn read_input(input: &Input, origin: Origin) -> Result<(String, Option<Stamp>), String> {
    read_input_limited(input, origin, MAX_BYTES)
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
    doc: Shared,
    layout: Layout,
    fonts: Fonts,
    cache: GlyphCache,
    anchor: Anchor,
    /// Shown centered instead of the document (read errors, the empty window).
    message: Option<String>,
    msg_ctx: TextCtx,
    toolbar: Toolbar,
    /// The shell's back stack isn't empty (the toolbar's Back button).
    can_back: bool,
    /// There's a settings file for the toolbar's ⚙ to open.
    settings: bool,
    /// What a toolbar click asked for, waiting for [`View::take_command`].
    command: Option<toolbar::Command>,
    width: u32,
    height: u32,
    /// Height of the strip above the document area (the toolbar of P9), in physical pixels.
    top_inset: u32,
    /// What the strip was asked to be: `top_inset` is this, cut to the window's height.
    top_inset_wanted: u32,
    /// Physical pixels per logical pixel from the window system.
    hidpi: f64,
    zoom: u16,
    /// Base font size (logical px) and maximum characters per line (`font_size`, `line_width`).
    text: (f32, f32),
    /// Wide layout: no cap on the column (`wide`, `W`).
    wide: bool,
    theme: ThemeMode,
    system_dark: bool,
    fade: ScrollbarFade,
    drag: Option<Drag>,
    /// The pointer is over the scrollbar's column: it shows, and stays until the pointer leaves.
    over_bar: bool,
    /// Characters already handed to a fallback scan, over all documents shown (the
    /// system fonts are scanned once per character, not once per reload).
    tried: HashSet<char>,
    /// Characters of the message that no face covers, waiting for `take_missing`.
    msg_missing: BTreeSet<char>,
    /// Ctrl + trackpad scroll not yet turned into zoom steps.
    zoom_pixels: f64,
    /// The link under the pointer (1-based id into `Doc::links`): pointer cursor and URL preview.
    hover: Option<u32>,
    /// The pointer is on a blocked remote image's placeholder (its note shows).
    hover_blocked: bool,
    /// The link the mouse button went down on; a click is a release on the same link.
    pressed: Option<u32>,
    /// While a message replaces a reloaded document: the reading position of the last good
    /// one, for the next successful reload (a file that is briefly missing, as editors and
    /// branch switches make it, doesn't cost the position).
    kept: Option<(Option<Place>, Anchor)>,
    images: Images,
    /// A frame was painted since the images near the viewport were last asked for.
    images_dirty: bool,
    /// Remote images in the document (0 when they can't be loaded: the toolbar has no button).
    remote_count: u32,
    /// The first frame of the document was out and the remote images announced (or loaded,
    /// with the `always` policy).
    announced: bool,
    /// Until when the toolbar's remote-images button pulses, and the toast shows.
    pulse: Option<Instant>,
    toast: Option<(Instant, String)>,
    /// The large-file notice and until when it shows: set by the first frame that
    /// paints it.
    notice: Option<(Option<Instant>, String)>,
    /// The selected text, and the mouse gesture making it.
    #[cfg(feature = "clipboard")]
    select: gesture::Select,
    /// The copy button on the code block under the pointer.
    #[cfg(feature = "clipboard")]
    copy: copybtn::CopyButton,
    /// The contents panel: whether it is open, and what it lists.
    #[cfg(feature = "toc")]
    toc: toc::Panel,
    /// The find bar: whether it is open, the query and the matches.
    #[cfg(feature = "find")]
    find: find::Bar,
}

impl View {
    /// A view of `doc`, or of `message` in place of a document, in a `width` × `height` window.
    pub fn new(
        fonts: Fonts,
        doc: Doc,
        message: Option<String>,
        size: (u32, u32),
        hidpi: f64,
    ) -> View {
        View::with_base(fonts, doc, message, size, hidpi, None)
    }

    /// [`View::new`] for a document in directory `base` (see [`View::set_image_base`]), which
    /// its images are attached to once.
    pub fn with_base(
        fonts: Fonts,
        doc: Doc,
        message: Option<String>,
        (width, height): (u32, u32),
        hidpi: f64,
        base: Option<PathBuf>,
    ) -> View {
        let hidpi = sane_scale(hidpi);
        let d = Params::default();
        let text = (d.font_size, d.line_width);
        let params = params(width, hidpi, 100, text, false);
        let mut view = View {
            layout: Layout::new(&doc, params),
            doc: share(doc),
            fonts,
            cache: GlyphCache::default(),
            anchor: Anchor::TOP,
            message,
            msg_ctx: TextCtx::default(),
            // Hidden until the shell shows it (the `toolbar` setting), so a bare view (tests,
            // the bench) has the whole window as its document area.
            toolbar: {
                let mut t = Toolbar::default();
                t.set_visible(false);
                t
            },
            can_back: false,
            settings: false,
            command: None,
            width,
            height,
            top_inset: 0,
            top_inset_wanted: 0,
            hidpi,
            zoom: 100,
            text,
            wide: false,
            theme: ThemeMode::System,
            system_dark: false,
            fade: ScrollbarFade::default(),
            drag: None,
            over_bar: false,
            tried: HashSet::new(),
            msg_missing: BTreeSet::new(),
            zoom_pixels: 0.0,
            hover: None,
            hover_blocked: false,
            pressed: None,
            kept: None,
            images: Images::default(),
            images_dirty: false,
            remote_count: 0,
            announced: false,
            pulse: None,
            toast: None,
            notice: None,
            #[cfg(feature = "clipboard")]
            select: gesture::Select::default(),
            #[cfg(feature = "clipboard")]
            copy: copybtn::CopyButton::default(),
            #[cfg(feature = "toc")]
            toc: toc::Panel::default(),
            #[cfg(feature = "find")]
            find: find::Bar::default(),
        };
        view.images.set_base(base);
        view.attach_images();
        view
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

    /// Base font size in logical pixels and maximum characters per line
    /// (`font_size`, `line_width`); the reading position is kept.
    pub fn set_text(&mut self, font_size: f32, line_width: f32) {
        self.text = (font_size, line_width);
        self.relayout();
    }

    /// Zoom, base font size, line width and wide layout in one relayout: what the settings file
    /// asks for at startup is measured once, not once for each.
    pub fn set_layout(&mut self, zoom: u16, font_size: f32, line_width: f32, wide: bool) {
        self.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.text = (font_size, line_width);
        self.wide = wide;
        self.relayout();
    }

    pub fn wide(&self) -> bool {
        self.wide
    }

    /// Wide layout on or off (the `wide` setting); the reading position is kept.
    pub fn set_wide(&mut self, wide: bool) {
        if std::mem::replace(&mut self.wide, wide) != wide {
            self.relayout();
        }
    }

    /// Set body text (`mono` false) or code (`mono` true) in a family loaded from disk, or back
    /// in the default faces with `None` (see [`Fonts::set_family`]); everything is remeasured,
    /// keeping the reading position. Returns false if the family's regular face couldn't be
    /// loaded (the defaults are used).
    pub fn set_family(&mut self, mono: bool, faces: Option<&[Option<(PathBuf, u32)>; 4]>) -> bool {
        let ok = self.fonts.set_family(mono, faces);
        #[cfg(feature = "toc")]
        self.toc.forget_rows();
        self.layout.invalidate_all();
        self.ensure();
        ok
    }

    /// The window size the view lays out and paints for (physical pixels).
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reserve `px` pixels at the top of the window for something else (the toolbar, P9): the
    /// document area, its scrolling, pages and scrollbar start below it.
    pub fn set_top_inset(&mut self, px: u32) {
        self.top_inset_wanted = px;
        self.top_inset = px.min(self.height);
        self.ensure();
    }

    pub fn top_inset(&self) -> u32 {
        self.top_inset
    }

    // ---- Toolbar --------------------------------------------------------------------------

    /// Show or hide the toolbar; the document area gives up or reclaims its height,
    /// keeping the reading position.
    pub fn set_toolbar(&mut self, visible: bool) {
        self.toolbar.set_visible(visible);
        self.set_top_inset(self.chrome_inset());
    }

    /// The height of what the document area starts below: the toolbar and the find bar, in
    /// physical pixels.
    fn chrome_inset(&self) -> u32 {
        #[cfg(feature = "find")]
        let find = self.find_height();
        #[cfg(not(feature = "find"))]
        let find = 0;
        self.toolbar.height(self.hidpi as f32) + find
    }

    pub fn toolbar_visible(&self) -> bool {
        self.toolbar.visible()
    }

    /// Whether the toolbar's Back is enabled (the shell owns the back stack).
    pub fn set_can_back(&mut self, can_back: bool) -> bool {
        std::mem::replace(&mut self.can_back, can_back) != can_back
    }

    /// Whether the toolbar shows ⚙ (there is a settings file to open).
    pub fn set_settings_available(&mut self, available: bool) {
        if std::mem::replace(&mut self.settings, available) != available {
            self.close_menu();
        }
    }

    /// Close the toolbar's dropdown, if open (a changed window or bar changes what it holds).
    fn close_menu(&mut self) -> bool {
        let ctx = self.toolbar_ctx();
        self.toolbar.close_menu(&ctx)
    }

    /// What a toolbar click asked for, if anything since the last call.
    pub fn take_command(&mut self) -> Option<toolbar::Command> {
        self.command.take()
    }

    fn toolbar_ctx(&self) -> toolbar::Ctx {
        toolbar::Ctx {
            width: self.width,
            height: self.height,
            scale: self.hidpi as f32,
            can_back: self.can_back,
            zoom: self.zoom,
            theme: self.theme,
            wide: self.wide,
            #[cfg(feature = "toc")]
            toc: self.toc.open,
            #[cfg(feature = "find")]
            find: self.find.open,
            settings: self.settings,
            remote: self.remote_count,
            remote_on: self.images.remote_on(),
            no_curl: self.images.no_curl(),
            pulse: self.pulse.is_some(),
        }
    }

    /// The document area's height.
    fn area_height(&self) -> u32 {
        self.height.saturating_sub(self.top_inset)
    }

    /// Width of the contents panel in physical pixels: 0 while it isn't showing.
    fn panel_width(&self) -> u32 {
        #[cfg(feature = "toc")]
        {
            self.toc_width()
        }
        #[cfg(not(feature = "toc"))]
        {
            0
        }
    }

    /// Where the document area starts: right of the panel when it pushes the document aside, at
    /// the window's edge when it covers it (or there is none).
    fn area_x(&self) -> u32 {
        #[cfg(feature = "toc")]
        {
            if self.toc_pushes() {
                self.toc_width()
            } else {
                0
            }
        }
        #[cfg(not(feature = "toc"))]
        {
            0
        }
    }

    /// The width the document is laid out in: the area's.
    fn layout_width(&self) -> u32 {
        self.width.saturating_sub(self.area_x())
    }

    /// The layout parameters for the window as it is.
    fn layout_params(&self) -> Params {
        params(
            self.layout_width(),
            self.hidpi,
            self.zoom,
            self.text,
            self.wide,
        )
    }

    /// The point is in the part of the document area that shows the document: not on the
    /// toolbar, and not under the contents panel.
    pub(super) fn in_area(&self, x: f64, y: f64) -> bool {
        (f64::from(self.panel_width())..f64::from(self.width)).contains(&x)
            && (f64::from(self.top_inset)..f64::from(self.height)).contains(&y)
    }

    /// Document x of window x: layout coordinates start at the area's left edge.
    pub(super) fn doc_x(&self, x: f64) -> f64 {
        x - f64::from(self.area_x())
    }

    /// Document y of window y, for a point in the area.
    pub(super) fn doc_y(&self, y: f64) -> f64 {
        self.scroll() + (y - f64::from(self.top_inset))
    }

    /// The window was resized (physical pixels).
    pub fn resize(&mut self, width: u32, height: u32) {
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.close_menu();
        }
        self.top_inset = self.top_inset_wanted.min(height);
        self.relayout();
    }

    pub fn set_hidpi(&mut self, hidpi: f64) {
        let hidpi = sane_scale(hidpi);
        if std::mem::replace(&mut self.hidpi, hidpi) != hidpi {
            self.close_menu();
        }
        // The toolbar and the find bar scale with the window, not the zoom.
        self.top_inset_wanted = self.chrome_inset();
        self.top_inset = self.top_inset_wanted.min(self.height);
        self.relayout();
    }

    /// New layout parameters; the anchor keeps the reading position. The blocks around it are
    /// measured at once: until they are, the heights are the old ones (rescaled by their
    /// estimates in the new column, nothing rewrapped), so a window that got narrower and taller
    /// would seem to end above the position, and clamping against that would throw it away.
    fn relayout(&mut self) {
        self.layout.set_params(self.layout_params());
        self.ensure();
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
        let anchor = self.layout.anchor_exact(y.clamp(0.0, self.max_scroll()));
        self.move_to(anchor);
    }

    /// Move the reading position. Leaving the one a ToC jump ended at drops that jump's mark.
    fn move_to(&mut self, anchor: Anchor) {
        #[cfg(feature = "toc")]
        if self.toc.aimed.is_some_and(|(_, at)| at != anchor) {
            self.toc.aimed = None;
        }
        self.anchor = anchor;
    }

    /// Scroll by `dy` physical pixels; the toolbar's open dropdown takes it instead of the
    /// document.
    pub fn scroll_by(&mut self, dy: f64, now: Instant) {
        if self.toolbar.scroll_menu(&self.toolbar_ctx(), dy as f32) {
            return;
        }
        self.set_scroll(self.scroll() + dy);
        self.fade.poke(now);
        #[cfg(feature = "clipboard")]
        self.follow_pointer();
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
        #[cfg(feature = "toc")]
        if self.toc_wheel_lines(lines) {
            return;
        }
        self.wheel_by(lines * WHEEL_LINES * self.line(), now);
    }

    /// Trackpad or wheel scrolling by `dy` physical pixels: the contents panel takes it when
    /// the pointer is over it, else the document (and the toolbar's dropdown) does, as in
    /// [`View::scroll_by`].
    pub fn wheel_by(&mut self, dy: f64, now: Instant) {
        // The find bar has nothing to scroll, and the document below it is out of reach.
        #[cfg(feature = "find")]
        if self.find_wheel() {
            return;
        }
        #[cfg(feature = "toc")]
        if self.toc_wheel(dy) {
            return;
        }
        self.scroll_by(dy, now);
    }

    /// Ctrl + trackpad scroll by `pixels` (positive = zoom in): a zoom step for every
    /// [`ZOOM_PIXELS`] gathered, the rest kept for the next event of the gesture.
    pub fn zoom_wheel(&mut self, pixels: f64, now: Instant) {
        self.zoom_pixels += pixels;
        while self.zoom_pixels.abs() >= ZOOM_PIXELS {
            let up = self.zoom_pixels > 0.0;
            self.zoom_pixels -= ZOOM_PIXELS.copysign(self.zoom_pixels);
            self.apply(if up { Action::ZoomIn } else { Action::ZoomOut }, now);
        }
    }

    /// The zoom modifier was released: what is left of a step doesn't carry into a later gesture.
    pub fn zoom_wheel_end(&mut self) {
        self.zoom_pixels = 0.0;
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
                self.move_to(Anchor::TOP);
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
            Action::PrevHeading => {
                self.heading(-1, now);
            }
            Action::NextHeading => {
                self.heading(1, now);
            }
            Action::Back => return Outcome::Back,
            Action::ToggleToolbar => {
                let visible = !self.toolbar.visible();
                self.set_toolbar(visible);
            }
            Action::Reload => return Outcome::Reload,
            Action::Open => return Outcome::Open,
            Action::ToggleWide => self.set_wide(!self.wide),
            #[cfg(feature = "toc")]
            Action::ToggleToc => self.set_toc(!self.toc.open),
            #[cfg(feature = "find")]
            Action::Find => self.find_focus(),
            #[cfg(feature = "find")]
            Action::ToggleFind => self.set_find(!self.find.open),
            Action::ToggleRemote => {
                if self.remote_count == 0 {
                    return Outcome::Nothing;
                }
                let on = !self.images.remote_on();
                self.set_remote(on);
            }
            Action::Escape if self.close_menu() => {}
            #[cfg(feature = "find")]
            Action::Escape if self.find_escape() => {}
            #[cfg(feature = "toc")]
            Action::Escape if self.toc_escape() => {}
            #[cfg(feature = "clipboard")]
            Action::Escape if self.clear_selection() => {}
            #[cfg(feature = "clipboard")]
            Action::SelectAll => {
                return if self.select_all() {
                    Outcome::Selected
                } else {
                    Outcome::Nothing
                };
            }
            #[cfg(feature = "clipboard")]
            Action::Copy => {
                return if self.selection().is_some() {
                    Outcome::Copy
                } else {
                    Outcome::Nothing
                };
            }
            Action::Escape | Action::Quit => return Outcome::Quit,
        }
        Outcome::Redraw
    }

    // ---- Scrollbar drag -------------------------------------------------------------------

    fn viewport(&self) -> Viewport {
        Viewport {
            area: Clip {
                x0: self.area_x().min(i32::MAX as u32) as i32,
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
    /// scrollbar is showing. Hovering the track's column shows it (before any press, since the
    /// pointer moves there first), so only a press that arrives without a move, as from touch
    /// or a script, still reaches a link under a hidden one.
    /// Returns true if the window needs a repaint: the toolbar or the scrollbar took the press, or
    /// the selection changed.
    pub fn press(&mut self, x: f64, y: f64, now: Instant) -> bool {
        #[cfg(feature = "clipboard")]
        self.cancel_copy();
        let ctx = self.toolbar_ctx();
        if self.toolbar.press(&ctx, x, y) {
            self.pressed = None;
            #[cfg(feature = "clipboard")]
            self.cancel_select();
            return true;
        }
        // The find bar's: its buttons, and the rest of it, which does nothing.
        #[cfg(feature = "find")]
        if self.press_find(x, y) {
            self.pressed = None;
            #[cfg(feature = "clipboard")]
            self.cancel_select();
            return true;
        }
        // The contents panel's, or the click outside it that closes it over the document.
        #[cfg(feature = "toc")]
        if self.press_toc(x, y) {
            self.pressed = None;
            #[cfg(feature = "clipboard")]
            self.cancel_select();
            return true;
        }
        self.ensure_content();
        let taken = self.press_scrollbar(x, y, now);
        // Not the scrollbar's: the copy button of a code block takes it, with no link or
        // selection.
        #[cfg(feature = "clipboard")]
        if !taken && self.press_copy(x, y) {
            self.pressed = None;
            self.cancel_select();
            self.copy_look();
            return true;
        }
        // Not that either: remember the link under it, for a click on release.
        self.pressed = if taken { None } else { self.link_at(x, y) };
        // Not the scrollbar's either: a press on text, which may start a selection.
        #[cfg(feature = "clipboard")]
        if taken {
            self.cancel_select();
        } else {
            return self.press_text(x, y, now);
        }
        taken
    }

    /// The scrollbar's thumb if (`x`, `y`) is in its column: the thumb and its margin to the
    /// right edge, a few pixels wider to the left so the thin thumb is easy to hit, below the
    /// toolbar. `None` when the document doesn't scroll.
    fn bar_column(&self, x: f64, y: f64) -> Option<render::Thumb> {
        let t = self.thumb_at(self.scroll())?;
        let slack = f64::from(t.x1 - t.x0);
        (x >= f64::from(t.x0) - slack && y >= f64::from(self.top_inset)).then_some(t)
    }

    /// The scrollbar is held on screen: dragged, or under the pointer.
    fn bar_held(&self) -> bool {
        self.drag.is_some() || self.over_bar
    }

    fn press_scrollbar(&mut self, x: f64, y: f64, now: Instant) -> bool {
        let Some(t) = self.bar_column(x, y) else {
            return false;
        };
        let (y0, y1) = (f64::from(t.y0), f64::from(t.y1));
        let on_thumb = (y0..y1).contains(&y);
        if !on_thumb && !self.fade.visible(now) && !self.over_bar {
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

    /// The mouse button was released. Returns true if a drag ended, or a press on the contents
    /// panel did (its row stops looking pressed, whether or not it was a click).
    pub fn release(&mut self, now: Instant) -> bool {
        #[cfg(feature = "toc")]
        let row = self.toc.pressed.is_some();
        #[cfg(not(feature = "toc"))]
        let row = false;
        if self.drag.take().is_some() {
            self.fade.poke(now);
            return true;
        }
        row
    }

    /// The mouse button came up at (`x`, `y`) after a press the toolbar took. Returns true if
    /// it needs a repaint; a click's command waits for [`View::take_command`].
    pub fn release_toolbar(&mut self, x: f64, y: f64) -> bool {
        let ctx = self.toolbar_ctx();
        let (handled, command) = self.toolbar.release(&ctx, x, y);
        if command.is_some() {
            self.command = command;
        }
        // The find bar's buttons act on their own, with no command for the shell.
        #[cfg(feature = "find")]
        let handled = handled | self.release_find(x, y, Instant::now());
        handled
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    // ---- Links and headings ---------------------------------------------------------------

    /// Hit-testing needs the blocks on screen built: scrolling, a jump or a resize evicts and
    /// rebuilds them in the next paint, which is after the shell asked who is under the pointer.
    fn ensure_content(&mut self) {
        if self.message.is_none() {
            self.ensure();
        }
    }

    /// The link (1-based id into `Doc::links`) under window point (`x`, `y`), if any. Only
    /// blocks near the viewport have content to hit, which covers everything on screen; a point
    /// outside the document area (a press-drag goes on reporting the pointer beyond the window,
    /// and the contents panel may cover the document's edge) hits nothing even where cached
    /// content continues, and neither does NaN.
    pub fn link_at(&self, x: f64, y: f64) -> Option<u32> {
        if self.message.is_some() || !self.in_area(x, y) {
            return None;
        }
        let hit = self
            .layout
            .hit(self.doc_x(x) as f32, self.doc_y(y) as f32)?;
        (hit.link != 0).then_some(hit.link)
    }

    /// The pointer moved to (`x`, `y`). Returns true if the link under it changed: repaint
    /// for the URL preview (and switch the pointer cursor, see [`View::hovered`]).
    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        self.hover_at(x, y, Instant::now())
    }

    /// [`View::hover`] at a given time (the toolbar's tooltip timer starts then).
    pub fn hover_at(&mut self, x: f64, y: f64, now: Instant) -> bool {
        self.ensure_content();
        let ctx = self.toolbar_ctx();
        let bar = self.toolbar.pointer(&ctx, x, y, now);
        let free = self.drag.is_none() && !self.toolbar.covers(&ctx, x, y);
        // The find bar takes the pointer as the toolbar does: no link under it, a tooltip on its
        // buttons.
        #[cfg(feature = "find")]
        let bar = bar | self.find_pointer(self.drag.is_none().then_some((x, y)), now);
        #[cfg(feature = "find")]
        let free = free && !self.find_covers(x, y);
        // A selection drag has no links under it: the text cursor stays.
        #[cfg(feature = "clipboard")]
        let free = free && !self.selecting();
        // The contents panel's rows light up under the pointer, and the document under it is
        // out of reach.
        #[cfg(feature = "toc")]
        let bar = bar | self.toc_pointer(Some((x, y)), free);
        #[cfg(feature = "toc")]
        let free = free && !self.toc_covers(x, y);
        let link = if free { self.link_at(x, y) } else { None };
        let bar = bar | self.set_over_bar(free && self.bar_column(x, y).is_some(), now);
        let blocked = cfg!(feature = "remote-images")
            && free
            && self
                .image_at(x, y)
                .is_some_and(|id| self.images.blocked(id));
        let note = std::mem::replace(&mut self.hover_blocked, blocked) != blocked;
        #[cfg(feature = "clipboard")]
        let note = note | self.copy_pointer(Some((x, y)));
        (std::mem::replace(&mut self.hover, link) != link) | bar | note
    }

    /// The pointer left the window. Returns true if a link was hovered (or the toolbar needs a
    /// repaint).
    pub fn hover_end(&mut self) -> bool {
        let note = std::mem::take(&mut self.hover_blocked);
        let bar = self.set_over_bar(false, Instant::now());
        #[cfg(feature = "find")]
        let bar = bar | self.find_pointer(None, Instant::now());
        #[cfg(feature = "toc")]
        let bar = bar | self.toc_pointer(None, false);
        #[cfg(feature = "clipboard")]
        let note = note | self.copy_pointer(None);
        self.hover.take().is_some() | self.toolbar.leave() | note | bar
    }

    /// The pointer went over the scrollbar's column (`over`) or left it. Entering shows the
    /// scrollbar, held while the pointer stays; leaving lets it linger and fade as after a
    /// scroll. Returns true if the scrollbar appeared (repaint).
    fn set_over_bar(&mut self, over: bool, now: Instant) -> bool {
        if std::mem::replace(&mut self.over_bar, over) == over {
            return false;
        }
        let shown = self.fade.visible(now) || self.drag.is_some();
        self.fade.poke(now);
        over && !shown
    }

    /// The target of the hovered link, as written in the document.
    pub fn hovered(&self) -> Option<&str> {
        let id = self.hover? as usize;
        self.doc.links.get(id.checked_sub(1)?).map(String::as_str)
    }

    /// The mouse button came up at (`x`, `y`): the target of the link clicked (pressed and
    /// released on the same link), if any. A click on a row of the contents panel is no link:
    /// it waits for [`View::take_jump`].
    pub fn click(&mut self, x: f64, y: f64) -> Option<String> {
        // A click on the contents panel is for the panel, whatever else was pressed.
        #[cfg(feature = "toc")]
        if let Some(row) = self.toc.pressed.take() {
            self.click_toc(row, x, y);
            return None;
        }
        let pressed = self.pressed.take()?;
        self.ensure_content();
        if self.link_at(x, y) != Some(pressed) {
            return None;
        }
        self.doc.links.get(pressed as usize - 1).cloned()
    }

    /// Scroll to the heading or footnote a `#fragment` names. Returns false if there's none.
    pub fn jump_to_anchor(&mut self, fragment: &str, now: Instant) -> bool {
        let Some(block) = crate::nav::find_anchor(&self.doc, fragment) else {
            return false;
        };
        self.jump_to_block(block, now);
        true
    }

    /// Scroll to `block`, a heading or a footnote.
    pub fn jump_to_block(&mut self, block: u32, now: Instant) {
        self.set_anchor(Anchor { block, frac: 0.0 });
        #[cfg(feature = "toc")]
        self.toc_aim(block);
        self.fade.poke(now);
    }

    /// Go to a reading position (from the back stack), kept inside the document.
    pub fn set_anchor(&mut self, anchor: Anchor) {
        let n = self.doc.blocks.len() as u32;
        self.move_to(if n == 0 {
            Anchor::TOP
        } else {
            Anchor {
                block: anchor.block.min(n - 1),
                frac: anchor.frac.clamp(0.0, 1.0),
            }
        });
        self.ensure();
    }

    /// Jump to the next (`dir` > 0) or previous heading from the reading position. Returns
    /// false if there's none that way.
    pub fn heading(&mut self, dir: i32, now: Instant) -> bool {
        let at = self.scroll();
        // From a heading just jumped to that couldn't come up to the top (near the end), not
        // from the top of the window.
        #[cfg(feature = "toc")]
        let at = self
            .toc_aimed()
            .map_or(at, |block| self.layout.top(block as usize));
        let mut blocks: Vec<u32> = self.doc.headings.iter().map(|h| h.block).collect();
        blocks.sort_unstable();
        blocks.dedup();
        let top = |b: u32| self.layout.top(b as usize);
        let target = if dir > 0 {
            blocks.iter().copied().find(|&b| top(b) > at + 0.5)
        } else {
            blocks.iter().rev().copied().find(|&b| top(b) < at - 0.5)
        };
        let Some(block) = target else {
            return false;
        };
        self.jump_to_block(block, now);
        true
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
        } else {
            self.ensure();
            let vp = self.viewport();
            #[cfg(feature = "clipboard")]
            let sel = self.selection();
            // Whatever uses the inset paints over this.
            frame.fill_rect(0, 0, w as i32, vp.area.y0, theme.bg);
            #[cfg(feature = "find")]
            let finds = {
                let doc: &Doc = &self.doc;
                let found = &self.find.found;
                (self.find.open && !found.matches.is_empty())
                    .then(|| (doc, found.marks(self.find.current)))
            };
            render::paint(
                &mut frame,
                vp,
                &self.layout,
                &self.fonts,
                &mut self.cache,
                theme,
                #[cfg(feature = "clipboard")]
                sel,
                #[cfg(feature = "find")]
                finds,
            );
            if cfg!(feature = "images") {
                self.images.begin_frame();
                render::paint_images(
                    &mut frame,
                    vp,
                    &self.layout,
                    &mut self.images,
                    theme,
                    #[cfg(feature = "clipboard")]
                    sel,
                );
                self.images_dirty = true;
            }
            #[cfg(feature = "clipboard")]
            self.paint_copy_button(&mut frame, vp.area, theme, now);
            if self.fade.visible(now) || self.bar_held() {
                let total = self.layout.total_height();
                render::paint_scrollbar(&mut frame, vp, total, self.scale(), theme);
            }
            let note = self
                .hovered()
                .or(self.hover_blocked.then_some(BLOCKED_NOTE));
            if let Some(text) = note.map(str::to_owned) {
                self.paint_preview(&mut frame, &text, theme);
            }
            if let Some((_, text)) = self.toast.clone() {
                self.paint_toast(&mut frame, &text, theme, false);
            }
            if let Some((until, text)) = self.notice.as_mut() {
                let text = text.clone();
                until.get_or_insert(now + NOTICE);
                self.paint_toast(&mut frame, &text, theme, true);
            }
            #[cfg(feature = "toc")]
            self.paint_toc(&mut frame, theme);
        }
        // Under the toolbar's dropdown, which hangs over it.
        #[cfg(feature = "find")]
        self.paint_find(&mut frame, theme);
        let ctx = self.toolbar_ctx();
        self.toolbar.paint(
            &mut frame,
            &ctx,
            &self.fonts,
            &mut self.cache,
            &mut self.msg_ctx,
            theme,
        );
    }

    /// The hovered link's target in a small box at the bottom left, cut short with
    /// an ellipsis when it doesn't fit.
    fn paint_preview(&mut self, frame: &mut Frame, url: &str, theme: &Theme) {
        let (w, h) = (frame.width() as f32, frame.height() as f32);
        // The box sits in what shows of the document, right of the contents panel.
        let left = self.panel_width() as f32;
        let scale = self.scale();
        let size = PREVIEW_SIZE * scale;
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size,
            role: Role::Muted,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let (pad_x, pad_y, margin) = (0.5 * size, 0.3 * size, (4.0 * scale).round());
        let max_w = ((w - left) * 0.6)
            .min(w - left - 2.0 * (margin + pad_x))
            .max(size);
        let (para, pl) = self.fit_preview(url, style, max_w);
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let box_w = pl.width.min(max_w) + 2.0 * pad_x;
        let box_h = pl.height + 2.0 * pad_y;
        let (x0, y1) = (left + margin, h - margin);
        let y0 = y1 - box_h;
        let px = |v: f32| v.round() as i32;
        let r = (0.3 * size).round();
        let rect = (px(x0), px(y0), px(x0 + box_w), px(y1));
        frame.rounded_rect(rect, r, None, theme.code_bg, 255);
        frame.rounded_rect(rect, r, Some(scale.round().max(1.0)), theme.border, 255);
        let content = BlockBox::from_para(pl, &para.styles);
        let prev = frame.set_clip(render::Clip {
            x0: rect.0,
            y0: rect.1,
            x1: rect.2 - px(pad_x) / 2,
            y1: rect.3,
        });
        render::paint_block(
            frame,
            &content,
            x0 + pad_x,
            y0 + pad_y,
            &self.fonts,
            &mut self.cache,
            theme,
            #[cfg(feature = "clipboard")]
            None,
        );
        frame.set_clip(prev);
    }

    /// A short note in a box at the bottom right (remote images are blocked and
    /// the toolbar that would say so is hidden), or at the top in the middle, below the
    /// toolbar (`top`: the large-file notice).
    fn paint_toast(&mut self, frame: &mut Frame, text: &str, theme: &Theme, top: bool) {
        let (w, h) = (frame.width() as f32, frame.height() as f32);
        let left = self.panel_width() as f32;
        let scale = self.hidpi as f32;
        let size = 13.0 * scale;
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size,
            role: Role::Text,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let mut para = Para::default();
        para.push_text(text, 0, style);
        let (pad_x, pad_y, margin) = (0.8 * size, 0.5 * size, (12.0 * scale).round());
        let max_w = (w - left - 2.0 * (margin + pad_x)).max(size);
        let pl = self
            .msg_ctx
            .layout(&self.fonts, &para, max_w, Align::Start, 1.4);
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let (box_w, box_h) = (pl.width + 2.0 * pad_x, pl.height + 2.0 * pad_y);
        let (x0, y0) = if top {
            let y0 = self.top_inset as f32 + margin;
            (left + ((w - left - box_w) / 2.0).max(0.0), y0)
        } else {
            ((w - margin - box_w).max(left), h - margin - box_h)
        };
        let (x1, y1) = ((x0 + box_w).min(w), y0 + box_h);
        let px = |v: f32| v.round() as i32;
        let rect = (px(x0), px(y0), px(x1), px(y1));
        let r = (0.4 * size).round();
        frame.rounded_rect(rect, r, None, theme.code_bg, 255);
        frame.rounded_rect(rect, r, Some(scale.round().max(1.0)), theme.border, 255);
        let content = BlockBox::from_para(pl, &para.styles);
        render::paint_block(
            frame,
            &content,
            x0 + pad_x,
            y0 + pad_y,
            &self.fonts,
            &mut self.cache,
            theme,
            #[cfg(feature = "clipboard")]
            None,
        );
    }

    /// A link's target laid out for the URL preview: see [`View::fit_line`], with the invisible
    /// characters shown.
    fn fit_preview(&mut self, url: &str, style: TextStyle, max_w: f32) -> (Para, ParaLayout) {
        self.fit_line(url, style, max_w, true)
    }

    /// `raw` laid out in one line no wider than `max_w`: cut short with a trailing `…` when it
    /// is wider, at the longest prefix that fits when measured (character counts say little
    /// about width: CJK and wide hosts take twice the room of ASCII). With `escape`, invisible
    /// characters (bidi controls, zero-width ones) are shown as `\u{202E}`, never acted on, as
    /// a URL's must be; text that starts with right-to-left text is still laid out left to right.
    fn fit_line(
        &mut self,
        raw: &str,
        style: TextStyle,
        max_w: f32,
        escape: bool,
    ) -> (Para, ParaLayout) {
        // Characters narrower than a fifth of an em are rare, so a longer line is cut to this
        // many before measuring: a huge link isn't laid out whole on every paint.
        let most = (max_w / (0.2 * style.size)) as usize + 1;
        // The first `most` characters as shown, and where each one ends in `text`: a cut never
        // falls inside an escape.
        let (mut text, mut ends) = (String::new(), Vec::new());
        let mut chars = raw.chars();
        for c in chars.by_ref().take(most) {
            if escape && crate::nav::is_invisible(c) {
                text.push_str(&format!("\\u{{{:04X}}}", c as u32));
            } else {
                text.push(c);
            }
            ends.push(text.len());
        }
        let cut = chars.next().is_some();
        // The first strong character would otherwise set the paragraph's direction.
        let rtl = text
            .chars()
            .any(|c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL));
        let prefix = |n: usize| -> String {
            let n = n.min(ends.len());
            let mut shown = String::from(if rtl { "\u{200E}" } else { "" });
            shown.push_str(&text[..n.checked_sub(1).map_or(0, |i| ends[i])]);
            if n < ends.len() || cut {
                shown.push('…');
            }
            shown
        };
        let lay = |view: &mut View, text: &str| {
            let mut para = Para::default();
            para.push_text(text, 0, style);
            let pl = view
                .msg_ctx
                .layout(&view.fonts, &para, 1.0e6, Align::Start, 1.4);
            (para, pl)
        };
        let first = lay(self, &prefix(most));
        if first.1.width <= max_w || most == 0 {
            return first;
        }
        // The widest prefix that fits (widths grow with the prefix); none still shows `…`.
        let (mut lo, mut hi) = (0, most);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if lay(self, &prefix(mid)).1.width <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lay(self, &prefix(lo))
    }

    /// One paragraph of muted text, centered in the area below the toolbar.
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
        // The message is laid out on every paint, so a face found later covers it then.
        self.msg_missing
            .extend(pl.missing.iter().filter(|c| !self.tried.contains(c)));
        let top = self.top_inset as f32;
        let oy = top + ((h - top - pl.height) / 2.0).max(0.0);
        let content = BlockBox::from_para(pl, &para.styles);
        let ox = (w - max_w) / 2.0;
        render::paint_block(
            frame,
            &content,
            ox,
            oy,
            &self.fonts,
            &mut self.cache,
            theme,
            #[cfg(feature = "clipboard")]
            None,
        );
    }

    // ---- Scrollbar timer ------------------------------------------------------------------

    /// When the event loop must wake for the scrollbar (`None`: never).
    pub fn deadline(&self) -> Option<Instant> {
        let fade = if self.bar_held() {
            None
        } else {
            self.fade.deadline()
        };
        #[cfg(feature = "clipboard")]
        let scroll = self.select_deadline();
        #[cfg(not(feature = "clipboard"))]
        let scroll = None;
        #[cfg(feature = "clipboard")]
        let checked = self.copy_deadline();
        #[cfg(not(feature = "clipboard"))]
        let checked = None;
        #[cfg(feature = "find")]
        let tip = self.find_deadline();
        #[cfg(not(feature = "find"))]
        let tip = None;
        [
            fade,
            self.toolbar.deadline(&self.toolbar_ctx()),
            self.pulse,
            self.toast.as_ref().map(|t| t.0),
            self.notice.as_ref().and_then(|n| n.0),
            scroll,
            checked,
            tip,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Hide the scrollbar or show the toolbar's tooltip if its time is up. Returns true if the
    /// window needs a repaint.
    pub fn expire(&mut self, now: Instant) -> bool {
        let fade = !self.bar_held() && self.fade.expire(now);
        let pulse = self.pulse.is_some_and(|t| now >= t);
        if pulse {
            self.pulse = None;
        }
        let toast = self.toast.as_ref().is_some_and(|t| now >= t.0);
        if toast {
            self.toast = None;
        }
        let notice = self
            .notice
            .as_ref()
            .is_some_and(|n| n.0.is_some_and(|t| now >= t));
        if notice {
            self.notice = None;
        }
        #[cfg(feature = "clipboard")]
        let scrolled = self.autoscroll(now) | self.copy_expire(now);
        #[cfg(not(feature = "clipboard"))]
        let scrolled = false;
        #[cfg(feature = "find")]
        let scrolled = scrolled | self.find_expire(now);
        fade | pulse | toast | notice | scrolled | self.toolbar.expire(&self.toolbar_ctx(), now)
    }

    /// What the event loop does each time it runs out of events: hide the scrollbar if its
    /// time is up (whatever woke the loop, and however busy measuring keeps it), then measure
    /// until `measure_until` (`None`: not yet), and say when to run again. A scrollbar on
    /// screen is repainted when measuring moves its thumb.
    pub fn tick(&mut self, now: Instant, measure_until: Option<Instant>) -> Tick {
        let mut redraw = self.expire(now);
        // Images wait for the first frame (`measure_until` is set once it's out).
        if measure_until.is_some() && self.message.is_none() {
            redraw |= self.announce(now);
            if std::mem::take(&mut self.images_dirty) {
                self.request_images();
            }
        }
        let showing = self.message.is_none() && (self.bar_held() || self.fade.visible(now));
        let thumb = showing.then(|| self.thumb_at(self.scroll()));
        let measured = measure_until.is_none_or(|t| self.idle(t));
        redraw |= thumb.is_some_and(|before| before != self.thumb_at(self.scroll()));
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
        missing.extend(std::mem::take(&mut self.msg_missing));
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
            #[cfg(feature = "toc")]
            self.toc.forget_rows();
            self.layout.invalidate_missing(&self.doc);
        }
        added
    }

    // ---- Images ---------------------------------------------------------------------------

    /// Hand the document and its new layout to the images: local sizes come from their files,
    /// remote ones from whether they're on.
    fn attach_images(&mut self) {
        self.images.attach(&self.doc, &mut self.layout);
        self.remote_count = if self.images.policy() == RemoteImages::Never {
            0
        } else {
            self.doc.remote_image_count().min(u32::MAX as usize) as u32
        };
    }

    /// The directory the document's relative image paths resolve against (`None`: stdin, and
    /// only absolute paths load).
    pub fn set_image_base(&mut self, base: Option<PathBuf>) {
        self.images.set_base(base);
        self.attach_images();
    }

    /// How the image threads wake the event loop when a result is waiting for
    /// [`View::poll_images`].
    pub fn set_image_wake(&mut self, wake: ImageWake) {
        self.images.set_wake(wake);
    }

    /// The `remote_images` setting: `never` turns them off and takes away the
    /// button and the `I` key.
    pub fn set_remote_policy(&mut self, policy: RemoteImages) {
        if policy == self.images.policy() {
            return;
        }
        self.images.set_policy(policy);
        if policy == RemoteImages::Never && self.images.remote_on() {
            self.set_remote(false);
        }
        self.remote_count = if policy == RemoteImages::Never {
            0
        } else {
            self.doc.remote_image_count().min(u32::MAX as usize) as u32
        };
        self.close_menu();
    }

    /// Remote images are on for this document.
    pub fn remote_on(&self) -> bool {
        self.images.remote_on()
    }

    /// Turn remote images on or off for this document; the reading position is kept.
    fn set_remote(&mut self, on: bool) {
        self.images.set_remote(on, &self.doc, &mut self.layout);
        self.pulse = None;
        self.toast = None;
        self.images_dirty = true;
        self.close_menu();
        self.ensure();
    }

    /// Show `text` in the toast box at the bottom right for a few seconds (a call before it
    /// expires replaces it). The caller repaints.
    pub fn show_toast(&mut self, text: String, now: Instant) {
        self.toast = Some((now + TOAST, text));
    }

    /// After the document's first frame: point out its blocked remote images once (the
    /// toolbar's button pulses, or a toast stands in for a hidden toolbar), or load them with
    /// the `always` policy. Returns true if the window needs a repaint.
    fn announce(&mut self, now: Instant) -> bool {
        if !cfg!(feature = "remote-images")
            || std::mem::replace(&mut self.announced, true)
            || self.remote_count == 0
        {
            return false;
        }
        match self.images.policy() {
            RemoteImages::Always => self.set_remote(true),
            RemoteImages::Ask if !self.images.remote_on() => {
                if self.toolbar.visible() {
                    self.pulse = Some(now + PULSE);
                } else {
                    let n = self.remote_count;
                    let s = if n == 1 { "" } else { "s" };
                    let text = format!("{n} remote image{s} blocked · press I to load");
                    self.toast = Some((now + TOAST, text));
                }
            }
            _ => return false,
        }
        true
    }

    /// Ask for the images within a screen of the viewport, those on screen first.
    fn request_images(&mut self) {
        let h = self.area_height() as f32;
        let y = self.scroll() as f32;
        let mut near = Vec::new();
        for i in self.layout.range(y - h, y + 2.0 * h) {
            let Some(content) = self.layout.geoms()[i].content.as_deref() else {
                continue;
            };
            let top = self.layout.y(i);
            for b in &content.boxes {
                let (y0, y1) = (top + b.y, top + b.y + b.b.ascent + b.b.descent);
                let off_screen = y1 <= y || y0 >= y + h;
                let size = |v: f32| v.round().max(1.0) as u32;
                near.push((
                    off_screen,
                    b.b.id,
                    size(b.b.width),
                    size(b.b.ascent + b.b.descent),
                ));
            }
        }
        // Stable: on screen first, then the rest in document order.
        near.sort_by_key(|n| n.0);
        let on_screen = near.iter().take_while(|n| !n.0).count();
        let boxes: Vec<(u32, u32, u32)> =
            near.into_iter().map(|(_, id, w, h)| (id, w, h)).collect();
        self.images.request(&boxes, on_screen);
    }

    /// Take what the image threads finished: decoded images show, fetched ones get their size
    /// (the reading position is kept). Returns true if the window needs a repaint.
    pub fn poll_images(&mut self, now: Instant) -> bool {
        let no_curl = self.images.no_curl();
        if !self.images.poll(&self.doc, &mut self.layout) {
            return false;
        }
        if !no_curl && self.images.no_curl() && !self.toolbar.visible() {
            self.toast = Some((now + TOAST, "Remote images: curl not found".into()));
        }
        if self.message.is_none() {
            self.ensure();
        }
        true
    }

    /// What has been loaded of the document's images (the RSS examples).
    pub fn image_stats(&self) -> crate::image::Stats {
        self.images.stats()
    }

    /// The image whose placeholder or box is at window point (`x`, `y`), if any.
    fn image_at(&self, x: f64, y: f64) -> Option<u32> {
        if self.message.is_some() || !self.in_area(x, y) {
            return None;
        }
        let hit = self
            .layout
            .hit(self.doc_x(x) as f32, self.doc_y(y) as f32)?;
        let block = &self.doc.blocks[hit.block as usize];
        let cells = self.doc.row_cells(block).map(|(_, spans)| spans);
        std::iter::once(self.doc.block_spans(block))
            .chain(cells)
            .flatten()
            .find(|s| s.image != 0 && (s.start..s.end.max(s.start + 1)).contains(&hit.doc))
            .map(|s| s.image)
    }

    // ---- Documents ------------------------------------------------------------------------

    /// Show a different document (or a message), from the top.
    pub fn open(&mut self, doc: Doc, message: Option<String>) {
        self.open_doc(doc, message);
    }

    /// [`View::open`] for a document in directory `base` (see [`View::set_image_base`]), which
    /// its images are attached to in one go.
    pub fn open_in(&mut self, doc: Doc, message: Option<String>, base: Option<PathBuf>) {
        self.images.set_base(base);
        self.open_doc(doc, message);
    }

    fn open_doc(&mut self, doc: Doc, message: Option<String>) {
        self.doc = share(doc);
        self.message = message;
        self.layout = Layout::new(&self.doc, self.layout_params());
        self.anchor = Anchor::TOP;
        self.drag = None;
        #[cfg(feature = "toc")]
        self.forget_toc();
        #[cfg(feature = "find")]
        self.find_again();
        #[cfg(feature = "clipboard")]
        {
            self.clear_selection();
            self.forget_copy();
        }
        self.kept = None;
        self.hover = None;
        self.hover_blocked = false;
        self.pressed = None;
        // Remote images are off again for another document.
        self.images.forget_remote();
        self.attach_images();
        self.announced = false;
        self.pulse = None;
        self.toast = None;
        self.notice = None;
    }

    /// Show `text` as the document's one-line notice from its next frame on, for a few seconds
    /// (see [`large_notice`]). Opening another document takes it away.
    pub fn set_notice(&mut self, text: Option<String>) {
        self.notice = text.map(|t| (None, t));
    }

    /// The notice showing or waiting for a frame.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|n| n.1.as_str())
    }

    /// Show a new version of the document (reload) or a message in its place. The position
    /// follows the nearest heading at or above it (found by its path of heading texts, then
    /// its slug), so edits above it don't move the text being read; without one
    /// (or if the heading is gone) it keeps the block index. A message keeps the position of
    /// the document it replaced for the next one.
    pub fn replace(&mut self, doc: Doc, message: Option<String>) {
        let (place, old) = self
            .kept
            .take()
            .unwrap_or_else(|| (self.place(), self.anchor));
        // Link ids belong to the old document, and so do the offsets of a selection.
        self.hover = None;
        self.hover_blocked = false;
        self.pressed = None;
        #[cfg(feature = "clipboard")]
        {
            self.clear_selection();
            self.forget_copy();
        }
        self.doc = share(doc);
        self.message = message;
        self.layout = Layout::new(&self.doc, self.layout_params());
        #[cfg(feature = "toc")]
        self.refresh_toc();
        // A reload keeps remote images on.
        self.attach_images();
        self.anchor = Anchor::TOP;
        // The matches are those of the new text, and the current one is the first from where the
        // position ends up: nothing scrolls.
        if self.message.is_some() {
            self.kept = Some((place, old));
            #[cfg(feature = "find")]
            self.find_again();
            return;
        }
        let n = self.doc.blocks.len();
        if n == 0 {
            #[cfg(feature = "find")]
            self.find_again();
            return;
        }
        let found = place.and_then(|p| {
            let h = self.locate(&p)?;
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
        #[cfg(feature = "find")]
        self.find_again();
    }

    /// Where the reading position is relative to the heading at or above it.
    fn place(&self) -> Option<Place> {
        if self.message.is_some() {
            return None;
        }
        let at = self.anchor.block;
        let (i, h) = self
            .doc
            .headings
            .iter()
            .enumerate()
            .filter(|(_, h)| h.block <= at)
            .max_by_key(|(_, h)| h.block)?;
        let paths = heading_paths(&self.doc);
        // A bare heading has no slug to find it by (see `Heading::bare`).
        let slug = if h.bare {
            String::new()
        } else {
            h.slug.clone()
        };
        let ordinal = paths[..i].iter().filter(|p| **p == paths[i]).count();
        let of = ordinal + paths[i..].iter().filter(|p| **p == paths[i]).count();
        Some(Place {
            path: paths[i].clone(),
            slug,
            ordinal,
            of,
            block: h.block,
            offset: at - h.block,
            frac: self.anchor.frac,
        })
    }

    /// The heading of the new document that `p` was at or below: the one with the same path;
    /// among several with it, the same ordinal if there are as many as before, else the
    /// nearest to the old block. Failing that, the same slug.
    fn locate(&self, p: &Place) -> Option<&Heading> {
        let paths = heading_paths(&self.doc);
        let same: Vec<&Heading> = self
            .doc
            .headings
            .iter()
            .zip(&paths)
            .filter(|(_, path)| **path == p.path)
            .map(|(h, _)| h)
            .collect();
        let by_path = if same.len() == p.of {
            same.get(p.ordinal).copied()
        } else {
            same.into_iter().min_by_key(|h| h.block.abs_diff(p.block))
        };
        // A bare heading is no match for a slug, nor is it a slug to match (see `place`).
        by_path.or_else(|| {
            let slug = Some(p.slug.as_str()).filter(|s| !s.is_empty())?;
            self.doc.headings.iter().find(|h| !h.bare && h.slug == slug)
        })
    }
}

/// The reading position as a heading, blocks below it, and a fraction of that block.
struct Place {
    /// The texts of the heading and of its ancestors, outermost first.
    path: Vec<String>,
    slug: String,
    /// Which of the `of` headings with this path it is.
    ordinal: usize,
    of: usize,
    /// The heading's block in the document it was taken from.
    block: u32,
    offset: u32,
    frac: f32,
}

/// For each heading, the texts of it and of its ancestors (the nearest preceding heading of
/// each lower level), outermost first. A heading split into chunks is its first chunk's text.
fn heading_paths(doc: &Doc) -> Vec<Vec<String>> {
    let mut stack: Vec<(u8, String)> = Vec::new();
    doc.headings
        .iter()
        .map(|h| {
            while stack.last().is_some_and(|(level, _)| *level >= h.level) {
                stack.pop();
            }
            let spans = doc.block_spans(&doc.blocks[h.block as usize]);
            stack.push((h.level, spans.iter().map(|s| doc.span_text(s)).collect()));
            stack.iter().map(|(_, text)| text.clone()).collect()
        })
        .collect()
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

/// Layout parameters for a window `width` wide at `hidpi` and `zoom` percent, in wide layout or
/// not.
fn params(
    width: u32,
    hidpi: f64,
    zoom: u16,
    (font_size, line_width): (f32, f32),
    wide: bool,
) -> Params {
    Params {
        width: width as f32,
        scale: (hidpi * f64::from(zoom) / 100.0) as f32,
        font_size,
        line_width,
        wide,
        page_pad: PAGE_PAD,
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
