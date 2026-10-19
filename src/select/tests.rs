use super::*;

fn fixture(name: &str) -> Option<String> {
    std::fs::read_to_string(format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).ok()
}

/// Offset of the first `needle` in the document's text.
fn at(doc: &Doc, needle: &str) -> u32 {
    doc.text
        .find(needle)
        .unwrap_or_else(|| panic!("no {needle:?}")) as u32
}

/// The word double-clicked at the first `target` in `src`, as text.
fn word<'a>(doc: &'a Doc, target: &str) -> &'a str {
    let (lo, hi) = word_unit(doc, Pos::new(block_with(doc, target), at(doc, target)));
    assert_eq!(lo.block, hi.block);
    &doc.text[lo.off as usize..hi.off as usize]
}

fn block_with(doc: &Doc, needle: &str) -> u32 {
    let off = at(doc, needle);
    let covers = |spans: &[Span]| spans.iter().any(|s| s.start <= off && off < s.end);
    doc.blocks
        .iter()
        .position(|b| covers(doc.block_spans(b)) || doc.row_cells(b).any(|(_, s)| covers(s)))
        .expect("a block holds it") as u32
}

#[test]
fn positions_sort_in_reading_order() {
    assert!(Pos::new(0, 99) < Pos::new(1, 0));
    assert!(Pos::start(3) < Pos::new(3, 1));
    assert!(Pos::new(3, 1_000_000) < Pos::end(3));
    assert!(Pos::end(3) < Pos::start(4));
    assert_eq!(Pos::new(2, 7), Pos { block: 2, off: 7 });
}

/// The ordering of positions rests on this: within a block, spans start where an earlier one
/// started or later (chunks, table cells left to right, the footnote back-link last).
fn check_span_order(doc: &Doc, what: &str) {
    for (i, b) in doc.blocks.iter().enumerate() {
        let spans: Vec<&Span> = match b.kind {
            BlockKind::TableRow { .. } => doc.row_cells(b).flat_map(|(_, s)| s).collect(),
            _ => doc.block_spans(b).iter().collect(),
        };
        assert!(
            spans.windows(2).all(|w| w[0].start <= w[1].start),
            "{what}: block {i} has spans out of order"
        );
    }
}

#[test]
fn span_starts_grow_within_every_block() {
    for name in [
        "spec.md",
        "readme.md",
        "tables.md",
        "golden.md",
        "inline.md",
        "images.md",
        "rtl.md",
        "cjk.md",
        "emoji.md",
        "huge.md",
    ] {
        // `huge.md` is generated, not committed.
        let Some(src) = fixture(name) else { continue };
        check_span_order(&Doc::parse(&src), name);
    }
    // Footnotes (definitions in the order of their numbers, back-links appended), split rows
    // and chunks of long blocks.
    let big = "word ".repeat(20_000);
    let src = format!(
        "a[^2] b[^1]\n\n[^1]: one\n\n[^2]: two\n\n    code\n\n{big}\n\n| a | b |\n|---|---|\n| {big} | {big} |\n| x | y |\n",
    );
    let doc = Doc::parse(&src);
    assert!(doc.blocks.iter().any(|b| b.joined));
    check_span_order(&doc, "synthetic");
    // A document big enough to be parsed in segments.
    let section = "# Title\n\n- item *one*\n- item two[^n]\n\n> quote\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```\ncode\tline\n```\n\n[^n]: note\n\n";
    let doc = Doc::parse(&section.repeat(5_000));
    assert!(doc.text.len() > 100_000);
    check_span_order(&doc, "segments");
}

#[test]
fn a_word_is_a_run_of_like_characters() {
    let doc = Doc::parse("hello world, don't stop. foo_bar abc123 x");
    assert_eq!(word(&doc, "hello"), "hello");
    assert_eq!(word(&doc, "llo"), "hello", "from the middle");
    assert_eq!(word(&doc, "world"), "world");
    assert_eq!(word(&doc, ", "), ",");
    assert_eq!(word(&doc, " world"), " ");
    assert_eq!(word(&doc, "don't"), "don't");
    assert_eq!(
        word(&doc, "'t"),
        "don't",
        "the apostrophe is part of the word"
    );
}

