//! The settings file: flat `key = value` lines, `#` comments, no sections,
//! no quoting. Hand-parsed, forgiving (unknown keys, bad values and garbage lines are skipped),
//! and written back line by line so comments, ordering and unknown keys survive.

use std::ffi::OsString;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::view::ThemeMode;

/// Saves wait this long after the last change.
pub const SAVE_DELAY: Duration = Duration::from_millis(500);

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

    fn value(&self) -> Option<&str> {
        self.kv.as_ref().map(|(_, v)| &self.raw[v.clone()])
    }
}

/// Key and value spans of a `key = value` line; `None` for blanks, comments and garbage. A `#`
/// only starts a comment at the start of a line, so values (paths) may contain one.
fn split(raw: &str) -> Option<(Range<usize>, Range<usize>)> {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
        return None;
    }
    let eq = raw.find('=')?;
    let key = trim_span(raw, 0..eq);
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
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        Settings {
            lines: text.lines().map(|l| Line::parse(l.to_owned())).collect(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines
            .iter()
            .rev()
            .find(|l| l.key() == Some(key))
            .and_then(Line::value)
    }

    /// Set `key` to `value`: rewrite the value on its (last) line, keeping the rest of the line,
    /// or append a line. Returns false if the value was already `value`.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        // Values are ours (numbers, words, paths checked by `FontPaths::value`): one line, no
        // surrounding blanks, so they read back exactly.
        debug_assert!(!value.contains(['\n', '\r']) && value.trim() == value);
        if let Some(line) = self.lines.iter_mut().rev().find(|l| l.key() == Some(key)) {
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
        self.lines.push(Line::parse(format!("{key} = {value}")));
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
        let mut out = String::new();
        for l in &self.lines {
            out.push_str(&l.raw);
            out.push('\n');
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
    /// System family names; `None` = the embedded faces.
    pub font_sans: Option<String>,
    pub font_mono: Option<String>,
    /// Faces cached for the families above.
    pub font_sans_path: Option<FontPaths>,
    pub font_mono_path: Option<FontPaths>,
}

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
            font_sans: None,
            font_mono: None,
            font_sans_path: None,
            font_mono_path: None,
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
            .and_then(|v| parse_in::<f32>(v, FONT_SIZE_RANGE))
            .filter(|n| n.is_finite());
        let line_width = s
            .get("line_width")
            .and_then(|v| parse_in::<f32>(v, LINE_WIDTH_RANGE))
            .filter(|n| n.is_finite());
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
            font_sans_path: font_sans.as_deref().and_then(|f| {
                FontPaths::parse(s.get("font_sans_path")?).filter(|p| p.family_is(f))
            }),
            font_mono_path: font_mono.as_deref().and_then(|f| {
                FontPaths::parse(s.get("font_mono_path")?).filter(|p| p.family_is(f))
            }),
            font_sans,
            font_mono,
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

    /// The setting's value, or `None` if a path can't be written on one line.
    pub fn value(&self) -> Option<String> {
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

/// Read the file: missing, unreadable or oversized files read as empty (all defaults).
pub fn load(path: &Path) -> (Settings, Option<Stamp>) {
    let st = stamp(path);
    if st.is_none_or(|(_, len)| len > MAX_FILE) {
        return (Settings::default(), st);
    }
    let text = std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    (Settings::parse(&text), st)
}

/// Write atomically: a temporary file next to it, then rename over it. Returns the new stamp.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<Option<Stamp>> {
    let dir = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, settings.text())?;
    if let Err(e) = std::fs::rename(&tmp, path) {
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
}

/// The settings as the app uses them: the file (if any), what was last applied from it, and
/// changes waiting to be saved. Without a path (`--no-settings`, or no config directory) it
/// gives the defaults and never touches the disk.
#[derive(Debug, Default)]
pub struct Store {
    path: Option<PathBuf>,
    settings: Settings,
    stamp: Option<Stamp>,
    saver: Saver,
    /// Theme and zoom as last read from or written to the file: only a change from these is
    /// written, so a bad value in the file isn't overwritten just because the view shows the
    /// default.
    synced: (ThemeMode, u16),
}

impl Store {
    pub fn open(path: Option<PathBuf>) -> Store {
        let (settings, stamp) = path.as_deref().map(load).unwrap_or_default();
        let mut store = Store {
            path,
            settings,
            stamp,
            ..Store::default()
        };
        store.resync();
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

    fn resync(&mut self) {
        let p = self.prefs();
        self.synced = (p.theme, p.zoom);
    }

    /// The view's theme and zoom now; a change is saved after [`SAVE_DELAY`].
    pub fn record(&mut self, theme: ThemeMode, zoom: u16, now: Instant) {
        if (theme, zoom) == self.synced {
            return;
        }
        if theme != self.synced.0 {
            self.settings.set("theme", theme_value(theme));
        }
        if zoom != self.synced.1 {
            self.settings.set("zoom", &zoom.to_string());
        }
        self.synced = (theme, zoom);
        self.mark(now);
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
        let a = self
            .settings
            .set_unless_default("window_width", &w.to_string(), &d.0.to_string());
        let b = self
            .settings
            .set_unless_default("window_height", &h.to_string(), &d.1.to_string());
        if a || b {
            self.mark(now);
        }
    }

    /// Remember the files a font family resolved to (`font_sans_path` / `font_mono_path`).
    pub fn record_fonts(&mut self, mono: bool, paths: &FontPaths, now: Instant) {
        let key = if mono {
            "font_mono_path"
        } else {
            "font_sans_path"
        };
        if let Some(v) = paths.value()
            && self.settings.set(key, &v)
        {
            self.mark(now);
        }
    }

    fn mark(&mut self, now: Instant) {
        if self.path.is_some() {
            self.saver.mark(now);
        }
    }

    /// When the loop must wake to save.
    pub fn deadline(&self) -> Option<Instant> {
        self.saver.deadline()
    }

    /// Save if a save is due at `now`.
    pub fn poll(&mut self, now: Instant) -> std::io::Result<()> {
        if self.saver.take_due(now) {
            return self.write();
        }
        Ok(())
    }

    /// Save now if anything is waiting (on close).
    pub fn flush(&mut self) -> std::io::Result<()> {
        if self.saver.take() {
            return self.write();
        }
        Ok(())
    }

    fn write(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        self.stamp = save(path, &self.settings)?;
        Ok(())
    }

    /// Re-read the file if it changed on disk since it was last read or written (on focus;
    /// live apply). Returns the new preferences then. The file wins over changes not
    /// saved yet (last write wins, as with two windows).
    pub fn refresh(&mut self) -> Option<Prefs> {
        let path = self.path.as_deref()?;
        if stamp(path) == self.stamp {
            return None;
        }
        let (settings, st) = load(path);
        self.settings = settings;
        self.stamp = st;
        self.saver = Saver::default();
        self.resync();
        Some(self.prefs())
    }
}

/// The ⚙ action: write the commented template if there is no file, then open the
/// file in the default editor.
pub fn open_in_editor(path: &Path) -> std::io::Result<()> {
    ensure_template(path)?;
    crate::platform::open(path.as_os_str())
}

/// The file written by the ⚙ action when there is none: every key commented out at its
/// default, with a line of explanation.
pub const TEMPLATE: &str = "\
# VOMD settings. Remove the # in front of a line to change it; delete a line to go back to
# the default. VOMD re-reads this file when its window gets focus.

# Color theme: system, light or dark (T cycles it).
# theme = system

# Zoom in percent, 50 to 300 (Ctrl + = / - / 0).
# zoom = 100

# Show the toolbar (B toggles it).
# toolbar = true

# Show the table of contents panel (O toggles it).
# toc = false

# Window size in logical pixels, saved when the window closes.
# window_width = 900
# window_height = 1000

# Remote (http/https) images: ask (load with I), never, or always.
# remote_images = ask

# Base font size in pixels at 100% zoom, 10 to 32.
# font_size = 16

# Maximum characters per line, 40 to 200.
# line_width = 80

# Fonts: default (built in) or the name of an installed family, e.g. Noto Sans. VOMD finds
# the files once and remembers them below as font_sans_path / font_mono_path.
# font_sans = default
# font_mono = default
";

/// Create the file from [`TEMPLATE`] if it doesn't exist. Returns true if it was created.
pub fn ensure_template(path: &Path) -> std::io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    save(path, &Settings::parse(TEMPLATE))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
