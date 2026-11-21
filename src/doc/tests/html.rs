//! `<details>` sections and HTML tables, as GitHub renders them (probed 2026-11-22).

use super::*;

/// A fuzz finding: a loose list in a cell (a paragraph in each item) reads as a tight one, as
/// outside cells, so a list cut between two segments (each then tight) comes out the same.
#[test]
fn loose_and_tight_lists_in_a_cell_are_alike() {
    let data = include_bytes!("../../../fuzz/seeds/doc/loose-list-in-a-cell.bin");
    let src = String::from_utf8_lossy(&data[1..]);
    assert_same("loose list in a cell", &src, 16 + usize::from(data[0]) * 8);
    let cell = |src: &str| texts(&parsed(src))[0].clone();
    assert_eq!(
        cell("<table><tr><td>\n\n- a\n\n- b\n\n</td></tr></table>\n"),
        cell("<table><tr><td>\n\n- a\n- b\n\n</td></tr></table>\n")
    );
}

/// A fuzz finding: an HTML table left open (a `<td` never closed) before a Markdown table, whose
/// cell has a `<td>`: that is the Markdown cell's text, not a cell of the HTML table's, and the
/// segments agree.
#[test]
fn an_html_cell_tag_in_a_markdown_row_is_text() {
    let data = include_bytes!("../../../fuzz/seeds/doc/html-cell-in-a-markdown-row.bin");
    let src = String::from_utf8_lossy(&data[1..]);
    assert_same(
        "html cell in a markdown row",
        &src,
        16 + usize::from(data[0]) * 8,
    );
    let doc = parsed("<table><tr><td x\n\n| <td> a | b |\n|---|---|\n| c | d |\n");
    assert_eq!(texts(&doc)[1..], [" a|b", "c|d"]);
}

/// The blocks' texts, a line each (a row's cells apart by `|`).
fn texts(doc: &Doc) -> Vec<String> {
    let text = |spans: &[Span]| spans.iter().map(|s| doc.span_text(s)).collect::<String>();
    (doc.blocks.iter())
        .map(|b| match b.kind {
            BlockKind::TableRow { .. } => {
                let cells: Vec<String> = doc.row_cells(b).map(|(_, s)| text(s)).collect();
                cells.join("|")
            }
            _ => text(doc.block_spans(b)),
        })
        .collect()
}

/// The sections as (summary, body, open).
fn sections(doc: &Doc) -> Vec<(Range<u32>, Range<u32>, bool)> {
    (doc.sections.iter())
        .map(|s| (s.summary.clone(), s.body.clone(), s.open))
        .collect()
}

fn parsed(src: &str) -> Doc {
    let doc = Doc::parse(src);
    check_invariants(&doc);
    doc
}

#[test]
fn a_section_is_its_summary_then_its_body() {
    insta::assert_snapshot!(parse(
        "<details>\n<summary>More &lt;info&gt; <code>x</code> <a href=\"#y\">y</a></summary>\n\n\
         Markdown *inside*.\n\n- an item\n\n</details>\n\nAfter.\n"
    ));
}

#[test]
fn open_is_open_whatever_its_value() {
    for tag in [
        "<details open>",
        "<details open=\"false\">",
        "<details OPEN class=x>",
    ] {
        let doc = parsed(&format!(
            "{tag}<summary>S</summary>\n\nBody\n\n</details>\n"
        ));
        assert_eq!(sections(&doc), [(0..1, 1..2, true)], "{tag}");
    }
    let doc = parsed("<details class=open><summary>S</summary>\n\nBody\n\n</details>\n");
    assert!(!doc.sections[0].open);
}

/// No summary: GitHub writes `<summary>Details</summary>`, real text (found, copied).
#[test]
fn a_section_without_a_summary_gets_details() {
    let doc = parsed("<details>\n\nJust a body.\n\n</details>\n");
    assert_eq!(texts(&doc), ["Details", "Just a body."]);
    assert_eq!(sections(&doc), [(0..1, 1..2, false)]);
    // Text before the summary goes in the body, after it.
    let doc =
        parsed("<details>\n\nFirst.\n\n<summary>The summary</summary>\n\nLast.\n\n</details>\n");
    assert_eq!(texts(&doc), ["The summary", "First.", "Last."]);
    assert_eq!(sections(&doc), [(0..1, 1..3, false)]);
    // An empty one, and an empty summary, still have a line for the triangle.
    let doc = parsed("<details><summary></summary></details>\n\nAfter\n");
    assert_eq!(texts(&doc), ["", "After"]);
    assert_eq!(sections(&doc), [(0..1, 1..1, false)]);
}

