use super::*;

fn ctx(width: u32) -> Ctx {
    Ctx {
        width,
        height: 600,
        scale: 1.0,
        can_back: true,
        zoom: 100,
        theme: ThemeMode::System,
        settings: true,
    }
}

fn items(s: &Slots) -> Vec<Item> {
    s.items.iter().map(|(i, _)| *i).collect()
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
            Item::Settings,
            Item::Reload,
            Item::Theme,
            Item::ZoomIn,
            Item::ZoomReset,
            Item::ZoomOut
        ]
    );
    assert!(s.overflow.is_empty());
    // Right group flush with the right edge, left to right: − % + theme reload settings.
    let r = |i| s.items.iter().find(|(x, _)| *x == i).unwrap().1;
    assert_eq!(r(Item::Settings).x1, 900.0 - EDGE);
    assert!(r(Item::ZoomOut).x1 <= r(Item::ZoomReset).x0 + 0.01);
    assert!(r(Item::ZoomIn).x1 <= r(Item::Theme).x0 + 0.01);
    // No settings file: no ⚙.
    let s = slots(&Ctx {
        settings: false,
        ..ctx(900)
    });
    assert!(!items(&s).contains(&Item::Settings));
}

#[test]
fn narrow_bar_collapses_zoom_first_then_theme() {
    let full = (EDGE + BUTTON + GAP + 2.0 * BUTTON + PERCENT + 3.0 * BUTTON + EDGE) as u32;
    assert!(slots(&ctx(full)).overflow.is_empty(), "exactly fits");
    let s = slots(&ctx(full - 1));
    assert_eq!(s.overflow, [Item::ZoomOut, Item::ZoomReset, Item::ZoomIn]);
    assert!(items(&s).contains(&Item::More) && items(&s).contains(&Item::Theme));
    let s = slots(&ctx(150));
    assert_eq!(
        s.overflow,
        [Item::ZoomOut, Item::ZoomReset, Item::ZoomIn, Item::Theme]
    );
    // Down to 320 px and below, everything stays reachable: shown or in the menu.
    for w in [320, 200, 120, 80, 40, 1] {
        let s = slots(&ctx(w));
        let mut all: Vec<Item> = items(&s);
        all.extend(&s.overflow);
        for item in [
            Item::Back,
            Item::ZoomOut,
            Item::ZoomReset,
            Item::ZoomIn,
            Item::Theme,
            Item::Reload,
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
        (Item::ZoomOut, Command::Action(Action::ZoomOut)),
        (Item::ZoomReset, Command::Action(Action::ZoomReset)),
        (Item::ZoomIn, Command::Action(Action::ZoomIn)),
        (Item::Theme, Command::Action(Action::CycleTheme)),
        (Item::Reload, Command::Action(Action::Reload)),
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
    assert!(t.menu_open());
    let rows = menu_rows(&c, &s);
    assert_eq!(rows.len(), s.overflow.len());
    assert!(
        rows.iter().all(|(_, r)| r.x0 >= 0.0 && r.x1 <= 200.0),
        "inside the window"
    );
    // The open menu covers what's under it.
    let (item, r) = rows[0];
    let (rx, ry) = (f64::from(r.x0 + 5.0), f64::from((r.y0 + r.y1) / 2.0));
    assert!(t.covers(&c, rx, ry));
    t.press(&c, rx, ry);
    assert_eq!(t.release(&c, rx, ry), (true, item.command()));
    assert!(!t.menu_open(), "closes on selection");
    // An outside press closes it and is swallowed.
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    assert!(t.menu_open());
    assert!(t.press(&c, 100.0, 500.0));
    assert!(!t.menu_open());
    assert_eq!(t.release(&c, 100.0, 500.0), (false, None));
    // Escape-style close.
    t.press(&c, mx, my);
    t.release(&c, mx, my);
    assert!(t.close_menu());
    assert!(!t.close_menu());
}

#[test]
fn tooltip_after_500ms_with_one_timer() {
    let c = ctx(900);
    let s = slots(&c);
    let t0 = Instant::now();
    let mut t = Toolbar::default();
    assert_eq!(t.deadline(), None);
    let (x, y) = center(&s, Item::Theme);
    assert!(t.pointer(&c, x, y, t0));
    assert!(
        !t.pointer(&c, x + 1.0, y, t0 + Duration::from_millis(100)),
        "same button"
    );
    assert_eq!(t.deadline(), Some(t0 + TOOLTIP_DELAY));
    assert!(!t.expire(t0 + Duration::from_millis(499)));
    assert!(t.expire(t0 + TOOLTIP_DELAY));
    assert_eq!(t.deadline(), None, "shown: no more timer");
    // Moving to another button restarts it; leaving clears it.
    let (x2, y2) = center(&s, Item::Reload);
    let t1 = t0 + Duration::from_secs(2);
    assert!(t.pointer(&c, x2, y2, t1));
    assert_eq!(t.deadline(), Some(t1 + TOOLTIP_DELAY));
    assert!(t.leave());
    assert_eq!(t.deadline(), None);
    // Off the bar: no timer.
    assert!(!t.pointer(&c, 400.0, 300.0, t1));
    assert_eq!(t.deadline(), None);
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
    assert!(Item::ZoomReset.label(&c).contains("100%"));
    assert_eq!(
        Item::Theme.label(&Ctx {
            theme: ThemeMode::Dark,
            ..c
        }),
        "Theme: dark (T)"
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
    t.close_menu();
    let (x, y) = center(&s, Item::Back);
    let t0 = Instant::now();
    t.pointer(&c, x, y, t0);
    t.expire(t0 + TOOLTIP_DELAY);
    paint(&t, &mut buf, &mut cache, &mut text);
    assert!(ink(&buf, 33, 70), "tooltip drawn under the bar");
}
