use std::time::{Duration, Instant};

use super::*;
use crate::doc::Doc;
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

#[test]
fn ctrl_f_opens_and_focuses_the_bar() {
    assert_eq!(key_action(&ch("f"), CTRL), Some(Action::Find));
    assert_eq!(key_action(&ch("F"), CTRL | SHIFT), Some(Action::Find));
    assert_eq!(key_action(&ch("f"), NONE), None);
    let mut v = view("# Title\n\nSome text.\n", 800, 600);
    assert!(!v.find_open());
    assert_eq!(v.top_inset(), 0);
    assert_eq!(press(&mut v, ch("f"), Some("\u{6}"), CTRL), Outcome::Redraw);
    assert!(v.find_open());
    assert_eq!(v.top_inset(), 32);
    // Again: still open, and the query shows selected so typing replaces it.
    typed(&mut v, "abc");
    assert_eq!(v.find_query(), "abc");
    press(&mut v, ch("f"), Some("\u{6}"), CTRL);
    assert!(v.find_open());
    typed(&mut v, "x");
    assert_eq!(v.find_query(), "x");
    // Typing goes on at its end afterwards.
    typed(&mut v, "y");
    assert_eq!(v.find_query(), "xy");
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
    assert_eq!(command, toolbar::Command::Action(Action::ToggleFind));
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
    // Arrows and pages scroll; Home and End jump.
    let top = v.scroll();
    press(&mut v, named(NamedKey::PageDown), None, NONE);
    assert!(v.scroll() > top);
    press(&mut v, named(NamedKey::ArrowDown), None, NONE);
    press(&mut v, named(NamedKey::End), None, NONE);
    assert!(v.scroll() > 0.0);
    press(&mut v, named(NamedKey::Home), None, NONE);
    assert_eq!(v.scroll(), 0.0);
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
    press(&mut v, ch("f"), Some("\u{6}"), CTRL);
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
    // An empty commit leaves the query as it is, selected-looking or not.
    press(&mut v, ch("f"), Some("\u{6}"), CTRL);
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
    // The bar open already: Ctrl+F takes a new selection.
    let mut v = view("Hello world\n", 800, 600);
    open(&mut v);
    typed(&mut v, "zzz");
    assert!(v.select_all());
    press(&mut v, ch("f"), Some("\u{6}"), CTRL);
    assert_eq!(v.find_query(), "Hello world");
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
fn a_long_query_is_cut_from_the_left_so_its_end_shows() {
    let mut v = view("text\n", 400, 300);
    open(&mut v);
    v.find_type(&"abcdefghij".repeat(20));
    v.find_type("END");
    let s = v.hidpi as f32;
    let style = TextStyle {
        bold: false,
        italic: false,
        mono: false,
        size: TEXT * s,
        role: Role::Text,
        deco: 0,
        shift: 0.0,
        link: 0,
    };
    let q = v.find_query().to_owned();
    let (para, pl) = v.fit_tail(&q, style, 200.0);
    assert!(pl.width <= 200.0, "{}", pl.width);
    let shown: String = para.text.clone();
    assert!(shown.starts_with('…') && shown.ends_with("END"), "{shown}");
    assert!(shown.chars().count() > 5);
    // A short one is whole.
    let (para, _) = v.fit_tail("short", style, 200.0);
    assert_eq!(para.text, "short");
    // Painting a narrow window with a long query does not panic, and keeps to its strip.
    for w in [60, 120, 400] {
        v.resize(w, 300);
        paint(&mut v, Instant::now());
    }
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
fn a_click_on_the_document_keeps_the_bar_open_and_typing_still_goes_to_it() {
    let mut v = view("foo bar\n\n[link](https://example.com)\n", 800, 600);
    open(&mut v);
    typed(&mut v, "fo");
    let now = Instant::now();
    let _ = v.press(60.0, 80.0, now);
    v.release(now);
    assert!(v.find_open());
    typed(&mut v, "o");
    assert_eq!(v.find_query(), "foo");
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
    // The reader scrolls away, and asks for the bar again: the query shows selected.
    v.set_anchor(Anchor {
        block: 70,
        frac: 0.0,
    });
    v.ensure();
    press(&mut v, ch("f"), Some("\u{6}"), CTRL);
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