#[test]
fn an_apostrophe_joins_only_between_word_characters() {
    let doc = Doc::parse("don't 'quoted' it\u{2019}s ''x");
    assert_eq!(word(&doc, "don't"), "don't");
    assert_eq!(word(&doc, "quoted"), "quoted");
    assert_eq!(word(&doc, "it\u{2019}s"), "it\u{2019}s");
    assert_eq!(word(&doc, "''x"), "''", "a run of the same punctuation");
}

#[test]
fn underscores_digits_and_punctuation_runs() {
    let doc = Doc::parse("foo_bar abc123 x --- y ...! z    w");
    assert_eq!(word(&doc, "foo_bar"), "foo_bar");
    assert_eq!(word(&doc, "123"), "abc123");
    assert_eq!(word(&doc, "---"), "---");
    assert_eq!(word(&doc, "..."), "...");
    assert_eq!(word(&doc, "!"), "!");
    assert_eq!(word(&doc, "    w"), "    ", "a run of whitespace");
}

#[test]
fn combining_marks_and_ideographs() {
    let doc = Doc::parse("cafe\u{301} au 日本語のテキスト and 한국어 text");
    assert_eq!(word(&doc, "cafe"), "cafe\u{301}", "the accent stays");
    assert_eq!(word(&doc, "au"), "au");
    assert_eq!(
        word(&doc, "日"),
        "日本語のテキスト",
        "a run of ideographs and kana"
    );
    assert_eq!(word(&doc, "語"), "日本語のテキスト");
    assert_eq!(
        word(&doc, "한"),
        "한국어",
        "Hangul has spaces: a word as usual"
    );
}

#[test]
fn a_word_goes_across_a_style_boundary_but_not_a_cell_or_block() {
    let doc = Doc::parse("**fo**o bar *b*az");
    assert_eq!(word(&doc, "fo"), "foo");
    assert_eq!(word(&doc, "az"), "baz");
    let doc = Doc::parse("| ab | cd |\n|---|---|\n| ef | gh |\n\nword\n\nnext");
    assert_eq!(word(&doc, "ab"), "ab", "not across the cell");
    assert_eq!(word(&doc, "cd"), "cd");
    assert_eq!(word(&doc, "gh"), "gh");
    assert_eq!(word(&doc, "word"), "word", "not into the next paragraph");
    assert_eq!(word(&doc, "next"), "next");
    // The block's end counts as the end of the word before it.
    let (lo, hi) = word_unit(&doc, Pos::end(block_with(&doc, "word")));
    assert_eq!(&doc.text[lo.off as usize..hi.off as usize], "word");
}

#[test]
fn a_block_without_text_is_its_own_word() {
    let doc = Doc::parse("a\n\n---\n\nb");
    let rule = doc
        .blocks
        .iter()
        .position(|b| b.kind == BlockKind::Rule)
        .unwrap() as u32;
    assert_eq!(
        word_unit(&doc, Pos::new(rule, 0)),
        (Pos::start(rule), Pos::end(rule))
    );
}

#[test]
fn dragging_by_words_snaps_to_whole_words() {
    let doc = Doc::parse("alpha beta gamma delta");
    let b = 0;
    let start = Pos::new(b, at(&doc, "beta"));
    let mut sel = Selection::word(&doc, start);
    let text = |sel: &Selection| {
        let (lo, hi) = sel.range();
        doc.text[lo.off as usize..hi.off as usize].to_owned()
    };
    assert_eq!(text(&sel), "beta");
    // Into the middle of "delta": the whole word joins.
    sel.extend(&doc, Pos::new(b, at(&doc, "lta")));
    assert_eq!(text(&sel), "beta gamma delta");
    // Back into the first word: only it.
    sel.extend(&doc, Pos::new(b, at(&doc, "eta")));
    assert_eq!(text(&sel), "beta");
    // Backwards into the middle of "alpha".
    sel.extend(&doc, Pos::new(b, at(&doc, "pha")));
    assert_eq!(text(&sel), "alpha beta");
    // Between words: nothing more is taken.
    sel.extend(&doc, Pos::new(b, at(&doc, "gamma")));
    assert_eq!(text(&sel), "beta ");
}

