use std::time::{Duration, Instant};

use super::*;
use crate::doc::Doc;
use crate::find::{self, MAX_QUERY};
use crate::fonts::Fonts;
use crate::theme::LIGHT;
use crate::view::{Action, key_action};
use winit::keyboard::SmolStr;

const NONE: ModifiersState = ModifiersState::empty();
const CTRL: ModifiersState = ModifiersState::CONTROL;
const SHIFT: ModifiersState = ModifiersState::SHIFT;

fn view(src: &str, w: u32, h: u32) -> View {
    let mut v = View::new(Fonts::new().unwrap(), Doc::parse(src), None, (w, h), 1.0);
    v.ensure();
    v
}

fn settle(v: &mut View) {
    while !v.idle(Instant::now() + Duration::from_secs(1)) {}
    v.ensure();
}

fn paint(v: &mut View, now: Instant) -> Vec<u32> {
    let (w, h) = v.size();
    let mut buf = vec![0; (w * h) as usize];
    v.paint(&mut buf, w as usize, now);
    buf
}

fn named(key: NamedKey) -> Key {
    Key::Named(key)
}

fn ch(s: &str) -> Key {
    Key::Character(SmolStr::new(s))
}

/// A key press as the shell handles it: the find bar first, then the shortcuts.
fn press(v: &mut View, key: Key, text: Option<&str>, mods: ModifiersState) -> Outcome {
    let now = Instant::now();
    if let Some(outcome) = v.find_key(&key, text, mods, now) {
        return outcome;
    }
    match key_action(&key, mods) {
        Some(action) => v.apply(action, now),
        None => Outcome::Nothing,
    }
}

/// Type `text` a character at a time.
fn typed(v: &mut View, text: &str) {
    for c in text.chars() {
        let s = c.to_string();
        let key = if c == ' ' {
            named(NamedKey::Space)
        } else {
            ch(&s)
        };
        press(v, key, Some(&s), NONE);
    }
}

fn enter(v: &mut View, mods: ModifiersState) {
    press(v, named(NamedKey::Enter), Some("\r"), mods);
}

/// `n` paragraphs of filler, the 1-based ones in `words` holding the word after them.
fn big(n: usize, words: &[(usize, &str)]) -> String {
    (1..=n)
        .map(|i| {
            let word = words.iter().find(|w| w.0 == i).map_or("", |w| w.1);
            format!(
                "Paragraph {i} {word} has filler text that goes on and on, so that it wraps \
                 across a few lines of the window, and takes some height of its own.\n\n"
            )
        })
        .collect()
}

fn open(v: &mut View) {
    v.apply(Action::Find, Instant::now());
}

const CTRL_F: &str = "\u{6}";

#[test]
fn ctrl_f_opens_and_closes_the_bar() {
    assert_eq!(key_action(&ch("f"), CTRL), Some(Action::Find));
    assert_eq!(key_action(&ch("F"), CTRL | SHIFT), Some(Action::Find));
    assert_eq!(key_action(&ch("f"), NONE), None);
    let mut v = view("# Title\n\nSome text.\n", 800, 600);
    assert!(!v.find_open());
    assert_eq!(v.top_inset(), 0);
    assert_eq!(press(&mut v, ch("f"), Some(CTRL_F), CTRL), Outcome::Redraw);
    assert!(v.find_open() && v.find_focused());
    assert_eq!(v.top_inset(), 32);
    typed(&mut v, "abc");
    assert_eq!(v.find_query(), "abc");
    // Again: closed, with the field focused.
    press(&mut v, ch("f"), Some(CTRL_F), CTRL);
    assert!(!v.find_open() && !v.find_focused());
    assert_eq!(v.top_inset(), 0);
    // And once more: open, the query kept and shown selected, so typing replaces it.
    press(&mut v, ch("f"), Some(CTRL_F), CTRL);
    assert!(v.find_open() && v.find_focused());
    assert_eq!(v.find_selected(), "abc");
    typed(&mut v, "x");
    assert_eq!(v.find_query(), "x");
    // Typing goes on at the caret afterwards.
    typed(&mut v, "y");
    assert_eq!(v.find_query(), "xy");
}

#[test]
fn ctrl_f_closes_the_bar_whoever_has_the_keyboard() {
    let mut v = view("text\n", 800, 600);
    open(&mut v);
    // The document has it (a click on the text): Ctrl+F still closes the bar.
    v.press(400.0, 300.0, Instant::now());
    v.release(Instant::now());
    assert!(v.find_open() && !v.find_focused());
    press(&mut v, ch("F"), Some(CTRL_F), CTRL | SHIFT);
    assert!(!v.find_open());
    // The toolbar's button does the same, and either way the bar opens with the keyboard.
    v.apply(Action::Find, Instant::now());
    assert!(v.find_open() && v.find_focused());
    v.apply(Action::Find, Instant::now());
    assert!(!v.find_open());
}

#[test]
fn the_bar_sits_below_the_toolbar_and_scales_with_the_display_not_the_zoom() {
    let mut v = view("text\n", 800, 600);
    v.set_toolbar(true);
    assert_eq!(v.top_inset(), 32);
    open(&mut v);
    assert_eq!(v.top_inset(), 64);
    assert_eq!(v.area_height(), 600 - 64);
    v.set_zoom(200);
    assert_eq!(v.top_inset(), 64, "zoom does not scale chrome");
    v.set_hidpi(2.0);
    assert_eq!(v.top_inset(), 128);
    assert_eq!(v.find_height(), 64);
    // The bar is the strip between the toolbar and the document.
    let r = v.find_rect();
    assert_eq!((r.y0, r.y1), (64.0, 128.0));
    v.set_toolbar(false);
    assert_eq!(v.top_inset(), 64, "at the top without the toolbar");
    assert_eq!(v.find_rect().y0, 0.0);
    v.set_find(false);
    assert_eq!(v.top_inset(), 0);
}

#[test]
fn opening_and_closing_keep_the_reading_position() {
    let src = big(80, &[]);
    let mut v = view(&src, 800, 500);
    v.set_anchor(Anchor {
        block: 30,
        frac: 0.4,
    });
    let before = v.anchor();
    open(&mut v);
    assert_eq!(v.anchor(), before);
    v.set_find(false);
    assert_eq!(v.anchor(), before);
}

#[test]
fn the_toolbar_button_toggles_the_bar() {
    let mut v = view("text\n", 800, 600);
    v.set_toolbar(true);
    let ctx = v.toolbar_ctx();
    assert!(!ctx.find);
    // Its slot: the one before ⚙ is not there (no settings), so it is the last of the group.
    let slots = toolbar::slots(&ctx);
    let (_, rect) = *slots
        .items
        .iter()
        .find(|(i, _)| *i == toolbar::Item::Find)
        .expect("a find button");
    let (x, y) = (
        f64::from(rect.x0 + rect.x1) / 2.0,
        f64::from(rect.y0 + rect.y1) / 2.0,
    );
    let now = Instant::now();
    assert!(v.press(x, y, now));
    assert!(v.release_toolbar(x, y));
    let command = v.take_command().expect("a click");
    assert_eq!(command, toolbar::Command::Action(Action::Find));
    let toolbar::Command::Action(action) = command else {
        panic!()
    };
    v.apply(action, now);
    assert!(v.find_open() && v.toolbar_ctx().find);
    v.apply(action, now);
    assert!(!v.find_open() && !v.toolbar_ctx().find);
}

#[test]
fn letters_go_to_the_query_while_the_bar_is_open() {
    let mut v = view("# One\n\n## Two\n\nSome text q c j k.\n", 800, 600);
    open(&mut v);
    for (c, name) in [("q", "quit"), ("c", "contents"), ("j", "down"), ("k", "up")] {
        assert_eq!(
            press(&mut v, ch(c), Some(c), NONE),
            Outcome::Redraw,
            "{name}"
        );
    }
    assert_eq!(v.find_query(), "qcjk");
    #[cfg(feature = "toc")]
    assert!(!v.toc_open(), "c typed, not the contents key");
    // Shifted letters, symbols and the keys that were shortcuts are text too.
    typed(&mut v, "[]tbwrgGi I");
    assert_eq!(v.find_query(), "qcjk[]tbwrgGi I");
    assert!(!v.toolbar_visible());
    assert_eq!(v.zoom(), 100);
    // Without the bar they are shortcuts again.
    v.set_find(false);
    assert_eq!(press(&mut v, ch("q"), Some("q"), NONE), Outcome::Quit);
}

#[test]
fn shortcuts_with_ctrl_and_keys_that_are_no_text_still_reach_the_document() {
    let src = big(60, &[]);
    let mut v = view(&src, 800, 600);
    open(&mut v);
    typed(&mut v, "p");
    // Zoom, quit and open work on the document.
    assert_eq!(
        press(&mut v, ch("="), Some("\u{1d}"), CTRL),
        Outcome::Redraw
    );
    assert_eq!(v.zoom(), 110);
    assert_eq!(press(&mut v, ch("-"), Some("-"), CTRL), Outcome::Redraw);
    assert_eq!(v.zoom(), 100);
    assert_eq!(press(&mut v, ch("w"), Some("\u{17}"), CTRL), Outcome::Quit);
    assert_eq!(press(&mut v, ch("o"), Some("\u{f}"), CTRL), Outcome::Open);
    assert_eq!(v.find_query(), "p", "no control character was typed");
    // Up, Down and the page keys scroll; Home, End and the sideways arrows are the field's.
    let top = v.scroll();
    press(&mut v, named(NamedKey::PageDown), None, NONE);
    assert!(v.scroll() > top);
    let down = v.scroll();
    press(&mut v, named(NamedKey::ArrowDown), None, NONE);
    assert!(v.scroll() > down);
    press(&mut v, named(NamedKey::ArrowUp), None, NONE);
    assert_eq!(v.scroll(), down);
    press(&mut v, named(NamedKey::PageUp), None, NONE);
    assert_eq!(v.scroll(), top);
    for key in [
        NamedKey::End,
        NamedKey::Home,
        NamedKey::ArrowLeft,
        NamedKey::ArrowRight,
    ] {
        press(&mut v, named(key), None, NONE);
        assert_eq!(v.scroll(), top, "{key:?} does not scroll");
    }
    // The query is what it was.
    assert_eq!(v.find_query(), "p");
    // Space is text, not a page.
    let at = v.scroll();
    typed(&mut v, " ");
    assert_eq!(v.find_query(), "p ");
    assert_eq!(v.scroll(), at);
    // A key with no text (a modifier alone, an unmapped key) does nothing.
    assert_eq!(
        press(&mut v, named(NamedKey::Shift), None, SHIFT),
        Outcome::Nothing
    );
    assert_eq!(
        press(&mut v, named(NamedKey::Tab), Some("\t"), NONE),
        Outcome::Nothing
    );
    assert_eq!(v.find_query(), "p ");
}

#[test]
fn backspace_deletes_a_character_and_ctrl_backspace_a_word() {
    let mut v = view("one two three\n", 800, 600);
    open(&mut v);
    typed(&mut v, "one two  three");
    assert_eq!(
        press(&mut v, named(NamedKey::Backspace), None, NONE),
        Outcome::Redraw
    );
    assert_eq!(v.find_query(), "one two  thre");
    press(&mut v, named(NamedKey::Backspace), None, CTRL);
    assert_eq!(v.find_query(), "one two  ");
    press(&mut v, named(NamedKey::Backspace), None, CTRL);
    assert_eq!(v.find_query(), "one ");
    press(&mut v, named(NamedKey::Backspace), None, CTRL);
    assert_eq!(v.find_query(), "");
    // Nothing left to delete, and the key is not Back.
    assert_eq!(
        press(&mut v, named(NamedKey::Backspace), None, NONE),
        Outcome::Redraw
    );
    // A character is a `char`: a multi-byte one goes whole.
    typed(&mut v, "añ😀");
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    assert_eq!(v.find_query(), "añ");
    // A query shown selected goes whole.
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    assert_eq!(v.find_query(), "");
    // Without the bar, Backspace goes back.
    v.set_find(false);
    assert_eq!(
        press(&mut v, named(NamedKey::Backspace), None, NONE),
        Outcome::Back
    );
}

#[test]
fn the_query_is_at_most_256_characters_and_has_no_control_characters() {
    let mut v = view("text\n", 800, 600);
    open(&mut v);
    v.find_type(&"a".repeat(300));
    assert_eq!(v.find_query().chars().count(), MAX_QUERY);
    v.find_type("more");
    assert_eq!(v.find_query().chars().count(), MAX_QUERY);
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    v.find_type("é😀");
    assert_eq!(v.find_query().chars().count(), MAX_QUERY);
    assert!(v.find_query().ends_with('é'));
    // Control characters (a pasted tab, a newline) are left out of what is committed.
    let mut v = view("text\n", 800, 600);
    open(&mut v);
    v.find_type("a\tb\nc\u{7f}d");
    assert_eq!(v.find_query(), "abcd");
    // An empty commit leaves the query as it is, selected or not.
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    v.find_type("");
    v.find_type("\t");
    assert_eq!(v.find_query(), "abcd");
    v.find_type("x");
    assert_eq!(v.find_query(), "x");
    // An input method's commit types into the open bar only.
    v.set_find(false);
    v.find_type("zzz");
    assert_eq!(v.find_query(), "x");
}

#[test]
fn matches_are_counted_and_stepped_around_the_ends() {
    let mut v = view("foo one\n\nFoo two\n\nxx FOO three\n\nnothing\n", 800, 600);
    assert_eq!(v.find_count(), "");
    open(&mut v);
    assert_eq!(v.find_count(), "", "an empty query says nothing");
    typed(&mut v, "foo");
    assert_eq!(v.find_matches().len(), 3);
    assert_eq!(
        (v.find_current(), v.find_count().as_str()),
        (Some(0), "1 / 3")
    );
    enter(&mut v, NONE);
    assert_eq!(v.find_count(), "2 / 3");
    enter(&mut v, NONE);
    enter(&mut v, NONE);
    assert_eq!(v.find_count(), "1 / 3", "next wraps");
    enter(&mut v, SHIFT);
    assert_eq!(v.find_count(), "3 / 3", "previous wraps");
    press(&mut v, named(NamedKey::F3), None, NONE);
    assert_eq!(v.find_count(), "1 / 3");
    press(&mut v, named(NamedKey::F3), None, SHIFT);
    assert_eq!(v.find_count(), "3 / 3");
    press(&mut v, ch("g"), Some("\u{7}"), CTRL);
    assert_eq!(v.find_count(), "1 / 3");
    press(&mut v, ch("G"), Some("\u{7}"), CTRL | SHIFT);
    assert_eq!(v.find_count(), "3 / 3");
    // Stepping does not edit the query.
    assert_eq!(v.find_query(), "foo");
    typed(&mut v, "x");
    assert_eq!(v.find_count(), "No matches");
    assert_eq!(v.find_current(), None);
    // Stepping with no match does nothing.
    enter(&mut v, NONE);
    assert_eq!(v.find_count(), "No matches");
}

