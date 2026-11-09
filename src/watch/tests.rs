use super::*;

use Kind::{Activity, Change};

const MS: Duration = Duration::from_millis(1);

/// Run `script` (events, each `ms` after the start) through a debouncer, checking it every
/// millisecond as a watcher would: the times (since the start) it fired at.
fn fires(script: &[(u64, Kind)], until_ms: u64) -> Vec<u64> {
    let t0 = Instant::now();
    let mut d = Debounce::default();
    let mut out = Vec::new();
    for ms in 0..=until_ms {
        let now = t0 + MS * ms as u32;
        for &(_, kind) in script.iter().filter(|e| e.0 == ms) {
            d.event(kind, now);
        }
        if d.fire(now) {
            out.push(ms);
        }
    }
    out
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

/// Every editor's save that was measured (see the design notes in `watch.rs`): one fire, the
/// quiet period after the event that completes it.
#[test]
fn every_save_pattern_fires_once_after_it_is_complete() {
    let q = ms(QUIET);
    type Pattern<'a> = (&'a str, &'a [(u64, Kind)], u64);
    let patterns: [Pattern; 7] = [
        // `>>`: MODIFY, CLOSE_WRITE.
        ("in place", &[(0, Activity), (1, Change)], 1),
        // `>` (VS Code, nano): MODIFY, MODIFY, CLOSE_WRITE.
        (
            "truncate and write",
            &[(0, Activity), (0, Activity), (2, Change)],
            2,
        ),
        // vim's writebackup: MOVED_FROM doc.md, CREATE, MODIFY, CLOSE_WRITE (the backup's own
        // names don't match).
        (
            "vim",
            &[(0, Activity), (1, Activity), (1, Activity), (3, Change)],
            3,
        ),
        // gedit, Kate, `sed -i`: only MOVED_TO doc.md.
        ("temp and rename", &[(5, Change)], 5),
        // JetBrains: MOVED_FROM doc.md, MOVED_TO doc.md.
        ("JetBrains", &[(0, Activity), (1, Change)], 1),
        // DELETE, CREATE, MODIFY, CLOSE_WRITE.
        (
            "delete and create",
            &[(0, Activity), (2, Activity), (2, Activity), (4, Change)],
            4,
        ),
        // `touch`: ATTRIB, CLOSE_WRITE.
        ("touch", &[(0, Activity), (0, Change)], 0),
    ];
    for (name, script, complete) in patterns {
        assert_eq!(fires(script, 3000), [complete + q], "{name}");
    }
}

/// A writer that never closes the file is shown once it pauses; one that never pauses, every
/// [`CAP`].
#[test]
fn writers_without_a_close_are_shown_after_a_pause_or_the_cap() {
    let qi = ms(QUIET_INCOMPLETE);
    assert_eq!(fires(&[(0, Activity), (10, Activity)], 3000), [10 + qi]);
    let cap = ms(CAP);
    let steady: Vec<(u64, Kind)> = (0..3000).step_by(20).map(|t| (t, Activity)).collect();
    let got = fires(&steady, 2999);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0], cap, "the first a cap after the first write");
    assert!(got[1] - got[0] <= cap + 20, "{got:?}");
}

/// A file deleted (or moved away) and left so fires once the incomplete quiet period is over:
/// reading it again says it's gone. Deleted and created again, it is a change.
#[test]
fn a_deleted_file_fires_once_and_a_recreated_one_is_a_change() {
    assert_eq!(fires(&[(0, Activity)], 3000), [ms(QUIET_INCOMPLETE)]);
    assert_eq!(
        fires(&[(0, Activity), (100, Activity), (101, Change)], 3000),
        [101 + ms(QUIET)]
    );
}

/// Idle has no deadline: the watcher waits without a timeout (the idle CPU budget), and is idle
/// again after every fire.
#[test]
fn idle_has_no_deadline() {
    let t0 = Instant::now();
    let mut d = Debounce::default();
    assert_eq!(d.deadline(), None);
    assert!(!d.fire(t0 + CAP * 10));
    assert_eq!(wait_ms(d.deadline(), t0), None);
    d.event(Change, t0);
    assert_eq!(d.deadline(), Some(t0 + QUIET));
    assert_eq!(wait_ms(d.deadline(), t0), Some(ms(QUIET) as u32));
    assert!(!d.fire(t0 + QUIET - MS));
    assert!(d.fire(t0 + QUIET));
    assert_eq!(d.deadline(), None);
    assert!(!d.fire(t0 + QUIET * 2), "once");
}

