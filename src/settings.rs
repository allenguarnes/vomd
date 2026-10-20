//! The settings file: flat `key = value` lines, `#` comments, no sections,
//! no quoting. Hand-parsed, forgiving (unknown keys, bad values and garbage lines are skipped),
//! and written back line by line so comments, ordering and unknown keys survive.

use std::ffi::OsString;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::view::ThemeMode;

/// Saves wait this long after the last change.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);

/// A save that failed is tried again after this long.
pub const RETRY_DELAY: Duration = Duration::from_secs(5);

/// One line of the file, kept verbatim. `kv` locates the key and value in `raw`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    raw: String,
    kv: Option<(Range<usize>, Range<usize>)>,
}

impl Line {
    fn parse(raw: String) -> Line {
        let kv = split(&raw);
        Line { raw, kv }
    }

    fn key(&self) -> Option<&str> {
        self.kv.as_ref().map(|(k, _)| &self.raw[k.clone()])
    }

    /// Keys match regardless of ASCII case (`Theme = dark` is `theme`).
    fn is_key(&self, key: &str) -> bool {
        self.key().is_some_and(|k| k.eq_ignore_ascii_case(key))
    }

    fn value(&self) -> Option<&str> {
        self.kv.as_ref().map(|(_, v)| &self.raw[v.clone()])
    }
}

/// Key and value spans of a `key = value` line; `None` for blanks, comments and garbage. A `#`
/// only starts a comment at the start of a line, so values (paths) may contain one. A byte order
/// mark in front of the line is skipped (editors add one to the file's first line).
fn split(raw: &str) -> Option<(Range<usize>, Range<usize>)> {
    let start = if raw.starts_with('\u{feff}') { 3 } else { 0 };
    let trimmed = raw[start..].trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
        return None;
    }
    let eq = raw.find('=')?;
    let key = trim_span(raw, start..eq);
    let value = trim_span(raw, eq + 1..raw.len());
    let k = &raw[key.clone()];
    if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    Some((key, value))
}

/// `range` of `s` without surrounding whitespace (an empty range at its end if all blank).
fn trim_span(s: &str, range: Range<usize>) -> Range<usize> {
    let part = &s[range.clone()];
    let start = range.start + (part.len() - part.trim_start().len());
    let end = range.end - (part.len() - part.trim_end().len());
    if start >= end { end..end } else { start..end }
}

/// The parsed file. Later lines win when a key repeats, and writes go to that line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    lines: Vec<Line>,
    /// The file used CRLF line ends (kept when it is written back).
    crlf: bool,
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        Settings {
            lines: text.lines().map(|l| Line::parse(l.to_owned())).collect(),
            crlf: text
                .split_once('\n')
                .is_some_and(|(first, _)| first.ends_with('\r')),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines
            .iter()
            .rev()
            .find(|l| l.is_key(key))
            .and_then(Line::value)
    }

    /// Set `key` to `value`: rewrite the value on its (last) line, keeping the rest of the line
    /// (and the key's spelling), or replace the template's commented default (`# key = value`),
    /// or append a line. Returns false if the value was already `value`.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        // Values are ours (numbers, words, paths checked by `FontPaths::value`): one line, no
        // surrounding blanks, so they read back exactly.
        debug_assert!(!value.contains(['\n', '\r']) && value.trim() == value);
        if let Some(line) = self.lines.iter_mut().rev().find(|l| l.is_key(key)) {
            if line.value() == Some(value) {
                return false;
            }
            let (_, v) = line.kv.clone().expect("keyed line");
            let mut raw = String::with_capacity(line.raw.len() + value.len());
            raw.push_str(&line.raw[..v.start]);
            if v.is_empty() && !line.raw[..v.start].ends_with(' ') {
                raw.push(' ');
            }
            raw.push_str(value);
            raw.push_str(&line.raw[v.end..]);
            *line = Line::parse(raw);
            return true;
        }
        let new = Line::parse(format!("{key} = {value}"));
        // Only the template's own lines: a comment of the user's stays where it is.
        let commented = self.lines.iter_mut().find(|l| {
            let Some(rest) = l.raw.strip_prefix("# ") else {
                return false;
            };
            split(rest).is_some_and(|(k, _)| rest[k].eq_ignore_ascii_case(key))
                && TEMPLATE.lines().any(|t| t == l.raw)
        });
        match commented {
            Some(line) => *line = new,
            None => self.lines.push(new),
        }
        true
    }

    /// Set `key` unless doing so changes nothing: when the key is absent and `value` is what the
    /// default already gives, the file stays as it is (written only on real change).
    pub fn set_unless_default(&mut self, key: &str, value: &str, default: &str) -> bool {
        if self.get(key).is_none() && value == default {
            return false;
        }
        self.set(key, value)
    }

    /// The file's text: every line as read (or rewritten), newline-terminated.
    pub fn text(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        for l in &self.lines {
            out.push_str(&l.raw);
            out.push_str(eol);
        }
        out
    }
}

