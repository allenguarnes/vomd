use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vomd-recent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `n` files `f0.md`, `f1.md`, … in `dir`, made.
fn files(dir: &Path, n: usize) -> Vec<PathBuf> {
    (0..n)
        .map(|i| {
            let p = dir.join(format!("f{i}.md"));
            std::fs::write(&p, "# hi\n").unwrap();
            p
        })
        .collect()
}

/// `paths` as the list keeps them ([`storable`]): in their canonical folder, which on Windows is
/// a verbatim path (`\\?\C:\…`) and on macOS may be another (`/var` is `/private/var`).
fn kept(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().map(|p| storable(p).unwrap()).collect()
}

#[test]
fn the_file_is_kept_in_the_state_folder() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        assert_eq!(
            path_from(env(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.local/state/vomd/recent"))
        );
        assert_eq!(
            path_from(env(&[("HOME", "/home/u"), ("XDG_STATE_HOME", "/s")])),
            Some(PathBuf::from("/s/vomd/recent"))
        );
        // A relative XDG_STATE_HOME is ignored, as the spec says.
        assert_eq!(
            path_from(env(&[("HOME", "/home/u"), ("XDG_STATE_HOME", "rel")])),
            Some(PathBuf::from("/home/u/.local/state/vomd/recent"))
        );
        assert_eq!(path_from(env(&[("HOME", "")])), None);
    }
    #[cfg(target_os = "macos")]
    assert_eq!(
        path_from(env(&[("HOME", "/Users/u")])),
        Some(PathBuf::from(
            "/Users/u/Library/Application Support/vomd/recent"
        ))
    );
    #[cfg(windows)]
    assert_eq!(
        path_from(env(&[("LOCALAPPDATA", r"C:\Users\u\AppData\Local")])),
        Some(PathBuf::from(r"C:\Users\u\AppData\Local\vomd\recent"))
    );
    assert_eq!(path_from(env(&[])), None);
    // A relative one would be under whatever folder VOMD was started in.
    assert_eq!(
        path_from(env(&[("HOME", "rel"), ("LOCALAPPDATA", "rel")])),
        None
    );
}

/// An absolute path in this platform's spelling.
fn abs(name: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"C:\d\{name}"))
    } else {
        PathBuf::from(format!("/d/{name}"))
    }
}

#[test]
fn parsing_keeps_absolute_paths_once_and_skips_the_rest() {
    let (a, b) = (abs("a.md"), abs("b.md"));
    let text = format!(
        "# comment\n\n{}\r\nrelative.md\n{}\n{}\n   \ngarbage \u{fffd} line\n",
        a.display(),
        b.display(),
        a.display()
    );
    assert_eq!(parse(&text), [a.clone(), b.clone()]);
    // What is written reads back the same.
    assert_eq!(parse(&text_of(&[a.clone(), b.clone()])), [a, b]);
    assert!(text_of(&[]).starts_with("# "), "the header");
    // At most KEPT.
    let many: String = (0..50)
        .map(|i| format!("{}\n", abs(&format!("{i}.md")).display()))
        .collect();
    let list = parse(&many);
    assert_eq!(list.len(), KEPT);
    assert_eq!(list[0], abs("0.md"));
}

fn text_of(list: &[PathBuf]) -> String {
    text(list)
}

#[test]
fn only_paths_that_read_back_are_stored() {
    let p = storable(Path::new("x.md")).unwrap();
    assert!(p.is_absolute(), "made absolute");
    assert!(p.ends_with("x.md"));
    assert_eq!(storable(Path::new("bad\nname.md")), None);
    assert_eq!(storable(Path::new("bad\rname.md")), None);
    // A trailing space isn't kept. Windows drops it when making the path absolute (as it does
    // when opening it: the file opened is the one without), so that one is kept there; spelled
    // verbatim, it is another file, and isn't.
    #[cfg(not(windows))]
    assert_eq!(storable(&abs("trailing.md ")), None);
    #[cfg(windows)]
    {
        let without = storable(&abs("trailing.md"));
        assert!(without.is_some());
        assert_eq!(storable(&abs("trailing.md ")), without);
        assert_eq!(storable(Path::new(r"\\?\C:\d\trailing.md ")), None);
    }
    // Not UTF-8: can't be a line of the file.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let raw = std::ffi::OsStr::from_bytes(b"/d/\xff.md");
        assert_eq!(storable(Path::new(raw)), None);
    }
}

