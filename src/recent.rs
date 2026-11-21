//! The files opened lately, for the empty window (and, on macOS, File ▸ Open Recent and the Dock).
//!
//! The list is a file of its own, not a key of `settings.ini`: one absolute path per line, newest
//! first, after a comment. It is state, kept per machine (`~/.local/state/vomd/recent`,
//! `%LOCALAPPDATA%\vomd\recent`, next to the settings on macOS), and every window (each is its own
//! process on Linux and Windows) reads it, changes it and writes it back whole, atomically, at
//! each change. Two windows recording in the same instant may lose one line, never the file.
//! Without a settings file (`--no-settings`, no config directory) or with `recent_files = off`
//! nothing is read or written; off also deletes the file.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// How many files the list keeps, and how many the empty window shows.
pub const KEPT: usize = 20;
pub const SHOWN: usize = 8;

/// A file larger than this is not ours: only its start is read.
const MAX_FILE: u64 = 64 * 1024;

/// The comment at the top of the file.
const HEADER: &str =
    "# Files recently opened in VOMD, newest first. Delete this file to clear the list.\n";

/// Where the list lives, from environment variables (`None` if those needed are unset).
pub fn path() -> Option<PathBuf> {
    path_from(|k| std::env::var_os(k))
}

pub fn path_from(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // (A relative one would put the list under whatever folder VOMD was started in.)
    let nonempty = |k: &str| {
        let v = env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        v.filter(|p| p.is_absolute())
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let base =
        nonempty("XDG_STATE_HOME").or_else(|| nonempty("HOME").map(|h| h.join(".local/state")));
    #[cfg(target_os = "macos")]
    let base = nonempty("HOME").map(|h| h.join("Library/Application Support"));
    // Local, not roaming like the settings: the paths are this machine's.
    #[cfg(windows)]
    let base = nonempty("LOCALAPPDATA");
    Some(base?.join("vomd").join("recent"))
}

/// The paths of the file's text: absolute ones only, each once, at most [`KEPT`]. Comments,
/// blanks and anything else (a line that isn't a path, one that wasn't valid UTF-8) are skipped.
pub fn parse(text: &str) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line.contains('\u{fffd}') {
            continue;
        }
        let p = PathBuf::from(line);
        if p.is_absolute() && !list.contains(&p) {
            list.push(p);
            if list.len() == KEPT {
                break;
            }
        }
    }
    list
}

/// The file's text for `list`.
pub fn text(list: &[PathBuf]) -> String {
    let mut out = String::from(HEADER);
    for p in list.iter().filter_map(|p| p.to_str()) {
        out.push_str(p);
        out.push('\n');
    }
    out
}

/// `path` as the list keeps it: in its canonical folder (symlinks and `..` resolved, so
/// `/tmp/a.md` from a shell and `/private/tmp/a.md` from Finder are one file), or absolute if
/// that fails; and only if it can be written on a line of its own and read back the same. On
/// Windows the canonical form is a verbatim path (`\\?\C:\…`), kept as it is: a file there may
/// need it (`chapter.md.` is another file than `chapter.md`); only [`label`] shows it plainly.
pub fn storable(path: &Path) -> Option<PathBuf> {
    let p = std::path::absolute(path).ok()?;
    // The folder only: a symlinked file stays the link the reader opened (repointed, it opens
    // what it points to then).
    let p = match (p.parent(), p.file_name()) {
        (Some(dir), Some(name)) => std::fs::canonicalize(dir).map_or(p.clone(), |d| d.join(name)),
        _ => p,
    };
    let s = p.to_str()?;
    (!s.contains(['\n', '\r']) && s.trim() == s).then_some(p)
}

/// A Windows verbatim path as the reader knows it, for showing only (opening it so could reach
/// another file): a drive's (`\\?\C:\…` is `C:\…`) or a share's (`\\?\UNC\server\…` is
/// `\\server\…`). `None`: not a verbatim path of a drive or a share.
fn unverbatim(s: &str) -> Option<String> {
    let rest = s.strip_prefix(r"\\?\")?;
    if let Some(share) = rest.strip_prefix(r"UNC\") {
        return Some(format!(r"\\{share}"));
    }
    (rest.as_bytes().get(1) == Some(&b':')).then(|| rest.to_owned())
}

/// The list in `file`: empty if there is none, or it can't be read.
pub fn read(file: &Path) -> Vec<PathBuf> {
    let mut bytes = Vec::new();
    let Ok(f) = std::fs::File::open(file) else {
        return Vec::new();
    };
    if f.take(MAX_FILE + 1).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    // Cut short: the last line may be part of one.
    if bytes.len() as u64 > MAX_FILE {
        bytes.truncate(bytes.iter().rposition(|&b| b == b'\n').unwrap_or(0));
    }
    parse(&String::from_utf8_lossy(&bytes))
}

/// `list` with `path` first (and not again further down), at most [`KEPT`].
pub fn with_first(mut list: Vec<PathBuf>, path: PathBuf) -> Vec<PathBuf> {
    list.retain(|p| *p != path);
    list.insert(0, path);
    list.truncate(KEPT);
    list
}

/// The file name and the folder of `path` as a row of the list shows them: the folder with a
/// separator at its end, and the home folder (`home`) as `~`.
pub fn label(path: &Path, home: Option<&Path>) -> (String, String) {
    let plain = path.to_str().and_then(unverbatim).map(PathBuf::from);
    let path = plain.as_deref().unwrap_or(path);
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    let sep = std::path::MAIN_SEPARATOR;
    let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) else {
        return (name, String::new());
    };
    let mut folder = match home.and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{sep}{}", rest.display()),
        None => dir.display().to_string(),
    };
    if !folder.ends_with(['/', '\\']) {
        folder.push(sep);
    }
    (name, folder)
}

