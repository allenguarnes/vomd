//! The Linux watcher: inotify on the document's directory, its events matched by name.
//!
//! A watch on the file itself would follow the old file through every save that replaces it (vim,
//! gedit, Kate, JetBrains, `sed -i` write a new file and move it over the old one) and see nothing
//! after. A directory's events name the file whichever way it is saved. A document reached
//! through a symlink has the link's directory watched and the directory of the file it points
//! to (edits go there).
//!
//! The functions are declared by hand, as `portal.rs` and the SVG process do: no crate.

use std::ffi::{OsString, c_char, c_ulong, c_void};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Kind, Report, State, settle, split, wait_ms};
use crate::view::Stamp;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 1;
const IN_NONBLOCK: i32 = 0o4000;
const IN_CLOEXEC: i32 = 0o2_000_000;

const IN_MODIFY: u32 = 0x2;
const IN_ATTRIB: u32 = 0x4;
const IN_CLOSE_WRITE: u32 = 0x8;
const IN_MOVED_FROM: u32 = 0x40;
const IN_MOVED_TO: u32 = 0x80;
const IN_CREATE: u32 = 0x100;
const IN_DELETE: u32 = 0x200;
const IN_DELETE_SELF: u32 = 0x400;
const IN_MOVE_SELF: u32 = 0x800;
const IN_Q_OVERFLOW: u32 = 0x4000;
const IN_IGNORED: u32 = 0x8000;
const IN_ONLYDIR: u32 = 0x0100_0000;
const IN_EXCL_UNLINK: u32 = 0x0400_0000;

/// What a directory is watched for: what a save does to the file in it, and the directory
/// itself going. Not opens or reads (VOMD's own read of the file wakes nothing).
const MASK: u32 = IN_CLOSE_WRITE
    | IN_MOVED_TO
    | IN_CREATE
    | IN_MODIFY
    | IN_DELETE
    | IN_MOVED_FROM
    | IN_ATTRIB
    | IN_DELETE_SELF
    | IN_MOVE_SELF
    | IN_ONLYDIR
    | IN_EXCL_UNLINK;

unsafe extern "C" {
    fn inotify_init1(flags: i32) -> i32;
    fn inotify_add_watch(fd: i32, path: *const c_char, mask: u32) -> i32;
    fn inotify_rm_watch(fd: i32, wd: i32) -> i32;
    fn read(fd: i32, buf: *mut c_void, n: usize) -> isize;
    fn poll(fds: *mut PollFd, n: c_ulong, timeout_ms: i32) -> i32;
}

pub(super) struct Watch(pub(super) Arc<Shared>);

/// The inotify descriptor (the process's one, never closed) and what is watched.
pub(super) struct Shared {
    fd: i32,
    pub(super) state: Mutex<State>,
}

impl State {
    /// What the event `mask` on watch `wd` for `name` says of the file, if anything.
    fn kind(&self, wd: i32, mask: u32, name: &[u8]) -> Option<Kind> {
        // Events were lost: whether the file changed is for its stamp to say.
        if mask & IN_Q_OVERFLOW != 0 {
            return self.path.is_some().then_some(Kind::Change);
        }
        if !self.names.iter().any(|(w, _)| *w == wd) {
            return None;
        }
        // The directory went (or moved): reading the file again says it's gone.
        if mask & (IN_DELETE_SELF | IN_MOVE_SELF | IN_IGNORED) != 0 {
            return Some(Kind::Activity);
        }
        let ours = |(w, n): &(i32, OsString)| *w == wd && n.as_bytes() == name;
        self.names
            .iter()
            .any(ours)
            .then_some(if mask & (IN_CLOSE_WRITE | IN_MOVED_TO) != 0 {
                Kind::Change
            } else {
                Kind::Activity
            })
    }
}

