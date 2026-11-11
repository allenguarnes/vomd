//! The live reload watcher against a real directory: every way editors save a file is reported
//! once, other files in the directory never are, and a file that goes away reads as gone. Runs on
//! Linux (inotify), Windows and macOS (the file's stamp, looked at every half second).
//!
//! Reports are waited for, never slept for ([`WAIT`] is only a limit). That nothing comes is
//! checked by waiting [`NOTHING`]: a slow runner can only make that pass, never fail.
#![cfg(any(target_os = "linux", windows, target_os = "macos"))]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, SystemTime};

use vomd::view::Stamp;
use vomd::watch::{self, Reloads, Reread, Watch};

const WAIT: Duration = Duration::from_secs(30);
/// Longer than the longest quiet period (on Windows and macOS, after the next look at the file),
/// with room: a report that is going to come has by then.
const NOTHING: Duration = Duration::from_millis(if cfg!(any(windows, target_os = "macos")) {
    1500
} else {
    900
});

/// A fresh directory for one test, under the target directory (`/tmp` may be in memory).
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("watch-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// What the watcher reports: the generation and what it found.
type Found = (u32, Reread);

/// A watcher whose reports come through a channel.
fn start() -> (Watch, Receiver<Found>) {
    let (tx, rx) = channel();
    let watch = watch::start(move |g, found| {
        let _ = tx.send((g, found));
    })
    .expect("a watcher");
    (watch, rx)
}

/// A watcher on one file, and what the window last took of it, as the window takes it.
struct Watched {
    watch: Watch,
    rx: Receiver<Found>,
    live: Reloads<()>,
    path: PathBuf,
    stamp: Option<Stamp>,
    /// What the window shows (`None`: the file gone).
    text: Option<String>,
    /// Versions written, for texts of different lengths (a stamp is a time and a size).
    n: usize,
}

impl Watched {
    /// Write `path` and watch it.
    fn new(path: PathBuf) -> Watched {
        let text = "# Version 0\n";
        std::fs::write(&path, text).unwrap();
        let (watch, rx) = start();
        let stamp = vomd::view::stamp(&path);
        let mut live = Reloads::default();
        live.generation = watch.target(Some(&path), stamp, false);
        Watched {
            watch,
            rx,
            live,
            path,
            stamp,
            text: Some(text.into()),
            n: 0,
        }
    }

    /// The next version's text.
    fn next(&mut self) -> String {
        self.n += 1;
        format!("# Version {}\n{}\n", self.n, "x".repeat(self.n))
    }

    /// Take a report as the window does (`main.rs`): show it if it is a version to show, and
    /// tell the watcher. Whether it was.
    fn take(&mut self, (g, found): &Found) -> bool {
        // (A file gone has no stamp, so its return reads.)
        let (stamp, text) = match found {
            Reread::Text(t, s) => (*s, Some(t.clone())),
            Reread::Gone => (None, None),
            other => panic!("only versions and the file gone are reported: {other:?}"),
        };
        let now = || vomd::view::stamp(&self.path);
        if !self.live.current(*g, stamp, self.stamp, now) {
            return false;
        }
        (self.stamp, self.text) = (stamp, text);
        self.watch.target(Some(&self.path), self.stamp, false);
        true
    }

    /// Wait for a report, which is a version to show, and show it.
    fn reported(&mut self) -> Reread {
        let found = self.rx.recv_timeout(WAIT).expect("a report");
        assert!(self.take(&found), "a version to show: {:?}", found.1);
        found.1
    }

    /// Take reports until the window shows `text` (others may come first: the file back where
    /// it was), and then no more.
    fn until(&mut self, text: &str, what: &str) {
        while self.text.as_deref() != Some(text) {
            let found = self.rx.recv_timeout(WAIT).expect(what);
            self.take(&found);
        }
        self.nothing(what);
    }

    /// Save a new version, and see it shown: the watch is in place.
    fn saved(&mut self, what: &str) {
        let text = self.next();
        std::fs::write(&self.path, &text).unwrap();
        self.until(&text, what);
    }

    /// Exactly one report comes, and the file reads as `text`.
    fn shows(&mut self, text: &str, what: &str) {
        match self.reported() {
            Reread::Text(t, _) => assert_eq!(t, text, "{what}"),
            other => panic!("{what}: {other:?}"),
        }
        self.nothing(what);
    }

    /// Exactly one report comes, and the file reads as gone.
    fn gone(&mut self, what: &str) {
        assert_eq!(self.reported(), Reread::Gone, "{what}");
        self.nothing(what);
    }

    fn nothing(&self, what: &str) {
        assert_eq!(
            self.rx.recv_timeout(NOTHING),
            Err(RecvTimeoutError::Timeout),
            "{what}: no more reports"
        );
    }

    fn sibling(&self, name: &str) -> PathBuf {
        self.path.with_file_name(name)
    }
}

/// Writing in place, truncating and writing, `touch`.
#[test]
fn saves_in_place_are_reported_once() {
    let mut w = Watched::new(scratch("in-place").join("doc.md"));
    let text = w.next();
    std::fs::write(&w.path, &text).unwrap();
    w.shows(&text, "truncate and write");

    let more = w.next();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&w.path)
        .unwrap();
    std::io::Write::write_all(&mut f, more.as_bytes()).unwrap();
    drop(f);
    w.shows(&(text.clone() + &more), "appended");

    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&w.path)
        .unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(f);
    w.shows(&(text + &more), "touched");
}

