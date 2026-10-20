use super::*;
use crate::doc::{Doc, MAX_BLOCK_BYTES};

/// The text of each match of `query` in `src`.
fn found<'a>(doc: &'a Doc, query: &str) -> Vec<&'a str> {
    search(doc, query)
        .matches
        .iter()
        .map(|m| &doc.text[m.start as usize..m.end as usize])
        .collect()
}

fn texts(src: &str, query: &str) -> Vec<String> {
    let doc = Doc::parse(src);
    found(&doc, query).into_iter().map(String::from).collect()
}

#[test]
fn letters_match_in_any_case() {
    assert_eq!(texts("Hello hello HELLO hElLo", "hello").len(), 4);
    assert_eq!(
        texts("Hello hello HELLO", "HeLLo"),
        ["Hello", "hello", "HELLO"]
    );
    assert!(texts("Hello", "help").is_empty());
    assert!(texts("Hello", "hello!").is_empty());
}

#[test]
fn non_ascii_letters_match_in_any_case_and_report_their_bytes() {
    let doc = Doc::parse("Ünï ÜNÏ ünï ß straße");
    let f = search(&doc, "üNÏ");
    let at: Vec<(u32, u32)> = f.matches.iter().map(|m| (m.start, m.end)).collect();
    assert_eq!(found(&doc, "üNÏ"), ["Ünï", "ÜNÏ", "ünï"]);
    // Two bytes a letter: the ranges are whole characters, on boundaries.
    assert_eq!(at[0].1 - at[0].0, 5);
    assert!(at.iter().all(|&(a, b)| {
        doc.text.is_char_boundary(a as usize) && doc.text.is_char_boundary(b as usize)
    }));
    // Greek and Cyrillic, and an astral letter (4 bytes in either case).
    assert_eq!(texts("ΑΒΓ αβγ", "βγ").len(), 2);
    assert_eq!(texts("ПРИВЕТ привет", "ВЕТ").len(), 2);
    assert_eq!(texts("𐐀 𐐨", "𐐨").len(), 2);
    // Characters without case match themselves, CJK and emoji included.
    assert_eq!(texts("日本語 日本語 🚀🚀", "日本"), ["日本", "日本"]);
    assert_eq!(texts("🚀🚀 🚀", "🚀"), ["🚀", "🚀", "🚀"]);
}

#[test]
fn a_lowercase_of_another_length_still_matches_the_original_bytes() {
    // `İ` (2 bytes) lowercases to `i` and a combining dot (3 bytes together).
    let doc = Doc::parse("İstanbul");
    assert_eq!(found(&doc, "i\u{307}stan"), ["İstan"]);
    assert_eq!(found(&doc, "İSTAN"), ["İstan"]);
    assert_eq!(found(&doc, "i"), ["İ"]);
    // The decomposed spelling is found by the composed capital, as the folded texts are equal.
    assert_eq!(texts("i\u{307}x", "İx"), ["i\u{307}x"]);
    // The Kelvin sign is a `k` once lowercase (3 bytes to 1).
    assert_eq!(texts("\u{212A}elvin", "kelvin"), ["\u{212A}elvin"]);
    assert_eq!(texts("kelvin", "\u{212A}"), ["k"]);
    // A match is never cut inside a character, whichever folded character it ends at.
    let doc = Doc::parse("xİy");
    let m = search(&doc, "i").matches[0];
    assert_eq!(&doc.text[m.start as usize..m.end as usize], "İ");
}

#[test]
fn a_match_goes_across_spans_but_not_blocks() {
    // Styles and links cut a paragraph into spans, not into texts.
    assert_eq!(texts("foo **bar** baz", "foo bar b"), ["foo bar b"]);
    assert_eq!(texts("a*b*`c`[d](https://x.y)e", "abcde"), ["abcde"]);
    // A soft break is a space.
    assert_eq!(
        texts("foo\nbar", "foo bar"),
        ["foo\nbar".replace('\n', " ")]
    );
    // Blocks are no text together.
    assert!(texts("foo\n\nbar", "foobar").is_empty());
    assert!(texts("foo\n\nbar", "foo bar").is_empty());
    assert!(texts("# foo\n\nbar", "foobar").is_empty());
    assert!(texts("- foo\n- bar", "foobar").is_empty());
    assert_eq!(texts("foo\n\nbar", "bar"), ["bar"]);
}