// ---- Typed preferences ----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteImages {
    Ask,
    Never,
    Always,
}

/// The settings VOMD understands, with defaults for anything missing or invalid.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub theme: ThemeMode,
    pub zoom: u16,
    pub toolbar: bool,
    pub toc: bool,
    /// Logical pixels.
    pub window: (u32, u32),
    pub remote_images: RemoteImages,
    pub font_size: f32,
    pub line_width: f32,
    /// Wide layout: the text spans the window instead of stopping at `line_width`.
    pub wide: bool,
    /// System family names; `None` = the embedded faces.
    pub font_sans: Option<String>,
    pub font_mono: Option<String>,
    /// Faces cached for the families above.
    pub font_sans_path: Option<FontPaths>,
    pub font_mono_path: Option<FontPaths>,
    /// Selecting text offers it as the primary selection (pasted with the middle button).
    pub primary_selection: bool,
}

/// What `primary_selection` is without a line for it: on where there is a primary selection (X11
/// and Wayland). Windows and macOS have none and will say off, here and in [`TEMPLATE`].
pub const DEFAULT_PRIMARY: bool = cfg!(all(unix, not(any(target_os = "macos", target_os = "ios"))));

pub const DEFAULT_WINDOW: (u32, u32) = (900, 1000);
pub const ZOOM_RANGE: (u16, u16) = (50, 300);
const WINDOW_RANGE: (u32, u32) = (200, 16384);
const FONT_SIZE_RANGE: (f32, f32) = (10.0, 32.0);
const LINE_WIDTH_RANGE: (f32, f32) = (40.0, 200.0);

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            theme: ThemeMode::System,
            zoom: 100,
            toolbar: true,
            toc: false,
            window: DEFAULT_WINDOW,
            remote_images: RemoteImages::Ask,
            font_size: 16.0,
            line_width: 80.0,
            wide: false,
            font_sans: None,
            font_mono: None,
            font_sans_path: None,
            font_mono_path: None,
            primary_selection: DEFAULT_PRIMARY,
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// A number within `range`, or `None` (out-of-range values are skipped, not clamped: they are
/// treated like any other bad value).
fn parse_in<T: std::str::FromStr + PartialOrd>(v: &str, (lo, hi): (T, T)) -> Option<T> {
    v.parse::<T>().ok().filter(|n| *n >= lo && *n <= hi)
}

fn parse_family(v: &str) -> Option<Option<String>> {
    match v {
        "" => None,
        d if d.eq_ignore_ascii_case("default") => Some(None),
        name => Some(Some(name.to_owned())),
    }
}