#[test]
fn a_block_is_all_its_chunks() {
    let big = "word ".repeat(10_000);
    let doc = Doc::parse(&format!("first\n\n{big}\n\nlast"));
    let chunks: Vec<usize> = (0..doc.blocks.len())
        .filter(|&i| doc.blocks[i].joined)
        .collect();
    assert!(chunks.len() >= 2, "{} blocks", doc.blocks.len());
    let (first, last) = (chunks[0] as u32 - 1, *chunks.last().unwrap() as u32);
    for b in first..=last {
        assert_eq!(
            block_unit(&doc, b),
            (Pos::start(first), Pos::end(last)),
            "{b}"
        );
    }
    assert_eq!(block_unit(&doc, 0), (Pos::start(0), Pos::end(0)));
    let n = doc.blocks.len() as u32 - 1;
    assert_eq!(block_unit(&doc, n), (Pos::start(n), Pos::end(n)));
    let sel = Selection::block(&doc, Pos::new(last, 5));
    assert_eq!(sel.range(), (Pos::start(first), Pos::end(last)));
}

#[test]
fn a_split_table_row_is_one_block() {
    let big = "word ".repeat(10_000);
    let doc = Doc::parse(&format!(
        "| a | b |\n|---|---|\n| {big} | y |\n| next | row |\n"
    ));
    let joined: Vec<u32> = (0..doc.blocks.len() as u32)
        .filter(|&i| doc.blocks[i as usize].joined)
        .collect();
    assert!(!joined.is_empty());
    let first = joined[0] - 1;
    let last = *joined.last().unwrap();
    assert!(matches!(
        doc.blocks[first as usize].kind,
        BlockKind::TableRow { .. }
    ));
    assert_eq!(
        block_unit(&doc, joined[0]),
        (Pos::start(first), Pos::end(last))
    );
    // The row after it is its own.
    let next = last + 1;
    assert_eq!(block_unit(&doc, next), (Pos::start(next), Pos::end(next)));
}

#[test]
fn a_selection_is_the_span_between_its_ends() {
    let (a, b) = (Pos::new(1, 4), Pos::new(3, 9));
    let mut sel = Selection::caret(a);
    assert!(sel.is_empty());
    let doc = Doc::parse("x\n\ny\n\nz\n\nw");
    sel.extend(&doc, b);
    assert_eq!(sel.range(), (a, b));
    // Dragging back past the anchor swaps the ends.
    sel.extend(&doc, Pos::new(0, 2));
    assert_eq!(sel.range(), (Pos::new(0, 2), a));
    // Back to the anchor: nothing is selected.
    sel.extend(&doc, a);
    assert!(sel.is_empty());
}

#[test]
fn all_is_the_whole_document_in_constant_time() {
    let doc = Doc::parse("a\n\nb\n\nc");
    let sel = Selection::all(&doc).unwrap();
    assert_eq!(sel.range(), (Pos::start(0), Pos::end(2)));
    assert!(!sel.is_empty());
    assert!(Selection::all(&Doc::default()).is_none());
}

#[test]
fn a_block_shows_what_the_selection_has_of_it() {
    let range = (Pos::new(1, 10), Pos::new(3, 4));
    assert!(BlockSel::of(0, range).is_none());
    let first = BlockSel::of(1, range).unwrap();
    assert!(!first.contains(9) && first.contains(10) && first.contains(u32::MAX - 1));
    assert!(first.past);
    let mid = BlockSel::of(2, range).unwrap();
    assert!(mid.contains(0) && mid.contains(1 << 30) && mid.past);
    let last = BlockSel::of(3, range).unwrap();
    assert!(last.contains(3) && !last.contains(4) && !last.past);
    assert!(BlockSel::of(4, range).is_none());
    // Ending at a block's very start leaves nothing of it; the one before shows the break.
    let range = (Pos::new(1, 10), Pos::start(3));
    assert!(BlockSel::of(3, range).is_none());
    assert!(BlockSel::of(2, range).unwrap().past);
    // Inside one block.
    let one = BlockSel::of(2, (Pos::new(2, 3), Pos::new(2, 8))).unwrap();
    assert!(!one.past && one.contains(3) && !one.contains(8));
    assert!(BlockSel::of(2, (Pos::new(2, 3), Pos::new(2, 3))).is_none());
}