#[test]
fn matches_do_not_overlap_and_come_in_reading_order() {
    assert_eq!(texts("aaaaa", "aa"), ["aa", "aa"]);
    assert_eq!(texts("abababab", "abab"), ["abab", "abab"]);
    let doc = Doc::parse("x foo\n\nfoo foo\n\n> foo");
    let f = search(&doc, "foo");
    let blocks: Vec<u32> = f.matches.iter().map(|m| m.block).collect();
    assert_eq!(blocks, [0, 1, 1, 2]);
    assert!(
        f.matches
            .windows(2)
            .all(|w| (w[0].block, w[0].start) < (w[1].block, w[1].start))
    );
    assert_eq!((f.total, f.first, f.at), (4, 0, 0));
}

#[test]
fn an_empty_query_has_no_matches() {
    assert_eq!(search(&Doc::parse("abc"), ""), Found::default());
    assert_eq!(search(&Doc::parse(""), "abc"), Found::default());
    assert_eq!(search(&Doc::parse(""), ""), Found::default());
}

#[test]
fn a_query_is_cut_to_its_limit() {
    let src = "a".repeat(MAX_QUERY + 40);
    let doc = Doc::parse(&src);
    let f = search(&doc, &src);
    assert_eq!(f.matches.len(), 1);
    assert_eq!(f.matches[0].end - f.matches[0].start, MAX_QUERY as u32);
}

#[test]
fn markup_urls_and_front_matter_are_not_text() {
    assert!(texts("[click](https://example.com/page)", "example").is_empty());
    assert_eq!(
        texts("[click](https://example.com/page)", "click"),
        ["click"]
    );
    assert!(texts("**bold** and `code`", "**").is_empty());
    assert!(texts("**bold** and `code`", "`").is_empty());
    assert!(texts("![alt](image.png)", "image.png").is_empty());
    assert!(texts("# Title\n\n## Sub", "#").is_empty());
    assert!(texts("---\ntitle: secret\n---\n\nbody", "secret").is_empty());
    assert_eq!(texts("---\ntitle: secret\n---\n\nbody", "body"), ["body"]);
    assert!(texts("<!-- hidden -->\n\nshown", "hidden").is_empty());
}

#[test]
fn the_text_of_images_and_footnotes_is_what_the_document_holds() {
    let doc = Doc::parse("![A cat](cat.png) and a note[^1].\n\n[^1]: The note.");
    assert_eq!(found(&doc, "a cat"), ["A cat"]);
    assert_eq!(found(&doc, "the note"), ["The note"]);
}

#[test]
fn table_cells_are_texts_of_their_own() {
    let src = "| h1 | h2 |\n|---|---|\n| foo | bar |\n| foobar | x |\n";
    let doc = Doc::parse(src);
    // `foobar` in one cell matches; `foo` next to `bar` across two cells does not.
    assert_eq!(found(&doc, "foobar"), ["foobar"]);
    assert_eq!(found(&doc, "foo").len(), 2);
    assert_eq!(found(&doc, "obar"), ["obar"]);
    assert!(found(&doc, "oba ").is_empty());
    assert!(found(&doc, "h1h2").is_empty());
    assert!(found(&doc, "ox").is_empty());
    assert_eq!(found(&doc, "H2"), ["h2"]);
}

#[test]
fn the_chunks_of_a_split_block_are_one_text() {
    // An unbroken run is cut at the size limit wherever that falls: the word is put at every
    // offset around it, and found each time.
    let mut straddled = 0;
    for at in MAX_BLOCK_BYTES - 12..MAX_BLOCK_BYTES + 12 {
        let src = format!("{}needle{}", "x".repeat(at), "x".repeat(200));
        let doc = Doc::parse(&src);
        assert!(doc.blocks.len() > 1, "{at}");
        assert!(doc.blocks[1].joined);
        let f = search(&doc, "NEEDLE");
        assert_eq!(f.matches.len(), 1, "{at}");
        let m = f.matches[0];
        assert_eq!(
            &doc.text[m.start as usize..m.end as usize],
            "needle",
            "{at}"
        );
        let second = doc.block_spans(&doc.blocks[1])[0].start;
        if m.start < second && second < m.end {
            straddled += 1;
            assert_eq!(m.block, 0, "the match is in the block it starts in");
        }
    }
    assert!(straddled > 0, "some offset puts the word across the cut");
    // The next block is another text, however long the one before it was.
    let src = format!("{}ab\n\ncd", "x".repeat(MAX_BLOCK_BYTES + 100));
    assert_eq!(texts(&src, "xab"), ["xab"]);
    assert!(texts(&src, "abcd").is_empty());
    assert!(texts(&src, "bc").is_empty());
}