#[test]
fn a_second_summary_is_body_text() {
    let doc = parsed(
        "<details>\n<summary>One</summary>\n\nBody\n\n<summary>Two</summary>\n\n</details>\n",
    );
    assert_eq!(texts(&doc), ["One", "Body", "Two"]);
    assert_eq!(sections(&doc), [(0..1, 1..3, false)]);
}

/// A summary of blocks (a heading, as GitHub's CSS keeps it by the triangle), headings with
/// their anchors in the summary and the body, and the anchors' blocks after a late summary.
#[test]
fn headings_in_sections_have_anchors() {
    let doc =
        parsed("<details>\n<summary>\n\n## Title\n\n</summary>\n\n### Inside\n\n</details>\n");
    assert_eq!(texts(&doc), ["Title", "Inside"]);
    assert_eq!(sections(&doc), [(0..1, 1..2, false)]);
    assert_eq!(doc.anchors, [("title".into(), 0), ("inside".into(), 1)]);
    let doc =
        parsed("<details>\n\n## Before\n\n<summary>Late</summary>\n\n## After\n\n</details>\n");
    assert_eq!(texts(&doc), ["Late", "Before", "After"]);
    assert_eq!(doc.anchors, [("before".into(), 1), ("after".into(), 2)]);
    assert_eq!(
        doc.headings.iter().map(|h| h.block).collect::<Vec<_>>(),
        [1, 2]
    );
}

/// A section in a summary (`<summary><details>`) is part of the summary, shown open as text:
/// it can't take the summary's line and with it the click that opens the outer section (Sol
/// r1, finding 2: an empty inner section was the summary's only line).
#[test]
fn a_section_in_a_summary() {
    let doc = parsed(
        "<details><summary>\n\n<details><summary>Inner</summary>\n\nX\n\n</details>\n\n</summary>\n\n\
         Body\n\n</details>\n",
    );
    assert_eq!(texts(&doc), ["Inner", "X", "Body"]);
    assert_eq!(sections(&doc), [(0..2, 2..3, false)]);
    assert_eq!(doc.summary_of(0), Some(0));
    assert_eq!(doc.summary_of(1), Some(0));
    let doc = parsed(
        "<details><summary><details><summary>Inner</summary></details></summary>outer body\
         </details>\n",
    );
    assert_eq!(sections(&doc), [(0..1, 1..2, false)]);
    assert_eq!(texts(&doc)[1], "outer body");
    // A footnote defined in the summary is moved out of it: its section stays a section (Sol
    // r2), as when the definition is written after the outer section.
    let doc = parsed(
        "<details><summary>\n\nSummary[^f]\n\n[^f]: <details><summary>FN</summary>FN body\
         </details>\n\n</summary>Outer body</details>\n",
    );
    let found = sections(&doc);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().all(|s| !s.2), "{found:?}");
    // Also from inside a section shown as text in that summary (Sol r3), as when the definition
    // is written after the outer section.
    let inside = parsed(
        "<details><summary>\n\n<details><summary>\n\nInner[^f]\n\n[^f]: <details><summary>FN\
         </summary>FN body</details>\n\n</summary>IB</details>\n\n</summary>OB</details>\n",
    );
    let after = parsed(
        "<details><summary>\n\n<details><summary>\n\nInner[^f]\n\n</summary>IB</details>\n\n\
         </summary>OB</details>\n\n[^f]: <details><summary>FN</summary>FN body</details>\n",
    );
    assert_eq!(sections(&inside), sections(&after));
    assert_eq!(sections(&inside).len(), 2, "{:?}", sections(&inside));
    assert_eq!(texts(&inside), texts(&after));
}