#[test]
fn the_count_is_of_every_match_though_a_window_of_them_is_kept() {
    let src = "a ".repeat(crate::find::WINDOW + 5);
    let mut v = view(&src, 800, 600);
    open(&mut v);
    typed(&mut v, "a");
    assert_eq!(v.find_total(), crate::find::WINDOW + 5);
    assert_eq!(v.find_matches().len(), crate::find::WINDOW);
    assert_eq!(v.find_count(), "1 / 10005");
    enter(&mut v, SHIFT);
    assert_eq!(v.find_count(), "10005 / 10005");
}

/// `n` paragraphs of a dozen `a`s.
fn as_(n: usize) -> String {
    ("a ".repeat(12) + "\n\n").repeat(n)
}

#[test]
fn stepping_walks_through_every_match_across_the_window_edges() {
    let n = 1000;
    let total = 12 * n;
    let mut v = view(&as_(n), 800, 500);
    open(&mut v);
    typed(&mut v, "a");
    assert_eq!(
        (v.find_total(), v.find_count().as_str()),
        (total, "1 / 12000")
    );
    let number = |v: &View| {
        let f = &v.find.found;
        f.first + v.find_current().unwrap()
    };
    // Forwards, from before the window's end to well past it.
    for i in 1..crate::find::WINDOW {
        enter(&mut v, NONE);
        assert_eq!(number(&v), i);
        let m = v.find_matches()[v.find_current().unwrap()];
        assert_eq!(m.block as usize, i / 12, "match {i}");
    }
    assert_eq!(v.find_count(), "10000 / 12000");
    assert!(v.find_matches().len() <= crate::find::WINDOW);
    // Around the end to the first, and back to the last (more than a window away).
    for _ in 0..total - crate::find::WINDOW + 1 {
        enter(&mut v, NONE);
    }
    assert_eq!(v.find_count(), "1 / 12000", "next wraps from the last");
    assert_eq!(v.find_current(), Some(0));
    enter(&mut v, SHIFT);
    assert_eq!(
        v.find_count(),
        "12000 / 12000",
        "previous wraps from the first"
    );
    // Backwards across the lower edge of the window around the last.
    for i in (0..total - 1).rev().take(crate::find::WINDOW) {
        enter(&mut v, SHIFT);
        assert_eq!(number(&v), i);
        let m = v.find_matches()[v.find_current().unwrap()];
        assert_eq!(m.block as usize, i / 12, "match {i}");
    }
    assert_eq!(v.find_count(), "2000 / 12000");
}

#[test]
fn a_common_first_letter_in_a_big_document_keeps_the_reading_position() {
    let src = big(3000, &[(10, "target"), (2500, "target")]);
    let mut v = view(&src, 800, 500);
    v.set_anchor(Anchor {
        block: 2000,
        frac: 0.0,
    });
    open(&mut v);
    typed(&mut v, "e");
    assert!(v.find_total() > crate::find::WINDOW, "{}", v.find_total());
    assert!(v.find_matches().len() <= crate::find::WINDOW);
    let m = v.find_matches()[v.find_current().unwrap()];
    assert!(
        m.block >= 2000,
        "from the reading position, not block {}",
        m.block
    );
    assert!(v.anchor().block >= 1990, "the view stays: {:?}", v.anchor());
    // Typing on continues from the match it is at.
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    typed(&mut v, "target");
    let m = v.find_matches()[v.find_current().unwrap()];
    assert_eq!(
        m.block, 2499,
        "the one below the reader, not the one in block 9"
    );
    assert_eq!((v.find_total(), v.find_count().as_str()), (2, "2 / 2"));
}

#[test]
fn opening_picks_the_first_match_from_the_reading_position() {
    let src = big(40, &[(3, "zebra"), (20, "zebra"), (35, "zebra")]);
    let mut v = view(&src, 800, 500);
    open(&mut v);
    typed(&mut v, "zebra");
    // From the top: the first.
    assert_eq!(v.find_current(), Some(0));
    v.set_find(false);
    // Read on to the middle: the one at or after it.
    v.set_anchor(Anchor {
        block: 20,
        frac: 0.0,
    });
    open(&mut v);
    assert_eq!(v.find_query(), "zebra", "the query is kept");
    // The matches are in blocks 2, 19 and 34, and the position is at block 20.
    assert_eq!(v.find_current(), Some(2));
    assert_eq!(v.find_matches()[2].block, 34);
}

#[test]
fn opening_wraps_to_the_first_when_nothing_is_after() {
    let src = big(40, &[(3, "zebra"), (20, "zebra")]);
    let mut v = view(&src, 800, 500);
    open(&mut v);
    typed(&mut v, "zebra");
    v.set_find(false);
    v.set_anchor(Anchor {
        block: 30,
        frac: 0.0,
    });
    open(&mut v);
    assert_eq!(v.find_current(), Some(0));
}

#[test]
fn a_match_far_down_a_big_document_comes_into_view_a_third_of_the_way_down() {
    let src = big(600, &[(450, "zebra")]);
    let mut v = view(&src, 800, 600);
    // Nothing of it has been shaped.
    let word = v.doc().text.find("zebra").unwrap() as u32;
    let block = v
        .doc()
        .blocks
        .iter()
        .position(|b| {
            v.doc()
                .block_spans(b)
                .iter()
                .any(|s| s.start <= word && word < s.end)
        })
        .unwrap();
    assert!(v.layout().geoms()[block].content.is_none());
    open(&mut v);
    typed(&mut v, "zebra");
    assert_eq!(v.find_matches().len(), 1);
    let m = v.find_matches()[0];
    assert_eq!(m.block as usize, block);
    assert!(v.layout().geoms()[block].content.is_some());
    let (top, bottom) = v.find_extent(m).expect("the block is shaped");
    let area = f64::from(v.area_height());
    assert!(
        (top - v.scroll() - area / 3.0).abs() < 2.0,
        "match at {} of {area}",
        top - v.scroll()
    );
    assert!(bottom <= v.scroll() + area);
    // The same after the rest was measured, and when stepping back to it from elsewhere.
    settle(&mut v);
    let (top2, _) = v.find_extent(m).unwrap();
    assert!((top2 - v.scroll() - area / 3.0).abs() < 40.0, "{top2}");
    v.apply(Action::Top, Instant::now());
    enter(&mut v, NONE);
    let (top3, _) = v.find_extent(m).unwrap();
    assert!((top3 - v.scroll() - area / 3.0).abs() < 2.0 || v.scroll() == v.max_scroll());
}

#[test]
fn a_match_in_view_does_not_scroll_and_one_out_of_view_does() {
    let src = big(120, &[(2, "zebra"), (3, "zebra"), (90, "zebra")]);
    let mut v = view(&src, 800, 700);
    open(&mut v);
    typed(&mut v, "zebra");
    // The first two are on screen at the start: stepping between them leaves the page alone.
    let at = v.anchor();
    assert_eq!(v.scroll(), 0.0);
    enter(&mut v, NONE);
    assert_eq!(v.find_current(), Some(1));
    assert_eq!(v.anchor(), at);
    // The third is far below.
    enter(&mut v, NONE);
    assert_eq!(v.find_current(), Some(2));
    assert!(v.scroll() > 1000.0);
    // Scrolling by hand does not change which is current.
    v.scroll_by(-400.0, Instant::now());
    v.scroll_by(900.0, Instant::now());
    assert_eq!(v.find_current(), Some(2));
    // Wrapping back up scrolls to the first (in view at the top: the page goes to its place only
    // if the match is out of view).
    enter(&mut v, NONE);
    assert_eq!(v.find_current(), Some(0));
    let m = v.find_matches()[0];
    let (top, bottom) = v.find_extent(m).unwrap();
    let area = f64::from(v.area_height());
    assert!(top >= v.scroll() && bottom <= v.scroll() + area);
}

#[test]
fn a_match_at_the_very_end_comes_as_far_as_the_document_goes() {
    let src = format!("{}last zebra\n", big(40, &[]));
    let mut v = view(&src, 800, 500);
    open(&mut v);
    typed(&mut v, "zebra");
    let m = v.find_matches()[0];
    let (top, bottom) = v.find_extent(m).unwrap();
    let area = f64::from(v.area_height());
    // Not enough document below it to put it a third down: it is in view at the end.
    assert!(top >= v.scroll() && bottom <= v.scroll() + area);
    assert!(v.scroll() <= v.max_scroll() + 0.5);
}

#[test]
fn matches_in_a_code_block_and_a_table_are_found_and_shown() {
    let src = format!(
        "{}```\nfn main() {{\n    println!(\"zebra\");\n}}\n```\n\n| a | b |\n|---|---|\n| x | zebra |\n",
        big(50, &[])
    );
    let mut v = view(&src, 800, 500);
    open(&mut v);
    typed(&mut v, "zebra");
    assert_eq!(v.find_matches().len(), 2);
    for i in 0..2 {
        let m = v.find_matches()[v.find_current().unwrap()];
        let (top, bottom) = v
            .find_extent(m)
            .unwrap_or_else(|| panic!("match {i} has lines"));
        assert!(top >= v.scroll() - 1.0 && bottom <= v.scroll() + f64::from(v.area_height()) + 1.0);
        enter(&mut v, NONE);
    }
}

#[test]
fn escape_closes_the_bar_before_anything_else() {
    let mut v = view("# One\n\ntext\n", 800, 600);
    open(&mut v);
    typed(&mut v, "te");
    let now = Instant::now();
    // The contents panel (wide enough to push the document, so Escape would not close it) waits
    // its turn.
    #[cfg(feature = "toc")]
    v.set_toc(true);
    assert_eq!(
        press(&mut v, named(NamedKey::Escape), None, NONE),
        Outcome::Redraw
    );
    assert!(!v.find_open());
    #[cfg(feature = "toc")]
    assert!(v.toc_open(), "the panel stays");
    assert_eq!(v.find_query(), "te", "the query is kept");
    assert_eq!(v.find_matches().len(), 0, "and the matches are not");
    assert_eq!(v.find_current(), None);
    // Now Escape is what it was: quit.
    assert_eq!(v.apply(Action::Escape, now), Outcome::Quit);
    #[cfg(feature = "clipboard")]
    {
        // With a selection, the bar goes first, then the selection, then the window.
        #[cfg(feature = "toc")]
        v.set_toc(false);
        assert!(v.select_all());
        open(&mut v);
        assert_eq!(
            press(&mut v, named(NamedKey::Escape), None, NONE),
            Outcome::Redraw
        );
        assert!(!v.find_open() && v.selection().is_some());
        assert_eq!(
            press(&mut v, named(NamedKey::Escape), None, NONE),
            Outcome::Redraw
        );
        assert!(v.selection().is_none());
        assert_eq!(
            press(&mut v, named(NamedKey::Escape), None, NONE),
            Outcome::Quit
        );
    }
}

#[test]
fn closing_keeps_the_query_for_the_next_time_and_typing_replaces_it() {
    let mut v = view("foo bar foo\n", 800, 600);
    open(&mut v);
    typed(&mut v, "foo");
    v.set_find(false);
    assert!(v.find_matches().is_empty());
    open(&mut v);
    assert_eq!(v.find_query(), "foo");
    assert_eq!(v.find_matches().len(), 2, "found again on opening");
    typed(&mut v, "bar");
    assert_eq!(v.find_query(), "bar", "shown selected: typing replaces it");
    assert_eq!(v.find_matches().len(), 1);
}

#[cfg(feature = "clipboard")]
#[test]
fn a_short_selection_of_one_line_fills_the_query() {
    let mut v = view("Hello world\n\nSecond paragraph\n", 800, 600);
    // A selection of two blocks is two lines: no query from it.
    assert!(v.select_all());
    open(&mut v);
    assert_eq!(v.find_query(), "");
    v.set_find(false);
    let mut v = view("Hello world\n", 800, 600);
    assert!(v.select_all());
    open(&mut v);
    assert_eq!(v.find_query(), "Hello world");
    assert_eq!(v.find_matches().len(), 1);
    // Closed and opened again with a new selection: the query is that.
    let mut v = view("Hello world\n", 800, 600);
    open(&mut v);
    typed(&mut v, "zzz");
    v.set_find(false);
    assert!(v.select_all());
    press(&mut v, ch("f"), Some(CTRL_F), CTRL);
    assert_eq!(v.find_query(), "Hello world");
    assert_eq!(v.find_selected(), "Hello world", "shown selected");
    // A long line is not a query either.
    let long = "word ".repeat(80);
    let mut v = view(&long, 800, 600);
    assert!(v.select_all());
    open(&mut v);
    assert_eq!(v.find_query(), "");
}

#[test]
fn reload_keeps_the_bar_the_query_and_the_place() {
    let src = big(80, &[(10, "zebra"), (50, "zebra")]);
    let mut v = view(&src, 800, 500);
    open(&mut v);
    typed(&mut v, "zebra");
    enter(&mut v, NONE);
    assert_eq!(v.find_current(), Some(1));
    let at = v.anchor();
    // The same document again, and then with a match more (earlier) and one less.
    v.replace(Doc::parse(&src), None);
    assert!(v.find_open());
    assert_eq!(v.find_query(), "zebra");
    assert_eq!(v.find_matches().len(), 2);
    assert_eq!(v.anchor(), at, "nothing scrolled");
    let m = v.find_matches()[v.find_current().unwrap()];
    assert!(m.block >= at.block, "the first match from the position");
    let src2 = big(80, &[(5, "zebra"), (10, "zebra"), (50, "zebra")]);
    v.replace(Doc::parse(&src2), None);
    assert_eq!(v.find_matches().len(), 3);
    let none = big(80, &[]);
    v.replace(Doc::parse(&none), None);
    assert!(v.find_open());
    assert_eq!(v.find_count(), "No matches");
    // A read error keeps it open as well (the message is no text).
    v.replace(Doc::parse(""), Some("Cannot open x".into()));
    assert!(v.find_open());
    assert_eq!(v.find_query(), "zebra");
    assert!(v.find_matches().is_empty());
    paint(&mut v, Instant::now());
    v.replace(Doc::parse(&src), None);
    assert_eq!(v.find_matches().len(), 2);
}

