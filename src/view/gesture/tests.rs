use std::time::Duration;

use super::*;
use crate::doc::Doc;
use crate::fonts::Fonts;
use crate::select::BlockSel;
use crate::view::{Action, Outcome, toolbar};

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

/// Window coordinates of the middle of cluster `k` on line `line` of block `b`.
fn at(v: &View, b: usize, line: usize, k: usize) -> (f64, f64) {
    let (x, y) = cluster(v, b, line, k, 0.5);
    (x, y)
}

/// A point `frac` of the way along cluster `k`.
fn cluster(v: &View, b: usize, line: usize, k: usize, frac: f32) -> (f64, f64) {
    let content = v.layout().geoms()[b].content.as_deref().unwrap();
    let l = &content.lines[line];
    let h = &content.hits[l.hits.start as usize + k];
    let x = v.layout().column().0 + h.x0 + (h.x1 - h.x0) * frac;
    let y = v.layout().y(b) + (l.top + l.bottom) / 2.0;
    (
        f64::from(x),
        f64::from(y) - v.scroll() + f64::from(v.top_inset()),
    )
}

fn doc_of(v: &View, b: usize, line: usize, k: usize) -> u32 {
    let content = v.layout().geoms()[b].content.as_deref().unwrap();
    content.hits[content.lines[line].hits.start as usize + k].doc
}

fn selected_text(v: &View) -> String {
    let (lo, hi) = v.selection().expect("something is selected");
    assert_eq!(lo.block, hi.block);
    v.doc().text[lo.off as usize..hi.off as usize].to_owned()
}

/// Every cluster the selection paints, as (block, offset).
fn painted(v: &View) -> Vec<(u32, u32)> {
    let Some(range) = v.selection() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (b, g) in v.layout().geoms().iter().enumerate() {
        let (Some(sel), Some(c)) = (BlockSel::of(b as u32, range), g.content.as_deref()) else {
            continue;
        };
        out.extend(
            c.hits
                .iter()
                .filter(|h| sel.contains(h.doc))
                .map(|h| (b as u32, h.doc)),
        );
    }
    out
}

fn drag(v: &mut View, from: (f64, f64), to: (f64, f64), now: Instant) {
    v.press(from.0, from.1, now);
    v.select_to(to.0, to.1, now);
}

#[test]
fn a_press_that_stays_within_the_threshold_is_a_click() {
    let now = Instant::now();
    let mut v = view("hello world", 800, 300);
    let (x, y) = at(&v, 0, 0, 2);
    assert!(!v.press(x, y, now), "nothing was selected to clear");
    assert!(!v.select_to(x + 3.0, y + 1.0, now), "within 4 px");
    assert!(!v.selecting());
    assert!(!v.release_select(), "a click makes no selection");
    assert_eq!(v.selection(), None);
    // Past the threshold it is a drag, and the movement before it counts.
    let now = now + Duration::from_secs(2);
    v.press(x, y, now);
    assert!(v.select_to(x + 8.0, y, now));
    assert!(v.selecting());
    assert!(v.release_select());
}

#[test]
fn the_threshold_is_in_logical_pixels_not_zoom() {
    let now = Instant::now();
    let mut v = view("hello world", 800, 300);
    v.set_zoom(200);
    let (x, y) = at(&v, 0, 0, 2);
    v.press(x, y, now);
    assert!(!v.select_to(x + 3.9, y, now), "zoom doesn't widen it");
    assert!(v.select_to(x + 4.1, y, now));
    // The window's scale does.
    let mut v = view("hello world", 800, 300);
    v.set_hidpi(2.0);
    v.ensure();
    let (x, y) = at(&v, 0, 0, 2);
    v.press(x, y, now);
    assert!(!v.select_to(x + 7.9, y, now), "4 logical px at 2x");
    assert!(v.select_to(x + 8.1, y, now));
}

