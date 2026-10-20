use std::time::{Duration, Instant};

use super::*;
use crate::doc::{Doc, MAX_BLOCK_BYTES};
use crate::fonts::Fonts;

fn view(src: &str, w: u32, h: u32) -> View {
    let mut v = View::new(Fonts::new().unwrap(), Doc::parse(src), None, (w, h), 1.0);
    v.ensure();
    v
}

fn settle(v: &mut View) {
    while !v.idle(Instant::now() + Duration::from_secs(1)) {}
    v.ensure();
}

/// `n` sections, each a heading `S0`, `S1`, ... and a paragraph.
fn sections(n: usize) -> String {
    (0..n)
        .map(|i| format!("## S{i}\n\nParagraph of section {i}.\n\n"))
        .collect()
}

/// A panel 260 wide and 300 tall below a bar 32 tall, at 1x.
fn geom() -> Geom {
    Geom {
        width: 260.0,
        y0: 32.0,
        y1: 332.0,
        scale: 1.0,
    }
}

/// Paint `v` in a window of its size.
fn paint(v: &mut View, now: Instant) -> Vec<u32> {
    let (w, h) = v.size();
    let mut buf = vec![0; (w * h) as usize];
    v.paint(&mut buf, w as usize, now);
    buf
}

#[test]
fn the_panel_pushes_the_document_only_when_there_is_room() {
    assert_eq!(width(1000, 1.0), 260);
    assert_eq!(width(200, 1.0), 200, "never wider than the window");
    assert_eq!(width(1000, 2.0), 520);
    // 260 for the panel and 480 for the document.
    assert!(pushes(740, 1.0));
    assert!(!pushes(739, 1.0));
    assert!(pushes(1480, 2.0));
    assert!(!pushes(1479, 2.0));
}

#[test]
fn entries_leave_out_the_headings_of_footnotes() {
    let src =
        "# One\n\nText[^n] and more.\n\n## Two\n\n[^n]:\n    # Note title\n\n    Note text.\n";
    let doc = Doc::parse(src);
    // The footnote's heading is in the document's list, after the others.
    assert_eq!(doc.headings.len(), 3);
    assert_eq!(doc.main_headings, 2);
    let list = List::of(&doc);
    let texts: Vec<String> = list
        .entries
        .iter()
        .map(|e| heading_text(&doc, e.block))
        .collect();
    assert_eq!(texts, ["One", "Two"]);
    assert_eq!(
        list.entries.iter().map(|e| e.heading).collect::<Vec<_>>(),
        [0, 1]
    );
    // Without footnotes, and without headings.
    assert_eq!(List::of(&Doc::parse("# A\n\n# B\n")).entries.len(), 2);
    assert!(List::of(&Doc::parse("plain\n")).entries.is_empty());
    assert!(List::of(&Doc::parse("")).entries.is_empty());
    // Only footnote headings: none of the main flow's.
    let only = Doc::parse("x[^n]\n\n[^n]:\n    # Inside\n");
    assert_eq!(only.headings.len(), 1);
    assert!(List::of(&only).entries.is_empty());
}

#[test]
fn rows_are_indented_by_level_below_the_shallowest() {
    let doc = Doc::parse("## Two\n\n#### Four\n\n### Three\n\n## Two again\n");
    let list = List::of(&doc);
    assert_eq!(list.shallowest, 2);
    let indents: Vec<f32> = (0..4).map(|i| list.indent(i)).collect();
    assert_eq!(indents, [0.0, 2.0 * INDENT, INDENT, 0.0]);
    // A document of h3s only is not indented at all.
    let list = List::of(&Doc::parse("### A\n\n### B\n"));
    assert_eq!((list.indent(0), list.indent(1)), (0.0, 0.0));
}

