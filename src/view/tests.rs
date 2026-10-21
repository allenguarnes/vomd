use super::*;
// Only the tests that need the embedded fonts use it.
#[cfg(all(feature = "images", feature = "embed-fonts"))]
use crate::layout::{ImageSize, Refusal};
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
        #[cfg(feature = "find")]
        (ch("f"), ctrl, Some(Action::Find)),
        #[cfg(feature = "find")]
        (ch("F"), ctrl | shift, Some(Action::Find)),
        (ch("f"), none, None),
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
        (ch("o"), ctrl, Some(Action::Open)),
        (ch("O"), ctrl | shift, Some(Action::Open)),
        (ch("o"), ModifiersState::SUPER, Some(Action::Open)),
        // Plain `o` is no shortcut.
        (ch("o"), none, None),
        // Shortcuts never fall through to plain keys.
        (ch("j"), ctrl, None),
        (ch("q"), ModifiersState::ALT, None),
        (ch("="), none, None),
        (ch("x"), none, None),
        (ch("b"), none, Some(Action::ToggleToolbar)),
        (ch("B"), none, Some(Action::ToggleToolbar)),
        (ch("w"), none, Some(Action::ToggleWide)),
        (ch("W"), none, Some(Action::ToggleWide)),
        (ch("W"), shift, Some(Action::ToggleWide)),
        (ch("W"), ctrl, Some(Action::Quit)),
        (ch("w"), ModifiersState::ALT, None),
        // Contents; Ctrl+C stays Copy (or nothing without selection support).
        #[cfg(feature = "toc")]
        (ch("c"), none, Some(Action::ToggleToc)),
        #[cfg(feature = "toc")]
        (ch("C"), none, Some(Action::ToggleToc)),
        #[cfg(feature = "toc")]
        (ch("C"), shift, Some(Action::ToggleToc)),
        #[cfg(not(feature = "toc"))]
        (ch("c"), none, None),
        #[cfg(feature = "toc")]
        (ch("c"), ModifiersState::ALT, None),
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
                .recv_timeout(std::time::Duration::from_secs(60))
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
            .recv_timeout(std::time::Duration::from_secs(60))
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

    #[test]
    fn wide_layout_keeps_the_reading_position() {
        let now = Instant::now();
        let mut v = view(&wrapping(100), 1600, 500);
        settle(&mut v);
        let (_, capped) = v.layout().column();
        v.set_scroll(v.layout().top(40) + 5.0);
        let block = v.anchor().block;
        assert_eq!(block, 40);
        assert!(!v.wide());
        assert_eq!(v.apply(Action::ToggleWide, now), Outcome::Redraw);
        v.ensure();
        assert!(v.wide());
        let (x, w) = v.layout().column();
        assert!(w > capped, "the column grew: {w} > {capped}");
        assert_eq!((x, w), (32.0, 1600.0 - 64.0), "margins only");
        assert_eq!(v.anchor().block, block, "wide");
        v.apply(Action::ToggleWide, now);
        v.ensure();
        assert!(!v.wide());
        assert_eq!(v.layout().column().1, capped, "back to the cap");
        assert_eq!(v.anchor().block, block, "normal again");
    }

    #[test]
    fn set_wide_does_nothing_when_unchanged() {
        let mut v = view(&paras(50), 1600, 500);
        settle(&mut v);
        let before = v.layout().params();
        v.set_wide(false);
        assert_eq!(v.layout().params(), before);
        assert!(v.layout().complete(), "nothing was remeasured");
        v.set_wide(true);
        assert!(v.layout().params().wide);
        assert!(v.wide());
        v.set_wide(true);
        assert!(v.wide());
        v.set_wide(false);
        assert_eq!(v.layout().params(), before);
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

    /// The pointer over the scrollbar's column shows it, holds it while it stays (no timer),
    /// and on leaving it lingers and fades as after a scroll. Elsewhere the pointer doesn't.
    #[test]
    fn hovering_the_scrollbar_column_shows_it() {
        let t0 = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        let thumb = v.thumb_at(0.0).unwrap();
        let x = f64::from(thumb.x1 - 1);
        let mut buf = vec![0; 600 * 400];
        let thumb_px = |buf: &[u32]| (0..400).any(|y| buf[y * 600 + 595] != LIGHT.bg);
        // Over the text: nothing.
        assert!(!v.hover_at(100.0, 300.0, t0));
        assert_eq!(v.deadline(), None);
        // Over the column, even far from the thumb: shown and held.
        assert!(v.hover_at(x, 300.0, t0), "appears");
        assert!(!v.hover_at(x, 310.0, t0), "already showing");
        assert_eq!(v.deadline(), None, "held, no timer");
        let late = t0 + Duration::from_secs(60);
        assert!(!v.expire(late));
        v.paint(&mut buf, 600, late);
        assert!(thumb_px(&buf), "still shown while hovered");
        // While hovered, a press on the track jumps there, as on a visible scrollbar.
        assert!(v.press(x, 300.0, late));
        v.release(late);
        // Leaving lets it linger, then hide.
        assert!(!v.hover_at(100.0, 300.0, late));
        let deadline = v.deadline().unwrap();
        assert!(deadline > late);
        assert!(!v.expire(deadline - Duration::from_millis(1)));
        assert!(v.expire(deadline));
        v.paint(&mut buf, 600, deadline);
        assert!(!thumb_px(&buf), "hidden after leaving");
        // Leaving the window over it does the same.
        assert!(v.hover_at(x, 300.0, deadline));
        assert!(!v.hover_end());
        assert!(v.deadline().is_some());
        // A document that doesn't scroll has no scrollbar to show.
        let mut short = view("Short.", 600, 400);
        settle(&mut short);
        assert!(!short.hover_at(595.0, 200.0, t0));
        assert_eq!(short.deadline(), None);
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

    /// Headings of nothing but punctuation or emoji have an empty slug, or one that moves with
    /// the others: the position follows their texts, and an empty slug is no match of its own.
    #[test]
    fn reload_among_headings_without_a_slug() {
        let sec = |h: &str| format!("## {h}\n\nbody {h}\n\nmore {h}\n\n");
        let end = format!("# End\n\n{}", paras(30));
        let src = format!("# A\n\n{}{}{}{end}", sec("!!!"), sec("🚀"), sec("!!!"));
        let mut v = view(&src, 600, 100);
        settle(&mut v);
        for text in ["more !!!", "more 🚀"] {
            let before = read_at(&mut v, text, 2.0);
            v.replace(Doc::parse(&src), None);
            assert_eq!(v.anchor(), before, "{text}");
        }
        // The same text, the second of two with the same path, stays the second.
        let blocks: Vec<u32> = (0..v.doc().blocks.len() as u32)
            .filter(|&b| block_text(&v, b) == "more !!!")
            .collect();
        v.set_scroll(v.layout().top(blocks[1] as usize));
        v.replace(
            Doc::parse(&src.replace("# End", "# Fin\n\nx\n\n# End")),
            None,
        );
        assert_eq!(v.anchor().block, blocks[1]);
        // The heading being read renamed: the empty slug it had is not the slug of the next
        // heading to lose its text, so the position stays at its block rather than jumping.
        let src = format!("# A\n\n{}{}{end}", sec("!!!"), sec("🚀"));
        v.replace(Doc::parse(&src), None);
        let before = read_at(&mut v, "more !!!", 2.0);
        assert_eq!(v.doc().headings[1].slug, "");
        v.replace(Doc::parse(&src.replacen("## !!!", "## Alpha", 1)), None);
        assert_eq!(v.doc().headings[2].slug, "");
        assert_eq!(
            v.anchor(),
            before,
            "no jump to the heading that is now `\"\"`"
        );
        // The same for the generated `-1`, `-2`: renaming the second of three leaves the third
        // with `-1`, which isn't the heading being read.
        let src = format!("# A\n\n{}{}{}{end}", sec("!!!"), sec("🚀"), sec("???"));
        v.replace(Doc::parse(&src), None);
        let before = read_at(&mut v, "more 🚀", 2.0);
        assert_eq!(v.doc().headings[2].slug, "-1");
        v.replace(Doc::parse(&src.replacen("## 🚀", "## Alpha", 1)), None);
        assert_eq!(v.doc().headings[3].slug, "-1");
        assert_eq!(
            v.anchor(),
            before,
            "no jump to the heading that is now `-1`"
        );
        // A heading actually named `-1` is found by its slug, though.
        let src = format!("# A\n\n{}{}{end}", sec("-1"), sec("Two"));
        v.replace(Doc::parse(&src), None);
        let before = read_at(&mut v, "more -1", 2.0);
        v.replace(
            Doc::parse(
                &src.replacen("## Two", "## Also", 1)
                    .replacen("# A", "# B", 1),
            ),
            None,
        );
        assert_eq!(v.anchor().block, before.block);
    }

    /// What the parser knows about a heading's slug is what reload goes by, not its text as laid
    /// out: a footnote's backlink, a split heading's later chunks and a literal `-1` all fooled a
    /// slug worked out again from the text in the path.
    #[test]
    fn reload_among_bare_headings_by_what_the_parser_knew() {
        let end = format!("# End\n\n{}", paras(30));
        // A footnote ending in a heading has a backlink after it, which the slug never saw.
        let note =
            |label: &str, h: &str| format!("[^{label}]:\n    text {label}\n\n    ## {h}\n\n");
        let src = format!(
            "# A\n\nx[^a] y[^b] z[^c]\n\n{}{}{}{}{end}",
            paras(10),
            note("a", "!!!"),
            note("b", "🚀"),
            note("c", "???"),
        );
        let mut v = view(&src, 600, 100);
        settle(&mut v);
        let rocket = (0..v.doc().blocks.len() as u32)
            .find(|&b| block_text(&v, b).starts_with("🚀"))
            .unwrap();
        v.set_scroll(v.layout().top(rocket as usize) + 2.0);
        let before = v.anchor();
        assert_eq!(before.block, rocket);
        let at = v.doc().headings.iter().position(|h| h.block == rocket);
        assert!(v.doc().headings[at.unwrap()].bare);
        v.replace(Doc::parse(&src.replacen("## 🚀", "## Alpha", 1)), None);
        assert_eq!(v.anchor().block, rocket, "no jump to the heading now `-1`");

        // A split heading is slugged whole, though the path holds its first chunk only.
        let split = format!("## {}Target", "!".repeat(16_384));
        let src = format!("# A\n\n{split}\n\nbelow the heading\n\n{end}");
        let mut v = view(&src, 600, 100);
        settle(&mut v);
        let target = v.doc().headings[1].clone();
        assert_eq!(target.slug, "target");
        assert!(!target.bare);
        let before = read_at(&mut v, "below the heading", 2.0);
        let src = src.replacen("# A", "# B", 1).replacen(
            &split,
            &format!("para one\n\npara two\n\n{split}"),
            1,
        );
        v.replace(Doc::parse(&src), None);
        assert_eq!(v.anchor().block, before.block + 2, "follows the slug");

        // A heading literally named `-1` is not the `-1` generated for an empty one.
        let literal = "## -1\n\nfirst literal\n\nmore literal\n\n## Two\n\nmore two\n\n";
        let src = format!("# A\n\n{literal}{end}");
        let mut v = view(&src, 600, 100);
        settle(&mut v);
        let before = read_at(&mut v, "more literal", 2.0);
        let src = format!("# B\n\n## !!!\n\nfirst bare\n\n## ???\n\nsecond bare\n\n{literal}{end}");
        let doc = Doc::parse(&src);
        let slugs: Vec<_> = doc.headings.iter().map(|h| h.slug.as_str()).collect();
        assert_eq!(slugs[1..4], ["", "-1", "-1-1"]);
        v.replace(doc, None);
        // The heading path is gone and no slug names the heading: the block position stays,
        // not the section of the `-1` that is now somebody else's.
        assert_eq!(block_text(&v, v.anchor().block), "???");
        assert_eq!(v.anchor(), before);
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
        // (Seconds apart: presses in a quick row are a double click, which doesn't follow.)
        for (n, (x, y)) in outside.into_iter().enumerate() {
            assert_eq!(v.link_at(x, y), None, "({x}, {y})");
            assert!(v.hover(lx, ly) || v.hovered().is_some());
            v.hover(x, y);
            assert_eq!(v.hovered(), None, "hover at ({x}, {y})");
            v.press(lx, ly, now + Duration::from_secs(n as u64));
            assert_eq!(v.click(x, y), None, "released at ({x}, {y})");
        }
        // Inside still works.
        assert!(v.hover(lx, ly));
        assert_eq!(v.hovered(), Some("https://example.com/x"));
        let now = now + Duration::from_secs(60);
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
        // Open is the shell's: the view only asks for it.
        let anchor = v.anchor();
        assert_eq!(v.apply(Action::Open, now), Outcome::Open);
        assert_eq!(v.anchor(), anchor);
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

    /// The view at 200 px wide (`⋯` holds zoom and wide layout) with the dropdown open.
    fn open_menu_view(now: Instant) -> View {
        let mut v = view(&paras(20), 200, 300);
        v.set_toolbar(true);
        let more = toolbar::slots(&v.toolbar_ctx())
            .items
            .iter()
            .find(|(i, _)| *i == toolbar::Item::More)
            .unwrap()
            .1;
        let (mx, my) = (f64::from((more.x0 + more.x1) / 2.0), 16.0);
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
        // Reload sits at the right edge, or just left of the find button (no settings file here).
        let find = if cfg!(feature = "find") { 32.0 } else { 0.0 };
        let (x, y) = (600.0 - 4.0 - 16.0 - find, 16.0);
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
    #[cfg(feature = "images")]
    fn frame(v: &mut View, now: Instant) {
        let (w, h) = v.size();
        let mut buf = vec![0u32; (w * h) as usize];
        v.paint(&mut buf, w as usize, now);
        v.tick(now, Some(now));
    }

    /// Poll the image threads until `done` holds (fails after a while).
    #[cfg(feature = "images")]
    fn wait_images(v: &mut View, done: impl Fn(&View) -> bool) {
        let t = Instant::now();
        while !done(v) {
            assert!(t.elapsed() < Duration::from_secs(60), "timed out");
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
            assert!(t.elapsed() < Duration::from_secs(60), "never painted");
            v.tick(Instant::now(), Some(Instant::now()));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(buf.iter().filter(|&&p| p == 0x119933).count(), 40 * 20);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An image over VOMD's limits says why and "click to load" in its placeholder (not text of
    /// the document: find doesn't find it), with the hand and a note under the pointer. A click
    /// loads it, once however many clicks; a drag or a Shift+click doesn't. In a link, a click
    /// follows the link and Ctrl+click loads it.
    #[cfg(feature = "images")]
    #[test]
    fn a_click_loads_an_image_past_the_limits() {
        let dir = std::env::temp_dir().join(format!("vomd-view-force-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = |w: u32, h: u32| {
            let mut out = Vec::new();
            let mut enc = ::png::Encoder::new(&mut out, w, h);
            enc.set_color(::png::ColorType::Grayscale);
            enc.set_depth(::png::BitDepth::One);
            let data = vec![0u8; w.div_ceil(8) as usize * h as usize];
            enc.write_header().unwrap().write_image_data(&data).unwrap();
            out
        };
        std::fs::write(dir.join("a.png"), big(9000, 8000)).unwrap();
        std::fs::write(dir.join("b.png"), big(8000, 9000)).unwrap();
        let mut v = view(
            "![big a](a.png)\n\n[![big b](b.png)](https://e.x/)",
            600,
            400,
        );
        v.set_image_base(Some(dir.clone()));
        let now = Instant::now();
        frame(&mut v, now);
        let refused = |w, h| Some(ImageSize::Refused(Refusal::Pixels(w, h)));
        assert_eq!(v.layout().image_size(1), refused(9000, 8000));
        assert_eq!(v.layout().image_size(2), refused(8000, 9000));
        let at = |v: &View, id| {
            (0..600)
                .flat_map(|x| (0..200).map(move |y| (f64::from(x), f64::from(y))))
                .find(|&(x, y)| v.image_at(x, y) == Some(id))
                .expect("a placeholder on screen")
        };
        let (a, b) = (at(&v, 1), at(&v, 2));
        // Under the pointer: the hand and the note; in a link, the link's.
        v.hover(a.0, a.1);
        assert_eq!(v.hover_note, Some(FORCE_NOTE));
        assert!(v.over_force() && v.hovered().is_none());
        v.hover(b.0, b.1);
        assert!(!v.over_force() && v.hovered().is_some());
        // Not the document's text.
        #[cfg(feature = "find")]
        {
            v.set_find(true);
            v.find_type("click to load");
            assert_eq!(v.find_total(), 0);
            v.set_find(false);
        }
        // A drag from it selects, and loads nothing; nor does a Shift+click.
        v.press(a.0, a.1, now);
        #[cfg(feature = "clipboard")]
        {
            v.select_to(a.0 + 200.0, a.1 + 60.0, now);
            v.release_select();
            assert_eq!(v.click(a.0 + 200.0, a.1 + 60.0), None);
            assert_eq!(v.layout().image_size(1), refused(9000, 8000));
            v.set_shift(true);
            v.press(a.0, a.1, now + Duration::from_secs(1));
            v.release_select();
            assert_eq!(v.click(a.0, a.1), None);
            assert_eq!(v.layout().image_size(1), refused(9000, 8000));
            v.set_shift(false);
        }
        // In a link: a click follows it, Ctrl+click loads the image.
        let later = now + Duration::from_secs(5);
        v.press(b.0, b.1, later);
        assert_eq!(v.click(b.0, b.1).as_deref(), Some("https://e.x/"));
        assert_eq!(v.layout().image_size(2), refused(8000, 9000));
        v.set_ctrl(true);
        v.press(b.0, b.1, later + Duration::from_secs(1));
        assert_eq!(v.click(b.0, b.1), None, "not followed");
        v.set_ctrl(false);
        assert_eq!(
            v.layout().image_size(2),
            Some(ImageSize::Pixels(8000, 9000))
        );
        // A double click loads it once.
        let t = later + Duration::from_secs(5);
        v.press(a.0, a.1, t);
        assert_eq!(v.click(a.0, a.1), None);
        v.ensure();
        v.press(a.0, a.1, t + Duration::from_millis(50));
        assert_eq!(v.click(a.0, a.1), None);
        assert_eq!(
            v.layout().image_size(1),
            Some(ImageSize::Pixels(9000, 8000))
        );
        wait_images(&mut v, |v| {
            v.images.forced_loads() >= 2 && !v.images.forcing()
        });
        assert_eq!(v.images.forced_loads(), 2, "one load each");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Shift+click is the selection's whether or not there is one: with none yet, on a fresh
    /// view, it loads no image either.
    #[cfg(all(feature = "images", feature = "clipboard"))]
    #[test]
    fn shift_click_without_a_selection_loads_no_image() {
        let dir = std::env::temp_dir().join(format!("vomd-view-shift-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Vec::new();
        let mut enc = ::png::Encoder::new(&mut png, 9000, 8000);
        enc.set_color(::png::ColorType::Grayscale);
        enc.set_depth(::png::BitDepth::One);
        let data = vec![0u8; 9000usize.div_ceil(8) * 8000];
        enc.write_header().unwrap().write_image_data(&data).unwrap();
        std::fs::write(dir.join("a.png"), &png).unwrap();
        let mut v = view("![big](a.png)", 600, 400);
        v.set_image_base(Some(dir.clone()));
        let now = Instant::now();
        frame(&mut v, now);
        let refused = Some(ImageSize::Refused(Refusal::Pixels(9000, 8000)));
        assert_eq!(v.layout().image_size(1), refused);
        let (x, y) = (0..600)
            .flat_map(|x| (0..200).map(move |y| (f64::from(x), f64::from(y))))
            .find(|&(x, y)| v.image_at(x, y) == Some(1))
            .expect("a placeholder on screen");
        v.set_shift(true);
        v.press(x, y, now);
        v.release_select();
        assert_eq!(v.click(x, y), None);
        assert_eq!(v.layout().image_size(1), refused);
        v.set_shift(false);
        // Without Shift the same click loads it.
        v.press(x, y, now + Duration::from_secs(5));
        assert_eq!(v.click(x, y), None);
        assert_eq!(
            v.layout().image_size(1),
            Some(ImageSize::Pixels(9000, 8000))
        );
        wait_images(&mut v, |v| {
            v.images.forced_loads() >= 1 && !v.images.forcing()
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Paint a frame of `v`, then run what the window does with the image threads until they
    /// have nothing left to do and nothing has changed for a while: a poll for what they finish,
    /// a frame if it says so.
    #[cfg(feature = "images")]
    fn settle_images(v: &mut View) {
        let t = Instant::now();
        frame(v, Instant::now());
        let mut quiet = 0;
        let mut was = (v.image_stats(), v.images.wakes());
        while quiet < 60 {
            assert!(t.elapsed() < Duration::from_secs(120), "never settled");
            if v.poll_images(Instant::now()) {
                frame(v, Instant::now());
            }
            std::thread::sleep(Duration::from_millis(5));
            let now = (v.image_stats(), v.images.wakes());
            quiet = if now == was && v.images.is_idle() {
                quiet + 1
            } else {
                0
            };
            was = now;
        }
    }

    /// No room for an image now is not a failure of the image (round 13), through the whole
    /// window: 835 x 10,000 (a bitmap that takes the budget) and 300 valid 1 x 1 images in one
    /// paragraph, shown at the bottom, then at half the zoom. The ones the cache had no room for
    /// while the bitmap was painted showed their alt text for good (45 of them, and then the
    /// bitmap's own replacement at the new zoom): none does, and with the bitmap's room gone to
    /// a smaller size every one is loaded.
    #[cfg(feature = "images")]
    #[test]
    fn images_without_room_are_not_failed_by_scrolling_or_zooming() {
        let dir = std::env::temp_dir().join(format!("vomd-view-room-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = |w: u32, h: u32| {
            let mut out = Vec::new();
            let mut enc = ::png::Encoder::new(&mut out, w, h);
            enc.set_color(::png::ColorType::Grayscale);
            enc.set_compression(::png::Compression::Fast);
            enc.write_header()
                .unwrap()
                .write_image_data(&vec![0u8; (w * h) as usize])
                .unwrap();
            out
        };
        std::fs::write(dir.join("big.png"), png(835, 10_000)).unwrap();
        std::fs::write(dir.join("tiny.png"), png(1, 1)).unwrap();
        let mut src = String::from("![b](big.png)");
        src.extend((0..300).map(|_| " ![t](tiny.png)"));
        let mut v = View::with_base(
            Fonts::new().unwrap(),
            Doc::parse(&src),
            None,
            (1920, 1080),
            1.0,
            Some(dir.clone()),
        );
        v.set_text(Params::default().font_size, 160.0);
        v.ensure();
        settle_images(&mut v);
        v.set_scroll(v.max_scroll());
        settle_images(&mut v);
        let stats = v.image_stats();
        println!(
            "at the bottom: {} of 301 decoded, {} failed",
            stats.decoded, stats.failed
        );
        let at_bottom = stats.failed;
        v.set_zoom(50);
        settle_images(&mut v);
        let stats = v.image_stats();
        println!(
            "at 50%: {} of 301 decoded, {} failed",
            stats.decoded, stats.failed
        );
        assert_eq!(
            (at_bottom, stats.failed),
            (0, 0),
            "an image with no room now is not failed, nor is the bitmap's replacement"
        );
        assert!((1..=301).all(|id| v.images.is_waiting(id)));
        assert!(
            (1..=301).all(|id| v.layout().image_size(id) != Some(ImageSize::Alt)),
            "an image shows its alt text"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A result nobody has painted is no pixels a window keeps (round 14), through the window: a
    /// local 1000 x 2000 PNG is decoded and its result taken after the last frame, and the window
    /// does not repaint (Wayland, until the compositor calls back: the event loop runs, and
    /// `tick` asks for the images again after the reader presses I). The 3200 x 2400 JPEG that
    /// is fetched for the part of the document it is scrolled to doesn't fit beside the PNG's
    /// 8 MB: the PNG's result was taken for painted, and the decode was run beside it, 1 MB over
    /// the budget with nothing painted (34.6 MB of the image code's live bytes in 33.5). It is
    /// evicted for the decode now.
    #[cfg(feature = "remote-images")]
    #[test]
    fn a_result_nobody_painted_is_evicted_for_a_decode_in_a_window_that_has_not_repainted() {
        use crate::remote::{Error, STEP};
        let dir = std::env::temp_dir().join(format!("vomd-view-unpainted-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Vec::new();
        {
            let mut enc = ::png::Encoder::new(&mut png, 1000, 2000);
            enc.set_color(::png::ColorType::Grayscale);
            enc.set_compression(::png::Compression::Fast);
            enc.write_header()
                .unwrap()
                .write_image_data(&vec![0u8; 1000 * 2000])
                .unwrap();
        }
        std::fs::write(dir.join("p.png"), &png).unwrap();
        // The fixture's JPEG, said to be 3200 x 2400.
        let mut jpeg = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/wide.jpg"),
        )
        .unwrap();
        let sof = jpeg.windows(2).position(|p| p == [0xFF, 0xC0]).unwrap();
        jpeg[sof + 5..sof + 7].copy_from_slice(&2400u16.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&3200u16.to_be_bytes());
        let mut src = String::from("![p](p.png)\n\n");
        src.extend((0..60).map(|i| format!("Text between them, line {i}.\n\n")));
        src.push_str("![j](https://x.y/j.jpg)\n");
        let mut v = View::with_base(
            Fonts::new().unwrap(),
            Doc::parse(&src),
            None,
            (1920, 1080),
            1.0,
            Some(dir.clone()),
        );
        v.set_text(Params::default().font_size, 160.0);
        v.images.set_policy(crate::settings::RemoteImages::Always);
        v.images
            .remote
            .set_fetcher(std::sync::Arc::new(move |_, grant| {
                for _ in 0..jpeg.len().div_ceil(STEP) {
                    if !grant(STEP) {
                        return Err(Error::Yielded);
                    }
                }
                Ok((std::sync::Arc::new(jpeg.clone()), 3200, 2400))
            }));
        v.ensure();
        // The frame that asks for the PNG; its result is taken, and nothing is painted after.
        frame(&mut v, Instant::now());
        let t = Instant::now();
        while v.image_stats().decodes < 1 {
            assert!(
                t.elapsed() < Duration::from_secs(60),
                "the PNG never decoded"
            );
            v.poll_images(Instant::now());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(v.image_stats().decoded, 1);
        // Scrolled to the JPEG, and the reader presses I until it is there: ticks and polls.
        v.set_scroll(v.max_scroll());
        let t = Instant::now();
        while v.image_stats().decodes < 2 {
            assert!(
                t.elapsed() < Duration::from_secs(60),
                "the JPEG never decoded: {}",
                v.images.memory_report()
            );
            v.set_remote(true);
            v.tick(Instant::now(), Some(Instant::now()));
            v.poll_images(Instant::now());
            std::thread::sleep(Duration::from_millis(2));
        }
        // The PNG's result went to make room: it was not painted, and nothing kept it.
        let report = v.images.memory_report();
        assert_eq!(v.image_stats().decoded, 1, "{report}");
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

    /// Files over 10 MB get a one-line notice for a few seconds from their first frame, drawn
    /// at the top below the toolbar.
    #[test]
    fn large_files_show_a_notice() {
        assert_eq!(large_notice(LARGE_FILE), None);
        assert_eq!(
            large_notice(23 << 20).as_deref(),
            Some("This file is 23 MB, so it may be slow")
        );
        let mut v = view(&paras(40), 600, 400);
        let mut plain = vec![0u32; 600 * 400];
        let now = Instant::now();
        v.paint(&mut plain, 600, now);
        v.set_notice(large_notice(LARGE_FILE + 1));
        assert_eq!(v.notice(), Some("This file is 10 MB, so it may be slow"));
        assert_eq!(v.deadline(), None, "the time starts with the first frame");
        let mut buf = vec![0u32; 600 * 400];
        v.paint(&mut buf, 600, now);
        assert_eq!(v.deadline(), Some(now + NOTICE));
        // Drawn in the top middle, nowhere near the bottom.
        let differs = |rows: std::ops::Range<usize>| {
            rows.flat_map(|y| (0..600).map(move |x| y * 600 + x))
                .any(|i| buf[i] != plain[i])
        };
        assert!(differs(0..60));
        assert!(!differs(200..400));
        // A later frame doesn't push the time back.
        v.paint(&mut buf, 600, now + Duration::from_secs(1));
        assert_eq!(v.deadline(), Some(now + NOTICE));
        assert!(!v.tick(now + Duration::from_secs(1), None).redraw);
        assert!(v.tick(now + NOTICE, None).redraw);
        assert_eq!(v.notice(), None);
        assert_eq!(v.deadline(), None);
        // Another document takes it away.
        v.set_notice(Some("x".into()));
        v.open(Doc::parse("# b"), None);
        assert_eq!(v.notice(), None);
    }

    /// The shell sets the notice after opening a document (argument, link, Back, drop) and
    /// leaves it alone on reload: `open_in` clears one showing, `replace` keeps it.
    #[test]
    fn a_reload_keeps_the_notice_and_opening_clears_it() {
        let mut v = view(&paras(40), 600, 400);
        v.set_notice(large_notice(LARGE_FILE + 1));
        let now = Instant::now();
        let mut buf = vec![0u32; 600 * 400];
        v.paint(&mut buf, 600, now);
        assert_eq!(v.deadline(), Some(now + NOTICE));
        v.replace(Doc::parse(&paras(41)), None);
        assert_eq!(v.notice(), Some("This file is 10 MB, so it may be slow"));
        assert_eq!(v.deadline(), Some(now + NOTICE), "the time isn't restarted");
        v.open_in(Doc::parse("# b"), None, None);
        assert_eq!(v.notice(), None);
        assert_eq!(v.deadline(), None);
        // The next document's own notice then shows from its first frame.
        v.set_notice(large_notice(LARGE_FILE + 1));
        assert_eq!(v.deadline(), None);
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
            assert_eq!(v.hover_note, Some(BLOCKED_NOTE));
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
            assert_eq!(v.hover_note, None, "on: not blocked");
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

    /// The contents panel.
    #[cfg(feature = "toc")]
    mod contents {
        use super::*;
        #[cfg(feature = "clipboard")]
        use crate::doc::BlockKind;

        /// `n` sections, each a heading `S0`, `S1`, ... and a paragraph with a link.
        fn sections(n: usize) -> String {
            (0..n)
                .map(|i| {
                    format!(
                        "## S{i}\n\nPadding words to push the text far right of any panel. \
                         [link {i}](https://a.example/{i}) of section {i}.\n\n"
                    )
                })
                .collect()
        }

        /// A block on screen with a link in it.
        fn link_block(v: &View) -> usize {
            let h = f64::from(v.height);
            v.layout()
                .range_exact(v.scroll(), v.scroll() + h)
                .find(|&b| {
                    v.layout().geoms()[b].content.is_some()
                        && v.doc()
                            .block_spans(&v.doc().blocks[b])
                            .iter()
                            .any(|s| s.link != 0)
                })
                .expect("a link on screen")
        }

        fn paint(v: &mut View, now: Instant) -> Vec<u32> {
            let (w, h) = v.size();
            let mut buf = vec![0; (w * h) as usize];
            v.paint(&mut buf, w as usize, now);
            buf
        }

        /// A window point on row `i` of the panel.
        fn row_point(v: &View, i: usize) -> (f64, f64) {
            let g = v.toc_geom();
            (40.0, g.top(i, v.toc.scroll) + g.row() / 2.0)
        }

        /// Press and release on (`x`, `y`) as the shell does, and what the click returned.
        fn click(v: &mut View, (x, y): (f64, f64), now: Instant) -> Option<String> {
            v.hover_at(x, y, now);
            v.press(x, y, now);
            v.release_toolbar(x, y);
            #[cfg(feature = "clipboard")]
            v.release_select();
            v.release(now);
            v.click(x, y)
        }

        /// Click on (`x`, `y`) as [`click`] does, and the heading block a row asked to jump to.
        fn click_row(v: &mut View, at: (f64, f64), now: Instant) -> Option<u32> {
            assert_eq!(click(v, at, now), None, "a row is no link");
            v.take_jump()
        }

        /// A view of `n` sections with the panel open, painted once.
        fn open_view(n: usize, w: u32, h: u32) -> View {
            let mut v = view(&sections(n), w, h);
            v.set_toc(true);
            settle(&mut v);
            paint(&mut v, Instant::now());
            v
        }

        #[test]
        fn the_key_the_action_and_the_toolbar_toggle_it() {
            let now = Instant::now();
            let mut v = view(&sections(5), 1000, 400);
            assert!(!v.toc_open());
            let action = key_action(&ch("c"), ModifiersState::empty()).unwrap();
            assert_eq!(action, Action::ToggleToc);
            assert_eq!(v.apply(action, now), Outcome::Redraw);
            assert!(v.toc_open());
            assert_eq!(v.apply(Action::ToggleToc, now), Outcome::Redraw);
            assert!(!v.toc_open());
            // The toolbar's button asks for the same action, and the bar shows its state.
            v.set_toolbar(true);
            let button = |v: &View| {
                let ctx = v.toolbar_ctx();
                let slots = toolbar::slots(&ctx);
                let r = slots
                    .items
                    .iter()
                    .find(|(i, _)| *i == toolbar::Item::Contents)
                    .unwrap()
                    .1;
                (
                    f64::from((r.x0 + r.x1) / 2.0),
                    f64::from((r.y0 + r.y1) / 2.0),
                )
            };
            let (x, y) = button(&v);
            assert!(v.press(x, y, now));
            assert!(v.release_toolbar(x, y));
            assert_eq!(
                v.take_command(),
                Some(toolbar::Command::Action(Action::ToggleToc))
            );
            assert!(!v.toolbar_ctx().toc);
            v.apply(Action::ToggleToc, now);
            assert!(v.toolbar_ctx().toc);
            // Below the bar, from the bar's bottom to the window's: the panel's rectangle.
            let g = v.toc_geom();
            assert_eq!((g.y0, g.y1, g.width), (32.0, 400.0, 260.0));
            // No Ctrl, Alt or Super: Ctrl+C is not it.
            assert_ne!(
                key_action(&ch("c"), ModifiersState::CONTROL),
                Some(Action::ToggleToc)
            );
            assert_ne!(
                key_action(&ch("c"), ModifiersState::SUPER),
                Some(Action::ToggleToc)
            );
        }

        #[test]
        fn a_wide_window_is_pushed_aside_keeping_the_position() {
            let now = Instant::now();
            let mut v = view(&sections(200), 1000, 400);
            settle(&mut v);
            v.set_scroll(v.layout().top(120) + 5.0);
            let anchor = v.anchor();
            assert_eq!(v.layout().params().width, 1000.0);
            v.set_toc(true);
            assert!(v.toc_pushes());
            assert_eq!(v.area_x(), 260);
            assert_eq!(v.layout().params().width, 740.0, "laid out in what is left");
            assert_eq!(v.anchor(), anchor, "the reading position is kept");
            // The document is painted from the panel's edge, and what hits it is shifted too.
            let area = v.viewport().area;
            assert_eq!((area.x0, area.x1), (260, 1000));
            let (col_x, col_w) = v.layout().column();
            assert!(col_x + col_w <= 740.0);
            // Nothing of the document is under the panel: the panel is the page's neighbour.
            settle(&mut v);
            let block = link_block(&v);
            let (x, y) = point_at(&v, block, "link");
            let (x, y) = (x + 260.0, y);
            assert!(
                v.link_at(x, y).is_some(),
                "the link is where the page shows it"
            );
            assert_eq!(v.link_at(x - 260.0, y), None, "and not where it was");
            assert!(x > 260.0);
            // Closing gives the width back, with the position.
            v.apply(Action::ToggleToc, now);
            assert_eq!(v.area_x(), 0);
            assert_eq!(v.layout().params().width, 1000.0);
            assert_eq!(v.anchor(), anchor);
            // Wide enough or not is decided by the window: resizing across the limit relays out.
            v.set_toc(true);
            v.resize(739, 400);
            assert!(!v.toc_pushes());
            assert_eq!(v.layout().params().width, 739.0);
            assert_eq!(v.anchor(), anchor);
            v.resize(740, 400);
            assert!(v.toc_pushes());
            assert_eq!(v.layout().params().width, 480.0);
            assert_eq!(v.anchor(), anchor);
            // The panel scales with the display, not the zoom.
            v.resize(1480, 400);
            v.set_hidpi(2.0);
            assert_eq!(v.toc_width(), 520);
            assert!(v.toc_pushes());
            v.set_zoom(150);
            assert_eq!(v.toc_width(), 520);
            v.set_hidpi(1.0);
            v.set_zoom(100);
        }

        #[test]
        fn a_narrow_window_is_covered_not_laid_out_again() {
            let now = Instant::now();
            let mut v = view(&sections(50), 700, 400);
            settle(&mut v);
            let (x, y) = point_at(&v, 1, "link");
            assert!(v.link_at(x, y).is_some());
            v.set_toc(true);
            assert!(!v.toc_pushes());
            assert_eq!(v.area_x(), 0);
            assert_eq!(v.toc_width(), 260);
            assert_eq!(v.layout().params().width, 700.0, "no relayout");
            // The document keeps its place, but what is under the panel can't be reached.
            assert!(x > 260.0 || v.link_at(x, y).is_none());
            for probe in [(20.0, 100.0), (259.0, 300.0)] {
                assert_eq!(v.link_at(probe.0, probe.1), None);
                assert!(v.toc_covers(probe.0, probe.1));
                assert!(!v.in_area(probe.0, probe.1));
            }
            assert!(!v.toc_covers(261.0, 100.0));
            assert!(v.in_area(261.0, 100.0));
            // What the document paints on top of the panel's neighbours stays right of it.
            paint(&mut v, now);
            v.apply(Action::ToggleToc, now);
            assert_eq!(v.layout().params().width, 700.0);
        }

        #[test]
        fn clicking_a_row_jumps_to_its_heading() {
            let now = Instant::now();
            let mut v = open_view(40, 1000, 400);
            // Rows light up and show the hand under the pointer.
            let p = row_point(&v, 3);
            assert!(v.hover_at(p.0, p.1, now));
            assert!(v.over_toc());
            assert_eq!(v.hovered(), None, "no URL preview for a row");
            assert!(v.hover_end());
            assert!(!v.over_toc());
            // The click asks for the heading's block, which the shell jumps to like a link.
            let block = click_row(&mut v, p, now).unwrap();
            assert_eq!(block, v.doc().headings[3].block);
            assert_eq!(v.take_jump(), None, "once");
            v.jump_to_block(block, now);
            assert_eq!(v.anchor().block, block);
            assert!(v.toc_open(), "a panel beside the document stays");
            // Pressed on one row and released on another, or off the panel: no click.
            let q = row_point(&v, 5);
            v.press(p.0, p.1, now);
            assert_eq!(v.click(q.0, q.1), None);
            v.press(p.0, p.1, now);
            assert_eq!(v.click(600.0, 100.0), None);
            // The space below the last row is no row.
            let mut short = open_view(3, 1000, 400);
            let last = row_point(&short, 2);
            assert!(short.toc_row_at(last.0, last.1).is_some());
            assert_eq!(click_row(&mut short, (40.0, last.1 + 40.0), now), None);
            // Over the document: a click follows links as ever.
            settle(&mut v);
            let block = link_block(&v);
            let (x, y) = point_at(&v, block, "link");
            let href = click(&mut v, (x + 260.0, y), now).unwrap();
            assert!(href.starts_with("https://a.example/"), "{href}");
        }

        /// A heading of punctuation or emoji has no anchor to follow, but its row jumps to it.
        #[test]
        fn clicking_a_row_without_an_anchor_jumps_to_its_heading() {
            let now = Instant::now();
            let pad = "Padding words of a paragraph.\n\n".repeat(12);
            let src = format!("# Top\n\n{pad}## !!!\n\n{pad}## 🚀\n\n{pad}## Last\n\n{pad}");
            let mut v = view(&src, 1000, 300);
            v.set_toc(true);
            settle(&mut v);
            paint(&mut v, now);
            for row in [1, 2] {
                let block = v.doc().headings[row].block;
                let p = row_point(&v, row);
                assert_eq!(click_row(&mut v, p, now), Some(block));
                v.jump_to_block(block, now);
                settle(&mut v);
                paint(&mut v, now);
                assert_eq!(v.anchor().block, block);
                assert_eq!(v.toc_current(), Some(row), "the row is marked");
            }
            assert_eq!(v.toc.aimed.map(|(b, _)| b), Some(v.doc().headings[2].block));
        }

        /// Over the document, a click on a row closes the panel as it follows the link, and a
        /// click on the document closes it and does nothing more.
        #[test]
        fn an_overlaid_panel_closes_on_a_click() {
            let now = Instant::now();
            let mut v = open_view(40, 600, 400);
            assert!(!v.toc_pushes());
            let p = row_point(&v, 4);
            let want = v.doc().headings[4].block;
            assert_eq!(click_row(&mut v, p, now), Some(want));
            assert!(!v.toc_open(), "a click on a heading closes it");
            // A click on the document outside the panel closes it, and follows no link.
            v.set_toc(true);
            settle(&mut v);
            paint(&mut v, now);
            let block = link_block(&v);
            let (x, y) = point_at(&v, block, "link");
            assert!(x > 260.0, "the link is beside the panel: {x}");
            assert!(v.link_at(x, y).is_some());
            assert!(v.press(x, y, now), "the press is taken");
            assert!(!v.toc_open());
            assert_eq!(v.click(x, y), None, "no link followed");
            #[cfg(feature = "clipboard")]
            {
                assert!(!v.release_select());
                assert_eq!(v.selection(), None);
            }
            // The toolbar's own clicks don't count as the document's.
            v.set_toolbar(true);
            v.set_toc(true);
            assert!(v.press(20.0, 16.0, now));
            assert!(v.toc_open(), "the toolbar took the press");
        }

        #[test]
        fn escape_closes_the_menu_then_a_covering_panel_then_the_selection() {
            let now = Instant::now();
            // The dropdown first, over a panel that covers the whole narrow window.
            let mut v = open_menu_view(now);
            v.set_toc(true);
            assert_eq!(v.apply(Action::Escape, now), Outcome::Redraw);
            assert!(v.toc_open(), "the menu went first");
            assert_eq!(v.apply(Action::Escape, now), Outcome::Redraw);
            assert!(!v.toc_open());
            assert_eq!(v.apply(Action::Escape, now), Outcome::Quit);
            // A selection stays until the panel over the document is gone.
            #[cfg(feature = "clipboard")]
            {
                let mut v = view(&sections(10), 600, 400);
                v.set_toc(true);
                assert_eq!(v.apply(Action::SelectAll, now), Outcome::Selected);
                assert_eq!(v.apply(Action::Escape, now), Outcome::Redraw);
                assert!(!v.toc_open() && v.selection().is_some());
                assert_eq!(v.apply(Action::Escape, now), Outcome::Redraw);
                assert_eq!(v.selection(), None);
                assert_eq!(v.apply(Action::Escape, now), Outcome::Quit);
            }
            // A panel beside the document is not covering it: Escape leaves it.
            let mut v = view(&sections(10), 1000, 400);
            v.set_toc(true);
            assert_eq!(v.apply(Action::Escape, now), Outcome::Quit);
            assert!(v.toc_open());
        }

        #[cfg(feature = "clipboard")]
        #[test]
        fn a_selection_does_not_start_in_the_panel() {
            let now = Instant::now();
            let mut v = open_view(20, 1000, 400);
            let p = row_point(&v, 2);
            assert!(v.press(p.0, p.1, now));
            assert!(
                !v.select_to(600.0, 200.0, now),
                "a drag from the panel selects nothing"
            );
            assert!(!v.selecting());
            assert_eq!(v.selection(), None);
            assert!(!v.release_select());
            // Double and triple presses on a row select no word or block either.
            for _ in 0..3 {
                v.press(p.0, p.1, now);
                v.release_select();
                assert_eq!(v.selection(), None);
            }
            // A drag that starts in the document and runs over the panel goes on: the pointer
            // counts as at the panel's edge, where the document starts.
            settle(&mut v);
            let (_, sy) = point_at(&v, 1, "Padding");
            v.press(700.0, sy, now);
            assert!(v.select_to(262.0, sy + 20.0, now));
            assert!(v.selecting());
            let at_edge = v.selection().unwrap();
            v.select_to(30.0, sy + 20.0, now);
            assert_eq!(
                v.selection(),
                Some(at_edge),
                "clamped to the document's edge"
            );
            assert!(v.release_select());
            // In a narrow window the same press closes the panel instead, and starts nothing.
            let mut v = open_view(20, 600, 400);
            assert!(v.press(400.0, 100.0, now));
            assert!(!v.select_to(100.0, 200.0, now));
            assert_eq!(v.selection(), None);
        }

        #[test]
        fn the_wheel_over_the_panel_scrolls_it_and_not_the_document() {
            let now = Instant::now();
            let mut v = open_view(100, 1000, 300);
            let before = v.scroll();
            let p = row_point(&v, 2);
            v.hover_at(p.0, p.1, now);
            v.wheel_lines(3.0, now);
            assert_eq!(v.scroll(), before, "the document stays");
            assert!(v.toc.scroll > 0.0);
            let scrolled = v.toc.scroll;
            v.wheel_by(40.0, now);
            assert_eq!(v.scroll(), before);
            assert_eq!(v.toc.scroll, scrolled + 40.0);
            v.wheel_by(-1.0e9, now);
            assert_eq!(v.toc.scroll, 0.0, "it stops at the top");
            v.wheel_by(1.0e9, now);
            assert_eq!(v.toc.scroll, v.toc_geom().max_scroll(100), "and at the end");
            assert_eq!(v.scroll(), before);
            // Over the document it is the document's, and the panel is left.
            v.toc.scroll = 100.0;
            v.hover_at(600.0, 100.0, now);
            v.wheel_lines(3.0, now);
            assert!(v.scroll() > before);
            assert_eq!(v.toc.scroll, 100.0);
            // The keys scroll the document whatever the pointer is over.
            v.hover_at(p.0, p.1, now);
            let at = v.scroll();
            v.apply(Action::Lines(2), now);
            assert!(v.scroll() > at);
            assert_eq!(v.toc.scroll, 100.0);
            // A pointer that left the window leaves the panel its own.
            v.hover_end();
            v.wheel_by(50.0, now);
            assert_eq!(v.toc.scroll, 100.0);
        }

        #[test]
        fn a_notch_over_the_panel_is_rows_of_its_own_whatever_the_zoom() {
            let now = Instant::now();
            let mut v = open_view(100, 1000, 300);
            let step = 3.0 * v.toc_geom().row();
            let p = row_point(&v, 2);
            v.hover_at(p.0, p.1, now);
            v.wheel_lines(2.0, now);
            assert_eq!(v.toc.scroll, 2.0 * step);
            v.set_zoom(v.zoom() * 2);
            v.toc.scroll = 0.0;
            v.hover_at(p.0, p.1, now);
            v.wheel_lines(2.0, now);
            assert_eq!(
                v.toc.scroll,
                2.0 * step,
                "the document's zoom doesn't change it"
            );
        }

        #[test]
        fn reload_keeps_the_panel_and_a_new_document_resets_it() {
            let now = Instant::now();
            let mut v = open_view(100, 1000, 300);
            v.hover_at(50.0, 100.0, now);
            v.wheel_by(600.0, now);
            let kept = v.toc.scroll;
            assert!(kept > 0.0);
            v.replace(Doc::parse(&sections(100)), None);
            paint(&mut v, now);
            assert!(v.toc_open());
            assert_eq!(v.toc.scroll, kept);
            // A message in place of the document hides the panel and gives the room back; the
            // next good version brings it back, with its scroll.
            v.replace(Doc::parse(""), Some("Cannot open x".into()));
            assert!(v.toc_open());
            assert_eq!((v.toc_width(), v.area_x()), (0, 0));
            assert_eq!(v.layout().params().width, 1000.0);
            paint(&mut v, now);
            assert!(!v.over_toc());
            v.replace(Doc::parse(&sections(100)), None);
            assert_eq!(v.toc_width(), 260);
            assert_eq!(v.layout().params().width, 740.0);
            paint(&mut v, now);
            assert_eq!(v.toc.scroll, kept);
            // Another document starts the panel at its top and keeps it open.
            v.open(Doc::parse(&sections(30)), None);
            assert!(v.toc_open());
            assert_eq!(v.toc.scroll, 0.0);
            assert_eq!(v.toc_rows(), 30);
            paint(&mut v, now);
            assert_eq!(v.toc_rows(), 30);
            // The empty window has no panel; the setting is only remembered.
            let mut v = view("", 1000, 400);
            v.replace(Doc::parse(""), Some("Drop a file".into()));
            v.set_toc(true);
            assert!(v.toc_open());
            assert_eq!(v.toc_width(), 0);
            assert!(!v.toc_covers(10.0, 100.0));
            paint(&mut v, now);
        }

        /// The scrollbar, the URL preview, the toast and the notice live in the document's area.
        #[test]
        fn the_documents_extras_keep_right_of_the_panel() {
            let now = Instant::now();
            let mut v = open_view(100, 1000, 400);
            v.set_theme_mode(ThemeMode::Light);
            settle(&mut v);
            v.set_scroll(v.layout().top(30));
            v.ensure();
            // The scrollbar is at the window's right edge, and only there.
            let thumb = v.thumb_at(v.scroll()).unwrap();
            assert!(thumb.x1 <= 1000 && thumb.x0 > 260);
            // A hovered link's preview, a toast and a notice sit in the area right of the panel.
            let block = link_block(&v);
            let (x, y) = point_at(&v, block, "link");
            v.hover_at(x + 260.0, y, now);
            v.show_toast("A toast".into(), now);
            v.set_notice(Some("A notice".into()));
            let buf = paint(&mut v, now);
            let border = LIGHT.border;
            // In the panel's columns only its separator (at x = 259) has the border's color,
            // beside the (current row's) accent bar and nothing else of the boxes' outlines.
            for y in 32..400 {
                for x in 0..259 {
                    assert_ne!(buf[y * 1000 + x], border, "({x}, {y})");
                }
            }
            assert!(
                (300..400).any(|y| (261..500).any(|x| buf[y * 1000 + x] == border)),
                "the preview"
            );
            assert_eq!(v.panel_width(), 260);
        }

        #[test]
        fn the_copy_button_and_image_placeholders_follow_the_shifted_document() {
            let now = Instant::now();
            let src = "Intro.\n\n```\nfn main() {\n    let x = 1;\n    let y = 2;\n}\n```\n\n## After\n\nMore text here.\n";
            let mut v = view(src, 1000, 400);
            v.set_toc(true);
            settle(&mut v);
            paint(&mut v, now);
            #[cfg(feature = "clipboard")]
            {
                let b = v
                    .doc()
                    .blocks
                    .iter()
                    .position(|b| b.kind == BlockKind::Code)
                    .unwrap();
                let c = v.layout().geoms()[b].content.as_deref().unwrap();
                let r = c.rects.iter().find(|r| r.role == Role::CodeBg).unwrap();
                let (col_x, _) = v.layout().column();
                let inside = (
                    f64::from(260.0 + col_x + r.x + r.w / 2.0),
                    v.layout().top(b) + f64::from(r.y + r.h / 2.0),
                );
                let button = v.copy_button_at(inside.0, inside.1).expect("a button");
                assert!(button.x0 > 260.0, "the button is on the page: {button:?}");
                assert!(v.press(button.x0 as f64 + 3.0, button.y0 as f64 + 3.0, now));
                assert!(
                    v.release_copy(button.x0 as f64 + 3.0, button.y0 as f64 + 3.0)
                        .is_some()
                );
                // The same point without the shift is the panel's: no code block there.
                assert!(
                    v.copy_button_at(inside.0 - 260.0, inside.1).is_none()
                        || inside.0 - 260.0 >= 260.0
                );
            }
            #[cfg(feature = "remote-images")]
            {
                use crate::settings::RemoteImages;
                // Nothing listens on the discard port; the image is blocked anyway.
                let mut v = view("Text ![r](http://127.0.0.1:9/a.png) more.\n", 1000, 400);
                v.set_remote_policy(RemoteImages::Ask);
                v.set_toc(true);
                settle(&mut v);
                paint(&mut v, now);
                let hit = (0..1000)
                    .flat_map(|x| (32..80).map(move |y| (f64::from(x), f64::from(y))))
                    .find(|&(x, y)| v.image_at(x, y) == Some(1))
                    .expect("a placeholder on screen");
                assert!(hit.0 > 260.0, "the placeholder is on the page: {hit:?}");
                assert!(v.hover_at(hit.0, hit.1, now));
                assert_eq!(v.hover_note, Some(BLOCKED_NOTE));
                // Under the panel the document is out of reach, hit or not.
                assert!(v.hover_at(hit.0 - 260.0, hit.1, now));
                assert_eq!(v.hover_note, None);
            }
        }
    }
}
