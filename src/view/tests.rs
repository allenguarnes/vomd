use super::*;
// Only the tests that need the embedded fonts use it.
#[cfg(all(feature = "images", feature = "embed-fonts"))]
use crate::layout::ImageSize;
use winit::keyboard::SmolStr;

fn ch(s: &str) -> Key {
    Key::Character(SmolStr::new(s))
}

#[test]
fn keys() {
    let none = ModifiersState::empty();
    let ctrl = ModifiersState::CONTROL;
    let shift = ModifiersState::SHIFT;
    let cases = [
        (ch("j"), none, Some(Action::Lines(1))),
        (ch("k"), none, Some(Action::Lines(-1))),
        (
            Key::Named(NamedKey::ArrowDown),
            none,
            Some(Action::Lines(1)),
        ),
        (Key::Named(NamedKey::PageUp), none, Some(Action::Pages(-1))),
        (Key::Named(NamedKey::Space), none, Some(Action::Pages(1))),
        (Key::Named(NamedKey::Space), shift, Some(Action::Pages(-1))),
        (ch("g"), none, Some(Action::Top)),
        (ch("G"), shift, Some(Action::Bottom)),
        (ch("g"), shift, Some(Action::Bottom)),
        // CapsLock: the character arrives uppercase without Shift.
        (ch("G"), none, Some(Action::Top)),
        (ch("J"), none, Some(Action::Lines(1))),
        (ch("K"), none, Some(Action::Lines(-1))),
        (ch("R"), none, Some(Action::Reload)),
        (ch("T"), none, Some(Action::CycleTheme)),
        (ch("Q"), none, Some(Action::Quit)),
        (ch("W"), ctrl, Some(Action::Quit)),
        (Key::Named(NamedKey::End), none, Some(Action::Bottom)),
        (ch("r"), none, Some(Action::Reload)),
        (Key::Named(NamedKey::F5), none, Some(Action::Reload)),
        (ch("t"), none, Some(Action::CycleTheme)),
        (ch("q"), none, Some(Action::Quit)),
        (Key::Named(NamedKey::Escape), none, Some(Action::Escape)),
        (ch("="), ctrl, Some(Action::ZoomIn)),
        (ch("+"), ctrl | shift, Some(Action::ZoomIn)),
        (ch("-"), ctrl, Some(Action::ZoomOut)),
        (ch("0"), ModifiersState::SUPER, Some(Action::ZoomReset)),
        (ch("w"), ctrl, Some(Action::Quit)),
        // Shortcuts never fall through to plain keys.
        (ch("j"), ctrl, None),
        (ch("q"), ModifiersState::ALT, None),
        (ch("="), none, None),
        (ch("x"), none, None),
        (ch("b"), none, Some(Action::ToggleToolbar)),
        (ch("B"), none, Some(Action::ToggleToolbar)),
        (ch("["), none, Some(Action::PrevHeading)),
        (ch("]"), none, Some(Action::NextHeading)),
        (Key::Named(NamedKey::Backspace), none, Some(Action::Back)),
        (
            Key::Named(NamedKey::ArrowLeft),
            ModifiersState::ALT,
            Some(Action::Back),
        ),
        (ch("["), ModifiersState::SUPER, Some(Action::Back)),
        (ch("["), ctrl, Some(Action::Back)),
        (Key::Named(NamedKey::ArrowLeft), none, None),
        (ch("]"), ModifiersState::ALT, None),
    ];
    for (key, mods, want) in cases {
        assert_eq!(key_action(&key, mods), want, "{key:?} {mods:?}");
    }
}

#[test]
fn theme_cycle() {
    assert_eq!(ThemeMode::System.next(), ThemeMode::Light);
    assert_eq!(ThemeMode::Light.next(), ThemeMode::Dark);
    assert_eq!(ThemeMode::Dark.next(), ThemeMode::System);
}

#[test]
fn titles() {
    assert_eq!(title(&Input::File("/a/b/notes.md".into())), "notes.md");
    assert_eq!(title(&Input::None), "vomd");
    assert!(title(&Input::Stdin).starts_with("vomd"));
}

#[test]
fn read_errors_and_lossy_utf8() {
    let err = read_input(
        &Input::File("/nonexistent/vomd-test.md".into()),
        Origin::Argument,
    )
    .unwrap_err();
    assert!(
        err.starts_with("Cannot open /nonexistent/vomd-test.md"),
        "{err}"
    );
    let dir = std::env::temp_dir().join(format!("vomd-view-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.md");
    std::fs::write(&path, b"ok \xff end").unwrap();
    let (text, st) = read_input(&Input::File(path.clone()), Origin::Argument).unwrap();
    assert_eq!(text, "ok \u{fffd} end");
    assert_eq!(st, stamp(&path));
    assert_eq!(st.unwrap().1, 8);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        read_input(&Input::None, Origin::Argument).unwrap(),
        (String::new(), None)
    );
}

#[test]
fn cursor_follows_the_link_under_the_pointer() {
    let c = hover_change;
    // Entering and leaving a link.
    assert_eq!(
        c(false, true, true),
        HoverChange {
            pointer: true,
            set_cursor: true,
            redraw: true
        }
    );
    assert_eq!(
        c(true, false, true),
        HoverChange {
            pointer: false,
            set_cursor: true,
            redraw: true
        }
    );
    // From one link to another: the preview changes, the cursor stays.
    assert_eq!(
        c(true, true, true),
        HoverChange {
            pointer: true,
            set_cursor: false,
            redraw: true
        }
    );
    // Nothing happened.
    assert_eq!(
        c(false, false, false),
        HoverChange {
            pointer: false,
            set_cursor: false,
            redraw: false
        }
    );
    // A new document reset the hover behind the shell's back (hover unchanged since then,
    // or not under a link any more): the hand must still go.
    assert_eq!(
        c(true, false, false),
        HoverChange {
            pointer: false,
            set_cursor: true,
            redraw: false
        }
    );
    // And one that has a link under the pointer after all takes it.
    assert_eq!(
        c(false, true, false),
        HoverChange {
            pointer: true,
            set_cursor: true,
            redraw: false
        }
    );
}