#[test]
fn heading_text_is_plain_and_on_one_line() {
    let doc = Doc::parse("# A *b* `c` [d](https://x.y) **e**f\n\nSetext\ntwo lines\n=====\n");
    let text = |i: usize| heading_text(&doc, doc.headings[i].block);
    assert_eq!(text(0), "A b c d ef");
    assert_eq!(text(1), "Setext two lines");
}

/// What `fit_line` makes of `text` for row `i` of the open panel, which must have laid out as many
/// glyphs.
fn row_shown(v: &mut View, i: usize, text: &str) -> String {
    let g = v.toc_geom();
    let s = g.scale as f32;
    let count = v.toc.rows[&(i as u32)].content.glyphs.len();
    let room = g.width as f32 - (PAD_X + v.toc.list.as_ref().unwrap().indent(i) + PAD_RIGHT) * s;
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
    let (para, pl) = v.fit_line(text, style, room, false);
    assert_eq!(count, pl.glyphs.len(), "row {i}");
    para.text
}

#[test]
fn a_heading_split_into_chunks_keeps_its_text() {
    let gap = " ".repeat(MAX_BLOCK_BYTES + 100);
    let src = format!("# A{gap}Z\n\nBody.\n\n## Next\n");
    let doc = Doc::parse(&src);
    let first = doc.headings[0].block as usize;
    assert!(doc.blocks[first + 1].joined, "the heading is cut in two");
    assert_eq!(heading_text(&doc, first as u32), "A Z");
    // The next heading is its own, and the list has one entry for the split one.
    let list = List::of(&doc);
    assert_eq!(list.entries.len(), 2);
    assert_eq!(heading_text(&doc, list.entries[1].block), "Next");
    assert!(!doc.blocks[list.entries[1].block as usize].joined);

    let mut v = view(&src, 900, 600);
    v.set_toc(true);
    paint(&mut v, Instant::now());
    let text = heading_text(&v.doc, v.toc.list.as_ref().unwrap().entries[0].block);
    assert_eq!(row_shown(&mut v, 0, &text), "A Z", "whole: no ellipsis");
}

#[test]
fn a_long_text_after_a_chunk_boundary_is_cut_with_an_ellipsis() {
    let tail = "word ".repeat(3 * MAX_BLOCK_BYTES / 5);
    let src = format!("# A{}{tail}\n\nBody.\n", " ".repeat(MAX_BLOCK_BYTES + 100));
    let doc = Doc::parse(&src);
    let block = doc.headings[0].block;
    assert!(
        doc.blocks[block as usize + 2].joined,
        "three chunks or more"
    );
    let text = heading_text(&doc, block);
    assert!(text.starts_with("A word word"), "{text}");
    // Bounded: nothing like the heading's megabytes, and no run of blanks.
    assert!(text.chars().count() <= HEADING_CHARS + 1, "{}", text.len());
    assert!(!text.contains("  ") && !text.ends_with(' '));

    let mut v = view(&src, 900, 600);
    v.set_toc(true);
    paint(&mut v, Instant::now());
    let shown = row_shown(&mut v, 0, &text);
    assert!(shown.ends_with('…'), "{shown}");
    assert!(shown.starts_with("A word"), "{shown}");
}

#[test]
fn the_current_heading_is_the_last_one_at_or_above_the_position() {
    let tops = [100.0, 200.0, 300.0];
    let at = |y: f64| current(3, |i| tops[i], y);
    assert_eq!(at(0.0), None, "before the first heading");
    assert_eq!(at(99.9), None);
    assert_eq!(at(100.0), Some(0), "a heading at the position counts");
    assert_eq!(at(199.0), Some(0));
    assert_eq!(at(200.0), Some(1));
    assert_eq!(at(250.0), Some(1));
    assert_eq!(at(300.0), Some(2));
    assert_eq!(at(1.0e12), Some(2));
    assert_eq!(current(0, |_| 0.0, 50.0), None, "no headings");
}

