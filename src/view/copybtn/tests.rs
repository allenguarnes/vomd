use std::time::Duration;

use super::*;
use crate::doc::Doc;
use crate::fonts::Fonts;
use crate::layout::Anchor;
use crate::theme::{DARK, LIGHT};
use crate::view::{ThemeMode, Wake};

fn view(src: &str, w: u32, h: u32) -> View {
    let mut v = View::new(Fonts::new().unwrap(), Doc::parse(src), None, (w, h), 1.0);
    v.ensure();
    v
}

fn settle(v: &mut View) {
    while !v.idle(Instant::now() + Duration::from_secs(1)) {}
    v.ensure();
}

const DOC: &str = "Intro paragraph.\n\n```rust\nfn main() {\n\tlet x = 1;\n\n    // above is blank\n}\n```\n\nClosing paragraph.\n";

/// The first code block of the document.
fn code_block(v: &View) -> usize {
    v.doc()
        .blocks
        .iter()
        .position(|b| b.kind == BlockKind::Code)
        .expect("a code block")
}

/// The window point in the middle of block `b`'s code rectangle (which is not on the button), or
/// a little below its top if it is tall.
fn in_code(v: &View, b: usize) -> (f64, f64) {
    let c = v.layout().geoms()[b].content.as_deref().unwrap();
    let (rx, ry, rw, rh) = code_rect(c).unwrap();
    let (col_x, _) = v.layout().column();
    (
        f64::from(col_x + rx + rw / 2.0),
        v.layout().top(b) + f64::from(ry + (rh / 2.0).min(60.0)) - v.scroll()
            + f64::from(v.top_inset()),
    )
}

/// The button of the code block at window point `at`.
fn button_at(v: &View, at: (f64, f64)) -> Rect {
    v.code_at(at.0, at.1).expect("a button there").button
}

fn middle(r: Rect) -> (f64, f64) {
    (f64::from(r.x0 + r.x1) / 2.0, f64::from(r.y0 + r.y1) / 2.0)
}

/// How many pixels of block `b`'s button are green (the check mark's color) in a frame painted
/// at `now`, a 640 × 480 window.
fn green_in_button(v: &mut View, b: usize, now: Instant) -> usize {
    let mut buf = vec![0; 640 * 480];
    v.paint(&mut buf, 640, now);
    let (x0, y0, x1, y1) = button_at(v, in_code(v, b)).px();
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = buf[y as usize * 640 + x as usize];
            let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
            g > r + 20 && g > b + 20
        })
        .count()
}

/// A view of `src` with the pointer on its first code block's button.
fn on_button(src: &str, w: u32, h: u32) -> (View, (f64, f64), usize) {
    let mut v = view(src, w, h);
    settle(&mut v);
    let b = code_block(&v);
    let at = middle(button_at(&v, in_code(&v, b)));
    (v, at, b)
}

#[test]
fn the_button_shows_over_a_code_block_and_nowhere_else() {
    let mut v = view(DOC, 640, 480);
    settle(&mut v);
    let b = code_block(&v);
    let (x, y) = in_code(&v, b);
    assert!(!v.over_copy());
    assert!(v.copy.shown.is_none());
    // On the paragraphs: nothing.
    assert!(!v.hover(100.0, 8.0));
    assert!(v.copy.shown.is_none());
    // Inside the block: the button shows (a repaint), not lit.
    assert!(v.hover(x, y), "entering the block changes the picture");
    assert_eq!(
        v.copy.shown,
        Some(Spot {
            block: b as u32,
            hot: false,
            down: false
        })
    );
    assert!(!v.over_copy());
    assert!(
        !v.hover(x + 3.0, y),
        "moving inside the block changes nothing"
    );
    // On the button: lit, and the hand.
    let at = middle(button_at(&v, (x, y)));
    assert!(v.hover(at.0, at.1));
    assert!(v.over_copy());
    assert!(
        !v.text_cursor(at.0, at.1),
        "not the text cursor over a button"
    );
    // Back to the block, then off it, then out of the window.
    assert!(v.hover(x, y));
    assert!(!v.over_copy());
    assert!(v.hover(x, 8.0), "leaving the block hides the button");
    assert!(v.copy.shown.is_none());
    v.hover(x, y);
    assert!(v.hover_end(), "the pointer left the window");
    assert!(v.copy.shown.is_none());
}