#[test]
fn a_drag_selects_from_the_press_to_the_pointer() {
    let now = Instant::now();
    let mut v = view("hello world and more", 800, 300);
    let (from, to) = (cluster(&v, 0, 0, 0, 0.25), cluster(&v, 0, 0, 4, 0.75));
    drag(&mut v, from, to, now);
    assert_eq!(selected_text(&v), "hello");
    // On to the next word, and back past the start.
    let to = cluster(&v, 0, 0, 10, 0.75);
    assert!(v.select_to(to.0, to.1, now));
    assert_eq!(selected_text(&v), "hello world");
    let to = cluster(&v, 0, 0, 0, 0.25);
    v.select_to(to.0, to.1, now);
    assert_eq!(v.selection(), None, "back at the start: nothing selected");
    assert!(!v.release_select());
}

#[test]
fn a_press_clears_the_selection_and_dragging_backwards_works() {
    let now = Instant::now();
    let mut v = view("hello world and more", 800, 300);
    let (from, to) = (cluster(&v, 0, 0, 10, 0.75), cluster(&v, 0, 0, 6, 0.25));
    drag(&mut v, from, to, now);
    assert_eq!(selected_text(&v), "world");
    v.release_select();
    let (x, y) = at(&v, 0, 0, 1);
    assert!(
        v.press(x, y, now + Duration::from_secs(2)),
        "repaint: cleared"
    );
    assert_eq!(v.selection(), None);
}

#[test]
fn a_double_click_selects_a_word_and_a_drag_goes_on_by_words() {
    let t = Instant::now();
    let mut v = view("alpha beta gamma delta", 800, 300);
    let (x, y) = at(&v, 0, 0, 7);
    assert!(!v.press(x, y, t));
    v.release_select();
    assert!(
        v.press(x, y, t + Duration::from_millis(200)),
        "the word is selected"
    );
    assert_eq!(selected_text(&v), "beta");
    assert!(v.release_select(), "completed: worth claiming PRIMARY");
    // Third press (a triple click) takes the paragraph.
    v.press(x, y, t + Duration::from_millis(400));
    let (lo, hi) = v.selection().unwrap();
    assert_eq!((lo, hi), (Pos::start(0), Pos::end(0)));
    v.release_select();
    // Dragging after a double click extends by whole words.
    let t = t + Duration::from_secs(5);
    v.press(x, y, t);
    v.release_select();
    v.press(x, y, t + Duration::from_millis(100));
    let to = cluster(&v, 0, 0, 18, 0.25);
    assert!(v.select_to(to.0, to.1, t + Duration::from_millis(110)));
    assert_eq!(selected_text(&v), "beta gamma delta");
}

#[test]
fn clicks_count_only_within_the_interval_and_the_distance() {
    let t = Instant::now();
    let mut v = view("alpha beta gamma delta", 800, 300);
    let (x, y) = at(&v, 0, 0, 7);
    let sec = Duration::from_millis;
    v.press(x, y, t);
    v.release_select();
    // Too late: a first click again, which clears nothing and selects nothing.
    assert!(!v.press(x, y, t + sec(501)));
    assert_eq!(v.selection(), None);
    v.release_select();
    // In time but too far away.
    assert!(!v.press(x + 10.0, y, t + sec(600)));
    assert_eq!(v.selection(), None);
    v.release_select();
    // Within both: a double click.
    assert!(v.press(x + 10.0, y + 1.0, t + sec(900)));
    assert!(v.selection().is_some());
    v.release_select();
    // A fourth press begins again.
    v.press(x + 10.0, y, t + sec(1000));
    v.release_select();
    v.press(x + 10.0, y, t + sec(1100));
    assert_eq!(v.selection(), None, "the fourth click is a single one");
}

#[test]
fn shift_click_extends_the_selection_from_its_anchor() {
    let now = Instant::now();
    let mut v = view("alpha beta gamma delta", 800, 300);
    let (from, to) = (cluster(&v, 0, 0, 0, 0.25), cluster(&v, 0, 0, 4, 0.75));
    drag(&mut v, from, to, now);
    v.release_select();
    v.set_shift(true);
    let far = cluster(&v, 0, 0, 15, 0.75);
    let later = now + Duration::from_secs(3);
    assert!(v.press(far.0, far.1, later));
    assert_eq!(selected_text(&v), "alpha beta gamma");
    assert!(v.release_select());
    // Without a selection, Shift+click is a plain click.
    v.set_shift(false);
    let (x, y) = at(&v, 0, 0, 1);
    v.press(x, y, later + Duration::from_secs(3));
    assert_eq!(v.selection(), None);
    v.set_shift(true);
    v.press(far.0, far.1, later + Duration::from_secs(6));
    assert_eq!(v.selection(), None);
}