#[cfg(feature = "embed-fonts")]
mod with_fonts {
    use super::*;
    use crate::fonts::Fonts;
    use crate::layout::{Anchor, ImageSize, Layout, Params};
    use crate::text::PHit;

    fn layout_at(doc: &Doc, fonts: &Fonts, width: f32) -> Layout {
        let mut l = Layout::new(
            doc,
            Params {
                width,
                ..Params::default()
            },
        );
        l.ensure(doc, fonts, Anchor::TOP, 1.0e6);
        l
    }

    fn content(l: &Layout, b: usize) -> &crate::layout::BlockBox {
        l.geoms()[b].content.as_deref().expect("built")
    }

    /// Document-area x of a block-coordinate x.
    fn x_of(l: &Layout, x: f32) -> f32 {
        l.column().0 + x
    }

    /// Document-area y of the middle of line `k` of block `b`.
    fn mid_y(l: &Layout, b: usize, k: usize) -> f32 {
        let line = &content(l, b).lines[k];
        l.y(b) + (line.top + line.bottom) / 2.0
    }

    fn line_hits(l: &Layout, b: usize, k: usize) -> Vec<PHit> {
        let c = content(l, b);
        let r = &c.lines[k].hits;
        c.hits[r.start as usize..r.end as usize].to_vec()
    }

    /// The offset that follows `d` among the block's clusters.
    fn after(l: &Layout, b: usize, d: u32) -> u32 {
        content(l, b)
            .hits
            .iter()
            .map(|h| h.doc)
            .filter(|&o| o > d)
            .min()
            .unwrap_or(END)
    }

    /// A point in the first or last quarter of `h`, on line `k` of block `b`.
    fn quarter(l: &Layout, b: usize, k: usize, h: &PHit, left: bool) -> (f32, f32) {
        let (a, z) = (h.x0.min(h.x1), h.x0.max(h.x1));
        let x = if left {
            a + (z - a) * 0.25
        } else {
            a + (z - a) * 0.75
        };
        (x_of(l, x), mid_y(l, b, k))
    }