#[test]
fn every_match_is_counted_and_a_window_of_them_is_kept_around_the_focus() {
    let n = 3 * WINDOW;
    let doc = Doc::parse(&"a ".repeat(n));
    // The match number `i` starts at byte `2 * i`.
    let start = |m: &Match| m.start as usize / 2;
    let at = |focus| {
        let f = search_from(&doc, "a", focus);
        assert_eq!(f.total, n, "the count is of them all");
        assert!(f.matches.len() <= WINDOW);
        assert!(
            f.matches
                .windows(2)
                .all(|w| start(&w[1]) == start(&w[0]) + 1)
        );
        assert_eq!(start(&f.matches[0]), f.first);
        assert!((f.first..f.first + f.matches.len()).contains(&f.at));
        assert_eq!(start(&f.matches[f.at - f.first]), f.at);
        f
    };
    let f = at(Focus::Index(0));
    assert_eq!((f.first, f.at, f.matches.len()), (0, 0, WINDOW));
    assert_eq!(search(&doc, "a"), f, "search is the one from the first");
    let f = at(Focus::Index(20_000));
    assert_eq!((f.first, f.at, f.matches.len()), (15_000, 20_000, WINDOW));
    let f = at(Focus::Index(n - 1));
    assert_eq!((f.first, f.at), (n - WINDOW, n - 1));
    assert_eq!(f.matches.len(), WINDOW);
    // A place in the text is the first match from it, whatever lies before.
    let m = at(Focus::Index(12_345)).matches[WINDOW / 2];
    let f = at(Focus::At(m.block, m.start));
    assert_eq!(f.at, 12_345);
    let f = at(Focus::At(m.block, m.start + 1));
    assert_eq!(f.at, 12_346);
    // With none from there on it goes around to the first.
    let f = at(Focus::At(u32::MAX, 0));
    assert_eq!((f.first, f.at), (0, 0));
    let f = at(Focus::Index(n));
    assert_eq!((f.first, f.at), (0, 0));
}

#[test]
fn a_few_matches_are_all_kept_whatever_the_focus() {
    let doc = Doc::parse("a b a b a");
    for focus in [Focus::Index(0), Focus::Index(2), Focus::At(0, 4)] {
        let f = search_from(&doc, "a", focus);
        assert_eq!((f.matches.len(), f.first, f.total), (3, 0, 3));
    }
    let f = search_from(&doc, "a", Focus::At(0, 3));
    assert_eq!(f.at, 1);
    let f = search_from(&doc, "a", Focus::At(0, 9));
    assert_eq!(f.at, 0, "after the last: around");
    let f = search_from(&doc, "zzz", Focus::At(0, 5));
    assert_eq!(f, Found::default());
}

#[test]
fn a_side_with_fewer_matches_than_its_half_leaves_the_room_to_the_other() {
    // Every match is kept while there are no more than the window holds, whatever the focus.
    let n = WINDOW - 1_000;
    let doc = Doc::parse(&"a ".repeat(n));
    for focus in [Focus::Index(0), Focus::Index(n / 2), Focus::Index(n - 1)] {
        let f = search_from(&doc, "a", focus);
        assert_eq!((f.matches.len(), f.first, f.total), (n, 0, n), "{focus:?}");
    }
    let doc = Doc::parse(&"a ".repeat(WINDOW));
    let f = search_from(&doc, "a", Focus::Index(WINDOW - 1));
    assert_eq!((f.matches.len(), f.first, f.total), (WINDOW, 0, WINDOW));
    // More than that: a short side keeps all it has and the other fills up the window.
    let n = WINDOW + 3_000;
    let doc = Doc::parse(&"a ".repeat(n));
    let f = search_from(&doc, "a", Focus::Index(100));
    assert_eq!((f.first, f.at, f.matches.len()), (0, 100, WINDOW));
    let f = search_from(&doc, "a", Focus::Index(n - 100));
    assert_eq!(
        (f.first, f.at, f.matches.len()),
        (n - WINDOW, n - 100, WINDOW)
    );
    let f = search_from(&doc, "a", Focus::Index(WINDOW / 2));
    assert_eq!((f.first, f.matches.len()), (0, WINDOW));
    // The window from the top of a document of `WINDOW + 1` is all but the last.
    let doc = Doc::parse(&"a ".repeat(6_000));
    let f = search(&doc, "a");
    assert_eq!((f.total, f.matches.len()), (6_000, 6_000));
}

