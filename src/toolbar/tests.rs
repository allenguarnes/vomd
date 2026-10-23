use super::*;

/// The contents button's width in the left group (none without the feature).
const TOC: f32 = if cfg!(feature = "toc") { BUTTON } else { 0.0 };
/// The find button's, in the right group.
const FIND: f32 = if cfg!(feature = "find") { BUTTON } else { 0.0 };

fn ctx(width: u32) -> Ctx {
    Ctx {
        width,
        height: 600,
        scale: 1.0,
        can_back: true,
        zoom: 100,
        theme: ThemeMode::System,
        wide: false,
        #[cfg(feature = "toc")]
        toc: false,
        #[cfg(feature = "find")]
        find: false,
        settings: true,
        remote: 0,
        remote_on: false,
        no_curl: false,
        pulse: false,
        animated: false,
        animations: false,
    }
}

fn items(s: &Slots) -> Vec<Item> {
    s.items.iter().map(|(i, _)| *i).collect()
}

/// The dropdown's rows, scrolled by `scroll`.
fn menu_rows(c: &Ctx, s: &Slots, scroll: f32) -> Vec<(Item, Rect)> {
    let b = menu_box(c, s).unwrap();
    (0..s.overflow.len())
        .map(|n| (s.overflow[n], menu_row(c, b, scroll, n)))
        .collect()
}

fn center(s: &Slots, item: Item) -> (f64, f64) {
    let r = s.items.iter().find(|(i, _)| *i == item).unwrap().1;
    (
        f64::from((r.x0 + r.x1) / 2.0),
        f64::from((r.y0 + r.y1) / 2.0),
    )
}

#[test]
fn wide_bar_shows_everything() {
    let s = slots(&ctx(900));
    assert_eq!(
        items(&s),
        [
            Item::Back,
            Item::Open,
            #[cfg(feature = "toc")]
            Item::Contents,
            Item::Settings,
            #[cfg(feature = "find")]
            Item::Find,
            Item::Reload,
            Item::Theme,
            Item::Wide,
            Item::ZoomIn,
            Item::ZoomReset,
            Item::ZoomOut
        ]
    );
    assert!(s.overflow.is_empty());
    // Right group flush with the right edge, left to right: − % + wide theme reload find settings.
    let r = |i| s.items.iter().find(|(x, _)| *x == i).unwrap().1;
    assert_eq!(r(Item::Settings).x1, 900.0 - EDGE);
    // Open right after Back.
    assert_eq!(r(Item::Open).x0, r(Item::Back).x1);
    assert_eq!(r(Item::Open).x1, EDGE + LEFT);
    #[cfg(feature = "toc")]
    {
        // The contents button follows Open.
        assert_eq!(r(Item::Contents).x0, r(Item::Open).x1);
        assert_eq!(r(Item::Contents).x1, EDGE + LEFT + BUTTON);
    }
    assert!(r(Item::ZoomOut).x1 <= r(Item::ZoomReset).x0 + 0.01);
    assert!(r(Item::ZoomIn).x1 <= r(Item::Wide).x0 + 0.01);
    assert!(r(Item::Wide).x1 <= r(Item::Theme).x0 + 0.01);
    // No settings file: no ⚙.
    let s = slots(&Ctx {
        settings: false,
        ..ctx(900)
    });
    assert!(!items(&s).contains(&Item::Settings));
}