    #[test]
    fn a_point_picks_the_edge_of_the_cluster_half_it_is_in() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("hello world");
        let l = layout_at(&doc, &fonts, 900.0);
        let hits = line_hits(&l, 0, 0);
        assert_eq!(hits.len(), 11);
        for (i, h) in hits.iter().enumerate() {
            let (x, y) = quarter(&l, 0, 0, h, true);
            assert_eq!(l.locate(x, y), Pos::new(0, h.doc), "left of {i}");
            let (x, y) = quarter(&l, 0, 0, h, false);
            let next = hits.get(i + 1).map_or(END, |n| n.doc);
            assert_eq!(l.locate(x, y), Pos::new(0, next), "right of {i}");
        }
    }

    #[test]
    fn beside_a_line_the_nearest_end_of_it() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("hello world");
        let l = layout_at(&doc, &fonts, 900.0);
        let y = mid_y(&l, 0, 0);
        assert_eq!(l.locate(-50.0, y), Pos::new(0, 0), "left of the column");
        assert_eq!(l.locate(5000.0, y), Pos::end(0), "right of the line");
        assert_eq!(l.locate(x_of(&l, 300.0), y), Pos::end(0), "past the text");
    }

    #[test]
    fn lines_blocks_and_the_document_have_ends() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse(
            "# Head\n\nsome words that wrap onto several lines of text in a narrow window\n\nlast",
        );
        let l = layout_at(&doc, &fonts, 200.0);
        let n = doc.blocks.len();
        assert_eq!(n, 3);
        assert!(content(&l, 1).lines.len() > 2);
        let col = x_of(&l, 30.0);
        // Above everything, and below it.
        assert_eq!(l.locate(col, -100.0), Pos::start(0));
        assert_eq!(
            l.locate(col, l.total_height() + 500.0),
            Pos::end(n as u32 - 1)
        );
        // In the spacing above a block: that block's start.
        let gap_y = l.y(1) + 1.0;
        assert!(gap_y < l.y(1) + content(&l, 1).lines[0].top);
        assert_eq!(l.locate(col, gap_y), Pos::start(1));
        // Between two lines (their edges meet) and past the last one's bottom.
        for k in 0..content(&l, 1).lines.len() {
            let got = l.locate(col, mid_y(&l, 1, k));
            assert_eq!(got.block, 1);
            assert_ne!(got.off, END, "line {k}");
        }
        let c = content(&l, 1);
        let below = l.y(1) + c.lines.last().unwrap().bottom + 0.5;
        if below < l.y(2) {
            assert_eq!(l.locate(col, below), Pos::end(1));
        }
        // The end of a wrapped line is where the next one begins (the space hangs).
        let first = line_hits(&l, 1, 0);
        let second = line_hits(&l, 1, 1);
        let far = x_of(&l, 190.0);
        assert_eq!(
            l.locate(far, mid_y(&l, 1, 0)),
            Pos::new(1, second.iter().map(|h| h.doc).min().unwrap()),
            "{} hits on the line",
            first.len()
        );
    }

    #[test]
    fn right_to_left_clusters_lead_on_the_right() {
        let fonts = Fonts::new().unwrap();
        let src = fixture("rtl.md").unwrap();
        let doc = Doc::parse(&src);
        let l = layout_at(&doc, &fonts, 700.0);
        let (mut rtl, mut ltr) = (0, 0);
        for b in 0..doc.blocks.len() {
            for k in 0..content(&l, b).lines.len() {
                for h in &line_hits(&l, b, k) {
                    let next = after(&l, b, h.doc);
                    // Pointing at the leading half gives the cluster's own offset.
                    let (lead_left, trail_left) = if h.rtl() {
                        (false, true)
                    } else {
                        (true, false)
                    };
                    let (x, y) = quarter(&l, b, k, h, lead_left);
                    assert_eq!(
                        l.locate(x, y),
                        Pos::new(b as u32, h.doc),
                        "block {b} line {k}"
                    );
                    let (x, y) = quarter(&l, b, k, h, trail_left);
                    assert_eq!(
                        l.locate(x, y),
                        Pos::new(b as u32, next),
                        "block {b} line {k}"
                    );
                    if h.rtl() { rtl += 1 } else { ltr += 1 }
                }
            }
        }
        assert!(
            rtl > 100 && ltr > 50,
            "{rtl} right to left, {ltr} left to right"
        );
    }

    #[test]
    fn each_cell_of_a_table_row_is_reached() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("| left | mid | right |\n|---|---|---|\n| one | two | three |\n");
        let l = layout_at(&doc, &fonts, 900.0);
        let row = 1;
        let c = content(&l, row);
        assert_eq!(c.lines.len(), 3, "a line for each cell");
        for (k, word) in ["one", "two", "three"].into_iter().enumerate() {
            let hits = line_hits(&l, row, k);
            let (x, y) = quarter(&l, row, k, &hits[1], true);
            let got = l.locate(x, y);
            assert_eq!(
                got,
                Pos::new(row as u32, doc.text.find(word).unwrap() as u32 + 1),
                "{word}"
            );
        }
        // The last cell's end.
        let hits = line_hits(&l, row, 2);
        let y = mid_y(&l, row, 2);
        assert_eq!(
            l.locate(x_of(&l, hits.last().unwrap().x1 + 5.0), y),
            Pos::end(row as u32)
        );
        // The padding above and below the row.
        assert_eq!(
            l.locate(x_of(&l, 20.0), l.y(row) + 1.0),
            Pos::start(row as u32)
        );
    }

    #[test]
    fn an_image_box_is_one_cluster() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("see ![pic](a.png) here");
        let mut l = Layout::new(&doc, Params::default());
        l.set_image(&doc, 1, ImageSize::Pixels(60, 40));
        l.ensure(&doc, &fonts, Anchor::TOP, 1.0e6);
        let c = content(&l, 0);
        assert_eq!(c.boxes.len(), 1);
        let alt = doc.text.find("pic").unwrap() as u32;
        assert_eq!(c.boxes[0].doc, alt);
        let b = c.boxes[0];
        let y = l.y(0) + b.y + 10.0;
        assert_eq!(l.locate(x_of(&l, b.x + 10.0), y), Pos::new(0, alt));
        let end = doc.text.find(" here").unwrap() as u32;
        assert_eq!(
            l.locate(x_of(&l, b.x + b.b.width - 10.0), y),
            Pos::new(0, end)
        );
    }

    #[test]
    fn a_tab_in_code_is_one_step() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("```\na\tb\n```\n");
        let l = layout_at(&doc, &fonts, 900.0);
        let hits = line_hits(&l, 0, 0);
        let tab = doc.text.find('\t').unwrap() as u32;
        let b = tab + 1;
        // Four columns of 'a' and the tab: three spaces map to the tab.
        let synthetic: Vec<&PHit> = hits.iter().filter(|h| h.doc == tab).collect();
        assert_eq!(synthetic.len(), 3);
        for (i, h) in synthetic.iter().enumerate() {
            let (x, y) = quarter(&l, 0, 0, h, true);
            assert_eq!(l.locate(x, y), Pos::new(0, tab), "left of space {i}");
            let (x, y) = quarter(&l, 0, 0, h, false);
            assert_eq!(l.locate(x, y), Pos::new(0, b), "right of space {i}");
        }
    }

    /// The rectangles a selection of block `b` makes: `(x0, y0, x1, y1)`.
    fn rects(l: &Layout, b: usize, range: (Pos, Pos)) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        if let Some(sel) = BlockSel::of(b as u32, range) {
            sel.rects(content(l, b), |x0, y0, x1, y1| out.push((x0, y0, x1, y1)));
        }
        out
    }

    #[test]
    fn a_run_of_clusters_is_one_rectangle_per_line() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("hello world");
        let l = layout_at(&doc, &fonts, 900.0);
        let hits = line_hits(&l, 0, 0);
        let line = &content(&l, 0).lines[0];
        let from = Pos::new(0, hits[2].doc);
        let to = Pos::new(0, hits[8].doc);
        let r = rects(&l, 0, (from, to));
        // 'llo wor': from the left of 'l' to the right of 'r'.
        assert_eq!(r, [(hits[2].x0, line.top, hits[7].x1, line.bottom)]);
        // Nothing past the line's end: the text stops there.
        assert!(rects(&l, 0, (Pos::start(0), Pos::end(0))).len() == 1);
        // An empty selection has no rectangle.
        assert!(rects(&l, 0, (from, from)).is_empty());
    }

    #[test]
    fn a_wrapped_selection_runs_on_past_each_line_end() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("some words that wrap onto several lines of text in a narrow window");
        let l = layout_at(&doc, &fonts, 200.0);
        let lines = content(&l, 0).lines.len();
        assert!(lines >= 3);
        let all = (Pos::start(0), Pos::end(0));
        let r = rects(&l, 0, all);
        // A rectangle per line, and one space beyond each but the last.
        assert_eq!(r.len(), lines + (lines - 1));
        let marks: Vec<_> = r
            .iter()
            .filter(|r| (r.2 - r.0) < 8.0 && (r.2 - r.0) > 2.0)
            .collect();
        assert_eq!(marks.len(), lines - 1);
        for m in &marks {
            let size = content(&l, 0).styles[0].size;
            assert!((m.2 - m.0 - 0.3 * size).abs() < 1e-3, "{m:?}");
        }
        // Selected up to the end of the second line only: its end is marked, the third is not.
        let second = line_hits(&l, 0, 1);
        let third = line_hits(&l, 0, 2);
        let to = Pos::new(0, third[0].doc);
        let r = rects(&l, 0, (Pos::new(0, second[0].doc), to));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].2, second.last().unwrap().x1);
        assert!(r[1].0 == second.last().unwrap().x1);
        // A selection that ends inside the line has no mark.
        let to = Pos::new(0, second[3].doc);
        assert_eq!(rects(&l, 0, (Pos::new(0, second[0].doc), to)).len(), 1);
    }

    #[test]
    fn an_empty_line_inside_the_selection_is_marked() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("```\nab\n\ncd\n```\n");
        let l = layout_at(&doc, &fonts, 900.0);
        assert_eq!(content(&l, 0).lines.len(), 3);
        let left = line_hits(&l, 0, 0)[0].x0;
        let all = (Pos::start(0), Pos::end(0));
        let r = rects(&l, 0, all);
        // Two lines of text with a mark at the end of the first, the empty line, and the text of
        // the second.
        let blank = content(&l, 0).lines[1].top;
        assert!(r.iter().any(|r| r.1 == blank && r.0 == left), "{r:?}");
        // Selected from the middle of the last line only: the empty line isn't.
        let from = Pos::new(0, line_hits(&l, 0, 2)[1].doc);
        let r = rects(&l, 0, (from, Pos::end(0)));
        assert!(r.iter().all(|r| r.1 != blank), "{r:?}");
    }

    #[test]
    fn a_marker_is_selected_with_the_start_of_its_block() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("- [x] done\n- plain\n\n> [!NOTE]\n> text\n");
        let l = layout_at(&doc, &fonts, 900.0);
        assert_eq!(doc.blocks.len(), 4);
        for b in [0, 1, 2] {
            let c = content(&l, b);
            assert!(c.decor.is_some(), "block {b} has a marker or title");
            let d = c.decor.unwrap();
            let all = rects(&l, b, (Pos::start(b as u32), Pos::end(b as u32)));
            assert!(all.contains(&(d[0], d[1], d[2], d[3])), "block {b}");
            // Selected from after the first character: the marker is left out.
            let first = (!c.lines.is_empty())
                .then(|| line_hits(&l, b, 0).first().map(|h| h.doc))
                .flatten();
            if let Some(first) = first {
                let r = rects(&l, b, (Pos::new(b as u32, first + 1), Pos::end(b as u32)));
                assert!(!r.contains(&(d[0], d[1], d[2], d[3])), "block {b}");
            }
        }
        // An alert's title has no text of its own: it is in or out with its start.
        assert!(rects(&l, 2, (Pos::new(2, 0), Pos::end(2))).len() == 1);
    }

    #[test]
    fn selected_runs_follow_the_visual_order_of_a_bidi_line() {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse("abc שלום def");
        let l = layout_at(&doc, &fonts, 900.0);
        let hits = line_hits(&l, 0, 0);
        let hebrew = doc.text.find('ש').unwrap() as u32;
        let def = doc.text.find("def").unwrap() as u32;
        // The Hebrew word alone is one contiguous run, at the same place as its clusters.
        let r = rects(&l, 0, (Pos::new(0, hebrew), Pos::new(0, def - 1)));
        let word: Vec<&PHit> = hits
            .iter()
            .filter(|h| (hebrew..def - 1).contains(&h.doc))
            .collect();
        assert_eq!(word.len(), 4);
        let x0 = word.iter().map(|h| h.x0).fold(f32::INFINITY, f32::min);
        let x1 = word.iter().map(|h| h.x1).fold(0.0, f32::max);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].0, r[0].2), (x0, x1));
        // From inside the Hebrew word to the end: two runs in visual order (the rest of the
        // word is at the left of what was selected before it), and the end is marked once.
        let r = rects(&l, 0, (Pos::new(0, hebrew + 4), Pos::end(0)));
        assert!(r.len() >= 2, "{r:?}");
    }
}