#[test]
fn no_selection_gesture_on_a_link_follows_it() {
    let t = Instant::now();
    let src = "see [the link](#x) and more\n\n# x\n";
    let mut v = view(src, 800, 300);
    let link = at(&v, 0, 0, 5);
    let (from, to) = (cluster(&v, 0, 0, 0, 0.25), cluster(&v, 0, 0, 2, 0.75));
    drag(&mut v, from, to, t);
    v.release_select();
    // Shift+click on a link extends the selection and doesn't follow it.
    v.set_shift(true);
    let later = t + Duration::from_secs(3);
    assert!(v.press(link.0, link.1, later));
    assert!(v.release_select());
    assert_eq!(v.click(link.0, link.1), None);
    assert!(v.selection().is_some_and(|(_, hi)| hi.off > 4));
    v.set_shift(false);
    // The second and third clicks of a multiple click that began on a link follow nothing, and
    // select nothing.
    let mut v = view(src, 800, 300);
    for i in 0..3u64 {
        v.press(link.0, link.1, t + Duration::from_millis(100 * i));
        v.release_select();
        let followed = v.click(link.0, link.1);
        assert_eq!(followed.is_some(), i == 0, "click {}", i + 1);
        assert_eq!(v.selection(), None);
    }
}

#[test]
fn a_drag_that_starts_on_a_link_selects_and_doesnt_follow_it() {
    let now = Instant::now();
    let mut v = view("see [the link](#x) and more\n\n# x\n", 800, 300);
    let link = doc_of(&v, 0, 0, 4);
    assert_eq!(v.doc().text.as_bytes()[link as usize], b't');
    let (x, y) = at(&v, 0, 0, 5);
    // A click follows it.
    v.press(x, y, now);
    v.release_select();
    assert_eq!(v.click(x, y).as_deref(), Some("#x"));
    // A drag from it selects, and the release is no click.
    let (from, to) = (cluster(&v, 0, 0, 5, 0.5), cluster(&v, 0, 0, 20, 0.5));
    let later = now + Duration::from_secs(2);
    drag(&mut v, from, to, later);
    assert!(v.selection().is_some());
    assert!(v.release_select());
    assert_eq!(v.click(to.0, to.1), None);
    // The pointer keeps the text cursor over the link while a drag runs.
    let mut v = view("see [the link](#x) and more\n\n# x\n", 800, 300);
    v.press(from.0, from.1, later);
    v.select_to(to.0, to.1, later);
    v.hover_at(to.0, to.1, later);
    assert_eq!(v.hovered(), None);
    assert!(v.text_cursor(to.0, to.1));
}

#[test]
fn a_double_click_on_a_link_follows_it_once_and_selects_nothing() {
    let t = Instant::now();
    let mut v = view("see [the link](#x) and more\n\n# x\n", 800, 300);
    let (x, y) = at(&v, 0, 0, 5);
    v.press(x, y, t);
    v.release_select();
    assert_eq!(
        v.click(x, y).as_deref(),
        Some("#x"),
        "the first click follows"
    );
    v.press(x, y, t + Duration::from_millis(150));
    v.release_select();
    assert_eq!(v.click(x, y), None, "not again");
    assert_eq!(v.selection(), None, "and no word of it is selected");
}