/// Watching another file forgets what was pending for the last one.
#[test]
fn another_target_drops_what_was_pending() {
    let t0 = Instant::now();
    let mut d = Debounce::default();
    d.event(Change, t0);
    d.reset();
    assert_eq!(d.deadline(), None);
    assert!(!d.fire(t0 + CAP));
}

/// A wait rounds up, so it never ends before the deadline and spins.
#[test]
fn waits_round_up_to_whole_milliseconds() {
    let t0 = Instant::now();
    assert_eq!(wait_ms(Some(t0 + Duration::from_micros(1)), t0), Some(1));
    assert_eq!(wait_ms(Some(t0 + Duration::from_micros(1500)), t0), Some(2));
    assert_eq!(wait_ms(Some(t0), t0 + MS), Some(0), "overdue");
}

/// A report is a version to show only if it is for the target watched now, the window doesn't
/// have it, and the file is that version now: one the window overtook (`R`, the focus reload,
/// another document) or the file did (a newer save, whose read comes) is dropped, and one the
/// window takes late isn't (it took an older version meanwhile).
#[test]
fn reports_are_versions_the_window_lacks_and_the_file_has() {
    let mut r = Reloads::<u8> {
        generation: 3,
        ..Reloads::default()
    };
    let (s0, s1, s2): (Option<Stamp>, _, _) = (Some((None, 1)), Some((None, 2)), Some((None, 3)));
    let no_look = || panic!("no need to look");
    assert!(r.current(3, s1, s0, || s1));
    assert!(r.current(3, s2, s1, || s2), "read before s1 was shown");
    assert!(r.current(3, None, s0, || None), "the file gone");
    assert!(!r.current(3, None, None, || None), "gone again");
    assert!(r.current(3, s0, None, || s0), "back");
    assert!(!r.current(2, s1, s0, || s1), "another document's");
    assert!(!r.current(3, s1, s0, || s2), "the file moved on");
    assert!(!r.current(3, s1, s0, || None), "the file went meanwhile");
    assert!(!r.current(3, s0, s0, no_look), "the window read it itself");
    // Held, the held version is the one the window has.
    r.hold(0, s1);
    assert!(!r.current(3, s1, s0, no_look));
    assert!(r.current(3, s2, s0, || s2));
}

/// Another path is another generation and drops what was pending for the last; the same path
/// again only says which version is shown. Nothing fires with nothing watched, and a fire is a
/// read running (the focus reload waits for it) until it ends.
#[test]
fn a_target_is_a_generation_and_a_fire_a_read() {
    let mut s = State::default();
    let t0 = Instant::now();
    s.debounce.event(Change, t0);
    assert!(!s.fire(t0 + CAP), "nothing watched");
    assert!(s.set(Some(Path::new("a.md")), None, true));
    let g = s.generation;
    s.debounce.event(Change, t0);
    assert!(!s.set(Some(Path::new("a.md")), Some((None, 1)), false));
    assert_eq!(
        (s.generation, s.shown, s.empty),
        (g, Some((None, 1)), false)
    );
    assert!(s.debounce.deadline().is_some(), "still pending");
    assert!(s.set(Some(Path::new("b.md")), None, false));
    assert_ne!(s.generation, g);
    assert_eq!(
        s.debounce.deadline(),
        None,
        "the last file's change is dropped"
    );
    s.debounce.event(Change, t0);
    assert!(!s.fire(t0));
    assert!(!s.reading);
    assert!(s.fire(t0 + QUIET));
    assert!(s.reading);
}

/// A version that comes while the reader drags is held (the newest one), and the focus reload
/// leaves it alone meanwhile; the window reading the file itself drops it.
#[test]
fn versions_are_held_during_a_drag() {
    let mut r = Reloads::<u8>::default();
    let s = Some((None, 1));
    r.hold(1, None);
    r.hold(2, s);
    assert!(r.busy(), "the focus reload waits for it");
    assert_eq!(r.take_held(), Some((2, s)));
    assert_eq!(r.take_held(), None);
    assert!(!r.busy());
    r.hold(3, s);
    r.reset();
    assert_eq!(r.take_held(), None);
}

/// The file gone while a version is held: that one is the last version, shown with no stamp
/// (the focus reload doesn't swap it for an error), and a version back is taken.
#[test]
fn a_held_version_is_the_last_once_the_file_is_gone() {
    let mut r = Reloads::<u8>::default();
    let (s0, s1) = (Some((None, 1)), Some((None, 2)));
    r.gone();
    assert_eq!(r.take_held(), None, "nothing held: nothing to do");
    r.hold(1, s1);
    r.gone();
    assert!(!r.current(0, None, s0, || None), "gone again");
    assert!(r.current(0, s1, s0, || s1), "back");
    assert_eq!(r.take_held(), Some((1, None)));
}