#[test]
fn another_document_is_searched_from_its_top() {
    let src = big(40, &[(30, "zebra")]);
    let mut v = view(&src, 800, 500);
    open(&mut v);
    typed(&mut v, "zebra");
    let other = big(40, &[(5, "zebra"), (25, "ZEBRA"), (26, "zebra")]);
    v.open(Doc::parse(&other), None);
    assert!(v.find_open());
    assert_eq!(v.find_matches().len(), 3);
    assert_eq!(v.find_current(), Some(0));
    assert_eq!(v.anchor(), Anchor::TOP);
    v.open(Doc::parse("nothing\n"), None);
    assert_eq!(v.find_count(), "No matches");
}

#[test]
fn highlights_are_painted_behind_the_text_current_one_stronger() {
    let mut v = view("alpha beta alpha\n\ngamma alpha\n", 800, 400);
    let now = Instant::now();
    let count = |buf: &[u32], color: u32| buf.iter().filter(|&&p| p == color).count();
    let before = paint(&mut v, now);
    assert_eq!(count(&before, LIGHT.find_match), 0);
    open(&mut v);
    typed(&mut v, "alpha");
    let buf = paint(&mut v, now);
    let (soft, strong) = (
        count(&buf, LIGHT.find_match),
        count(&buf, LIGHT.find_current),
    );
    assert!(soft > 0 && strong > 0, "{soft} {strong}");
    // Two matches are soft and one strong, of about the same width each.
    assert!(
        (soft as f64 / strong as f64 - 2.0).abs() < 0.5,
        "{soft} soft, {strong} strong"
    );
    // The washes are in the document area, below the bar.
    let (w, _) = v.size();
    let top = v.top_inset() as usize;
    assert!(
        (0..top)
            .flat_map(|y| (0..w as usize).map(move |x| (x, y)))
            .all(|(x, y)| buf[y * w as usize + x] != LIGHT.find_match)
    );
    // Stepping moves the strong one.
    enter(&mut v, NONE);
    let next = paint(&mut v, now);
    assert_ne!(buf, next);
    assert_eq!(count(&next, LIGHT.find_current), strong);
    // Closing takes all of it away.
    v.set_find(false);
    let gone = paint(&mut v, now);
    assert_eq!(
        count(&gone, LIGHT.find_match) + count(&gone, LIGHT.find_current),
        0
    );
    assert_eq!(gone, before);
}

#[test]
fn the_dark_theme_has_its_own_washes() {
    let mut v = view("alpha beta alpha\n", 800, 400);
    v.set_theme_mode(crate::view::ThemeMode::Dark);
    open(&mut v);
    typed(&mut v, "alpha");
    let buf = paint(&mut v, Instant::now());
    let count = |color: u32| buf.iter().filter(|&&p| p == color).count();
    assert!(count(crate::theme::DARK.find_match) > 0);
    assert!(count(crate::theme::DARK.find_current) > 0);
    assert_eq!(count(LIGHT.find_match), 0);
}

#[cfg(feature = "clipboard")]
#[test]
fn the_selection_is_painted_over_the_matches() {
    let mut v = view("alpha beta alpha\n", 800, 400);
    open(&mut v);
    typed(&mut v, "alpha");
    assert!(v.select_all());
    let buf = paint(&mut v, Instant::now());
    let count = |color: u32| buf.iter().filter(|&&p| p == color).count();
    assert_eq!(count(LIGHT.find_match) + count(LIGHT.find_current), 0);
    assert!(count(LIGHT.selection) > 0);
}

#[test]
fn the_bar_is_painted_in_its_strip() {
    let mut v = view("alpha beta alpha\n", 800, 400);
    v.set_toolbar(true);
    open(&mut v);
    typed(&mut v, "alpha");
    let buf = paint(&mut v, Instant::now());
    let at = |x: usize, y: usize| buf[y * 800 + x];
    // The strip has the toolbar's color, the field the page's, between the toolbar and the page.
    assert_eq!(at(790, 40), LIGHT.code_bg);
    assert_eq!(at(120, 48), LIGHT.bg);
    assert_eq!(at(790, 63), LIGHT.border, "its bottom edge");
    // Text and icons are in it: some ink on both sides of the field.
    let ink = |x0: usize, x1: usize| {
        (33..64)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| at(x, y) == LIGHT.text || at(x, y) == LIGHT.muted)
            .count()
    };
    assert!(ink(8, 120) > 0, "the query");
    assert!(ink(600, 800) > 0, "the count and the buttons");
}

#[test]
fn the_buttons_step_and_close_and_the_field_keeps_the_focus() {
    let mut v = view("foo one\n\nfoo two\n\nfoo three\n", 800, 600);
    open(&mut v);
    typed(&mut v, "foo");
    let now = Instant::now();
    let [(_, close), (_, next), (_, prev)] = {
        let b = v.find_buttons();
        [b[0], b[1], b[2]]
    };
    let mid = |r: Rect| (f64::from(r.x0 + r.x1) / 2.0, f64::from(r.y0 + r.y1) / 2.0);
    let click = |v: &mut View, (x, y): (f64, f64)| {
        assert!(v.press(x, y, now), "the bar takes the press");
        assert!(v.release_toolbar(x, y), "and the release");
        assert_eq!(v.click(x, y), None);
    };
    assert_eq!(v.find_current(), Some(0));
    click(&mut v, mid(next));
    assert_eq!(v.find_current(), Some(1));
    click(&mut v, mid(next));
    click(&mut v, mid(next));
    assert_eq!(v.find_current(), Some(0));
    click(&mut v, mid(prev));
    assert_eq!(v.find_current(), Some(2));
    // A press on the field, or on the bar's empty part, does nothing and the bar stays.
    let field = (40.0, 16.0);
    click(&mut v, field);
    click(&mut v, (300.0, 16.0));
    assert!(v.find_open());
    assert_eq!(v.find_current(), Some(2));
    #[cfg(feature = "clipboard")]
    assert!(v.selection().is_none());
    // A press that moves off the button before it is released is no click.
    let (x, y) = mid(next);
    assert!(v.press(x, y, now));
    assert!(v.release_toolbar(40.0, 16.0));
    assert_eq!(v.find_current(), Some(2));
    // Close.
    click(&mut v, mid(close));
    assert!(!v.find_open());
    assert_eq!(v.top_inset(), 0);
}

#[test]
fn a_click_on_the_document_keeps_the_bar_open_and_takes_the_keyboard_from_the_field() {
    let mut v = view("foo bar\n\n[link](https://example.com)\n", 800, 600);
    open(&mut v);
    typed(&mut v, "fo");
    let now = Instant::now();
    let _ = v.press(60.0, 80.0, now);
    v.release(now);
    assert!(v.find_open() && !v.find_focused());
    typed(&mut v, "o");
    assert_eq!(v.find_query(), "fo", "the keys are the document's");
}

#[test]
fn a_click_on_the_document_that_blurs_the_field_asks_for_a_repaint() {
    let mut v = view("foo bar\n\nsome more text here\n", 800, 600);
    open(&mut v);
    typed(&mut v, "fo");
    let now = Instant::now();
    assert!(v.press(60.0, 80.0, now), "the caret and the selection go");
    v.release(now);
    assert!(!v.find_focused());
    // (With the clipboard, a press on text starts a selection, which is a change of its own.)
    #[cfg(not(feature = "clipboard"))]
    {
        assert!(!v.press(60.0, 80.0, now), "nothing changes the second time");
        v.release(now);
        // Closed, the same click has nothing to blur.
        v.set_find(false);
        assert!(!v.press(60.0, 80.0, now));
    }
}

#[test]
fn ctrl_home_and_ctrl_end_are_not_the_fields_to_take() {
    let mut v = field_view(600, "par");
    press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
    for key in [NamedKey::Home, NamedKey::End] {
        // They go on to the document, which has no use for them.
        assert!(
            v.find_key(&named(key), None, CTRL, Instant::now())
                .is_none()
        );
        assert_eq!(caret(&v), 2, "{key:?} did not move the caret");
        assert!(
            v.find_key(&named(key), None, CTRL | SHIFT, Instant::now())
                .is_none()
        );
        assert_eq!(v.find_selected(), "");
    }
    press(&mut v, named(NamedKey::Home), None, NONE);
    assert_eq!(caret(&v), 0, "Home is the field's");
}

#[test]
fn the_bar_takes_the_pointer_and_the_wheel() {
    let src = big(60, &[]);
    let mut v = view(&src, 800, 600);
    v.set_toolbar(true);
    open(&mut v);
    settle(&mut v);
    let now = Instant::now();
    // Over the bar (y 32..64): no link, no scroll.
    v.hover_at(300.0, 48.0, now);
    let at = v.scroll();
    v.wheel_lines(3.0, now);
    v.wheel_by(200.0, now);
    assert_eq!(v.scroll(), at, "the wheel over the bar does nothing");
    assert!(v.find_covers(300.0, 48.0) && !v.find_covers(300.0, 20.0));
    // Over the document it scrolls again.
    v.hover_at(300.0, 300.0, now);
    v.wheel_lines(3.0, now);
    assert!(v.scroll() > at);
    // And over the bar once more it does not.
    let at = v.scroll();
    v.hover_at(300.0, 48.0, now);
    v.wheel_lines(-3.0, now);
    assert_eq!(v.scroll(), at);
    v.hover_end();
    v.wheel_lines(-3.0, now);
    assert!(v.scroll() < at, "gone from the bar, the wheel works");
}

#[test]
fn hovering_a_button_shows_its_tooltip_after_the_delay() {
    let mut v = view("foo\n", 800, 600);
    open(&mut v);
    typed(&mut v, "foo");
    let now = Instant::now();
    let (_, next) = v.find_buttons()[1];
    let (x, y) = (
        f64::from(next.x0 + next.x1) / 2.0,
        f64::from(next.y0 + next.y1) / 2.0,
    );
    assert!(v.hover_at(x, y, now), "the button lights up");
    assert_eq!(v.find_deadline(), Some(now + toolbar::TOOLTIP_DELAY));
    assert!(!v.find_expire(now + Duration::from_millis(100)));
    let plain = paint(&mut v, now);
    assert!(v.find_expire(now + toolbar::TOOLTIP_DELAY));
    assert_eq!(v.find_deadline(), None);
    assert!(
        v.deadline()
            .is_none_or(|d| d > now + toolbar::TOOLTIP_DELAY)
    );
    let tip = paint(&mut v, now);
    assert_ne!(plain, tip, "the tooltip is drawn");
    // Moving off takes it away.
    assert!(v.hover_at(300.0, 300.0, now));
    assert_eq!(paint(&mut v, now).len(), tip.len());
    assert_eq!(v.find_deadline(), None);
    // The labels name the keys.
    assert!(Spot::Next.label().contains("Enter") && Spot::Prev.label().contains("Shift"));
    assert!(Spot::Close.label().contains("Esc"));
}

#[test]
fn the_buttons_are_disabled_without_matches() {
    let mut v = view("foo\n", 800, 600);
    open(&mut v);
    typed(&mut v, "zzz");
    let (_, next) = v.find_buttons()[1];
    let (x, y) = (
        f64::from(next.x0 + next.x1) / 2.0,
        f64::from(next.y0 + next.y1) / 2.0,
    );
    let now = Instant::now();
    assert!(v.press(x, y, now));
    v.release_toolbar(x, y);
    assert_eq!(v.find_current(), None);
    assert!(v.find_open());
    paint(&mut v, now);
}

#[test]
fn a_message_in_place_of_the_document_has_the_bar_too() {
    let mut v = View::new(
        Fonts::new().unwrap(),
        Doc::parse(""),
        Some("Cannot open it".into()),
        (800, 600),
        1.0,
    );
    open(&mut v);
    typed(&mut v, "abc");
    assert_eq!(v.find_count(), "No matches");
    let buf = paint(&mut v, Instant::now());
    assert_eq!(buf[10 * 800 + 790], LIGHT.code_bg);
    enter(&mut v, NONE);
    v.set_find(false);
}

/// The toolbar's menu is above the bar: each Escape takes away one layer, the menu first.
#[test]
fn escape_closes_the_open_menu_before_the_bar_and_the_bar_before_quitting() {
    let mut v = view(&big(40, &[(5, "te")]), 300, 400);
    v.set_toolbar(true);
    open(&mut v);
    typed(&mut v, "te");
    let more = crate::toolbar::slots(&v.toolbar_ctx())
        .items
        .iter()
        .find(|(i, _)| *i == crate::toolbar::Item::More)
        .expect("a narrow window folds the toolbar into the menu")
        .1;
    let (mx, my) = (f64::from((more.x0 + more.x1) / 2.0), 16.0);
    let now = Instant::now();
    assert!(v.press(mx, my, now));
    v.release_toolbar(mx, my);
    assert!(v.toolbar.menu_open(&v.toolbar_ctx()));
    assert!(v.find_open());
    let escape = |v: &mut View| press(v, named(NamedKey::Escape), None, NONE);
    assert_eq!(escape(&mut v), Outcome::Redraw);
    assert!(
        !v.toolbar.menu_open(&v.toolbar_ctx()),
        "the menu goes first"
    );
    assert!(v.find_open(), "the bar stays");
    assert_eq!(v.find_query(), "te");
    assert!(v.find_current().is_some());
    assert_eq!(escape(&mut v), Outcome::Redraw);
    assert!(!v.find_open());
    assert_eq!(escape(&mut v), Outcome::Quit);
}

/// A 300 x 100 window with the bar open and the toolbar's dropdown open over it, too short to
/// show its rows: the dropdown covers the right of the bar, its first row over the bar's buttons.
fn menu_over_bar() -> (View, Instant) {
    let now = Instant::now();
    let mut v = view(&big(60, &[(5, "te")]), 300, 100);
    v.set_toolbar(true);
    open(&mut v);
    typed(&mut v, "te");
    settle(&mut v);
    let more = crate::toolbar::slots(&v.toolbar_ctx())
        .items
        .iter()
        .find(|(i, _)| *i == crate::toolbar::Item::More)
        .expect("a narrow window folds the toolbar into the menu")
        .1;
    let (mx, my) = (f64::from((more.x0 + more.x1) / 2.0), 16.0);
    assert!(v.press(mx, my, now));
    v.release_toolbar(mx, my);
    assert!(v.toolbar.menu_open(&v.toolbar_ctx()));
    (v, now)
}