#[test]
fn narrow_bar_collapses_zoom_first_then_wide_then_theme() {
    let full =
        (EDGE + LEFT + TOC + GAP + 2.0 * BUTTON + PERCENT + 4.0 * BUTTON + FIND + EDGE) as u32;
    assert!(slots(&ctx(full)).overflow.is_empty(), "exactly fits");
    let s = slots(&ctx(full - 1));
    assert_eq!(s.overflow, [Item::ZoomOut, Item::ZoomReset, Item::ZoomIn]);
    assert!(items(&s).contains(&Item::More) && items(&s).contains(&Item::Theme));
    let s = slots(&ctx(182 + (TOC + FIND) as u32));
    assert_eq!(
        s.overflow,
        [
            Item::ZoomOut,
            Item::ZoomReset,
            Item::ZoomIn,
            Item::Wide,
            Item::Theme
        ]
    );
    // Wide layout goes before the theme: with room for the theme, reload, find and ⚙ only, it's
    // in the dropdown.
    let s = slots(&ctx(232 + (TOC + FIND) as u32));
    assert_eq!(s.overflow.last(), Some(&Item::Wide));
    assert!(items(&s).contains(&Item::Theme));
    // Down to 320 px and below, everything stays reachable: shown or in the menu.
    for w in [320, 200, 120, 80, 40, 1] {
        let s = slots(&ctx(w));
        let mut all: Vec<Item> = items(&s);
        all.extend(&s.overflow);
        for item in [
            Item::Back,
            Item::Open,
            #[cfg(feature = "toc")]
            Item::Contents,
            Item::ZoomOut,
            Item::ZoomReset,
            Item::ZoomIn,
            Item::Wide,
            Item::Theme,
            Item::Reload,
            #[cfg(feature = "find")]
            Item::Find,
            Item::Settings,
        ] {
            assert!(all.contains(&item), "{item:?} at {w}");
        }
        // Shown items never overlap.
        let mut xs: Vec<(f32, f32)> = s.items.iter().map(|(_, r)| (r.x0, r.x1)).collect();
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(
            xs.windows(2).all(|p| p[0].1 <= p[1].0 + 0.01),
            "{w}: {xs:?}"
        );
    }
}

#[test]
fn hidpi_scales_the_bar() {
    let mut c = ctx(1800);
    c.scale = 2.0;
    let s = slots(&c);
    assert!(s.overflow.is_empty());
    assert_eq!(s.items[0].1.y1, 64.0);
    assert_eq!(Toolbar::default().height(2.0), 64);
    assert_eq!(Toolbar::default().height(1.25), 40);
}

#[test]
fn clicks_map_to_the_shortcuts_actions() {
    let c = ctx(900);
    let s = slots(&c);
    let mut t = Toolbar::default();
    for (item, want) in [
        (Item::Back, Command::Action(Action::Back)),
        (Item::Open, Command::Action(Action::Open)),
        #[cfg(feature = "toc")]
        (Item::Contents, Command::Action(Action::ToggleToc)),
        (Item::ZoomOut, Command::Action(Action::ZoomOut)),
        (Item::ZoomReset, Command::Action(Action::ZoomReset)),
        (Item::ZoomIn, Command::Action(Action::ZoomIn)),
        (Item::Wide, Command::Action(Action::ToggleWide)),
        (Item::Theme, Command::Action(Action::CycleTheme)),
        (Item::Reload, Command::Action(Action::Reload)),
        #[cfg(feature = "find")]
        (Item::Find, Command::Action(Action::Find)),
        (Item::Settings, Command::Settings),
    ] {
        let (x, y) = center(&s, item);
        assert!(t.press(&c, x, y));
        assert_eq!(t.release(&c, x, y), (true, Some(want)), "{item:?}");
    }
    // Released elsewhere: no click.
    let (x, y) = center(&s, Item::Reload);
    t.press(&c, x, y);
    assert_eq!(t.release(&c, 10.0, 300.0), (true, None));
    // Back is disabled with an empty history.
    let off = Ctx {
        can_back: false,
        ..c
    };
    let (x, y) = center(&s, Item::Back);
    t.press(&off, x, y);
    assert_eq!(t.release(&off, x, y), (true, None));
    // Off the bar, the document gets the press.
    assert!(!t.press(&c, 400.0, 300.0));
    assert_eq!(t.release(&c, 400.0, 300.0), (false, None));
}

