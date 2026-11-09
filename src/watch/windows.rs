//! The Windows watcher: a change notification on the document's directory, and the file's stamp.
//!
//! The notification says only that something in the directory changed, not what: the file's
//! stamp, compared with the one seen last, says whether it was the document. That also leaves out
//! matching its name (long or 8.3, in any case). A busy directory wakes the thread for every
//! change in it, which the debounce folds into one look at the stamp per quiet period.
//!
//! The functions are declared by hand, as in `platform/windows.rs`.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Kind, Report, State, settle, split, wait_ms};
use crate::view::{self, Stamp};

type Handle = *mut c_void;

const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x1;
const FILE_NOTIFY_CHANGE_SIZE: u32 = 0x8;
const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x10;
/// What a save does in the directory: a file written, made, deleted or renamed.
const FILTER: u32 =
    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;
const INFINITE: u32 = u32::MAX;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn FindFirstChangeNotificationW(dir: *const u16, subtree: i32, filter: u32) -> Handle;
    fn FindNextChangeNotification(h: Handle) -> i32;
    fn FindCloseChangeNotification(h: Handle) -> i32;
    fn WaitForMultipleObjects(n: u32, hs: *const Handle, all: i32, ms: u32) -> u32;
    fn CreateEventW(sa: *mut c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
}

pub(super) struct Watch(pub(super) Arc<Shared>);

pub(super) struct Shared {
    /// An auto-reset event, set when the target changes: the thread waits on it beside the
    /// change notification, which only it opens and closes. (An address, to be sent.)
    wake: usize,
    pub(super) state: Mutex<State>,
}

pub(super) fn start(report: impl Report) -> Option<Watch> {
    // SAFETY: no attributes and no name: an unnamed auto-reset event, not set.
    let wake = unsafe { CreateEventW(null_mut(), 0, 0, null()) };
    if wake.is_null() {
        return None;
    }
    let shared = Arc::new(Shared {
        wake: wake as usize,
        state: Mutex::default(),
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
        // SAFETY: the event, which is never closed.
        unsafe { SetEvent(self.0.wake as Handle) };
        s.generation
    }
}

/// A change notification on `dir` (null if it can't be had: the folder isn't there).
fn open(dir: &Path) -> Handle {
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path, read during the call.
    let h = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, FILTER) };
    if h == INVALID_HANDLE_VALUE {
        null_mut()
    } else {
        h
    }
}

fn close(h: &mut Handle) {
    if !h.is_null() {
        // SAFETY: a notification this thread opened, closed once.
        unsafe { FindCloseChangeNotification(*h) };
        *h = null_mut();
    }
}

/// The watcher thread: wait for the directory to change (with a timeout only while a change is
/// pending), and once it has been quiet, read the file again (its stamp says whether it was the
/// document that changed).
fn run(shared: &Shared, report: &impl Report) {
    let wake = shared.wake as Handle;
    let mut change: Handle = null_mut();
    let mut timeout = INFINITE;
    loop {
        let handles = [wake, change];
        let n = if change.is_null() { 1 } else { 2 };
        // SAFETY: `n` valid handles.
        let r = unsafe { WaitForMultipleObjects(n, handles.as_ptr(), 0, timeout) };
        let now = Instant::now();
        let Ok(mut guard) = shared.state.lock() else {
            return;
        };
        let s = &mut *guard;
        if r == WAIT_OBJECT_0 {
            if std::mem::take(&mut s.again) {
                close(&mut change);
                if let Some(path) = &s.path {
                    change = split(path).map_or(null_mut(), |(dir, _)| open(&dir));
                    // A save since the version shown was read, before the notification was there.
                    if view::stamp(path) != s.shown {
                        s.debounce.event(Kind::Change, now);
                    }
                }
            }
        } else if r == WAIT_OBJECT_0 + 1 {
            s.debounce.event(Kind::Change, now);
            // SAFETY: the notification just signalled.
            if unsafe { FindNextChangeNotification(change) } == 0 {
                close(&mut change);
            }
        } else if r != WAIT_OBJECT_0 + 0x102 {
            // (Not a timeout: a wait that failed must not spin.)
            close(&mut change);
        }
        let fire = s.fire(now);
        timeout = wait_ms(s.debounce.deadline(), now).unwrap_or(INFINITE);
        let dir = fire.then(|| s.path.as_deref().and_then(split)).flatten();
        drop(guard);
        if fire {
            settle(&shared.state, report);
            // The folder went: its notification is let go (the window looks again later).
            if dir.is_some_and(|(dir, _)| !dir.is_dir()) {
                close(&mut change);
            }
        }
    }
}