#[test]
fn the_toolbar_and_the_scrollbar_start_no_selection() {
    let now = Instant::now();
    let mut v = view(&paras(200), 600, 400);
    v.set_toolbar(true);
    settle(&mut v);
    let (from, to) = (at(&v, 0, 0, 2), at(&v, 0, 0, 12));
    v.press(from.0, from.1, now);
    v.select_to(to.0, to.1, now);
    v.release_select();
    let before = v.selection();
    assert!(before.is_some());
    // The toolbar takes the press, keeps the selection, and a drag from it selects nothing.
    assert!(v.press(20.0, 16.0, now + Duration::from_secs(2)));
    assert!(!v.select_to(300.0, 100.0, now));
    assert!(!v.selecting());
    assert_eq!(v.selection(), before);
    v.release_toolbar(20.0, 16.0);
    // The scrollbar's thumb too.
    let t = v.thumb_at(v.scroll()).unwrap();
    let (tx, ty) = (f64::from(t.x0 + t.x1) / 2.0, f64::from(t.y0 + t.y1) / 2.0);
    assert!(v.press(tx, ty, now + Duration::from_secs(4)));
    assert!(v.dragging());
    v.select_to(tx - 100.0, ty + 40.0, now);
    assert!(!v.selecting());
    assert_eq!(v.selection(), before);
    v.release(now);
}

#[test]
fn a_press_that_closes_the_dropdown_starts_no_selection() {
    let now = Instant::now();
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
    // A press on the document closes it (the toolbar takes it) and begins no gesture.
    let (x, y) = at(&v, 0, 0, 3);
    assert!(
        v.press(x, y, now + Duration::from_secs(1)),
        "taken by the toolbar"
    );
    v.select_to(x + 50.0, y, now);
    assert_eq!(v.selection(), None);
}

#[test]
fn zoom_resize_and_wide_layout_keep_the_selection() {
    let now = Instant::now();
    let src = "Intro paragraph with some words.\n\n- item one\n- item two\n\n```\ncode here\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nTail text.";
    let mut v = view(src, 700, 500);
    settle(&mut v);
    let last = v.doc().blocks.len() as u32 - 1;
    let from = cluster(&v, 0, 0, 3, 0.25);
    let n = v.layout().geoms()[0].content.as_ref().unwrap().hits.len();
    assert!(n > 5);
    v.press(from.0, from.1, now);
    v.select_to(from.0 + 40.0, from.1 + 300.0, now);
    v.release_select();
    let sel = v.selection().unwrap();
    assert!(sel.1.block > 3);
    let all: Vec<(u32, u32)> = painted(&v);
    assert!(!all.is_empty());
    let docs = |v: &View| {
        let mut p = painted(v);
        p.sort_unstable();
        p
    };
    let start = docs(&v);
    for step in 0..7 {
        match step {
            0 => v.apply(Action::ZoomIn, now),
            1 => v.apply(Action::ZoomOut, now),
            2 => v.apply(Action::ZoomOut, now),
            3 => {
                v.resize(300, 500);
                Outcome::Redraw
            }
            4 => v.apply(Action::ToggleWide, now),
            5 => {
                v.resize(700, 500);
                v.apply(Action::ZoomReset, now)
            }
            // The fonts change (a face found for a missing character is the same): everything
            // is measured again.
            _ => {
                v.set_family(false, None);
                Outcome::Redraw
            }
        };
        assert_eq!(
            v.selection(),
            Some(sel),
            "step {step}: the value is unchanged"
        );
    }
    assert_eq!(last, v.doc().blocks.len() as u32 - 1);
    // Back at the first width and zoom: the same clusters are painted. (The blocks near the
    // viewport are the only ones with content, which is every block of this document.)
    settle(&mut v);
    v.ensure();
    assert_eq!(docs(&v), start);
}

#[test]
fn a_new_document_or_a_reload_clears_the_selection_and_the_drag() {
    let now = Instant::now();
    for reopen in [false, true] {
        let mut v = view("alpha beta gamma", 800, 300);
        let (from, to) = (cluster(&v, 0, 0, 0, 0.25), cluster(&v, 0, 0, 8, 0.75));
        drag(&mut v, from, to, now);
        assert!(v.selection().is_some() && v.selecting());
        let doc = Doc::parse("completely different text");
        if reopen {
            v.open(doc, None);
        } else {
            v.replace(doc, None);
        }
        v.ensure();
        assert_eq!(v.selection(), None);
        assert!(!v.selecting(), "the drag was cancelled");
        assert!(!v.select_to(to.0, to.1, now));
        assert!(!v.release_select());
    }
}