impl Prefs {
    pub fn from_settings(s: &Settings) -> Prefs {
        let d = Prefs::default();
        let theme = s
            .get("theme")
            .and_then(|v| match v.to_ascii_lowercase().as_str() {
                "system" => Some(ThemeMode::System),
                "light" => Some(ThemeMode::Light),
                "dark" => Some(ThemeMode::Dark),
                _ => None,
            });
        let remote = s
            .get("remote_images")
            .and_then(|v| match v.to_ascii_lowercase().as_str() {
                "ask" => Some(RemoteImages::Ask),
                "never" => Some(RemoteImages::Never),
                "always" => Some(RemoteImages::Always),
                _ => None,
            });
        let font_size = s
            .get("font_size")
            .and_then(|v| parse_in::<f32>(v, FONT_SIZE_RANGE));
        let line_width = s
            .get("line_width")
            .and_then(|v| parse_in::<f32>(v, LINE_WIDTH_RANGE));
        let font_sans = s.get("font_sans").and_then(parse_family).unwrap_or(None);
        let font_mono = s.get("font_mono").and_then(parse_family).unwrap_or(None);
        Prefs {
            theme: theme.unwrap_or(d.theme),
            zoom: s
                .get("zoom")
                .and_then(|v| parse_in(v, ZOOM_RANGE))
                .unwrap_or(d.zoom),
            toolbar: s.get("toolbar").and_then(parse_bool).unwrap_or(d.toolbar),
            toc: s.get("toc").and_then(parse_bool).unwrap_or(d.toc),
            window: (
                s.get("window_width")
                    .and_then(|v| parse_in(v, WINDOW_RANGE))
                    .unwrap_or(d.window.0),
                s.get("window_height")
                    .and_then(|v| parse_in(v, WINDOW_RANGE))
                    .unwrap_or(d.window.1),
            ),
            remote_images: remote.unwrap_or(d.remote_images),
            font_size: font_size.unwrap_or(d.font_size),
            line_width: line_width.unwrap_or(d.line_width),
            wide: s.get("wide").and_then(parse_bool).unwrap_or(d.wide),
            font_sans_path: font_sans.as_deref().and_then(|f| {
                FontPaths::parse(s.get("font_sans_path")?).filter(|p| p.family_is(f))
            }),
            font_mono_path: font_mono.as_deref().and_then(|f| {
                FontPaths::parse(s.get("font_mono_path")?).filter(|p| p.family_is(f))
            }),
            font_sans,
            font_mono,
            primary_selection: s
                .get("primary_selection")
                .and_then(parse_bool)
                .unwrap_or(d.primary_selection),
        }
    }
}

pub fn theme_value(t: ThemeMode) -> &'static str {
    match t {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

// ---- Cached font paths ------------------------------------------------------------------------

/// The files a family resolved to, cached in `font_sans_path` / `font_mono_path` so later
/// launches skip the system font scan. Written as
/// `Family | regular | bold | italic | bold italic`; each face is a path, optionally followed by
/// `@N` for a face in a collection; a style the family lacks is left empty (and synthesized).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontPaths {
    pub family: String,
    /// Regular, bold, italic, bold italic.
    pub faces: [Option<(PathBuf, u32)>; 4],
}

impl FontPaths {
    pub fn parse(v: &str) -> Option<FontPaths> {
        let mut parts = v.split('|').map(str::trim);
        let family = parts.next().filter(|f| !f.is_empty())?.to_owned();
        let mut faces: [Option<(PathBuf, u32)>; 4] = Default::default();
        for face in &mut faces {
            *face = parts
                .next()
                .filter(|p| !p.is_empty())
                .map(|p| match p.rsplit_once('@') {
                    Some((path, n)) if !path.is_empty() => match n.parse::<u32>() {
                        Ok(i) => (PathBuf::from(path), i),
                        Err(_) => (PathBuf::from(p), 0),
                    },
                    _ => (PathBuf::from(p), 0),
                });
        }
        // The regular face is required; extra fields are ignored (a newer format).
        faces[0].as_ref()?;
        Some(FontPaths { family, faces })
    }