/// The nesting limit counts the sections of the text and of footnote definitions apart, and a
/// footnote's section left open past it ends with the footnote (Sol r4).
#[test]
fn footnote_sections_at_the_nesting_limit() {
    let pre = "<details><summary>S</summary>\n\n".repeat(32);
    let post = "</details>\n\n".repeat(32);
    let def = "[^f]: <details><summary>FN</summary>FN body</details>\n\n";
    let inside = parsed(&format!("{pre}Ref[^f]\n\n{def}{post}"));
    let after = parsed(&format!("{pre}Ref[^f]\n\n{post}{def}"));
    assert_eq!(sections(&inside).len(), 33);
    assert_eq!(sections(&inside), sections(&after));
    // An unclosed one, past the limit, doesn't take the next footnote's section.
    let first = "[^f]: <details><summary>FN</summary>FN body\n\n";
    let second = "[^g]: <details><summary>GN</summary>GN body</details>\n\n";
    let doc = parsed(&format!(
        "{}Ref[^f][^g]\n\n{first}{post}{second}",
        "<details><summary>S</summary>\n\n".repeat(32)
    ));
    assert!(sections(&doc).len() >= 33, "{:?}", sections(&doc).len());
}

#[test]
fn sections_nest() {
    let doc = parsed(
        "<details open><summary>Outer</summary>\n\nA\n\n<details><summary>Inner</summary>\n\n\
         B\n\n</details>\n\nC\n\n</details>\n",
    );
    assert_eq!(texts(&doc), ["Outer", "A", "Inner", "B", "C"]);
    assert_eq!(sections(&doc), [(0..1, 1..5, true), (2..3, 3..4, false)]);
    assert_eq!(doc.sections_around(3), [0, 1]);
    assert_eq!(doc.sections_around(5), Vec::<usize>::new());
    assert_eq!(doc.summary_of(2), Some(1));
    assert_eq!(doc.summary_of(1), None);
    // A summary-less inner section is turned inside the outer one's body.
    let doc = parsed(
        "<details>\n\nA\n\n<details>\n\nB\n\n</details>\n\n<summary>Late</summary>\n\n</details>\n",
    );
    assert_eq!(texts(&doc), ["Late", "A", "Details", "B"]);
    assert_eq!(sections(&doc), [(0..1, 1..4, false), (2..3, 3..4, false)]);
}

/// The HTML parser closes an unclosed `<details>` at the end of the document, and an inline
/// `</details>` where it is, in the middle of a paragraph.
#[test]
fn where_sections_end() {
    let doc = parsed("<details><summary>S</summary>\n\nA\n\n# B\n\nC\n");
    assert_eq!(sections(&doc), [(0..1, 1..4, false)]);
    let doc = parsed("<details><summary>S</summary>\n\nin it </details> out of it\n\nAfter\n");
    let trimmed: Vec<_> = texts(&doc).iter().map(|t| t.trim().to_owned()).collect();
    assert_eq!(trimmed, ["S", "in it", "out of it", "After"]);
    assert_eq!(sections(&doc), [(0..1, 1..2, false)]);
    // A stray end tag is nothing.
    let doc = parsed("Text </details> more\n");
    assert_eq!(texts(&doc), ["Text  more"]);
    assert!(doc.sections.is_empty());
}

/// In a list item or a quote, the end of the item or quote ends the section; in a cell of a
/// table it is shown open, as text.
#[test]
fn sections_end_with_their_container() {
    let doc = parsed("- <details><summary>S</summary>\n\n  In the item.\n- Next item\n");
    assert_eq!(texts(&doc), ["S", "In the item.", "Next item"]);
    assert_eq!(sections(&doc), [(0..1, 1..2, false)]);
    assert_eq!(
        doc.blocks[0].marker,
        Marker::Bullet,
        "the item's bullet by the triangle"
    );
    let doc = parsed("> <details><summary>S</summary>\n>\n> Quoted.\n\nNot quoted.\n");
    assert_eq!(sections(&doc), [(0..1, 1..2, false)]);
    let doc =
        parsed("<table><tr><td><details><summary>S</summary>Body</details></td></tr></table>\n");
    assert!(doc.sections.is_empty());
    assert_eq!(texts(&doc), ["S\nBody"]);
    let doc = parsed("| a |\n|---|\n| <details><summary>S</summary>x</details> |\n");
    assert!(doc.sections.is_empty());
}

/// A section in a footnote moves with its definition.
#[test]
fn sections_in_footnotes() {
    let doc = parsed(
        "Text[^n].\n\n[^n]: <details><summary>Note summary</summary>\n\n    Note body.\n\n\
         <details><summary>Main</summary>\n\nMain body.\n\n</details>\n",
    );
    let s: Vec<_> = (doc.sections.iter())
        .map(|s| texts(&doc)[s.summary.start as usize].clone())
        .collect();
    assert_eq!(s, ["Main", "Note summary"]);
    let note = &doc.sections[1];
    assert!(texts(&doc)[note.body.start as usize].starts_with("Note body."));
}

