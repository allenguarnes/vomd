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
}