    /// The setting's value, or `None` if the family or a path can't be written on one line.
    pub fn value(&self) -> Option<String> {
        // A name with a `|` would read back as a shorter family (and never match again).
        if self.family.contains(['|', '\n', '\r']) {
            return None;
        }
        let mut out = self.family.clone();
        for face in &self.faces {
            out.push_str(" | ");
            if let Some((path, index)) = face {
                let s = path.to_str()?;
                if s.contains(['|', '\n', '\r']) || s.trim() != s {
                    return None;
                }
                out.push_str(s);
                // A trailing `@digits` in the file name itself must not read as an index.
                if *index != 0
                    || s.rsplit_once('@')
                        .is_some_and(|(_, n)| n.parse::<u32>().is_ok())
                {
                    out.push_str(&format!("@{index}"));
                }
            }
        }
        // Missing styles at the end leave a trailing blank, which wouldn't read back.
        Some(out.trim_end().to_owned())
    }

    pub fn family_is(&self, family: &str) -> bool {
        self.family.eq_ignore_ascii_case(family.trim())
    }

    /// Every cached file still exists (otherwise re-resolve).
    pub fn exists(&self) -> bool {
        self.faces.iter().flatten().all(|(p, _)| p.is_file())
    }
}

// ---- File location and I/O ----------------------------------------------------------------

/// Where the settings file lives, from environment variables (no `dirs` crate). `None` if the
/// variables needed are unset.
pub fn path() -> Option<PathBuf> {
    path_from(|k| std::env::var_os(k))
}

pub fn path_from(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let nonempty = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = nonempty("XDG_CONFIG_HOME")
        .filter(|p| p.is_absolute())
        .or_else(|| nonempty("HOME").map(|h| h.join(".config")));
    #[cfg(target_os = "macos")]
    // TODO(P14): untested.
    let base = nonempty("HOME").map(|h| h.join("Library/Application Support"));
    #[cfg(windows)]
    // TODO(P14): untested.
    let base = nonempty("APPDATA");
    Some(base?.join("vomd").join("settings.ini"))
}

/// Modification time and size, to notice edits on focus (live apply).
pub type Stamp = (Option<SystemTime>, u64);

pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok(), m.len()))
}

/// Settings files beyond this are not ours (or not settings); ignore them.
const MAX_FILE: u64 = 256 * 1024;

/// The file as read.
#[derive(Debug, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    pub stamp: Option<Stamp>,
    /// False if the file exists but couldn't be read as it is (unreadable, oversized, not UTF-8):
    /// what could be parsed still applies, but writing it back would destroy the rest.
    pub faithful: bool,
}

/// Read the file; a missing one reads as empty (all defaults).
pub fn load(path: &Path) -> Loaded {
    let stamp = stamp(path);
    let (text, faithful) = match std::fs::metadata(path) {
        // A file in the way of the directory (`NotADirectory`) leaves nothing to destroy either.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            (String::new(), true)
        }
        Ok(m) if m.len() <= MAX_FILE => match std::fs::read(path).map(String::from_utf8) {
            Ok(Ok(text)) => (text, true),
            Ok(Err(e)) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), false),
            Err(_) => (String::new(), false),
        },
        _ => (String::new(), false),
    };
    Loaded {
        settings: Settings::parse(&text),
        stamp,
        faithful,
    }
}

/// The file a settings path really is: a symlink (a dotfiles setup) is followed, also when it
/// dangles, so saving replaces the target and not the link.
fn resolve(path: &Path) -> PathBuf {
    let mut p = path.to_owned();
    for _ in 0..8 {
        let Ok(link) = std::fs::read_link(&p) else {
            break;
        };
        p = p.parent().map_or(link.clone(), |d| d.join(&link));
    }
    p
}

/// The file has no write permission (a config frozen on purpose): saving would replace it all
/// the same, so it is left alone.
fn frozen(path: &Path) -> bool {
    std::fs::metadata(resolve(path)).is_ok_and(|m| m.permissions().readonly())
}