/// A table with one row of two cells, `left` and `right`, which is split into fragments.
fn split_row(left: &str, right: &str) -> Doc {
    let doc = Doc::parse(&format!("| a | b |\n|---|---|\n| {left} | {right} |\n"));
    let row = doc.blocks.iter().position(|b| b.joined).expect("split") - 1;
    assert!(doc.blocks[row + 1..].iter().all(|b| b.joined));
    doc
}

#[test]
fn a_word_cut_by_the_split_of_a_cell_is_found_but_cells_stay_apart() {
    let long = "y ".repeat(MAX_BLOCK_BYTES);
    // The word is put at every offset around the cut of the first cell, and found each time.
    let mut straddled = 0;
    for at in 0..12 {
        let left = format!(
            "{}needle{}",
            "x".repeat(MAX_BLOCK_BYTES / 2 - 8 + at),
            "x".repeat(9_000)
        );
        let doc = split_row(&left, &long);
        let f = search(&doc, "needle");
        assert_eq!(f.matches.len(), 1, "{at}");
        let m = f.matches[0];
        assert_eq!(
            &doc.text[m.start as usize..m.end as usize],
            "needle",
            "{at}"
        );
        let later = &doc.blocks[m.block as usize + 1..];
        let cut = later.iter().take_while(|b| b.joined).any(|b| {
            doc.block_spans(b)
                .iter()
                .any(|s| m.start < s.start && s.start < m.end)
        });
        straddled += usize::from(cut);
    }
    assert!(straddled > 0, "some offset puts the word across the cut");
    // The cell on the right is the same.
    let right = format!(
        "{}needle{}",
        "x".repeat(MAX_BLOCK_BYTES / 2 - 3),
        "x".repeat(9_000)
    );
    assert_eq!(search(&split_row(&long, &right), "needle").matches.len(), 1);
    // A word across the two cells is no word, nor is one across the two rows' fragments.
    let doc = split_row(&format!("{long}x"), &format!("y{long}"));
    assert!(search(&doc, "xy").matches.is_empty());
    assert!(search(&doc, "x y").matches.is_empty());
}

#[test]
fn the_matches_of_a_split_row_come_in_the_order_of_its_fragments() {
    // The cells' texts follow each other in the text: the pieces of the left one, then of the
    // right one. The fragments take one piece of each in turn.
    let doc = split_row(
        &format!("{}needle", "x ".repeat(4_500)),
        &format!("{}{}", "needle ".repeat(4), "y ".repeat(4_500)),
    );
    let f = search(&doc, "needle");
    assert_eq!(f.total, 5);
    assert!(
        f.matches
            .windows(2)
            .all(|w| (w[0].block, w[0].start) < (w[1].block, w[1].start))
    );
    assert!(f.matches.iter().any(|m| m.block > f.matches[0].block));
    // Not in the order of the text: the right cell's first match starts before the left cell's
    // second piece's.
    assert!(f.matches.windows(2).any(|w| w[0].start > w[1].start));
    // A window from any of them is a run of these.
    for i in 0..5 {
        let w = search_from(&doc, "needle", Focus::Index(i));
        assert_eq!((w.matches.len(), w.at), (5, i));
    }
}

/// One line of `hits` as (offset, x0, x1): a block laid out by hand, no font needed.
fn boxed(hits: &[(u32, f32, f32)]) -> crate::layout::BlockBox {
    let hits: Vec<_> = hits
        .iter()
        .map(|&(doc, x0, x1)| crate::text::PHit {
            x0,
            x1,
            doc,
            style: 0,
        })
        .collect();
    let line = crate::text::PLine {
        top: 0.0,
        baseline: 10.0,
        bottom: 12.0,
        width: 0.0,
        cont: false,
        hits: 0..hits.len() as u32,
    };
    crate::layout::BlockBox {
        hits,
        lines: vec![line],
        ..Default::default()
    }
}

/// The (x0, x1, current) washes of `current` of the matches in `matches` on a block of one line.
fn washes(
    doc: &Doc,
    found: &Found,
    current: usize,
    content: &crate::layout::BlockBox,
) -> Vec<(f32, f32, bool)> {
    let marks = found.marks(Some(current));
    let block = marks.of_block(doc, 0).expect("marks");
    let mut out = Vec::new();
    block.rects(content, |x0, _, x1, _, current| out.push((x0, x1, current)));
    out
}