/// Over the bar's previous button and under the dropdown, and over its close button, clear of it.
const UNDER: (f64, f64) = (215.0, 48.0);
const BESIDE: (f64, f64) = (280.0, 48.0);

#[test]
fn the_dropdown_over_the_bar_takes_the_wheel_and_the_bar_where_it_is_clear() {
    let (mut v, now) = menu_over_bar();
    let ctx = v.toolbar_ctx();
    assert!(v.find_covers(UNDER.0, UNDER.1) && v.toolbar.menu_covers(&ctx, UNDER.0, UNDER.1));
    assert!(v.find_covers(BESIDE.0, BESIDE.1) && !v.toolbar.menu_covers(&ctx, BESIDE.0, BESIDE.1));
    let at = v.scroll();
    // Where the dropdown covers the bar the wheel scrolls its rows.
    v.hover_at(UNDER.0, UNDER.1, now);
    v.wheel_by(10.0, now);
    assert_eq!(v.toolbar.menu_scroll(), Some(10.0));
    v.wheel_lines(1.0, now);
    assert!(v.toolbar.menu_scroll() > Some(10.0));
    // Where it is clear the bar takes the wheel and does nothing with it, the dropdown's rows
    // and the document alike.
    let rows = v.toolbar.menu_scroll();
    v.hover_at(BESIDE.0, BESIDE.1, now);
    v.wheel_by(10.0, now);
    v.wheel_lines(1.0, now);
    assert_eq!(v.toolbar.menu_scroll(), rows);
    assert_eq!(v.scroll(), at);
    // The dropdown closing under a pointer that stays on the bar leaves the wheel with the bar.
    v.hover_at(UNDER.0, UNDER.1, now);
    v.close_menu();
    v.wheel_by(10.0, now);
    assert_eq!(v.scroll(), at);
}

#[test]
fn a_press_on_the_dropdown_over_the_bar_is_the_dropdowns_and_not_the_bars() {
    let (mut v, now) = menu_over_bar();
    let (spot, button) = v.find_buttons()[2];
    assert_eq!(spot, Spot::Prev);
    assert!(button.contains(UNDER.0, UNDER.1));
    let current = v.find_current();
    // The previous button is under the dropdown's first row: the row is picked.
    assert!(v.press(UNDER.0, UNDER.1, now));
    assert!(v.find.pressed.is_none(), "the bar saw no press");
    v.release_toolbar(UNDER.0, UNDER.1);
    assert!(!v.toolbar.menu_open(&v.toolbar_ctx()), "the row was picked");
    assert_eq!(
        v.find_current(),
        current,
        "the button under the row stayed put"
    );
    // A press on the bar clear of the dropdown closes it and does nothing else.
    let (mut v, now) = menu_over_bar();
    let current = v.find_current();
    assert!(v.press(BESIDE.0, BESIDE.1, now));
    assert!(v.find.pressed.is_none());
    v.release_toolbar(BESIDE.0, BESIDE.1);
    assert!(!v.toolbar.menu_open(&v.toolbar_ctx()));
    assert!(v.find_open(), "the close button did not act");
    assert_eq!(v.find_current(), current);
}

#[test]
fn hovering_the_dropdown_over_the_bar_lights_no_button() {
    let (mut v, now) = menu_over_bar();
    assert!(v.toolbar.menu_covers(&v.toolbar_ctx(), UNDER.0, UNDER.1));
    v.hover_at(UNDER.0, UNDER.1, now);
    assert_eq!(
        v.find.hover, None,
        "no tint on the button under the dropdown"
    );
    assert_eq!(v.find_deadline(), None, "and no tooltip is due");
    // Clear of it the button is the bar's again.
    v.hover_at(BESIDE.0, BESIDE.1, now);
    assert_eq!(v.find.hover, Some(Spot::Close));
    v.hover_at(UNDER.0, UNDER.1, now);
    assert_eq!(v.find.hover, None);
    assert_eq!(v.find_deadline(), None);
    // Nor is there a link under it: the dropdown is not the document's.
    assert_eq!(v.hovered(), None);
}

/// Closing the bar by its button or the toolbar's does not close a menu that is open.
#[test]
fn closing_the_bar_leaves_the_menu_alone() {
    let mut v = view(&big(40, &[(5, "te")]), 300, 400);
    v.set_toolbar(true);
    open(&mut v);
    let more = crate::toolbar::slots(&v.toolbar_ctx())
        .items
        .iter()
        .find(|(i, _)| *i == crate::toolbar::Item::More)
        .unwrap()
        .1;
    let (mx, my) = (f64::from((more.x0 + more.x1) / 2.0), 16.0);
    assert!(v.press(mx, my, Instant::now()));
    v.release_toolbar(mx, my);
    assert!(v.toolbar.menu_open(&v.toolbar_ctx()));
    v.set_find(false);
    assert!(v.toolbar.menu_open(&v.toolbar_ctx()));
    // Opening, on the other hand, takes it away.
    v.set_find(true);
    assert!(!v.toolbar.menu_open(&v.toolbar_ctx()));
}

/// In a window too narrow for the panel to sit beside the document it covers it, and Escape has
/// the find bar first.
#[cfg(feature = "toc")]
#[test]
fn escape_closes_the_bar_before_a_panel_that_covers_the_document() {
    let mut v = view("# One\n\n## Two\n\ntext\n", 600, 400);
    v.set_toc(true);
    assert!(v.toc_open());
    open(&mut v);
    typed(&mut v, "te");
    assert_eq!(
        press(&mut v, named(NamedKey::Escape), None, NONE),
        Outcome::Redraw
    );
    assert!(!v.find_open() && v.toc_open());
    assert_eq!(
        press(&mut v, named(NamedKey::Escape), None, NONE),
        Outcome::Redraw
    );
    assert!(!v.toc_open());
    assert_eq!(
        press(&mut v, named(NamedKey::Escape), None, NONE),
        Outcome::Quit
    );
}

/// The panel starts below the bar like the document does.
#[cfg(feature = "toc")]
#[test]
fn the_contents_panel_starts_below_the_bar() {
    let mut v = view("# One\n\n## Two\n\ntext\n", 900, 400);
    v.set_toc(true);
    assert_eq!(v.toc_geom().y0, 0.0);
    open(&mut v);
    assert_eq!(v.toc_geom().y0, 32.0);
    assert!(!v.toc_covers(50.0, 10.0), "the bar is above the panel");
    assert!(v.toc_covers(50.0, 40.0));
    assert!(v.find_covers(50.0, 10.0));
    paint(&mut v, Instant::now());
}

/// A match is washed where a selection of the same text would be: per line, over the clusters
/// of the match, in either direction, with nothing for a line's end.
#[cfg(feature = "clipboard")]
#[test]
fn matches_are_washed_where_the_same_selection_would_be() {
    use crate::select::{BlockSel, Pos};
    let src = "hello مرحبا بالعالم world שלום עולם again\n\n".repeat(30);
    let mut v = view(&src, 360, 400);
    for query in ["hello", "بالعالم", "עולם", "o م", "world שלום", "ם a"] {
        v.set_find(false);
        open(&mut v);
        v.find_type(query);
        assert!(!v.find_matches().is_empty(), "{query}");
        let m = v.find_matches()[0];
        let content = v.layout().geoms()[m.block as usize]
            .content
            .as_deref()
            .expect("the first block is shaped");
        let marks = v.find_found().marks(None);
        let block = marks.of_block(v.doc(), m.block).unwrap();
        let mut washed = Vec::new();
        block.rects(content, |x0, y0, x1, y1, _| washed.push((x0, y0, x1, y1)));
        assert!(!washed.is_empty(), "{query}");
        // The selection of the first match only; later matches in the block are not in it.
        let sel = BlockSel::of(
            m.block,
            (Pos::new(m.block, m.start), Pos::new(m.block, m.end)),
        )
        .unwrap();
        let mut selected = Vec::new();
        sel.rects(content, |x0, y0, x1, y1| selected.push((x0, y0, x1, y1)));
        // A selection that goes on past a line's end marks the end, which the match does not.
        for r in &washed {
            assert!(selected.contains(r), "{query}: {r:?} in {selected:?}");
        }
    }
}

#[test]
fn a_new_query_is_looked_for_from_the_reading_position_not_the_old_match() {
    // Both words are in every paragraph, `zebra` first.
    let src: String = (1..=120)
        .map(|i| {
            format!("Paragraph {i} has a zebra and a yak, in text that goes on for a while.\n\n")
        })
        .collect();
    let mut v = view(&src, 800, 500);
    open(&mut v);
    typed(&mut v, "zebra");
    assert_eq!(v.find_current_start(), Some((0, v.find_matches()[0].start)));
    // The reader scrolls away, and selects the query to type another.
    v.set_anchor(Anchor {
        block: 70,
        frac: 0.0,
    });
    v.ensure();
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    typed(&mut v, "yak");
    let m = v.find_matches()[v.find_current().unwrap()];
    assert!(m.block >= 69, "from block 70, not {}", m.block);
    assert!(v.anchor().block >= 65, "{:?}", v.anchor());
    // Typing on, with the match in view, keeps it.
    typed(&mut v, ",");
    assert_eq!(v.find_matches()[v.find_current().unwrap()].block, m.block);
    // The same when it was not a replaced query that was edited: the reader scrolled away from
    // the current match.
    v.set_anchor(Anchor {
        block: 20,
        frac: 0.0,
    });
    v.ensure();
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    let m = v.find_matches()[v.find_current().unwrap()];
    assert!(m.block >= 19, "from block 20, not {}", m.block);
}

/// The rectangles that wash the first match of `query` in the first block that has one.
fn washed(v: &mut View, query: &str) -> Vec<(f32, f32, f32, f32)> {
    v.set_find(false);
    open(v);
    v.find_type(query);
    assert!(!v.find_matches().is_empty(), "{query}");
    let m = v.find_matches()[0];
    let content = v.layout().geoms()[m.block as usize]
        .content
        .as_deref()
        .expect("the block is shaped");
    let marks = v.find_found().marks(None);
    let block = marks.of_block(v.doc(), m.block).unwrap();
    let mut rects = Vec::new();
    block.rects(content, |x0, y0, x1, y1, _| rects.push((x0, y0, x1, y1)));
    rects
}

/// A view of `src` with the face in `path` as a fallback for what the embedded fonts lack (none
/// when the machine has no such file).
fn view_with_face(src: &str, path: &str) -> Option<View> {
    let file = std::path::PathBuf::from(path);
    if !file.exists() {
        return None;
    }
    let mut v = view(src, 600, 300);
    let missing = v.take_missing();
    let found = crate::text::fallback::find_in(&[file], &missing, &|_, _| false);
    assert!(v.add_fallbacks(&found));
    settle(&mut v);
    Some(v)
}

#[test]
fn a_match_that_starts_inside_a_cluster_washes_the_whole_cluster() {
    let face = "/usr/share/fonts/TTF/DejaVuSans.ttf";
    let src = "سلام\n\ncafe\u{301}\n";
    let Some(mut v) = view_with_face(src, face) else {
        return;
    };
    // Lam–alef is one cluster of this face, and the match is its second letter.
    let clusters = |v: &View, block: usize| -> Vec<u32> {
        let content = v.layout().geoms()[block].content.as_deref().unwrap();
        content.hits.iter().map(|h| h.doc).collect()
    };
    let arabic = clusters(&v, 0);
    assert!(arabic.len() < 4, "the ligature is one cluster: {arabic:?}");
    let whole = washed(&mut v, "لا");
    let inner = washed(&mut v, "ا");
    assert!(!inner.is_empty());
    assert!(inner.iter().all(|r| r.2 > r.0 && r.3 > r.1));
    assert_eq!(
        inner, whole,
        "all of the cluster, as when the match is all of it"
    );
    // The mark joins the `e` it follows, and washes the two.
    let base = washed(&mut v, "e\u{301}");
    let mark = washed(&mut v, "\u{301}");
    assert!(!mark.is_empty());
    assert_eq!(base, mark);
}

/// The pixels of the view painted in the current match's color.
fn strong(v: &mut View) -> usize {
    let buf = paint(v, Instant::now());
    buf.iter().filter(|&&p| p == LIGHT.find_current).count()
}

#[test]
fn the_current_match_keeps_its_color_when_another_shares_its_cluster() {
    // Two marks after one `e`: one cluster, and each mark is a match of its own.
    let mut v = view("be\u{301}\u{301} and more\n", 600, 300);
    let content = v.layout().geoms()[0].content.as_deref().unwrap();
    assert_eq!(
        content
            .hits
            .iter()
            .filter(|h| h.doc >= 1 && h.doc < 6)
            .count(),
        1
    );
    open(&mut v);
    typed(&mut v, "\u{301}");
    assert_eq!(v.find_total(), 2);
    for step in 0..4 {
        assert!(strong(&mut v) > 0, "step {step}: no strong wash");
        enter(&mut v, NONE);
    }
    // The strong color is as wide as the cluster, whichever of the two is current.
    let first = strong(&mut v);
    enter(&mut v, NONE);
    assert_eq!(strong(&mut v), first);
}

#[test]
fn a_match_that_starts_in_a_cluster_on_the_line_above_is_brought_into_view_from_there() {
    let mut found = 0;
    for pad in 0..40 {
        let src = format!("{}xe\u{301} word {}\n", "w ".repeat(pad), "z ".repeat(60));
        let mut v = view(&src, 220, 200);
        open(&mut v);
        typed(&mut v, "\u{301} word");
        assert_eq!(v.find_total(), 1);
        let m = v.find_matches()[0];
        let content = v.layout().geoms()[0].content.as_deref().unwrap();
        let line_of = |doc: u32| {
            content
                .lines
                .iter()
                .position(|l| {
                    content.hits[l.hits.start as usize..l.hits.end as usize]
                        .iter()
                        .any(|h| h.doc == doc)
                })
                .unwrap()
        };
        // The mark joins the `e` before it (a byte earlier than the match's start).
        let (a, b) = (line_of(m.start - 1), line_of(m.end - 1));
        if a == b {
            continue;
        }
        found += 1;
        let (top, bottom) = v.find_extent(m).unwrap();
        assert!(
            top <= f64::from(content.lines[a].top) + v.layout().top(0),
            "pad {pad}: the extent starts at {top}, below the first line"
        );
        assert!(bottom >= f64::from(content.lines[b].bottom) + v.layout().top(0));
    }
    assert!(found > 0, "some padding wraps the match");
}