/// Write atomically: a temporary file next to the (resolved) file, with its permissions, then
/// rename over it. A read-only file is not replaced (`PermissionDenied`). Returns the new stamp.
pub fn save(path: &Path, settings: &Settings) -> io::Result<Option<Stamp>> {
    if frozen(path) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let target = resolve(path);
    let dir = target.parent().ok_or(io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        if let Ok(m) = std::fs::metadata(&target) {
            f.set_permissions(m.permissions())?;
        }
        f.write_all(settings.text().as_bytes())
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, &target)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(stamp(path))
}

/// Debounced saving: changes mark the settings dirty, and they are written [`SAVE_DELAY`] after
/// the last one, or at once on close.
#[derive(Clone, Copy, Debug, Default)]
pub struct Saver {
    due: Option<Instant>,
}

impl Saver {
    pub fn mark(&mut self, now: Instant) {
        self.due = Some(now + SAVE_DELAY);
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.due
    }

    pub fn dirty(&self) -> bool {
        self.due.is_some()
    }

    /// True (and clean again) if a save is due at `now`.
    pub fn take_due(&mut self, now: Instant) -> bool {
        if self.due.is_some_and(|t| now >= t) {
            self.due = None;
            return true;
        }
        false
    }

    /// True (and clean again) if anything is waiting to be saved, due or not.
    pub fn take(&mut self) -> bool {
        self.due.take().is_some()
    }

    /// A save failed: try again after [`RETRY_DELAY`] (or sooner, with the next change).
    pub fn retry(&mut self, now: Instant) {
        self.due = Some(now + RETRY_DELAY);
    }
}

/// The settings as the app uses them: the file (if any), what was last applied from it, and
/// changes waiting to be saved. Without a path (`--no-settings`, or no config directory) it
/// gives the defaults and never touches the disk.
///
/// Saving never overwrites the file with a stale copy: changes are kept as `(key, value)`
/// pending writes, and a save first reads the file again (whatever its stamp says); if it
/// changed since it was read (another window, a hand edit) its lines are taken as they are and
/// the pending keys go on top, so for each key the last write wins and nothing else is lost.
#[derive(Debug, Default)]
pub struct Store {
    path: Option<PathBuf>,
    settings: Settings,
    stamp: Option<Stamp>,
    saver: Saver,
    /// Changes not written yet, one per key.
    pending: Vec<(String, String)>,
    /// The file couldn't be read faithfully, or has no write permission: used as far as it
    /// parsed, never written.
    read_only: bool,
    /// A save found changes from outside and merged them in; the next [`Store::refresh`] reports
    /// them though the file's stamp is ours.
    foreign: bool,
    /// The last save failed (and was reported); retries stay quiet until one works.
    failing: bool,
    /// Theme and zoom as last read from or written to the file: only a change from these is
    /// written, so a bad value in the file isn't overwritten just because the view shows the
    /// default.
    synced: (ThemeMode, u16),
}