#[test]
fn escape_closes_the_dropdown_then_clears_the_selection_then_quits() {
    let now = Instant::now();
    let mut v = view(&paras(20), 200, 300);
    v.set_toolbar(true);
    let more = toolbar::slots(&v.toolbar_ctx())
        .items
        .iter()
        .find(|(i, _)| *i == toolbar::Item::More)
        .unwrap()
        .1;
    let (mx, my) = (f64::from((more.x0 + more.x1) / 2.0), 16.0);
    let (from, to) = (cluster(&v, 0, 0, 0, 0.25), cluster(&v, 0, 0, 5, 0.75));
    drag(&mut v, from, to, now);
    v.release_select();
    assert!(v.selection().is_some());
    v.press(mx, my, now + Duration::from_secs(2));
    v.release_toolbar(mx, my);
    assert_eq!(v.apply(Action::Escape, now), Outcome::Redraw, "the menu");
    assert!(v.selection().is_some(), "the selection waits");
    assert_eq!(
        v.apply(Action::Escape, now),
        Outcome::Redraw,
        "the selection"
    );
    assert_eq!(v.selection(), None);
    assert_eq!(v.apply(Action::Escape, now), Outcome::Quit);
    assert_eq!(v.apply(Action::Quit, now), Outcome::Quit);
}

#[test]
fn select_all_takes_the_whole_document_at_any_size() {
    let now = Instant::now();
    let mut v = view(&paras(3000), 600, 400);
    assert_eq!(v.apply(Action::SelectAll, now), Outcome::Selected);
    let n = v.doc().blocks.len() as u32;
    assert_eq!(v.selection(), Some((Pos::start(0), Pos::end(n - 1))));
    // Painting only reads the blocks on screen.
    let mut buf = vec![0; 600 * 400];
    v.paint(&mut buf, 600, now);
    assert!(!painted(&v).is_empty());
    assert!(
        painted(&v)
            .iter()
            .all(|&(b, _)| (b as usize) < v.layout().geoms().len())
    );
    // Nothing to select in an empty document or a message.
    let mut v = View::new(Fonts::new().unwrap(), Doc::default(), None, (600, 400), 1.0);
    assert_eq!(v.apply(Action::SelectAll, now), Outcome::Nothing);
    let mut v = View::new(
        Fonts::new().unwrap(),
        Doc::parse("text"),
        Some("A message".into()),
        (600, 400),
        1.0,
    );
    assert_eq!(v.apply(Action::SelectAll, now), Outcome::Nothing);
    assert_eq!(v.selection(), None);
}

#[test]
fn ctrl_a_is_select_all() {
    use winit::keyboard::{Key, ModifiersState, SmolStr};
    let a = Key::Character(SmolStr::new("a"));
    assert_eq!(
        crate::view::key_action(&a, ModifiersState::CONTROL),
        Some(Action::SelectAll)
    );
    assert_eq!(
        crate::view::key_action(&a, ModifiersState::SUPER),
        Some(Action::SelectAll)
    );
    assert_eq!(crate::view::key_action(&a, ModifiersState::empty()), None);
}

