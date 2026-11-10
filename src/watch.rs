//! Live reload: a thread that watches the open document's file and says when it was saved, and
//! what the window does with that ([`Reloads`], [`reread`]).
//!
//! One watcher per process, started after the first frame. Once an event has come, it waits for
//! the quiet period of its [`Debounce`], then reads the file again itself (off the window's
//! thread) and reports what it found. On Linux it is inotify on the document's directory (an
//! editor's save replaces the file, which a watch on the file would lose), matched by name: the
//! thread blocks in the system's call with no timeout while nothing happens, and never polls (no
//! CPU when idle). On Windows it looks at the file's stamp twice a second, keeping nothing open
//! between looks (`poll.rs`: a handle would lock the folders above the document). Elsewhere there is none: the
//! window's focus reload is all there is, and it stays the fallback everywhere (network file
//! systems, inotify's limits).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cli::Input;
use crate::view::{self, Origin, Stamp};

#[cfg(target_os = "linux")]
mod inotify;
#[cfg(target_os = "linux")]
use inotify as sys;
#[cfg(any(windows, test))]
mod poll;
#[cfg(windows)]
use poll as sys;

/// How long the file has to be left alone after a complete save (a file closed after writing, or
/// one moved into place) before it is read: every editor's save is over well within it, and a
/// second write (a formatter on save) folds into it. On Windows a change is only a stamp that
/// differs, so every change waits the same there, a little longer.
pub const QUIET: Duration = Duration::from_millis(if cfg!(windows) { 150 } else { 75 });
/// The same after only signs of a save (written to and not closed, deleted, created): a writer
/// that keeps the file open, or a name that comes back (an editor's delete and create).
pub const QUIET_INCOMPLETE: Duration = Duration::from_millis(500);
/// A file written to without a pause is still shown this often.
pub const CAP: Duration = Duration::from_secs(1);
/// How many times a read is tried again when the file changed under it or couldn't be read.
const RETRIES: u32 = 2;

/// What an event says of the watched file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A complete version is in place: closed after writing, or moved there.
    Change,
    /// Something happened to it: written to, created, deleted, moved away, its folder too.
    Activity,
}

/// When a burst of events ends: the first event arms it, and it fires once the file has been
/// left alone for [`QUIET`] (after a [`Kind::Change`]) or [`QUIET_INCOMPLETE`] (only activity),
/// or [`CAP`] after the first event, whichever comes first. Times are given, so it is tested
/// without threads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Debounce {
    armed: Option<Armed>,
}

#[derive(Clone, Copy, Debug)]
struct Armed {
    first: Instant,
    last: Instant,
    complete: bool,
}

impl Debounce {
    /// An event for the watched file at `now`.
    pub fn event(&mut self, kind: Kind, now: Instant) {
        let a = self.armed.get_or_insert(Armed {
            first: now,
            last: now,
            complete: false,
        });
        a.last = a.last.max(now);
        a.complete |= kind == Kind::Change;
    }

    /// When it fires (`None`: never, nothing is pending, and the watcher waits without a
    /// timeout).
    pub fn deadline(&self) -> Option<Instant> {
        let a = self.armed?;
        let quiet = if a.complete { QUIET } else { QUIET_INCOMPLETE };
        Some((a.last + quiet).min(a.first + CAP))
    }

    /// Whether it fires at `now`; if it does, it is idle again.
    pub fn fire(&mut self, now: Instant) -> bool {
        let due = self.deadline().is_some_and(|d| now >= d);
        if due {
            self.armed = None;
        }
        due
    }

    /// Forget what is pending (another file is watched).
    pub fn reset(&mut self) {
        self.armed = None;
    }
}

/// How long a wait for `deadline` is from `now`, in whole milliseconds rounded up (so the wait
/// doesn't end before it); `None`: no deadline, no timeout.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn wait_ms(deadline: Option<Instant>, now: Instant) -> Option<u32> {
    let left = deadline?.saturating_duration_since(now);
    // (No `u128`: its division is code of its own.)
    let ms =
        left.as_secs().saturating_mul(1000) + u64::from(left.subsec_nanos().div_ceil(1_000_000));
    Some(ms.min(u64::from(u32::MAX - 1)) as u32)
}

/// What the window does with the reports: which are versions to show, and one held while the
/// reader drags.
#[derive(Debug)]
pub struct Reloads<T> {
    /// The generation of what is watched: reports for any other are stale.
    pub generation: u32,
    /// A version held, with its stamp.
    held: Option<(T, Option<Stamp>)>,
}

impl<T> Default for Reloads<T> {
    fn default() -> Self {
        Reloads {
            generation: 0,
            held: None,
        }
    }
}

