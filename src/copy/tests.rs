use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use super::*;
use crate::select::Selection;

fn doc(src: &str) -> Arc<Doc> {
    Arc::new(Doc::parse(src))
}

/// The caret before (`end` false) or after the first `needle` of the document's text.
fn pos(doc: &Doc, needle: &str, end: bool) -> Pos {
    let start = doc
        .text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in the text"));
    let off = start + if end { needle.len() } else { 0 };
    let at = if end { off - 1 } else { off } as u32;
    let holds = |spans: &[Span]| spans.iter().any(|s| s.start <= at && at < s.end);
    let block = doc
        .blocks
        .iter()
        .position(|b| holds(doc.block_spans(b)) || doc.row_cells(b).any(|(_, s)| holds(s)))
        .expect("a block holds it");
    Pos::new(block as u32, off as u32)
}

/// Everything the cursor makes for `range`, in pieces of `max` bytes.
fn text_in(doc: &Arc<Doc>, range: (Pos, Pos), max: usize) -> String {
    let mut cursor = TextCursor::new(doc.clone(), range.0, range.1);
    let mut out = String::new();
    while cursor.fill(&mut out, max) {}
    out
}

/// The whole document, as Ctrl+A and Ctrl+C make it.
fn all(src: &str) -> String {
    let d = doc(src);
    text_in(&d, Selection::all(&d).unwrap().range(), usize::MAX)
}

/// From the start of the first `from` to the end of the first `to`.
fn between(src: &str, from: &str, to: &str) -> String {
    let d = doc(src);
    text_in(&d, (pos(&d, from, false), pos(&d, to, true)), usize::MAX)
}

/// A source and the text copied from it, line by line, tabs shown as arrows, so the snapshot
/// shows every break, tab and space that ends a line.
fn shown(src: &str, text: &str) -> String {
    let mut out = format!("source: {src:?}\n");
    for line in text.split('\n') {
        out.push_str("│ ");
        out.push_str(&line.replace('\t', "→"));
        out.push('\n');
    }
    out
}

/// `all` of each source, shown.
fn all_of(sources: &[&str]) -> String {
    sources.iter().map(|src| shown(src, &all(src))).collect()
}

/// `between` for each (source, from, to), shown.
fn between_of(cases: &[(&str, &str, &str)]) -> String {
    cases
        .iter()
        .map(|(src, from, to)| shown(&format!("{src} [{from}..{to}]"), &between(src, from, to)))
        .collect()
}

#[test]
fn inline_styles_are_just_their_text() {
    insta::assert_snapshot!(all_of(&[
        "plain **bold** *italic* ***both*** ~~strike~~ `code` H<sub>2</sub>O x<sup>2</sup> <b>html</b>\n",
        "a [link text](https://example.com) and <https://auto.example>\n",
        "math $x^2$ and a [^1] mark\n\n[^1]: note\n",
    ]));
}

#[test]
fn images_are_their_alt_text() {
    // A box or a placeholder: the text is the same.
    insta::assert_snapshot!(all_of(&[
        "see ![the alt text](a.png) and ![](b.png) here\n",
        "[![linked **alt**](c.png)](https://example.com)\n",
    ]));
}

#[test]
fn line_breaks() {
    insta::assert_snapshot!(all_of(&[
        "soft\nbreak and hard\\\nbreak and <br> html break\n"
    ]));
}

#[test]
fn headings_paragraphs_and_rules() {
    insta::assert_snapshot!(all_of(&[
        "# Title\n\nFirst paragraph.\n\n---\n\n## Sub *heading*\n\nSecond.\n\n***\n\nlast"
    ]));
}

#[test]
fn code_is_verbatim() {
    insta::assert_snapshot!(all_of(&[
        "before\n\n```rust\nfn main() {\n\tlet a\t= 1;\n\n    // blank above\n}\n```\n\nafter\n",
        "    indented code\n    more\n\ntext\n",
    ]));
}