#[test]
fn dropdown_opens_selects_and_closes() {
    let c = ctx(200);
    let s = slots(&c);
    assert!(!s.overflow.is_empty());
    let mut t = Toolbar::default();
    let (mx, my) = center(&s, Item::More);
    t.press(&c, mx, my);
    assert_eq!(t.release(&c, mx, my), (true, None));
    assert!(t.menu_open(&c));
    let rows = menu_rows(&c, &s, 0.0);
    assert_eq!(rows.len(), s.overflow.len());
    assert!(
        rows.iter().all(|(_, r)| r.x0 >= 0.0 && r.x1 <= 200.0),
        "inside the window"
    );
    // The open menu covers what's under it.
    let (item, r) = rows[0];
    let (rx, ry) = (f64::from(r.x0 + 5.0), f64::from((r.y0 + r.y1) / 2.0));
    assert!(t.covers(&c, rx, ry) && t.menu_covers(&c, rx, ry));
    t.press(&c, rx, ry);
    assert_eq!(t.release(&c, rx, ry), (true, item.command()));
    assert!(!t.menu_open(&c), "closes on selection");
    // An outside press closes it and is swallowed.
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    assert!(t.menu_open(&c));
    // The bar is not the dropdown: only the box below it is.
    assert!(t.covers(&c, 5.0, 5.0) && !t.menu_covers(&c, 5.0, 5.0));
    assert!(!t.menu_covers(&c, 100.0, 500.0));
    assert!(t.press(&c, 100.0, 500.0));
    assert!(!t.menu_open(&c));
    assert_eq!(t.release(&c, 100.0, 500.0), (false, None));
    // Escape-style close.
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    assert!(t.close_menu(&c));
    assert!(!t.close_menu(&c));
}

/// Nothing outside the window is hit: a drag can end there, and rows or buttons the window's
/// edge cuts off are only hit inside it.
#[test]
fn nothing_outside_the_window_is_hit() {
    let c = Ctx {
        height: 100,
        ..ctx(200)
    };
    let s = slots(&c);
    let mut t = Toolbar::default();
    let (mx, my) = center(&s, Item::More);
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    let rows = menu_rows(&c, &s, 0.0);
    let (item, r) = rows[2];
    assert_eq!(item, Item::ZoomIn);
    assert!(r.y1 > 100.0, "the row runs past the window");
    let x = f64::from(r.x0 + 5.0);
    assert!(t.press(&c, x, 98.0));
    assert_eq!(t.release(&c, x, 110.0), (true, None), "released below");
    assert!(t.menu_open(&c), "still open: no click happened");
    // The same click that ends inside the window acts.
    t.press(&c, x, 98.0);
    assert_eq!(t.release(&c, x, 99.0), (true, item.command()));
    // A bar button cut off by a very short window.
    let c = Ctx {
        height: 20,
        ..ctx(900)
    };
    let (bx, _) = center(&slots(&c), Item::Back);
    t.press(&c, bx, 19.0);
    assert_eq!(t.release(&c, bx, 25.0), (true, None));
    t.press(&c, bx, 19.0);
    assert_eq!(t.release(&c, bx, 19.5), (true, Item::Back.command()));
}

/// Every row of a dropdown the window cuts short is reachable by scrolling it, and the
/// scrolling stops at both ends.
#[test]
fn short_window_scrolls_the_dropdown() {
    let c = Ctx {
        height: 100,
        ..ctx(182 + (TOC + FIND) as u32)
    };
    let s = slots(&c);
    assert_eq!(
        s.overflow.len(),
        5,
        "zoom, wide and theme are in the dropdown"
    );
    let mut t = Toolbar::default();
    assert!(!t.scroll_menu(&c, 10.0), "closed: the document's");
    let (mx, my) = center(&s, Item::More);
    for (n, &item) in s.overflow.iter().enumerate() {
        t.press(&c, mx, my);
        t.release(&c, mx, my);
        assert!(t.menu_open(&c));
        // Row `n` to the top, as far as the end of the list allows.
        assert!(t.scroll_menu(&c, n as f32 * ROW));
        let b = menu_box(&c, &s).unwrap();
        assert!(b.y1 <= 100.0, "the box ends with the window");
        let r = menu_row(&c, b, t.menu.unwrap(), n);
        assert!(
            r.y0 >= b.y0 && r.y1 <= b.y1,
            "row {n} is in the box: {r:?} {b:?}"
        );
        let (x, y) = (f64::from(r.x0 + 5.0), f64::from((r.y0 + r.y1) / 2.0));
        assert!(t.covers(&c, x, y));
        t.press(&c, x, y);
        assert_eq!(t.release(&c, x, y), (true, item.command()), "row {n}");
    }
    // The ends: nothing above the first row, nothing below the last.
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    assert!(t.scroll_menu(&c, -1.0e6));
    assert_eq!(t.menu, Some(0.0));
    assert!(t.scroll_menu(&c, 1.0e6));
    let b = menu_box(&c, &s).unwrap();
    let last = menu_row(&c, b, t.menu.unwrap(), 4);
    assert_eq!(
        last.y1 + EDGE,
        b.y1,
        "the last row ends at the box's padding"
    );
    assert!(t.scroll_menu(&c, f32::NAN));
    assert_eq!(t.menu, Some(0.0), "NaN is no scroll");
    // A dropdown that fits has nothing to scroll.
    let tall = ctx(150);
    let mut t = Toolbar::default();
    let (mx, my) = center(&slots(&tall), Item::More);
    t.press(&tall, mx, my);
    t.release(&tall, mx, my);
    assert!(t.scroll_menu(&tall, 50.0), "open: it takes the scrolling");
    assert_eq!(t.menu, Some(0.0));
}