#[test]
fn the_matches_of_split_row_fragments_are_all_washed() {
    let left = format!("{}needle", "x ".repeat(4_500));
    let right = format!("{}{}", "needle ".repeat(4), "y ".repeat(4_500));
    let src = format!("| a | b |\n|---|---|\n| {left} | {right} |\n");
    let mut v = view(&src, 800, 600);
    let first = v.doc().blocks.iter().position(|b| b.joined).unwrap() - 1;
    open(&mut v);
    typed(&mut v, "needle");
    assert_eq!(v.find_total(), 5);
    // Shape every fragment, whichever way the view reaches them.
    for b in first..v.doc().blocks.len() {
        v.set_anchor(Anchor {
            block: b as u32,
            frac: 0.0,
        });
        v.ensure();
        let Some(content) = v.layout().geoms()[b].content.as_deref() else {
            continue;
        };
        let marks = v.find_found().marks(None);
        let block = marks.of_block(v.doc(), b as u32).unwrap();
        let mut rects = 0;
        block.rects(content, |_, _, _, _, _| rects += 1);
        // The matches that begin in this fragment, each its own rectangle at least (the one that
        // goes across a cut is washed in both).
        let own = v
            .find_matches()
            .iter()
            .filter(|m| m.block as usize == b)
            .count();
        assert!(rects >= own, "fragment {b}: {rects} rectangles for {own}");
    }
}

/// The offsets of the clusters of the first block that are washed when every match of `query`
/// is marked, and the line (its top) of each.
fn washed_clusters(v: &mut View, query: &str) -> Vec<(u32, f32)> {
    v.set_find(false);
    open(v);
    v.find_type(query);
    let found = v.find_found();
    let marks = found.marks(None);
    let mut out = Vec::new();
    for b in 0..v.doc().blocks.len() as u32 {
        let Some(content) = v.layout().geoms()[b as usize].content.as_deref() else {
            continue;
        };
        let Some(block) = marks.of_block(v.doc(), b) else {
            continue;
        };
        let mut rects = Vec::new();
        block.rects(content, |x0, y0, x1, y1, _| rects.push((x0, y0, x1, y1)));
        for line in &content.lines {
            for h in &content.hits[line.hits.start as usize..line.hits.end as usize] {
                let inside = |r: &(f32, f32, f32, f32)| {
                    r.1 == line.top && r.0 <= h.x0 + 0.01 && h.x1 <= r.2 + 0.01
                };
                if rects.iter().any(inside) {
                    out.push((h.doc, line.top));
                }
            }
        }
    }
    out
}

#[test]
fn whitespace_the_layout_left_out_is_not_a_part_of_the_cluster_before_it() {
    // The space after `foo` is where the line breaks, so it has no cluster: the match of it
    // and `bar` washes `bar` alone, not the `o` before.
    let src = "w w w w w w w w foo bar baz more text";
    let mut v = view(src, 220, 300);
    let foo = src.find("foo").unwrap() as u32;
    let bar = src.find("bar").unwrap() as u32;
    let washed = washed_clusters(&mut v, " bar");
    let offs: Vec<u32> = washed.iter().map(|w| w.0).collect();
    assert_eq!(offs, [bar, bar + 1, bar + 2]);
    let content = v.layout().geoms()[0].content.as_deref().unwrap();
    let line_of = |off| {
        content.lines.iter().position(|l| {
            content.hits[l.hits.start as usize..l.hits.end as usize]
                .iter()
                .any(|h| h.doc == off)
        })
    };
    assert_ne!(line_of(foo + 2), line_of(bar), "the break is after `foo`");
}

#[test]
fn trailing_spaces_in_a_code_line_wash_nothing_and_have_a_line() {
    let mut v = view("```\nfoo   \n```\n", 400, 300);
    assert!(washed_clusters(&mut v, " ").is_empty());
    let spaces: Vec<Match> = v.find_matches().to_vec();
    assert_eq!(spaces.len(), 3);
    // Each sits on the line of `foo`, which the view can scroll to.
    let foo = {
        v.set_find(false);
        open(&mut v);
        v.find_type("foo");
        v.find_extent(v.find_matches()[0]).unwrap()
    };
    for m in spaces {
        assert_eq!(v.find_extent(m), Some(foo), "{m:?}");
    }
}

#[test]
fn a_match_of_omitted_whitespace_alone_is_still_stepped_to() {
    let filler = "Some filler text.\n\n".repeat(60);
    let src = format!("{filler}```\nfoo   \n```\n\n{filler}");
    let mut v = view(&src, 700, 300);
    open(&mut v);
    v.find_type("  ");
    assert_eq!(v.find_total(), 1);
    assert!(washed_clusters(&mut v, "  ").is_empty());
    let m = v.find_matches()[v.find_current().unwrap()];
    let (top, bottom) = v.find_extent(m).expect("the line of the text before it");
    assert!(top >= v.scroll() - 1.0 && bottom <= v.scroll() + f64::from(v.area_height()) + 1.0);
}

#[test]
fn whitespace_in_a_span_with_no_cluster_is_brought_into_view_from_the_text_beside_it() {
    // The two spaces of the emphasis are the break between `foo` and `bar`, so the span has no
    // cluster, and the line is that of the text next to it.
    let src = format!("{}foo<em>  </em>bar {}", "w ".repeat(308), "z ".repeat(100));
    let mut v = view(&src, 220, 300);
    open(&mut v);
    typed(&mut v, "  ");
    assert_eq!(v.find_total(), 1);
    let m = v.find_matches()[0];
    let (top, bottom) = v.find_extent(m).expect("the match has a line");
    let area = f64::from(v.area_height());
    assert!(top > area, "the match is below the first screen ({top})");
    assert!(v.scroll() > 0.0, "the first match was not scrolled to");
    assert!(
        top >= v.scroll() && bottom <= v.scroll() + area,
        "{top} is not in view"
    );
    // Next keeps it there, and it paints nothing of its own.
    v.set_scroll(0.0);
    v.ensure();
    enter(&mut v, NONE);
    assert!(v.scroll() > 0.0);
    assert!(washed_clusters(&mut v, "  ").is_empty());
}

#[test]
fn whitespace_in_a_table_cell_without_a_cluster_takes_a_line_of_its_own_cell() {
    let cell = format!("{}foo<em>  </em>bar", "w ".repeat(40));
    let src = format!("| a | b |\n|---|---|\n| {cell} | other |\n");
    let mut v = view(&src, 300, 300);
    open(&mut v);
    typed(&mut v, "  ");
    assert_eq!(v.find_total(), 1);
    let m = v.find_matches()[0];
    assert_eq!(&v.doc().text[m.start as usize..m.end as usize], "  ");
    assert!(v.find_extent(m).is_some(), "the match has a line");
}

#[test]
fn a_match_in_a_block_with_no_cluster_is_revealed_at_the_top_of_the_block() {
    let filler = "A paragraph that is a line or two long.\n\n".repeat(40);
    let src = format!("{filler}```\n   \n```\n\n{filler}");
    let mut v = view(&src, 400, 300);
    open(&mut v);
    typed(&mut v, "  ");
    assert!(v.find_total() > 0);
    let m = v.find_matches()[0];
    let content = v.layout().geoms()[m.block as usize].content.as_deref();
    assert!(
        content.is_some_and(|c| c.hits.is_empty()),
        "the block has clusters: the last resort is not reached"
    );
    let (top, bottom) = v.find_extent(m).expect("a found match has a place");
    let block = v.layout().top(m.block as usize);
    assert!(
        (block..block + 64.0).contains(&top),
        "{top} is not at {block}"
    );
    let area = f64::from(v.area_height());
    assert!(v.scroll() > 0.0);
    assert!(
        top >= v.scroll() && bottom <= v.scroll() + area,
        "{top} is not in view"
    );
}

// ---- The field ---------------------------------------------------------------------------------

/// The window x of the caret place at byte offset `at` in the query, as the field is laid out now.
fn place_x(v: &mut View, at: usize) -> f64 {
    let (_, inner) = v.find_geometry();
    let text = v.find_text();
    f64::from(inner.x0 - v.find.scroll + text.x(at))
}

/// The window x halfway through the character at byte offset `at`, which is `len` bytes long.
fn over_x(v: &mut View, at: usize, len: usize) -> f64 {
    (place_x(v, at) + place_x(v, at + len)) / 2.0
}

/// A click on the field at window x `x`: the press and the release, as the shell sends them.
/// True if what it selected is offered as the primary selection.
fn click(v: &mut View, x: f64, at: Instant) -> bool {
    v.press(x, 16.0, at);
    let offer = v.release_find_drag();
    v.release(at);
    offer
}

/// A view with the bar open on a plain document, the query `query` typed, and the field laid out.
fn field_view(width: u32, query: &str) -> View {
    let mut v = view("some text to search\n\nmore text, and more\n", width, 300);
    open(&mut v);
    v.find_type(query);
    paint(&mut v, Instant::now());
    v
}

fn caret(v: &View) -> usize {
    v.find_caret()
}

#[test]
fn the_caret_moves_where_typing_and_the_arrows_say() {
    let mut v = field_view(600, "alpha beta");
    assert_eq!(caret(&v), 10);
    press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
    press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
    typed(&mut v, "X");
    assert_eq!(v.find_query(), "alpha beXta");
    assert_eq!(caret(&v), 9);
    press(&mut v, named(NamedKey::Home), None, NONE);
    assert_eq!(caret(&v), 0);
    press(&mut v, named(NamedKey::ArrowRight), None, CTRL);
    assert_eq!(caret(&v), 5, "Ctrl+Right is a word");
    press(&mut v, named(NamedKey::ArrowRight), None, CTRL);
    assert_eq!(caret(&v), 11);
    press(&mut v, named(NamedKey::ArrowLeft), None, CTRL);
    assert_eq!(caret(&v), 6);
    press(&mut v, named(NamedKey::End), None, NONE);
    assert_eq!(caret(&v), 11);
    press(&mut v, named(NamedKey::Delete), None, NONE);
    assert_eq!(v.find_query(), "alpha beXta", "nothing after the caret");
    press(&mut v, named(NamedKey::Home), None, NONE);
    press(&mut v, named(NamedKey::Delete), None, NONE);
    assert_eq!(v.find_query(), "lpha beXta");
    press(&mut v, named(NamedKey::Delete), None, CTRL);
    assert_eq!(v.find_query(), " beXta", "Ctrl+Delete is a word");
    press(&mut v, named(NamedKey::ArrowRight), None, NONE);
    press(&mut v, named(NamedKey::Backspace), None, CTRL);
    assert_eq!(v.find_query(), "beXta", "and Ctrl+Backspace");
}

#[test]
fn shift_selects_ctrl_a_selects_all_and_typing_replaces_the_selection() {
    let mut v = field_view(600, "alpha beta");
    assert_eq!(
        press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT),
        selected()
    );
    press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT);
    assert_eq!(v.find_selected(), "ta");
    press(&mut v, named(NamedKey::ArrowLeft), None, CTRL | SHIFT);
    assert_eq!(v.find_selected(), "beta");
    press(&mut v, named(NamedKey::Home), None, SHIFT);
    assert_eq!(v.find_selected(), "alpha beta");
    press(&mut v, named(NamedKey::ArrowRight), None, NONE);
    assert_eq!(v.find_selected(), "");
    assert_eq!(caret(&v), 10, "Right goes to the end of the selection");
    assert_eq!(press(&mut v, ch("a"), Some("\u{1}"), CTRL), selected());
    assert_eq!(v.find_selected(), "alpha beta");
    typed(&mut v, "x");
    assert_eq!(v.find_query(), "x");
    // A selection in the middle, replaced, and deleted.
    v.find_type("yz");
    press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT);
    typed(&mut v, "Q");
    assert_eq!(v.find_query(), "xyQ");
    press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT);
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    assert_eq!(v.find_query(), "xy");
    // The matches follow.
    v.find_type("ext");
    assert_eq!(v.find_matches().len(), 0);
    press(&mut v, named(NamedKey::Backspace), None, CTRL);
    press(&mut v, named(NamedKey::Backspace), None, CTRL);
    typed(&mut v, "text");
    assert_eq!(v.find_matches().len(), 2);
}

#[cfg(feature = "clipboard")]
#[test]
fn a_selection_made_with_the_keys_is_offered_as_primary_and_a_motion_is_not() {
    let mut v = field_view(600, "alpha");
    for (key, mods, selects) in [
        (NamedKey::ArrowLeft, NONE, false),
        (NamedKey::ArrowLeft, SHIFT, true),
        (NamedKey::Home, SHIFT, true),
        (NamedKey::End, NONE, false),
    ] {
        let outcome = press(&mut v, named(key), None, mods);
        assert_eq!(outcome == selected(), selects, "{key:?} {mods:?}");
    }
    assert_eq!(selected(), Outcome::Selected);
    press(&mut v, named(NamedKey::Home), None, SHIFT);
    assert_eq!(
        v.offered().and_then(|c| match c {
            Content::Text(t) => Some(t.to_string()),
            Content::Lazy { .. } => None,
        }),
        Some("alpha".to_owned()),
        "while the field has the keyboard, what is offered is its selection"
    );
}

#[test]
fn the_caret_stays_between_characters_of_every_size() {
    let mut v = field_view(600, "é日😀z");
    for want in [9, 5, 2, 0] {
        press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
        assert_eq!(caret(&v), want);
    }
    press(&mut v, named(NamedKey::Delete), None, NONE);
    assert_eq!(v.find_query(), "日😀z");
    press(&mut v, named(NamedKey::End), None, NONE);
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    press(&mut v, named(NamedKey::Backspace), None, NONE);
    assert_eq!(v.find_query(), "日");
}

