use std::time::Instant;

use super::*;
use crate::doc::Doc;
use crate::fonts::Fonts;
use crate::theme::{DARK, LIGHT};
use crate::toolbar::Command;
use crate::view::ThemeMode;

const INVITE: &str = "Press Ctrl+O to open a Markdown file, or drop one here";

/// `n` files `/home/u/notes/f0.md`, … (they needn't be there: the shell leaves out those that
/// aren't).
fn files(n: usize) -> Vec<PathBuf> {
    (0..n)
        .map(|i| PathBuf::from(format!("/home/u/notes/f{i}.md")))
        .collect()
}

/// The empty window `w` × `h` at 1x with `n` recent files, painted once.
fn empty(w: u32, h: u32, n: usize) -> View {
    let mut v = View::new(
        Fonts::new().unwrap(),
        Doc::default(),
        Some(INVITE.into()),
        (w, h),
        1.0,
    );
    v.set_recent(files(n), Some(Path::new("/home/u")));
    paint(&mut v);
    v
}

fn paint(v: &mut View) -> Vec<u32> {
    let (w, h) = v.size();
    let mut buf = vec![0; (w * h) as usize];
    v.paint(&mut buf, w as usize, Instant::now());
    buf
}

/// The middle of what `hit` was painted at.
fn center(v: &View, hit: Hit) -> (f64, f64) {
    let &(_, (x0, y0, x1, y1)) = v.recent.spots.iter().find(|(h, _)| *h == hit).unwrap();
    (f64::from(x0 + x1) / 2.0, f64::from(y0 + y1) / 2.0)
}

fn key(c: &str) -> Key {
    Key::Character(c.into())
}

fn named(k: NamedKey) -> Key {
    Key::Named(k)
}

#[test]
fn the_plan_drops_rows_from_the_bottom_and_clear_before_the_last() {
    let msg = 24.0;
    // Room for everything: 8 rows and Clear, the block centered.
    let p = plan(900.0, 0.0, 1000.0, 1.0, msg, 8);
    assert_eq!((p.rows, p.clear), (8, true));
    let total = msg + GAP + HEAD + 8.0 * ROW + CLEAR_GAP + CLEAR;
    assert_eq!(p.message_y, ((1000.0 - total) / 2.0_f32).round());
    assert_eq!(p.head_y, p.message_y + msg + GAP);
    assert_eq!(p.rows_y, p.head_y + HEAD);
    assert_eq!(p.clear_y, p.rows_y + 8.0 * ROW + CLEAR_GAP);
    // The list is 640 wide at most, centered, with 32 at the sides at least.
    assert_eq!((p.x0, p.x1), (130.0, 770.0));
    let narrow = plan(300.0, 0.0, 1000.0, 1.0, msg, 8);
    assert_eq!((narrow.x0, narrow.x1), (32.0, 268.0));
    // Fewer rows as the window gets lower, Clear kept until a single row is left.
    let fits = |h: f32| {
        let p = plan(900.0, 0.0, h, 1.0, msg, 8);
        (p.rows, p.clear)
    };
    let need = |rows: f32, clear: bool| {
        2.0 * MARGIN + msg + GAP + HEAD + rows * ROW + if clear { CLEAR_GAP + CLEAR } else { 0.0 }
    };
    assert_eq!(fits(need(8.0, true)), (8, true));
    assert_eq!(fits(need(8.0, true) - 1.0), (7, true));
    assert_eq!(fits(need(3.0, true)), (3, true));
    assert_eq!(fits(need(1.0, true)), (1, true));
    assert_eq!(fits(need(1.0, true) - 1.0), (1, false), "Clear goes first");
    assert_eq!(
        fits(need(1.0, false) - 1.0),
        (0, false),
        "then the last row"
    );
    assert_eq!(fits(10.0), (0, false), "the invitation stays");
    // A shorter list, and none.
    assert_eq!(fits(1000.0).0, 8);
    assert_eq!(plan(900.0, 0.0, 1000.0, 1.0, msg, 3).rows, 3);
    assert_eq!(plan(900.0, 0.0, 1000.0, 1.0, msg, 0).rows, 0);
    // Below a toolbar: the block is centered in the page under it.
    let below = plan(900.0, 40.0, 960.0, 1.0, msg, 8);
    assert_eq!(below.message_y, ((960.0 - total) / 2.0_f32 + 40.0).round());
    // Zoom (or a 2x display) scales it all.
    let big = plan(900.0, 0.0, 600.0, 2.0, msg * 2.0, 8);
    assert!(big.rows < 8 && big.rows > 0);
    assert_eq!(big.rows_y - big.head_y, 2.0 * HEAD);
}