/// A press on another bar button closes the dropdown and still acts.
#[test]
fn another_button_closes_the_dropdown() {
    let c = ctx(200 + (TOC + FIND) as u32);
    let s = slots(&c);
    let mut t = Toolbar::default();
    let (mx, my) = center(&s, Item::More);
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    assert!(t.menu_open(&c));
    let (rx, ry) = center(&s, Item::Reload);
    assert!(t.press(&c, rx, ry));
    assert!(!t.menu_open(&c), "closed by the press");
    assert_eq!(
        t.release(&c, rx, ry),
        (true, Item::Reload.command()),
        "the click still acts"
    );
    // `⋯` itself toggles.
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    t.press(&c, mx, my);
    assert!(t.menu_open(&c), "open until released");
    t.release(&c, mx, my);
    assert!(!t.menu_open(&c));
}

/// A dropdown whose `⋯` is gone (the window got wide) is closed for every purpose, and stays
/// closed when the window narrows again.
#[test]
fn dropdown_without_overflow_is_closed() {
    let (narrow, wide) = (ctx(200), ctx(900));
    let s = slots(&narrow);
    let mut t = Toolbar::default();
    let (mx, my) = center(&s, Item::More);
    t.press(&narrow, mx, my);
    t.release(&narrow, mx, my);
    assert!(t.menu_open(&narrow));
    assert!(!t.menu_open(&wide));
    // A hover timer isn't held back by a menu nobody can see.
    let now = Instant::now();
    let (x, y) = center(&slots(&wide), Item::Theme);
    assert!(t.pointer(&wide, x, y, now));
    assert_eq!(t.deadline(&wide), Some(now + TOOLTIP_DELAY));
    assert!(t.expire(&wide, now + TOOLTIP_DELAY));
    // The next press is the document's, not swallowed.
    assert!(!t.press(&wide, 100.0, 500.0));
    assert!(!t.menu_open(&narrow), "not open again at the narrow width");
    // Escape has nothing to close.
    t.press(&narrow, mx, my);
    t.release(&narrow, mx, my);
    assert!(!t.close_menu(&wide));
    assert!(!t.close_menu(&narrow));
}

/// A tooltip isn't due the moment the dropdown closes: the timer only runs while it's shut.
#[test]
fn no_tooltip_timer_under_the_dropdown() {
    let c = ctx(200 + (TOC + FIND) as u32);
    let s = slots(&c);
    let mut t = Toolbar::default();
    let (mx, my) = center(&s, Item::More);
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    let t0 = Instant::now();
    let (rx, ry) = center(&s, Item::Reload);
    assert!(t.pointer(&c, rx, ry, t0));
    assert_eq!(t.deadline(&c), None);
    assert!(t.close_menu(&c));
    assert_eq!(t.deadline(&c), None, "not due at once");
    assert!(!t.expire(&c, t0 + 10 * TOOLTIP_DELAY));
    // Moving on to a button arms it from then on.
    let t1 = t0 + Duration::from_secs(5);
    let (sx, sy) = center(&s, Item::Settings);
    assert!(t.pointer(&c, sx, sy, t1));
    assert_eq!(t.deadline(&c), Some(t1 + TOOLTIP_DELAY));
}