/// When the reader lets go, the held version is shown only if the file is still that version
/// (or gone); otherwise it is dropped, the version shown stays, and the watcher's guarded read
/// of the file (which differs from the version shown) brings what it is now.
#[test]
fn a_held_version_is_checked_when_the_reader_lets_go() {
    let mut r = Reloads::<u8>::default();
    let (a, b, c) = (Some((None, 1)), Some((None, 2)), Some((None, 3)));
    assert_eq!(r.release(b), None, "nothing held");
    r.hold(1, b);
    assert_eq!(r.release(b), Some((1, b)), "still the file");
    r.hold(1, b);
    assert_eq!(r.release(a), None, "put back as shown");
    assert!(!r.busy());
    r.hold(1, b);
    assert_eq!(
        r.release(c),
        None,
        "moved on (mid-save, maybe): the watcher's read brings it"
    );
    r.hold(1, b);
    assert_eq!(r.release(None), Some((1, b)), "gone: the last version");
    r.hold(1, b);
    r.gone();
    assert_eq!(
        r.release(c),
        None,
        "gone, then back as another: read and reported"
    );
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vomd-watch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn rereading_finds_a_new_version_the_same_one_or_none() {
    let dir = temp_dir("reread");
    let doc = dir.join("doc.md");
    std::fs::write(&doc, "# One").unwrap();
    let no_wait = |_| panic!("no wait");
    let Reread::Text(text, stamp) = reread(&doc, None, false, no_wait) else {
        panic!("read");
    };
    assert_eq!(text, "# One");
    assert_eq!(stamp, view::stamp(&doc));
    assert_eq!(reread(&doc, stamp, false, no_wait), Reread::Same);
    std::fs::write(&doc, "# Two, longer").unwrap();
    assert!(
        matches!(reread(&doc, stamp, false, no_wait), Reread::Text(t, _) if t == "# Two, longer")
    );
    std::fs::remove_file(&doc).unwrap();
    assert_eq!(reread(&doc, stamp, false, no_wait), Reread::Gone);
    // A link to nothing is gone too.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("nothing.md"), &doc).unwrap();
        assert_eq!(reread(&doc, stamp, false, no_wait), Reread::Gone);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An empty file where text was shown waits once for the writer (here, the editor fills it
/// meanwhile); still empty after that, it is shown empty. Where nothing was shown, at once.
#[test]
fn an_emptied_file_is_given_time_to_fill() {
    let dir = temp_dir("empty");
    let doc = dir.join("doc.md");
    std::fs::write(&doc, "").unwrap();
    let mut waits = Vec::new();
    let got = reread(&doc, None, false, |d| {
        waits.push(d);
        std::fs::write(&doc, "# Filled").unwrap();
    });
    assert!(matches!(got, Reread::Text(t, _) if t == "# Filled"));
    assert_eq!(waits, [QUIET_INCOMPLETE]);
    std::fs::write(&doc, "").unwrap();
    let mut waits = 0;
    let got = reread(&doc, None, false, |_| waits += 1);
    assert!(matches!(got, Reread::Text(t, _) if t.is_empty()));
    assert_eq!(waits, 1, "emptied on purpose");
    let got = reread(&doc, None, true, |_| panic!("no wait"));
    assert!(matches!(got, Reread::Text(t, _) if t.is_empty()));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A file that can never be read is left at once (not a file, too large, not ours to read):
/// the version shown stays. (One that fails for a moment is tried again: [`reread`].)
#[test]
fn an_unreadable_file_is_left_at_once() {
    let dir = temp_dir("failed");
    let no_wait = |_| panic!("no wait");
    // A directory where the file was: there, but not a file to read.
    assert_eq!(reread(&dir, None, false, no_wait), Reread::Failed);
    // (Sparse: nothing is written, or read.)
    let big = dir.join("big.md");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(view::MAX_BYTES + 1)
        .unwrap();
    assert_eq!(reread(&big, None, false, no_wait), Reread::Failed);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.join("locked.md");
        std::fs::write(&locked, "# Mine").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o200)).unwrap();
        // (Root reads it anyway.)
        if std::fs::read(&locked).is_err() {
            assert_eq!(reread(&locked, None, false, no_wait), Reread::Failed);
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_path_splits_into_its_directory_and_name() {
    let s = |p: &str| split(Path::new(p)).map(|(d, n)| (d, n.into_string().unwrap()));
    assert_eq!(s("doc.md"), Some((".".into(), "doc.md".into())));
    assert_eq!(s("a/b/doc.md"), Some(("a/b".into(), "doc.md".into())));
    assert_eq!(s("/doc.md"), Some(("/".into(), "doc.md".into())));
    assert_eq!(s("a/.."), None);
}