#[test]
fn rows_show_as_far_as_the_window_allows() {
    assert_eq!(empty(900, 1000, 12).recent_rows(), 8, "eight at most");
    assert_eq!(empty(900, 1000, 3).recent_rows(), 3);
    // A small window: fewer, but the invitation always.
    let small = empty(300, 200, 8);
    let rows = small.recent_rows();
    assert!(rows < 8, "{rows}");
    // At 300% zoom, fewer still (here, none).
    let mut zoomed = empty(300, 200, 8);
    zoomed.set_zoom(300);
    paint(&mut zoomed);
    assert!(zoomed.recent_rows() <= rows);
    assert_eq!(zoomed.recent_rows(), 0);
    // None at all: the empty window as before.
    let none = empty(900, 1000, 0);
    assert!(!none.recent_showing());
    assert!(none.recent.spots.is_empty());
}

#[test]
fn the_list_shows_only_with_the_invitation() {
    let mut v = empty(900, 1000, 3);
    assert!(v.recent_showing());
    // A document (or an error in its place) forgets it: the shell gives it again for the
    // empty window only.
    v.open(Doc::parse("# Hi"), None);
    assert!(!v.recent_showing());
    assert_eq!(v.recent_path(0), None);
    v.open(Doc::default(), Some("Cannot open x.md".into()));
    assert!(!v.recent_showing());
    assert_eq!(v.recent_key(&key("1"), ModifiersState::empty()), None);
}

#[test]
fn digits_open_their_row() {
    let mut v = empty(900, 1000, 3);
    let none = ModifiersState::empty();
    assert_eq!(v.recent_key(&key("1"), none), Some(Outcome::OpenRecent(0)));
    assert_eq!(v.recent_key(&key("3"), none), Some(Outcome::OpenRecent(2)));
    assert_eq!(v.recent_key(&key("4"), none), None, "no fourth row");
    assert_eq!(v.recent_key(&key("0"), none), None);
    assert_eq!(v.recent_key(&key("9"), none), None);
    assert_eq!(v.recent_path(2), Some(PathBuf::from("/home/u/notes/f2.md")));
    // Not with Ctrl, Alt or Cmd: those are other keys' (Ctrl+0 resets the zoom).
    for mods in [
        ModifiersState::CONTROL,
        ModifiersState::ALT,
        ModifiersState::SUPER,
    ] {
        assert_eq!(v.recent_key(&key("1"), mods), None);
    }
    // Shift is fine (some layouts need it for digits).
    assert_eq!(
        v.recent_key(&key("2"), ModifiersState::SHIFT),
        Some(Outcome::OpenRecent(1))
    );
    // Only the rows that show: in a small window, a digit past them does nothing.
    let mut small = empty(300, 200, 8);
    let rows = small.recent_rows();
    let past = (rows + 1).to_string();
    assert_eq!(small.recent_key(&key(&past), none), None);
}

#[test]
fn arrows_pick_a_row_for_enter_and_delete() {
    let mut v = empty(900, 1000, 3);
    let none = ModifiersState::empty();
    // Nothing picked: Enter and Delete do nothing (Enter isn't bound elsewhere either).
    assert_eq!(v.recent_key(&named(NamedKey::Enter), none), None);
    assert_eq!(v.recent_key(&named(NamedKey::Delete), none), None);
    let down = named(NamedKey::ArrowDown);
    assert_eq!(v.recent_key(&down, none), Some(Outcome::Redraw));
    assert_eq!(v.recent.focus, Some(0));
    v.recent_key(&down, none);
    v.recent_key(&key("j"), none);
    v.recent_key(&down, none);
    assert_eq!(v.recent.focus, Some(2), "stops at the last");
    v.recent_key(&key("k"), none);
    assert_eq!(v.recent.focus, Some(1));
    assert_eq!(
        v.recent_key(&named(NamedKey::Enter), none),
        Some(Outcome::OpenRecent(1))
    );
    assert_eq!(
        v.recent_key(&named(NamedKey::Delete), none),
        Some(Outcome::ForgetRecent(1))
    );
    assert_eq!(
        v.recent_key(&named(NamedKey::Backspace), none),
        Some(Outcome::ForgetRecent(1)),
        "the Mac's delete key"
    );
    v.recent_key(&named(NamedKey::ArrowUp), none);
    v.recent_key(&named(NamedKey::ArrowUp), none);
    assert_eq!(v.recent.focus, Some(0), "stops at the first");
    // Up first picks the last row.
    let mut w = empty(900, 1000, 3);
    w.recent_key(&named(NamedKey::ArrowUp), none);
    assert_eq!(w.recent.focus, Some(2));
    // Given the list again after a Delete, the pick stays as far as the list reaches.
    w.set_recent(files(2), None);
    assert_eq!(w.recent.focus, Some(1));
    w.set_recent(Vec::new(), None);
    assert_eq!(w.recent.focus, None);
    // Other keys are left to the rest (Escape quits, Home does nothing).
    assert_eq!(v.recent_key(&named(NamedKey::Escape), none), None);
    assert_eq!(v.recent_key(&key("q"), none), None);
}