#[test]
fn lists() {
    insta::assert_snapshot!(all_of(&[
        "- one\n- two\n  - nested\n    - deeper\n  - nested two\n- three\n\ntext after\n",
        "3. third\n4. fourth\n   1. inner\n10. tenth\n",
        "- [x] done\n- [ ] open\n",
        // A loose list: the paragraphs of an item are a blank line apart, and so is the item
        // that follows an item of several blocks.
        "1. first item\n\n   its second paragraph\n\n2. second item\n3. third\n",
        "- item\n\n  ```\n  code\ttab\n  more\n  ```\n\n- next\n",
        "- a line\\\n  after a break\n- b\n",
    ]));
}

#[test]
fn lists_that_follow_each_other_stay_apart() {
    insta::assert_snapshot!(all_of(&[
        "1. one\n2. two\n\n- bullet\n- more\n",
        "- bullet\n\n1. one\n2. two\n",
        "- [ ] task\n- bullet\n",
        // A list inside an item doesn't end the list around it.
        "1. one\n   - nested\n2. two\n",
        // But one that follows it at the outer level is another list.
        "1. one\n   - nested\n\n- bullet\n",
    ]));
}

#[test]
fn tables_in_list_items() {
    insta::assert_snapshot!(all_of(&[
        "- | a | b |\n  |---|---|\n  | 1 | 2 |\n- next\n",
        "- item\n\n  | a |\n  |---|\n  | 1 |\n",
        "1. | a |\n   |---|\n   | 1 |\n\n   after\n",
        // Code first in an item is read the same way.
        "- ```\n  code\n  more\n  ```\n- next\n",
        "> - | a |\n>   |---|\n>   | 1 |\n",
        // An empty item, and one whose only content is a nested list, have no block to join.
        "- \n- next\n",
        "- - nested\n- next\n",
    ]));
}

#[test]
fn quotes() {
    insta::assert_snapshot!(all_of(&[
        "> one\n>\n> two\n\nafter\n\n> separate\n",
        "> outer\n>\n> > inner\n> > more\n>\n> back\n",
        "> - item one\n> - item two\n>   continued\\\n>   line\n",
        "- > quote in an item\n  > more\n- next\n",
        "> a hard\\\n> break\n",
    ]));
}

#[test]
fn alerts() {
    insta::assert_snapshot!(all_of(&[
        "> [!NOTE]\n> Useful information.\n\nplain\n\n> [!WARNING]\n> Careful.\n>\n> Second paragraph.\n",
        "> [!TIP]\n> one\n\n> [!IMPORTANT]\n> two\n\n> [!CAUTION]\n> three\n",
    ]));
}

#[test]
fn tables() {
    insta::assert_snapshot!(all_of(&[
        "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |\n| *d* | `e` | f<br>g |\n\nafter\n",
        // A sparse row keeps its columns.
        "| a | b | c |\n|---|---|---|\n| 1 |\n| | 2 | |\n| 3 | 4 | 5 |\n",
        // Tabs and breaks in a cell are spaces.
        "| a | b |\n|---|---|\n| x\ty | m<br>n |\n",
        "text\n\n| h |\n|---|\n| 1 |\n\n| h2 | h3 |\n|---|---|\n| 2 | 3 |\n",
    ]));
}

#[test]
fn a_split_row_is_one_line_cell_by_cell() {
    let big = "word ".repeat(10_000);
    let src =
        format!("| a | b | c |\n|---|---|---|\n| {big} | y | {big} |\n| last | row | here |\n");
    let d = doc(&src);
    assert!(d.blocks.iter().filter(|b| b.joined).count() >= 2);
    let text = all(&src);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "the header, the split row, the last row");
    assert_eq!(lines[0], "a\tb\tc");
    let cells: Vec<&str> = lines[1].split('\t').collect();
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0], big.trim_end());
    assert_eq!(cells[1], "y");
    assert_eq!(cells[2], big.trim_end());
    assert_eq!(lines[2], "last\trow\there");
}