pub(super) fn start(report: impl Report) -> Option<Watch> {
    // SAFETY: plain integers in and out.
    let fd = unsafe { inotify_init1(IN_NONBLOCK | IN_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let shared = Arc::new(Shared {
        fd,
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
        s.watch(self.0.fd);
        s.generation
    }
}

impl State {
    /// Watch the directories the path is seen from now (a symlink pointed elsewhere, or a
    /// directory gone and back, changes them), and drop the watches no longer needed. Done when
    /// the window names a target and again before each read, so the watcher keeps up with a
    /// repointed link by itself: the read that follows sees any save made before the new watch.
    fn watch(&mut self, fd: i32) {
        let mut names: Vec<(i32, OsString)> = Vec::new();
        for (dir, name) in self.path.as_deref().map(places).unwrap_or_default() {
            // (A NUL inside the path ends it early: then nothing that matters is watched.)
            let mut dir = dir.into_os_string().into_vec();
            dir.push(0);
            // SAFETY: a NUL-terminated path, read during the call.
            let wd = unsafe { inotify_add_watch(fd, dir.as_ptr().cast(), MASK) };
            if wd >= 0 {
                names.push((wd, name));
            }
        }
        // (The same directory has the same descriptor: it stays.)
        for &(wd, _) in &self.names {
            if !names.iter().any(|(w, _)| *w == wd) {
                // SAFETY: plain integers.
                unsafe { inotify_rm_watch(fd, wd) };
            }
        }
        self.names = names;
    }
}

/// Where `path` is seen from: its directory and name, and the directory and name of the file it
/// ends at, which for a symlink are others. (For a plain file they are the same directory, whose
/// watch is the same descriptor: matching twice costs nothing.)
fn places(path: &Path) -> Vec<(PathBuf, OsString)> {
    let mut out: Vec<_> = split(path).into_iter().collect();
    out.extend(std::fs::canonicalize(path).ok().as_deref().and_then(split));
    out
}

/// The events in what a read of the descriptor gave: (watch, mask, name).
fn events(mut buf: &[u8]) -> impl Iterator<Item = (i32, u32, &[u8])> {
    std::iter::from_fn(move || {
        let field = |i: usize| buf.get(i..i + 4)?.try_into().ok();
        let wd = i32::from_ne_bytes(field(0)?);
        let mask = u32::from_ne_bytes(field(4)?);
        let len = u32::from_ne_bytes(field(12)?) as usize;
        let name = buf.get(16..16 + len)?;
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(len)];
        buf = &buf[16 + len..];
        Some((wd, mask, name))
    })
}

/// The watcher thread: wait for events (with a timeout only while one is pending), fold them,
/// and read the file again once a burst is over.
fn run(shared: &Shared, report: &impl Report) {
    // Aligned for the events' header, and room for many (a name is at most 256 bytes).
    let mut buf = [0u64; 1024];
    let mut timeout = -1;
    loop {
        let mut pfd = PollFd {
            fd: shared.fd,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: one valid entry.
        unsafe { poll(&mut pfd, 1, timeout) };
        let now = Instant::now();
        // (Read under the lock: a new target waits that long, and no event is matched against
        // the old one after it.)
        let Ok(mut s) = shared.state.lock() else {
            return;
        };
        loop {
            // SAFETY: a buffer of the length given, which the call fills.
            let n = unsafe { read(shared.fd, buf.as_mut_ptr().cast(), size_of_val(&buf)) };
            let Some(n) = usize::try_from(n).ok().filter(|&n| n > 0) else {
                break;
            };
            // SAFETY: the `n` bytes the read filled.
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), n) };
            for (wd, mask, name) in events(bytes) {
                if let Some(kind) = s.kind(wd, mask, name) {
                    s.debounce.event(kind, now);
                }
            }
        }
        let fire = s.fire(now);
        if fire {
            s.watch(shared.fd);
        }
        timeout =
            wait_ms(s.debounce.deadline(), now).map_or(-1, |ms| ms.min(i32::MAX as u32) as i32);
        drop(s);
        if fire {
            settle(&shared.state, report);
            // (Its events since are queued: they are read at once.)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn event(wd: i32, mask: u32, name: &str) -> Vec<u8> {
        let mut len = if name.is_empty() { 0 } else { name.len() + 1 };
        len = len.next_multiple_of(16);
        let mut v = Vec::new();
        v.extend(wd.to_ne_bytes());
        v.extend(mask.to_ne_bytes());
        v.extend(0u32.to_ne_bytes());
        v.extend((len as u32).to_ne_bytes());
        let mut n = name.as_bytes().to_vec();
        n.resize(len, 0);
        v.extend(n);
        v
    }

    #[test]
    fn events_decode_with_their_names_unpadded() {
        let mut buf = event(3, IN_MOVED_TO, "doc.md");
        buf.extend(event(3, IN_DELETE_SELF, ""));
        buf.extend(event(4, IN_CREATE, "a-name-of-exactly-16"));
        let got: Vec<_> = events(&buf).collect();
        assert_eq!(
            got,
            [
                (3, IN_MOVED_TO, &b"doc.md"[..]),
                (3, IN_DELETE_SELF, &b""[..]),
                (4, IN_CREATE, &b"a-name-of-exactly-16"[..]),
            ]
        );
        // A short read stops at the last whole event.
        assert_eq!(events(&buf[..buf.len() - 1]).count(), 2);
    }

    /// Only the file's own name counts, in its own directory: backups, swap files and vim's
    /// write test are other names, and so is the same name in the link's other directory.
    #[test]
    fn events_count_only_for_the_files_name() {
        let s = State {
            path: Some("d/doc.md".into()),
            names: vec![(1, "doc.md".into()), (2, "real.md".into())],
            ..State::default()
        };
        for name in [
            "doc.md~",
            ".doc.md.swp",
            "4913",
            "doc.md___jb_tmp___",
            "notes.txt",
        ] {
            assert_eq!(s.kind(1, IN_CLOSE_WRITE, name.as_bytes()), None, "{name}");
        }
        assert_eq!(s.kind(2, IN_CLOSE_WRITE, b"doc.md"), None);
        assert_eq!(s.kind(9, IN_CLOSE_WRITE, b"doc.md"), None, "a watch let go");
        assert_eq!(s.kind(9, IN_IGNORED, b""), None);
        assert_eq!(s.kind(1, IN_CLOSE_WRITE, b"doc.md"), Some(Kind::Change));
        assert_eq!(s.kind(2, IN_MOVED_TO, b"real.md"), Some(Kind::Change));
        for mask in [IN_MODIFY, IN_CREATE, IN_DELETE, IN_MOVED_FROM, IN_ATTRIB] {
            assert_eq!(s.kind(1, mask, b"doc.md"), Some(Kind::Activity));
        }
        for mask in [IN_DELETE_SELF, IN_MOVE_SELF, IN_IGNORED] {
            assert_eq!(
                s.kind(1, mask, b""),
                Some(Kind::Activity),
                "the folder went"
            );
        }
        assert_eq!(s.kind(-1, IN_Q_OVERFLOW, b""), Some(Kind::Change));
        let none = State::default();
        assert_eq!(none.kind(-1, IN_Q_OVERFLOW, b""), None, "nothing watched");
    }

    /// A plain file is seen from its directory; a symlink from the link's directory and the
    /// target's.
    #[test]
    fn a_symlink_is_seen_from_both_directories() {
        let dir = std::env::temp_dir().join(format!("vomd-places-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/real.md"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("real/real.md"), dir.join("doc.md")).unwrap();
        let real = std::fs::canonicalize(dir.join("real")).unwrap();
        assert_eq!(
            places(&dir.join("doc.md")),
            [
                (dir.clone(), OsStr::new("doc.md").to_owned()),
                (real.clone(), "real.md".into())
            ]
        );
        let plain = (real.clone(), OsStr::new("real.md").to_owned());
        assert_eq!(places(&real.join("real.md")), [plain.clone(), plain]);
        // A link to a link: the file at the end.
        std::os::unix::fs::symlink(dir.join("doc.md"), dir.join("again.md")).unwrap();
        assert_eq!(places(&dir.join("again.md"))[1].1, "real.md");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
