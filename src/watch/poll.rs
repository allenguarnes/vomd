//! The Windows watcher: the file's stamp, looked at every [`TICK`] while a file is watched.
//!
//! It keeps nothing open between looks (a look's metadata call opens the file for a moment):
//! Windows refuses to rename a folder while something under it is open, so a change
//! notification on the document's folder would keep the folders above it (the project the
//! document is in) from being renamed in Explorer. The stamp leaves out matching the file's name too (long or 8.3, in any case). A look
//! is one metadata call, made outside the lock; with nothing watched, the thread waits on the
//! target alone, without a timeout. A stamp that changed is an event for the same [`Debounce`] as
//! on Linux, so a save is read within a tick and [`QUIET`] of it.
//!
//! Only `std`: it is tested on Linux too.
//!
//! [`Debounce`]: super::Debounce
//! [`QUIET`]: super::QUIET

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{Kind, Report, State, settle};
use crate::view::{self, Stamp};

/// How often the watched file's stamp is looked at. A file gone is only activity: it waits
/// [`super::QUIET_INCOMPLETE`] (no less than a tick), so a file deleted and made again (an
/// editor's save) is seen back at the next look rather than reported gone.
pub(super) const TICK: Duration = Duration::from_millis(500);
const _: () = assert!(super::QUIET_INCOMPLETE.as_millis() >= TICK.as_millis());

pub(super) struct Watch(pub(super) Arc<Shared>);

pub(super) struct Shared {
    pub(super) state: Mutex<State>,
    /// Notified when the target changes.
    wake: Condvar,
}

pub(super) fn start(report: impl Report) -> Option<Watch> {
    let shared = Arc::new(Shared {
        state: Mutex::default(),
        wake: Condvar::new(),
    });
    let theirs = shared.clone();
    std::thread::spawn(move || run(&theirs, &report));
    Some(Watch(shared))
}

impl Watch {
    pub(super) fn target(&self, path: Option<&Path>, shown: Option<Stamp>, empty: bool) -> u32 {
        let Ok(mut s) = self.0.state.lock() else {
            return 0;
        };
        s.set(path, shown, empty);
        s.again = true;
        self.0.wake.notify_one();
        s.generation
    }
}

/// The thread ends with the watcher (the window's lives as long as the process; the tests make
/// many).
impl Drop for Watch {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0.state.lock() {
            s.stop = true;
            self.0.wake.notify_one();
        }
    }
}

/// The watcher thread: look at the file's stamp every tick (and at once for a target), and once
/// a change has been left alone, read the file again.
fn run(shared: &Shared, report: &impl Report) {
    let Ok(mut s) = shared.state.lock() else {
        return;
    };
    // The stamp seen last, and when to look again.
    let (mut seen, mut next) = (None, Instant::now());
    loop {
        if s.stop {
            return;
        }
        if std::mem::take(&mut s.again) {
            // From the version shown: a save since it was read is a change.
            (seen, next) = (s.shown, Instant::now());
        }
        if let Some(path) = (Instant::now() >= next).then(|| s.path.clone()).flatten() {
            drop(s);
            let stamp = view::stamp(&path);
            let now = Instant::now();
            next = now + TICK;
            let Ok(guard) = shared.state.lock() else {
                return;
            };
            s = guard;
            // (A target meanwhile is looked at again, from the top.)
            if !s.again && stamp != seen {
                seen = stamp;
                let kind = if stamp.is_some() {
                    Kind::Change
                } else {
                    Kind::Activity
                };
                s.debounce.event(kind, now);
            }
        }
        let now = Instant::now();
        if s.fire(now) {
            drop(s);
            settle(&shared.state, report);
            let Ok(guard) = shared.state.lock() else {
                return;
            };
            s = guard;
            continue;
        }
        if s.again || s.stop {
            continue;
        }
        let tick = s.path.is_some().then_some(next);
        let woke = match s.debounce.deadline().into_iter().chain(tick).min() {
            None => shared.wake.wait(s).ok(),
            Some(t) => shared
                .wake
                .wait_timeout(s, t.saturating_duration_since(now))
                .ok()
                .map(|(s, _)| s),
        };
        let Some(guard) = woke else {
            return;
        };
        s = guard;
    }
}
