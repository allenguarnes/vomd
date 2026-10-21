use super::Motion::*;
use super::*;

/// A field holding `text` with the caret at `caret` (a character count), nothing selected.
fn field(text: &str, caret: usize) -> Field {
    let mut f = Field::default();
    f.set(text);
    let at = text.char_indices().nth(caret).map_or(text.len(), |c| c.0);
    f.place(at, false);
    f
}

/// The text with the selection in brackets and the caret as `|` where nothing is selected.
fn show(f: &Field) -> String {
    let (lo, hi) = f.range();
    let t = f.text();
    if lo == hi {
        format!("{}|{}", &t[..lo], &t[lo..])
    } else if f.caret() == hi {
        format!("{}[{}]|{}", &t[..lo], &t[lo..hi], &t[hi..])
    } else {
        format!("{}|[{}]{}", &t[..lo], &t[lo..hi], &t[hi..])
    }
}

#[test]
fn a_new_text_is_selected_whole() {
    let mut f = Field::default();
    assert!(!f.all_selected(), "an empty field has nothing to select");
    f.set("hello");
    assert!(f.all_selected());
    assert_eq!(f.selection(), "hello");
    assert_eq!(show(&f), "[hello]|");
    f.place(2, false);
    assert!(!f.has_selection() && !f.all_selected());
    f.select_all();
    assert_eq!(show(&f), "[hello]|");
}

#[test]
fn left_and_right_move_by_character_and_stop_at_the_ends() {
    let mut f = field("abc", 3);
    f.go(Left, false);
    assert_eq!(show(&f), "ab|c");
    f.go(Left, false);
    f.go(Left, false);
    f.go(Left, false);
    assert_eq!(show(&f), "|abc");
    f.go(Right, false);
    assert_eq!(show(&f), "a|bc");
    for _ in 0..5 {
        f.go(Right, false);
    }
    assert_eq!(show(&f), "abc|");
}

#[test]
fn home_and_end_go_to_the_ends() {
    let mut f = field("one two", 3);
    f.go(Home, false);
    assert_eq!(show(&f), "|one two");
    f.go(End, false);
    assert_eq!(show(&f), "one two|");
}

#[test]
fn multi_byte_characters_are_never_split() {
    let mut f = field("é日😀z", 0);
    let mut seen = vec![f.caret()];
    for _ in 0..4 {
        f.go(Right, false);
        seen.push(f.caret());
    }
    assert_eq!(seen, [0, 2, 5, 9, 10]);
    for want in [9, 5, 2, 0] {
        f.go(Left, false);
        assert_eq!(f.caret(), want);
    }
    // A place in the middle of a character is the start of it.
    f.place(3, false);
    assert_eq!(f.caret(), 2);
    f.place(7, false);
    assert_eq!(f.caret(), 5);
    f.place(99, false);
    assert_eq!(f.caret(), 10);
}

#[test]
fn combining_marks_stay_with_their_character_in_motions() {
    // "e" with an acute accent as two characters, then "x".
    let mut f = field("e\u{301}x", 0);
    f.go(Right, false);
    assert_eq!(f.caret(), 3, "the mark is passed with its base");
    f.go(Left, false);
    assert_eq!(f.caret(), 0);
    // Backspace takes one character, so the accent can be taken off the letter.
    let mut f = field("e\u{301}", 2);
    assert!(f.backspace(false));
    assert_eq!(f.text(), "e");
    // Delete takes the character with its marks.
    let mut f = field("e\u{301}x", 0);
    assert!(f.delete(false));
    assert_eq!(f.text(), "x");
}

#[test]
fn words_are_runs_of_letters_or_of_punctuation() {
    let mut f = field("foo  bar.baz_1 qux", 0);
    let mut stops = Vec::new();
    for _ in 0..6 {
        f.go(WordRight, false);
        stops.push(f.caret());
    }
    // End of "foo", of "bar", the ".", "baz_1", "qux", and the end again.
    assert_eq!(stops, [3, 8, 9, 14, 18, 18]);
    let mut back = Vec::new();
    for _ in 0..6 {
        f.go(WordLeft, false);
        back.push(f.caret());
    }
    assert_eq!(back, [15, 9, 8, 5, 0, 0]);
}