/// Cut into segments inside a section's body, or with one open across the cut, the document
/// is the same as one parser's.
#[test]
fn sections_across_segments() {
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!(
            "<details{}>\n<summary>Section {i}</summary>\n\nParagraph {i} of the body.\n\n\
             - item\n- item\n\n",
            if i % 3 == 0 { " open" } else { "" }
        ));
        if i % 2 == 0 {
            src.push_str("</details>\n\n");
        }
        if i % 7 == 0 {
            src.push_str("Text </details> after an inline end.\n\n");
        }
    }
    for budget in [200, 400, 1000] {
        assert!(assert_same("sections", &src, budget) > 3);
    }
}

#[test]
fn an_html_table_is_a_table() {
    insta::assert_snapshot!(parse(
        "<table>\n<thead>\n<tr><th>Name</th><th align=\"right\">Score</th><td>Plain</td></tr>\n\
         </thead>\n<tbody>\n<tr><td valign=\"top\">a</td><th>b</th><td align=CENTER \
         valign=bottom>c</td></tr>\n</tbody>\n</table>\n"
    ));
}

/// What isn't in a cell: the caption and text before the first row are a paragraph before the
/// table (as GitHub shows them), text between rows one after it (the rows stay together).
#[test]
fn text_outside_cells() {
    let doc = parsed(
        "<table><caption>The caption</caption><tr><td>a</td></tr> stray <tr><td>b</td></tr>\
         </table>\n",
    );
    assert_eq!(texts(&doc), ["The caption", "a", "b", "stray"]);
    // Whitespace between the tags is nothing, and so is a table with no cell.
    let doc = parsed("<table>\n  <tr>\n    <td> a </td>\n  </tr>\n</table>\n\n<table></table>\n");
    assert_eq!(texts(&doc), ["a"]);
    assert_eq!(doc.tables.len(), 1);
    let doc = parsed("<table><tr></tr></table>\n");
    assert!(doc.blocks.is_empty() && doc.tables.is_empty());
}

/// The HTML parser's implicit ends: a `<td>` ends the open cell, a `<tr>` the open row, a
/// `<td>` with no row opens one.
#[test]
fn implicit_ends() {
    let doc = parsed("<table><td>a<td>b<tr><td>c</table>\n");
    assert_eq!(texts(&doc), ["a|b", "c"]);
    // Ragged rows have the whole grid.
    assert_eq!(doc.tables[0].aligns.len(), 2);
    // Outside a table, rows and cells are their text.
    let doc = parsed("<tr><td>a</td><td>b</td></tr>\n");
    assert!(doc.tables.is_empty());
    assert_eq!(texts(&doc), ["a b "]);
}

#[test]
fn markdown_in_cells() {
    insta::assert_snapshot!(parse(
        "<table><tr><td>\n\n**Bold** start.\n\nSecond paragraph.\n\n- one\n- two\n  1. nested\n\n\
         ```rust\nfn main() {}\n```\n\n### Heading\n\n</td><td><p>a</p><p>b<br>c</p></td></tr>\
         </table>\n"
    ));
}

/// A table in a cell is flattened into it: a line per row, its cells apart by ` · `.
#[test]
fn nested_tables_are_lines() {
    let doc = parsed(
        "<table><tr><td>out<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>\
         </td><td>x</td></tr></table>\n",
    );
    assert_eq!(texts(&doc), ["out\na · b\nc|x"]);
    let doc = parsed("<table><tr><td>\n\n| h | i |\n|---|---|\n| j | k |\n\n</td></tr></table>\n");
    assert_eq!(texts(&doc), ["h · i\nj · k"]);
}

#[test]
fn spans_and_widths() {
    insta::assert_snapshot!(parse(
        "<table align=\"center\"><tr><td colspan=2 width=\"120px;\">wide</td><td rowspan=\"3\">\
         tall</td></tr><tr><td width=40%>a</td><td>b</td></tr><tr><td colspan=\"0\">c</td>\
         <td>d</td></tr><tr><td>e</td><td>f</td><td>g</td></tr></table>\n"
    ));
}