#[test]
fn the_pointer_off_the_block_sideways_hides_the_button() {
    let mut v = view(DOC, 1400, 480);
    settle(&mut v);
    let b = code_block(&v);
    let (x, y) = in_code(&v, b);
    let (col_x, col_w) = v.layout().column();
    assert!(col_x > 100.0, "the column is centered in a wide window");
    v.hover(x, y);
    assert!(v.copy.shown.is_some());
    // In the margin right of the code block (the block ends at the column's edge).
    v.hover(f64::from(col_x + col_w) + 20.0, y);
    assert!(v.copy.shown.is_none());
    v.hover(f64::from(col_x) - 20.0, y);
    assert!(v.copy.shown.is_none());
}

#[test]
fn the_button_sits_in_the_upper_right_corner_at_every_zoom_and_scale() {
    for (zoom, hidpi) in [
        (100, 1.0),
        (50, 1.0),
        (150, 1.0),
        (300, 1.0),
        (100, 2.0),
        (200, 1.5),
    ] {
        let mut v = View::new(
            Fonts::new().unwrap(),
            Doc::parse(DOC),
            None,
            (900, 700),
            hidpi,
        );
        v.set_zoom(zoom);
        settle(&mut v);
        let b = code_block(&v);
        let r = button_at(&v, in_code(&v, b));
        let s = (hidpi * f64::from(zoom) / 100.0) as f32;
        let (col_x, col_w) = v.layout().column();
        let c = v.layout().geoms()[b].content.as_deref().unwrap();
        let (rx, ry, rw, _) = code_rect(c).unwrap();
        let (left, top) = (col_x + rx, (v.layout().top(b) - v.scroll()) as f32 + ry);
        let what = format!("zoom {zoom}, scale {hidpi}");
        assert!(
            (r.x1 - r.x0 - SIZE * s).abs() < 0.01,
            "{what}: width {}",
            r.x1 - r.x0
        );
        assert!((r.y1 - r.y0 - SIZE * s).abs() < 0.01, "{what}: height");
        assert!(
            (left + rw - r.x1 - INSET * s).abs() < 0.01,
            "{what}: {} from the right edge",
            left + rw - r.x1
        );
        assert!(
            (r.y0 - top - INSET * s).abs() < 0.01,
            "{what}: {} from the top",
            r.y0 - top
        );
        assert_eq!(
            left + rw,
            col_x + col_w,
            "{what}: the block ends at the column"
        );
    }
}

#[test]
fn a_block_in_a_list_or_quote_has_its_button_at_its_own_corner() {
    let src = "```\ntop\n```\n\n- item\n\n  ```\n  in a list\n  ```\n\n> ```\n> in a quote\n> ```\n\n> - a\n>\n>   ```\n>   both\n>   ```\n";
    let mut v = view(src, 700, 900);
    settle(&mut v);
    let blocks: Vec<usize> = (0..v.doc().blocks.len())
        .filter(|&i| v.doc().blocks[i].kind == BlockKind::Code)
        .collect();
    assert_eq!(blocks.len(), 4);
    let mut lefts = Vec::new();
    for &b in &blocks {
        let at = in_code(&v, b);
        let r = button_at(&v, at);
        let c = v.layout().geoms()[b].content.as_deref().unwrap();
        let (rx, ry, rw, _) = code_rect(c).unwrap();
        let (col_x, _) = v.layout().column();
        // Inside its own block, at its right edge and its top, whatever holds it.
        assert!((col_x + rx + rw - r.x1 - INSET).abs() < 0.01, "block {b}");
        let top = (v.layout().top(b) - v.scroll()) as f32 + ry;
        assert!((r.y0 - top - INSET).abs() < 0.01, "block {b}");
        assert!(
            f64::from(r.x0) > f64::from(col_x + rx),
            "block {b}: inside the block"
        );
        lefts.push(col_x + rx);
        // It is the block's, not a neighbor's: pressing it asks for that block's code.
        let mut v2 = view(src, 700, 900);
        settle(&mut v2);
        v2.hover(at.0, at.1);
        let m = middle(r);
        assert!(v2.press(m.0, m.1, Instant::now()));
        let got = v2.release_copy(m.0, m.1).expect("a click");
        assert_eq!(got.block as usize, b);
    }
    assert!(
        lefts[1] > lefts[0] && lefts[2] > lefts[0] && lefts[3] > lefts[2],
        "{lefts:?}"
    );
}