#[test]
fn tooltip_after_500ms_with_one_timer() {
    let c = ctx(900);
    let s = slots(&c);
    let t0 = Instant::now();
    let mut t = Toolbar::default();
    assert_eq!(t.deadline(&c), None);
    let (x, y) = center(&s, Item::Theme);
    assert!(t.pointer(&c, x, y, t0));
    assert!(
        !t.pointer(&c, x + 1.0, y, t0 + Duration::from_millis(100)),
        "same button"
    );
    assert_eq!(t.deadline(&c), Some(t0 + TOOLTIP_DELAY));
    assert!(!t.expire(&c, t0 + Duration::from_millis(499)));
    assert!(t.expire(&c, t0 + TOOLTIP_DELAY));
    assert_eq!(t.deadline(&c), None, "shown: no more timer");
    // Moving to another button restarts it; leaving clears it.
    let (x2, y2) = center(&s, Item::Reload);
    let t1 = t0 + Duration::from_secs(2);
    assert!(t.pointer(&c, x2, y2, t1));
    assert_eq!(t.deadline(&c), Some(t1 + TOOLTIP_DELAY));
    assert!(t.leave());
    assert_eq!(t.deadline(&c), None);
    // Off the bar: no timer.
    assert!(!t.pointer(&c, 400.0, 300.0, t1));
    assert_eq!(t.deadline(&c), None);
}

#[test]
fn hidden_bar_takes_nothing() {
    let c = ctx(900);
    let mut t = Toolbar::default();
    t.set_visible(false);
    assert_eq!(t.height(1.0), 0);
    assert!(!t.covers(&c, 10.0, 10.0));
    assert!(!t.press(&c, 10.0, 10.0));
    assert!(!t.pointer(&c, 10.0, 10.0, Instant::now()));
    t.set_visible(true);
    assert_eq!(t.height(1.0), 32);
    assert!(t.covers(&c, 10.0, 10.0));
}

#[test]
fn labels_name_the_shortcut() {
    let c = ctx(900);
    assert_eq!(Item::Back.label(&c), "Back (Backspace)");
    assert_eq!(Item::Open.label(&c), "Open a file (Ctrl+O)");
    assert!(Item::ZoomReset.label(&c).contains("100%"));
    assert_eq!(
        Item::Theme.label(&Ctx {
            theme: ThemeMode::Dark,
            ..c
        }),
        "Theme: dark (T)"
    );
}

/// The contents button sits with Back and Open, names its shortcut either way round, and goes into
/// the dropdown last of all, at the top of it.
#[cfg(feature = "toc")]
#[test]
fn the_contents_button_goes_last() {
    let c = ctx(900);
    assert_eq!(Item::Contents.label(&c), "Contents (C)");
    assert_eq!(
        Item::Contents.label(&Ctx { toc: true, ..c }),
        "Hide contents (C)"
    );
    // One pixel short of the room for all of it, the right group's leading groups are in `⋯`
    // and the button is still where it was; narrower than the left group and `⋯`, it joins them.
    let s = slots(&ctx(200 + TOC as u32));
    assert!(items(&s).contains(&Item::Contents));
    assert!(!s.overflow.contains(&Item::Contents));
    let edge = (EDGE + LEFT + TOC + GAP + BUTTON + EDGE) as u32;
    let s = slots(&ctx(edge));
    assert!(items(&s).contains(&Item::Contents), "{:?}", s.overflow);
    let s = slots(&ctx(edge - 1));
    assert_eq!(s.overflow.first(), Some(&Item::Contents));
    assert!(!items(&s).contains(&Item::Contents));
    let more = center(&s, Item::More);
    // The dropdown's first row is the button, and its click is the toggle.
    let mut t = Toolbar::default();
    let c = ctx(edge - 1);
    t.press(&c, more.0, more.1);
    t.release(&c, more.0, more.1);
    let rows = menu_rows(&c, &s, 0.0);
    assert_eq!(rows[0].0, Item::Contents);
    let (x, y) = (f64::from(rows[0].1.x0 + 5.0), f64::from(rows[0].1.y0 + 5.0));
    t.press(&c, x, y);
    assert_eq!(
        t.release(&c, x, y),
        (true, Some(Command::Action(Action::ToggleToc)))
    );
}