#[test]
fn the_caret_at_the_end_is_where_the_text_ends() {
    for query in [
        "AV", "To", "Ty.", "fi", "ffi", "r,", "mm", "日本", "e\u{301}",
    ] {
        let mut v = field_view(600, query);
        let end = place_x(&mut v, query.len());
        let (_, inner) = v.find_geometry();
        let width = v.find_line(query, Role::Text).1;
        assert!(
            (end - f64::from(inner.x0 + width)).abs() < 0.01,
            "{query:?}: the caret is at {end}, the text ends at {}",
            inner.x0 + width
        );
    }
}

#[test]
fn the_caret_advances_over_spaces_at_the_end_of_the_query() {
    let mut v = field_view(600, "ab");
    let a = place_x(&mut v, 2);
    typed(&mut v, " ");
    let b = place_x(&mut v, 3);
    typed(&mut v, "  ");
    let c = place_x(&mut v, 5);
    assert!(a < b && b < c, "{a} {b} {c}");
    assert!(place_x(&mut v, 0) < place_x(&mut v, 1));
}

#[test]
fn the_text_scrolls_to_keep_the_caret_in_the_field() {
    let mut v = field_view(300, "");
    let (_, inner) = v.find_geometry();
    v.find_type(&"abcdefghij".repeat(10));
    assert!(v.find.scroll > 0.0, "the end of the query is what shows");
    let at = |v: &mut View| place_x(v, v.find_caret());
    let visible = |v: &mut View| {
        let x = at(v) as f32;
        x >= inner.x0 && x <= inner.x1
    };
    assert!(visible(&mut v));
    press(&mut v, named(NamedKey::Home), None, NONE);
    assert_eq!(v.find.scroll, 0.0, "back to the start");
    assert!(visible(&mut v));
    // A caret moved to the right edge brings the text along, a step at a time.
    let mut last = 0.0;
    for _ in 0..60 {
        press(&mut v, named(NamedKey::ArrowRight), None, NONE);
        assert!(visible(&mut v), "caret {}", caret(&v));
        assert!(v.find.scroll >= last, "the text only goes left");
        last = v.find.scroll;
    }
    assert!(last > 0.0);
    // And back to the left again.
    for _ in 0..60 {
        press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
        assert!(visible(&mut v));
    }
    // Deleting the end of a scrolled text does not leave a gap at its right.
    press(&mut v, named(NamedKey::End), None, NONE);
    let full = v.find.scroll;
    for _ in 0..40 {
        press(&mut v, named(NamedKey::Backspace), None, NONE);
    }
    assert!(v.find.scroll < full);
    assert!(visible(&mut v));
    // A window narrower than the field's room: it still paints.
    for w in [60, 120, 400] {
        v.resize(w, 300);
        paint(&mut v, Instant::now());
        assert!(v.find.scroll >= 0.0);
    }
}

#[test]
fn the_ime_window_follows_the_caret() {
    let mut v = field_view(600, "abcdef");
    let end = v.find_ime_area().0;
    press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
    press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
    assert!(v.find_ime_area().0 < end);
    press(&mut v, named(NamedKey::Home), None, NONE);
    let start = v.find_ime_area().0;
    assert!(start < v.find_ime_area().0 + 1 && start < end);
}

// ---- Focus -------------------------------------------------------------------------------------

#[test]
fn a_blurred_field_leaves_letters_and_sideways_keys_to_the_document() {
    let src = big(60, &[]);
    let mut v = view(&src, 800, 500);
    open(&mut v);
    let (top, theme) = (v.scroll(), v.theme_mode());
    // Focused: j and t are text.
    typed(&mut v, "parjt");
    assert_eq!(v.find_query(), "parjt");
    assert_eq!((v.scroll(), v.theme_mode()), (top, theme));
    for _ in 0..2 {
        press(&mut v, named(NamedKey::Backspace), None, NONE);
    }
    let found = v.find_matches().len();
    assert!(found > 0);
    // A click on the document blurs the field: the bar stays, with its matches.
    let now = Instant::now();
    v.press(400.0, 300.0, now);
    v.release(now);
    assert!(v.find_open() && !v.find_focused());
    assert_eq!(v.find_matches().len(), found, "the highlights stay");
    press(&mut v, ch("j"), Some("j"), NONE);
    assert!(v.scroll() > top, "j scrolls the document");
    press(&mut v, ch("t"), Some("t"), NONE);
    assert_ne!(v.theme_mode(), theme, "t is the theme key");
    assert_eq!(v.find_query(), "par", "the query is as it was");
    // Typed and committed text goes nowhere.
    v.find_type("zzz");
    assert_eq!(v.find_query(), "par");
    // Left and Right do nothing there; Home and End are the document's.
    for key in [NamedKey::ArrowLeft, NamedKey::ArrowRight] {
        assert_eq!(press(&mut v, named(key), None, NONE), Outcome::Nothing);
    }
    assert_eq!(v.find_caret(), 3, "the caret did not move");
    press(&mut v, named(NamedKey::End), None, NONE);
    assert!(v.scroll() > top);
    press(&mut v, named(NamedKey::Home), None, NONE);
    assert_eq!(v.scroll(), 0.0);
    // Backspace is the document's too: Back.
    assert_eq!(
        press(&mut v, named(NamedKey::Backspace), None, NONE),
        Outcome::Back
    );
}

#[test]
fn enter_goes_to_the_document_when_blurred_and_f3_and_ctrl_g_still_step() {
    let mut v = view("foo one\n\nFoo two\n\nxx FOO three\n", 800, 600);
    open(&mut v);
    typed(&mut v, "foo");
    let now = Instant::now();
    v.press(400.0, 300.0, now);
    v.release(now);
    assert!(!v.find_focused());
    assert_eq!(v.find_count(), "1 / 3");
    assert_eq!(
        press(&mut v, named(NamedKey::Enter), Some("\r"), NONE),
        Outcome::Nothing,
        "Enter is no key of the document's"
    );
    assert_eq!(v.find_count(), "1 / 3", "and does not step");
    press(&mut v, named(NamedKey::F3), None, NONE);
    assert_eq!(v.find_count(), "2 / 3");
    press(&mut v, named(NamedKey::F3), None, SHIFT);
    assert_eq!(v.find_count(), "1 / 3");
    press(&mut v, ch("g"), Some("\u{7}"), CTRL);
    assert_eq!(v.find_count(), "2 / 3");
    press(&mut v, ch("G"), Some("\u{7}"), CTRL | SHIFT);
    assert_eq!(v.find_count(), "1 / 3");
    // Focused again by a click in the field, Enter steps.
    click(&mut v, 30.0, now);
    assert!(v.find_focused());
    enter(&mut v, NONE);
    assert_eq!(v.find_count(), "2 / 3");
}

#[test]
fn a_click_in_the_field_a_reopening_and_the_button_give_the_field_the_keyboard() {
    let mut v = view("text\n", 800, 600);
    open(&mut v);
    let now = Instant::now();
    v.press(400.0, 300.0, now);
    v.release(now);
    assert!(!v.find_focused());
    // The bar's margin, beside the field, doesn't.
    v.press(3.0, 16.0, now);
    v.release_find_drag();
    v.release(now);
    assert!(!v.find_focused(), "beside the field is not the field");
    click(&mut v, 30.0, now);
    assert!(v.find_focused());
    // Close and open again, by key and by the toolbar's action.
    v.press(400.0, 300.0, now);
    assert!(!v.find_focused());
    v.apply(Action::Find, now);
    assert!(!v.find_open());
    v.apply(Action::Find, now);
    assert!(v.find_open() && v.find_focused());
}

#[test]
fn a_selection_drag_on_the_document_blurs_the_field_and_the_scrollbar_does_not() {
    let src = big(60, &[]);
    let mut v = view(&src, 800, 500);
    open(&mut v);
    settle(&mut v);
    let now = Instant::now();
    // The scrollbar's track, at the window's right edge, is not a click on the text.
    v.hover_at(796.0, 300.0, now);
    v.press(796.0, 300.0, now);
    v.release(now);
    assert!(v.find_focused());
    v.press(100.0, 200.0, now);
    #[cfg(feature = "clipboard")]
    v.select_to(300.0, 260.0, now);
    assert!(!v.find_focused());
    v.release(now);
    #[cfg(feature = "clipboard")]
    {
        v.release_select();
        // Ctrl+C copies the document's selection; Ctrl+A selects the document.
        assert_eq!(
            press(&mut v, ch("c"), Some("\u{3}"), CTRL),
            Outcome::Copy,
            "the document's selection"
        );
        assert!(matches!(v.offered(), Some(Content::Text(t)) if !t.is_empty()));
        assert_eq!(
            press(&mut v, ch("a"), Some("\u{1}"), CTRL),
            Outcome::Selected
        );
        assert!(v.selection().is_some());
    }
}

#[test]
fn escape_closes_the_bar_before_other_things_whoever_has_the_keyboard() {
    let mut v = view("text\n", 800, 600);
    open(&mut v);
    let now = Instant::now();
    v.press(400.0, 300.0, now);
    v.release(now);
    assert!(!v.find_focused());
    assert_eq!(
        press(&mut v, named(NamedKey::Escape), None, NONE),
        Outcome::Redraw
    );
    assert!(!v.find_open());
}

#[test]
fn the_toolbar_button_names_the_shortcut_for_closing_too() {
    let mut v = view("text\n", 800, 600);
    v.set_toolbar(true);
    assert_eq!(toolbar::Item::Find.label(&v.toolbar_ctx()), "Find (Ctrl+F)");
    open(&mut v);
    assert_eq!(
        toolbar::Item::Find.label(&v.toolbar_ctx()),
        "Close find (Ctrl+F)"
    );
}

// ---- The mouse ---------------------------------------------------------------------------------

#[test]
fn a_click_puts_the_caret_between_the_characters_nearest_it() {
    let mut v = field_view(600, "alpha beta");
    let now = Instant::now();
    for at in [0, 3, 6, 10] {
        let x = place_x(&mut v, at);
        click(
            &mut v,
            x,
            now + Duration::from_secs(u64::try_from(at).unwrap()),
        );
        assert_eq!(caret(&v), at);
        assert_eq!(v.find_selected(), "");
    }
    // Between two places goes to the nearer.
    let (a, b) = (place_x(&mut v, 3), place_x(&mut v, 4));
    click(&mut v, a + (b - a) * 0.3, now + Duration::from_secs(20));
    assert_eq!(caret(&v), 3);
    click(&mut v, a + (b - a) * 0.7, now + Duration::from_secs(30));
    assert_eq!(caret(&v), 4);
    // Past the end of the text, and before its start.
    let (field, inner) = v.find_geometry();
    click(
        &mut v,
        f64::from(inner.x1) - 1.0,
        now + Duration::from_secs(40),
    );
    assert_eq!(caret(&v), 10);
    click(
        &mut v,
        f64::from(field.x0) + 1.0,
        now + Duration::from_secs(50),
    );
    assert_eq!(caret(&v), 0);
    // Typing goes in at the caret.
    typed(&mut v, "_");
    assert_eq!(v.find_query(), "_alpha beta");
}

#[test]
fn a_drag_selects_and_is_offered_as_primary() {
    let mut v = field_view(600, "alpha beta gamma");
    let now = Instant::now();
    let (x0, x1) = (place_x(&mut v, 3), place_x(&mut v, 8));
    v.press(x0, 16.0, now);
    assert!(v.find_dragging());
    assert!(v.find_drag_to(x1, now));
    assert_eq!(v.find_selected(), "ha be");
    let (x2, x3) = (place_x(&mut v, 1), place_x(&mut v, 14));
    v.find_drag_to(x3, now);
    assert_eq!(v.find_selected(), "ha beta gam");
    v.find_drag_to(x2, now);
    assert_eq!(v.find_selected(), "lp", "backwards from the press");
    assert!(v.release_find_drag(), "a selection is worth offering");
    assert!(!v.find_dragging());
    assert!(!v.release_find_drag());
    // A plain click selects nothing to offer.
    assert!(!click(&mut v, x0, now + Duration::from_secs(5)));
    // Shift extends a selection from the caret.
    v.set_shift(true);
    let x = place_x(&mut v, 9);
    v.press(x, 16.0, now + Duration::from_secs(10));
    v.release_find_drag();
    v.set_shift(false);
    assert_eq!(v.find_selected(), "ha bet");
}

#[test]
fn double_click_selects_a_word_and_triple_click_everything() {
    let mut v = field_view(600, "alpha beta gamma");
    let now = Instant::now();
    let x = over_x(&mut v, 7, 1);
    v.press(x, 16.0, now);
    v.release_find_drag();
    assert_eq!(v.find_selected(), "");
    v.press(x, 16.0, now + Duration::from_millis(100));
    assert!(v.release_find_drag());
    assert_eq!(v.find_selected(), "beta");
    v.press(x, 16.0, now + Duration::from_millis(200));
    assert!(v.release_find_drag());
    assert_eq!(v.find_selected(), "alpha beta gamma");
    // A fourth begins again.
    v.press(x, 16.0, now + Duration::from_millis(300));
    v.release_find_drag();
    assert_eq!(v.find_selected(), "");
    // Too late, or too far, is a click of its own.
    v.press(x, 16.0, now + Duration::from_secs(5));
    v.release_find_drag();
    v.press(
        x,
        16.0,
        now + Duration::from_secs(5) + Duration::from_millis(900),
    );
    v.release_find_drag();
    assert_eq!(v.find_selected(), "");
    // A drag after a double click goes by words.
    let (a, b) = (over_x(&mut v, 7, 1), over_x(&mut v, 12, 1));
    let t = now + Duration::from_secs(20);
    click(&mut v, a, t);
    v.press(a, 16.0, t + Duration::from_millis(100));
    v.find_drag_to(b, t + Duration::from_millis(100));
    assert_eq!(v.find_selected(), "beta gamma");
    let c = over_x(&mut v, 1, 1);
    v.find_drag_to(c, t + Duration::from_millis(100));
    assert_eq!(v.find_selected(), "alpha beta");
    v.release_find_drag();
    // A double click on white space selects it, one on the end of the text its last word.
    let t = now + Duration::from_secs(40);
    let x = over_x(&mut v, 5, 1);
    click(&mut v, x, t);
    click(&mut v, x, t + Duration::from_millis(100));
    assert_eq!(v.find_selected(), " ");
    let t = now + Duration::from_secs(60);
    let end = f64::from(v.find_geometry().1.x1) - 1.0;
    click(&mut v, end, t);
    click(&mut v, end, t + Duration::from_millis(100));
    assert_eq!(v.find_selected(), "gamma");
}