/// Saves that write a new file and put it in place: through a temporary name (gedit, Kate,
/// `sed -i`), vim's backup, JetBrains' safe write, delete and create.
#[test]
fn saves_that_replace_the_file_are_reported_once() {
    let mut w = Watched::new(scratch("replace").join("doc.md"));

    let text = w.next();
    let tmp = w.sibling(".goutputstream-X1");
    std::fs::write(&tmp, &text).unwrap();
    std::fs::rename(&tmp, &w.path).unwrap();
    w.shows(&text, "temporary and rename");

    let text = w.next();
    let backup = w.sibling("doc.md~");
    std::fs::rename(&w.path, &backup).unwrap();
    std::fs::write(&w.path, &text).unwrap();
    std::fs::remove_file(&backup).unwrap();
    w.shows(&text, "vim's backup");

    let text = w.next();
    let (tmp, old) = (
        w.sibling("doc.md___jb_tmp___"),
        w.sibling("doc.md___jb_old___"),
    );
    std::fs::write(&tmp, &text).unwrap();
    std::fs::rename(&w.path, &old).unwrap();
    std::fs::rename(&tmp, &w.path).unwrap();
    std::fs::remove_file(&old).unwrap();
    w.shows(&text, "JetBrains");

    let text = w.next();
    std::fs::remove_file(&w.path).unwrap();
    std::fs::write(&w.path, &text).unwrap();
    w.shows(&text, "delete and create");
}

/// Other files in the directory, an editor's swap and backup files among them, never report.
#[test]
fn other_files_are_never_reported() {
    let mut w = Watched::new(scratch("others").join("doc.md"));
    for name in ["notes.txt", ".doc.md.swp", "doc.md~", "4913", "doc.md.bak"] {
        std::fs::write(w.sibling(name), "x").unwrap();
    }
    std::fs::remove_file(w.sibling("4913")).unwrap();
    std::fs::rename(w.sibling("notes.txt"), w.sibling("other.md")).unwrap();
    w.nothing("other names");
    // And the file itself still does.
    let text = w.next();
    std::fs::write(&w.path, &text).unwrap();
    w.shows(&text, "after the others");
}

/// Deleted or moved away, the file reads as gone; back, it reads again.
#[test]
fn a_file_that_goes_reads_as_gone_and_comes_back() {
    let mut w = Watched::new(scratch("gone").join("doc.md"));
    std::fs::rename(&w.path, w.sibling("moved.md")).unwrap();
    w.gone("moved away");
    std::fs::rename(w.sibling("moved.md"), &w.path).unwrap();
    w.shows("# Version 0\n", "moved back");
    std::fs::remove_file(&w.path).unwrap();
    w.gone("deleted");
    let text = w.next();
    std::fs::write(&w.path, &text).unwrap();
    w.shows(&text, "made again");
}

/// Another target has another generation, and the last one's file reports no more.
#[test]
fn another_target_reports_with_its_generation() {
    let dir = scratch("retarget");
    // (Written before the watch starts: its events would be the new target's.)
    let b = dir.join("b.md");
    std::fs::write(&b, "# B").unwrap();
    let mut w = Watched::new(dir.join("a.md"));
    let stamp = vomd::view::stamp(&b);
    let g = w.watch.target(Some(&b), stamp, false);
    assert_ne!(g, w.live.generation);
    assert_eq!(
        w.watch.target(Some(&b), stamp, false),
        g,
        "the same file keeps it"
    );
    w.live.generation = g;
    w.path = b;
    w.stamp = stamp;
    std::fs::write(w.sibling("a.md"), "# A, again").unwrap();
    w.nothing("the old target");
    std::fs::write(&w.path, "# B, again").unwrap();
    w.shows("# B, again", "the new target");
    // Nothing watched: nothing reported.
    w.live.generation = w.watch.target(None, None, false);
    std::fs::write(&w.path, "# B, once more").unwrap();
    w.nothing("no target");
}