#[test]
fn a_drag_outside_the_area_scrolls_at_the_frame_rate_and_stops_with_the_button() {
    let t = Instant::now();
    let ms = Duration::from_millis;
    let mut v = view(&paras(400), 600, 300);
    v.set_toolbar(true);
    settle(&mut v);
    let from = at(&v, 3, 0, 2);
    v.press(from.0, from.1, t);
    // Inside the area: no timer for it.
    v.select_to(from.0 + 20.0, from.1 + 10.0, t);
    let idle = v.deadline();
    assert!(idle.is_none_or(|d| d != t + ms(16)));
    // Below the window.
    let below = f64::from(v.height) + 30.0;
    v.select_to(from.0 + 20.0, below, t);
    assert_eq!(v.deadline(), Some(t + ms(16)), "one frame from the press");
    let before = v.scroll();
    assert!(!v.expire(t + ms(10)), "not due yet");
    assert!(v.expire(t + ms(16)), "a step");
    let one = v.scroll() - before;
    assert!(one > 0.0);
    assert_eq!(v.deadline(), Some(t + ms(32)));
    // The focus follows: the selection reaches further down.
    let end = v.selection().unwrap().1;
    assert!(end.block > 3);
    // The farther the pointer, the faster.
    let mut far = view(&paras(400), 600, 300);
    far.set_toolbar(true);
    settle(&mut far);
    let from = at(&far, 3, 0, 2);
    far.press(from.0, from.1, t);
    far.select_to(from.0 + 20.0, f64::from(far.height) + 150.0, t);
    let before = far.scroll();
    far.expire(t + ms(16));
    assert!(
        far.scroll() - before > 5.0 * one,
        "{} vs {one}",
        far.scroll() - before
    );
    // A long frame is cut short.
    let before = v.scroll();
    v.expire(t + ms(32 + 5000));
    assert!(v.scroll() - before < 4000.0 * 0.11);
    // The pointer comes back inside: no more steps.
    v.select_to(from.0, f64::from(v.height) - 40.0, t + ms(100));
    assert!(
        v.deadline().is_none_or(|d| d > t + ms(1000)),
        "{:?}",
        v.deadline()
    );
    // Out again, then the button goes up.
    v.select_to(from.0, below, t + ms(200));
    assert!(v.deadline().is_some_and(|d| d <= t + ms(300)));
    v.release_select();
    assert!(v.deadline().is_none_or(|d| d > t + ms(1000)));
    assert!(!v.expire(t + ms(300)), "no press, no scrolling");
}

#[test]
fn a_drag_above_the_area_scrolls_up_and_the_toolbar_strip_counts() {
    let t = Instant::now();
    let ms = Duration::from_millis;
    let mut v = view(&paras(400), 600, 300);
    v.set_toolbar(true);
    settle(&mut v);
    v.set_scroll(v.layout().top(100));
    v.ensure();
    let start = v.scroll();
    let from = at(&v, 103, 0, 2);
    v.press(from.0, from.1, t);
    // In the toolbar's strip: above the area.
    v.select_to(from.0, 10.0, t);
    assert_eq!(v.deadline(), Some(t + ms(16)));
    v.expire(t + ms(16));
    assert!(v.scroll() < start);
    assert!(v.selection().unwrap().0.block < 103);
}

#[test]
fn the_edge_band_scrolls_in_a_maximized_window() {
    let t = Instant::now();
    let mut v = view(&paras(400), 600, 300);
    settle(&mut v);
    let from = at(&v, 3, 0, 2);
    v.press(from.0, from.1, t);
    // The last row of pixels is inside the area, and still scrolls.
    v.select_to(from.0, 299.0, t);
    assert!(v.deadline().is_some());
    let before = v.scroll();
    v.expire(t + Duration::from_millis(16));
    assert!(v.scroll() > before);
    // 4 px in is not at the edge.
    v.select_to(from.0, 296.0, t + Duration::from_millis(20));
    assert!(v.deadline().is_none_or(|d| d > t + Duration::from_secs(1)));
}

#[test]
fn scrolling_stops_at_the_end_of_the_document() {
    let t = Instant::now();
    let ms = Duration::from_millis;
    let mut v = view(&paras(40), 600, 300);
    settle(&mut v);
    v.apply(Action::Bottom, t);
    let from = at(&v, 38, 0, 2);
    v.press(from.0, from.1, t);
    v.select_to(from.0, 400.0, t);
    assert!(v.deadline().is_some());
    v.expire(t + ms(16));
    // Nothing moved: nothing more to wait for, so an idle window sleeps.
    assert!(
        v.deadline().is_none_or(|d| d > t + ms(1000)),
        "{:?}",
        v.deadline()
    );
}