/// The home folder that [`label`] shows as `~`: `HOME` on Linux and macOS, none on Windows.
/// Canonical, as the folders of the list are (`/home` may be a link to `/usr/home`), found once.
pub fn home() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
        let home = HOME.get_or_init(|| {
            let h = Some(PathBuf::from(std::env::var_os("HOME")?));
            let h = h.filter(|h| h.is_absolute() && h.parent().is_some())?;
            Some(std::fs::canonicalize(&h).unwrap_or(h))
        });
        home.clone()
    }
    #[cfg(not(unix))]
    None
}

/// The titles of macOS's Open Recent items: the file names, and for a name the list has more
/// than once, its folder after it too.
pub fn menu_titles(list: &[PathBuf], home: Option<&Path>) -> Vec<String> {
    let labels: Vec<(String, String)> = list.iter().map(|p| label(p, home)).collect();
    labels
        .iter()
        .map(|(name, folder)| {
            if labels.iter().filter(|(n, _)| n == name).count() > 1 {
                format!("{name} — {folder}")
            } else {
                name.clone()
            }
        })
        .collect()
}

/// The list as the app keeps it: the file, if there is one to use, and whether the setting is on.
#[derive(Debug, Default)]
pub struct Recent {
    file: Option<PathBuf>,
    on: bool,
}

impl Recent {
    /// The list in `file` (`None`: no list, ever), on until [`Recent::set_on`] says otherwise.
    pub fn new(file: Option<PathBuf>) -> Recent {
        Recent { file, on: true }
    }

    /// The file to use: none while off.
    fn file(&self) -> Option<&Path> {
        self.file.as_deref().filter(|_| self.on)
    }

    /// The file to use (none while off), for a look at the list off the main thread.
    pub fn file_path(&self) -> Option<PathBuf> {
        self.file().map(Path::to_owned)
    }

    /// Follow the `recent_files` setting. Off deletes the file: nothing is kept.
    pub fn set_on(&mut self, on: bool) -> io::Result<()> {
        self.on = on;
        if on {
            return Ok(());
        }
        match &self.file {
            Some(file) => forget(file),
            None => Ok(()),
        }
    }

    /// Whether there is a list at all (a file, and the setting on).
    pub fn on(&self) -> bool {
        self.file().is_some()
    }

    /// Every file the list has, newest first.
    pub fn list(&self) -> Vec<PathBuf> {
        self.file().map(read).unwrap_or_default()
    }

    /// `path` was opened: it goes first. Returns false if it isn't kept (no list, a path that
    /// can't be written as a line, or a list made read-only on purpose, which is left as it is).
    pub fn record(&mut self, path: &Path) -> io::Result<bool> {
        let (Some(file), Some(path)) = (self.file(), storable(path)) else {
            return Ok(false);
        };
        if crate::settings::frozen(file) {
            return Ok(false);
        }
        let list = with_first(read(file), path);
        crate::settings::write_atomic(file, text(&list).as_bytes(), true)?;
        Ok(true)
    }

    /// Take `path` off the list.
    pub fn remove(&mut self, path: &Path) -> io::Result<()> {
        let Some(file) = self.file() else {
            return Ok(());
        };
        let mut list = read(file);
        let before = list.len();
        list.retain(|p| p != path);
        match list.len() {
            n if n == before => Ok(()),
            0 => forget(file),
            _ => crate::settings::write_atomic(file, text(&list).as_bytes(), true),
        }
    }

    /// Empty the list (its file is deleted).
    pub fn clear(&mut self) -> io::Result<()> {
        match self.file() {
            Some(file) => forget(file),
            None => Ok(()),
        }
    }
}

/// The files to show of `list`: those that are there now (one on a drive that is out, or
/// deleted, is skipped but kept), at most [`SHOWN`]. Each is looked at (`stat`), which a stale
/// network mount can hang: call it off the main thread.
pub fn shown(list: Vec<PathBuf>) -> Vec<PathBuf> {
    list.into_iter()
        .filter(|p| p.is_file())
        .take(SHOWN)
        .collect()
}

/// Delete the list's file (even a read-only one: off and Clear keep nothing).
fn forget(file: &Path) -> io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The looks at which files of the list are there ([`shown`], on a thread of their own): one at
/// a time (on a hung mount one may never end), another asked for meanwhile runs when it is done,
/// and an answer from before the list was emptied (Clear, off) is thrown away.
#[derive(Debug, Default)]
pub struct Looks {
    generation: u32,
    busy: bool,
    again: bool,
}

impl Looks {
    /// A look is wanted: the generation to start one with now, or `None` while one runs (it is
    /// asked again when that one is done).
    pub fn start(&mut self) -> Option<u32> {
        if self.busy {
            self.again = true;
            return None;
        }
        self.busy = true;
        Some(self.generation)
    }

    /// The list was emptied: what a running look finds is out of date.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// The look of `generation` answered (or couldn't start): whether its answer is still the
    /// list's, and whether to look again.
    pub fn done(&mut self, generation: u32) -> (bool, bool) {
        self.busy = false;
        let again = std::mem::take(&mut self.again);
        (generation == self.generation, again)
    }
}

#[cfg(test)]
mod tests;