#[test]
fn a_click_on_a_row_or_clear_asks_the_shell() {
    let mut v = empty(900, 1000, 3);
    let now = Instant::now();
    let (x, y) = center(&v, Hit::Row(1));
    assert!(v.hover(x, y), "hovering a row repaints");
    assert!(v.over_recent(), "the hand");
    assert!(v.press(x, y, now));
    v.release(now);
    assert_eq!(v.click(x, y), None, "no link");
    assert_eq!(
        v.take_command(),
        Some(Command::Action(Action::OpenRecent(1)))
    );
    assert_eq!(v.apply(Action::OpenRecent(1), now), Outcome::OpenRecent(1));
    // Pressed on one row and released on another: nothing.
    let (x2, y2) = center(&v, Hit::Row(2));
    v.press(x, y, now);
    v.release(now);
    v.click(x2, y2);
    assert_eq!(v.take_command(), None);
    // Clear list.
    let (cx, cy) = center(&v, Hit::Clear);
    v.press(cx, cy, now);
    v.release(now);
    v.click(cx, cy);
    assert_eq!(v.take_command(), Some(Command::Action(Action::ClearRecent)));
    assert_eq!(v.apply(Action::ClearRecent, now), Outcome::ClearRecent);
    // Off the rows: no hand, no command.
    assert!(v.hover(5.0, 5.0));
    assert!(!v.over_recent());
    assert!(!v.press(5.0, 5.0, now));
    v.click(5.0, 5.0);
    assert_eq!(v.take_command(), None);
    // Leaving the window drops the hover.
    v.hover(x, y);
    assert!(v.hover_end());
    assert!(!v.over_recent());
}

/// Another window opened B.md, and the list comes again ([B.md, A.md]) with `1` already queued,
/// before the next paint: the key acts on nothing the reader hasn't seen. Painted, it opens the
/// row shown first, B.md.
#[test]
fn keys_wait_for_a_new_list_to_be_painted() {
    let none = ModifiersState::empty();
    let (a, b) = (PathBuf::from("/d/A.md"), PathBuf::from("/d/B.md"));
    let mut v = empty(900, 1000, 0);
    v.set_recent(vec![a.clone()], None);
    paint(&mut v);
    v.recent_key(&named(NamedKey::ArrowDown), none);
    v.set_recent(vec![b.clone(), a.clone()], None);
    for k in [
        key("1"),
        key("2"),
        key("j"),
        named(NamedKey::Enter),
        named(NamedKey::Delete),
    ] {
        assert_eq!(v.recent_key(&k, none), Some(Outcome::Nothing), "{k:?}");
    }
    assert_eq!(
        v.recent_key(&key("q"), none),
        None,
        "other keys are left alone"
    );
    assert_eq!(v.recent.focus, Some(1), "A.md stays picked where it went");
    paint(&mut v);
    assert_eq!(v.recent_key(&key("1"), none), Some(Outcome::OpenRecent(0)));
    assert_eq!(v.recent_path(0), Some(b));
    assert_eq!(
        v.recent_key(&named(NamedKey::Enter), none),
        Some(Outcome::OpenRecent(1))
    );
    assert_eq!(v.recent_path(1), Some(a));
}

/// The pick follows its file when the list comes again in another order.
#[test]
fn the_pick_follows_its_file() {
    let none = ModifiersState::empty();
    let mut v = empty(900, 1000, 3);
    v.recent_key(&named(NamedKey::ArrowDown), none);
    v.recent_key(&named(NamedKey::ArrowDown), none);
    assert_eq!(v.recent.focus, Some(1));
    let mut list = files(3);
    list.insert(0, PathBuf::from("/home/u/new.md"));
    v.set_recent(list, None);
    assert_eq!(v.recent.focus, Some(2), "f1.md, now the third row");
}

/// The shell gives the list again when the window gets focus, which a click into it brings just
/// before the press: the same list keeps where its rows are, so the click lands.
#[test]
fn the_same_list_again_keeps_its_rows() {
    let mut v = empty(900, 1000, 3);
    let now = Instant::now();
    let (x, y) = center(&v, Hit::Row(0));
    v.set_recent(files(3), Some(Path::new("/home/u")));
    assert!(v.press(x, y, now));
    v.release(now);
    v.click(x, y);
    assert_eq!(
        v.take_command(),
        Some(Command::Action(Action::OpenRecent(0)))
    );
    // Another list: laid out again on the next paint.
    v.set_recent(files(2), None);
    assert!(!v.press(x, y, now));
}