/// A file in a folder that doesn't exist yet is watched once it does (the window looks again on
/// a focus reload), without a panic before.
#[test]
fn an_unwatchable_folder_is_left_unwatched() {
    let dir = scratch("unwatchable");
    let path = dir.join("later/doc.md");
    let (watch, rx) = start();
    let g = watch.target(Some(&path), None, false);
    std::fs::create_dir(dir.join("later")).unwrap();
    std::fs::write(&path, "# Here").unwrap();
    let stamp = vomd::view::stamp(&path);
    assert_eq!(watch.target(Some(&path), stamp, false), g);
    std::fs::write(&path, "# Here, edited").unwrap();
    let (got, found) = rx.recv_timeout(WAIT).expect("a report");
    assert_eq!(got, g);
    assert!(matches!(found, Reread::Text(t, _) if t == "# Here, edited"));
}

/// The folder the document is in can still be renamed and deleted while it is watched (Explorer
/// on Windows must not find it locked), and the file then reads as gone. Each is done once a save
/// was reported: the watch is in place (on Windows the thread first looks at the file after
/// `target`).
#[test]
fn the_folder_can_be_renamed_and_deleted_while_watched() {
    let dir = scratch("folder");
    let sub = dir.join("docs");
    std::fs::create_dir(&sub).unwrap();
    let mut w = Watched::new(sub.join("doc.md"));
    w.saved("watched");
    let moved = dir.join("docs-moved");
    std::fs::rename(&sub, &moved).expect("the watched folder renames");
    // (Windows may say nothing of its own folder going: only Linux is held to a report.)
    if cfg!(target_os = "linux") {
        w.gone("its folder moved");
    }
    std::fs::rename(&moved, &sub).unwrap();
    // The window looks again (the focus reload, `R`).
    w.watch.target(Some(&w.path), w.stamp, false);
    w.saved("watched again");
    std::fs::remove_dir_all(&sub).expect("the watched folder deletes");
    if cfg!(target_os = "linux") {
        w.gone("its folder deleted");
    }
}

/// The folders above the document's can be renamed while it is watched (a project folder isn't
/// locked because a document in it is shown), and the watch goes on.
#[test]
fn the_folder_above_can_be_renamed_while_watched() {
    let dir = scratch("above");
    let (project, moved) = (dir.join("project"), dir.join("project-moved"));
    std::fs::create_dir_all(project.join("docs")).unwrap();
    let mut w = Watched::new(project.join("docs/doc.md"));
    w.saved("watched");
    std::fs::rename(&project, &moved).expect("the folder above renames");
    std::fs::rename(&moved, &project).expect("and back");
    w.saved("still watched");
    std::fs::rename(&dir, dir.with_extension("moved")).expect("the one above that renames");
    std::fs::rename(dir.with_extension("moved"), &dir).unwrap();
    w.saved("still watched, again");
    // Cleanup only: whether a watched folder can be deleted is the test above's question.
    drop(w);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A version held while the reader drags is checked when they let go: a file put back as shown
/// meanwhile (a backup restored, which the watcher has nothing to report about) keeps what is
/// shown, and a save under way (truncated, not written yet) is never shown half-done: the
/// watcher's guarded read brings it once written.
#[test]
fn a_version_held_during_a_drag_is_checked_on_release() {
    let mut w = Watched::new(scratch("held").join("doc.md"));
    let shown = w.text.clone().unwrap();
    // Held, then the file put back as shown.
    let backup = w.path.with_extension("bak");
    std::fs::rename(&w.path, &backup).unwrap();
    let held = w.next();
    std::fs::write(&w.path, &held).unwrap();
    let hold = |w: &mut Watched| {
        let (g, found) = w.rx.recv_timeout(WAIT).expect("the save's report");
        let Reread::Text(_, stamp) = found else {
            panic!("a version: {found:?}");
        };
        let now = || vomd::view::stamp(&w.path);
        assert!(w.live.current(g, stamp, w.stamp, now));
        w.live.hold((), stamp);
    };
    hold(&mut w);
    std::fs::rename(&backup, &w.path).unwrap();
    assert_eq!(
        w.live.release(vomd::view::stamp(&w.path)),
        None,
        "put back as shown"
    );
    assert_eq!(w.text.as_deref(), Some(shown.as_str()));
    w.nothing("the file is what is shown");
    // Held, then a save under way when the reader lets go.
    let held = w.next();
    std::fs::write(&w.path, &held).unwrap();
    hold(&mut w);
    let mut file = std::fs::File::create(&w.path).unwrap();
    assert_eq!(w.live.release(vomd::view::stamp(&w.path)), None, "mid-save");
    assert_eq!(
        w.text.as_deref(),
        Some(shown.as_str()),
        "the version shown stays"
    );
    let last = w.next();
    std::io::Write::write_all(&mut file, last.as_bytes()).unwrap();
    drop(file);
    w.until(&last, "the save, once written");
}

/// A save the window takes late (it was still laying out the last one, a big document, or a
/// format on save came right after the save) is shown after it, never dropped for it.
#[test]
fn a_save_the_window_takes_late_is_shown() {
    let mut w = Watched::new(scratch("late").join("doc.md"));
    let first = w.next();
    std::fs::write(&w.path, &first).unwrap();
    let one = w.rx.recv_timeout(WAIT).expect("the first save's report");
    // The window is busy: the second save is read before it has taken the first.
    let second = w.next();
    std::fs::write(&w.path, &second).unwrap();
    let two = w.rx.recv_timeout(WAIT).expect("the second save's report");
    // (The first is one the file has moved on from: the window may skip it.)
    w.take(&one);
    assert!(w.take(&two), "the second");
    w.until(&second, "the second, shown");
}

/// A document opened through a symlink: an edit of the file it points to reports, and so does
/// the link pointed elsewhere (`ln -sf`); the window then looks again and the new file reports.
#[cfg(unix)]
#[test]
fn a_symlinked_document_follows_its_target() {
    let dir = scratch("symlink");
    std::fs::create_dir(dir.join("real")).unwrap();
    let real = dir.join("real/real.md");
    let link = dir.join("doc.md");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut w = Watched::new(link.clone());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "# Version 0\n");

    let text = w.next();
    std::fs::write(&real, &text).unwrap();
    w.shows(&text, "the target edited");

    let other = dir.join("real/other.md");
    std::fs::write(&other, "# Other").unwrap();
    let tmp = dir.join("doc.md.tmp");
    std::os::unix::fs::symlink(&other, &tmp).unwrap();
    std::fs::rename(&tmp, &link).unwrap();
    w.shows("# Other", "the link pointed elsewhere");

    assert_eq!(
        w.watch.target(Some(&link), w.stamp, false),
        w.live.generation,
        "looked at again"
    );
    std::fs::write(&real, "# Old target").unwrap();
    w.nothing("the old target");
    std::fs::write(&other, "# Other, edited").unwrap();
    w.shows("# Other, edited", "the new target edited");
}