impl Store {
    pub fn open(path: Option<PathBuf>) -> Store {
        let mut store = Store {
            path,
            ..Store::default()
        };
        if let Some(path) = &store.path {
            let loaded = load(path);
            store.adopt(loaded);
        }
        store
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn prefs(&self) -> Prefs {
        Prefs::from_settings(&self.settings)
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// `loaded` can't be written back (see `read_only`).
    fn locked(&self, loaded: &Loaded) -> bool {
        !loaded.faithful || self.path.as_deref().is_some_and(frozen)
    }

    /// Take `loaded` as the file's state, with the pending changes still on top (those the file
    /// already has are dropped).
    fn adopt(&mut self, loaded: Loaded) {
        self.read_only = self.locked(&loaded);
        self.settings = loaded.settings;
        self.stamp = loaded.stamp;
        let settings = &mut self.settings;
        if self.read_only {
            self.pending.clear();
        }
        self.pending.retain(|(k, v)| settings.get(k) != Some(v));
        for (k, v) in &self.pending {
            settings.set(k, v);
        }
        if self.pending.is_empty() {
            self.saver = Saver::default();
        }
        let p = self.prefs();
        self.synced = (p.theme, p.zoom);
    }

    /// Set `key` to `value` (not theme and zoom: [`Store::record`]); a change is saved after
    /// [`SAVE_DELAY`]. For the toolbar and panel toggles.
    pub fn set(&mut self, key: &str, value: &str, now: Instant) {
        if self.settings.set(key, value) {
            self.track(key, value, now);
        }
    }

    /// Queue a change for saving.
    fn track(&mut self, key: &str, value: &str, now: Instant) {
        if self.path.is_none() || self.read_only {
            return;
        }
        self.pending.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
        self.pending.push((key.to_owned(), value.to_owned()));
        self.saver.mark(now);
    }

    /// The view's theme and zoom now; a change is saved after [`SAVE_DELAY`].
    pub fn record(&mut self, theme: ThemeMode, zoom: u16, now: Instant) {
        if (theme, zoom) == self.synced {
            return;
        }
        if theme != self.synced.0 {
            self.set("theme", theme_value(theme), now);
        }
        if zoom != self.synced.1 {
            self.set("zoom", &zoom.to_string(), now);
        }
        self.synced = (theme, zoom);
    }

    /// The window's size in logical pixels at close: recorded if it differs from what the file
    /// gives (saved by the [`Store::flush`] that follows).
    pub fn record_window(&mut self, (w, h): (u32, u32), now: Instant) {
        let (w, h) = (
            w.clamp(WINDOW_RANGE.0, WINDOW_RANGE.1),
            h.clamp(WINDOW_RANGE.0, WINDOW_RANGE.1),
        );
        if (w, h) == self.prefs().window {
            return;
        }
        let d = DEFAULT_WINDOW;
        for (key, v, default) in [("window_width", w, d.0), ("window_height", h, d.1)] {
            let v = v.to_string();
            if self
                .settings
                .set_unless_default(key, &v, &default.to_string())
            {
                self.track(key, &v, now);
            }
        }
    }

    /// Remember the files a font family resolved to (`font_sans_path` / `font_mono_path`).
    pub fn record_fonts(&mut self, mono: bool, paths: &FontPaths, now: Instant) {
        let key = if mono {
            "font_mono_path"
        } else {
            "font_sans_path"
        };
        if let Some(v) = paths.value() {
            self.set(key, &v, now);
        }
    }

    /// When the loop must wake to save.
    pub fn deadline(&self) -> Option<Instant> {
        self.saver.deadline()
    }

    /// Save if a save is due at `now`.
    pub fn poll(&mut self, now: Instant) -> io::Result<()> {
        if self.saver.take_due(now) {
            return self.write(now);
        }
        Ok(())
    }

    /// Save now if anything is waiting (on close, and when the window loses focus).
    pub fn flush(&mut self) -> io::Result<()> {
        if self.saver.take() {
            return self.write(Instant::now());
        }
        Ok(())
    }

    /// Write the pending changes. A failure keeps them pending and schedules a retry; only the
    /// first of a run of failures is returned.
    fn write(&mut self, now: Instant) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        // Always read the file again, not only when its stamp changed: an edit of the same size
        // within the timestamp's granularity looks like no edit at all.
        let (before, changed) = (self.prefs(), stamp(&path) != self.stamp);
        self.adopt(load(&path));
        if changed || self.prefs() != before {
            self.foreign = true;
        }
        // Made read-only since it was read (the stamp doesn't show a chmod).
        if !self.read_only && frozen(&path) {
            self.read_only = true;
            self.pending.clear();
        }
        if self.pending.is_empty() {
            return Ok(());
        }
        match save(&path, &self.settings) {
            Ok(stamp) => {
                self.stamp = stamp;
                self.pending.clear();
                self.failing = false;
                Ok(())
            }
            Err(e) => {
                self.saver.retry(now);
                if std::mem::replace(&mut self.failing, true) {
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    /// The preferences if a save merged changes from outside into the file (they are not in the
    /// view yet), once; so the next [`Store::record`] doesn't write the old values back.
    pub fn take_merged(&mut self) -> Option<Prefs> {
        std::mem::take(&mut self.foreign).then(|| self.prefs())
    }

    /// Re-read the file if it changed on disk since it was last read or written (on focus;
    /// live apply). Returns the new preferences then. Changes not saved yet stay on
    /// top of what the file says (last write wins, as with two windows, and a failed save isn't
    /// lost); they are saved as usual. A read-only store looks at the file every time: a chmod
    /// doesn't change the stamp.
    pub fn refresh(&mut self) -> Option<Prefs> {
        let path = self.path.clone()?;
        let merged = self.take_merged();
        let changed = stamp(&path) != self.stamp;
        if !changed && !self.read_only {
            return merged;
        }
        let loaded = load(&path);
        // Still read-only and the same file: what was changed in this window stays.
        if !changed && self.locked(&loaded) {
            return merged;
        }
        self.adopt(loaded);
        Some(self.prefs())
    }

    /// The ⚙ action's file, for the default editor: save what is pending and write the
    /// commented template if there is no file. `Unsupported` without a settings file
    /// (`--no-settings`, no config directory: `path()` is `None`, which disables the button).
    pub fn editor_file(&mut self) -> io::Result<PathBuf> {
        self.prepare_file()?;
        self.path
            .clone()
            .ok_or_else(|| io::ErrorKind::Unsupported.into())
    }

    /// The file part of [`Store::editor_file`]: a file made here is the template with the
    /// pending changes on top (not a file of just those), and what is pending is written.
    fn prepare_file(&mut self) -> io::Result<()> {
        let path = self.path.clone().ok_or(io::ErrorKind::Unsupported)?;
        if !self.read_only && ensure_template(&path)? {
            self.adopt(load(&path));
        }
        self.flush()
    }
}

/// The template with the default of `primary_selection` the platform has (see
/// [`DEFAULT_PRIMARY`]): `on` or `off`.
macro_rules! template {
    ($primary:literal) => {
        concat!(
            "\
# VOMD settings. Remove the # in front of a line to change it; delete a line to go back to
# the default. VOMD re-reads this file when its window gets focus. Comments go on a line of
# their own: after a value, a # is part of the value.

# Color theme: system, light or dark (T cycles it).
# theme = system

# Zoom in percent, 50 to 300 (Ctrl + = / - / 0).
# zoom = 100

# Show the toolbar (B toggles it).
# toolbar = true

# Show the table of contents panel (C toggles it).
# toc = false

# Window size in logical pixels, saved when the window closes.
# window_width = 900
# window_height = 1000

# Remote (http/https) images: ask (load with I), never, or always. With always, every
# document you open contacts the servers its images are on.
# remote_images = ask

# Base font size in pixels at 100% zoom, 10 to 32.
# font_size = 16

# Maximum characters per line, 40 to 200.
# line_width = 80

# Wide layout: the text spans the window instead of stopping at line_width (W toggles it).
# wide = false

# Selecting text with the mouse (or Ctrl+A) offers it as the primary selection, which a middle
# click pastes elsewhere: on or off. Ctrl+C copies to the clipboard either way.
# primary_selection = ",
            $primary,
            "

# Fonts: default (built in) or the name of an installed family, e.g. Noto Sans. VOMD finds
# the files once and remembers them below as font_sans_path / font_mono_path. A family that
# isn't installed falls back to the built-in font.
# font_sans = default
# font_mono = default
"
        )
    };
}

/// The file written by the ⚙ action when there is none: every key commented out at its
/// default, with a line of explanation.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
pub const TEMPLATE: &str = template!("on");
#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
pub const TEMPLATE: &str = template!("off");

/// Create the file from [`TEMPLATE`] if it doesn't exist (a dangling symlink gets the file at
/// its target). Returns true if it was created.
pub fn ensure_template(path: &Path) -> io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(_) => return Ok(false),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        Err(_) => {}
    }
    save(path, &Settings::parse(TEMPLATE))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