/// Rows sit below a toolbar too, and a click on the toolbar is the toolbar's.
#[test]
fn the_toolbar_pushes_the_list_down() {
    let mut v = empty(900, 1000, 3);
    let before = center(&v, Hit::Row(0)).1;
    v.set_toolbar(true);
    paint(&mut v);
    let after = center(&v, Hit::Row(0)).1;
    assert!(after > before);
    assert!(
        v.recent
            .spots
            .iter()
            .all(|(_, r)| r.1 >= v.top_inset() as i32)
    );
}

/// Painted in both themes, with only the theme's colors: the rows' text in the link color, the
/// rest muted, a picked row tinted.
#[test]
fn the_list_paints_in_both_themes() {
    for (mode, theme) in [(ThemeMode::Light, LIGHT), (ThemeMode::Dark, DARK)] {
        let mut v = empty(900, 1000, 3);
        v.set_theme_mode(mode);
        let buf = paint(&mut v);
        let first = pixels(&v, &buf, Hit::Row(0));
        assert_eq!(edge(&v, &buf, Hit::Row(0)), theme.bg, "no tint at rest");
        // Link-colored text (the name), anti-aliased toward it.
        let near = |a: u32, b: u32| {
            (0..3).all(|i| ((a >> (8 * i)) & 0xff).abs_diff((b >> (8 * i)) & 0xff) < 40)
        };
        assert!(first.iter().any(|&p| near(p, theme.link)));
        // Picked: tinted.
        v.recent_key(&named(NamedKey::ArrowDown), ModifiersState::empty());
        let buf = paint(&mut v);
        assert_ne!(edge(&v, &buf, Hit::Row(0)), theme.bg);
        assert_eq!(edge(&v, &buf, Hit::Row(1)), theme.bg);
    }
}

/// The pixels painted where `hit` is.
fn pixels(v: &View, buf: &[u32], hit: Hit) -> Vec<u32> {
    let w = v.size().0;
    let &(_, (x0, y0, x1, y1)) = v.recent.spots.iter().find(|(h, _)| *h == hit).unwrap();
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| buf[(y as u32 * w + x as u32) as usize]))
        .collect()
}

/// The pixel at the left of `hit`, halfway down: the tint's, or the background.
fn edge(v: &View, buf: &[u32], hit: Hit) -> u32 {
    let w = v.size().0;
    let &(_, (x0, y0, _, y1)) = v.recent.spots.iter().find(|(h, _)| *h == hit).unwrap();
    buf[(((y0 + y1) / 2) as u32 * w + x0 as u32 + 2) as usize]
}

/// Bidi controls in a folder's name are shown escaped, never acted on.
#[test]
fn invisible_characters_are_shown() {
    let mut v = empty(900, 1000, 0);
    #[cfg(not(windows))]
    let (path, folder) = ("/d/a\u{202E}b/x.md", "/d/a\\u{202E}b/");
    #[cfg(windows)]
    let (path, folder) = ("C:\\d\\a\u{202E}b\\x.md", "C:\\d\\a\\u{202E}b\\");
    v.set_recent(vec![PathBuf::from(path)], None);
    assert_eq!(v.recent.entries[0].folder, folder);
}

/// A folder too long for the row keeps its end: the most specific part.
#[test]
fn long_folders_are_cut_at_their_start() {
    let mut v = empty(900, 1000, 0);
    let s = style(16.0, Role::Muted);
    let long = "/very/long/path/that/goes/on/and/on/to/the/notes/2026/";
    let (_, whole) = v.fit_start(long, s, 1e6);
    let (para, pl) = v.fit_start(long, s, whole.width / 2.0);
    assert!(pl.width <= whole.width / 2.0);
    let text = &para.text;
    assert!(text.starts_with('…'), "{text}");
    assert!(text.ends_with("/notes/2026/"), "{text}");
    // As much as fits: one more character would not.
    let more = format!("…{}", &long[long.len() - text.len() + "…".len() - 1..]);
    assert!(v.fit_start(&more, s, 1e6).1.width > whole.width / 2.0);
    // Huge: only its end is laid out, and the cut is the same.
    let huge = format!("{}{long}", "/deep".repeat(2000));
    assert_eq!(v.fit_start(&huge, s, whole.width / 2.0).0.text, *text);
    // Room for nothing but the ellipsis.
    let (para, _) = v.fit_start(long, s, 1.0);
    assert_eq!(para.text, "…");
    // Short enough: as it is.
    let (para, _) = v.fit_start("~/a/", s, 1e6);
    assert_eq!(para.text, "~/a/");
}