#[test]
fn footnotes() {
    insta::assert_snapshot!(all_of(&[
        "Text[^a] more[^b].\n\n[^a]: First note.\n[^b]: Second note\n    over two lines\n\n    and a second paragraph.\n",
        "Ref[^x]\n\n[^x]:\n    ```\n    code\n    ```\n",
    ]));
}

#[test]
fn partial_selections() {
    let list = "- alpha beta\n- gamma delta\n- epsilon\n";
    insta::assert_snapshot!(between_of(&[
        // Starting inside an item: no bullet; the next items have theirs.
        (list, "beta", "gamma"),
        // Ending inside one.
        (list, "alpha", "gam"),
        // A whole item has its bullet.
        ("- alpha\n- beta\n- gamma\n", "beta", "beta"),
        // Within one paragraph and across blocks.
        (
            "The quick brown fox.\n\nJumps over.\n\nThe dog.\n",
            "brown",
            "Jumps"
        ),
        // Inside a quote: the lines after the first have the marks.
        ("> one two\\\n> three four\\\n> five six\n", "two", "five"),
        // Inside a heading and a code block.
        ("# A heading here\n\nmore\n", "heading", "more"),
        ("```\nline one\nline two\nline three\n```\n", "one", "two"),
    ]));
}

#[test]
fn partial_table_rows_and_cells() {
    let src = "| h1 | h2 | h3 | h4 |\n|---|---|---|---|\n| alpha | bravo | charlie | delta |\n| echo | foxtrot | golf | hotel |\n";
    insta::assert_snapshot!(between_of(&[
        // From the middle of cell 2 to the middle of cell 4 of one row.
        (src, "avo", "del"),
        // Down across rows: the first row from its middle, the second up to a cell.
        (src, "avo", "foxtrot"),
        // Inside a cell, and the whole of one.
        (src, "harl", "harl"),
        (src, "golf", "golf"),
        // The header and the first row.
        (src, "h2", "alpha"),
    ]));
}

#[test]
fn a_selection_in_a_sparse_table_takes_the_columns_it_reaches() {
    // The first and third cells of the rows are empty, and so is the last of the second.
    let src = "| a | b | c | d |\n|---|---|---|---|\n| | bravo | | delta |\n| x | | zulu | |\n";
    insta::assert_snapshot!(between_of(&[
        // A word in the second column is in it, not after an empty column 0.
        (src, "bravo", "bravo"),
        (src, "delta", "delta"),
        // Empty cells between the two selected ones keep their tabs.
        (src, "bravo", "delta"),
        // Down from the header: the first row is whole from its start.
        (src, "b", "bravo"),
        // From the middle of one row, through the end of it, to a cell of the next.
        (src, "bravo", "zulu"),
        // The last cell with text, with nothing selected of the empty one after it.
        (src, "zulu", "zulu"),
        (src, "x", "zulu"),
    ]));
    insta::assert_snapshot!(all_of(&[
        src,
        // Empty cells at the start, in the middle and at the end all count in a whole row.
        "| a | b | c | d |\n|---|---|---|---|\n| | | | |\n| | b | | |\n| a | | | |\n",
    ]));
}

#[test]
fn a_table_that_starts_the_document_keeps_its_empty_first_column_only_when_the_row_is_taken() {
    // The first text of the document is in the second column, at offset 0.
    let src = "| | beta | gamma |\n|---|---|---|\n| | bravo | delta |\n";
    let d = doc(src);
    assert_eq!(first_off(&d, &d.blocks[0]), 0);
    // A double click on the word: its cell only.
    let word = (Pos::new(0, 0), Pos::new(0, 4));
    assert_eq!(text_in(&d, word, usize::MAX), "beta");
    // Select all, and a triple click on the row: the row, from its first column.
    let all = Selection::all(&d).unwrap().range();
    assert_eq!(
        text_in(&d, all, usize::MAX),
        "\tbeta\tgamma\n\tbravo\tdelta"
    );
    let row = (Pos::start(0), Pos::end(0));
    assert_eq!(text_in(&d, row, usize::MAX), "\tbeta\tgamma");
    // From the start of the document to a later cell of the row.
    let to_gamma = (
        Pos::start(0),
        Pos::new(0, d.text.find("gamma").unwrap() as u32 + 5),
    );
    assert_eq!(text_in(&d, to_gamma, usize::MAX), "\tbeta\tgamma");
}