#[test]
fn rows_are_hit_inside_the_panel_only() {
    let g = geom();
    // The padding above the first row is no row; each row is 26 tall.
    assert_eq!(g.row_at(32.0, 0.0, 10), None);
    assert_eq!(g.row_at(37.9, 0.0, 10), None);
    assert_eq!(g.row_at(38.0, 0.0, 10), Some(0));
    assert_eq!(g.row_at(63.9, 0.0, 10), Some(0));
    assert_eq!(g.row_at(64.0, 0.0, 10), Some(1));
    // Above the panel (the toolbar), and below it.
    assert_eq!(g.row_at(31.0, 0.0, 10), None);
    assert_eq!(g.row_at(332.0, 0.0, 10), None);
    // Scrolled by 52: row 2 is the first row on screen, at the top.
    assert_eq!(g.row_at(39.0, 52.0, 10), Some(2));
    // Fewer rows than room: the space below the last is no row.
    assert_eq!(g.row_at(38.0 + 3.0 * 26.0 + 1.0, 0.0, 3), None);
    assert_eq!(g.row_at(38.0 + 2.0 * 26.0 + 1.0, 0.0, 3), Some(2));
    assert_eq!(g.row_at(100.0, 0.0, 0), None, "no rows");
    // Scaled up, rows and padding scale with it.
    let g2 = Geom {
        scale: 2.0,
        y0: 64.0,
        y1: 664.0,
        width: 520.0,
    };
    assert_eq!(g2.row_at(64.0 + 12.0 + 51.0, 0.0, 10), Some(0));
    assert_eq!(g2.row_at(64.0 + 12.0 + 52.0, 0.0, 10), Some(1));
    // A row's top is where the hit test finds it.
    for i in [0, 3, 9] {
        let top = g.top(i, 0.0);
        assert_eq!(g.row_at(top, 0.0, 10), Some(i));
        assert_eq!(
            g.row_at(top - 0.5, 0.0, 10),
            i.checked_sub(1).filter(|_| i > 0)
        );
    }
}

#[test]
fn the_scroll_stays_within_the_content() {
    let g = geom();
    // Ten rows are 12 + 260 = 272 tall: they fit in 300.
    assert_eq!(g.max_scroll(10), 0.0);
    assert_eq!(g.max_scroll(0), 0.0);
    // A hundred are 2612 tall.
    assert_eq!(g.max_scroll(100), 2612.0 - 300.0);
    // The visible rows are all of those on screen, partly or wholly, and no more.
    assert_eq!(g.visible(0.0, 100), 0..12);
    assert_eq!(g.visible(0.0, 5), 0..5);
    assert_eq!(g.visible(g.max_scroll(100), 100), 88..100);
    assert_eq!(
        g.visible(1.0e9, 100),
        100..100,
        "past the end shows nothing"
    );
    for scroll in [0.0, 13.0, 500.0, g.max_scroll(100)] {
        let seen = g.visible(scroll, 100);
        for i in 0..100 {
            let (top, bottom) = (g.top(i, scroll), g.top(i, scroll) + g.row());
            let on = bottom > g.y0 && top < g.y1;
            assert_eq!(seen.contains(&i), on, "row {i} at {scroll}");
        }
    }
}

#[test]
fn showing_a_row_moves_the_panel_no_further_than_needed() {
    let g = geom();
    let rows = 100;
    // Already in view: nothing moves.
    assert_eq!(g.show(0.0, 3, rows), 0.0);
    assert_eq!(g.show(100.0, 8, rows), 100.0);
    // Below the view: the row ends at the bottom edge.
    let s = g.show(0.0, 30, rows);
    assert_eq!(g.top(30, s) + g.row(), g.y1);
    // Above it: the row starts at the top edge.
    let s = g.show(1000.0, 20, rows);
    assert_eq!(g.top(20, s), g.y0);
    // The first row takes the padding above it along, the last the padding below.
    assert_eq!(g.show(500.0, 0, rows), 0.0);
    assert_eq!(g.show(0.0, 99, rows), g.max_scroll(rows));
    // A list that fits doesn't scroll at all.
    assert_eq!(g.show(0.0, 4, 10), 0.0);
}