#[test]
fn recording_puts_the_file_first_once_and_keeps_twenty() {
    let dir = temp_dir("record");
    let file = dir.join("state/vomd/recent");
    let mut r = Recent::new(Some(file.clone()));
    assert!(r.list().is_empty(), "no file yet");
    let fs = files(&dir, 25);
    let ks = kept(&fs);
    for f in &fs[..3] {
        assert!(r.record(f).unwrap());
    }
    assert_eq!(r.list(), [ks[2].clone(), ks[1].clone(), ks[0].clone()]);
    // Again: moves to the top, not twice.
    r.record(&fs[0]).unwrap();
    assert_eq!(r.list(), [ks[0].clone(), ks[2].clone(), ks[1].clone()]);
    for f in &fs {
        r.record(f).unwrap();
    }
    let list = r.list();
    assert_eq!(list.len(), KEPT);
    assert_eq!(list[0], ks[24]);
    assert_eq!(list[KEPT - 1], ks[5]);
    // The file is what it says it is.
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with(HEADER));
    assert_eq!(text.lines().count(), KEPT + 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_files_are_hidden_but_kept() {
    let dir = temp_dir("missing");
    let mut r = Recent::new(Some(dir.join("recent")));
    let fs = files(&dir, 12);
    let ks = kept(&fs);
    for f in &fs {
        r.record(f).unwrap();
    }
    std::fs::remove_file(&fs[11]).unwrap();
    std::fs::remove_file(&fs[9]).unwrap();
    let on = shown(r.list());
    assert_eq!(on.len(), SHOWN);
    assert_eq!(on[0], ks[10]);
    assert_eq!(on[1], ks[8]);
    assert!(!on.contains(&ks[9]));
    // Still in the file: the drive may come back.
    assert_eq!(r.list().len(), 12);
    // A folder isn't a file to show either.
    r.record(&dir).unwrap();
    assert_eq!(shown(r.list())[0], ks[10]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Two windows, each its own process with its own store, recording in turn: both are kept, and
/// what one removed stays removed for the other.
#[test]
fn two_windows_share_the_file() {
    let dir = temp_dir("two");
    let file = dir.join("recent");
    let (mut one, mut two) = (Recent::new(Some(file.clone())), Recent::new(Some(file)));
    let fs = files(&dir, 4);
    let ks = kept(&fs);
    one.record(&fs[0]).unwrap();
    two.record(&fs[1]).unwrap();
    one.record(&fs[2]).unwrap();
    two.record(&fs[3]).unwrap();
    let want = [ks[3].clone(), ks[2].clone(), ks[1].clone(), ks[0].clone()];
    assert_eq!(one.list(), want);
    assert_eq!(two.list(), want);
    // (A row is taken off by the path the list has, as the window does.)
    one.remove(&ks[2]).unwrap();
    assert_eq!(two.list(), [ks[3].clone(), ks[1].clone(), ks[0].clone()]);
    two.clear().unwrap();
    assert!(one.list().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A rewrite replaces the file whole (a temporary file renamed over it): a reader never sees
/// half of it, and nothing is left behind.
#[test]
fn rewrites_are_atomic() {
    let dir = temp_dir("atomic");
    let file = dir.join("recent");
    let mut r = Recent::new(Some(file.clone()));
    let fs = files(&dir, 3);
    let reader = {
        let file = file.clone();
        std::thread::spawn(move || {
            for _ in 0..2000 {
                if let Ok(text) = std::fs::read_to_string(&file) {
                    assert!(text.is_empty() || text.starts_with(HEADER) && text.ends_with('\n'));
                }
            }
        })
    };
    for i in 0..300 {
        r.record(&fs[i % 3]).unwrap();
    }
    reader.join().unwrap();
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_corrupt_or_huge_file_reads_as_what_parses() {
    let dir = temp_dir("corrupt");
    let file = dir.join("recent");
    let good = abs("good.md");
    let mut bytes = b"\x00\xff\xfe garbage\n".to_vec();
    bytes.extend_from_slice(good.to_str().unwrap().as_bytes());
    bytes.extend_from_slice(b"\n\xc3(\n");
    std::fs::write(&file, &bytes).unwrap();
    assert_eq!(read(&file), std::slice::from_ref(&good));
    // Huge: only its start is read, and a line cut short isn't taken.
    let mut huge = format!("{}\n", good.display());
    while huge.len() < MAX_FILE as usize + 10 {
        huge.push_str(&format!("{}\n", abs(&"x".repeat(100)).display()));
    }
    std::fs::write(&file, &huge).unwrap();
    let list = read(&file);
    assert_eq!(list[0], good);
    assert!(list.len() <= KEPT);
    // A folder in its place, or nothing: empty.
    assert!(read(&dir).is_empty());
    assert!(read(&dir.join("none")).is_empty());
    // Recording over a corrupt file keeps what parsed.
    std::fs::write(&file, &bytes).unwrap();
    let fs = files(&dir, 1);
    let mut r = Recent::new(Some(file.clone()));
    r.record(&fs[0]).unwrap();
    assert_eq!(r.list(), [kept(&fs).remove(0), good]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `--no-settings` (no file) and `recent_files = off` neither read nor write anything; off
/// deletes the file.
#[test]
fn without_a_file_or_off_nothing_is_kept() {
    let dir = temp_dir("off");
    let fs = files(&dir, 2);
    let mut none = Recent::new(None);
    assert!(!none.on());
    assert!(!none.record(&fs[0]).unwrap());
    assert!(none.list().is_empty() && shown(none.list()).is_empty());
    none.clear().unwrap();
    none.set_on(false).unwrap();
    let file = dir.join("recent");
    let mut r = Recent::new(Some(file.clone()));
    assert!(r.on());
    r.record(&fs[0]).unwrap();
    assert!(file.exists());
    r.set_on(false).unwrap();
    assert!(!file.exists(), "off forgets the list");
    assert!(!r.on());
    assert!(!r.record(&fs[1]).unwrap());
    assert!(!file.exists(), "nothing recorded while off");
    // Written by another window meanwhile: still not read.
    std::fs::write(&file, text(&[fs[1].clone()])).unwrap();
    assert!(r.list().is_empty());
    r.set_on(true).unwrap();
    assert_eq!(r.list(), [fs[1].clone()]);
    r.clear().unwrap();
    assert!(!file.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A list made read-only on purpose is used and not added to (quietly), but Clear and off still
/// delete it: they promise nothing is kept.
#[cfg(unix)]
#[test]
fn a_read_only_list_is_not_added_to_but_cleared() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("frozen");
    let file = dir.join("recent");
    let fs = files(&dir, 3);
    std::fs::write(&file, text(&[fs[0].clone(), fs[2].clone()])).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let mut r = Recent::new(Some(file.clone()));
    assert!(!r.record(&fs[1]).unwrap(), "not kept, and no error");
    assert_eq!(r.list(), [fs[0].clone(), fs[2].clone()]);
    assert!(r.remove(&fs[0]).is_err(), "said: the row stays");
    r.clear().unwrap();
    assert!(!file.exists());
    std::fs::write(&file, text(&[fs[0].clone()])).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    r.set_on(false).unwrap();
    assert!(!file.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Taking the last row off deletes the file, as Clear does.
#[test]
fn removing_the_last_row_deletes_the_file() {
    let dir = temp_dir("last");
    let file = dir.join("recent");
    let fs = files(&dir, 1);
    let mut r = Recent::new(Some(file.clone()));
    r.record(&fs[0]).unwrap();
    r.remove(&dir.join("not-there.md")).unwrap();
    assert!(file.exists(), "nothing removed: left as it is");
    r.remove(&kept(&fs)[0]).unwrap();
    assert!(!file.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Windows: a file given plainly (`C:\…`, as on a command line) is recorded as the verbatim
/// path the list keeps (`\\?\C:\…`), so the two spellings are one entry.
#[cfg(windows)]
#[test]
fn plain_and_verbatim_spellings_are_one_entry() {
    let dir = temp_dir("spellings");
    let file = dir.join("recent");
    let fs = files(&dir, 1);
    let verbatim = kept(&fs).remove(0);
    let plain = PathBuf::from(unverbatim(verbatim.to_str().unwrap()).unwrap());
    let mut r = Recent::new(Some(file.clone()));
    r.record(&plain).unwrap();
    assert_eq!(r.list(), std::slice::from_ref(&verbatim));
    r.record(&verbatim).unwrap();
    assert_eq!(r.list(), std::slice::from_ref(&verbatim));
    r.remove(&verbatim).unwrap();
    assert!(!file.exists(), "the last row taken off");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The list and the folders made for it are the owner's only.
#[cfg(unix)]
#[test]
fn the_list_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("private");
    let file = dir.join("state/vomd/recent");
    let fs = files(&dir, 1);
    Recent::new(Some(file.clone())).record(&fs[0]).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&dir.join("state/vomd")), 0o700);
    assert_eq!(mode(&dir.join("state")), 0o700);
    // A temporary file a crashed process of the same number left (world-readable) is replaced,
    // not reused with its permissions.
    std::fs::remove_file(&file).unwrap();
    let tmp = dir.join(format!("state/vomd/recent.{}.tmp", std::process::id()));
    std::fs::write(&tmp, "old").unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
    Recent::new(Some(file.clone())).record(&fs[0]).unwrap();
    assert_eq!(mode(&file), 0o600);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Windows' verbatim paths are shown as the reader knows them: a drive's and a share's.
#[test]
fn verbatim_paths_are_made_plain() {
    assert_eq!(
        unverbatim(r"\\?\C:\docs\a.md").as_deref(),
        Some(r"C:\docs\a.md")
    );
    assert_eq!(
        unverbatim(r"\\?\UNC\server\share\a.md").as_deref(),
        Some(r"\\server\share\a.md")
    );
    assert_eq!(unverbatim(r"\\?\Volume{x}\a.md"), None);
    assert_eq!(unverbatim(r"C:\a.md"), None);
    // Only in a row's words: the path kept is the verbatim one (see below).
    #[cfg(windows)]
    assert_eq!(
        label(Path::new(r"\\?\C:\docs\a.md"), None),
        ("a.md".into(), r"C:\docs\".into())
    );
}

/// Windows: the list keeps the canonical verbatim path as it is, so a file only a verbatim path
/// reaches (`chapter.md.`, next to a `chapter.md`) opens again, not its plain-named neighbour.
/// (Runs on Windows only, in CI: Win32's name rules can't be reproduced elsewhere.)
#[cfg(windows)]
#[test]
fn verbatim_paths_are_kept_and_reopen_the_same_file() {
    let dir = temp_dir("verbatim");
    let canon = std::fs::canonicalize(&dir).unwrap();
    let plain = canon.join("chapter.md");
    let dotted = PathBuf::from(format!("{}.", plain.display()));
    std::fs::write(&plain, "plain").unwrap();
    std::fs::write(&dotted, "dotted").unwrap();
    let mut r = Recent::new(Some(dir.join("recent")));
    assert!(r.record(&dotted).unwrap());
    let kept = r.list()[0].clone();
    assert!(kept.to_str().unwrap().starts_with(r"\\?\"), "{kept:?}");
    assert!(kept.to_str().unwrap().ends_with("chapter.md."), "{kept:?}");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "dotted");
    assert_eq!(shown(r.list()), [kept]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A look runs at a time; one asked for meanwhile runs after it.
#[test]
fn looks_run_one_at_a_time() {
    let mut looks = Looks::default();
    let g = looks.start().unwrap();
    assert_eq!(looks.start(), None, "one running");
    assert_eq!(looks.start(), None);
    assert_eq!(looks.done(g), (true, true), "its answer stands; look again");
    let g = looks.start().unwrap();
    assert_eq!(looks.done(g), (true, false));
}

/// A look still running when the list is emptied (Clear, off): its late answer is thrown away,
/// and so is that of a look started before, which a hung mount may hold for long.
#[test]
fn a_late_answer_from_before_a_clear_is_thrown_away() {
    let mut looks = Looks::default();
    let g = looks.start().unwrap();
    looks.invalidate(); // Clear while it runs (the window emptied its rows at once)
    assert_eq!(looks.start(), None, "still one at a time");
    assert_eq!(
        looks.done(g),
        (false, true),
        "obsolete; the look asked for meanwhile runs"
    );
    let g2 = looks.start().unwrap();
    assert_eq!(
        looks.done(g2),
        (true, false),
        "a look after the Clear stands"
    );
    // Off, then on again, with a look from before off answering last.
    let g3 = looks.start().unwrap();
    looks.invalidate();
    assert_eq!(looks.done(g3), (false, false));
}

/// The same file reached through a symlink, or with `..`, is one entry: its canonical path.
#[cfg(unix)]
#[test]
fn paths_are_canonical() {
    let dir = temp_dir("canonical");
    let file = dir.join("recent");
    let fs = files(&dir, 1);
    let real = std::fs::canonicalize(&fs[0]).unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::os::unix::fs::symlink(&dir, dir.join("link")).unwrap();
    let mut r = Recent::new(Some(file));
    r.record(&dir.join("link/f0.md")).unwrap();
    r.record(&dir.join("sub/../f0.md")).unwrap();
    r.record(&fs[0]).unwrap();
    assert_eq!(r.list(), std::slice::from_ref(&real));
    // A symlinked file stays the link (repointed, it opens its new target), in the canonical
    // folder.
    std::os::unix::fs::symlink(&fs[0], dir.join("today.md")).unwrap();
    r.record(&dir.join("link/today.md")).unwrap();
    assert_eq!(r.list()[0], real.with_file_name("today.md"));
    // A file that isn't there (any more) is kept as given, made absolute.
    assert_eq!(
        storable(Path::new("/no/such/x.md")),
        Some("/no/such/x.md".into())
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn labels_show_the_home_folder_as_a_tilde() {
    #[cfg(unix)]
    {
        let home = Path::new("/home/u");
        let l = |p: &str| label(Path::new(p), Some(home));
        assert_eq!(l("/home/u/notes/a.md"), ("a.md".into(), "~/notes/".into()));
        assert_eq!(l("/home/u/a.md"), ("a.md".into(), "~/".into()));
        assert_eq!(
            l("/home/user2/a.md"),
            ("a.md".into(), "/home/user2/".into())
        );
        assert_eq!(l("/a.md"), ("a.md".into(), "/".into()));
        assert_eq!(
            label(Path::new("/srv/x/b.md"), None),
            ("b.md".into(), "/srv/x/".into())
        );
    }
    #[cfg(windows)]
    assert_eq!(
        label(Path::new(r"C:\docs\a.md"), None),
        ("a.md".into(), r"C:\docs\".into())
    );
}

#[test]
fn menu_titles_name_the_folder_only_for_a_name_seen_twice() {
    let list = [abs("a/README.md"), abs("notes.md"), abs("b/README.md")];
    let titles = menu_titles(&list, None);
    assert_eq!(titles[1], "notes.md");
    assert!(titles[0].starts_with("README.md — ") && titles[0].contains('a'));
    assert!(titles[2].starts_with("README.md — ") && titles[2].contains('b'));
    assert_ne!(titles[0], titles[2]);
    assert!(menu_titles(&[], None).is_empty());
}