#[test]
fn a_selection_across_the_fragments_of_a_row_gives_the_cells_it_reaches() {
    let src = format!(
        "| left | right |\n|---|---|\n| {} | ZZZ |\n",
        "left ".repeat(5000)
    );
    let d = doc(&src);
    let z = d.text.find("ZZZ").unwrap() as u32;
    let first = d
        .blocks
        .iter()
        .position(|b| d.row_cells(b).any(|(_, s)| s.iter().any(|s| s.start == z)))
        .unwrap() as u32;
    assert!(d.blocks[first as usize + 1].joined, "the row is split");
    let next = d.row_cells(&d.blocks[first as usize + 1]).next().unwrap().1[0].start;
    // From ZZZ (the second column of the first fragment) to the fifth character of the second
    // fragment (the first column): both pieces, in the columns they belong to.
    let range = (Pos::new(first, z), Pos::new(first + 1, next + 5));
    assert_eq!(text_in(&d, range, usize::MAX), "left \tZZZ");
    // The same in pieces of any size.
    assert_eq!(text_in(&d, range, 1), "left \tZZZ");
    // Ending in the second fragment's column 0 with nothing of ZZZ before: the one column.
    let tail = (Pos::new(first, z + 3), Pos::new(first + 1, next + 5));
    assert_eq!(text_in(&d, tail, usize::MAX), "left ");
}

#[test]
fn a_selection_that_ends_where_a_block_starts_leaves_it_out() {
    let d = doc("first\n\nsecond\n\nthird\n");
    let from = Pos::start(0);
    // The caret at the start of the third block: nothing of it, and no separator.
    assert_eq!(
        text_in(&d, (from, Pos::start(2)), usize::MAX),
        "first\n\nsecond"
    );
    assert_eq!(text_in(&d, (from, Pos::start(1)), usize::MAX), "first");
    assert_eq!(
        text_in(&d, (Pos::end(0), Pos::end(2)), usize::MAX),
        "second\n\nthird"
    );
    assert_eq!(text_in(&d, (from, from), usize::MAX), "");
}

#[test]
fn the_text_is_the_same_in_pieces_of_any_size() {
    let seeds = [
        "# T\n\n- a\n  - b *c*\n- [x] d\n\n> q\n> > r\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```\nx\ty\n```\n\nend[^1]\n\n[^1]: n\n".to_owned(),
        std::fs::read_to_string(format!("{}/fixtures/spec.md", env!("CARGO_MANIFEST_DIR")))
            .unwrap(),
        std::fs::read_to_string(format!("{}/fixtures/tables.md", env!("CARGO_MANIFEST_DIR")))
            .unwrap(),
    ];
    for (i, src) in seeds.iter().enumerate() {
        let d = doc(src);
        let whole = Selection::all(&d).unwrap().range();
        let want = text_in(&d, whole, usize::MAX);
        assert!(!want.is_empty());
        for max in [1, 7, 64 * 1024, 1 << 20] {
            if max == 1 && want.len() > 300_000 {
                continue;
            }
            assert_eq!(text_in(&d, whole, max), want, "seed {i}, pieces of {max}");
        }
        // And a partial selection through the middle.
        let mid = (
            Pos::new(d.blocks.len() as u32 / 3, 0),
            Pos::new(d.blocks.len() as u32 * 2 / 3, 5),
        );
        let want = text_in(&d, mid, usize::MAX);
        for max in [1, 7, 64 * 1024] {
            assert_eq!(
                text_in(&d, mid, max),
                want,
                "seed {i} middle, pieces of {max}"
            );
        }
    }
}