#[test]
fn a_block_too_small_to_hold_the_button_has_none() {
    let now = Instant::now();
    // Narrow: the block's width less the margins can't hold the button.
    let mut narrow = view(DOC, 60, 480);
    settle(&mut narrow);
    let b = code_block(&narrow);
    let at = in_code(&narrow, b);
    assert!(narrow.code_at(at.0, at.1).is_none(), "60 px wide");
    narrow.hover(at.0, at.1);
    assert!(narrow.copy.shown.is_none());
    narrow.press(at.0, at.1, now);
    assert!(narrow.release_copy(at.0, at.1).is_none());
    // The same block in a window wide enough has it.
    let mut wide = view(DOC, 640, 480);
    settle(&mut wide);
    let at = in_code(&wide, b);
    assert!(wide.code_at(at.0, at.1).is_some());
    // Short: with a font this small a one-line block is shorter than the button and its margins.
    let mut small = view("```\none\n```\n", 640, 480);
    small.set_text(6.0, 80.0);
    settle(&mut small);
    let at = in_code(&small, 0);
    assert!(small.code_at(at.0, at.1).is_none());
    small.set_text(16.0, 80.0);
    settle(&mut small);
    let at = in_code(&small, 0);
    assert!(small.code_at(at.0, at.1).is_some());
}

#[test]
fn a_code_block_with_nothing_in_it_has_no_button() {
    let mut v = view("```\n```\n\ntext\n", 640, 480);
    settle(&mut v);
    let at = in_code(&v, 0);
    assert!(v.code_at(at.0, at.1).is_none());
}

#[test]
fn zoom_changes_the_button_with_the_block() {
    let (mut v, _, b) = on_button(DOC, 900, 700);
    let size = |v: &View| {
        let r = button_at(v, in_code(v, b));
        r.x1 - r.x0
    };
    let at100 = size(&v);
    v.set_zoom(200);
    settle(&mut v);
    let at200 = size(&v);
    assert!((at200 - 2.0 * at100).abs() < 0.01, "{at100} {at200}");
    v.set_zoom(50);
    settle(&mut v);
    assert!((size(&v) - 0.5 * at100).abs() < 0.01);
}

#[test]
fn a_click_on_the_button_asks_for_the_blocks_code() {
    let now = Instant::now();
    let (mut v, at, b) = on_button(DOC, 640, 480);
    v.hover(at.0, at.1);
    assert!(v.press(at.0, at.1, now), "the button takes the press");
    assert_eq!(v.copy.shown.map(|s| s.down), Some(true));
    let copy = v.release_copy(at.0, at.1).expect("released on it: a click");
    assert_eq!(copy.block as usize, b);
    let text: String = copy.content.chunks().collect();
    assert_eq!(
        text,
        "fn main() {\n\tlet x = 1;\n\n    // above is blank\n}"
    );
    assert!(v.release_copy(at.0, at.1).is_none(), "one click per press");
    // Pressed and released apart: no click.
    v.press(at.0, at.1, now);
    assert!(
        v.release_copy(at.0 - 60.0, at.1).is_none(),
        "released off the button"
    );
    v.press(at.0, at.1, now);
    v.cancel_copy();
    assert!(
        v.release_copy(at.0, at.1).is_none(),
        "the press was cancelled"
    );
    // Pressed beside the button and released on it: no click either.
    let beside = in_code(&v, b);
    assert!(!v.press(beside.0, beside.1, now));
    assert!(v.release_copy(at.0, at.1).is_none());
}