impl<T> Reloads<T> {
    /// Whether a report is a version to show: its target is the one watched now, the window
    /// doesn't have it (shown, `shown`, or held), and it is what the file is now (`now`, asked
    /// only then; the file gone is a version with no stamp). So the window takes reports however
    /// late, and never goes back: one the file has moved on from is dropped, and the read of the
    /// newer one is coming (the newer one's save woke the watcher after this read began).
    /// Known limit: two saves of the same length within one tick of the file system's clock have
    /// the same stamp, so the second isn't shown until the next save (or `R`).
    pub fn current(
        &self,
        generation: u32,
        stamp: Option<Stamp>,
        shown: Option<Stamp>,
        now: impl FnOnce() -> Option<Stamp>,
    ) -> bool {
        let latest = self.held.as_ref().map_or(shown, |h| h.1);
        generation == self.generation && stamp != latest && stamp == now()
    }

    /// The window read the document itself or opened another: nothing is held.
    pub fn reset(&mut self) {
        self.held = None;
    }

    /// A version is held: the focus reload leaves it to that.
    pub fn busy(&self) -> bool {
        self.held.is_some()
    }

    /// Keep a new version, stamped `stamp`, until the reader lets go of the mouse (one newer
    /// replaces it).
    pub fn hold(&mut self, version: T, stamp: Option<Stamp>) {
        self.held = Some((version, stamp));
    }

    /// The version held, to show now, and its stamp.
    pub fn take_held(&mut self) -> Option<(T, Option<Stamp>)> {
        self.held.take()
    }

    /// The reader let go: the version held, to show now, if it is still what the file is (its
    /// stamp is `now`) or the file is gone (it is then the last version there was). Otherwise
    /// it is dropped and the version shown stays: a file put back as shown needs nothing (and
    /// the watcher has nothing to say about it), and a file that moved on to another version is
    /// one the watcher reads and reports, guarded, as for any save (its shown version is still
    /// the window's, which the file now differs from).
    pub fn release(&mut self, now: Option<Stamp>) -> Option<(T, Option<Stamp>)> {
        let (version, stamp) = self.held.take()?;
        (now.is_none() || now == stamp).then_some((version, stamp))
    }

    /// The file is gone: a version held is the last one there was, shown as the file gone (with
    /// no stamp, so the focus reload leaves it too).
    pub fn gone(&mut self) {
        if let Some(h) = &mut self.held {
            h.1 = None;
        }
    }
}

/// What reading the file again found.
#[derive(Debug, PartialEq, Eq)]
pub enum Reread {
    /// Its stamp is the one shown: nothing to show.
    Same,
    /// It isn't there (deleted, moved away, its folder too, a link to nothing).
    Gone,
    /// It couldn't be read (again, or for good: too large, not a file, not ours to read): the
    /// version shown stays (the focus reload and `R` say why).
    Failed,
    /// The new version, read whole, and its stamp.
    Text(String, Option<Stamp>),
}

/// Read `path` again for a live reload, never showing a half-written version: the file's stamp
/// is compared before and after (a write during the read reads it again), an empty file where
/// the shown version (`shown`) wasn't empty is given [`QUIET_INCOMPLETE`] to fill (an editor
/// that truncates first), and a failed read is tried again after it (an editor holding the file
/// while it saves), unless it can't succeed. `wait` sleeps (on the reading thread; the tests
/// don't).
pub fn reread(
    path: &Path,
    shown: Option<Stamp>,
    shown_empty: bool,
    mut wait: impl FnMut(Duration),
) -> Reread {
    let input = Input::File(path.to_path_buf());
    let (mut tries, mut empty_waited) = (0, false);
    loop {
        match std::fs::metadata(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Reread::Gone,
            // (Not read at all.)
            Ok(m) if !m.is_file() || m.len() > view::MAX_BYTES => return Reread::Failed,
            _ => {}
        }
        let now = view::stamp(path);
        if now.is_some() && now == shown {
            return Reread::Same;
        }
        match view::read_input(&input, Origin::Navigation) {
            Ok((text, stamp)) if tries >= RETRIES || view::stamp(path) == stamp => {
                if text.is_empty() && !shown_empty && !empty_waited {
                    empty_waited = true;
                    wait(QUIET_INCOMPLETE);
                    continue;
                }
                return Reread::Text(text, stamp);
            }
            Ok(_) => wait(QUIET),
            // (On Windows a file being replaced refuses to open for a moment.)
            Err(_)
                if tries >= RETRIES
                    || cfg!(unix)
                        && std::fs::File::open(path)
                            .is_err_and(|e| e.kind() == ErrorKind::PermissionDenied) =>
            {
                return Reread::Failed;
            }
            Err(_) => wait(QUIET_INCOMPLETE),
        }
        tries += 1;
    }
}