/// `colspan` stops at 1,000 columns, and a cell past them goes on in the last one; `rowspan=0`
/// goes to the table's end. The columns spans add are one per 8 bytes of the document at most,
/// so tables take memory in proportion to their source.
#[test]
fn span_limits() {
    let room = "x".repeat(8_000);
    let doc = parsed(&format!(
        "<table><tr><td colspan=5000>a</td><td>b</td><td>c</td></tr></table>\n\n{room}\n"
    ));
    assert_eq!(doc.tables[0].aligns.len(), html_table::MAX_HTML_COLS);
    assert_eq!(texts(&doc)[0], "a b c");
    let src = "<table><td colspan=1000>x</table>\n\n".repeat(100);
    let doc = parsed(&src);
    let cols: usize = doc.tables.iter().map(|t| t.aligns.len()).sum();
    assert!(cols <= 100 + src.len() / 8, "{cols} columns");
    let doc = parsed(
        "<table><tr><td rowspan=0>a</td><td>b</td></tr><tr><td>c</td></tr><tr><td>d</td></tr>\
         </table>\n",
    );
    let r = doc.tables[0].rowspans[0];
    assert_eq!((r.col, r.first, r.end), (0, 0, 3));
    // The cells below it start in the next column.
    assert_eq!(
        doc.cells.iter().map(|c| c.col).collect::<Vec<_>>(),
        [0, 1, 1, 1]
    );
}

/// Huge spans over many rows cost the rows, not rows × columns.
#[test]
fn spans_are_linear() {
    let src = |rows: usize| {
        let mut s = String::from("<table><tr><td rowspan=65534 colspan=1000>x</td></tr>\n");
        for _ in 0..rows {
            s.push_str("<tr><td>a</td><td rowspan=2 colspan=999>b</td></tr>\n");
        }
        s
    };
    assert_linear("spans", 4_000, |n| {
        let doc = Doc::parse(&src(n));
        assert!(doc.tables[0].aligns.len() <= html_table::MAX_HTML_COLS);
    });
}

/// A table nobody closes ends with the container it opened in, or at the document's end: what
/// follows is in its open cell, as on GitHub.
#[test]
fn unclosed_tables() {
    let doc = parsed("- <table><tr><td>a\n- next\n");
    assert_eq!(texts(&doc), ["a", "next"]);
    let doc = parsed("<table><tr><td>\n\nall\n\n# of\n\nthe rest\n");
    assert_eq!(texts(&doc), ["all\n\nof\n\nthe rest"]);
    // A footnote definition can't be in a table: it ends it.
    let doc = parsed("Text[^1]\n\n<table><tr><td>a\n\n[^1]: note\n");
    assert!(texts(&doc).contains(&"a".to_owned()));
}

/// A heading in a cell anchors at its row.
#[test]
fn heading_in_a_cell_anchors_at_the_row() {
    let doc = parsed("Before\n\n<table><tr><td>\n\n## Cell heading\n\n</td></tr></table>\n");
    assert_eq!(doc.anchors, [("cell-heading".into(), 1)]);
}

/// HTML tables cut into segments (rows spanning a cut, Markdown in cells across one, a table
/// left open) come out as one parser makes them.
#[test]
fn html_tables_across_segments() {
    let mut src = String::new();
    for i in 0..150 {
        src.push_str(&format!(
            "<table>\n<tr><th>h{i}</th><td rowspan=2>r</td></tr>\n<tr><td>\n\nMarkdown {i}\n\n\
             - item\n\n</td></tr>\n"
        ));
        if i % 5 != 0 {
            src.push_str("</table>\n\n");
        }
        src.push_str("Between.\n\n");
    }
    for budget in [200, 400, 1000] {
        assert_same("html tables", &src, budget);
    }
}

/// A row of an HTML table longer than a block is split into fragments that keep its cells'
/// spans and alignment.
#[test]
fn long_html_rows_split() {
    let long = "word ".repeat(5_000);
    let doc = parsed(&format!(
        "<table><tr><td colspan=2 align=right>{long}</td><th>x</th></tr></table>\n"
    ));
    assert!(doc.blocks.len() > 1);
    for b in &doc.blocks {
        for c in doc.row_cell_list(b) {
            if c.col == 0 {
                assert_eq!((c.span, c.align), (2, Align::Right));
            } else {
                assert_eq!(c.flags, Cell::TH);
            }
        }
    }
}