#[cfg(feature = "embed-fonts")]
#[test]
fn paints_bar_icons_menu_and_tooltip() {
    use crate::theme::LIGHT;
    let fonts = Fonts::new().unwrap();
    let mut cache = GlyphCache::default();
    let mut text = TextCtx::default();
    let c = ctx(200);
    let mut buf = vec![0u32; 200 * 300];
    let mut t = Toolbar::default();
    let paint = |t: &Toolbar, buf: &mut Vec<u32>, cache: &mut GlyphCache, text: &mut TextCtx| {
        buf.fill(LIGHT.bg);
        let mut f = Frame::new(buf, 200, 300, 200);
        t.paint(&mut f, &c, &fonts, cache, text, &LIGHT);
    };
    paint(&t, &mut buf, &mut cache, &mut text);
    // Bar background, divider, and icon ink in the bar only.
    assert_eq!(buf[200 * 2 + 100], LIGHT.code_bg);
    assert_eq!(buf[200 * 31 + 100], LIGHT.border);
    let ink = |buf: &[u32], y0: usize, y1: usize| {
        (y0..y1).any(|y| {
            (0..200).any(|x| ![LIGHT.bg, LIGHT.code_bg, LIGHT.border].contains(&buf[y * 200 + x]))
        })
    };
    assert!(ink(&buf, 0, 31), "icons drawn");
    assert!(!ink(&buf, 33, 300), "nothing below the bar");
    // Open menu: drawn below the bar.
    let s = slots(&c);
    let (mx, my) = center(&s, Item::More);
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    paint(&t, &mut buf, &mut cache, &mut text);
    assert!(ink(&buf, 36, 200), "menu rows drawn");
    // Tooltip.
    t.close_menu(&c);
    let (x, y) = center(&s, Item::Back);
    let t0 = Instant::now();
    t.pointer(&c, x, y, t0);
    t.expire(&c, t0 + TOOLTIP_DELAY);
    paint(&t, &mut buf, &mut cache, &mut text);
    assert!(ink(&buf, 33, 70), "tooltip drawn under the bar");
}

/// With remote images in the document, their button leads the right group, collapses first,
/// counts them in its label, toggles them, and says when `curl` is missing.
#[test]
fn the_remote_images_button() {
    let c = Ctx {
        remote: 3,
        ..ctx(900)
    };
    let s = slots(&c);
    assert_eq!(
        items(&s).last(),
        Some(&Item::Images),
        "leftmost of the right group"
    );
    let r = |i| s.items.iter().find(|(x, _)| *x == i).unwrap().1;
    assert!(r(Item::Images).x1 <= r(Item::ZoomOut).x0 + 0.01);
    assert_eq!(Item::Images.label(&c), "Load 3 remote images (I)");
    assert_eq!(
        Item::Images.label(&Ctx { remote: 1, ..c }),
        "Load 1 remote image (I)"
    );
    assert_eq!(
        Item::Images.label(&Ctx {
            remote_on: true,
            ..c
        }),
        "Hide remote images (I)"
    );
    let mut t = Toolbar::default();
    let (x, y) = center(&s, Item::Images);
    assert!(t.press(&c, x, y));
    assert_eq!(
        t.release(&c, x, y),
        (true, Some(Command::Action(Action::ToggleRemote)))
    );
    // No curl: it says so and does nothing.
    let none = Ctx { no_curl: true, ..c };
    assert_eq!(Item::Images.label(&none), "Remote images: curl not found");
    t.press(&none, x, y);
    assert_eq!(t.release(&none, x, y), (true, None));
    // Narrower: the zoom goes into `⋯` first, then it.
    let full =
        (EDGE + LEFT + TOC + GAP + 3.0 * BUTTON + PERCENT + 4.0 * BUTTON + FIND + EDGE) as u32;
    assert!(slots(&Ctx { width: full, ..c }).overflow.is_empty());
    let s = slots(&Ctx {
        width: full - 1,
        ..c
    });
    assert_eq!(s.overflow, [Item::ZoomOut, Item::ZoomReset, Item::ZoomIn]);
    assert!(items(&s).contains(&Item::Images));
    let s = slots(&Ctx {
        width: 232 + FIND as u32,
        ..c
    });
    assert_eq!(
        s.overflow[..4],
        [Item::Images, Item::ZoomOut, Item::ZoomReset, Item::ZoomIn]
    );
}