#[test]
fn the_wheel_moves_the_focus_under_a_held_pointer() {
    let t = Instant::now();
    let mut v = view(&paras(400), 600, 300);
    settle(&mut v);
    let from = at(&v, 3, 0, 2);
    v.press(from.0, from.1, t);
    v.select_to(from.0 + 30.0, from.1 + 40.0, t);
    let before = v.selection().unwrap().1;
    v.wheel_lines(5.0, t);
    let after = v.selection().unwrap().1;
    assert!(after.block > before.block, "{before:?} → {after:?}");
}

#[test]
fn the_text_cursor_is_over_text_only() {
    let now = Instant::now();
    let mut v = view("hello world\n\n[a link](#x) here\n\n# x\n", 800, 300);
    v.set_toolbar(true);
    let (tx, ty) = at(&v, 0, 0, 3);
    v.hover_at(tx, ty, now);
    assert!(v.text_cursor(tx, ty), "over text");
    // Past the end of the line, in the margin and over the toolbar: the arrow.
    assert!(!v.text_cursor(tx + 400.0, ty));
    assert!(!v.text_cursor(5.0, ty));
    assert!(!v.text_cursor(tx, 10.0), "toolbar strip");
    // A link keeps its hand.
    let (lx, ly) = at(&v, 1, 0, 3);
    v.hover_at(lx, ly, now);
    assert_eq!(v.hovered(), Some("#x"));
    assert!(!v.text_cursor(lx, ly));
}

#[test]
fn the_count_cycles_through_a_triple_click() {
    let mut s = Select::default();
    let t = Instant::now();
    let at = (10.0, 10.0);
    let counts: Vec<u8> = (0..5)
        .map(|i| s.count(t + Duration::from_millis(100 * i), at, 4.0))
        .collect();
    assert_eq!(counts, [1, 2, 3, 1, 2]);
    s.cancel();
    assert_eq!(s.count(t + Duration::from_millis(600), at, 4.0), 1);
}

#[test]
fn a_selection_is_painted_under_the_text_and_only_while_it_exists() {
    let now = Instant::now();
    let mut v = view("hello world and more text", 800, 300);
    let mut buf = vec![0; 800 * 300];
    let count = |buf: &[u32], color| buf.iter().filter(|&&p| p == color).count();
    v.paint(&mut buf, 800, now);
    assert_eq!(count(&buf, crate::theme::LIGHT.selection), 0);
    let (from, to) = (cluster(&v, 0, 0, 0, 0.25), cluster(&v, 0, 0, 4, 0.75));
    drag(&mut v, from, to, now);
    v.paint(&mut buf, 800, now);
    let shown = count(&buf, crate::theme::LIGHT.selection);
    assert!(shown > 500, "{shown} pixels of the selection color");
    v.apply(Action::Escape, now);
    v.paint(&mut buf, 800, now);
    assert_eq!(count(&buf, crate::theme::LIGHT.selection), 0);
}

#[test]
fn the_copy_keys() {
    use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};
    let c = Key::Character(SmolStr::new("c"));
    let (ctrl, shift) = (ModifiersState::CONTROL, ModifiersState::SHIFT);
    let key = crate::view::key_action;
    assert_eq!(key(&c, ctrl), Some(Action::Copy));
    assert_eq!(
        key(&c, ctrl | shift),
        Some(Action::Copy),
        "a terminal's habit"
    );
    assert_eq!(key(&c, ModifiersState::SUPER), Some(Action::Copy));
    assert_eq!(
        key(&Key::Character(SmolStr::new("C")), ctrl | shift),
        Some(Action::Copy)
    );
    assert_eq!(key(&Key::Named(NamedKey::Insert), ctrl), Some(Action::Copy));
    assert_eq!(
        key(&Key::Named(NamedKey::Copy), ModifiersState::empty()),
        Some(Action::Copy)
    );
    // Plain `c` and Insert alone are no shortcuts.
    assert_eq!(key(&c, ModifiersState::empty()), None);
    assert_eq!(
        key(&Key::Named(NamedKey::Insert), ModifiersState::empty()),
        None
    );
    assert_eq!(
        key(&Key::Named(NamedKey::Insert), shift),
        None,
        "Shift+Insert pastes"
    );
}