#[test]
fn a_click_on_the_button_leaves_the_selection_and_the_links_alone() {
    let now = Instant::now();
    let src = "A [link](https://example.com/x) here.\n\n```\nsome code\n```\n\nMore text.\n";
    let mut v = view(src, 640, 480);
    settle(&mut v);
    let b = code_block(&v);
    let at = middle(button_at(&v, in_code(&v, b)));
    // Press a link, select everything (the press of the link stays), then click the button.
    let content = v.layout().geoms()[0].content.as_deref().unwrap();
    let hit = content
        .hits
        .iter()
        .find(|h| content.styles[h.style_index()].link != 0)
        .unwrap();
    let (lx, ly) = (
        f64::from(v.layout().column().0 + (hit.x0 + hit.x1) / 2.0),
        f64::from((content.lines[0].top + content.lines[0].bottom) / 2.0),
    );
    assert!(v.link_at(lx, ly).is_some(), "the link is where it was put");
    v.press(lx, ly, now);
    assert!(v.pressed.is_some());
    assert!(v.select_all());
    let before = v.selection();
    assert!(before.is_some());
    assert!(v.press(at.0, at.1, now + Duration::from_secs(1)));
    assert_eq!(v.pressed, None, "the click isn't on a link");
    assert_eq!(v.click(at.0, at.1), None);
    assert_eq!(v.selection(), before, "the selection is as it was");
    // A drag that starts on the button selects nothing and scrolls nothing.
    assert!(!v.select_to(at.0 + 30.0, at.1 + 30.0, now));
    assert!(!v.selecting());
    assert!(!v.release_select(), "nothing to offer as PRIMARY");
    assert!(
        v.release_copy(at.0 + 30.0, at.1 + 30.0).is_none(),
        "released elsewhere"
    );
    // With nothing selected, a click on the button selects nothing either.
    v.clear_selection();
    assert!(v.press(at.0, at.1, now + Duration::from_secs(2)));
    assert_eq!(v.selection(), None);
    assert!(!v.release_select());
    assert!(v.release_copy(at.0, at.1).is_some());
    assert_eq!(v.selection(), None);
}

#[test]
fn clicking_the_button_twice_isnt_a_double_click_that_selects_a_word() {
    let now = Instant::now();
    let (mut v, at, _) = on_button(DOC, 640, 480);
    for n in 0..3 {
        let t = now + Duration::from_millis(100 * n);
        assert!(v.press(at.0, at.1, t));
        assert!(v.release_copy(at.0, at.1).is_some());
        assert_eq!(v.selection(), None, "click {n}");
    }
}

#[test]
fn the_toolbar_covers_the_button_where_they_meet() {
    let now = Instant::now();
    let src = format!(
        "{}\n\n{DOC}\n\n{}",
        "text\n\n".repeat(60),
        "more\n\n".repeat(60)
    );
    let mut v = view(&src, 640, 480);
    v.set_toolbar(true);
    settle(&mut v);
    let b = code_block(&v);
    v.set_scroll(v.layout().top(b) - 100.0);
    settle(&mut v);
    // Scrolled until the block's top is 20 px above the document area: the button is half
    // under the toolbar.
    let c = v.layout().geoms()[b].content.as_deref().unwrap();
    let (_, ry, _, _) = code_rect(c).unwrap();
    v.set_scroll(v.layout().top(b) + f64::from(ry) + 20.0);
    settle(&mut v);
    let top = f64::from(v.top_inset());
    let x = f64::from(v.layout().column().0 + v.layout().column().1) - 8.0 - 14.0;
    // The visible half of the button is a button; the half under the toolbar is the toolbar's.
    let (below, above) = ((x, top + 6.0), (x, top - 6.0));
    assert!(v.code_at(below.0, below.1).is_some());
    v.hover_at(below.0, below.1, now);
    assert!(v.over_copy(), "on the visible part");
    v.hover_at(above.0, above.1, now);
    assert!(!v.over_copy(), "not on the toolbar");
    assert!(v.copy.shown.is_none());
    v.press(above.0, above.1, now);
    assert!(v.release_copy(above.0, above.1).is_none());
    // With the block's top further up there is no button to hit at all.
    v.set_scroll(v.layout().top(b) + f64::from(ry) + 200.0);
    settle(&mut v);
    v.hover_at(below.0, below.1, now);
    assert!(v.copy.shown.is_none());
}