/// What a link, Back or a drop can reach: only regular files, and a bounded amount of them.
#[test]
fn navigation_refuses_what_is_not_a_regular_file() {
    let dir = std::env::temp_dir().join(format!("vomd-view-read-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let read = |p: &Path| read_input(&Input::File(p.to_owned()), Origin::Navigation);
    let err = read(&dir).unwrap_err();
    assert!(err.contains("not a regular file"), "{err}");
    #[cfg(unix)]
    {
        let link = dir.join("zero.md");
        std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
        let err = read(&link).unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");
        // A symlink to a regular file is followed.
        std::fs::write(dir.join("real.md"), "# hi").unwrap();
        let ok = dir.join("alias.md");
        std::os::unix::fs::symlink(dir.join("real.md"), &ok).unwrap();
        assert_eq!(read(&ok).unwrap().0, "# hi");
        // A FIFO blocks an open until something writes to it: it must not be opened.
        let fifo = dir.join("pipe.md");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            let (tx, rx) = std::sync::mpsc::channel();
            let path = fifo.clone();
            std::thread::spawn(move || {
                let _ = tx.send(read_input(&Input::File(path), Origin::Navigation));
            });
            let err = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("reading a FIFO blocked")
                .unwrap_err();
            assert!(err.contains("not a regular file"), "{err}");
        }
        // The refusal comes from the look at the path, before any open: a FIFO nobody may open
        // is still "not a regular file" rather than "permission denied" (as root the open
        // succeeds and the opened file is refused with the same message).
        let shut = dir.join("shut.md");
        let made = std::process::Command::new("mkfifo")
            .args(["-m", "200"])
            .arg(&shut)
            .status();
        if made.is_ok_and(|s| s.success()) {
            let err = read(&shut).unwrap_err();
            assert!(err.contains("not a regular file"), "{err}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `vomd [FILE]` takes whatever the shell names: `<(cmd)`, `/dev/stdin`, a FIFO.
#[cfg(unix)]
#[test]
fn the_argument_may_be_a_pipe_or_a_device() {
    let dir = std::env::temp_dir().join(format!("vomd-view-arg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let arg = |p: &Path| read_input(&Input::File(p.to_owned()), Origin::Argument);
    // A device reads (as empty here), and has no stamp to compare.
    assert_eq!(arg(Path::new("/dev/null")).unwrap(), (String::new(), None));
    // The size limit holds for everything, an endless device too.
    let err = read_input_limited(&Input::File("/dev/zero".into()), Origin::Argument, 1 << 20)
        .unwrap_err();
    assert!(err.ends_with("is larger than 1 MB"), "{err}");
    // A directory can't be read, with a message.
    assert!(arg(&dir).is_err());
    // A FIFO is read once its writer has written (the open waits for it).
    let fifo = dir.join("pipe.md");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if made.is_ok_and(|s| s.success()) {
        let path = fifo.clone();
        let writer = std::thread::spawn(move || std::fs::write(path, "# piped\n"));
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(read_input(&Input::File(path), Origin::Argument));
        });
        let read = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("reading a FIFO with a writer blocked");
        assert_eq!(read.unwrap(), ("# piped\n".to_owned(), None));
        writer.join().unwrap().unwrap();
        // Not again: reload and Back keep what was read.
        assert!(!rereadable(&Input::File(fifo)));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn only_regular_files_are_read_again() {
    let dir = std::env::temp_dir().join(format!("vomd-view-again-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.md");
    std::fs::write(&file, "# a").unwrap();
    assert!(rereadable(&Input::File(file.clone())));
    // A file that is gone is read again to say why.
    assert!(rereadable(&Input::File(dir.join("gone.md"))));
    assert!(!rereadable(&Input::File(dir.clone())));
    assert!(!rereadable(&Input::Stdin));
    assert!(!rereadable(&Input::None));
    #[cfg(unix)]
    {
        assert!(!rereadable(&Input::File("/dev/null".into())));
        let link = dir.join("alias.md");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(rereadable(&Input::File(link)));
        let dev = dir.join("dev.md");
        std::os::unix::fs::symlink("/dev/null", &dev).unwrap();
        assert!(!rereadable(&Input::File(dev)));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reads_stop_at_the_limit() {
    use std::io::Cursor;
    assert_eq!(
        read_limited(Cursor::new(b"abcd"), 0, 4).unwrap().unwrap(),
        b"abcd"
    );
    assert_eq!(read_limited(Cursor::new(b"abcde"), 0, 4).unwrap(), None);
    // An endless reader ends at the limit, not at the end of memory.
    assert_eq!(read_limited(std::io::repeat(b'x'), 0, 1000).unwrap(), None);
    // The size hint is only a hint, whatever it says.
    assert_eq!(
        read_limited(Cursor::new(b"ab"), u64::MAX, 4)
            .unwrap()
            .unwrap(),
        b"ab"
    );
    let dir = std::env::temp_dir().join(format!("vomd-view-limit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.md");
    std::fs::write(&path, "0123456789").unwrap();
    let input = Input::File(path);
    assert_eq!(
        read_input_limited(&input, Origin::Navigation, 10)
            .unwrap()
            .0,
        "0123456789"
    );
    let err = read_input_limited(&input, Origin::Argument, 9).unwrap_err();
    assert!(err.ends_with("is larger than 0 MB"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
    const { assert!(MAX_BYTES > 10 << 20, "well above the 10 MB notice") };
}

#[cfg(feature = "embed-fonts")]
mod with_fonts {
    use super::*;
    use std::time::Duration;

    fn view(src: &str, w: u32, h: u32) -> View {
        let mut v = View::new(Fonts::new().unwrap(), Doc::parse(src), None, (w, h), 1.0);
        v.ensure();
        v
    }

    fn settle(v: &mut View) {
        while !v.idle(Instant::now() + Duration::from_secs(1)) {}
        v.ensure();
    }

    fn paras(n: usize) -> String {
        (0..n)
            .map(|i| format!("Paragraph {i} with a few words of text.\n\n"))
            .collect()
    }

    /// `n` paragraphs of very different lengths (so their estimated heights are off).
    fn mixed(n: usize) -> String {
        (0..n)
            .map(|i| {
                let words = 3 + (i * 37) % 90;
                let text: Vec<String> = (0..words).map(|w| format!("word{}", i + w)).collect();
                format!("{}\n\n", text.join(" "))
            })
            .collect()
    }

    #[test]
    fn scrolling_clamps() {
        let now = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        assert_eq!(v.scroll(), 0.0);
        v.apply(Action::Lines(-3), now);
        assert_eq!(v.scroll(), 0.0, "can't scroll above the top");
        v.apply(Action::Lines(2), now);
        assert!((v.scroll() - 48.0).abs() < 1e-3, "{}", v.scroll());
        v.apply(Action::Pages(1), now);
        assert!((v.scroll() - (48.0 + 400.0 - 24.0)).abs() < 1e-3);
        v.scroll_by(1e9, now);
        assert!((v.scroll() - v.max_scroll()).abs() < 1e-3);
        v.apply(Action::Top, now);
        assert_eq!(v.scroll(), 0.0);
        v.wheel_lines(1.0, now);
        assert!((v.scroll() - 72.0).abs() < 1e-3, "one notch = three lines");
    }

    #[test]
    fn short_document_does_not_scroll() {
        let now = Instant::now();
        let mut v = view("# Short\n\nOnly a line.", 600, 800);
        settle(&mut v);
        assert_eq!(v.max_scroll(), 0.0);
        v.apply(Action::Pages(3), now);
        v.apply(Action::Bottom, now);
        assert_eq!(v.scroll(), 0.0);
    }

    /// `End` lands at the true end even though the blocks near it are only estimated until
    /// measured.
    #[test]
    fn bottom_reaches_the_end() {
        let now = Instant::now();
        let mut v = view(&paras(3000), 500, 300);
        v.apply(Action::Bottom, now);
        v.ensure();
        let n = v.doc().blocks.len();
        assert!((v.scroll() - v.max_scroll()).abs() < 1e-3);
        let range = v.layout().range_exact(v.scroll(), v.scroll() + 300.0);
        assert_eq!(range.end, n, "the last block is on screen");
        let last = &v.layout().geoms()[n - 1];
        assert!(last.measured && last.content.is_some());
    }

    #[test]
    fn zoom_and_resize_keep_the_reading_position() {
        let now = Instant::now();
        let mut v = view(&paras(300), 700, 500);
        settle(&mut v);
        v.set_scroll(v.layout().top(120) + 5.0);
        let block = v.anchor().block;
        assert_eq!(block, 120);
        for action in [
            Action::ZoomIn,
            Action::ZoomIn,
            Action::ZoomOut,
            Action::ZoomReset,
        ] {
            v.apply(action, now);
            v.ensure();
            assert_eq!(v.anchor().block, block, "{action:?}");
        }
        v.resize(350, 500);
        v.ensure();
        assert_eq!(v.anchor().block, block, "narrower");
        v.set_hidpi(2.0);
        v.ensure();
        assert_eq!(v.anchor().block, block, "HiDPI");
        assert_eq!(v.scale(), 2.0);
    }

    /// A lot of text that wraps differently at every width.
    fn wrapping(n: usize) -> String {
        (0..n)
            .map(|_| format!("{}\n\n", "some normal words to wrap ".repeat(15)))
            .collect()
    }

    /// Narrower and taller at once: the old heights (not rewrapped) would make the document
    /// end above the position, but the new wrapping doesn't, so the position stays.
    #[test]
    fn resize_keeps_the_position_when_narrower_and_taller() {
        let now = Instant::now();
        let mut v = view(&wrapping(100), 900, 500);
        v.apply(Action::Bottom, now);
        v.ensure();
        let before = v.anchor();
        assert!(before.block >= 90, "{before:?}");
        v.resize(300, 900);
        assert_eq!(v.anchor(), before, "right after the resize");
        v.ensure();
        assert_eq!(v.anchor(), before, "after measuring");
        assert!(v.scroll() <= v.max_scroll() + 0.5);
        // The same for the strip above the document area, and for a size change in two steps.
        v.set_top_inset(50);
        v.resize(280, 1000);
        v.set_top_inset(0);
        v.ensure();
        assert_eq!(v.anchor(), before, "inset");
        // Moving by input before the next paint doesn't clamp against the old wrapping either.
        v.resize(900, 500);
        v.resize(300, 900);
        v.wheel_lines(-1.0, now);
        v.ensure();
        assert!(v.anchor().block <= before.block, "{:?}", v.anchor());
        assert!(v.scroll() <= v.max_scroll() + 0.5);
    }

    /// A zoom or HiDPI change measures around the position before anything clamps it.
    #[test]
    fn zoom_and_hidpi_keep_the_position_at_the_end() {
        let now = Instant::now();
        let mut v = view(&wrapping(100), 900, 500);
        v.set_top_inset(100);
        v.apply(Action::Bottom, now);
        v.ensure();
        let before = v.anchor();
        v.set_hidpi(1.5);
        v.set_top_inset(0);
        assert_eq!(v.anchor(), before, "HiDPI");
        v.set_top_inset(100);
        v.apply(Action::ZoomOut, now);
        v.set_top_inset(0);
        assert_eq!(v.anchor(), before, "zoom out");
        assert!(v.scroll() <= v.max_scroll() + 0.5);
    }

    /// A window that really got shorter text than the position settles at the real end.
    #[test]
    fn resize_wider_and_taller_clamps_to_the_real_end() {
        let now = Instant::now();
        let mut v = view(&wrapping(100), 300, 500);
        v.apply(Action::Bottom, now);
        v.ensure();
        v.resize(1200, 1200);
        v.ensure();
        let n = v.doc().blocks.len();
        assert!((v.scroll() - v.max_scroll()).abs() < 1e-3);
        assert_eq!(
            v.layout().range_exact(v.scroll(), v.scroll() + 1200.0).end,
            n
        );
        let last = &v.layout().geoms()[n - 1];
        assert!(last.measured && last.content.is_some());
        // A document that fits the new window altogether starts at its top.
        let mut v = view(&wrapping(5), 300, 300);
        v.apply(Action::Bottom, now);
        v.resize(1200, 1200);
        v.ensure();
        assert_eq!(v.scroll(), 0.0);
    }

    #[test]
    fn zoom_limits() {
        let now = Instant::now();
        let mut v = view("text", 400, 300);
        for _ in 0..40 {
            v.apply(Action::ZoomIn, now);
        }
        assert_eq!(v.zoom(), ZOOM_MAX);
        for _ in 0..40 {
            v.apply(Action::ZoomOut, now);
        }
        assert_eq!(v.zoom(), ZOOM_MIN);
        v.apply(Action::ZoomReset, now);
        assert_eq!((v.zoom(), v.scale()), (100, 1.0));
    }

    #[test]
    fn themes() {
        let now = Instant::now();
        let mut v = view("text", 400, 300);
        assert!(!v.dark());
        assert!(v.set_system_dark(true), "system dark shows");
        assert!(!v.set_system_dark(true), "no change");
        v.apply(Action::CycleTheme, now);
        assert_eq!((v.theme_mode(), v.dark()), (ThemeMode::Light, false));
        assert!(
            !v.set_system_dark(false),
            "an explicit theme ignores the system"
        );
        v.apply(Action::CycleTheme, now);
        assert_eq!((v.theme_mode(), v.dark()), (ThemeMode::Dark, true));
        v.apply(Action::CycleTheme, now);
        assert_eq!((v.theme_mode(), v.dark()), (ThemeMode::System, false));
        let mut buf = vec![0; 400 * 300];
        v.set_theme_mode(ThemeMode::Dark);
        v.paint(&mut buf, 400, now);
        assert_eq!(buf[0], DARK.bg);
    }

    /// An edit above the heading being read doesn't move the text at the top of the window.
    #[test]
    fn reload_follows_the_heading() {
        let body = paras(40);
        let src = format!("# Intro\n\n{body}## Target\n\n{body}");
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let target = v.doc().headings[1].block as usize;
        v.set_scroll(v.layout().top(target) + 30.0);
        let edited =
            format!("# Intro\n\nA new paragraph.\n\nAnd another.\n\n{body}## Target\n\n{body}");
        v.replace(Doc::parse(&edited), None);
        v.ensure();
        let new_target = v
            .doc()
            .headings
            .iter()
            .find(|h| h.slug == "target")
            .unwrap();
        assert_eq!(new_target.block as usize, target + 2);
        assert!(
            (v.scroll() - (v.layout().top(new_target.block as usize) + 30.0)).abs() < 1e-3,
            "{} vs {}",
            v.scroll(),
            v.layout().top(new_target.block as usize) + 30.0
        );
    }

    #[test]
    fn reload_without_headings_keeps_the_block_and_clamps() {
        let mut v = view(&paras(100), 600, 400);
        settle(&mut v);
        v.set_scroll(v.layout().top(60));
        v.replace(Doc::parse(&paras(100)), None);
        assert_eq!(v.anchor().block, 60);
        v.replace(Doc::parse(&paras(10)), None);
        v.ensure();
        assert!(v.scroll() <= v.max_scroll() + 0.5);
        v.replace(Doc::default(), Some("Cannot open x".into()));
        assert_eq!(v.message(), Some("Cannot open x"));
        assert_eq!(v.anchor(), Anchor::TOP);
    }

    #[test]
    fn open_starts_at_the_top() {
        let mut v = view(&paras(100), 600, 400);
        settle(&mut v);
        v.set_scroll(500.0);
        v.open(Doc::parse(&paras(100)), None);
        assert_eq!(v.anchor(), Anchor::TOP);
    }

    #[test]
    fn message_is_centered() {
        let fonts = Fonts::new().unwrap();
        let mut v = View::new(
            fonts,
            Doc::default(),
            Some("Drop a Markdown file here".into()),
            (600, 400),
            1.0,
        );
        let mut buf = vec![0; 600 * 400];
        v.paint(&mut buf, 600, Instant::now());
        let ink: Vec<(usize, usize)> = (0..400)
            .flat_map(|y| (0..600).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[y * 600 + x] != LIGHT.bg)
            .collect();
        assert!(!ink.is_empty());
        let (x0, x1) = (
            ink.iter().map(|p| p.0).min().unwrap(),
            ink.iter().map(|p| p.0).max().unwrap(),
        );
        let (y0, y1) = (
            ink.iter().map(|p| p.1).min().unwrap(),
            ink.iter().map(|p| p.1).max().unwrap(),
        );
        assert!((x0 as i32 + x1 as i32 - 600).abs() <= 4, "{x0}..{x1}");
        assert!((y0 as i32 + y1 as i32 - 400).abs() <= 12, "{y0}..{y1}");
    }

    /// An error message is shown with the same fallback fonts as a document: its uncovered
    /// characters are reported, once.
    #[test]
    fn message_characters_are_scanned_once() {
        let mut v = View::new(
            Fonts::new().unwrap(),
            Doc::default(),
            Some("Cannot open /tmp/한국어-문서.md".into()),
            (600, 400),
            1.0,
        );
        assert!(v.take_missing().is_empty(), "nothing painted yet");
        let mut buf = vec![0; 600 * 400];
        v.paint(&mut buf, 600, Instant::now());
        let missing = v.take_missing();
        assert!(
            missing.contains(&'한') && missing.contains(&'문'),
            "{missing:?}"
        );
        assert!(missing.iter().all(|&c| c as u32 > 0x7f), "{missing:?}");
        v.paint(&mut buf, 600, Instant::now());
        assert!(
            v.take_missing().is_empty(),
            "the same characters, the same scan"
        );
        // The same characters in the next message (a failed reload) aren't new either.
        v.replace(Doc::default(), Some("Cannot open 한국어".into()));
        v.paint(&mut buf, 600, Instant::now());
        assert!(v.take_missing().is_empty());
        v.replace(Doc::default(), Some("Cannot open 字体".into()));
        v.paint(&mut buf, 600, Instant::now());
        let new = v.take_missing();
        assert!(new.contains(&'字') && !new.contains(&'한'), "{new:?}");
    }

    #[test]
    fn scrollbar_drag() {
        let now = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        let thumb = v.thumb_at(0.0).unwrap();
        let (tx, ty) = (f64::from(thumb.x1 - 1), f64::from(thumb.y0 + 2));
        assert!(
            !v.press(100.0, 100.0, now),
            "presses in the text aren't the scrollbar's"
        );
        assert!(v.press(tx, ty, now), "grab the thumb at the top");
        assert!(v.dragging());
        assert_eq!(v.scroll(), 0.0, "grabbing doesn't move it");
        assert!(v.drag_to(1000.0, now));
        assert!(
            (v.scroll() - v.max_scroll()).abs() < 1e-3,
            "dragged to the end"
        );
        assert!(v.drag_to(-1000.0, now));
        assert_eq!(v.scroll(), 0.0);
        assert!(v.release(now));
        assert!(!v.dragging() && !v.drag_to(300.0, now));
        // A press on the shown track centers the thumb there.
        v.scroll_by(1.0, now);
        assert!(v.press(tx, 200.0, now));
        let mid = v.scroll() / v.max_scroll();
        assert!((mid - 0.5).abs() < 0.02, "{mid}");
        v.release(now);
        // Nothing to drag when everything fits.
        let mut short = view("short", 600, 400);
        assert!(!short.press(595.0, 5.0, now));
    }

    /// A hidden scrollbar takes presses on its thumb only: a link under the rest of its column
    /// (P8) stays clickable.
    #[test]
    fn hidden_scrollbar_only_takes_the_thumb() {
        let t0 = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        let thumb = v.thumb_at(0.0).unwrap();
        let x = f64::from(thumb.x1 - 1);
        assert_eq!(v.deadline(), None, "hidden");
        assert!(!v.press(x, 300.0, t0), "track, hidden");
        assert!(!v.dragging());
        assert!(v.press(x, f64::from(thumb.y0 + 1), t0), "thumb, hidden");
        assert!(v.dragging());
        v.release(t0);
        // Shown, then timed out again.
        v.scroll_by(10.0, t0);
        let late = v.deadline().unwrap();
        assert!(
            v.press(x, 300.0, late - Duration::from_millis(1)),
            "track, shown"
        );
        v.release(t0);
        let late = late.max(v.deadline().unwrap());
        assert!(v.expire(late));
        // (The thumb moved to the middle with that press.)
        assert!(!v.press(x, 20.0, late), "track, hidden again");
    }

    #[test]
    fn top_inset_shifts_the_document_area() {
        let t0 = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        let total = v.layout().total_height();
        let flat_max = v.max_scroll();
        v.set_top_inset(40);
        assert_eq!(v.top_inset(), 40);
        assert!((v.max_scroll() - (flat_max + 40.0)).abs() < 1e-3);
        assert!(f64::from(total) - v.max_scroll() - 360.0 < 1.0);
        // Pages fit the area, not the window.
        v.apply(Action::Pages(1), t0);
        assert!((v.scroll() - (360.0 - 24.0)).abs() < 1e-3, "{}", v.scroll());
        // Nothing is drawn in the inset but the background; the scrollbar starts below it.
        let mut buf = vec![0x123456; 600 * 400];
        v.paint(&mut buf, 600, t0);
        assert!(
            buf[..40 * 600].iter().all(|&p| p == LIGHT.bg),
            "inset is bare"
        );
        assert!(buf[40 * 600..].iter().any(|&p| p != LIGHT.bg));
        let thumb = v.thumb_at(v.scroll()).unwrap();
        assert!(thumb.y0 >= 40, "{thumb:?}");
        // A press in the inset isn't the scrollbar's; the thumb drags the whole range.
        let x = f64::from(thumb.x1 - 1);
        assert!(!v.press(x, 10.0, t0));
        v.apply(Action::Top, t0);
        let thumb = v.thumb_at(0.0).unwrap();
        assert!(v.press(x, f64::from(thumb.y0 + 1), t0));
        assert!(v.drag_to(1000.0, t0));
        assert!((v.scroll() - v.max_scroll()).abs() < 1e-3);
        v.release(t0);
        // End lands on the true end of the shorter area.
        v.apply(Action::Top, t0);
        v.apply(Action::Bottom, t0);
        v.ensure();
        let n = v.doc().blocks.len();
        assert_eq!(
            v.layout().range_exact(v.scroll(), v.scroll() + 360.0).end,
            n
        );
        // Removing it restores everything; one taller than the window is clamped.
        v.set_top_inset(0);
        assert!((v.max_scroll() - flat_max).abs() < 1e-3);
        v.set_top_inset(9999);
        assert_eq!(v.top_inset(), 400);
    }

    #[test]
    fn scrollbar_shows_then_hides_once() {
        let t0 = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        assert_eq!(v.deadline(), None);
        v.apply(Action::Lines(1), t0);
        let deadline = v.deadline().unwrap();
        let mut buf = vec![0; 600 * 400];
        v.paint(&mut buf, 600, t0);
        let thumb_px = |buf: &[u32]| (0..400).any(|y| buf[y * 600 + 595] != LIGHT.bg);
        assert!(thumb_px(&buf), "visible right after scrolling");
        assert!(!v.expire(deadline - Duration::from_millis(1)));
        assert!(v.expire(deadline));
        assert_eq!(v.deadline(), None);
        v.paint(&mut buf, 600, deadline);
        assert!(!thumb_px(&buf), "hidden after the deadline");
        // While dragging it stays and there's no timer.
        v.press(595.0, 5.0, deadline);
        assert_eq!(v.deadline(), None);
        assert!(!v.expire(deadline + Duration::from_secs(10)));
    }

    /// A one-pixel scroll moves exactly one pixel far down a tall document (f64 anchors).
    #[test]
    fn pixel_scroll_deep_in_a_tall_document() {
        let now = Instant::now();
        let mut v = view(&"Line of text.\n\n".repeat(400_000), 500, 300);
        let deep = v.layout().top(399_000) + 0.25;
        v.set_scroll(deep);
        v.ensure();
        let before = v.scroll();
        assert!(before > 1.5e7, "{before}");
        v.scroll_by(1.0, now);
        assert!(
            (v.scroll() - before - 1.0).abs() < 1e-3,
            "{}",
            v.scroll() - before
        );
    }

    #[test]
    fn missing_characters_are_reported_once() {
        let mut v = view("Hangul 한국어 and more 한", 600, 300);
        let missing = v.take_missing();
        assert!(missing.contains(&'한'), "{missing:?}");
        assert!(v.take_missing().is_empty());
        assert!(!v.add_fallbacks(&[]));
    }

    /// The scrollbar hides on time even while idle layout keeps the loop polling.
    #[test]
    fn tick_hides_the_scrollbar_while_measuring() {
        let t0 = Instant::now();
        let mut v = view(&paras(5000), 600, 400);
        assert!(!v.layout().complete());
        let tick = v.tick(t0, None);
        assert_eq!(
            tick,
            Tick {
                redraw: false,
                wake: Wake::Event
            },
            "nothing to do before the first frame"
        );
        v.apply(Action::Lines(1), t0);
        let deadline = v.deadline().unwrap();
        // A short slice: far from finished, so the loop polls, and the bar stays until due.
        let slice = |t: Instant| Some(t + Duration::from_micros(200));
        let tick = v.tick(deadline - Duration::from_millis(1), slice(t0));
        assert_eq!(
            tick,
            Tick {
                redraw: false,
                wake: Wake::Poll
            }
        );
        let tick = v.tick(deadline, slice(t0));
        assert_eq!(
            tick,
            Tick {
                redraw: true,
                wake: Wake::Poll
            },
            "due while still measuring"
        );
        assert_eq!(v.deadline(), None);
        // Once measured, with a deadline pending, it sleeps until then; with none, forever.
        v.apply(Action::Lines(1), deadline);
        let tick = v.tick(deadline, Some(Instant::now() + Duration::from_secs(60)));
        assert!(v.layout().complete());
        assert_eq!(
            tick,
            Tick {
                redraw: false,
                wake: Wake::At(v.deadline().unwrap())
            }
        );
        let later = v.deadline().unwrap();
        let tick = v.tick(later, Some(Instant::now() + Duration::from_secs(60)));
        assert_eq!(
            tick,
            Tick {
                redraw: true,
                wake: Wake::Event
            }
        );
    }

    #[test]
    fn missing_characters_are_scanned_once_across_reloads() {
        let src = "Hangul 한국어 and more 한";
        let mut v = view(src, 600, 300);
        let first = v.take_missing();
        assert!(first.contains(&'한'), "{first:?}");
        // The same text again, as a reload does: nothing new to look for.
        v.replace(Doc::parse(src), None);
        v.ensure();
        assert!(v.take_missing().is_empty(), "same characters, same scan");
        v.open(Doc::parse(src), None);
        v.ensure();
        assert!(v.take_missing().is_empty(), "also for another document");
        // A character that appears later is new.
        v.replace(Doc::parse(&format!("{src} \u{5b57}\u{4f53}")), None);
        v.ensure();
        let new = v.take_missing();
        assert!(!new.contains(&'한') && !new.is_empty(), "{new:?}");
        assert!(new.iter().all(|c| !first.contains(c)));
    }

    /// Reload with identical text, with the position deep in a long section of blocks of very
    /// different heights: the same anchor (not a drift of several blocks, as pixels gave).
    #[test]
    fn reload_unchanged_keeps_the_anchor_deep_in_a_section() {
        let src = format!(
            "# Title\n\nintro\n\n## Long\n\n{}## After\n\nend\n",
            mixed(80)
        );
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let long = v
            .doc()
            .headings
            .iter()
            .find(|h| h.slug == "long")
            .unwrap()
            .block;
        for (offset, dy) in [(1, 0.0), (50, 7.0), (51, 40.0), (60, 3.0)] {
            v.set_scroll(v.layout().top((long + offset) as usize) + dy);
            let before = v.anchor();
            assert_eq!(before.block, long + offset);
            v.replace(Doc::parse(&src), None);
            assert_eq!(v.anchor(), before, "offset {offset}");
            v.ensure();
            assert_eq!(v.anchor(), before, "offset {offset}, after measuring");
            // And it stays put through further reloads.
            v.replace(Doc::parse(&src), None);
            assert_eq!(v.anchor(), before);
        }
    }

    #[test]
    fn reload_unchanged_at_the_end_and_on_the_heading() {
        let src = format!("# Title\n\n{}", mixed(60));
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        v.apply(Action::Bottom, Instant::now());
        let before = v.anchor();
        v.replace(Doc::parse(&src), None);
        assert_eq!(v.anchor(), before, "bottom of the document");
        v.set_scroll(0.0);
        v.replace(Doc::parse(&src), None);
        assert_eq!(v.anchor(), Anchor::TOP, "at the heading itself");
    }

    /// The same text stays at the top when the section above the heading changes, even when
    /// the position is far below the heading.
    #[test]
    fn reload_edit_above_keeps_the_text() {
        let body = mixed(60);
        let src = format!("# Intro\n\nshort\n\n## Target\n\n{body}");
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let target = v.doc().headings[1].block;
        v.set_scroll(v.layout().top((target + 40) as usize) + 11.0);
        let before = v.anchor();
        let edited = format!("# Intro\n\nshort\n\nmore\n\nand more\n\n## Target\n\n{body}");
        v.replace(Doc::parse(&edited), None);
        assert_eq!(
            v.anchor(),
            Anchor {
                block: before.block + 2,
                frac: before.frac
            }
        );
    }

    /// Edits inside the section, or one that cuts it short, give a position inside the
    /// section and no panic.
    #[test]
    fn reload_edit_inside_the_section() {
        let body = mixed(60);
        let src = format!("# Intro\n\n## Target\n\n{body}## Next\n\ntail\n");
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let target = v.doc().headings[1].block;
        v.set_scroll(v.layout().top((target + 30) as usize));
        // A paragraph added before the position.
        let longer = format!("# Intro\n\n## Target\n\nnew\n\n{body}## Next\n\ntail\n");
        v.replace(Doc::parse(&longer), None);
        let a = v.anchor();
        assert!(a.block >= target + 29 && a.block <= target + 32, "{a:?}");
        // The section shrinks below the position: the last block of it.
        let shorter = format!("# Intro\n\n## Target\n\n{}## Next\n\ntail\n", mixed(10));
        v.replace(Doc::parse(&shorter), None);
        let next = v
            .doc()
            .headings
            .iter()
            .find(|h| h.slug == "next")
            .unwrap()
            .block;
        assert!(
            v.anchor().block > target && v.anchor().block < next,
            "{:?}",
            v.anchor()
        );
        v.ensure();
        assert!(v.scroll() <= v.max_scroll() + 0.5);
    }

    #[test]
    fn reload_with_the_heading_removed_keeps_the_block() {
        let src = format!("# One\n\n{}## Two\n\n{}", mixed(30), mixed(30));
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let two = v.doc().headings[1].block;
        v.set_scroll(v.layout().top((two + 10) as usize));
        let before = v.anchor();
        let edited = src.replace("## Two", "Two");
        v.replace(Doc::parse(&edited), None);
        // "two" is gone from the new text, so the position keeps its block and fraction.
        assert_eq!(v.doc().headings.len(), 1);
        assert_eq!(v.anchor(), before);
        // Not a panic: the heading (and everything) gone.
        v.replace(Doc::parse("tiny"), None);
        assert_eq!(v.anchor().block, 0);
    }

    /// A changelog of versions `top` down to 1, each with the same two headings.
    fn changelog(top: u32) -> String {
        let mut s = String::from("# Changelog\n\n");
        for n in (1..=top).rev() {
            s += &format!(
                "## {n}.0.0\n\n### Added\n\nAdded thing {n}.1\n\nAdded thing {n}.2\n\n\
                 ### Fixed\n\nFixed thing {n}.1\n\n"
            );
        }
        s
    }

    fn block_text(v: &View, block: u32) -> String {
        let doc = v.doc();
        let spans = doc.block_spans(&doc.blocks[block as usize]);
        spans.iter().map(|s| doc.span_text(s)).collect()
    }

    /// Scroll to the block with exactly `text`, `dy` pixels into it.
    fn read_at(v: &mut View, text: &str, dy: f64) -> Anchor {
        let block = (0..v.doc().blocks.len() as u32)
            .find(|&b| block_text(v, b) == text)
            .unwrap();
        v.set_scroll(v.layout().top(block as usize) + dy);
        assert_eq!(v.anchor().block, block);
        v.anchor()
    }

    /// Repeated headings get numbered slugs, so a heading added above shifts every number below
    /// it: the position follows the headings' texts instead.
    #[test]
    fn reload_with_a_repeated_heading_added_above() {
        let src = changelog(8);
        let mut v = view(&src, 600, 100);
        settle(&mut v);
        for text in ["Added thing 5.1", "Added thing 5.2", "Fixed thing 5.1"] {
            let before = read_at(&mut v, text, 3.0);
            v.replace(Doc::parse(&changelog(9)), None);
            assert_eq!(block_text(&v, v.anchor().block), text);
            assert_eq!(v.anchor().frac, before.frac);
            assert!(v.anchor().block > before.block);
            // The same text again: the same anchor, and still through the next reload.
            v.replace(Doc::parse(&changelog(9)), None);
            assert_eq!(block_text(&v, v.anchor().block), text);
            v.replace(Doc::parse(&src), None);
            assert_eq!(v.anchor(), before, "back to the original");
        }
    }

    /// Headings with the same path (same text, same parents): by ordinal while there are as
    /// many as before, else the one nearest the old block.
    #[test]
    fn reload_among_identical_headings() {
        let sec = |n: u32| format!("### Example\n\nbody {n}\n\nmore {n}\n\n");
        let end = format!("# End\n\n{}", paras(30));
        let src = format!("# A\n\n{}{}{}{end}", sec(1), sec(2), sec(3));
        let mut v = view(&src, 600, 100);
        settle(&mut v);
        let before = read_at(&mut v, "more 2", 2.0);
        v.replace(Doc::parse(&src), None);
        assert_eq!(v.anchor(), before, "identical");
        // A paragraph added to the first: same count, so the second still.
        let edited = src.replace("body 1", "body 1\n\nextra");
        v.replace(Doc::parse(&edited), None);
        assert_eq!(block_text(&v, v.anchor().block), "more 2");
        // One more at the end: different count, so the nearest to where it was.
        let four = format!("# A\n\n{}{}{}{}{end}", sec(1), sec(2), sec(3), sec(4));
        v.replace(Doc::parse(&four), None);
        assert_eq!(block_text(&v, v.anchor().block), "more 2");
        read_at(&mut v, "more 4", 0.0);
        v.replace(Doc::parse(&src), None);
        assert_eq!(
            block_text(&v, v.anchor().block),
            "more 3",
            "the fourth is gone"
        );
    }

    /// The path gone from the new text: the slug if it is still there, else the block.
    #[test]
    fn reload_falls_back_from_the_path_to_the_slug_to_the_block() {
        let src = changelog(8);
        let mut v = view(&src, 600, 100);
        settle(&mut v);
        let intro = |s: &str| s.replace("# Changelog\n\n", "# Changelog\n\nIntro.\n\n");
        // A parent renamed (and a paragraph added above): the slug still finds the heading.
        let before = read_at(&mut v, "Added thing 5.1", 3.0);
        let edited = intro(&src.replace("## 5.0.0", "## 5.0.0 (yanked)"));
        v.replace(Doc::parse(&edited), None);
        assert_eq!(block_text(&v, v.anchor().block), "Added thing 5.1");
        assert_eq!(v.anchor().block, before.block + 1, "an extra block above");
        // The last "Added" renamed: no path, and its slug (added-7) is gone too, so the block.
        v.replace(Doc::parse(&src), None);
        let before = read_at(&mut v, "Added thing 1.2", 5.0);
        let edited = intro(&src.replace("## 1.0.0\n\n### Added", "## 1.0.0\n\n### Additions"));
        v.replace(Doc::parse(&edited), None);
        assert_eq!(v.anchor(), before, "the same block index, and fraction");
    }

    /// A reload that fails (the file briefly missing) doesn't lose the position.
    #[test]
    fn reload_keeps_the_position_through_errors() {
        let body = mixed(40);
        let src = format!("# Intro\n\nshort\n\n## Target\n\n{body}");
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let target = v.doc().headings[1].block;
        v.set_scroll(v.layout().top((target + 20) as usize) + 5.0);
        let before = v.anchor();
        let gone = || (Doc::default(), Some("Cannot open x".to_string()));
        let (doc, msg) = gone();
        v.replace(doc, msg);
        assert_eq!(v.anchor(), Anchor::TOP);
        v.replace(Doc::parse(&src), None);
        assert_eq!(v.message(), None);
        assert_eq!(v.anchor(), before, "error, then the same file");
        // Twice in a row, and the file back with an edit above: it follows the heading.
        for _ in 0..2 {
            let (doc, msg) = gone();
            v.replace(doc, msg);
            assert_eq!(v.anchor(), Anchor::TOP);
        }
        let edited = format!("# Intro\n\nshort\n\nmore\n\n## Target\n\n{body}");
        v.replace(Doc::parse(&edited), None);
        assert_eq!(
            v.anchor(),
            Anchor {
                block: before.block + 1,
                frac: before.frac
            }
        );
        // Without headings: the block.
        let plain = paras(100);
        v.open(Doc::parse(&plain), None);
        v.set_scroll(v.layout().top(60));
        let (doc, msg) = gone();
        v.replace(doc, msg);
        v.replace(Doc::parse(&plain), None);
        assert_eq!(v.anchor().block, 60);
    }

    /// A different document starts at the top whatever was kept, and a successful reload
    /// forgets it.
    #[test]
    fn open_after_an_error_starts_at_the_top() {
        let src = format!("# One\n\n{}", mixed(30));
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        v.set_scroll(v.layout().top(20));
        v.replace(Doc::default(), Some("Cannot open x".into()));
        v.open(Doc::parse(&src), None);
        assert_eq!(v.anchor(), Anchor::TOP);
        // The kept position doesn't come back after a document was opened in between.
        v.replace(Doc::default(), Some("Cannot open x".into()));
        v.open(Doc::default(), Some("Cannot open y".into()));
        v.replace(Doc::parse(&src), None);
        assert_eq!(v.anchor(), Anchor::TOP);
        // And a reload that worked leaves nothing behind for the next error to restore.
        v.set_scroll(v.layout().top(10));
        let at = v.anchor();
        v.replace(Doc::parse(&src), None);
        v.replace(Doc::default(), Some("gone".into()));
        v.open(Doc::parse(&src), None);
        v.set_scroll(v.layout().top(5));
        v.replace(Doc::parse(&src), None);
        assert_ne!(v.anchor(), at);
        assert_eq!(v.anchor().block, 5);
    }

    /// What is left of a zoom step when Ctrl is released doesn't carry into the next gesture.
    #[test]
    fn zoom_wheel_leftovers_are_dropped_with_the_modifier() {
        let now = Instant::now();
        let mut v = view(&paras(10), 600, 400);
        v.zoom_wheel(ZOOM_PIXELS * 0.6, now);
        assert_eq!(v.zoom(), 100);
        v.zoom_wheel(ZOOM_PIXELS * 0.6, now);
        assert_eq!(v.zoom(), 110, "one step from 1.2, with 0.2 left over");
        v.zoom_wheel_end();
        v.zoom_wheel(ZOOM_PIXELS * 0.9, now);
        assert_eq!(v.zoom(), 110, "the 0.2 is gone");
        v.zoom_wheel(-ZOOM_PIXELS * 2.5, now);
        assert_eq!(v.zoom(), 100, "a step down from -1.6, with -0.6 left over");
        v.zoom_wheel(-ZOOM_PIXELS * 0.5, now);
        assert_eq!(v.zoom(), 90);
    }

    /// A scrollbar on screen follows the total height as idle layout measures; a hidden one
    /// costs no repaint.
    #[test]
    fn tick_repaints_a_visible_thumb_that_measuring_moved() {
        let t0 = Instant::now();
        let slice = || Some(Instant::now() + Duration::from_micros(300));
        let mut v = view(&mixed(1200), 600, 400);
        // Deep in the document, where the thumb's position follows the total.
        v.set_scroll(v.layout().top(600));
        v.apply(Action::Lines(1), t0);
        let mut moved = 0;
        while !v.layout().complete() {
            let before = v.thumb_at(v.scroll());
            let tick = v.tick(t0, slice());
            let after = v.thumb_at(v.scroll());
            assert_eq!(tick.redraw, before != after);
            moved += usize::from(before != after);
        }
        assert!(moved > 0, "the estimates were off, so the thumb moved");
        // Hidden: measuring repaints nothing.
        let mut v = view(&mixed(1200), 600, 400);
        // Deep in the document, where the thumb's position follows the total.
        v.set_scroll(v.layout().top(600));
        v.apply(Action::Lines(1), t0);
        let due = v.deadline().unwrap();
        assert!(v.tick(due, slice()).redraw, "hides");
        let mut moved = 0;
        while !v.layout().complete() {
            let before = v.thumb_at(v.scroll());
            assert!(!v.tick(due, slice()).redraw);
            moved += usize::from(before != v.thumb_at(v.scroll()));
        }
        assert!(moved > 0, "it did move, unseen");
        // Dragging keeps it showing.
        let mut v = view(&mixed(1200), 600, 400);
        // Deep in the document, where the thumb's position follows the total.
        v.set_scroll(v.layout().top(600));
        let t = v.thumb_at(v.scroll()).unwrap();
        assert!(v.press(f64::from(t.x0), f64::from(t.y0 + 1), t0));
        let due = t0 + Duration::from_secs(60);
        let mut moved = 0;
        while !v.layout().complete() {
            let before = v.thumb_at(v.scroll());
            let tick = v.tick(due, slice());
            assert_eq!(tick.redraw, before != v.thumb_at(v.scroll()));
            moved += usize::from(tick.redraw);
        }
        assert!(moved > 0);
    }

    #[test]
    fn text_size_and_family_keep_the_reading_position() {
        let mut v = view(&paras(300), 700, 500);
        settle(&mut v);
        v.set_scroll(v.layout().top(150) + 3.0);
        let block = v.anchor().block;
        v.set_text(20.0, 60.0);
        assert_eq!(v.layout().params().font_size, 20.0);
        assert_eq!(v.layout().params().line_width, 60.0);
        assert_eq!(v.anchor().block, block);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
        let jb = [
            Some((dir.join("JetBrainsMono-Regular.ttf"), 0)),
            Some((dir.join("JetBrainsMono-Bold.ttf"), 0)),
            None,
            None,
        ];
        let before: Vec<_> = v.layout().geoms()[block as usize]
            .content
            .as_ref()
            .unwrap()
            .glyphs
            .iter()
            .map(|g| g.face)
            .collect();
        assert!(v.set_family(false, Some(&jb)));
        assert_eq!(v.anchor().block, block);
        let after: Vec<_> = v.layout().geoms()[block as usize]
            .content
            .as_ref()
            .expect("remeasured around the anchor")
            .glyphs
            .iter()
            .map(|g| g.face)
            .collect();
        assert_ne!(before, after, "set in the new family");
        assert!(v.set_family(false, None));
        assert_eq!(v.anchor().block, block);
    }

    fn jb_family() -> [Option<(PathBuf, u32)>; 4] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
        [
            Some((dir.join("JetBrainsMono-Regular.ttf"), 0)),
            Some((dir.join("JetBrainsMono-Bold.ttf"), 0)),
            None,
            None,
        ]
    }

    /// Box drawing: JetBrains Mono has it, the default Inter doesn't.
    const BOX: char = '\u{2500}';

    fn covered(v: &View, ch: char) -> bool {
        v.fonts
            .select(v.fonts.primary(false, false, false), ch)
            .is_some()
    }

    /// Characters a family covered, and no other face did, stay covered when the family is
    /// left, and are never reported as missing for a scan that couldn't find them.
    #[test]
    fn switching_away_from_a_family_keeps_its_characters() {
        let mut v = view("Lines ─── and more", 600, 300);
        let missing = v.take_missing();
        assert!(missing.contains(&BOX), "{missing:?}");
        // The scan found nothing (no such font installed); the family has it.
        assert!(v.set_family(false, Some(&jb_family())));
        settle(&mut v);
        assert!(covered(&v, BOX));
        assert!(v.set_family(false, None));
        settle(&mut v);
        assert!(covered(&v, BOX), "still covered through the old family");
        assert!(v.take_missing().is_empty(), "nothing for a scan to do");
        // A character the family covered before it was ever missing, the same.
        let mut v = view("Plain", 600, 300);
        assert!(v.set_family(false, Some(&jb_family())));
        v.replace(Doc::parse("Lines ─── and more"), None);
        settle(&mut v);
        assert!(v.take_missing().is_empty());
        assert!(v.set_family(false, None));
        settle(&mut v);
        assert!(covered(&v, BOX));
        assert!(v.take_missing().is_empty());
    }

    /// Startup with a cached family: the zoom and text settings are applied first, with the
    /// defaults; what that queued and the family covers must not trigger a scan.
    #[test]
    fn a_cached_family_applied_after_zoom_needs_no_scan_for_what_it_covers() {
        let mut v = view("Lines \u{2500}\u{2500} and \u{E000}", 600, 300);
        v.set_zoom(125);
        v.set_text(17.0, 90.0);
        assert!(v.set_family(false, Some(&jb_family())));
        settle(&mut v);
        assert!(covered(&v, BOX));
        assert_eq!(v.take_missing(), vec!['\u{E000}']);
    }

    /// A face loaded for a family in use is not a fallback, so a scan may find it for the text
    /// in the other kind of slot.
    #[test]
    fn a_scan_can_pick_a_loaded_family_face() {
        use crate::text::fallback::find_in;
        let mut v = view("Lines ─── and more\n\n`code ───`", 600, 300);
        // Code in JetBrains Mono; body text stays in Inter, and lacks the box drawing.
        assert!(v.set_family(true, Some(&jb_family())));
        settle(&mut v);
        assert!(!covered(&v, BOX));
        assert_eq!(v.take_missing(), vec![BOX]);
        let sources = v.font_sources();
        let files = [jb_family()[0].clone().unwrap().0];
        let skip = |p: &std::path::Path, i: u32| sources.iter().any(|(sp, si)| sp == p && *si == i);
        let found = find_in(&files, &[BOX], &skip);
        assert_eq!(found.len(), 1, "the loaded face is still a candidate");
        assert!(v.add_fallbacks(&found));
        settle(&mut v);
        assert!(covered(&v, BOX));
        assert_eq!(v.font_sources().len(), 1);
        assert!(!v.add_fallbacks(&found), "once");
    }

    /// A window point over the first glyph of `needle` in block `block` (laid out).
    fn point_at(v: &View, block: usize, needle: &str) -> (f64, f64) {
        let doc = v.doc();
        let b = &doc.blocks[block];
        let span = doc
            .block_spans(b)
            .iter()
            .find(|s| doc.span_text(s).contains(needle))
            .expect("needle in block");
        let offset = span.start + doc.span_text(span).find(needle).unwrap() as u32;
        let content = v.layout().geoms()[block]
            .content
            .as_ref()
            .expect("laid out");
        let line = content
            .lines
            .iter()
            .find(|l| {
                content.hits[l.hits.start as usize..l.hits.end as usize]
                    .iter()
                    .any(|h| h.doc >= offset)
            })
            .unwrap();
        let hit = content.hits[line.hits.start as usize..line.hits.end as usize]
            .iter()
            .find(|h| h.doc >= offset)
            .unwrap();
        let (col_x, _) = v.layout().column();
        let x = f64::from(col_x + (hit.x0 + hit.x1) / 2.0);
        let y = v.layout().top(block) - v.scroll() + f64::from((line.top + line.bottom) / 2.0);
        (x, y)
    }

    #[test]
    fn links_hover_and_click() {
        let now = Instant::now();
        let mut v = view(
            "Intro with [a link](https://example.com/x) and plain words here.\n\n# Target\n",
            600,
            400,
        );
        settle(&mut v);
        let (lx, ly) = point_at(&v, 0, "a link");
        let (px, py) = point_at(&v, 0, "plain");
        assert_eq!(
            v.doc().links[v.link_at(lx, ly).unwrap() as usize - 1],
            "https://example.com/x"
        );
        assert_eq!(v.link_at(px, py), None);
        assert!(v.hover(lx, ly), "entering a link changes the hover");
        assert_eq!(v.hovered(), Some("https://example.com/x"));
        assert!(!v.hover(lx + 1.0, ly), "same link");
        assert!(v.hover(px, py));
        assert_eq!(v.hovered(), None);
        v.hover(lx, ly);
        assert!(v.hover_end());
        assert_eq!(v.hovered(), None);
        // A click is press and release on the same link.
        assert!(!v.press(lx, ly, now), "the scrollbar doesn't take it");
        assert_eq!(v.click(lx, ly).as_deref(), Some("https://example.com/x"));
        assert_eq!(v.click(lx, ly), None, "one click per press");
        v.press(lx, ly, now);
        assert_eq!(v.click(px, py), None, "released elsewhere");
        v.press(px, py, now);
        assert_eq!(v.click(lx, ly), None, "pressed elsewhere");
        // The preview is painted at the bottom left while hovering.
        let mut buf = vec![0; 600 * 400];
        v.paint(&mut buf, 600, now);
        let corner =
            |buf: &[u32]| (390..396).any(|y| (6..40).any(|x| buf[y * 600 + x] != LIGHT.bg));
        assert!(!corner(&buf));
        v.hover(lx, ly);
        v.paint(&mut buf, 600, now);
        assert!(corner(&buf), "preview box drawn");
        // A new document forgets the link under the pointer, and the hand cursor shown over it
        // has to go although the hover reports no change any more.
        let pointer = v.hovered().is_some();
        assert!(pointer);
        v.replace(Doc::parse("Other."), None);
        assert_eq!(v.hovered(), None);
        let changed = v.hover_end();
        let update = hover_change(pointer, v.hovered().is_some(), changed);
        assert!(!changed && update.set_cursor && !update.pointer);
        v.hover(lx, ly);
        v.open(Doc::parse("Other."), None);
        let update = hover_change(true, v.hovered().is_some(), v.hover(lx, ly));
        assert!(update.set_cursor && !update.pointer, "{update:?}");
    }

    /// A press on a link that wraps on below the window, dragged out and released there (to
    /// cancel) must not follow it: cached content extends past the viewport, but the pointer
    /// outside the window is over nothing.
    #[test]
    fn pointer_outside_the_window_is_over_no_link() {
        let now = Instant::now();
        let words: String = (0..80).map(|i| format!("word{i} ")).collect();
        let src = format!("Start [{words}zzend](https://example.com/x) done.\n");
        let mut v = view(&src, 600, 120);
        settle(&mut v);
        let (lx, ly) = point_at(&v, 0, "word0");
        let (ex, ey) = point_at(&v, 0, "zzend");
        assert!(ey > 120.0, "the link goes on below the window: {ey}");
        assert!(v.link_at(lx, ly).is_some());
        assert!(v.link_at(ex, ey).is_none(), "below the window");
        // Left and right of the window on the first line, above the document, NaN.
        let outside = [
            (lx, 125.0),
            (lx, 1000.0),
            (610.0, ly),
            (-5.0, ly),
            (lx, -5.0),
            (f64::NAN, ly),
            (lx, f64::NAN),
        ];
        for (x, y) in outside {
            assert_eq!(v.link_at(x, y), None, "({x}, {y})");
            assert!(v.hover(lx, ly) || v.hovered().is_some());
            v.hover(x, y);
            assert_eq!(v.hovered(), None, "hover at ({x}, {y})");
            v.press(lx, ly, now);
            assert_eq!(v.click(x, y), None, "released at ({x}, {y})");
        }
        // Inside still works.
        assert!(v.hover(lx, ly));
        assert_eq!(v.hovered(), Some("https://example.com/x"));
        v.press(lx, ly, now);
        assert_eq!(v.click(lx, ly).as_deref(), Some("https://example.com/x"));
        v.set_top_inset(20);
        assert_eq!(v.link_at(lx, 5.0), None, "inside the inset");
    }

    /// Scrolling away evicts the blocks at the top, and scrolling back leaves them unbuilt
    /// until the next paint: the hover refresh that follows an input event (and comes first)
    /// must still find the link under a pointer that hasn't moved.
    #[test]
    fn hover_refresh_finds_links_in_blocks_not_yet_rebuilt() {
        let now = Instant::now();
        let src = format!(
            "Intro with [a link](https://example.com/x) here.\n\n{}",
            paras(300)
        );
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let (lx, ly) = point_at(&v, 0, "a link");
        assert!(v.hover(lx, ly));
        let mut buf = vec![0; 600 * 400];
        v.scroll_by(1.0e9, now);
        v.paint(&mut buf, 600, now);
        assert!(
            v.layout().geoms()[0].content.is_none(),
            "the top was evicted"
        );
        assert!(v.hover(lx, ly), "a screen of other text under the pointer");
        assert_eq!(v.hovered(), None);
        // Back at the top, as the End/Home keys do, with no paint in between.
        v.scroll_by(-1.0e9, now);
        assert!(v.layout().geoms()[0].content.is_none(), "not rebuilt yet");
        let changed = v.hover(lx, ly);
        assert!(changed, "hover finds the link again without a paint");
        assert_eq!(v.hovered(), Some("https://example.com/x"));
        // A click that comes before the repaint hits it too.
        v.scroll_by(1.0e9, now);
        v.paint(&mut buf, 600, now);
        v.scroll_by(-1.0e9, now);
        assert!(!v.press(lx, ly, now));
        assert_eq!(v.click(lx, ly).as_deref(), Some("https://example.com/x"));
    }

    /// A bidi override in a URL would show `…/report\u{202E}fdp.exe` as `…/reportexe.pdf`:
    /// the preview spells such characters out.
    #[test]
    fn preview_spells_out_invisible_characters() {
        let mut v = view("x", 600, 400);
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: 12.0,
            role: Role::Muted,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let (para, _) = v.fit_preview(
            "https://example.invalid/report\u{202E}fdp.exe",
            style,
            1.0e4,
        );
        assert_eq!(para.text, "https://example.invalid/report\\u{202E}fdp.exe");
        let hidden =
            "a\u{200B}b\u{200E}c\u{2066}d\u{2069}e\u{FEFF}f\u{061C}g\u{202A}h\u{00AD}i\u{2060}j";
        let (para, _) = v.fit_preview(hidden, style, 1.0e4);
        assert_eq!(
            para.text,
            "a\\u{200B}b\\u{200E}c\\u{2066}d\\u{2069}e\\u{FEFF}f\\u{061C}g\\u{202A}h\\u{00AD}i\\u{2060}j"
        );
        assert!(!para.text.chars().any(crate::nav::is_invisible));
        // A cut never falls inside an escape: the text before `…` is a whole number of
        // characters of the shown URL.
        let url = format!("https://a.b/{}", "\u{202E}".repeat(40));
        for max_w in [40.0, 90.0, 150.0, 220.0] {
            let (para, pl) = v.fit_preview(&url, style, max_w);
            assert!(pl.width <= max_w);
            let shown = para.text.trim_end_matches('…');
            let tail = shown.strip_prefix("https://a.b/").unwrap_or("");
            assert!(tail.split("\\u{202E}").all(str::is_empty), "{shown}");
            assert!(shown.matches('\\').count() == tail.matches("\\u{202E}").count());
        }
        // Right-to-left text in a host or path stays: it is not an invisible character, and
        // the URL is laid out left to right even when it starts with it.
        for url in ["https://example.com/مثال/اختبار", "מבחן.example/x"] {
            let (para, _) = v.fit_preview(url, style, 1.0e4);
            assert!(para.text.ends_with(url), "{}", para.text);
            assert_eq!(v.msg_ctx.shape(&v.fonts, &para).base_level, 0, "{url}");
        }
    }

    /// The preview box is never wider than allowed and a cut URL always ends with `…` inside
    /// it, whatever the characters measure (CJK, wide hosts).
    #[test]
    fn preview_ellipsis_stays_inside_the_box() {
        let mut v = view("x", 600, 400);
        let style = TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: 12.0,
            role: Role::Muted,
            deco: 0,
            shift: 0.0,
            link: 0,
        };
        let long_ascii = format!("https://example.com/{}", "segment/".repeat(60));
        let long_cjk = format!("https://例え.jp/{}", "日本語のページ/".repeat(40));
        let wide = format!("https://example.com/{}", "ＷＩＤＥ".repeat(40));
        for max_w in [60.0, 200.0, 345.0] {
            for url in [&long_ascii, &long_cjk, &wide] {
                let (para, pl) = v.fit_preview(url, style, max_w);
                assert!(pl.width <= max_w, "{} > {max_w}: {}", pl.width, para.text);
                assert!(para.text.ends_with('…'), "{}", para.text);
                assert!(url.starts_with(para.text.trim_end_matches('…')));
                // The longest prefix that fits: one more character doesn't.
                let n = para.text.chars().count() - 1;
                let more: String = url.chars().take(n + 1).chain(['…']).collect();
                let mut p = Para::default();
                p.push_text(&more, 0, style);
                let w = v
                    .msg_ctx
                    .layout(&v.fonts, &p, 1.0e6, Align::Start, 1.4)
                    .width;
                assert!(w > max_w || n + 1 >= url.chars().count(), "{max_w}: {w}");
            }
        }
        // A URL that fits is shown whole.
        let (para, pl) = v.fit_preview("https://a.b/c", style, 345.0);
        assert_eq!(para.text, "https://a.b/c");
        assert!(pl.width <= 345.0);
        // A box too narrow for anything shows just the ellipsis.
        let (para, _) = v.fit_preview(&long_ascii, style, 1.0);
        assert_eq!(para.text, "…");
    }

    #[test]
    fn anchors_and_footnotes_jump() {
        let now = Instant::now();
        let src = format!(
            "# Top\n\nSee [below](#far-section) and a note[^n].\n\n{}## Far Section\n\n{}[^n]: The note.\n",
            paras(80),
            paras(30)
        );
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let far = v
            .doc()
            .headings
            .iter()
            .find(|h| h.slug == "far-section")
            .unwrap()
            .block;
        assert!(v.jump_to_anchor("far-section", now));
        assert_eq!(
            v.anchor(),
            Anchor {
                block: far,
                frac: 0.0
            }
        );
        assert!(
            v.jump_to_anchor("FAR-SECTION", now),
            "case-insensitive fallback"
        );
        assert!(!v.jump_to_anchor("missing", now));
        assert_eq!(v.anchor().block, far, "a missing anchor doesn't move");
        // Footnote reference → note.
        let note = v.doc().anchors.iter().find(|(s, _)| s == "fn-n").unwrap().1;
        assert!(v.jump_to_anchor("fn-n", now));
        // The note is at the end: the view settles at the bottom with the note on screen.
        let shown = v.layout().range_exact(v.scroll(), v.scroll() + 400.0);
        assert!(shown.contains(&(note as usize)), "{shown:?} {note}");
        assert!(v.jump_to_anchor("", now));
        assert_eq!(v.scroll(), 0.0);
        // Back-stack positions are clamped into the document.
        v.set_anchor(Anchor {
            block: 1_000_000,
            frac: 3.0,
        });
        assert!(v.anchor().block < v.doc().blocks.len() as u32);
    }

    #[test]
    fn heading_navigation() {
        let now = Instant::now();
        let src = format!(
            "# One\n\n{}## Two\n\n{}## Three\n\n{}",
            paras(20),
            paras(20),
            paras(20)
        );
        let mut v = view(&src, 600, 400);
        settle(&mut v);
        let blocks: Vec<u32> = v.doc().headings.iter().map(|h| h.block).collect();
        assert_eq!(blocks.len(), 3);
        assert!(v.heading(1, now));
        assert_eq!(v.anchor().block, blocks[1]);
        assert!(v.heading(1, now));
        assert_eq!(v.anchor().block, blocks[2]);
        assert!(!v.heading(1, now), "no heading after the last");
        assert!(v.heading(-1, now));
        assert_eq!(v.anchor().block, blocks[1]);
        // From the middle of a section, back goes to its own heading first.
        v.scroll_by(200.0, now);
        assert!(v.heading(-1, now));
        assert_eq!(v.anchor().block, blocks[1]);
        assert!(v.heading(-1, now));
        assert_eq!(v.anchor().block, blocks[0]);
        assert!(
            !v.heading(-1, now),
            "already at the first heading (at the top)"
        );
        assert_eq!(v.apply(Action::NextHeading, now), Outcome::Redraw);
        assert_eq!(v.anchor().block, blocks[1]);
        assert_eq!(v.apply(Action::Back, now), Outcome::Back);
    }

    #[test]
    fn toolbar_takes_its_strip_and_its_clicks() {
        let now = Instant::now();
        let mut v = view(
            &format!("[top link](https://a.example/)\n\n{}", paras(200)),
            600,
            400,
        );
        settle(&mut v);
        assert_eq!(
            (v.top_inset(), v.toolbar_visible()),
            (0, false),
            "a bare view has no bar"
        );
        v.set_scroll(v.layout().top(100) + 5.0);
        let anchor = v.anchor();
        v.set_toolbar(true);
        assert_eq!(v.top_inset(), 32);
        assert_eq!(
            v.anchor(),
            anchor,
            "showing the bar keeps the reading position"
        );
        assert_eq!(v.apply(Action::ToggleToolbar, now), Outcome::Redraw);
        assert_eq!((v.top_inset(), v.toolbar_visible()), (0, false));
        assert_eq!(v.anchor(), anchor);
        v.apply(Action::ToggleToolbar, now);
        assert_eq!(v.top_inset(), 32);
        // HiDPI scales the bar, not the zoom.
        v.set_hidpi(2.0);
        assert_eq!(v.top_inset(), 64);
        v.set_hidpi(1.0);
        v.apply(Action::ZoomIn, now);
        assert_eq!(v.top_inset(), 32);
        v.apply(Action::ZoomReset, now);
        // A click on the bar is a toolbar command, never a link under it.
        v.apply(Action::Top, now);
        v.set_can_back(true);
        assert!(v.press(20.0, 16.0, now), "Back button");
        assert!(!v.release(now));
        assert!(v.release_toolbar(20.0, 16.0));
        assert_eq!(v.click(20.0, 16.0), None);
        assert_eq!(
            v.take_command(),
            Some(toolbar::Command::Action(Action::Back))
        );
        assert_eq!(v.take_command(), None);
        // Hovering the bar shows no link, and starts the tooltip timer.
        assert!(v.hover_at(20.0, 16.0, now));
        assert_eq!(v.hovered(), None);
        assert_eq!(v.deadline(), Some(now + toolbar::TOOLTIP_DELAY));
        assert!(v.expire(now + toolbar::TOOLTIP_DELAY));
        assert!(v.hover_end());
        // Only the scrollbar's own timer (from the jump to the top) may remain.
        assert!(
            v.deadline()
                .is_none_or(|t| t != now + toolbar::TOOLTIP_DELAY)
        );
    }

    /// The view at 200 px wide (`⋯` holds zoom and theme) with the dropdown open.
    fn open_menu_view(now: Instant) -> View {
        let mut v = view(&paras(20), 200, 300);
        v.set_toolbar(true);
        let (mx, my) = (200.0 - 4.0 - 3.0 * 32.0 + 16.0, 16.0);
        assert!(v.press(mx, my, now));
        v.release_toolbar(mx, my);
        v
    }

    #[test]
    fn escape_closes_the_toolbar_menu_first() {
        let now = Instant::now();
        let mut v = open_menu_view(now);
        assert_eq!(
            v.apply(Action::Escape, now),
            Outcome::Redraw,
            "closes the menu"
        );
        assert_eq!(v.apply(Action::Escape, now), Outcome::Quit);
        // The quit keys always quit, whatever is open.
        let mut v = open_menu_view(now);
        assert_eq!(v.apply(Action::Quit, now), Outcome::Quit);
    }

    /// The wheel and the scrolling keys go to the open dropdown, not the document under it.
    #[test]
    fn open_dropdown_takes_the_scrolling() {
        let now = Instant::now();
        let mut v = open_menu_view(now);
        settle(&mut v);
        v.scroll_by(100.0, now);
        v.apply(Action::Pages(1), now);
        assert_eq!(v.scroll(), 0.0, "the document stays put");
        assert_eq!(v.apply(Action::Escape, now), Outcome::Redraw);
        v.scroll_by(100.0, now);
        assert!(v.scroll() > 0.0, "closed: the document scrolls");
    }

    /// Widening the window until nothing overflows closes the dropdown that went with the
    /// `⋯`: the next press is the document's and Escape quits.
    #[test]
    fn widening_the_window_closes_the_dropdown() {
        let now = Instant::now();
        let mut v = open_menu_view(now);
        v.resize(900, 300);
        assert_eq!(v.apply(Action::Escape, now), Outcome::Quit);
        let mut v = open_menu_view(now);
        v.resize(900, 300);
        // A press below the bar isn't swallowed (here it starts nothing, but isn't taken by
        // the toolbar either: the shell sees `false` and goes on to the document).
        assert!(!v.press(300.0, 200.0, now));
        assert!(!v.release_toolbar(300.0, 200.0));
        // And it doesn't reopen when the window narrows again.
        v.resize(200, 300);
        assert!(!v.press(100.0, 200.0, now));
    }

    #[test]
    fn toolbar_grows_back_with_the_window() {
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        v.set_toolbar(true);
        v.set_scroll(v.layout().top(100) + 5.0);
        let anchor = v.anchor();
        v.resize(600, 20);
        assert_eq!(v.top_inset(), 20, "cut to a short window");
        v.resize(600, 400);
        assert_eq!(v.top_inset(), 32, "the whole bar again");
        v.ensure();
        assert_eq!(v.anchor(), anchor);
    }

    fn message_view(w: u32, h: u32) -> View {
        View::new(
            Fonts::new().unwrap(),
            Doc::default(),
            Some("Drop a Markdown file here".into()),
            (w, h),
            1.0,
        )
    }

    /// The message screen has its toolbar like the document: painted, and its buttons work.
    #[test]
    fn message_screen_paints_the_toolbar_and_takes_its_clicks() {
        let now = Instant::now();
        let mut v = message_view(600, 400);
        v.set_toolbar(true);
        let mut buf = vec![0; 600 * 400];
        v.paint(&mut buf, 600, now);
        assert_eq!(buf[2 * 600 + 300], LIGHT.code_bg, "bar background");
        assert_eq!(buf[31 * 600 + 300], LIGHT.border, "divider");
        assert!(
            buf[..32 * 600]
                .iter()
                .any(|&p| ![LIGHT.code_bg, LIGHT.border].contains(&p)),
            "icons"
        );
        // The message is centered in the area below the bar.
        let ink: Vec<usize> = (33..400)
            .filter(|&y| (0..600).any(|x| buf[y * 600 + x] != LIGHT.bg))
            .collect();
        let (y0, y1) = (*ink.first().unwrap(), *ink.last().unwrap());
        assert!(
            (y0 as i32 + y1 as i32 - (32 + 400)).abs() <= 12,
            "{y0}..{y1}"
        );
        // Reload sits at the right edge (no settings file here).
        let (x, y) = (600.0 - 4.0 - 16.0, 16.0);
        assert!(v.hover_at(x, y, now));
        assert_eq!(v.deadline(), Some(now + toolbar::TOOLTIP_DELAY));
        assert!(v.press(x, y, now));
        assert!(v.release_toolbar(x, y));
        assert_eq!(
            v.take_command(),
            Some(toolbar::Command::Action(Action::Reload))
        );
        // Hidden: nothing is painted or clicked there.
        v.set_toolbar(false);
        v.paint(&mut buf, 600, now);
        assert!(buf[..32 * 600].iter().all(|&p| p == LIGHT.bg));
        assert!(!v.press(x, y, now));
    }

    /// Paint, then let the loop run after the first frame (images are asked for).
    fn frame(v: &mut View, now: Instant) {
        let (w, h) = v.size();
        let mut buf = vec![0u32; (w * h) as usize];
        v.paint(&mut buf, w as usize, now);
        v.tick(now, Some(now));
    }

    /// Poll the image threads until `done` holds (fails after a while).
    #[cfg(feature = "remote-images")]
    fn wait_images(v: &mut View, done: impl Fn(&View) -> bool) {
        let t = Instant::now();
        while !done(v) {
            assert!(t.elapsed() < Duration::from_secs(20), "timed out");
            if v.poll_images(Instant::now()) {
                frame(v, Instant::now());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A local image is decoded after the first frame and painted in its box.
    #[cfg(feature = "images")]
    #[test]
    fn local_images_are_painted() {
        let dir = std::env::temp_dir().join(format!("vomd-view-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Vec::new();
        let mut enc = ::png::Encoder::new(&mut png, 40, 20);
        enc.set_color(::png::ColorType::Rgb);
        enc.write_header()
            .unwrap()
            .write_image_data(&[0x11, 0x99, 0x33].repeat(800))
            .unwrap();
        std::fs::write(dir.join("a.png"), &png).unwrap();
        let mut v = view("![a](a.png)", 600, 400);
        v.set_image_base(Some(dir.clone()));
        let now = Instant::now();
        frame(&mut v, now);
        assert_eq!(v.layout().image_size(1), Some(ImageSize::Pixels(40, 20)));
        let t = Instant::now();
        let mut buf = vec![0u32; 600 * 400];
        loop {
            v.poll_images(Instant::now());
            v.paint(&mut buf, 600, Instant::now());
            if buf.contains(&0x119933) {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "never painted");
            v.tick(Instant::now(), Some(Instant::now()));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(buf.iter().filter(|&&p| p == 0x119933).count(), 40 * 20);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A document's images are attached once when it is opened, base directory included.
    #[cfg(feature = "images")]
    #[test]
    fn opening_a_document_attaches_its_images_once() {
        let mut v = view("![a](a.png)", 600, 400);
        let before = v.images.generation();
        v.open_in(Doc::parse("![b](b.png)"), None, Some(std::env::temp_dir()));
        assert_eq!(v.images.generation(), before + 1);
        v.open(Doc::parse("![c](c.png)"), None);
        assert_eq!(v.images.generation(), before + 2);
    }

    /// A view made with its base directory has attached its images once, base included (the
    /// first document, which `main.rs` hands the base to).
    #[cfg(feature = "images")]
    #[test]
    fn a_new_view_attaches_its_images_once() {
        let dir = std::env::temp_dir();
        let v = View::with_base(
            Fonts::new().unwrap(),
            Doc::parse("![a](a.png)"),
            None,
            (600, 400),
            1.0,
            Some(dir.clone()),
        );
        assert_eq!(v.images.generation(), 1);
        assert_eq!(v.images.base(), Some(dir.as_path()));
        assert_eq!(view("![a](a.png)", 600, 400).images.generation(), 1);
    }

    #[cfg(feature = "remote-images")]
    mod remote {
        use super::*;

        /// Nothing listens on the discard port: a fetch fails at once, without the network.
        const DOC: &str = "Text ![r](http://127.0.0.1:9/a.png) and ![s](HTTPS://127.0.0.1:9/b.png)";

        fn shown(src: &str) -> View {
            let mut v = view(src, 600, 400);
            v.set_toolbar(true);
            v
        }

        #[test]
        fn remote_images_wait_for_the_reader() {
            assert_eq!(
                key_action(&ch("i"), ModifiersState::empty()),
                Some(Action::ToggleRemote)
            );
            let mut v = shown(DOC);
            let now = Instant::now();
            for i in 0..3 {
                frame(&mut v, now + Duration::from_millis(i));
            }
            // Blocked: alt text, nothing fetched, the button counts them and pulses once.
            assert!(!v.images.remote.started());
            assert_eq!(v.layout().image_size(1), Some(ImageSize::Alt));
            assert_eq!(v.toolbar_ctx().remote, 2);
            assert!(!v.toolbar_ctx().remote_on);
            assert_eq!(v.deadline(), Some(now + PULSE));
            assert!(v.tick(now + PULSE, Some(now)).redraw);
            assert!(v.pulse.is_none());
            // Hovering a placeholder says how to load it.
            let hit = (0..600)
                .flat_map(|x| (0..80).map(move |y| (f64::from(x), f64::from(y))))
                .find(|&(x, y)| v.image_at(x, y) == Some(1))
                .expect("a placeholder on screen");
            assert!(v.hover(hit.0, hit.1));
            assert!(v.hover_blocked);
            assert_eq!(v.hovered(), None, "not a link: no hand cursor");
            // `I`: they load (a box while fetching), and fail here, back to alt text.
            assert_eq!(v.apply(Action::ToggleRemote, now), Outcome::Redraw);
            assert!(v.remote_on());
            v.ensure();
            assert_eq!(v.layout().image_size(1), Some(ImageSize::Loading));
            assert_eq!(
                v.layout().image_size(2),
                Some(ImageSize::Loading),
                "any case of https"
            );
            frame(&mut v, now);
            assert!(v.images.remote.started());
            wait_images(&mut v, |v| {
                v.layout().image_size(1) == Some(ImageSize::Alt)
                    && v.layout().image_size(2) == Some(ImageSize::Alt)
            });
            v.hover(hit.0, hit.1);
            assert!(!v.hover_blocked, "on: not blocked");
            // A reload keeps them on; another document turns them off.
            v.replace(Doc::parse(DOC), None);
            assert!(v.remote_on());
            v.open(Doc::parse(DOC), None);
            assert!(!v.remote_on());
            // `I` again hides them.
            v.apply(Action::ToggleRemote, now);
            v.apply(Action::ToggleRemote, now);
            assert!(!v.remote_on());
            v.ensure();
            assert_eq!(v.layout().image_size(1), Some(ImageSize::Alt));
        }

        #[test]
        fn the_policy_decides() {
            // never: no button, no key, no note.
            let mut v = shown(DOC);
            v.set_remote_policy(RemoteImages::Never);
            assert_eq!(v.toolbar_ctx().remote, 0);
            assert_eq!(
                v.apply(Action::ToggleRemote, Instant::now()),
                Outcome::Nothing
            );
            frame(&mut v, Instant::now());
            assert!(v.pulse.is_none() && !v.remote_on() && !v.images.remote.started());
            // always: on after the first frame, never before.
            let mut v = shown(DOC);
            v.set_remote_policy(RemoteImages::Always);
            let mut buf = vec![0u32; 600 * 400];
            v.paint(&mut buf, 600, Instant::now());
            v.tick(Instant::now(), None);
            assert!(!v.remote_on(), "not before the first frame is out");
            v.tick(Instant::now(), Some(Instant::now()));
            assert!(v.remote_on());
            // A document without remote images has no button.
            let v = shown("![local](a.png)");
            assert_eq!(v.toolbar_ctx().remote, 0);
        }

        /// With the toolbar hidden, a toast says what the pulsing button would have.
        #[test]
        fn a_toast_stands_in_for_a_hidden_toolbar() {
            let mut v = view(DOC, 600, 400);
            let now = Instant::now();
            frame(&mut v, now);
            let (until, text) = v.toast.clone().unwrap();
            assert_eq!(until, now + TOAST);
            assert_eq!(text, "2 remote images blocked · press I to load");
            assert!(v.pulse.is_none());
            // Once per document.
            v.tick(now + TOAST, Some(now));
            assert!(v.toast.is_none());
            frame(&mut v, now + TOAST);
            assert!(v.toast.is_none());
        }
    }
}