/// With an animated image in the document, the animations button follows the remote-images one
/// (or leads the right group), plays or pauses them all, says which with its icon and label, and
/// goes into `⋯` with the remote-images button, after the zoom.
#[test]
fn the_animations_button() {
    assert!(!items(&slots(&ctx(900))).contains(&Item::Animations));
    let c = Ctx {
        animated: true,
        ..ctx(900)
    };
    let s = slots(&c);
    assert_eq!(items(&s).last(), Some(&Item::Animations));
    let both = Ctx { remote: 2, ..c };
    let s2 = slots(&both);
    let r = |s: &Slots, i| s.items.iter().find(|(x, _)| *x == i).unwrap().1;
    assert!(r(&s2, Item::Images).x1 <= r(&s2, Item::Animations).x0 + 0.01);
    assert!(r(&s2, Item::Animations).x1 <= r(&s2, Item::ZoomOut).x0 + 0.01);
    assert_eq!(Item::Animations.label(&c), "Play animations (A)");
    assert_eq!(Item::Animations.icon(&c), Some(icon::PLAY));
    let on = Ctx {
        animations: true,
        ..c
    };
    assert_eq!(Item::Animations.label(&on), "Pause animations (A)");
    assert_eq!(Item::Animations.icon(&on), Some(icon::PAUSE));
    let mut t = Toolbar::default();
    let (x, y) = center(&s, Item::Animations);
    assert!(t.press(&c, x, y));
    assert_eq!(
        t.release(&c, x, y),
        (true, Some(Command::Action(Action::ToggleAnimations)))
    );
    // Narrow: after the zoom, it goes with the remote-images button.
    let s = slots(&Ctx {
        width: 232 + FIND as u32,
        ..both
    });
    assert_eq!(
        s.overflow[..5],
        [
            Item::Images,
            Item::Animations,
            Item::ZoomOut,
            Item::ZoomReset,
            Item::ZoomIn
        ]
    );
}

/// The button's badge and pulse are painted in the link color.
#[cfg(feature = "remote-images")]
#[test]
fn the_badge_and_pulse_are_painted() {
    let fonts = Fonts::new().unwrap();
    let (mut cache, mut text) = (GlyphCache::default(), TextCtx::default());
    let paint = |c: &Ctx, cache: &mut GlyphCache, text: &mut TextCtx| {
        let mut buf = vec![0u32; 900 * 40];
        let mut frame = Frame::new(&mut buf, 900, 40, 900);
        Toolbar::default().paint(&mut frame, c, &fonts, cache, text, &crate::theme::LIGHT);
        buf
    };
    let c = Ctx {
        remote: 3,
        ..ctx(900)
    };
    let r = slots(&c)
        .items
        .iter()
        .find(|(i, _)| *i == Item::Images)
        .unwrap()
        .1;
    let link = crate::theme::LIGHT.link;
    let count = |buf: &[u32]| {
        (r.x0 as usize..r.x1 as usize)
            .flat_map(|x| (0..32).map(move |y| (x, y)))
            .filter(|&(x, y)| buf[y * 900 + x] == link)
            .count()
    };
    let badge = count(&paint(&c, &mut cache, &mut text));
    assert!(badge > 20, "a badge: {badge}");
    let on = count(&paint(
        &Ctx {
            remote_on: true,
            ..c
        },
        &mut cache,
        &mut text,
    ));
    assert_eq!(on, 0, "no badge once they're on");
    assert!(count(&paint(&ctx(900), &mut cache, &mut text)) == 0);
    let pulse = paint(&Ctx { pulse: true, ..c }, &mut cache, &mut text);
    let plain = paint(&c, &mut cache, &mut text);
    assert_ne!(pulse, plain, "the pulse shows");
}