/// Review r1, finding 1: nested sections without summaries cost a parse linear in the
/// document (they nest at most 32 deep; deeper tags are nothing, their text shown).
#[test]
fn deeply_nested_sections_parse_in_linear_time() {
    let src = |n: usize| "<details>\n".repeat(2_000) + &"# h\n\npara\n\n".repeat(n);
    assert_linear("nested sections", 5_000, |n| {
        let doc = Doc::parse(&src(n));
        assert_eq!(doc.sections.len(), 32);
    });
    // Review r2, finding 1: sections without summaries in a footnote, each with a heading.
    let note = |n: usize| {
        "ref[^a]\n\n[^a]: start\n\n".to_owned()
            + &"    <details>\n\n    # h\n\n    </details>\n\n".repeat(n)
    };
    assert_linear("sections in a footnote", 2_000, |n| {
        let doc = Doc::parse(&note(n));
        assert_eq!(doc.sections.len(), n);
    });
    // Past the deepest, the tags are nothing until the last kept section ends.
    let deep = "<details>\n\n".repeat(33) + "in\n\n" + &"</details>\n\n".repeat(33) + "out\n";
    let doc = parsed(&deep);
    assert_eq!(doc.sections.len(), 32);
    let out = doc.blocks.len() as u32 - 1;
    assert!(
        doc.sections.iter().all(|s| s.body.end <= out),
        "`out` is outside"
    );
}

/// Review r1, finding 2: a cell or table that ends inside a Markdown block in the cell (a list,
/// a quote, a Markdown table) leaves the rest of the document as it was.
#[test]
fn a_cell_ending_inside_a_markdown_block() {
    let doc = parsed("<table><tr><td>\n\n- x </td></tr></table>\n\n- a\n  - b\n");
    let lists: Vec<u8> = doc.blocks.iter().map(|b| b.list).collect();
    assert_eq!(lists[1..], [1, 2], "{lists:?}");
    let doc = parsed(
        "- item\n  <details><summary>S</summary>\n\n  <table><tr><td>\n\n  - x </td></tr></table>\
         \n\n  inside\n\n  </details>\n- second\n  - nested\n\nafter\n",
    );
    let t = texts(&doc);
    let at = |s: &str| t.iter().position(|x| x == s).unwrap();
    assert_eq!(doc.blocks[at("second")].list, 1);
    assert_eq!(doc.blocks[at("nested")].list, 2);
    assert!(doc.sections[0].body.contains(&(at("inside") as u32)));
    let doc = parsed("> <table><tr><td>\n>\n> > q </td></tr></table>\n>\n> still quoted\n");
    assert_eq!(doc.blocks.last().unwrap().quote, 1);
    let doc = parsed(
        "<table><tr><td>\n\n| a | b |\n|---|---|\n| 1 </td></tr></table> | 2 |\n| 3 | 4 |\n\n\
         after\n",
    );
    // (The end tags close the Markdown table in the cell, as in a browser; what follows stays
    // in the cell, whole, its cells sound.)
    assert!(texts(&doc).last().unwrap().ends_with("3 · 4\n\nafter"));
}

/// Review r2, nits 3 and 4: an ordered list a cell took goes on at the cell's number after the
/// table ends in it, and text after `</table>` on its line has no space in front.
#[test]
fn what_follows_a_table_ending_in_a_list() {
    let doc = parsed("<table><tr><td>\n\n1. one\n2. two </td></tr></table> tail\n3. three\n");
    let three = doc.blocks.last().unwrap();
    assert_eq!(three.marker, Marker::Ordered(3));
    assert!(
        texts(&doc).contains(&"tail".to_owned()),
        "{:?}",
        texts(&doc)
    );
}

/// Review r1, finding 3: `</details>` indented under the last item of a list in the section
/// ends the section, as on GitHub.
#[test]
fn an_indented_end_tag_ends_the_section() {
    let doc = parsed(
        "<details><summary>Steps</summary>\n\n1. Do this\n2. Do that\n   </details>\n\n\
         ## Next heading\n",
    );
    let t = texts(&doc);
    assert_eq!(t.last().unwrap(), "Next heading");
    assert_eq!(doc.sections[0].body.end as usize, t.len() - 1);
}

/// Review r1, finding 8: after a section in a summary, the rest of the summary is the outer
/// section's.
#[test]
fn summary_text_after_a_section_in_it() {
    let doc = parsed(
        "<details><summary>\n\nA\n\n<details><summary>I</summary>\n\nX\n\n</details>\n\nB\n\n\
         </summary>\n\nBody\n\n</details>\n",
    );
    let b = texts(&doc).iter().position(|t| t == "B").unwrap() as u32;
    assert_eq!(doc.summary_of(b), Some(0));
}
