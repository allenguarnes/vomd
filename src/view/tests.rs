use super::*;
// Only the tests that need the embedded fonts use it.
#[cfg(all(feature = "images", feature = "embed-fonts"))]
use crate::layout::{ImageSize, Refusal};
use winit::keyboard::{KeyCode, SmolStr};

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
        // Animations; Ctrl+A stays Select all (or nothing without selection support).
        #[cfg(feature = "images")]
        (ch("a"), none, Some(Action::ToggleAnimations)),
        #[cfg(feature = "images")]
        (ch("A"), shift, Some(Action::ToggleAnimations)),
        #[cfg(not(feature = "images"))]
        (ch("a"), none, None),
        #[cfg(feature = "clipboard")]
        (ch("a"), ctrl, Some(Action::SelectAll)),
        (ch("a"), ModifiersState::ALT, None),
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

/// The Mac's own shortcuts: ⌘R reloads, ⌘↑ and ⌘↓ go to the top and the bottom, ⌘, opens the
/// settings, ⌘M minimizes. Elsewhere they are nothing (Ctrl+R, Ctrl+M) or the plain keys' own
/// with a modifier that stops them (Ctrl+↑).
#[test]
fn mac_keys() {
    let cmd = ModifiersState::SUPER;
    let up = Key::Named(NamedKey::ArrowUp);
    let down = Key::Named(NamedKey::ArrowDown);
    if cfg!(target_os = "macos") {
        #[cfg(target_os = "macos")]
        for (key, want) in [
            (ch("r"), Action::Reload),
            (ch("R"), Action::Reload),
            (up.clone(), Action::Top),
            (down.clone(), Action::Bottom),
            (ch(","), Action::Settings),
            (ch("m"), Action::Minimize),
        ] {
            assert_eq!(key_action(&key, cmd), Some(want), "{key:?}");
        }
    } else {
        for key in [ch("r"), up, down, ch(","), ch("m")] {
            assert_eq!(key_action(&key, cmd), None, "{key:?}");
        }
    }
    // Plain, they keep their meaning.
    assert_eq!(
        key_action(&Key::Named(NamedKey::ArrowUp), ModifiersState::empty()),
        Some(Action::Lines(-1))
    );
}