#[test]
fn matches_that_share_a_cluster_show_it_and_the_current_one_has_it() {
    // `office` with `ffi` one cluster (bytes 1..4): the `f`s and the `i` start inside it.
    let doc = Doc::parse("office");
    let content = boxed(&[
        (0, 0.0, 10.0),
        (1, 10.0, 40.0),
        (4, 40.0, 50.0),
        (5, 50.0, 60.0),
    ]);
    let f = search(&doc, "f");
    assert_eq!(f.matches.len(), 2);
    for current in 0..2 {
        assert_eq!(
            washes(&doc, &f, current, &content),
            [(10.0, 40.0, true)],
            "match {current}"
        );
    }
    // With none current it is washed, once.
    let marks = f.marks(None);
    let mut out = Vec::new();
    marks
        .of_block(&doc, 0)
        .unwrap()
        .rects(&content, |x0, _, x1, _, current| {
            out.push((x0, x1, current))
        });
    assert_eq!(out, [(10.0, 40.0, false)]);
    // A match of both letters has the cluster all the same.
    let f = search(&doc, "fi");
    assert_eq!(f.matches.len(), 1);
    assert_eq!(washes(&doc, &f, 0, &content), [(10.0, 40.0, true)]);
}

#[test]
fn a_match_that_starts_inside_a_cluster_is_shown_on_the_cluster_s_line() {
    // The mark that the match starts at is in the cluster `c\u{301}` at the end of a line; the
    // rest of the match is on the line below.
    let doc = Doc::parse("xc\u{301} yz");
    let m = search(&doc, "\u{301} y");
    assert_eq!(m.matches.len(), 1);
    // Clusters: `x` (0), `c` with its mark (1..4), `y` (5), `z` (6): the match starts at 2, inside
    // the second, and the space after it has none.
    let mut content = boxed(&[(0, 0.0, 5.0), (1, 5.0, 12.0), (5, 0.0, 5.0), (6, 5.0, 10.0)]);
    content.lines = vec![
        crate::text::PLine {
            top: 0.0,
            baseline: 10.0,
            bottom: 12.0,
            width: 0.0,
            cont: false,
            hits: 0..2,
        },
        crate::text::PLine {
            top: 12.0,
            baseline: 22.0,
            bottom: 24.0,
            width: 0.0,
            cont: true,
            hits: 2..4,
        },
    ];
    let marks = m.marks(Some(0));
    let mut tops = Vec::new();
    marks
        .of_block(&doc, 0)
        .unwrap()
        .rects(&content, |_, y0, _, _, _| tops.push(y0));
    assert_eq!(tops, [0.0, 12.0]);
}

#[test]
fn a_chunk_that_no_match_reaches_has_no_marks() {
    // Four chunks of one paragraph, a match in the first only, then one across the first cut.
    let src = format!("needle{}", "x".repeat(MAX_BLOCK_BYTES * 3));
    let doc = Doc::parse(&src);
    let last = doc.blocks.len() as u32 - 1;
    assert!(last >= 3);
    let f = search(&doc, "needle");
    let marks = f.marks(Some(0));
    assert!(marks.of_block(&doc, 0).is_some());
    for block in 1..=last {
        assert!(marks.of_block(&doc, block).is_none(), "{block}");
    }
    let second = doc.block_spans(&doc.blocks[1])[0].start as usize;
    let src = format!(
        "{}needle{}",
        "x".repeat(second - 3),
        "x".repeat(MAX_BLOCK_BYTES * 2)
    );
    let doc = Doc::parse(&src);
    let f = search(&doc, "needle");
    let marks = f.marks(Some(0));
    let reached: Vec<u32> = (0..doc.blocks.len() as u32)
        .filter(|&b| marks.of_block(&doc, b).is_some())
        .collect();
    assert_eq!(reached, [0, 1]);
}