#[test]
fn the_button_follows_the_block_as_it_scrolls_and_goes_with_it() {
    let src = format!(
        "{}\n\n{DOC}\n\n{}",
        "text\n\n".repeat(30),
        "more\n\n".repeat(60)
    );
    let mut v = view(&src, 640, 300);
    settle(&mut v);
    let b = code_block(&v);
    v.set_scroll(v.layout().top(b) - 40.0);
    settle(&mut v);
    let (x, y) = in_code(&v, b);
    v.hover(x, y);
    let first = button_at(&v, (x, y));
    assert!(v.copy.shown.is_some());
    // The wheel: the button moves up with the block, and the pointer is still over it.
    let now = Instant::now();
    v.scroll_by(20.0, now);
    v.hover(x, y);
    let moved = button_at(&v, (x, y));
    assert!(
        (first.y0 - moved.y0 - 20.0).abs() < 0.5,
        "{first:?} {moved:?}"
    );
    assert!(v.copy.shown.is_some());
    // Scrolled until the pointer is over a paragraph: the button is gone.
    v.scroll_by(600.0, now);
    settle(&mut v);
    assert!(v.hover(x, y), "the button went");
    assert!(v.copy.shown.is_none());
    assert!(v.code_at(x, y).is_none());
}

#[test]
fn a_block_taller_than_the_window_keeps_its_button_at_its_top() {
    // GitHub's choice: the button is at the top of the block and scrolls away with it.
    let now = Instant::now();
    let code: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let src = format!("Before.\n\n```\n{code}```\n\nAfter.\n");
    let mut v = view(&src, 640, 300);
    settle(&mut v);
    let b = code_block(&v);
    let (x, y) = in_code(&v, b);
    v.hover(x, y);
    let before = button_at(&v, (x, y));
    // A little down: the button went up by as much, the pointer being over the block still.
    v.scroll_by(30.0, now);
    settle(&mut v);
    v.hover(x, y);
    let later = button_at(&v, (x, y));
    assert!(
        (before.y0 - later.y0 - 30.0).abs() < 0.5,
        "{before:?} {later:?}"
    );
    // Further, the block's top is out of the window: the pointer is over the block, with no button.
    v.scroll_by(400.0, now);
    settle(&mut v);
    assert!(
        v.code_at(300.0, 150.0).is_none(),
        "no button for a block whose top is gone"
    );
    v.hover(300.0, 150.0);
    assert!(v.copy.shown.is_none());
    assert!(!v.over_copy());
    // And back up, it is where it was.
    v.scroll_by(-430.0, now);
    settle(&mut v);
    v.hover(x, y);
    let back = button_at(&v, (x, y));
    assert!((back.y0 - before.y0).abs() < 0.5 && (back.y1 - before.y1).abs() < 0.5);
}

#[test]
fn a_huge_block_has_one_button_at_the_top_of_its_first_chunk() {
    // Chunks of 16 KB each, one block to the reader.
    let code: String = (0..30_000)
        .map(|i| format!("line {i} of the code\n"))
        .collect();
    let src = format!("Before.\n\n```\n{code}```\n\nAfter.\n");
    let mut v = view(&src, 640, 400);
    settle(&mut v);
    let chunks: Vec<usize> = (0..v.doc().blocks.len())
        .filter(|&i| v.doc().blocks[i].kind == BlockKind::Code)
        .collect();
    assert!(chunks.len() > 20, "{} chunks", chunks.len());
    let first = chunks[0];
    let (x, y) = in_code(&v, first);
    let from_first = button_at(&v, (x, y));
    // Down in the middle of the block, with the top far above the window: the pointer is over
    // a later chunk, and there is no button to show.
    let mid = v.layout().top(chunks[10]);
    v.set_scroll(mid);
    settle(&mut v);
    let (x, y) = in_code(&v, chunks[10]);
    assert!(v.code_at(x, y).is_none());
    // The copy a click would ask for (from the top) is the whole block, however far it goes.
    v.set_scroll(0.0);
    settle(&mut v);
    let at = middle(from_first);
    assert!(v.press(at.0, at.1, Instant::now()));
    let copy = v.release_copy(at.0, at.1).unwrap();
    assert_eq!(copy.block as usize, first);
    assert!(
        matches!(copy.content, Content::Lazy { code: true, .. }),
        "not made at once"
    );
    assert_eq!(copy.content.size_hint(), code.len() - 1);
    assert_eq!(
        copy.content.chunks().collect::<String>(),
        code.trim_end_matches('\n')
    );
}

