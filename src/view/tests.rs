use super::*;
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
        (Key::Named(NamedKey::End), none, Some(Action::Bottom)),
        (ch("r"), none, Some(Action::Reload)),
        (Key::Named(NamedKey::F5), none, Some(Action::Reload)),
        (ch("t"), none, Some(Action::CycleTheme)),
        (ch("q"), none, Some(Action::Quit)),
        (Key::Named(NamedKey::Escape), none, Some(Action::Quit)),
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
    let err = read_input(&Input::File("/nonexistent/vomd-test.md".into())).unwrap_err();
    assert!(
        err.starts_with("Cannot open /nonexistent/vomd-test.md"),
        "{err}"
    );
    let dir = std::env::temp_dir().join(format!("vomd-view-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.md");
    std::fs::write(&path, b"ok \xff end").unwrap();
    let (text, st) = read_input(&Input::File(path.clone())).unwrap();
    assert_eq!(text, "ok \u{fffd} end");
    assert_eq!(st, stamp(&path));
    assert_eq!(st.unwrap().1, 8);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(read_input(&Input::None).unwrap(), (String::new(), None));
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

    #[test]
    fn scrollbar_drag() {
        let now = Instant::now();
        let mut v = view(&paras(200), 600, 400);
        settle(&mut v);
        assert!(
            !v.press(100.0, 100.0, now),
            "presses in the text aren't the scrollbar's"
        );
        assert!(v.press(595.0, 5.0, now), "grab the thumb at the top");
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
        // A press on the track centers the thumb there.
        assert!(v.press(595.0, 200.0, now));
        let mid = v.scroll() / v.max_scroll();
        assert!((mid - 0.5).abs() < 0.02, "{mid}");
        v.release(now);
        // Nothing to drag when everything fits.
        let mut short = view("short", 600, 400);
        assert!(!short.press(595.0, 5.0, now));
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
}