/// A link pointed at a file in another directory is followed by the watcher itself, whether
/// or not the window takes (or is free to take) the first version it reports: a later save there
/// is reported too.
#[cfg(unix)]
#[test]
fn a_link_pointed_elsewhere_is_followed_without_the_window() {
    let dir = scratch("relink");
    for d in ["old", "new"] {
        std::fs::create_dir(dir.join(d)).unwrap();
    }
    let link = dir.join("doc.md");
    std::os::unix::fs::symlink(dir.join("old/a.md"), &link).unwrap();
    let mut w = Watched::new(link.clone());
    std::fs::write(dir.join("new/b.md"), "# B").unwrap();
    let tmp = dir.join("doc.md.tmp");
    std::os::unix::fs::symlink(dir.join("new/b.md"), &tmp).unwrap();
    std::fs::rename(&tmp, &link).unwrap();
    // Reported, but the window doesn't take it (dragging, or busy until the next save came).
    let (_, found) = w.rx.recv_timeout(WAIT).expect("the link pointed elsewhere");
    assert!(
        matches!(found, Reread::Text(ref t, _) if t == "# B"),
        "{found:?}"
    );
    std::fs::write(dir.join("new/b.md"), "# C, longer").unwrap();
    w.until("# C, longer", "a save in the new target's directory");
}

/// Saves faster than the file is read never pile up: a burst is one read, and saves during a
/// read fold into one more, which finds the last version.
#[test]
fn rapid_saves_fold_into_few_reads() {
    let mut w = Watched::new(scratch("rapid").join("doc.md"));
    let mut text = String::new();
    for _ in 0..50 {
        text = w.next();
        std::fs::write(&w.path, &text).unwrap();
    }
    let mut reads = 0;
    while w.text.as_deref() != Some(&text) {
        reads += 1;
        let found = w.rx.recv_timeout(WAIT).expect("a report");
        assert!(matches!(found.1, Reread::Text(..)), "{:?}", found.1);
        // (One read in the burst finds a version the file has since moved on from.)
        w.take(&found);
    }
    w.nothing("after the last version");
    assert!(reads <= 3, "{reads} reads for 50 saves");
}