/// With the button inside the dropdown, `⋯` pulses in its place.
#[cfg(feature = "remote-images")]
#[test]
fn the_pulse_moves_to_the_more_button_with_the_images_button() {
    let fonts = Fonts::new().unwrap();
    let (mut cache, mut text) = (GlyphCache::default(), TextCtx::default());
    let c = Ctx {
        remote: 3,
        ..ctx(200)
    };
    let s = slots(&c);
    assert!(s.overflow.contains(&Item::Images));
    let r = s.items.iter().find(|(i, _)| *i == Item::More).unwrap().1;
    let mut paint = |pulse: bool| {
        let mut buf = vec![0u32; 200 * 40];
        let mut frame = Frame::new(&mut buf, 200, 40, 200);
        let c = Ctx { pulse, ..c };
        Toolbar::default().paint(
            &mut frame,
            &c,
            &fonts,
            &mut cache,
            &mut text,
            &crate::theme::LIGHT,
        );
        buf
    };
    let (plain, pulse) = (paint(false), paint(true));
    let changed = |x: usize, y: usize| plain[y * 200 + x] != pulse[y * 200 + x];
    let on_more = (r.x0 as usize..r.x1 as usize)
        .flat_map(|x| (0..32).map(move |y| (x, y)))
        .filter(|&(x, y)| changed(x, y))
        .count();
    assert!(on_more > 20, "`⋯` pulses: {on_more}");
}

#[test]
fn wide_button_names_and_draws_the_layout_it_switches_to() {
    let c = ctx(900);
    let on = Ctx { wide: true, ..c };
    assert_eq!(Item::Wide.label(&c), "Wide layout (W)");
    assert_eq!(Item::Wide.label(&on), "Normal width (W)");
    assert_eq!(Item::Wide.icon(&c), Some(icon::UNFOLD));
    assert_eq!(Item::Wide.icon(&on), Some(icon::FOLD));
    // Both click the same action: the view flips whatever it has.
    assert_eq!(
        Item::Wide.command(),
        Some(Command::Action(Action::ToggleWide))
    );
}

/// The find button sits between reload and ⚙, names its shortcut either way round, and lights up
/// while the bar is open like the contents button does.
#[cfg(feature = "find")]
#[test]
fn the_find_button_shows_whether_the_bar_is_open() {
    let c = ctx(900);
    assert_eq!(Item::Find.label(&c), "Find (Ctrl+F)");
    assert_eq!(
        Item::Find.label(&Ctx { find: true, ..c }),
        "Close find (Ctrl+F)"
    );
    let s = slots(&c);
    let r = |i| s.items.iter().find(|(x, _)| *x == i).unwrap().1;
    assert_eq!(r(Item::Find).x1, r(Item::Settings).x0);
    assert_eq!(r(Item::Reload).x1, r(Item::Find).x0);
    // It goes into the dropdown after reload and before ⚙ is lost: a window with room for ⚙, find
    // and `⋯` only has reload and the rest in it.
    let narrow = ctx((EDGE + LEFT + TOC + GAP + 2.0 * BUTTON + BUTTON + EDGE) as u32);
    let s = slots(&narrow);
    assert!(items(&s).contains(&Item::Find) && items(&s).contains(&Item::Settings));
    assert!(s.overflow.contains(&Item::Reload));
}

#[cfg(all(feature = "find", feature = "embed-fonts"))]
#[test]
fn the_find_button_is_lit_while_the_bar_is_open() {
    use crate::theme::LIGHT;
    let fonts = Fonts::new().unwrap();
    let (mut cache, mut text) = (GlyphCache::default(), TextCtx::default());
    let mut paint = |c: &Ctx| {
        let mut buf = vec![LIGHT.bg; 900 * 40];
        let mut f = Frame::new(&mut buf, 900, 40, 900);
        Toolbar::default().paint(&mut f, c, &fonts, &mut cache, &mut text, &LIGHT);
        buf
    };
    let c = ctx(900);
    let (off, on) = (paint(&c), paint(&Ctx { find: true, ..c }));
    let r = slots(&c)
        .items
        .iter()
        .find(|(i, _)| *i == Item::Find)
        .unwrap()
        .1;
    // The button's corner area differs (the tint), nothing else does.
    let tint = |buf: &[u32]| buf[(r.y0 as usize + 8) * 900 + r.x0 as usize + 5];
    assert_ne!(tint(&off), tint(&on));
    let changed = off.iter().zip(&on).filter(|(a, b)| a != b).count();
    assert!(changed > 0);
    let outside = |i: usize| i % 900 < r.x0 as usize || i % 900 >= r.x1 as usize;
    assert!(
        off.iter()
            .zip(&on)
            .enumerate()
            .all(|(i, (a, b))| !outside(i) || a == b)
    );
}