/// A key held when the window gets focus arrives as a synthetic press, and on X11 and Windows
/// its auto-repeat as ordinary presses: none of them acts, or `Q` held down would close the next
/// window once the first is gone.
#[test]
fn held_keys_at_focus_do_not_act_until_released() {
    use ElementState::{Pressed, Released};
    let key = |c| PhysicalKey::Code(c);
    let (q, down) = (key(KeyCode::KeyQ), key(KeyCode::ArrowDown));
    let mut held = HeldKeys::default();
    // Q was down before the window had focus: the synthetic press, then the repeats.
    assert!(!held.is_user_press(q, Pressed, true));
    for _ in 0..3 {
        assert!(!held.is_user_press(q, Pressed, false));
    }
    // A key pressed in the window acts, and its repeats too.
    for _ in 0..3 {
        assert!(held.is_user_press(down, Pressed, false));
    }
    assert!(!held.is_user_press(down, Released, false));
    // Q comes up (a synthetic release counts too): the next press is the user's.
    assert!(!held.is_user_press(q, Released, false));
    assert!(held.is_user_press(q, Pressed, false));
    assert!(!held.is_user_press(q, Released, true));
    // A synthetic press then focus lost: forgotten, a later press acts.
    assert!(!held.is_user_press(q, Pressed, true));
    held.clear();
    assert!(held.is_user_press(q, Pressed, false));
    // Releases never act.
    assert!(!held.is_user_press(down, Released, false));
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
    #[cfg(feature = "images")]
    use crate::image::AnimState;
    #[cfg(feature = "images")]
    use crate::viewer::Button;
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

    /// A directory with the two images of a `<picture>` test: `l.png` (30 by 10) and `d.png`
    /// (20 by 40).
    fn picture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vomd-view-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, w, h) in [("l.png", 30, 10), ("d.png", 20, 40)] {
            let mut out = Vec::new();
            let mut enc = ::png::Encoder::new(&mut out, w, h);
            enc.set_color(::png::ColorType::Grayscale);
            enc.write_header()
                .unwrap()
                .write_image_data(&vec![0u8; (w * h) as usize])
                .unwrap();
            std::fs::write(dir.join(name), out).unwrap();
        }
        dir
    }

    const PICTURE: &str = "<picture>\n<source media=\"(prefers-color-scheme: dark)\" \
                           srcset=\"d.png\">\n<img src=\"l.png\" alt=\"Logo\">\n</picture>\n";

    /// A `<picture>` shows the image for the theme on screen, and the other one when the theme
    /// changes (the T key, the system's, the setting, a reload, another document): the layout
    /// sizes it from its own file.
    #[test]
    fn a_picture_follows_the_theme() {
        let now = Instant::now();
        let dir = picture_dir("picture");
        let src = PICTURE;
        let mut v = View::with_base(
            Fonts::new().unwrap(),
            Doc::parse(src),
            None,
            (400, 300),
            1.0,
            Some(dir.clone()),
        );
        v.ensure();
        let shown = |v: &View| (v.doc().images[0].src.to_string(), v.layout().image_size(1));
        #[cfg(feature = "images")]
        let size = |w, h| Some(ImageSize::Pixels(w, h));
        #[cfg(not(feature = "images"))]
        let size = |_, _| Some(ImageSize::Alt);
        let light = ("l.png".to_owned(), size(30, 10));
        let dark = ("d.png".to_owned(), size(20, 40));
        assert_eq!(shown(&v), light);
        assert!(v.set_system_dark(true));
        assert_eq!(shown(&v), dark, "the system's dark theme");
        v.apply(Action::CycleTheme, now);
        assert_eq!(shown(&v), light, "T: light");
        v.apply(Action::CycleTheme, now);
        assert_eq!(shown(&v), dark, "T: dark");
        v.set_theme_mode(ThemeMode::Light);
        assert_eq!(shown(&v), light, "the setting");
        v.set_theme_mode(ThemeMode::System);
        assert_eq!(shown(&v), dark);
        v.replace(Doc::parse(src), None);
        v.ensure();
        assert_eq!(shown(&v), dark, "a reload in the dark theme");
        assert_eq!(v.doc().text, "Logo");
        // Another document (a link, Back, the Open dialog) in the dark theme.
        v.open_in(Doc::parse(src), None, Some(dir.clone()));
        v.ensure();
        assert_eq!(shown(&v), dark, "open_in");
        v.set_theme_mode(ThemeMode::Light);
        assert_eq!(shown(&v), light);
        v.set_theme_mode(ThemeMode::Dark);
        v.set_theme_mode(ThemeMode::Light);
        v.open(Doc::parse(src), None);
        v.ensure();
        assert_eq!(shown(&v), light, "open: light");
        v.set_theme_mode(ThemeMode::Dark);
        v.open(Doc::parse(src), None);
        v.ensure();
        assert_eq!(shown(&v), dark, "open: dark");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// While the clipboard holds a large lazy copy of the document, a theme change can't swap
    /// the picture's image: it does when the copy is let go, without another theme change.
    #[cfg(all(feature = "clipboard", feature = "images"))]
    #[test]
    fn a_picture_waits_for_a_held_copy() {
        use crate::copy::Content;
        use crate::select::Pos;
        let now = Instant::now();
        let dir = picture_dir("held");
        let src = format!("{PICTURE}\n{}", "Line of text.\n\n".repeat(25_000));
        let mut v = View::with_base(
            Fonts::new().unwrap(),
            Doc::parse(&src),
            None,
            (400, 300),
            1.0,
            Some(dir.clone()),
        );
        v.ensure();
        let last = v.doc().blocks.len() as u32 - 1;
        let held = Content::of(&v.doc, (Pos::start(0), Pos::end(last)));
        assert!(matches!(held, Content::Lazy { .. }), "a large copy is lazy");
        let shown = |v: &View| (v.doc().images[0].src.to_string(), v.layout().image_size(1));
        let (light, dark) = (
            ("l.png".to_owned(), Some(ImageSize::Pixels(30, 10))),
            ("d.png".to_owned(), Some(ImageSize::Pixels(20, 40))),
        );
        v.set_theme_mode(ThemeMode::Dark);
        assert_eq!(shown(&v), light, "held: the old image stays");
        assert!(!v.tick(now, None).redraw);
        assert_eq!(shown(&v), light, "still held");
        drop(held);
        assert!(v.tick(now, None).redraw, "the image changed");
        assert_eq!(
            shown(&v),
            dark,
            "let go: the dark image, with no new theme change"
        );
        assert!(!v.theme_wait);
        // Nothing left to wait for.
        assert!(!v.tick(now, None).redraw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The window loads the math face for a document with formulas, and keeps what it laid
    /// out across a live reload and a resize: only a changed formula is laid out again.
    #[cfg(feature = "math")]
    #[test]
    fn formulas_are_laid_out_once_per_window() {
        let mut v = view("Plain text.\n", 600, 400);
        assert_eq!(v.fonts.math(), None, "no formula, no math face");
        let src = "A $x^2$ and\n\n$$\n\\frac{a}{b} \\tag{1}\n$$\n";
        v.replace(Doc::parse(src), None);
        settle(&mut v);
        assert!(v.fonts.math().is_some());
        let laid = v.math.borrow().laid;
        assert_eq!(laid, 2);
        v.replace(Doc::parse(src), None);
        v.resize(300, 500);
        settle(&mut v);
        assert_eq!(v.math.borrow().laid, laid);
        v.replace(Doc::parse(&src.replace("x^2", "x^3")), None);
        settle(&mut v);
        assert_eq!(v.math.borrow().laid, laid + 1);
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

    /// A live reload (a save in the editor beside the window) keeps the place being read, and
    /// at the very end of the document follows the end as the text grows; a document that fits
    /// the window stays at its top.
    #[test]
    fn a_reload_at_the_end_follows_the_end() {
        let mut v = view(&paras(100), 600, 400);
        settle(&mut v);
        v.set_scroll(v.layout().top(40));
        v.replace(Doc::parse(&paras(120)), None);
        assert_eq!(v.anchor().block, 40, "the place is kept");
        v.apply(Action::Bottom, Instant::now());
        for n in [130, 160, 90] {
            v.replace(Doc::parse(&paras(n)), None);
            settle(&mut v);
            assert!(
                (v.scroll() - v.max_scroll()).abs() < 0.5,
                "{n}: at the end ({} of {})",
                v.scroll(),
                v.max_scroll()
            );
            let range = v.layout().range_exact(v.scroll(), v.scroll() + 400.0);
            assert_eq!(range.end, n, "the last paragraph shows");
        }
        // Not quite at the end: the place, not the end.
        v.set_scroll(v.max_scroll() - 100.0);
        let place = v.anchor();
        v.replace(Doc::parse(&paras(100)), None);
        assert_eq!(v.anchor(), place);
        // A document that fits has no end to follow.
        let mut v = view(&paras(2), 600, 400);
        settle(&mut v);
        v.replace(Doc::parse(&paras(100)), None);
        assert_eq!(v.scroll(), 0.0);
    }

    /// While the scrollbar's thumb (or a selection) is dragged, a live reload waits: the shell
    /// asks [`View::holding`] and shows the new version when the button is up.
    #[test]
    fn dragging_holds_a_live_reload() {
        let now = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        assert!(!v.holding());
        let thumb = v.thumb_at(0.0).unwrap();
        assert!(v.press(f64::from(thumb.x1 - 1), f64::from(thumb.y0 + 2), now));
        assert!(v.holding(), "the thumb is dragged");
        v.release(now);
        assert!(!v.holding());
        #[cfg(feature = "clipboard")]
        {
            v.press(100.0, 100.0, now);
            assert!(!v.holding(), "a press is no drag yet");
            v.select_to(300.0, 200.0, now);
            assert!(v.holding(), "a selection is dragged");
            v.release_select();
            assert!(!v.holding());
        }
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

    /// A window that opens under the pointer, with the pointer in the scrollbar's column, shows
    /// the scrollbar as a scroll does: it fades on its own after the same delay, once, however
    /// often the pointer is looked at again without moving. Moving the pointer holds it.
    #[test]
    fn scrollbar_under_a_resting_pointer_fades_on_its_own() {
        let t0 = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        let x = f64::from(v.thumb_at(0.0).unwrap().x1 - 1);
        let mut buf = vec![0; 600 * 400];
        let thumb_px = |buf: &[u32]| (0..400).any(|y| buf[y * 600 + 595] != LIGHT.bg);
        // The first position seen (the window opened there): shown, with a deadline.
        assert!(v.hover_at(x, 300.0, t0), "appears");
        let deadline = v.deadline().expect("fades like after a scroll");
        v.apply(Action::Lines(0), t0);
        assert_eq!(v.deadline(), Some(deadline), "the same delay as a scroll");
        // Events that look at the pointer again, where it is, don't push the deadline back.
        assert!(!v.hover_at(x, 300.0, t0 + Duration::from_millis(100)));
        assert_eq!(v.deadline(), Some(deadline));
        v.paint(&mut buf, 600, t0);
        assert!(thumb_px(&buf));
        // The loop wakes at the deadline, hides it, and then has nothing to wake for.
        let tick = v.tick(t0, None);
        assert!(matches!(tick.wake, Wake::At(t) if t == deadline));
        assert!(v.expire(deadline));
        v.paint(&mut buf, 600, deadline);
        assert!(!thumb_px(&buf), "hidden without pointer input");
        assert!(!v.hover_at(x, 300.0, deadline));
        assert_eq!(
            v.deadline(),
            None,
            "not shown again while the pointer rests"
        );
        assert!(matches!(v.tick(deadline, None).wake, Wake::Event));
        // Moving the pointer in the column shows it and holds it.
        assert!(v.hover_at(x, 310.0, deadline), "appears");
        assert_eq!(v.deadline(), None, "held, no timer");
    }

    /// A pointer that comes back into the window somewhere else has moved: the bar holds. One that
    /// comes back at the same pixel (the window was covered and uncovered) is at rest: it fades.
    #[test]
    fn scrollbar_after_the_pointer_leaves_and_comes_back() {
        let t0 = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        let x = f64::from(v.thumb_at(0.0).unwrap().x1 - 1);
        assert!(v.hover_at(x, 300.0, t0));
        assert!(v.deadline().is_some(), "the window opened under it: fades");
        v.hover_end();
        v.hover_at(x, 300.0, t0);
        assert!(
            v.deadline().is_some(),
            "back at the same pixel: at rest, fades"
        );
        v.hover_end();
        v.hover_at(x, 320.0, t0);
        assert_eq!(v.deadline(), None, "back somewhere else: moved, held");
        // A view that never saw the pointer: its first sight is at rest.
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        v.hover_end();
        assert!(v.hover_at(x, 300.0, t0));
        assert!(
            v.deadline().is_some(),
            "first sight after hover_end: at rest"
        );
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

    /// A reload (or a live reload: the same `replace`) paints the local images whose files are
    /// unchanged in its first frame, from the pixels it had: none is decoded again and none shows
    /// as an empty box in between. An animated one stays animated.
    #[cfg(feature = "images")]
    #[test]
    fn a_reload_paints_unchanged_images_at_once() {
        let dir = std::env::temp_dir().join(format!("vomd-view-carry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Vec::new();
        let mut enc = ::png::Encoder::new(&mut png, 40, 20);
        enc.set_color(::png::ColorType::Rgb);
        enc.write_header()
            .unwrap()
            .write_image_data(&[0x11, 0x99, 0x33].repeat(800))
            .unwrap();
        std::fs::write(dir.join("a.png"), &png).unwrap();
        let gif = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/anim.gif");
        std::fs::copy(gif, dir.join("anim.gif")).unwrap();
        let mut v = view("![a](a.png)\n\n![g](anim.gif)", 600, 400);
        v.set_image_base(Some(dir.clone()));
        frame(&mut v, Instant::now());
        wait_images(&mut v, |v| {
            v.image_stats().decoded >= 2 && v.images.animated(2).is_some()
        });
        let decodes = v.image_stats().decodes;
        let mut buf = vec![0u32; 600 * 400];
        for edited in [
            "Edited.\n\n![a](a.png)\n\n![g](anim.gif)",
            "![a](a.png)\n\n![g](anim.gif)",
        ] {
            v.replace(Doc::parse(edited), None);
            v.paint(&mut buf, 600, Instant::now());
            assert_eq!(
                buf.iter().filter(|&&p| p == 0x119933).count(),
                40 * 20,
                "painted in the reload's first frame"
            );
            assert!(v.images.animated(2).is_some(), "still animated");
            v.tick(Instant::now(), Some(Instant::now()));
            v.poll_images(Instant::now());
        }
        assert_eq!(v.image_stats().decodes, decodes, "nothing decoded again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An animation in view plays: the loop wakes when its next frame is due, and the tick
    /// repaints. It waits (no timer) while the window can't be seen, is of no size, or doesn't
    /// show it (scrolled away), and goes on when it is shown again.
    #[cfg(feature = "images")]
    #[test]
    fn animations_play_only_while_the_window_shows_them() {
        let dir = std::env::temp_dir().join(format!("vomd-view-anim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gif = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/anim.gif");
        std::fs::copy(gif, dir.join("anim.gif")).unwrap();
        let mut v = view(&format!("![a](anim.gif)\n\n{}", paras(300)), 600, 400);
        v.set_image_base(Some(dir.clone()));
        v.images.set_animations(true);
        let due = |v: &mut View| {
            let t = Instant::now();
            loop {
                frame(v, Instant::now());
                v.poll_images(Instant::now());
                if let Some(due) = v.anim_deadline() {
                    return due;
                }
                assert!(t.elapsed() < Duration::from_secs(20), "never due");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let at = due(&mut v);
        assert!(v.animating());
        assert!(v.deadline().is_some_and(|d| d <= at));
        assert!(
            !v.tick(at, Some(at)).redraw,
            "only its box is painted again"
        );
        assert_eq!(v.damage().len(), 1, "its next frame is shown");
        // Hidden (minimized, covered), then shown again.
        due(&mut v);
        assert!(!v.set_hidden(true));
        assert_eq!(v.anim_deadline(), None);
        assert!(!v.animating());
        assert!(v.set_hidden(false), "shown again, it is painted again");
        assert!(v.anim_deadline().is_some());
        // Of no size.
        v.resize(0, 0);
        assert_eq!(v.anim_deadline(), None);
        v.resize(600, 400);
        due(&mut v);
        // Scrolled away: the frame that doesn't paint it leaves nothing due.
        v.scroll_by(1e9, Instant::now());
        frame(&mut v, Instant::now());
        assert_eq!(v.anim_deadline(), None);
        assert!(!v.animating());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An animated image shows the hand under the pointer, with no note, and a click opens it in
    /// the viewer (it no longer plays or pauses it): there `Space` plays or pauses it, and it is
    /// painted from its frames. A drag doesn't open it. In a link a click follows the link and
    /// Ctrl+click opens the image. `A` plays or pauses every one, and the toolbar has its button
    /// once the document has an animated image.
    #[cfg(feature = "images")]
    #[test]
    fn a_click_opens_an_animated_image_where_it_plays() {
        let dir = std::env::temp_dir().join(format!("vomd-view-play-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gif = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/anim.gif");
        std::fs::copy(&gif, dir.join("a.gif")).unwrap();
        std::fs::copy(&gif, dir.join("b.gif")).unwrap();
        let mut v = view(
            "<img src=\"a.gif\" width=120>\n\n[<img src=\"b.gif\" width=120>](https://e.x/)\n\nText.",
            600,
            400,
        );
        v.set_image_base(Some(dir.clone()));
        let now = Instant::now();
        assert!(!v.toolbar_ctx().animated);
        let t = Instant::now();
        while v.images.anim_state(1).is_none() || v.images.anim_state(2).is_none() {
            assert!(t.elapsed() < Duration::from_secs(20), "never decoded");
            frame(&mut v, Instant::now());
            v.poll_images(Instant::now());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(v.toolbar_ctx().animated && !v.toolbar_ctx().animations);
        let state = |v: &View, id| v.images.anim_state(id).unwrap();
        assert_eq!(state(&v, 1), AnimState::Paused);
        let at = |v: &View, id| {
            (0..600)
                .flat_map(|x| (0..300).map(move |y| (f64::from(x), f64::from(y))))
                .find(|&(x, y)| v.image_at(x, y) == Some(id))
                .map(|(x, y)| (x + 10.0, y + 10.0))
                .expect("on screen")
        };
        let (a, b) = (at(&v, 1), at(&v, 2));
        // Under the pointer: the hand, no note; in a link, the link's.
        v.hover(a.0, a.1);
        assert_eq!(v.hover_note, None);
        assert!(v.over_force() && v.hovered().is_none());
        v.hover(b.0, b.1);
        assert!(!v.over_force(), "hand");
        assert!(v.hovered().is_some(), "link");
        // A drag from it opens nothing.
        v.press(a.0, a.1, now);
        assert_eq!(v.click(a.0 + 300.0, a.1 + 100.0), None);
        assert!(!v.viewing());
        // A click opens it, as it was: paused. `Space` plays it there, and pauses it.
        v.press(a.0, a.1, now + Duration::from_secs(1));
        assert_eq!(v.click(a.0, a.1), None);
        assert!(v.viewing());
        assert_eq!(state(&v, 1), AnimState::Paused);
        let space = Key::Named(NamedKey::Space);
        let later = now + Duration::from_secs(5);
        let none = ModifiersState::empty();
        // Shift+Space steps back (here, nowhere to go), and doesn't play.
        v.viewer_key(&space, ModifiersState::SHIFT, later);
        assert_eq!(state(&v, 1), AnimState::Paused);
        assert!(v.viewing());
        assert_eq!(v.viewer_key(&space, none, later), Some(Outcome::Redraw));
        assert_eq!(state(&v, 1), AnimState::Playing);
        assert_eq!(state(&v, 2), AnimState::Paused);
        // The label says what each state is and what Space does.
        for (s, has) in [
            (AnimState::Playing, "pauses"),
            (AnimState::Paused, "plays"),
            (AnimState::Waiting, "plays now"),
            (AnimState::Over, "animation limits"),
            (AnimState::Failed, "tries again"),
        ] {
            assert!(super::viewer::anim_note(s).contains(has), "{s:?}");
        }
        // It plays in the viewer: frames come, each painting the window whole.
        let t = Instant::now();
        loop {
            frame(&mut v, Instant::now());
            v.poll_images(Instant::now());
            if let Some(due) = v.anim_deadline() {
                assert!(v.tick(due, Some(due)).redraw, "the viewer is painted whole");
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "never played");
            std::thread::sleep(Duration::from_millis(2));
        }
        v.viewer_key(&space, none, later);
        assert_eq!(state(&v, 1), AnimState::Paused);
        let esc = Key::Named(NamedKey::Escape);
        assert_eq!(v.viewer_key(&esc, none, later), Some(Outcome::Redraw));
        assert!(!v.viewing());
        // In a link: a click follows it, Ctrl+click opens the image.
        v.press(b.0, b.1, later);
        assert_eq!(v.click(b.0, b.1).as_deref(), Some("https://e.x/"));
        assert!(!v.viewing());
        v.set_ctrl(true);
        v.press(b.0, b.1, later + Duration::from_secs(1));
        assert_eq!(v.click(b.0, b.1), None, "not followed");
        v.set_ctrl(false);
        assert!(v.viewing() && v.viewer.as_ref().unwrap().id == 2);
        v.close_viewer();
        // `A`: all of them play, what was chosen for one forgotten; again, none.
        assert_eq!(v.apply(Action::ToggleAnimations, later), Outcome::Redraw);
        assert!(v.animations() && v.toolbar_ctx().animations);
        assert_eq!(
            (state(&v, 1), state(&v, 2)),
            (AnimState::Playing, AnimState::Playing)
        );
        v.apply(Action::ToggleAnimations, later);
        assert!(!v.animations());
        assert_eq!(
            (state(&v, 1), state(&v, 2)),
            (AnimState::Paused, AnimState::Paused)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PNG of `w` × `h`, RGB, its left half `a` and its right half `b`.
    #[cfg(feature = "images")]
    fn halves(w: u32, h: u32, a: [u8; 3], b: [u8; 3]) -> Vec<u8> {
        let mut png = Vec::new();
        let mut enc = ::png::Encoder::new(&mut png, w, h);
        enc.set_color(::png::ColorType::Rgb);
        let data: Vec<u8> = (0..h)
            .flat_map(|_| (0..w).flat_map(move |x| if x < w / 2 { a } else { b }))
            .collect();
        enc.write_header().unwrap().write_image_data(&data).unwrap();
        png
    }

    /// A view of `md` over a directory with `big.png` (1200 × 800), `other.png` (300 × 200),
    /// `tiny.png` (20 × 20) and `wide.png` (2400 × 400), its images decoded; and the directory.
    #[cfg(feature = "images")]
    fn image_view(name: &str, md: &str, w: u32, h: u32) -> (View, PathBuf) {
        let dir = std::env::temp_dir().join(format!("vomd-viewer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = |n: &str, w, h, a, b| std::fs::write(dir.join(n), halves(w, h, a, b)).unwrap();
        png("big.png", 1200, 800, [200, 0, 0], [0, 0, 200]);
        png("other.png", 300, 200, [0, 150, 0], [0, 150, 0]);
        png("tiny.png", 20, 20, [9, 9, 9], [9, 9, 9]);
        png("wide.png", 2400, 400, [50, 50, 50], [250, 250, 0]);
        let mut v = view(md, w, h);
        v.set_image_base(Some(dir.clone()));
        let n = v.doc().images.len() as u32;
        frame(&mut v, Instant::now());
        wait_images(&mut v, |v| {
            (1..=n).all(|id| v.images.shown(id) || v.layout.image_size(id) == Some(ImageSize::Alt))
        });
        (v, dir)
    }

    /// The middle of image `id`'s box on screen.
    #[cfg(feature = "images")]
    fn image_point(v: &View, id: u32) -> (f64, f64) {
        let (w, h) = v.size();
        let points: Vec<(f64, f64)> = (0..w)
            .step_by(2)
            .flat_map(|x| (0..h).step_by(2).map(move |y| (f64::from(x), f64::from(y))))
            .filter(|&(x, y)| v.image_at(x, y) == Some(id))
            .collect();
        assert!(!points.is_empty(), "image {id} on screen");
        let n = points.len() as f64;
        let sum = points
            .iter()
            .fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        (sum.0 / n, sum.1 / n)
    }

    /// Wait until the viewer's decode is back and nothing is asked for.
    #[cfg(feature = "images")]
    fn settle_viewer(v: &mut View) {
        let t = Instant::now();
        loop {
            let now = Instant::now();
            v.poll_images(now);
            if let Some(d) = v.viewer_deadline() {
                v.expire(d.max(now));
            }
            let vw = v.viewer.as_ref().unwrap();
            if vw.asked.is_none() && vw.settle_at.is_none() {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(30), "never settled");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Which images a click opens: one that shows its pixels, at least 48 × 48; not a tiny one,
    /// nor one beside the document. The hand shows over one, with no note.
    #[cfg(feature = "images")]
    #[test]
    fn a_click_opens_what_shows_its_pixels() {
        let md = "![big](big.png)\n\n![tiny](tiny.png)\n\n![none](none.png)\n\nText.";
        let (mut v, dir) = image_view("openable", md, 800, 1400);
        frame(&mut v, Instant::now());
        let big = image_point(&v, 1);
        assert_eq!(v.openable_at(big.0, big.1), Some(1));
        let tiny = image_point(&v, 2);
        assert_eq!(v.openable_at(tiny.0, tiny.1), None, "too small");
        assert_eq!(v.openable_at(-5.0, big.1), None);
        assert_eq!(v.openable_at(f64::NAN, f64::NAN), None);
        v.hover(big.0, big.1);
        assert!(v.over_force() && v.hover_note.is_none());
        v.hover(tiny.0, tiny.1);
        assert!(!v.over_force());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A click and stepping use one rule: the box the document asks for is at least 48 each
    /// way, however it is zoomed. A large image given a small box is neither clicked nor
    /// stepped to, a small one given a large box (or a percentage) both.
    #[cfg(feature = "images")]
    #[test]
    fn a_click_and_stepping_agree_on_which_images_count() {
        let md = "<img src=\"big.png\" width=30>\n\n<img src=\"tiny.png\" width=300>\n\n\
                  <img src=\"tiny.png\" width=\"50%\">\n\n![other](other.png)\n\n![t](tiny.png)";
        let (mut v, dir) = image_view("agree", md, 800, 1800);
        frame(&mut v, Instant::now());
        let now = Instant::now();
        for (id, opens) in [(1, false), (2, true), (3, true), (4, true), (5, false)] {
            let at = image_point(&v, id);
            assert_eq!(v.openable_at(at.0, at.1), opens.then_some(id), "click {id}");
        }
        assert!(v.open_image(4, now));
        assert_eq!(v.viewer.as_ref().unwrap().list, vec![2, 3, 4]);
        assert_eq!(v.viewer_place(), Some((3, 3)));
        v.close_viewer();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A middle click in the viewer doesn't reach the find bar's field under it (it would paste
    /// the primary selection into the query, and the search could move the document).
    #[cfg(all(feature = "images", feature = "find", feature = "clipboard"))]
    #[test]
    fn a_middle_click_in_the_viewer_leaves_the_find_bar_alone() {
        let (mut v, dir) = image_view("middle", "![big](big.png)\n\ntext", 600, 600);
        v.set_find(true);
        frame(&mut v, Instant::now());
        let field = (0..600)
            .step_by(4)
            .map(f64::from)
            .find(|&x| v.find_middle(x, 16.0).is_some())
            .expect("a point on the field");
        assert!(v.open_image(1, Instant::now()));
        assert_eq!(v.find_middle(field, 16.0), None);
        v.close_viewer();
        assert!(v.find_middle(field, 16.0).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pinch zooms the viewer's image by what the fingers did: e^(magnification).
    #[cfg(feature = "images")]
    #[test]
    fn a_pinch_zooms_the_viewers_image_by_the_magnification() {
        let (mut v, dir) = image_view("pinch", "![big](big.png)", 600, 600);
        let now = Instant::now();
        assert!(v.open_image(1, now));
        let s0 = v.viewer.as_ref().unwrap().tf.s;
        // A pinch of 0.05 (what the window reports), as the document counts it.
        v.zoom_wheel(0.05 * 10.0 * ZOOM_PIXELS, now);
        let s1 = v.viewer.as_ref().unwrap().tf.s;
        v.zoom_wheel(-0.05 * 10.0 * ZOOM_PIXELS, now);
        let s2 = v.viewer.as_ref().unwrap().tf.s;
        assert!(((s1 / s0) - 0.05f64.exp()).abs() < 1e-9, "{}", s1 / s0);
        assert!(s2 < s1);
        v.close_viewer();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A click opens the viewer and paints the image over the whole window, at Fit; `Esc`,
    /// `Backspace` and a click beside the image close it (never quitting), and the document is
    /// where it was: its position, its selection and the find bar.
    #[cfg(feature = "images")]
    #[test]
    fn the_viewer_opens_on_a_click_and_leaves_the_document_as_it_was() {
        let md = format!("{}![big](big.png)\n\n{}", paras(3), paras(80));
        let (mut v, dir) = image_view("open", &md, 600, 400);
        let now = Instant::now();
        v.scroll_by(30.0, now);
        frame(&mut v, now);
        #[cfg(feature = "clipboard")]
        assert_eq!(v.apply(Action::SelectAll, now), Outcome::Selected);
        #[cfg(feature = "find")]
        {
            v.set_find(true);
            v.find_type("Paragraph 2");
        }
        frame(&mut v, now);
        let (anchor, scroll) = (v.anchor(), v.scroll());
        #[cfg(feature = "clipboard")]
        let sel = v.selection();
        #[cfg(feature = "find")]
        let found = (v.find_query().to_string(), v.find.current);
        let at = image_point(&v, 1);
        let none = ModifiersState::empty();
        let close = |v: &mut View, key: Key| {
            assert_eq!(v.viewer_key(&key, none, now), Some(Outcome::Redraw));
            assert!(!v.viewing());
        };
        for (i, how) in ["esc", "backspace", "click"].into_iter().enumerate() {
            let t = now + Duration::from_secs(10 * i as u64 + 1);
            v.press(at.0, at.1, t);
            assert_eq!(v.click(at.0, at.1), None);
            assert!(v.viewing(), "{how}");
            // Painted over everything: the backdrop at the corners, the image in the middle.
            let mut buf = vec![0u32; 600 * 400];
            v.paint(&mut buf, 600, t);
            assert_eq!(buf[600 * 399], LIGHT.bg);
            let vw = v.viewer.as_ref().unwrap();
            assert!(vw.at_fit() && vw.label_shows(t));
            let (x, y, w, h) = vw.rect();
            let px = |x: i32, y: i32| buf[(y as usize) * 600 + x as usize];
            assert_eq!(px(x + w as i32 / 4, y + h as i32 / 2), 0xC80000);
            assert_eq!(px(x + w as i32 * 3 / 4, y + h as i32 / 2), 0x0000C8);
            settle_viewer(&mut v);
            match how {
                "esc" => close(&mut v, Key::Named(NamedKey::Escape)),
                "backspace" => close(&mut v, Key::Named(NamedKey::Backspace)),
                _ => {
                    let t = t + Duration::from_secs(2);
                    v.press(300.0, 395.0, t);
                    assert_eq!(v.click(300.0, 395.0), None);
                    assert!(!v.viewing());
                }
            }
            assert_eq!((v.anchor(), v.scroll()), (anchor, scroll), "{how}");
            #[cfg(feature = "clipboard")]
            assert_eq!(v.selection(), sel, "{how}");
            #[cfg(feature = "find")]
            {
                assert!(v.find_open());
                assert_eq!((v.find_query().to_string(), v.find.current), found);
            }
        }
        // Alt+← is Back: it closes too, and goes nowhere.
        v.press(at.0, at.1, now + Duration::from_secs(60));
        v.click(at.0, at.1);
        let back = v.viewer_key(&Key::Named(NamedKey::ArrowLeft), ModifiersState::ALT, now);
        assert_eq!(back, Some(Outcome::Redraw));
        assert!(!v.viewing());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A drag that starts on an image selects and opens nothing; Shift+click opens nothing; the
    /// second click of a double click on an image does nothing in the viewer; a small image
    /// (whose Fit is 100%) closes on a click, a large one switches between Fit and 100%.
    #[cfg(feature = "images")]
    #[test]
    fn drags_shift_clicks_and_double_clicks() {
        let md = "![big](big.png)\n\n![other](other.png)\n\nText after.";
        let (mut v, dir) = image_view("clicks", md, 800, 1200);
        frame(&mut v, Instant::now());
        let now = Instant::now();
        let big = image_point(&v, 1);
        v.press(big.0, big.1, now);
        #[cfg(feature = "clipboard")]
        assert!(v.select_to(big.0 + 200.0, big.1 + 300.0, now));
        assert_eq!(v.click(big.0 + 200.0, big.1 + 300.0), None);
        assert!(!v.viewing(), "a drag");
        #[cfg(feature = "clipboard")]
        {
            v.release_select();
            v.set_shift(true);
            v.press(big.0, big.1, now + Duration::from_secs(1));
            v.click(big.0, big.1);
            assert!(!v.viewing(), "Shift+click");
            v.set_shift(false);
        }
        // A double click: the first click opens, the second is ignored.
        let t = now + Duration::from_secs(5);
        v.press(big.0, big.1, t);
        v.click(big.0, big.1);
        assert!(v.viewing());
        let s = v.viewer.as_ref().unwrap().tf.s;
        v.press(big.0 + 1.0, big.1, t + Duration::from_millis(150));
        v.click(big.0 + 1.0, big.1);
        assert!(
            v.viewing() && v.viewer.as_ref().unwrap().tf.s == s,
            "ignored"
        );
        // A click on it: 100% about the pointer, then Fit.
        let mid = |v: &View| {
            let (x, y, w, h) = v.viewer.as_ref().unwrap().rect();
            (
                f64::from(x) + f64::from(w) / 2.0,
                f64::from(y) + f64::from(h) / 2.0,
            )
        };
        let m = mid(&v);
        v.press(m.0, m.1, t + Duration::from_secs(1));
        v.click(m.0, m.1);
        assert_eq!(v.viewer.as_ref().unwrap().tf.s, 1.0);
        // Dragged: it moves, and that is no click.
        let x = v.viewer.as_ref().unwrap().tf.x;
        v.press(400.0, 400.0, t + Duration::from_secs(2));
        assert!(v.viewer_drag(350.0, 400.0, t + Duration::from_secs(2)));
        assert_eq!(v.viewer_cursor(), Some(winit::window::CursorIcon::Grabbing));
        v.release(t);
        v.click(350.0, 400.0);
        let vw = v.viewer.as_ref().unwrap();
        assert!(vw.tf.x == x - 50.0 && vw.tf.s == 1.0);
        v.hover(350.0, 400.0);
        assert_eq!(v.viewer_cursor(), Some(winit::window::CursorIcon::Grab));
        v.press(400.0, 400.0, t + Duration::from_secs(3));
        v.click(400.0, 400.0);
        assert!(v.viewer.as_ref().unwrap().at_fit());
        v.close_viewer();
        assert_eq!(v.viewer_cursor(), None);
        // The small one: opened at 100%, a click on it closes.
        let other = image_point(&v, 2);
        v.press(other.0, other.1, t + Duration::from_secs(10));
        v.click(other.0, other.1);
        assert!(v.viewing());
        assert_eq!(v.viewer.as_ref().unwrap().tf.s, 1.0);
        let m = mid(&v);
        v.press(m.0, m.1, t + Duration::from_secs(11));
        v.click(m.0, m.1);
        assert!(!v.viewing());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Keys in the viewer: zoom, Fit ↔ 100%, moving the image, stepping through the document's
    /// images (not the tiny one, and no further at the ends), with the place in the label; the
    /// document's keys that mean nothing here do nothing, and Q still quits.
    #[cfg(feature = "images")]
    #[test]
    fn keys_zoom_move_and_step_through_the_images() {
        let md = "![big](big.png)\n\n![tiny](tiny.png)\n\n![wide](wide.png)\n\n\
                  ![other](other.png)\n\nText.";
        let (mut v, dir) = image_view("keys", md, 800, 700);
        frame(&mut v, Instant::now());
        let now = Instant::now();
        let at = image_point(&v, 1);
        v.press(at.0, at.1, now);
        v.click(at.0, at.1);
        let none = ModifiersState::empty();
        let key = |v: &mut View, k: Key, m: ModifiersState| v.viewer_key(&k, m, now);
        let s = |v: &View| v.viewer.as_ref().unwrap().tf.s;
        let fit = s(&v);
        key(&mut v, ch("+"), none);
        assert!((s(&v) - fit * 1.25).abs() < 1e-9);
        key(&mut v, ch("="), ModifiersState::CONTROL);
        key(&mut v, ch("-"), none);
        assert!((s(&v) - fit * 1.25).abs() < 1e-9);
        key(&mut v, ch("0"), none);
        assert_eq!(s(&v), fit, "Fit again");
        key(&mut v, ch("0"), ModifiersState::SUPER);
        assert_eq!(s(&v), 1.0, "100% with ⌘ (or Ctrl) too");
        let y = v.viewer.as_ref().unwrap().tf.y;
        key(&mut v, Key::Named(NamedKey::ArrowDown), none);
        assert_eq!(v.viewer.as_ref().unwrap().tf.y, y - 40.0);
        key(&mut v, ch("k"), none);
        assert_eq!(v.viewer.as_ref().unwrap().tf.y, y);
        // ← → move an image wider than the window.
        let x = v.viewer.as_ref().unwrap().tf.x;
        key(&mut v, Key::Named(NamedKey::ArrowRight), none);
        assert_eq!(v.viewer.as_ref().unwrap().tf.x, x - 40.0);
        // Nothing here: the contents panel, wide, find, select all, copy.
        for (k, m) in [
            (ch("c"), none),
            (ch("w"), none),
            (ch("b"), none),
            (ch("f"), ModifiersState::CONTROL),
            (ch("a"), ModifiersState::CONTROL),
            (ch("c"), ModifiersState::CONTROL),
            (ch("["), none),
        ] {
            assert_eq!(key(&mut v, k, m), Some(Outcome::Nothing));
        }
        assert!(v.viewing() && !v.toolbar_visible());
        // Stepping: at Fit, ← → step; PgDn, Space, End, Home too.
        key(&mut v, ch("0"), none);
        let id = |v: &View| v.viewer.as_ref().unwrap().id;
        let place = |v: &View| v.viewer_place();
        assert_eq!(place(&v), Some((1, 3)));
        key(&mut v, Key::Named(NamedKey::ArrowLeft), none);
        assert_eq!(id(&v), 1, "no further");
        key(&mut v, Key::Named(NamedKey::ArrowRight), none);
        assert_eq!(id(&v), 3, "the tiny one is skipped");
        assert_eq!(place(&v), Some((2, 3)));
        assert!(v.viewer.as_ref().unwrap().at_fit());
        key(&mut v, Key::Named(NamedKey::PageDown), none);
        assert_eq!(id(&v), 4);
        key(&mut v, Key::Named(NamedKey::Space), none);
        assert_eq!(id(&v), 4, "the last");
        key(&mut v, Key::Named(NamedKey::Space), ModifiersState::SHIFT);
        assert_eq!(id(&v), 3);
        key(&mut v, Key::Named(NamedKey::Home), none);
        assert_eq!(id(&v), 1);
        key(&mut v, Key::Named(NamedKey::End), none);
        assert_eq!(id(&v), 4);
        let label = v.viewer.as_ref().unwrap().label(now, place(&v), None);
        assert_eq!(label, "100% · 3 / 3");
        // The document's: Q quits, T switches the theme.
        assert_eq!(key(&mut v, ch("t"), none), Some(Outcome::Redraw));
        assert!(v.viewing());
        assert_eq!(key(&mut v, ch("q"), none), Some(Outcome::Quit));
        assert_eq!(
            key(&mut v, ch("w"), ModifiersState::CONTROL),
            Some(Outcome::Quit)
        );
        v.close_viewer();
        assert_eq!(
            key(&mut v, ch("q"), none),
            None,
            "closed: the document's keys"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ctrl + wheel (and a touchpad's zoom) zooms about the pointer; the wheel moves the image,
    /// across with Shift; the document doesn't scroll under the viewer.
    #[cfg(feature = "images")]
    #[test]
    fn the_wheel_moves_and_zooms_the_image() {
        let md = format!("![big](big.png)\n\n{}", paras(60));
        let (mut v, dir) = image_view("wheel", &md, 600, 400);
        frame(&mut v, Instant::now());
        let now = Instant::now();
        let at = image_point(&v, 1);
        v.press(at.0, at.1, now);
        v.click(at.0, at.1);
        v.hover(450.0, 300.0);
        let under = |v: &View| {
            let vw = v.viewer.as_ref().unwrap();
            ((450.0 - vw.tf.x) / vw.tf.s, (300.0 - vw.tf.y) / vw.tf.s)
        };
        let before = under(&v);
        assert!(v.viewer_wheel((0.0, 2.0), true, true, false, now));
        let after = under(&v);
        assert!((before.0 - after.0).abs() < 1e-6 && (before.1 - after.1).abs() < 1e-6);
        let fit = v.viewer.as_ref().unwrap().fit_zoom();
        assert!((v.viewer.as_ref().unwrap().tf.s - fit * 1.5625).abs() < 1e-9);
        v.zoom_wheel(100.0, now);
        assert!(v.viewer.as_ref().unwrap().tf.s > fit * 1.5625);
        let scroll = v.scroll();
        // Fit, then 100%, at the image's bottom right corner.
        v.viewer_key(&ch("0"), ModifiersState::empty(), now);
        v.viewer_key(&ch("0"), ModifiersState::empty(), now);
        assert_eq!(v.viewer.as_ref().unwrap().tf.s, 1.0);
        v.viewer_wheel((-1e9, -1e9), false, false, false, now);
        let tf = v.viewer.as_ref().unwrap().tf;
        assert!(v.viewer_wheel((0.0, 1.0), true, false, false, now));
        assert_eq!(v.viewer.as_ref().unwrap().tf.y, tf.y + 120.0);
        assert!(v.viewer_wheel((0.0, 30.0), false, false, true, now));
        assert_eq!(v.viewer.as_ref().unwrap().tf.x, tf.x + 30.0);
        v.wheel_by(500.0, now);
        v.wheel_lines(5.0, now);
        assert_eq!(v.scroll(), scroll);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The viewer decodes the image at the size the zoom needs once it settles, and then nothing
    /// runs: no deadline, the forced thread gone. Closed, what it held is gone.
    #[cfg(feature = "images")]
    #[test]
    fn the_viewer_settles_with_nothing_running() {
        let md = "![big](big.png)";
        let (mut v, dir) = image_view("settle", md, 600, 400);
        frame(&mut v, Instant::now());
        let now = Instant::now();
        let at = image_point(&v, 1);
        v.press(at.0, at.1, now);
        v.click(at.0, at.1);
        let asked = v.viewer.as_ref().unwrap().asked.expect("asked at once");
        assert_eq!(asked.size, v.viewer.as_ref().unwrap().target());
        settle_viewer(&mut v);
        let vw = v.viewer.as_ref().unwrap();
        let detail = vw.detail.as_ref().expect("decoded");
        assert_eq!((detail.width, detail.height), vw.target());
        assert!(vw.preview.is_none(), "the preview went");
        // 100%: the image's own size.
        v.viewer_key(&ch("0"), ModifiersState::empty(), now);
        settle_viewer(&mut v);
        let detail = v.viewer.as_ref().unwrap().detail.as_ref().unwrap();
        assert_eq!((detail.width, detail.height), (1200, 800));
        let mut buf = vec![0u32; 600 * 400];
        let t = Instant::now();
        while v.images.forcing() {
            assert!(t.elapsed() < Duration::from_secs(10), "the thread ends");
            std::thread::sleep(Duration::from_millis(2));
        }
        let later = Instant::now() + Duration::from_secs(5);
        v.expire(later);
        v.paint(&mut buf, 600, later);
        assert_eq!(v.deadline(), None, "no timer");
        assert!(!v.tick(later, Some(later)).redraw);
        v.close_viewer();
        assert!(v.viewer.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reload keeps the viewer on its image (by its file: its id may change), decoding it
    /// again if the file changed; an image gone keeps its pixels until the viewer closes. A
    /// theme that changes a `<picture>`'s image changes the viewer's.
    #[cfg(feature = "images")]
    #[test]
    fn a_reload_or_a_theme_keeps_the_viewer_on_its_image() {
        let md = "![big](big.png)\n\n![other](other.png)";
        let (mut v, dir) = image_view("reload", md, 600, 1000);
        frame(&mut v, Instant::now());
        let now = Instant::now();
        let at = image_point(&v, 2);
        v.press(at.0, at.1, now);
        v.click(at.0, at.1);
        settle_viewer(&mut v);
        // Moved down a place: the same file, the same pixels, nothing decoded again.
        v.replace(
            Doc::parse("![x](none.png)\n\n![big](big.png)\n\n![other](other.png)"),
            None,
        );
        let vw = v.viewer.as_ref().unwrap();
        assert_eq!(vw.id, 3);
        assert!(vw.detail.is_some() && vw.asked.is_none());
        // Its file changed (another size): decoded again, and fitted.
        std::fs::write(
            dir.join("other.png"),
            halves(250, 250, [1, 2, 3], [1, 2, 3]),
        )
        .unwrap();
        v.replace(Doc::parse("![other](other.png)"), None);
        let vw = v.viewer.as_ref().unwrap();
        assert_eq!((vw.id, vw.natural), (1, (250, 250)));
        assert!(vw.asked.is_some());
        settle_viewer(&mut v);
        let d = v.viewer.as_ref().unwrap().detail.as_ref().unwrap();
        assert_eq!((d.width, d.height, d.data[0]), (250, 250, 0xFF01_0203));
        // Gone: the viewer closes, and nothing it was asked afterwards shows another image.
        v.replace(Doc::parse("No images."), None);
        assert!(!v.viewing() && !v.viewer_waiting());
        v.close_viewer();
        // A `<picture>`: the theme's image.
        let pdir = picture_dir("viewer-picture");
        let src = "<picture>\n<source media=\"(prefers-color-scheme: dark)\" srcset=\"d.png\">\n\
                   <img src=\"l.png\" alt=\"Logo\" width=200>\n</picture>\n";
        let mut v = view(src, 600, 400);
        v.set_image_base(Some(pdir.clone()));
        frame(&mut v, Instant::now());
        wait_images(&mut v, |v| v.images.shown(1));
        frame(&mut v, Instant::now());
        // (Its images are small: a click wouldn't open it, so it is opened as the tests do.)
        assert!(v.open_image(1, now));
        assert_eq!(v.viewer.as_ref().unwrap().natural, (30, 10));
        v.set_theme_mode(ThemeMode::Dark);
        let vw = v.viewer.as_ref().unwrap();
        assert_eq!(vw.natural, (20, 40));
        assert!(vw.asked.is_some());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&pdir);
    }

    /// A reload that drops the image the viewer shows closes it: its id is another image's in the
    /// new document (the case the review found), so a zoom, a resize, `0`, `+` or Full resolution
    /// after it must not decode or show that one.
    #[cfg(feature = "images")]
    #[test]
    fn a_reload_that_drops_the_image_closes_the_viewer() {
        let md = "![big](big.png)\n\n![other](other.png)";
        let (mut v, dir) = image_view("reload-gone", md, 600, 1000);
        frame(&mut v, Instant::now());
        let now = Instant::now();
        let at = image_point(&v, 1);
        v.press(at.0, at.1, now);
        v.click(at.0, at.1);
        assert!(v.viewing());
        settle_viewer(&mut v);
        // A decode in flight (a zoom) when the reload comes.
        v.viewer_key(&ch("+"), ModifiersState::empty(), now);
        // The image is gone, and another one has its id.
        v.replace(Doc::parse("![other](other.png)"), None);
        assert!(!v.viewing(), "the viewer closes");
        assert!(!v.viewer_waiting());
        for key in ["0", "+", "f"] {
            v.viewer_key(&ch(key), ModifiersState::empty(), now);
        }
        v.resize(500, 700);
        frame(&mut v, Instant::now());
        assert!(!v.viewing() && !v.viewer_waiting());
        assert!(v.images.take_detail().is_none(), "nothing decoded for it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reload keeps the viewer on the copy it shows of an image the document has more than once
    /// (the nearest copy), not the first one.
    #[cfg(feature = "images")]
    #[test]
    fn a_reload_keeps_the_viewer_on_its_copy_of_a_repeated_image() {
        let md = "![a](big.png)\n\n![b](big.png)\n\n![c](big.png)";
        let (mut v, dir) = image_view("reload-copies", md, 600, 1000);
        frame(&mut v, Instant::now());
        wait_images(&mut v, |v| v.images.shown(3));
        frame(&mut v, Instant::now());
        let now = Instant::now();
        assert!(v.open_image(3, now));
        settle_viewer(&mut v);
        v.replace(Doc::parse(md), None);
        assert_eq!(v.viewer.as_ref().unwrap().id, 3, "the same copy");
        // One copy fewer before it: the nearest copy, the last one.
        v.replace(Doc::parse("![b](big.png)\n\n![c](big.png)"), None);
        assert_eq!(v.viewer.as_ref().unwrap().id, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Full resolution: offered only for an image with more pixels than the detail holds, and
    /// pressed, the part on screen is decoded at the image's own resolution (as decoding all of
    /// it at that size would make it); off again on close.
    #[cfg(feature = "images")]
    #[test]
    fn full_resolution_is_offered_for_large_images() {
        let md = "![big](big.png)\n\n![huge](huge.png)";
        let dir0 = std::env::temp_dir().join(format!("vomd-viewer-huge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir0);
        std::fs::create_dir_all(&dir0).unwrap();
        // 5000 × 3500 (17.5 MP): stripes of 7 pixels, so a part shows where it is from.
        let (w, h) = (5000u32, 3500u32);
        let mut png = Vec::new();
        let mut enc = ::png::Encoder::new(&mut png, w, h);
        enc.set_color(::png::ColorType::Grayscale);
        let data: Vec<u8> = (0..h)
            .flat_map(|_| (0..w).map(|x| (x / 7 % 2 * 255) as u8))
            .collect();
        enc.write_header().unwrap().write_image_data(&data).unwrap();
        let (mut v, dir) = image_view("full", md, 600, 1200);
        std::fs::write(dir.join("huge.png"), &png).unwrap();
        drop(png);
        v.replace(Doc::parse(md), None);
        frame(&mut v, Instant::now());
        wait_images(&mut v, |v| v.images.shown(2));
        frame(&mut v, Instant::now());
        let now = Instant::now();
        let mut buf = vec![0u32; 600 * 1200];
        let at = image_point(&v, 1);
        v.press(at.0, at.1, now);
        v.click(at.0, at.1);
        v.paint(&mut buf, 600, now);
        assert_eq!(v.viewer.as_ref().unwrap().buttons.len(), 1, "✕ alone");
        v.close_viewer();
        let at = image_point(&v, 2);
        v.press(at.0, at.1, now);
        v.click(at.0, at.1);
        v.paint(&mut buf, 600, now);
        let vw = v.viewer.as_ref().unwrap();
        let (b, r) = vw.buttons[1];
        assert_eq!(b, Button::Full);
        let p = (
            (f64::from(r.x0) + f64::from(r.x1)) / 2.0,
            (f64::from(r.y0) + f64::from(r.y1)) / 2.0,
        );
        v.hover(p.0, p.1);
        assert_eq!(v.viewer_cursor(), Some(winit::window::CursorIcon::Pointer));
        v.press(p.0, p.1, now + Duration::from_secs(1));
        v.click(p.0, p.1);
        assert!(v.viewer.as_ref().unwrap().full);
        // At 100%, once the detail is there, the part on screen at full resolution.
        v.viewer_key(&ch("0"), ModifiersState::empty(), now);
        settle_viewer(&mut v);
        let vw = v.viewer.as_ref().unwrap();
        let (part, crop) = vw.crop.clone().expect("the part");
        assert_eq!(crop.full, (5000, 3500));
        assert!(part.width <= 601 && part.height <= 1201);
        let whole = crate::image::decode(&std::fs::read(dir.join("huge.png")).unwrap(), 5000, 3500)
            .unwrap();
        for (y, row) in part.data.chunks(part.width as usize).enumerate() {
            let from = (crop.at.1 as usize + y) * 5000 + crop.at.0 as usize;
            assert_eq!(
                row,
                &whole.data[from..from + part.width as usize],
                "row {y}"
            );
        }
        drop(whole);
        // Moved, another part; the button again: off, the part goes.
        v.viewer_key(
            &Key::Named(NamedKey::ArrowRight),
            ModifiersState::empty(),
            now,
        );
        settle_viewer(&mut v);
        let x = v.viewer.as_ref().unwrap().crop.as_ref().unwrap().1.at.0;
        assert_ne!(x, crop.at.0);
        v.viewer_key(&ch("f"), ModifiersState::empty(), now);
        assert!(v.viewer.as_ref().unwrap().crop.is_none());
        v.viewer_key(&ch("f"), ModifiersState::empty(), now);
        assert!(v.viewer.as_ref().unwrap().full);
        v.close_viewer();
        v.press(at.0, at.1, now + Duration::from_secs(9));
        v.click(at.0, at.1);
        assert!(!v.viewer.as_ref().unwrap().full, "off again once closed");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir0);
    }

    /// A view of `anim.gif` (6 × 4) in a box `width` wide (filling most of the window), between
    /// headings and paragraphs, its animation on; and the directory it is in.
    #[cfg(feature = "images")]
    fn anim_view(w: u32, h: u32, width: u32) -> (View, std::path::PathBuf) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vomd-view-damage-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gif = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/anim.gif");
        std::fs::copy(gif, dir.join("anim.gif")).unwrap();
        let src = format!(
            "# Top\n\nSee [a link](https://example.com/x) here.\n\n\
             <img src=\"anim.gif\" width={width}>\n\n## Below\n\n{}",
            paras(40)
        );
        let mut v = view(&src, w, h);
        v.set_image_base(Some(dir.clone()));
        v.images.set_animations(true);
        (v, dir)
    }

    /// An animation that ends (play-once, its frames kept or streamed) or is refused for what it
    /// costs (over the limits) has its badge painted with no help: a window paints whole only
    /// when asked, and its boxes when an animation changed them, and what it shows is what a
    /// whole paint shows in that state.
    #[cfg(feature = "images")]
    #[test]
    fn an_animation_that_ends_or_is_refused_paints_its_badge_by_itself() {
        let dir = std::env::temp_dir().join(format!("vomd-view-badge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut once = Vec::new();
        {
            let mut e = ::gif::Encoder::new(&mut once, 400, 300, &[255, 0, 0, 0, 255, 0]).unwrap();
            for n in 0..6 {
                let frame = ::gif::Frame {
                    width: 400,
                    height: 300,
                    delay: 10,
                    buffer: std::borrow::Cow::Owned(vec![n % 2; 400 * 300]),
                    ..::gif::Frame::default()
                };
                e.write_frame(&frame).unwrap();
            }
        }
        std::fs::write(dir.join("once.gif"), once).unwrap();
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images");
        std::fs::copy(fixtures.join("big-anim.webp"), dir.join("over.webp")).unwrap();
        // (A box of 400: its frames are streamed; of 200: they are kept.)
        for (file, width, end) in [
            ("once.gif", 400, AnimState::Paused),
            ("once.gif", 200, AnimState::Paused),
            ("over.webp", 400, AnimState::Over),
        ] {
            let mut v = view(&format!("<img src=\"{file}\" width={width}>"), 480, 500);
            v.set_image_base(Some(dir.clone()));
            settle(&mut v);
            let mut buf = vec![0; 480 * 500];
            let t = Instant::now();
            while v.images.animated(1).is_none() {
                let now = Instant::now();
                v.paint(&mut buf, 480, now);
                v.request_images();
                v.poll_images(now);
                assert!(t.elapsed() < Duration::from_secs(10), "{file}: not decoded");
                std::thread::sleep(Duration::from_millis(1));
            }
            while v.poll_images(Instant::now()) {}
            v.images.set_animations(true);
            let mut now = Instant::now();
            v.images.set_anim_clock(Some(now));
            v.paint(&mut buf, 480, now);
            let t = Instant::now();
            while v.images.anim_state(1) != Some(end) {
                assert!(t.elapsed() < Duration::from_secs(10), "{file}: no end");
                let mut redraw = v.poll_images(now);
                if let Some(due) = v.anim_deadline() {
                    now = now.max(due);
                    v.images.set_anim_clock(Some(now));
                    redraw |= v.expire(now);
                }
                if redraw {
                    v.paint(&mut buf, 480, now);
                } else if !v.damage().is_empty() {
                    let rects = v.damage().to_vec();
                    v.paint_rects(&mut buf, 480, &rects, now);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            // What the wakes that follow ask for is painted too, and nothing else is.
            for _ in 0..20 {
                if v.poll_images(now) {
                    v.paint(&mut buf, 480, now);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut whole = vec![0; 480 * 500];
            v.paint(&mut whole, 480, now);
            let stale = buf.iter().zip(&whole).filter(|(a, b)| a != b).count();
            assert_eq!(stale, 0, "{file} at {width}: {end:?}: stale pixels");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An animation that waits beside another because their boxes are big is read again when a
    /// zoom out makes them small, and plays (the cost it waited with was for the old box).
    #[cfg(feature = "images")]
    #[test]
    fn a_waiting_animation_plays_once_a_zoom_out_makes_it_fit() {
        fn gif(w: u16, h: u16) -> Vec<u8> {
            let palette: Vec<u8> = (0..64u8)
                .flat_map(|i| [i * 4, 255 - i * 4, i * 2])
                .collect();
            let mut bytes = Vec::new();
            {
                let mut e = ::gif::Encoder::new(&mut bytes, w, h, &palette).unwrap();
                e.set_repeat(::gif::Repeat::Infinite).unwrap();
                for n in 0..10 {
                    let buffer = (0..usize::from(h))
                        .flat_map(|y| {
                            (0..usize::from(w)).map(move |x| ((x / 8 + y / 8 + n) % 64) as u8)
                        })
                        .collect();
                    let frame = ::gif::Frame {
                        width: w,
                        height: h,
                        delay: 10,
                        buffer: std::borrow::Cow::Owned(buffer),
                        ..Default::default()
                    };
                    e.write_frame(&frame).unwrap();
                }
            }
            bytes
        }
        let dir = std::env::temp_dir().join(format!("vomd-view-zoom-fit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.gif"), gif(900, 600)).unwrap();
        std::fs::write(dir.join("b.gif"), gif(800, 600)).unwrap();
        let mut v = view(
            "![above](a.gif)\n\n<img src=\"b.gif\" width=720>",
            2050,
            2560,
        );
        v.set_hidpi(2.0);
        v.set_wide(true);
        v.set_image_base(Some(dir.clone()));
        let mut buf = vec![0; 2050 * 2560];
        let mut run = |v: &mut View, time: Duration, until: &dyn Fn(&View) -> bool| {
            let t = Instant::now();
            while t.elapsed() < time && !until(v) {
                let now = Instant::now();
                v.paint(&mut buf, 2050, now);
                v.tick(now, Some(now));
                v.poll_images(now);
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        run(&mut v, Duration::from_secs(10), &|v| {
            v.images.animated(1).is_some() && v.images.animated(2).is_some()
        });
        v.set_animations(true);
        // (At 2× the boxes are 1800 × 1200 and 1440 × 1080: two don't fit beside each other.)
        run(&mut v, Duration::from_secs(10), &|v| {
            v.images.anim_state(2) == Some(AnimState::Waiting)
        });
        assert_eq!(v.images.anim_state(1), Some(AnimState::Playing));
        assert_eq!(v.images.anim_state(2), Some(AnimState::Waiting));
        v.set_zoom(50);
        run(&mut v, Duration::from_secs(10), &|v| {
            v.images.anim_state(2) == Some(AnimState::Playing)
        });
        let state = v.images.anim_state(2);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(state, Some(AnimState::Playing), "zoomed out, both fit");
    }

    /// One remote animation in two boxes of different sizes, waiting beside the one above it (at
    /// 2×, wide): the window settles, painting the changed boxes as the one above plays, not
    /// whole again and again (each size is measured once, and neither undoes the other's).
    #[cfg(all(feature = "images", feature = "remote-images"))]
    #[test]
    fn a_waiting_remote_animation_in_two_boxes_asks_for_no_stream_of_paints() {
        fn gif(w: u16, h: u16) -> Vec<u8> {
            let palette: Vec<u8> = (0..64u8)
                .flat_map(|i| [i * 4, 255 - i * 4, i * 2])
                .collect();
            let mut bytes = Vec::new();
            {
                let mut e = ::gif::Encoder::new(&mut bytes, w, h, &palette).unwrap();
                e.set_repeat(::gif::Repeat::Infinite).unwrap();
                for n in 0..10 {
                    let buffer = (0..usize::from(h))
                        .flat_map(|y| {
                            (0..usize::from(w)).map(move |x| ((x / 8 + y / 8 + n) % 64) as u8)
                        })
                        .collect();
                    let frame = ::gif::Frame {
                        width: w,
                        height: h,
                        delay: 10,
                        buffer: std::borrow::Cow::Owned(buffer),
                        ..Default::default()
                    };
                    e.write_frame(&frame).unwrap();
                }
            }
            bytes
        }
        let dir = std::env::temp_dir().join(format!("vomd-view-two-boxes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.gif"), gif(900, 600)).unwrap();
        let body = std::sync::Arc::new(gif(800, 600));
        let mut v = view(
            "![holder](a.gif)\n\n<img src=\"https://x.y/b.gif\" width=720>\n\n\
             <img src=\"https://x.y/b.gif\" width=360>",
            2050,
            3200,
        );
        v.set_hidpi(2.0);
        v.set_wide(true);
        v.set_image_base(Some(dir.clone()));
        v.images
            .remote
            .set_fetcher(std::sync::Arc::new(move |_, _| {
                Ok((body.clone(), 800, 600))
            }));
        v.set_remote_policy(crate::settings::RemoteImages::Always);
        v.set_remote(true);
        settle(&mut v);
        let mut buf = vec![0; 2050 * 3200];
        let t = Instant::now();
        while (1..=3).any(|id| v.images.animated(id).is_none()) {
            assert!(t.elapsed() < Duration::from_secs(10), "stills not loaded");
            let now = Instant::now();
            v.paint(&mut buf, 2050, now);
            v.request_images();
            v.poll_images(now);
            std::thread::sleep(Duration::from_millis(1));
        }
        while v.poll_images(Instant::now()) {}
        v.set_animations(true);
        v.paint(&mut buf, 2050, Instant::now());
        let (mut full, mut partial) = (0, 0);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            let now = Instant::now();
            let tick = v.tick(now, Some(now));
            if v.poll_images(now) || tick.redraw {
                full += 1;
                v.paint(&mut buf, 2050, now);
            } else if !v.damage().is_empty() {
                partial += 1;
                let rects = v.damage().to_vec();
                v.paint_rects(&mut buf, 2050, &rects, now);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let state = v.images.anim_state(2);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(full <= 20, "{full} full paints");
        assert!(
            partial >= 3,
            "the one above plays: {partial} partial paints"
        );
        assert_eq!(state, Some(AnimState::Waiting));
    }

    /// sol-6 M1 and M2, in a real 2× wide view: a remote 800×600, 90-frame, 8 MB GIF at
    /// 1440×1080 beside a 1800×1800 still, normal limits, its body gone before it was measured:
    /// fetching it doesn't fit beside the still, so it waits (it doesn't say it plays, with nothing
    /// to make it). A click fetches it past the limits; that failing, only the animation fails:
    /// its still stays, and a click asks again and plays it.
    #[cfg(all(feature = "images", feature = "remote-images"))]
    #[test]
    fn a_remote_animation_whose_body_cant_be_fetched_waits_and_a_failed_click_keeps_its_still() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        fn gif(w: u16, h: u16) -> Vec<u8> {
            let palette: Vec<u8> = (0..64u8)
                .flat_map(|i| [i * 4, 255 - i * 4, i * 2])
                .collect();
            let mut bytes = Vec::new();
            {
                let mut e = ::gif::Encoder::new(&mut bytes, w, h, &palette).unwrap();
                e.set_repeat(::gif::Repeat::Infinite).unwrap();
                for n in 0..90 {
                    let buffer = (0..usize::from(h))
                        .flat_map(|y| {
                            (0..usize::from(w)).map(move |x| ((x / 8 + y / 8 + n) % 64) as u8)
                        })
                        .collect();
                    let frame = ::gif::Frame {
                        width: w,
                        height: h,
                        delay: 10,
                        buffer: std::borrow::Cow::Owned(buffer),
                        ..Default::default()
                    };
                    e.write_frame(&frame).unwrap();
                }
            }
            bytes
        }
        let dir = std::env::temp_dir().join(format!("vomd-view-body-fails-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/alpha.png"),
            dir.join("still.png"),
        )
        .unwrap();
        let body = std::sync::Arc::new(gif(800, 600));
        let outside = body.to_vec();
        let mut v = view(
            "<img src=\"https://x.y/a.gif\" width=720>\n\n\
             <img src=\"still.png\" width=900 height=900>",
            2050,
            3200,
        );
        v.set_hidpi(2.0);
        v.set_wide(true);
        v.set_image_base(Some(dir.clone()));
        v.images
            .remote
            .set_fetcher(std::sync::Arc::new(move |_, _| {
                Ok((std::sync::Arc::new(body.to_vec()), 800, 600))
            }));
        let (attempts, fails) = (
            std::sync::Arc::new(AtomicUsize::new(0)),
            std::sync::Arc::new(AtomicBool::new(true)),
        );
        let (tries, fail) = (attempts.clone(), fails.clone());
        v.images.remote.forced_fetch = std::sync::Arc::new(move |_, _| {
            tries.fetch_add(1, Ordering::Relaxed);
            if fail.load(Ordering::Relaxed) {
                Err(Refusal::Failed)
            } else {
                Ok(outside.clone())
            }
        });
        v.set_remote_policy(crate::settings::RemoteImages::Always);
        v.set_remote(true);
        settle(&mut v);
        let mut buf = vec![0; 2050 * 3200];
        let t = Instant::now();
        while v.images.animated(1).is_none() || v.image_stats().decoded < 2 {
            assert!(t.elapsed() < Duration::from_secs(10), "stills not loaded");
            let now = Instant::now();
            v.paint(&mut buf, 2050, now);
            v.request_images();
            v.poll_images(now);
            std::thread::sleep(Duration::from_millis(2));
        }
        v.set_animations(true);
        // (How many frames it showed: partial paints.)
        let run = |v: &mut View, buf: &mut [u32], time: Duration| {
            let (t, mut partial) = (Instant::now(), 0);
            while t.elapsed() < time {
                let now = Instant::now();
                let tick = v.tick(now, Some(now));
                if v.poll_images(now) || tick.redraw {
                    v.paint(buf, 2050, now);
                    v.request_images();
                } else if !v.damage().is_empty() {
                    let rects = v.damage().to_vec();
                    v.paint_rects(buf, 2050, &rects, now);
                    partial += 1;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            partial
        };
        // (Paints and requests that keep it asked for: its body goes for the still's room.)
        let t = Instant::now();
        while v.images.anim_state(1) != Some(AnimState::Waiting) {
            assert!(t.elapsed() < Duration::from_secs(5), "it says it plays");
            let now = Instant::now();
            v.paint(&mut buf, 2050, now);
            v.request_images();
            v.poll_images(now);
            std::thread::sleep(Duration::from_millis(2));
        }
        run(&mut v, &mut buf, Duration::from_millis(500));
        assert_eq!(v.images.anim_state(1), Some(AnimState::Waiting));
        // (A click opens the viewer: what played the animation was its play.)
        let click = |v: &mut View| v.images.play(1);
        click(&mut v);
        run(&mut v, &mut buf, Duration::from_secs(1));
        // (Not fetched again by itself: a request made before a load's result is taken may ask
        // for it once more, and that is all.)
        let tried = attempts.load(Ordering::Relaxed);
        run(&mut v, &mut buf, Duration::from_millis(500));
        assert!(tried >= 1);
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            tried,
            "fetched again by itself"
        );
        assert_eq!(v.layout.image_size(1), Some(ImageSize::Pixels(800, 600)));
        assert_eq!(v.images.anim_state(1), Some(AnimState::Failed));
        assert_eq!(v.openable_at(100.0, 100.0), Some(1));
        fails.store(false, Ordering::Relaxed);
        click(&mut v);
        let partial = run(&mut v, &mut buf, Duration::from_secs(2));
        assert!(partial >= 3, "played again: {partial} frames");
        assert_eq!(v.images.anim_state(1), Some(AnimState::Playing));
        assert!(attempts.load(Ordering::Relaxed) > tried);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sol-7 M1, in a real 2× wide view: a looping remote 1300×1000, 40-frame GIF (9.9 MB),
    /// clicked to play past the limits, plays from its cached body; a local 800×600, ten-frame
    /// one and a still below it, all 1440×1080, come into view as the window grows. The body
    /// is held while the remote one plays: the local one has no room beside it, so it waits
    /// (no frame comes; it doesn't say it plays), and a click plays it past the limits too.
    #[cfg(all(feature = "images", feature = "remote-images"))]
    #[test]
    fn an_animation_with_no_room_beside_a_body_played_past_the_limits_waits() {
        fn gif(w: u16, h: u16, frames: usize) -> Vec<u8> {
            let palette: Vec<u8> = (0..64u8)
                .flat_map(|i| [i * 4, 255 - i * 4, i * 2])
                .collect();
            let mut bytes = Vec::new();
            {
                let mut e = ::gif::Encoder::new(&mut bytes, w, h, &palette).unwrap();
                e.set_repeat(::gif::Repeat::Infinite).unwrap();
                for n in 0..frames {
                    let buffer = (0..usize::from(h))
                        .flat_map(|y| {
                            (0..usize::from(w)).map(move |x| ((x / 8 + y / 8 + n) % 64) as u8)
                        })
                        .collect();
                    let frame = ::gif::Frame {
                        width: w,
                        height: h,
                        delay: 10,
                        buffer: std::borrow::Cow::Owned(buffer),
                        ..Default::default()
                    };
                    e.write_frame(&frame).unwrap();
                }
            }
            bytes
        }
        let dir =
            std::env::temp_dir().join(format!("vomd-view-forced-body-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("local.gif"), gif(800, 600, 10)).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/alpha.png"),
            dir.join("still.png"),
        )
        .unwrap();
        let body = gif(1300, 1000, 40);
        let mut v = view(
            "<img src=\"https://x.y/a.gif\" width=720 height=540>\n\n\
             <img src=\"local.gif\" width=720 height=540>\n\n\
             <img src=\"still.png\" width=720 height=540>",
            2050,
            1100,
        );
        v.set_hidpi(2.0);
        v.set_wide(true);
        v.set_image_base(Some(dir.clone()));
        v.images
            .remote
            .set_fetcher(std::sync::Arc::new(move |_, _| {
                Ok((std::sync::Arc::new(body.to_vec()), 1300, 1000))
            }));
        v.set_remote_policy(crate::settings::RemoteImages::Always);
        v.set_remote(true);
        v.set_animations(true);
        settle(&mut v);
        let mut buf = vec![0; 2050 * 1100];
        let t = Instant::now();
        while v.images.anim_state(1) != Some(AnimState::Over) {
            assert!(t.elapsed() < Duration::from_secs(10), "not over the limits");
            let now = Instant::now();
            v.paint(&mut buf, 2050, now);
            v.request_images();
            v.poll_images(now);
            std::thread::sleep(Duration::from_millis(2));
        }
        // (Played past the limits as `Space` plays it in the viewer.)
        let click = |v: &mut View, y: f64| {
            let id = v.image_at(100.0, y).unwrap();
            v.images.play(id);
        };
        // (The frames each showed: partial paints of the remote's box, and of the local's; for
        // at least `time`, then until `done` says so, as a slow machine shows its first frames
        // late.)
        let run =
            |v: &mut View, buf: &mut [u32], time: Duration, done: &dyn Fn(usize, usize) -> bool| {
                let (t, mut remote, mut local) = (Instant::now(), 0, 0);
                while t.elapsed() < time || (!done(remote, local) && t.elapsed() < time * 10) {
                    let now = Instant::now();
                    let tick = v.tick(now, Some(now));
                    if v.poll_images(now) || tick.redraw {
                        v.paint(buf, 2050, now);
                        v.request_images();
                    } else if !v.damage().is_empty() {
                        let rects = v.damage().to_vec();
                        remote += usize::from(rects.iter().any(|r| r.y0 < 1100));
                        local += usize::from(rects.iter().any(|r| r.y0 > 1100 && r.y0 < 2200));
                        v.paint_rects(buf, 2050, &rects, now);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                (remote, local)
            };
        click(&mut v, 100.0);
        let remote_plays = |remote: usize, _: usize| remote >= 3;
        assert!(run(&mut v, &mut buf, Duration::from_secs(1), &remote_plays).0 >= 3);
        v.resize(2050, 3500);
        buf.resize(2050 * 3500, 0);
        settle(&mut v);
        v.paint(&mut buf, 2050, Instant::now());
        v.request_images();
        let (remote, local) = run(&mut v, &mut buf, Duration::from_secs(3), &remote_plays);
        assert!(remote >= 3, "the remote one plays: {remote}");
        assert_eq!(local, 0);
        assert_eq!(v.images.anim_state(2), Some(AnimState::Waiting));
        click(&mut v, 1200.0);
        let both_play = |remote: usize, local: usize| remote >= 3 && local >= 3;
        let (remote, local) = run(&mut v, &mut buf, Duration::from_secs(2), &both_play);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(local >= 3, "the local one plays past the limits: {local}");
        assert!(remote >= 3, "the remote one plays on: {remote}");
        assert_eq!(v.images.anim_state(2), Some(AnimState::Playing));
    }

    /// Wait for the animation's next frame and show it (a tick at its time, which paints nothing
    /// whole): what of the window it changed.
    #[cfg(feature = "images")]
    fn next_damage(v: &mut View) -> Vec<Clip> {
        let t = Instant::now();
        loop {
            v.poll_images(Instant::now());
            if let Some(at) = v.anim_deadline() {
                assert!(!v.tick(at, Some(at)).redraw, "only the animation changed");
                if !v.damage().is_empty() {
                    return v.damage().to_vec();
                }
            }
            assert!(t.elapsed() < Duration::from_secs(20), "no next frame");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A paint that can't be made (a minimized window has no size) drops what the animations
    /// changed: the loop of the event loop asks to paint it for as long as it is there.
    #[cfg(feature = "images")]
    #[test]
    fn a_paint_that_cannot_be_made_drops_the_damage() {
        let (mut v, dir) = anim_view(640, 400, 600);
        let t = Instant::now();
        while v.anim_deadline().is_none() {
            assert!(t.elapsed() < Duration::from_secs(20), "never plays");
            frame(&mut v, Instant::now());
            v.poll_images(Instant::now());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!next_damage(&mut v).is_empty());
        v.discard_damage();
        assert!(v.damage().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A frame painted only where the animation changed, over the frame before it (or over the
    /// one before that, with what changed in between: Wayland's two buffers), is the frame
    /// painted whole, whatever is over the image.
    #[cfg(feature = "images")]
    fn paints_in_part_as_whole(v: &mut View, what: &str) {
        let (w, h) = v.size();
        let whole = |v: &mut View| {
            let mut buf = vec![0u32; (w * h) as usize];
            v.paint(&mut buf, w as usize, Instant::now());
            buf
        };
        let differ = |a: &[u32], b: &[u32]| a.iter().zip(b).filter(|(a, b)| a != b).count();
        // Until it plays: its first frame painted, its next made.
        let t = Instant::now();
        while v.anim_deadline().is_none() {
            assert!(t.elapsed() < Duration::from_secs(20), "{what}: never plays");
            frame(v, Instant::now());
            v.poll_images(Instant::now());
            std::thread::sleep(Duration::from_millis(2));
        }
        // The buffer holds the frame before (X11, Windows).
        let mut a = whole(v);
        let changed = next_damage(v);
        let rects = render::damage(&changed, 1, None).unwrap();
        let before = a.clone();
        v.paint_rects(&mut a, w as usize, &rects, Instant::now());
        assert!(v.damage().is_empty());
        assert!(differ(&a, &before) > 0, "{what}: a frame of its own");
        let b = whole(v);
        assert_eq!(
            differ(&a, &b),
            0,
            "{what}: the frame before, painted in part"
        );
        // Two buffers: the one before last missed the last frame's change too.
        let mut y = whole(v);
        let c1 = next_damage(v);
        assert_eq!(
            render::damage(&c1, 2, None),
            None,
            "{what}: the last change unknown"
        );
        whole(v);
        let c2 = next_damage(v);
        let rects = render::damage(&c2, 2, Some(&c1)).unwrap();
        v.paint_rects(&mut y, w as usize, &rects, Instant::now());
        let b = whole(v);
        assert_eq!(
            differ(&y, &b),
            0,
            "{what}: two frames before, painted in part"
        );
    }

    /// An animation's tick paints only its box, under and over everything else as a whole
    /// paint has it: the selection's wash, find's marks, the contents panel, the scrollbar,
    /// the link preview and the toolbar's dropdown.
    #[cfg(feature = "images")]
    #[test]
    fn an_animation_tick_paints_its_box_as_a_whole_paint_does() {
        let now = Instant::now();
        type Setup = fn(&mut View);
        let cases: [(&str, Setup); 7] = [
            ("plain", |_| {}),
            ("selection", |v| {
                #[cfg(feature = "clipboard")]
                assert_eq!(
                    v.apply(Action::SelectAll, Instant::now()),
                    Outcome::Selected
                );
                let _ = v;
            }),
            ("find", |v| {
                #[cfg(feature = "find")]
                {
                    v.apply(Action::Find, Instant::now());
                    v.find_type("e");
                }
                let _ = v;
            }),
            ("contents", |v| {
                #[cfg(feature = "toc")]
                v.set_toc(true);
                let _ = v;
            }),
            ("scrollbar", |v| {
                v.scroll_by(30.0, Instant::now());
                v.fade.poke(Instant::now() + Duration::from_secs(3600));
            }),
            ("preview", |v| {
                v.hover_note = Some("https://example.com/a/long/way/down")
            }),
            ("toolbar", |v| v.set_toolbar(true)),
        ];
        for (what, setup) in cases {
            let (mut v, dir) = anim_view(600, 400, 560);
            setup(&mut v);
            paints_in_part_as_whole(&mut v, what);
            let _ = std::fs::remove_dir_all(&dir);
        }
        // Its frames made at the GIF's size, enlarged a whole 100 times by the paint.
        let (mut v, dir) = anim_view(700, 500, 600);
        paints_in_part_as_whole(&mut v, "whole");
        let _ = std::fs::remove_dir_all(&dir);
        // The dropdown hangs over the image in a narrow window.
        let (mut v, dir) = anim_view(200, 300, 190);
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
        paints_in_part_as_whole(&mut v, "dropdown");
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

        /// A remote image that only a theme change brings (a `<picture>`'s dark image) is
        /// loaded by `always`, and announced under `ask`, though the document had none to
        /// announce at its first frame.
        #[test]
        fn a_remote_image_a_theme_brings_is_announced() {
            const SRC: &str = "<picture>\n<source media=\"(prefers-color-scheme: dark)\" \
                               srcset=\"https://127.0.0.1:9/d.png\">\n\
                               <img src=\"l.png\" alt=\"L\">\n</picture>\n";
            let now = Instant::now();
            let mut v = view(SRC, 600, 400);
            v.set_remote_policy(RemoteImages::Always);
            v.tick(now, Some(now));
            assert!(!v.remote_on() && v.remote_count == 0);
            v.set_theme_mode(ThemeMode::Dark);
            assert_eq!(v.remote_count, 1);
            v.tick(now, Some(now));
            assert!(v.remote_on(), "always: loaded");
            // ask, no toolbar: the toast.
            let mut v = view(SRC, 600, 400);
            v.tick(now, Some(now));
            assert!(v.toast.is_none());
            v.set_theme_mode(ThemeMode::Dark);
            v.tick(now, Some(now));
            let (_, text) = v.toast.clone().expect("announced");
            assert_eq!(text, "1 remote image blocked · press I to load");
            assert!(!v.remote_on());
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
                    .position(|b| matches!(b.kind, BlockKind::Code(_)))
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