/// Window coordinates of the middle of the cluster at `off` of the text, in block `b`.
fn at_offset(v: &View, b: usize, off: u32) -> (f64, f64) {
    let content = v.layout().geoms()[b].content.as_deref().unwrap();
    let (l, h) = content
        .lines
        .iter()
        .find_map(|l| {
            let hits = &content.hits[l.hits.start as usize..l.hits.end as usize];
            hits.iter().find(|h| h.doc == off).map(|h| (l, h))
        })
        .expect("the offset is drawn");
    (
        f64::from(v.layout().column().0 + (h.x0 + h.x1) / 2.0),
        f64::from(v.layout().y(b) + (l.top + l.bottom) / 2.0) - v.scroll()
            + f64::from(v.top_inset()),
    )
}

/// Double click on the character that starts at `off` of block `b`, and what a copy gives.
fn double_click_copy(v: &mut View, b: usize, off: u32) -> String {
    let (x, y) = at_offset(v, b, off);
    let t = Instant::now() + Duration::from_secs(10);
    v.press(x, y, t);
    v.release_select();
    v.press(x, y, t + Duration::from_millis(200));
    v.release_select();
    v.copy_content().unwrap().chunks().collect()
}

#[test]
fn a_double_click_in_a_sparse_table_copies_the_word_alone() {
    let mut v = view("| | beta | |\n|---|---|---|\n| | bravo | |\n", 800, 300);
    let word = |v: &View, w: &str| v.doc().text.find(w).unwrap() as u32;
    // A word in the second column, after an empty first cell, in a row that is the first block
    // of the document (its text starts at offset 0) and in one that isn't.
    let (beta, bravo) = (word(&v, "beta"), word(&v, "bravo"));
    assert_eq!(beta, 0);
    assert_eq!(double_click_copy(&mut v, 0, beta + 1), "beta");
    assert_eq!(double_click_copy(&mut v, 1, bravo + 1), "bravo");
    // A triple click takes the row whole, from its first column to its last.
    let (x, y) = at_offset(&v, 1, bravo + 1);
    let t = Instant::now() + Duration::from_secs(20);
    for ms in [0, 100, 200] {
        v.press(x, y, t + Duration::from_millis(ms));
        v.release_select();
    }
    let row = v.copy_content().unwrap().chunks().collect::<String>();
    assert_eq!(row, "\tbravo\t");
    // And select all gives the whole table.
    v.apply(Action::SelectAll, t);
    let all = v.copy_content().unwrap().chunks().collect::<String>();
    assert_eq!(all, "\tbeta\t\n\tbravo\t");
}

#[test]
fn copying_gives_the_selected_text_and_does_nothing_without_a_selection() {
    let now = Instant::now();
    let mut v = view("alpha beta gamma\n\n- item one\n- item two\n", 800, 300);
    assert_eq!(
        v.apply(Action::Copy, now),
        Outcome::Nothing,
        "nothing selected: no copy"
    );
    assert!(v.copy_content().is_none());
    let (from, to) = (cluster(&v, 0, 0, 6, 0.25), cluster(&v, 0, 0, 9, 0.75));
    drag(&mut v, from, to, now);
    assert_eq!(v.apply(Action::Copy, now), Outcome::Copy);
    let Some(Content::Text(text)) = v.copy_content() else {
        panic!("a short selection is its text");
    };
    assert_eq!(&*text, "beta");
    // Select all, and the bullets are there; the view's document is shared, not copied.
    v.apply(Action::SelectAll, now);
    let text = v.copy_content().unwrap().chunks().collect::<String>();
    assert_eq!(text, "alpha beta gamma\n\n- item one\n- item two");
    // After a reload the copy still gives what was copied.
    let content = v.copy_content().unwrap();
    v.replace(Doc::parse("something else entirely"), None);
    assert_eq!(content.chunks().collect::<String>(), text);
    assert!(
        v.copy_content().is_none(),
        "the selection went with the old document"
    );
}