/// What the watcher calls, from its thread, when the file changed: the target's generation and
/// what it found ([`Reread::Text`] or [`Reread::Gone`]; the same version or one that can't be
/// read says nothing).
pub trait Report: Fn(u32, Reread) + Send + 'static {}
impl<F: Fn(u32, Reread) + Send + 'static> Report for F {}

/// What is watched, shared by the window and the watcher thread.
#[derive(Default)]
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
struct State {
    generation: u32,
    path: Option<PathBuf>,
    /// The stamp of the version the window shows, and whether that one is empty.
    shown: Option<Stamp>,
    empty: bool,
    debounce: Debounce,
    /// A read is running.
    reading: bool,
    /// The watched directories' descriptors, each with the name the file has there.
    #[cfg(target_os = "linux")]
    names: Vec<(i32, std::ffi::OsString)>,
    /// The thread is to look at the target again.
    #[cfg(any(windows, test))]
    again: bool,
    /// The watcher was dropped: the thread ends.
    #[cfg(any(windows, test))]
    stop: bool,
}

#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
impl State {
    /// The window watches `path` and shows the version stamped `shown`. Another path is another
    /// generation, and what was pending for the last is forgotten. Whether it is another.
    fn set(&mut self, path: Option<&Path>, shown: Option<Stamp>, empty: bool) -> bool {
        (self.shown, self.empty) = (shown, empty);
        let new = self.path.as_deref() != path;
        if new {
            self.generation = self.generation.wrapping_add(1);
            self.path = path.map(Path::to_path_buf);
            self.debounce.reset();
        }
        new
    }

    /// Whether the debounce fires at `now`; then a read is running (see [`settle`]) until it ends.
    fn fire(&mut self, now: Instant) -> bool {
        let fire = self.debounce.fire(now) && self.path.is_some();
        self.reading |= fire;
        fire
    }
}

/// A burst of events ended (the thread's [`State::fire`]): read the file again, on the watcher
/// thread and outside the lock, and report what changed. Events meanwhile are only queued, and
/// fold into one more read after: one read at a time and at most one more, however fast the saves.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
fn settle(state: &std::sync::Mutex<State>, report: &impl Report) {
    let Some((generation, path, shown, empty)) = state
        .lock()
        .ok()
        .and_then(|s| Some((s.generation, s.path.clone()?, s.shown, s.empty)))
    else {
        return;
    };
    let found = reread(&path, shown, empty, std::thread::sleep);
    let gone = found == Reread::Gone;
    if let Ok(mut s) = state.lock() {
        s.reading = false;
        // (So the file's return reads, even with the stamp it had.)
        if gone && s.generation == generation {
            s.shown = None;
        }
    }
    if gone || matches!(found, Reread::Text(..)) {
        report(generation, found);
    }
}

/// The watcher (see the module's documentation).
pub struct Watch(#[cfg(any(target_os = "linux", windows))] sys::Watch);

/// Start the watcher thread, which calls `report` from it. `None`: there's none here (not Linux
/// or Windows) or the system has none to give (inotify's limits): the focus reload stays.
pub fn start(report: impl Report) -> Option<Watch> {
    #[cfg(any(target_os = "linux", windows))]
    return sys::start(report).map(Watch);
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        drop(report);
        None
    }
}

impl Watch {
    /// Watch `path` from now on (`None`: nothing), whose version shown is stamped `shown` (and
    /// `empty` or not), and return the generation its reports carry. The same path again keeps
    /// the generation and only looks again at what to watch: a symlink pointed elsewhere, a
    /// folder that went and is back. A folder that can't be watched leaves the file unwatched,
    /// without a word.
    pub fn target(&self, path: Option<&Path>, shown: Option<Stamp>, empty: bool) -> u32 {
        #[cfg(any(target_os = "linux", windows))]
        return self.0.target(path, shown, empty);
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = (path, shown, empty);
            0
        }
    }

    /// A save is pending or being read: the focus reload leaves it to that.
    pub fn busy(&self) -> bool {
        #[cfg(any(target_os = "linux", windows))]
        return self
            .0
            .0
            .state
            .lock()
            .is_ok_and(|s| s.reading || s.debounce.deadline().is_some());
        #[cfg(not(any(target_os = "linux", windows)))]
        false
    }
}

/// The directory `path` is in and its name there.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn split(path: &Path) -> Option<(PathBuf, std::ffi::OsString)> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    Some((dir, path.file_name()?.to_owned()))
}

#[cfg(test)]
mod tests;