#[test]
fn a_press_elsewhere_ends_a_double_click_on_the_field() {
    let mut v = field_view(600, "alpha beta gamma");
    let now = Instant::now();
    let x = over_x(&mut v, 7, 1);
    // A button of the bar between two clicks on the field.
    click(&mut v, x, now);
    let (_, button) = v.find_buttons()[0];
    let (bx, by) = ((button.x0 + button.x1) / 2.0, (button.y0 + button.y1) / 2.0);
    v.press(
        f64::from(bx),
        f64::from(by),
        now + Duration::from_millis(100),
    );
    v.release(now + Duration::from_millis(150));
    click(&mut v, x, now + Duration::from_millis(200));
    assert_eq!(v.find_selected(), "", "a click, not a double click");
    // The document between two clicks on the field.
    let t = now + Duration::from_secs(5);
    click(&mut v, x, t);
    v.press(40.0, 150.0, t + Duration::from_millis(100));
    v.release(t + Duration::from_millis(150));
    assert!(!v.find_focused());
    click(&mut v, x, t + Duration::from_millis(200));
    assert_eq!(v.find_selected(), "", "a click, not a double click");
    assert!(v.find_focused());
}

#[test]
fn a_drag_past_the_end_of_the_field_scrolls_the_text_on_its_own() {
    let mut v = field_view(300, "");
    v.find_type(&"abcdefghij".repeat(10));
    press(&mut v, named(NamedKey::Home), None, NONE);
    let (_, inner) = v.find_geometry();
    let now = Instant::now();
    v.press(f64::from(inner.x0) + 10.0, 16.0, now);
    // Inside the field: no timer.
    v.find_drag_to(f64::from(inner.x0) + 50.0, now);
    assert_eq!(v.find_deadline(), None);
    // Past its right end: the text scrolls a step when the time comes, and the selection grows.
    let past = f64::from(inner.x1) + 40.0;
    v.find_drag_to(past, now);
    let due = v.find_deadline().expect("a step is due");
    assert!(!v.find_expire(now), "not yet");
    let mut t = due;
    let mut last = (v.find.scroll, v.find_selected().len());
    for _ in 0..10 {
        assert!(v.find_expire(t));
        assert!(v.find.scroll > last.0 && v.find_selected().len() > last.1);
        last = (v.find.scroll, v.find_selected().len());
        t = v.find_deadline().expect("it goes on") + Duration::ZERO;
    }
    // Back inside the field the timer is gone.
    v.find_drag_to(f64::from(inner.x0 + inner.x1) / 2.0, t);
    assert_eq!(v.find_deadline(), None);
    // Past the left end it scrolls back.
    v.find_drag_to(f64::from(inner.x0) - 30.0, t);
    let before = v.find.scroll;
    let t = v.find_deadline().unwrap();
    assert!(v.find_expire(t));
    assert!(v.find.scroll < before);
    v.release_find_drag();
    assert_eq!(v.find_deadline(), None);
    // The scroll stops at the end of the text, and the timer with it.
    press(&mut v, named(NamedKey::Home), None, NONE);
    v.press(f64::from(inner.x0) + 10.0, 16.0, t);
    v.find_drag_to(past, t);
    for _ in 0..2000 {
        let Some(due) = v.find_deadline() else { break };
        v.find_expire(due);
    }
    assert_eq!(v.find_deadline(), None, "the end of the text was reached");
    assert_eq!(
        v.find_caret(),
        100,
        "the selection went to the end of the query"
    );
}

#[test]
fn the_text_cursor_is_over_the_field() {
    let mut v = field_view(600, "abc");
    paint(&mut v, Instant::now());
    let (field, _) = v.find_geometry();
    let (x, y) = (f64::from(field.x0 + 20.0), 16.0);
    assert!(v.text_cursor(x, y));
    assert!(v.find_over_field(x, y));
    // Not over the buttons, or the document, or when the bar is closed.
    assert!(!v.text_cursor(590.0, 16.0));
    assert!(!v.text_cursor(300.0, 100.0));
    v.set_find(false);
    assert!(!v.text_cursor(x, y));
}

// ---- Paste -------------------------------------------------------------------------------------

#[test]
fn a_paste_is_its_first_line_with_tabs_for_spaces_and_no_other_control_characters() {
    assert_eq!(find::pasted_query("one line"), "one line");
    assert_eq!(find::pasted_query("first\nsecond"), "first");
    assert_eq!(find::pasted_query("first\r\nsecond"), "first");
    assert_eq!(find::pasted_query("first\rsecond"), "first");
    assert_eq!(find::pasted_query("trailing\n"), "trailing");
    assert_eq!(find::pasted_query("\nafter a blank line"), "");
    assert_eq!(find::pasted_query("a\tb\t\tc"), "a b  c");
    assert_eq!(find::pasted_query("a\u{7}b\u{1b}[0mc\u{7f}d"), "ab[0mcd");
    assert_eq!(
        find::pasted_query("日本\u{200b}語 é😀"),
        "日本\u{200b}語 é😀"
    );
    assert_eq!(find::pasted_query(""), "");
}

/// Every character the layout breaks a line at.
const BREAKS: [char; 7] = [
    '\r', '\n', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}',
];

/// The height of the query as the field lays it out.
fn field_height(v: &mut View) -> f32 {
    v.find_text().height
}

#[test]
fn a_paste_with_any_line_separator_is_its_first_line_and_the_field_stays_one_line() {
    let mut one = field_view(600, "first");
    let height = field_height(&mut one);
    for c in BREAKS {
        assert_eq!(
            find::pasted_query(&format!("first{c}second")),
            "first",
            "{c:?}"
        );
        assert_eq!(find::pasted_query(&format!("{c}first")), "", "{c:?}");
        let mut v = field_view(600, "");
        let ticket = v.find_ticket();
        assert!(v.find_paste(ticket, &format!("first{c}second")), "{c:?}");
        assert_eq!(v.find_query(), "first", "{c:?}");
        assert_eq!(field_height(&mut v), height, "{c:?}: one line");
    }
    assert_eq!(find::pasted_query("first\r\nsecond"), "first");
}

#[test]
fn typed_and_committed_text_with_a_line_separator_makes_no_second_line() {
    let mut one = field_view(600, "ab");
    let height = field_height(&mut one);
    for c in BREAKS {
        let mut v = field_view(600, "");
        v.find_type(&format!("a{c}b"));
        assert_eq!(v.find_query(), "ab", "{c:?}: an input method's commit");
        assert_eq!(field_height(&mut v), height, "{c:?}");
        let mut v = field_view(600, "");
        let text = format!("a{c}");
        press(&mut v, ch("a"), Some(&text), NONE);
        assert_eq!(v.find_query(), "a", "{c:?}: a key's text");
        assert_eq!(
            field_height(&mut v),
            field_height(&mut field_view(600, "a"))
        );
    }
}

#[cfg(feature = "clipboard")]
#[test]
fn a_document_selection_with_any_line_separator_is_no_query() {
    // The parser turns a CR, LF and the like in a paragraph into a space; these it keeps.
    for c in ['\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}'] {
        let mut v = view(&format!("Hello{c}world\n"), 800, 600);
        assert!(v.select_all());
        open(&mut v);
        assert_eq!(v.find_query(), "", "{c:?}");
    }
    assert!(find::is_line_break('\u{2028}') && find::is_line_break('\u{85}'));
    assert!(!find::is_line_break('\t') && !find::is_line_break(' '));
}

#[test]
fn a_paste_goes_in_at_the_caret_over_the_selection_and_up_to_256_characters() {
    let mut v = field_view(600, "alpha beta");
    let ticket = v.find_ticket();
    assert!(v.find_paste(ticket, "XY\nignored"));
    assert_eq!(
        v.find_query(),
        "alpha betaXY",
        "at the caret, which was at the end"
    );
    press(&mut v, named(NamedKey::ArrowLeft), None, NONE);
    assert!(v.find_paste(ticket, "\t-"));
    assert_eq!(v.find_query(), "alpha betaX -Y");
    // Over a selection.
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    assert!(v.find_paste(ticket, "XY"));
    assert_eq!(
        v.find_query(),
        "XY",
        "the query was selected, so it is replaced"
    );
    // Nothing to paste (an empty text, or only a line break) changes nothing, whatever is selected.
    press(&mut v, named(NamedKey::Home), None, SHIFT);
    assert!(!v.find_paste(ticket, ""));
    assert!(!v.find_paste(ticket, "\nx"));
    assert_eq!(v.find_query(), "XY");
    // The cap holds, a selection making room.
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    v.find_paste(ticket, &"a".repeat(300));
    assert_eq!(v.find_query().chars().count(), MAX_QUERY);
    press(&mut v, named(NamedKey::Home), None, NONE);
    for _ in 0..4 {
        press(&mut v, named(NamedKey::ArrowRight), None, SHIFT);
    }
    v.find_paste(ticket, &"b".repeat(10));
    assert_eq!(v.find_query().chars().count(), MAX_QUERY);
    assert_eq!(v.find_query().matches('b').count(), 4);
    // The matches are found again.
    v.find_paste(ticket, "x");
    assert!(v.find_matches().is_empty());
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    v.find_paste(ticket, "text");
    assert_eq!(v.find_matches().len(), 2);
}

#[test]
fn a_paste_that_comes_after_the_bar_closed_or_the_field_was_left_inserts_nothing() {
    let mut v = field_view(600, "abc");
    let now = Instant::now();
    // The bar is closed in between.
    let ticket = v.find_ticket();
    v.set_find(false);
    assert!(!v.find_paste(ticket, "x"));
    // Closed and open again in between: the same.
    let ticket = {
        v.set_find(true);
        v.find_ticket()
    };
    v.set_find(false);
    v.set_find(true);
    assert!(!v.find_paste(ticket, "x"), "another opening");
    assert_eq!(v.find_query(), "abc");
    // The field is left in between (a click on the document).
    let ticket = v.find_ticket();
    v.press(400.0, 250.0, now);
    v.release(now);
    assert!(!v.find_paste(ticket, "x"));
    assert!(
        !v.find_paste(v.find_ticket(), "x"),
        "and while it stays left"
    );
    assert_eq!(v.find_query(), "abc");
    // The field has the keyboard again: a paste asked for now lands, and one asked for
    // before the click still does not.
    let end = place_x(&mut v, 3);
    click(&mut v, end, now);
    assert!(!v.find_paste(ticket, "x"));
    let fresh = v.find_ticket();
    assert!(v.find_paste(fresh, "x"));
    assert_eq!(v.find_query(), "abcx");
    // Typing in between does not matter.
    let ticket = v.find_ticket();
    typed(&mut v, "yz");
    assert!(v.find_paste(ticket, "!"));
    assert_eq!(v.find_query(), "abcxyz!");
}

#[cfg(feature = "clipboard")]
#[test]
fn copy_cut_and_paste_keys_are_the_fields_while_it_has_the_keyboard() {
    use crate::clipboard::Which;
    let text = |c: Option<Content>| match c {
        Some(Content::Text(t)) => Some(t.to_string()),
        _ => None,
    };
    let mut v = field_view(600, "alpha beta");
    // Nothing selected: nothing to copy or cut.
    press(&mut v, named(NamedKey::End), None, NONE);
    assert_eq!(
        press(&mut v, ch("c"), Some("\u{3}"), CTRL),
        Outcome::Nothing
    );
    assert_eq!(
        press(&mut v, ch("x"), Some("\u{18}"), CTRL),
        Outcome::Nothing
    );
    assert_eq!(v.find_query(), "alpha beta");
    // Copy: the selection is offered, the query stays.
    press(&mut v, named(NamedKey::ArrowLeft), None, CTRL | SHIFT);
    assert_eq!(press(&mut v, ch("c"), Some("\u{3}"), CTRL), Outcome::Copy);
    assert_eq!(text(v.offered()), Some("beta".into()));
    assert_eq!(v.find_query(), "alpha beta");
    assert_eq!(
        press(&mut v, named(NamedKey::Insert), None, CTRL),
        Outcome::Copy
    );
    assert_eq!(
        press(&mut v, ch("C"), Some("\u{3}"), CTRL | SHIFT),
        Outcome::Copy
    );
    // Cut: the selection leaves the query, and waits to be offered.
    assert_eq!(press(&mut v, ch("x"), Some("\u{18}"), CTRL), Outcome::Cut);
    assert_eq!(v.find_query(), "alpha ");
    assert_eq!(text(v.take_cut()), Some("beta".into()));
    assert_eq!(text(v.take_cut()), None, "once");
    // Paste: Ctrl+V and Shift+Insert ask for the clipboard, the shell reads it.
    assert_eq!(
        press(&mut v, ch("v"), Some("\u{16}"), CTRL),
        Outcome::Paste(Which::Clipboard)
    );
    assert_eq!(
        press(&mut v, named(NamedKey::Insert), None, SHIFT),
        Outcome::Paste(Which::Clipboard)
    );
    // The matches follow a cut.
    assert_eq!(v.find_matches().len(), 0);
    // Not while the document has the keyboard: Ctrl+C is the document's copy, with its own
    // selection (none: nothing), and Ctrl+V and Ctrl+X do nothing there.
    let now = Instant::now();
    v.press(400.0, 250.0, now);
    v.release(now);
    assert_eq!(
        press(&mut v, ch("c"), Some("\u{3}"), CTRL),
        Outcome::Nothing
    );
    assert_eq!(
        press(&mut v, ch("v"), Some("\u{16}"), CTRL),
        Outcome::Nothing
    );
    assert_eq!(
        press(&mut v, ch("x"), Some("\u{18}"), CTRL),
        Outcome::Nothing
    );
    assert_eq!(v.find_query(), "alpha ");
}

#[cfg(feature = "clipboard")]
#[test]
fn a_middle_click_on_the_field_puts_the_caret_there_and_asks_for_a_paste_of_primary() {
    let mut v = field_view(600, "alpha beta");
    let now = Instant::now();
    let x = place_x(&mut v, 3);
    let ticket = v.find_middle(x, 16.0).expect("on the field");
    assert_eq!(caret(&v), 3);
    assert_eq!(v.find_selected(), "");
    assert!(v.find_paste(ticket, "XY"));
    assert_eq!(v.find_query(), "alpXYha beta");
    // Not on the buttons, the document, or a closed bar.
    assert_eq!(v.find_middle(590.0, 16.0), None);
    assert_eq!(v.find_middle(300.0, 100.0), None);
    // It takes the keyboard back for the field.
    v.press(400.0, 250.0, now);
    v.release(now);
    assert!(!v.find_focused());
    let x = place_x(&mut v, 0);
    assert!(v.find_middle(x, 16.0).is_some());
    assert!(v.find_focused());
    v.set_find(false);
    assert_eq!(v.find_middle(x, 16.0), None);
}