#[test]
fn a_word_cut_over_several_fragments_is_found_in_order_in_every_cell() {
    // 600 cells that each give 32 bytes a fragment: a 100-character word is cut over four.
    let word = "needle ".repeat(14).trim_end().to_string();
    let cell = format!("{}{}{}", "q".repeat(50), word, "q".repeat(50));
    let cells = vec![cell.as_str(); 600];
    let doc = Doc::parse(&format!(
        "|{}\n|{}\n|{}\n",
        " h |".repeat(600),
        "---|".repeat(600),
        format_args!(" {} |", cells.join(" | "))
    ));
    assert!(doc.blocks.iter().filter(|b| b.joined).count() >= 4);
    let f = search(&doc, &word);
    assert_eq!(f.total, 600);
    assert!(
        f.matches
            .iter()
            .all(|m| { doc.text[m.start as usize..m.end as usize] == word })
    );
    assert!(
        f.matches
            .windows(2)
            .all(|w| (w[0].block, w[0].start) < (w[1].block, w[1].start))
    );
    // A word ends in a different fragment than it starts in: the matches are not all of the
    // fragments they start in.
    let spans = |m: &Match| {
        (0..doc.blocks.len())
            .filter(|&b| {
                doc.block_spans(&doc.blocks[b])
                    .iter()
                    .any(|s| m.start < s.end && s.start < m.end)
            })
            .count()
    };
    assert!(f.matches.iter().any(|m| spans(m) >= 3));
    for i in [0, 299, 599] {
        assert_eq!(search_from(&doc, &word, Focus::Index(i)).at, i);
    }
}

#[test]
fn whitespace_that_was_left_out_belongs_to_no_cluster() {
    // `ab cd` with the space at the break left out: `b` ends at the space.
    let doc = Doc::parse("ab cd");
    let content = boxed(&[(0, 0.0, 5.0), (1, 5.0, 10.0), (3, 0.0, 5.0), (4, 5.0, 10.0)]);
    let f = search(&doc, " c");
    assert_eq!(washes(&doc, &f, 0, &content), [(0.0, 5.0, true)]);
    // Nothing to wash for the space alone.
    let f = search(&doc, " ");
    assert_eq!(f.matches.len(), 1);
    assert!(washes(&doc, &f, 0, &content).is_empty());
    // A space that is a cluster of its own, with a combining mark, holds the mark.
    let doc = Doc::parse("a \u{301}b");
    let content = boxed(&[(0, 0.0, 5.0), (1, 5.0, 12.0), (4, 12.0, 17.0)]);
    let f = search(&doc, "\u{301}");
    assert_eq!(washes(&doc, &f, 0, &content), [(5.0, 12.0, true)]);
    // And a ligature holds its letters, up to the next cluster.
    let doc = Doc::parse("office x");
    let content = boxed(&[
        (0, 0.0, 10.0),
        (1, 10.0, 40.0),
        (4, 40.0, 50.0),
        (5, 50.0, 60.0),
    ]);
    let f = search(&doc, "fic");
    assert_eq!(washes(&doc, &f, 0, &content), [(10.0, 50.0, true)]);
}

#[test]
fn the_marks_of_a_late_chunk_are_found_without_looking_at_the_matches_before_it() {
    // Thousands of matches in the first chunks of a paragraph, a few in the last.
    let src = format!(
        "{}{}{}",
        "q ".repeat(6000),
        "x".repeat(MAX_BLOCK_BYTES * 2),
        " q q q"
    );
    let doc = Doc::parse(&src);
    let last = doc.blocks.len() as u32 - 1;
    assert!(last >= 3);
    let f = search(&doc, "q");
    assert_eq!(f.total, 6003);
    assert_eq!(f.reach, 1);
    let marks = f.marks(Some(f.matches.len() - 1));
    let block = marks.of_block(&doc, last).expect("marks");
    assert_eq!(block.matches.len(), 3);
    assert_eq!(block.current, Some(2));
    // The chunks between have none, whatever is before.
    assert!(marks.of_block(&doc, 2).is_none());
    // A match that runs on from the chunk before is a candidate, though it starts well before.
    let probe = Doc::parse(&"x".repeat(MAX_BLOCK_BYTES * 3));
    let second = probe.block_spans(&probe.blocks[1])[0].start as usize;
    let src = format!(
        "{}needle{}",
        "x".repeat(second - 3),
        "x".repeat(MAX_BLOCK_BYTES * 2)
    );
    let doc = Doc::parse(&src);
    let f = search(&doc, "needle");
    assert_eq!(f.reach, 6);
    let reached: Vec<u32> = (0..doc.blocks.len() as u32)
        .filter(|&b| f.marks(Some(0)).of_block(&doc, b).is_some())
        .collect();
    assert_eq!(reached, [0, 1]);
}