#[test]
fn a_long_heading_is_cut_with_an_ellipsis_inside_the_row() {
    let long = "A very long heading that goes on and on and could not possibly fit in the panel";
    let src = format!(
        "# Short\n\n## {long}\n\n### 日本語のとても長い見出しがここにありますがパネルには入りません\n"
    );
    let mut v = view(&src, 900, 600);
    v.set_toc(true);
    paint(&mut v, Instant::now());
    let g = v.toc_geom();
    let s = g.scale as f32;
    for (i, whole) in [(0, true), (1, false), (2, false)] {
        let (right, count) = {
            let glyphs = &v.toc.rows[&(i as u32)].content.glyphs;
            (glyphs.iter().map(|g| g.x).fold(0.0, f32::max), glyphs.len())
        };
        let room =
            g.width as f32 - (PAD_X + v.toc.list.as_ref().unwrap().indent(i) + PAD_RIGHT) * s;
        assert!(right <= room, "row {i}: {right} > {room}");
        let full = heading_text(&v.doc, v.toc.list.as_ref().unwrap().entries[i].block);
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
        let (para, pl) = v.fit_line(&full, style, room, false);
        assert!(pl.width <= room);
        assert_eq!(para.text.ends_with('…'), !whole, "{}", para.text);
        assert!(full.starts_with(para.text.trim_end_matches('…')));
        assert_eq!(count, pl.glyphs.len(), "row {i}");
    }
    // Invisible characters in a heading are the heading's own: they aren't spelled out.
    let v2 = view("# a\u{200D}b\n", 900, 600);
    let mut v2 = v2;
    v2.set_toc(true);
    paint(&mut v2, Instant::now());
    let text = heading_text(&v2.doc, 0);
    let (para, _) = v2.fit_line(
        &text,
        TextStyle {
            bold: false,
            italic: false,
            mono: false,
            size: 13.0,
            role: Role::Text,
            deco: 0,
            shift: 0.0,
            link: 0,
        },
        1.0e4,
        false,
    );
    assert_eq!(para.text, "a\u{200D}b");
}

#[test]
fn the_panel_follows_the_current_heading_when_it_changes() {
    let now = Instant::now();
    let mut v = view(&sections(60), 1000, 400);
    v.set_toc(true);
    settle(&mut v);
    paint(&mut v, now);
    assert_eq!(v.toc.scroll, 0.0);
    assert_eq!(v.toc_current(), Some(0));
    let g = v.toc_geom();
    // Jump far down: the panel scrolls to show the new current row, no further than needed.
    assert!(v.jump_to_anchor("s40", now));
    paint(&mut v, now);
    assert_eq!(v.toc_current(), Some(40));
    assert_eq!(
        g.top(40, v.toc.scroll) + g.row(),
        g.y1,
        "row 40 at the bottom edge"
    );
    // Back up: the row is at the top edge.
    assert!(v.jump_to_anchor("s5", now));
    paint(&mut v, now);
    assert_eq!(v.toc_current(), Some(5));
    assert_eq!(g.top(5, v.toc.scroll), g.y0);
    // A heading just scrolled to (a little below the window's top) is the current one.
    v.set_scroll(v.layout.top(v.doc.headings[7].block as usize) - 6.0);
    paint(&mut v, now);
    assert_eq!(v.toc_current(), Some(7));
    // The reader scrolls the panel: it stays while the current heading doesn't change.
    v.toc.pointer = Some((50.0, 100.0));
    assert!(v.toc_wheel(300.0));
    let scrolled = v.toc.scroll;
    v.scroll_by(5.0, now);
    paint(&mut v, now);
    assert_eq!(v.toc_current(), Some(7));
    assert_eq!(v.toc.scroll, scrolled, "same heading: the panel is left");
    // The next heading comes: the panel brings it into view.
    v.jump_to_anchor("s8", now);
    paint(&mut v, now);
    let top = g.top(8, v.toc.scroll);
    assert!(top >= g.y0 && top + g.row() <= g.y1, "row 8 shows at {top}");
}