/// Segmented documents (the 5 MB fixture is generated, not committed: a synthetic one stands in).
fn big_doc() -> Arc<Doc> {
    let path = format!("{}/fixtures/huge.md", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(path).unwrap_or_else(|_| {
        let section = "# Section\n\nSome *text* with `code` and a [link](#x).\n\n- item one\n- item two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n";
        section.repeat(5_000_000 / section.len() + 1)
    });
    doc(&src)
}

#[test]
fn chunks_of_a_big_selection_are_made_as_they_are_asked_for() {
    let d = big_doc();
    assert!(d.text.len() > 1_000_000);
    let range = Selection::all(&d).unwrap().range();
    let content = Content::of(&d, range);
    assert!(
        matches!(content, Content::Lazy { .. }),
        "too big to make at once"
    );
    assert!(content.size_hint() > 1_000_000);
    let mut total = 0;
    let mut pieces = 0;
    for piece in content.chunks() {
        assert!(
            !piece.is_empty() && piece.len() <= CHUNK + 4,
            "{}",
            piece.len()
        );
        total += piece.len();
        pieces += 1;
    }
    assert!(pieces > 10);
    assert!(total >= content.size_hint());
    // The pieces are the same text as one pass makes.
    let joined: String = content.chunks().collect();
    assert_eq!(joined, text_in(&d, range, usize::MAX));
    assert_eq!(joined.len(), total);
}

/// Counts the bytes the thread that allocates holds, and the most it held at once, so a test
/// can say what a piece of code needs, whatever the tests running beside it do.
struct Counting;

thread_local! {
    static HELD: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn held(bytes: isize) {
    // (A thread that is gone has no counter, and nothing to measure.)
    let _ = HELD.try_with(|h| {
        h.set(h.get() + bytes);
        let _ = PEAK.try_with(|p| p.set(p.get().max(h.get())));
    });
}

// SAFETY: every call goes straight to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        held(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        held(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// The most this thread holds beyond what it held before, while `run` goes.
fn peak_growth(run: impl FnOnce()) -> isize {
    let before = HELD.with(Cell::get);
    PEAK.with(|p| p.set(before));
    run();
    PEAK.with(Cell::get) - before
}

/// Streaming the text of the whole document keeps a piece of it, not all of it.
#[test]
fn streaming_a_big_selection_holds_one_piece_at_a_time() {
    let d = big_doc();
    let range = Selection::all(&d).unwrap().range();
    let content = Content::of(&d, range);
    let mut sunk = 0usize;
    let peak = peak_growth(|| {
        for piece in content.chunks() {
            sunk += piece.len();
        }
    });
    assert!(sunk > 3_000_000, "{sunk} bytes streamed");
    // A piece, the block being made and what the cursor keeps: under 200 KB for megabytes.
    assert!(
        peak < 200 * 1024,
        "{peak} bytes held at the most while streaming {sunk}"
    );
    // Making it all at once holds all of it, which is what streaming is for.
    let all = peak_growth(|| drop(text_in(&d, range, usize::MAX)));
    assert!(
        all as usize > sunk,
        "{all} bytes held making the whole text"
    );
}

#[test]
fn a_short_selection_is_made_at_once_and_doesnt_hold_the_document() {
    let d = doc("hello world\n\nsecond\n");
    let range = (Pos::start(0), Pos::end(1));
    let content = Content::of(&d, range);
    let Content::Text(text) = &content else {
        panic!("short text is made at once");
    };
    assert_eq!(&**text, "hello world\n\nsecond");
    assert_eq!(
        Arc::strong_count(&d),
        1,
        "no reference to the document is kept"
    );
    assert_eq!(content.size_hint(), text.len());
    assert_eq!(
        content.chunks().collect::<String>(),
        "hello world\n\nsecond"
    );
    assert!(
        Content::of(&d, (Pos::start(0), Pos::start(0)))
            .chunks()
            .next()
            .is_none()
    );
}

#[test]
fn a_lazy_copy_keeps_the_document_it_was_taken_from() {
    let d = big_doc();
    let content = Content::of(&d, Selection::all(&d).unwrap().range());
    let first: String = content.chunks().take(2).collect();
    drop(d);
    // The document is gone from the view, but not from the copy.
    assert_eq!(content.chunks().take(2).collect::<String>(), first);
}

#[test]
fn pieces_never_split_a_character() {
    let src = "日本語のテキストと🙂絵文字 ".repeat(20_000);
    let d = doc(&src);
    let content = Content::of(&d, Selection::all(&d).unwrap().range());
    assert!(matches!(content, Content::Lazy { .. }));
    let pieces: Vec<String> = content.chunks().collect();
    assert!(pieces.len() > 3);
    assert_eq!(pieces.concat(), src.trim_end());
    // The same for a short text, cut at the chunk size.
    let text: Arc<str> = "é".repeat(CHUNK).into();
    let pieces: Vec<String> = Content::Text(text.clone()).chunks().collect();
    assert_eq!(pieces.concat(), *text);
    assert!(pieces.iter().all(|p| p.len() <= CHUNK && !p.is_empty()));
}

/// The first and last chunk of each code block of the document, as the copy button names them.
fn code_blocks(d: &Doc) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for (i, b) in d.blocks.iter().enumerate() {
        if b.kind != BlockKind::Code {
            continue;
        }
        match runs.last_mut() {
            Some(run) if b.joined => run.1 = i as u32,
            _ => runs.push((i as u32, i as u32)),
        }
    }
    runs
}

/// What the button of each code block of `src` copies, in the pieces the clipboard asks for.
fn buttons(src: &str) -> Vec<String> {
    let d = doc(src);
    code_blocks(&d)
        .into_iter()
        .map(|(first, last)| Content::code(&d, first, last).chunks().collect())
        .collect()
}

fn button(src: &str) -> String {
    let all = buttons(src);
    assert_eq!(all.len(), 1, "one code block in {src:?}");
    all.into_iter().next().unwrap()
}

#[test]
fn the_button_copies_the_code_as_written() {
    // No language tag, no prompt stripped, and no line break after the last line.
    assert_eq!(button("```\nplain\n```\n"), "plain");
    assert_eq!(
        button("```console\n$ ls -l\ntotal 0\n```"),
        "$ ls -l\ntotal 0"
    );
    assert_eq!(button("~~~sh title=\"x\"\n# comment\n~~~"), "# comment");
    // Tabs, indentation, runs of spaces and blank lines stay as they are.
    assert_eq!(
        button("```rust\nfn f() {\n\treturn 1;  // tab\n\n    deep   spaced\n\n\n}\n```"),
        "fn f() {\n\treturn 1;  // tab\n\n    deep   spaced\n\n\n}"
    );
    assert_eq!(button("```\nλ 日本 🙂\n```"), "λ 日本 🙂");
    // Markup in code is text.
    assert_eq!(
        button("```\n**not bold** [x](y) <b>\n```"),
        "**not bold** [x](y) <b>"
    );
}

#[test]
fn the_button_drops_the_line_break_that_ends_the_code_and_no_more() {
    // The last line's own break isn't code; a blank last line is, and is kept.
    assert_eq!(button("```\na\n```"), "a");
    assert_eq!(button("```\na\n\n```"), "a\n");
    assert_eq!(button("```\na\n\n\n```"), "a\n\n");
    assert_eq!(button("```\n\na\n```"), "\na");
    // An unclosed fence ends with the document.
    assert_eq!(button("```\na\nb\n"), "a\nb");
}

#[test]
fn the_button_gives_an_indented_block_its_code_without_the_indent_that_makes_it_one() {
    assert_eq!(
        button("para\n\n    one\n    two\n\n    three\n\nafter\n"),
        "one\ntwo\n\nthree"
    );
    assert_eq!(button("para\n\n\ttabbed\n\t\tdeeper\n"), "tabbed\n\tdeeper");
}

#[test]
fn the_button_copies_the_code_and_not_the_quote_or_list_around_it() {
    assert_eq!(button("> ```\n> q1\n>\n> q2\n> ```\n"), "q1\n\nq2");
    assert_eq!(button("> > ```\n> > deep\n> > ```\n"), "deep");
    assert_eq!(button("- item\n\n  ```\n  c1\n\n  c2\n  ```\n"), "c1\n\nc2");
    assert_eq!(
        button("1. one\n   - two\n\n     ```\n     inner\n       indented\n     ```\n"),
        "inner\n  indented"
    );
    assert_eq!(button("> - a\n>\n>   ```\n>   in\n>   ```\n"), "in");
    assert_eq!(
        button("> [!NOTE]\n> ```\n> note code\n> ```\n"),
        "note code"
    );
    // (A list item that starts with code puts it on the marker's line.)
    assert_eq!(button("- ```\n  first\n  ```\n"), "first");
}

#[test]
fn each_code_block_has_its_own_copy() {
    assert_eq!(
        buttons("```\nA\n```\n\ntext\n\n```\nB1\nB2\n```\n\n    C\n"),
        ["A", "B1\nB2", "C"]
    );
    // Two blocks that touch are still two.
    assert_eq!(buttons("```\nA\n```\n```\nB\n```\n"), ["A", "B"]);
}

#[test]
fn a_code_block_with_nothing_in_it_copies_nothing() {
    assert_eq!(button("```\n```\n"), "");
}

#[test]
fn the_button_leaves_a_selection_copy_as_it_was() {
    // Ctrl+C of the same block still says how a quote holds it.
    let d = doc("> ```\n> q1\n> ```\n");
    let (first, last) = code_blocks(&d)[0];
    let range = (Pos::start(first), Pos::end(last));
    let by_selection: String = Content::of(&d, range).chunks().collect();
    let by_button: String = Content::code(&d, first, last).chunks().collect();
    assert_eq!(by_selection, "> q1");
    assert_eq!(by_button, "q1");
}

/// `lines` lines of code, `line 0`, `line 1`, and so on, each with a tab and a gap now and then.
fn long_code(lines: usize) -> String {
    let mut code = String::new();
    for i in 0..lines {
        code.push_str(&format!("line {i}\twith some text to fill it"));
        if i % 7 == 6 {
            code.push('\n');
        }
        if i + 1 < lines {
            code.push('\n');
        }
    }
    code
}

#[test]
fn the_code_of_a_huge_block_is_made_as_it_is_pasted() {
    // A block of several MB: chunks of the document, one block to the button.
    let code = long_code(120_000);
    assert!(code.len() > 3_000_000);
    let d = doc(&format!("Before.\n\n```text\n{code}\n```\n\nAfter.\n"));
    let runs = code_blocks(&d);
    assert_eq!(runs.len(), 1);
    assert!(runs[0].1 > runs[0].0 + 100, "the block is many chunks");
    let content = Content::code(&d, runs[0].0, runs[0].1);
    assert!(
        matches!(content, Content::Lazy { code: true, .. }),
        "too big to make at once"
    );
    assert_eq!(content.size_hint(), code.len());
    let mut total = 0;
    let mut joined = String::new();
    for piece in content.chunks() {
        assert!(!piece.is_empty() && piece.len() <= CHUNK + 4);
        total += piece.len();
        joined.push_str(&piece);
    }
    assert_eq!(total, code.len());
    assert!(joined == code, "the pieces are the code, byte for byte");
}

#[test]
fn a_block_in_a_document_that_is_parsed_in_segments_copies_exactly() {
    // Over 1 MB, so the document is parsed in segments, and the code is among them.
    let filler = "A paragraph of plain words that fills the document up to a size.\n\n";
    let code = "if x {\n\ty()\n\n    z()\n}";
    let src = format!(
        "{}```go\n{code}\n```\n\n{}",
        filler.repeat(15_000),
        filler.repeat(15_000)
    );
    assert!(src.len() > 1_000_000);
    assert_eq!(buttons(&src), [code]);
    // And a block that is itself bigger than a segment, among them.
    let long = long_code(2_000);
    let src = format!(
        "{}```\n{long}\n```\n\n{}",
        filler.repeat(20_000),
        filler.repeat(20_000)
    );
    assert_eq!(buttons(&src), [long]);
}