#[test]
fn a_block_in_a_big_document_works_the_same() {
    // Over 1 MB: parsed in segments, laid out near the viewport only.
    let filler = "A paragraph of plain words that fills the document up to a size.\n\n";
    let code = "if x {\n\ty()\n\n    z()\n}";
    let src = format!(
        "{}```go\n{code}\n```\n\n{}",
        filler.repeat(10_000),
        filler.repeat(10_000)
    );
    assert!(src.len() > 1_000_000 / 2);
    let src = format!("{}{src}", filler.repeat(15_000));
    assert!(src.len() > 1_000_000);
    let mut v = view(&src, 640, 480);
    let b = code_block(&v);
    v.set_anchor(Anchor {
        block: b as u32 - 2,
        frac: 0.0,
    });
    v.ensure();
    let at = in_code(&v, b);
    v.hover(at.0, at.1);
    let m = middle(button_at(&v, at));
    assert!(v.press(m.0, m.1, Instant::now()));
    let copy = v.release_copy(m.0, m.1).unwrap();
    assert_eq!(copy.content.chunks().collect::<String>(), code);
}

#[test]
fn the_check_mark_shows_for_two_seconds_then_the_copy_icon_is_back() {
    let now = Instant::now();
    let (mut v, at, b) = on_button(DOC, 640, 480);
    v.hover(at.0, at.1);
    let ink = |v: &mut View, t: Instant| green_in_button(v, b, t);
    assert_eq!(v.copy_deadline(), None, "no timer before a copy");
    assert_eq!(ink(&mut v, now), 0, "the copy icon isn't green");
    v.code_copied(b as u32, now);
    assert_eq!(v.copy_deadline(), Some(now + CHECK));
    assert_eq!(CHECK, Duration::from_secs(2));
    assert_eq!(
        v.deadline(),
        Some(now + CHECK),
        "the event loop wakes for it"
    );
    assert!(
        ink(&mut v, now + Duration::from_millis(1900)) > 10,
        "the check mark shows"
    );
    assert!(!v.expire(now + Duration::from_millis(1999)));
    assert!(v.expire(now + CHECK), "its time is up: repaint");
    assert_eq!(v.copy_deadline(), None);
    assert_eq!(v.deadline(), None, "nothing more to wake for");
    assert_eq!(ink(&mut v, now + CHECK), 0, "the copy icon is back");
    assert!(!v.expire(now + CHECK + Duration::from_secs(1)), "once");
    // The loop is told to wake at the deadline, and sleeps after it.
    v.code_copied(b as u32, now);
    assert_eq!(v.tick(now, None).wake, Wake::At(now + CHECK));
    let tick = v.tick(now + CHECK, None);
    assert!(tick.redraw);
    assert_eq!(tick.wake, Wake::Event);
}

#[test]
fn the_check_mark_belongs_to_the_block_that_was_copied() {
    let now = Instant::now();
    let src = "```\none\n```\n\n```\ntwo\n```\n";
    let mut v = view(src, 640, 480);
    settle(&mut v);
    let blocks: Vec<usize> = (0..v.doc().blocks.len())
        .filter(|&i| v.doc().blocks[i].kind == BlockKind::Code)
        .collect();
    v.code_copied(blocks[0] as u32, now);
    let checked = |v: &mut View, b: usize| {
        let at = in_code(v, b);
        v.hover(at.0, at.1);
        green_in_button(v, b, now) > 10
    };
    assert!(checked(&mut v, blocks[0]));
    assert!(
        !checked(&mut v, blocks[1]),
        "the other block's button is the copy icon"
    );
}