#[test]
fn words_across_scripts_and_marks() {
    let mut f = field("日本語 café ünï", 0);
    f.go(WordRight, false);
    assert_eq!(show(&f), "日本語| café ünï");
    f.go(WordRight, false);
    assert_eq!(show(&f), "日本語 café| ünï");
    f.go(WordLeft, false);
    assert_eq!(show(&f), "日本語 |café ünï");
}

#[test]
fn shift_extends_the_selection_from_where_the_caret_was() {
    let mut f = field("abcdef", 2);
    f.go(Right, true);
    f.go(Right, true);
    assert_eq!(show(&f), "ab[cd]|ef");
    f.go(Left, true);
    assert_eq!(show(&f), "ab[c]|def");
    for _ in 0..3 {
        f.go(Left, true);
    }
    assert_eq!(show(&f), "|[ab]cdef", "the caret went past the anchor");
    assert_eq!(f.selection(), "ab");
    f.go(End, true);
    assert_eq!(show(&f), "ab[cdef]|");
    f.go(Home, true);
    assert_eq!(show(&f), "|[ab]cdef");
    f.go(WordRight, true);
    assert_eq!(show(&f), "ab[cdef]|");
}

#[test]
fn an_arrow_without_shift_gives_up_a_selection_at_its_edge() {
    let mut f = field("abcdef", 1);
    f.go(Right, true);
    f.go(Right, true);
    assert_eq!(show(&f), "a[bc]|def");
    f.go(Left, false);
    assert_eq!(show(&f), "a|bcdef", "Left goes to the start");
    let mut f = field("abcdef", 1);
    f.go(Right, true);
    f.go(Right, true);
    f.go(Right, false);
    assert_eq!(show(&f), "abc|def", "Right goes to the end");
    // A selection made backwards collapses to the same edges.
    let mut f = field("abcdef", 3);
    f.go(Left, true);
    f.go(Left, true);
    f.go(Right, false);
    assert_eq!(show(&f), "abc|def");
    // The other motions go on from the caret.
    let mut f = field("ab cd ef", 3);
    f.go(Right, true);
    f.go(Right, true);
    f.go(WordRight, false);
    assert_eq!(show(&f), "ab cd ef|");
    f.select(0, 4);
    f.go(Home, false);
    assert_eq!(show(&f), "|ab cd ef");
}

#[test]
fn typing_replaces_the_selection_or_goes_at_the_caret() {
    let mut f = field("abcdef", 3);
    assert!(f.insert("XY"));
    assert_eq!(show(&f), "abcXY|def");
    f.select(1, 4);
    assert!(f.insert("-"));
    assert_eq!(show(&f), "a-|Ydef");
    f.select_all();
    assert!(f.insert("new"));
    assert_eq!(show(&f), "new|");
    // Nothing typed is no edit, and keeps the selection.
    f.select_all();
    assert!(!f.insert(""));
    assert_eq!(show(&f), "[new]|");
}

#[test]
fn backspace_and_delete_take_the_selection_or_a_character() {
    let mut f = field("abcd", 2);
    assert!(f.backspace(false));
    assert_eq!(show(&f), "a|cd");
    assert!(f.delete(false));
    assert_eq!(show(&f), "a|d");
    f.select(0, 2);
    assert!(f.backspace(false));
    assert_eq!(show(&f), "|");
    let mut f = field("abcd", 1);
    f.go(Right, true);
    f.go(Right, true);
    assert!(f.delete(false));
    assert_eq!(show(&f), "a|d");
    // At the ends there is nothing to delete.
    let mut f = field("ab", 0);
    assert!(!f.backspace(false) && !f.backspace(true));
    f.go(End, false);
    assert!(!f.delete(false) && !f.delete(true));
    assert_eq!(f.text(), "ab");
    // An empty field too.
    let mut f = Field::default();
    assert!(!f.backspace(false) && !f.delete(false) && f.cut().is_none());
}