// ---- Painting ----------------------------------------------------------------------------------

#[test]
fn the_selection_in_the_field_is_washed_and_the_caret_shows_only_when_focused() {
    let theme = LIGHT;
    let mut v = field_view(400, "alpha beta");
    let now = Instant::now();
    let count = |buf: &[u32], color| {
        let (w, _) = (400usize, 0);
        buf[..w * 32].iter().filter(|&&p| p == color).count()
    };
    // Focused with the whole query selected (as it opens after a Ctrl+A): the wash, the caret.
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    let buf = paint(&mut v, now);
    let wash = count(&buf, theme.selection);
    assert!(wash > 100, "{wash}");
    assert!(count(&buf, theme.text) > 0);
    // Nothing selected: no wash.
    press(&mut v, named(NamedKey::End), None, NONE);
    let buf = paint(&mut v, now);
    assert_eq!(count(&buf, theme.selection), 0);
    // A wider selection washes more.
    press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT);
    let one = count(&paint(&mut v, now), theme.selection);
    press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT);
    let two = count(&paint(&mut v, now), theme.selection);
    assert!(one > 0 && two > one, "{one} {two}");
    // Blurred: dimmer (a different color), and no caret.
    let focused = paint(&mut v, now);
    v.press(400.0 - 1.0, 250.0, now);
    v.release(now);
    assert!(!v.find_focused());
    let blurred = paint(&mut v, now);
    assert_eq!(
        count(&blurred, theme.selection),
        0,
        "dimmer than the focused wash"
    );
    assert!(blurred[..400 * 32] != focused[..400 * 32]);
    let dim = blurred[..400 * 32]
        .iter()
        .filter(|&&p| p != theme.code_bg && p != theme.bg && p != theme.border)
        .count();
    assert!(dim > 0);
}

// ---- Right to left text in the field -----------------------------------------------------------

const HEBREW: &str = "שלום";
const MIXED: &str = "abc שלום def";

/// The columns of the field's rows that are washed as a selection (the first and the last, and
/// how many in all).
fn washed_columns(v: &mut View) -> Option<(i32, i32, usize)> {
    let buf = paint(v, Instant::now());
    let (field, _) = v.find.rects.unwrap();
    let (x0, y0, x1, y1) = field.px();
    let width = v.size().0 as usize;
    let mut columns = Vec::new();
    for x in x0..x1 {
        let hit = (y0..y1).any(|y| buf[y as usize * width + x as usize] == LIGHT.selection);
        if hit {
            columns.push(x);
        }
    }
    Some((*columns.first()?, *columns.last()?, columns.len()))
}

/// The window x range of the clusters that start in the bytes `lo..hi` of the query, as laid out.
fn extent_of(v: &mut View, lo: u32, hi: u32) -> (f32, f32) {
    let (_, inner) = v.find_geometry();
    let text = v.find_text();
    let near = text
        .content
        .hits
        .iter()
        .filter(|h| (lo..hi).contains(&h.doc))
        .map(|h| (h.x0.min(h.x1), h.x0.max(h.x1)))
        .fold((f32::MAX, f32::MIN), |a, h| (a.0.min(h.0), a.1.max(h.1)));
    (
        inner.x0 - v.find.scroll + near.0,
        inner.x0 - v.find.scroll + near.1,
    )
}

fn caret_shows(v: &mut View) -> bool {
    paint(v, Instant::now());
    let (_, inner) = v.find_geometry();
    v.find.caret >= inner.x0 && v.find.caret + v.find_caret_width() <= inner.x1 + 0.5
}

#[test]
fn the_selection_of_a_right_to_left_word_in_a_left_to_right_query_is_washed_where_its_glyphs_are() {
    let mut v = field_view(600, "");
    v.find_type(MIXED);
    // `שלום`, in the middle of the query: its clusters, not the stretch between its ends.
    let (lo, hi) = (
        MIXED.find(HEBREW).unwrap(),
        MIXED.find(HEBREW).unwrap() + HEBREW.len(),
    );
    v.find.field.select(lo, hi);
    let (a, b) = extent_of(&mut v, lo as u32, hi as u32);
    let (first, last, count) = washed_columns(&mut v).expect("the selection is washed");
    assert!(
        (first as f32 - a).abs() <= 2.0 && (last as f32 - b).abs() <= 2.0,
        "washed {first}..{last}, the glyphs are at {a}..{b}"
    );
    assert!(
        count as f32 >= (b - a) * 0.9,
        "{count} columns for {a}..{b}"
    );
    // And nothing of the Latin letters around it.
    let before = extent_of(&mut v, 0, 3);
    let after = extent_of(&mut v, 13, 16);
    assert!(first as f32 >= before.1 - 1.0, "{first} is over `abc`");
    assert!(last as f32 <= after.0 + 1.0, "{last} is over `def`");
}

#[test]
fn the_selection_of_a_right_to_left_query_is_washed_over_all_of_it_or_the_part_selected() {
    let mut v = field_view(600, "");
    v.find_type(HEBREW);
    let all = extent_of(&mut v, 0, HEBREW.len() as u32);
    v.find.field.select(0, HEBREW.len());
    let (first, last, _) = washed_columns(&mut v).expect("washed");
    assert!((first as f32 - all.0).abs() <= 2.0 && (last as f32 - all.1).abs() <= 2.0);
    // The first two letters are the right-hand ones.
    v.find.field.select(0, 4);
    let part = extent_of(&mut v, 0, 4);
    let (first, last, _) = washed_columns(&mut v).expect("washed");
    assert!((first as f32 - part.0).abs() <= 2.0 && (last as f32 - part.1).abs() <= 2.0);
    assert!(
        part.0 > all.0 + 5.0 && (part.1 - all.1).abs() < 1.0,
        "{part:?} in {all:?}"
    );
}

#[test]
fn the_caret_stays_in_the_field_in_right_to_left_and_mixed_queries() {
    for query in [
        HEBREW.to_owned(),
        MIXED.to_owned(),
        format!("{HEBREW} ").repeat(12),
        format!("{MIXED} ").repeat(8),
    ] {
        let mut v = field_view(260, "");
        v.find_type(&query);
        assert!(caret_shows(&mut v), "{query}: after typing");
        for (key, mods) in [
            (NamedKey::Home, NONE),
            (NamedKey::End, NONE),
            (NamedKey::Home, SHIFT),
            (NamedKey::ArrowRight, CTRL),
            (NamedKey::ArrowLeft, NONE),
            (NamedKey::End, SHIFT),
            (NamedKey::ArrowLeft, CTRL),
        ] {
            press(&mut v, named(key), None, mods);
            assert!(caret_shows(&mut v), "{query}: after {key:?} {mods:?}");
        }
        // The text can be scrolled no further than the end of what shows.
        let (_, inner) = v.find_geometry();
        let text = v.find_text();
        assert!(
            v.find.scroll
                <= (text.width + v.find_caret_width() - (inner.x1 - inner.x0)).max(0.0) + 0.01
        );
    }
}

#[test]
fn home_and_end_of_a_right_to_left_query_put_the_caret_at_the_edges_of_its_text() {
    let mut v = field_view(600, "");
    v.find_type(HEBREW);
    let (_, inner) = v.find_geometry();
    let text = v.find_text();
    // The text starts at the field's left, and its width is that of its glyphs.
    let hits = &text.content.hits;
    let right = hits.iter().map(|h| h.x1).fold(0.0, f32::max);
    assert!(
        (text.width - right).abs() < 0.01 && text.width < 100.0,
        "{}",
        text.width
    );
    press(&mut v, named(NamedKey::Home), None, NONE);
    paint(&mut v, Instant::now());
    let home = v.find.caret;
    press(&mut v, named(NamedKey::End), None, NONE);
    paint(&mut v, Instant::now());
    let end = v.find.caret;
    // Home is the start of the text, which is at its right; End is the left end.
    assert!(
        home > end && (home - inner.x0 - text.width).abs() < 1.0,
        "{home} {end}"
    );
    assert!((end - inner.x0).abs() < 1.5, "{end} against {}", inner.x0);
}

#[test]
fn a_click_and_a_double_click_on_right_to_left_text_find_the_character_under_them() {
    let mut v = field_view(600, "");
    v.find_type(MIXED);
    let now = Instant::now();
    // The middle of `ו`, the third letter of the word.
    let at = MIXED.find(HEBREW).unwrap() + 4;
    let x = over_x(&mut v, at, 2);
    v.press(x, 16.0, now);
    v.release_find_drag();
    assert_eq!(v.find_selected(), "");
    assert!([at, at + 2].contains(&caret(&v)), "caret at {}", caret(&v));
    v.press(x, 16.0, now + Duration::from_millis(100));
    v.release_find_drag();
    assert_eq!(v.find_selected(), HEBREW);
}

#[test]
fn a_drag_across_right_to_left_text_selects_the_letters_between() {
    let mut v = field_view(600, "");
    v.find_type(HEBREW);
    // From the second letter's leading edge (on its right) to the fourth's, going left.
    let (a, b) = (place_x(&mut v, 2), place_x(&mut v, 6));
    assert!(a > b);
    let now = Instant::now();
    v.press(a, 16.0, now);
    v.find_drag_to(b, now);
    v.release_find_drag();
    assert_eq!(v.find_selected(), "לו");
    // And the other way, back past where it began.
    v.press(b, 16.0, now + Duration::from_secs(10));
    let start = place_x(&mut v, 0);
    v.find_drag_to(start, now + Duration::from_secs(10));
    v.release_find_drag();
    assert_eq!(v.find_selected(), "שלו");
}

#[test]
fn a_press_on_the_toolbar_ends_a_double_click_on_the_field() {
    let mut v = view("some text to search\n", 600, 300);
    v.set_toolbar(true);
    open(&mut v);
    v.find_type("alpha beta gamma");
    paint(&mut v, Instant::now());
    let now = Instant::now();
    let x = over_x(&mut v, 7, 1);
    let y = f64::from(v.find_geometry().0.y0) + 4.0;
    // A button of the toolbar (the find button itself: its press only arms it).
    let slots = toolbar::slots(&v.toolbar_ctx());
    let (_, rect) = *slots
        .items
        .iter()
        .find(|(i, _)| *i == toolbar::Item::Find)
        .expect("a find button");
    let (bx, by) = (
        f64::from(rect.x0 + rect.x1) / 2.0,
        f64::from(rect.y0 + rect.y1) / 2.0,
    );
    v.press(x, y, now);
    v.release_find_drag();
    assert!(v.press(bx, by, now + Duration::from_millis(50)));
    v.release_toolbar(bx, by);
    v.take_command();
    v.press(x, y, now + Duration::from_millis(100));
    v.release_find_drag();
    assert_eq!(
        v.find_selected(),
        "",
        "a click of its own, not a double one"
    );
    // Without the toolbar's press between them, the same two make a double click.
    v.press(x, y, now + Duration::from_secs(10));
    v.release_find_drag();
    v.press(
        x,
        y,
        now + Duration::from_secs(10) + Duration::from_millis(100),
    );
    v.release_find_drag();
    assert_eq!(v.find_selected(), "beta");
}

/// The stretches `v`'s field washes for its selection, from the text's own layout.
fn washes(v: &mut View) -> Vec<(f32, f32)> {
    let text = v.find_text();
    let (lo, hi) = v.find.field.range();
    let mut out = Vec::new();
    text.selection(lo, hi, |x0, x1| out.push((x0, x1)));
    out
}

#[test]
fn a_selected_space_at_the_end_of_the_query_is_washed_and_not_the_letter_before() {
    let mut v = field_view(600, "abc ");
    press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT);
    assert_eq!(v.find_selected(), " ");
    let text = v.find_text();
    let (a, b) = (text.x(3), text.x(4));
    assert!(b > a, "the space has an extent: {a}..{b}");
    let washed = washes(&mut v);
    assert_eq!(washed.len(), 1, "{washed:?}");
    assert!(
        (washed[0].0 - a).abs() < 0.5 && (washed[0].1 - b).abs() < 0.5,
        "washed {washed:?}, the space is at {a}..{b}"
    );
    // Selecting the last two characters washes the letter and the space together, to the end.
    press(&mut v, named(NamedKey::ArrowLeft), None, SHIFT);
    let washed = washes(&mut v);
    assert_eq!(washed.len(), 1);
    assert!((washed[0].0 - text.x(2)).abs() < 0.5 && (washed[0].1 - b).abs() < 0.5);
}

#[test]
fn the_selection_of_a_query_of_spaces_is_washed() {
    let mut v = field_view(600, "   ");
    press(&mut v, ch("a"), Some("\u{1}"), CTRL);
    assert_eq!(v.find_selected(), "   ");
    let text = v.find_text();
    let washed = washes(&mut v);
    assert_eq!(washed.len(), 1, "{washed:?}");
    assert!(
        (washed[0].0 - text.x(0)).abs() < 0.5 && (washed[0].1 - text.x(3)).abs() < 0.5,
        "{washed:?} against {}..{}",
        text.x(0),
        text.x(3)
    );
    assert!(washed[0].1 > washed[0].0);
    paint(&mut v, Instant::now());
}

#[test]
fn the_caret_and_clicks_past_spaces_at_the_end_of_the_query_are_after_them() {
    let mut v = field_view(600, "ab  ");
    paint(&mut v, Instant::now());
    let (_, inner) = v.find_geometry();
    let text = v.find_text();
    assert!(text.x(4) > text.x(3) && text.x(3) > text.x(2));
    assert!((v.find.caret - (inner.x0 - v.find.scroll + text.x(4))).abs() < 0.5);
    // A click well past the end puts the caret at the end; one over the second space, in it.
    let now = Instant::now();
    click(&mut v, f64::from(inner.x1) - 4.0, now);
    assert_eq!(caret(&v), 4);
    let x = over_x(&mut v, 3, 1);
    click(&mut v, x, now + Duration::from_secs(10));
    assert!([3, 4].contains(&caret(&v)));
    assert_eq!(v.find_text().over(text.x(3) + 1.0), 3, "over the space");
}