#[test]
fn before_the_first_heading_nothing_is_current() {
    let now = Instant::now();
    let src = format!("Preface text.\n\n{}", sections(30));
    let mut v = view(&src, 1000, 400);
    v.set_toc(true);
    paint(&mut v, now);
    assert_eq!(v.toc_current(), None);
    assert!(v.jump_to_anchor("s2", now));
    paint(&mut v, now);
    assert_eq!(v.toc_current(), Some(2));
    v.apply(crate::view::Action::Top, now);
    paint(&mut v, now);
    assert_eq!(v.toc_current(), None);
}

#[test]
fn the_panel_scroll_is_clamped_when_the_content_shrinks() {
    let now = Instant::now();
    let mut v = view(&sections(100), 1000, 400);
    v.set_toc(true);
    paint(&mut v, now);
    v.toc.pointer = Some((50.0, 100.0));
    v.toc_wheel(1.0e9);
    assert_eq!(v.toc.scroll, v.toc_geom().max_scroll(100));
    v.toc_wheel(-1.0e9);
    assert_eq!(v.toc.scroll, 0.0);
    v.toc_wheel(1.0e9);
    // A reload with a short document: the scroll comes back within it.
    v.replace(Doc::parse(&sections(3)), None);
    paint(&mut v, now);
    assert_eq!(v.toc.scroll, 0.0);
    // ... and one with the same length keeps it.
    v.replace(Doc::parse(&sections(100)), None);
    paint(&mut v, now);
    v.toc_wheel(700.0);
    let kept = v.toc.scroll;
    assert!(kept > 0.0);
    v.replace(Doc::parse(&sections(100)), None);
    paint(&mut v, now);
    assert_eq!(v.toc.scroll, kept);
}

/// The panel is painted in the window's left, over (not in) the document in the overlay case, and
/// the document's own picture is pushed right in the other.
#[test]
fn the_panel_is_painted_at_the_left_with_the_current_row_marked() {
    use crate::theme::LIGHT;
    let now = Instant::now();
    let mut v = view(&sections(5), 1000, 400);
    v.set_theme_mode(crate::view::ThemeMode::Light);
    let closed = paint(&mut v, now);
    assert!(closed.iter().all(|&p| p == LIGHT.bg || p != LIGHT.code_bg));
    v.set_toc(true);
    let buf = paint(&mut v, now);
    let at = |x: usize, y: usize| buf[y * 1000 + x];
    // The background is the toolbar's, a step off the page; the separator is the border's.
    assert_eq!(at(10, 200), LIGHT.code_bg);
    assert_eq!(at(259, 200), LIGHT.border);
    assert_eq!(at(261, 200), LIGHT.bg, "the document's page");
    // The current row (the first) has its mark in the link color at the panel's left edge.
    let marked = (0..400).any(|y| (4..7).any(|x| at(x, y) == LIGHT.link));
    assert!(marked, "the accent bar");
    // Text in the rows: some pixels differ from the background.
    assert!((38..64).any(|y| (12..60).any(|x| at(x, y) != LIGHT.code_bg)));
    // No headings: the one muted line.
    let mut empty = view("just text\n", 1000, 400);
    empty.set_theme_mode(crate::view::ThemeMode::Light);
    empty.set_toc(true);
    let buf = paint(&mut empty, now);
    assert!((0..60).any(|y| (12..90).any(|x| buf[y * 1000 + x] != LIGHT.code_bg)));
    assert!(empty.toc_rows() == 0 && !empty.over_toc());
}