#[test]
fn a_new_document_forgets_the_buttons_state() {
    let now = Instant::now();
    let (mut v, at, b) = on_button(DOC, 640, 480);
    v.hover(at.0, at.1);
    v.press(at.0, at.1, now);
    v.code_copied(b as u32, now);
    v.replace(Doc::parse("Plain text only.\n"), None);
    assert_eq!(v.copy_deadline(), None);
    assert!(v.release_copy(at.0, at.1).is_none());
    v.hover(at.0, at.1);
    assert!(v.copy.shown.is_none());
    // (The pointer is remembered: the next document's block under it gets its button.)
    v.open(Doc::parse(DOC), None);
    settle(&mut v);
    v.hover(at.0, at.1);
    assert!(v.copy.shown.is_some());
}

#[test]
fn nothing_is_drawn_for_the_button_when_the_pointer_is_elsewhere() {
    let now = Instant::now();
    let mut v = view(DOC, 640, 480);
    settle(&mut v);
    let mut plain = vec![0; 640 * 480];
    v.paint(&mut plain, 640, now);
    let (x, y) = in_code(&v, code_block(&v));
    v.hover(100.0, 8.0);
    let mut elsewhere = vec![0; 640 * 480];
    v.paint(&mut elsewhere, 640, now);
    assert_eq!(plain, elsewhere);
    v.hover(x, y);
    let mut over = vec![0; 640 * 480];
    v.paint(&mut over, 640, now);
    assert_ne!(plain, over, "the button is drawn over the block");
    // Only the button's own rectangle differs.
    let r = button_at(&v, (x, y));
    let (x0, y0, x1, y1) = r.px();
    for (i, (a, b)) in plain.iter().zip(&over).enumerate() {
        let (px, py) = ((i % 640) as i32, (i / 640) as i32);
        if a != b {
            assert!(
                (x0..x1).contains(&px) && (y0..y1).contains(&py),
                "({px}, {py})"
            );
        }
    }
}

#[test]
fn the_button_is_readable_in_both_themes_and_lights_up() {
    let now = Instant::now();
    for (mode, theme) in [(ThemeMode::Light, &LIGHT), (ThemeMode::Dark, &DARK)] {
        let (mut v, at, b) = on_button(DOC, 640, 480);
        v.set_theme_mode(mode);
        let (x, y) = in_code(&v, b);
        let r = button_at(&v, (x, y));
        let (x0, y0, _, _) = r.px();
        let mut buf = vec![0; 640 * 480];
        v.hover(x, y);
        v.paint(&mut buf, 640, now);
        // Inside the button's edge (a pixel in from a corner's rounding): the page's color,
        // against the code's own.
        let (px, py) = (x0 + (r.x1 - r.x0) as i32 / 2, y0 + 2);
        assert_eq!(buf[py as usize * 640 + px as usize], theme.bg, "resting");
        assert_ne!(theme.bg, theme.code_bg);
        v.hover(at.0, at.1);
        v.paint(&mut buf, 640, now);
        let lit = buf[py as usize * 640 + px as usize];
        assert_ne!(lit, theme.bg, "lit under the pointer");
        v.press(at.0, at.1, now);
        v.paint(&mut buf, 640, now);
        let down = buf[py as usize * 640 + px as usize];
        assert_ne!(down, lit, "pressed is lit more");
    }
}

#[test]
fn the_wide_layout_moves_the_button_with_the_column() {
    let mut v = view(DOC, 1600, 600);
    let b = code_block(&v);
    let mut rights = Vec::new();
    for wide in [false, true] {
        v.set_wide(wide);
        settle(&mut v);
        let r = button_at(&v, in_code(&v, b));
        let (col_x, col_w) = v.layout().column();
        // In the block's corner, whatever the column is.
        assert!((col_x + col_w - r.x1 - INSET).abs() < 0.01, "wide {wide}");
        rights.push(r.x1);
    }
    assert!(
        rights[1] > rights[0] + 100.0,
        "{rights:?}: the wide column reaches further"
    );
}