#[test]
fn ctrl_backspace_and_ctrl_delete_take_a_word() {
    let mut f = field("one two three", 7);
    assert!(f.backspace(true));
    assert_eq!(show(&f), "one | three");
    assert!(f.backspace(true));
    assert_eq!(
        show(&f),
        "| three",
        "the white space before the word goes with it"
    );
    let mut f = field("one two three", 3);
    assert!(f.delete(true));
    assert_eq!(show(&f), "one| three");
    assert!(f.delete(true));
    assert_eq!(show(&f), "one|");
    // With a selection, the selection goes whole.
    let mut f = field("one two three", 0);
    f.go(WordRight, true);
    assert!(f.backspace(true));
    assert_eq!(show(&f), "| two three");
    // Punctuation is a word of its own.
    let mut f = field("a.b", 3);
    assert!(f.backspace(true));
    assert_eq!(show(&f), "a.|");
    assert!(f.backspace(true));
    assert_eq!(show(&f), "a|");
}

#[test]
fn the_text_is_at_most_256_characters_whatever_is_selected() {
    let mut f = field(&"a".repeat(MAX_QUERY), MAX_QUERY);
    assert!(!f.insert("b"), "full");
    assert_eq!(f.text().chars().count(), MAX_QUERY);
    // Over a selection the room is the selection's.
    f.select(10, 20);
    assert!(f.insert(&"b".repeat(50)));
    assert_eq!(f.text().chars().count(), MAX_QUERY);
    assert_eq!(f.text().matches('b').count(), 10);
    assert_eq!(f.caret(), 20);
    // Everything selected: a whole new text, cut at the limit, never inside a character.
    f.select_all();
    assert!(f.insert(&"日".repeat(400)));
    assert_eq!(f.text().chars().count(), MAX_QUERY);
    assert_eq!(f.text().len(), MAX_QUERY * 3);
    assert_eq!(f.caret(), f.text().len());
    // In the middle, up to what fits.
    let mut f = field(&"a".repeat(MAX_QUERY - 2), 5);
    assert!(f.insert("é😀xyz"));
    assert_eq!(f.text().chars().count(), MAX_QUERY);
    assert_eq!(&f.text()[5..], format!("é😀{}", "a".repeat(MAX_QUERY - 7)));
}

#[test]
fn double_click_selects_the_word_the_white_space_or_the_punctuation() {
    let f = field("foo  bar.baz", 0);
    assert_eq!(f.word_at(1), (0, 3));
    assert_eq!(f.word_at(0), (0, 3));
    assert_eq!(f.word_at(3), (3, 5), "the white space after a word");
    assert_eq!(f.word_at(5), (5, 8));
    assert_eq!(f.word_at(8), (8, 9), "the dot");
    assert_eq!(f.word_at(10), (9, 12));
    assert_eq!(f.word_at(12), (9, 12), "at the end, the word before it");
    assert_eq!(Field::default().word_at(0), (0, 0));
    let f = field("日本 😀", 0);
    assert_eq!(
        f.word_at(1),
        (0, 6),
        "inside a character is that character's word"
    );
    assert_eq!(f.word_at(6), (6, 7));
    assert_eq!(
        f.word_at(8),
        (7, 11),
        "an emoji is punctuation, a word of its own"
    );
}

#[test]
fn cut_takes_the_selection_out() {
    let mut f = field("abcdef", 1);
    assert_eq!(f.cut(), None);
    f.go(Right, true);
    f.go(Right, true);
    assert_eq!(f.cut().as_deref(), Some("bc"));
    assert_eq!(show(&f), "a|def");
}

#[test]
fn select_snaps_to_characters() {
    let mut f = field("é日z", 0);
    f.select(1, 4);
    assert_eq!(f.range(), (0, 2));
    f.select(99, 0);
    assert_eq!(f.range(), (0, 6));
    assert_eq!(f.caret(), 0);
}
