//! Collapsible sections (hidden blocks) and HTML tables (spans, alignment).

use super::*;

/// `n` sections of `k` body paragraphs each, closed, between paragraphs.
fn sections(n: usize, k: usize) -> Doc {
    let mut src = String::new();
    for i in 0..n {
        src.push_str(&format!(
            "Before {i}\n\n<details><summary>Section {i}</summary>\n\n"
        ));
        for j in 0..k {
            src.push_str(&format!("Body {i}.{j}, a paragraph of some words.\n\n"));
        }
        src.push_str("</details>\n\n");
    }
    Doc::parse(&src)
}

fn laid_out(doc: &Doc, fonts: &Fonts) -> Layout {
    let mut l = complete(doc, fonts, params(900.0, 1.0));
    l.ensure(doc, fonts, Anchor::TOP, 100_000.0);
    l
}

/// A closed section's body has no height and is never laid out; its summary has the triangle.
#[test]
fn closed_bodies_are_hidden() {
    let fonts = Fonts::new().unwrap();
    let doc = sections(3, 4);
    let l = laid_out(&doc, &fonts);
    check_geometry(&l);
    for s in &doc.sections {
        for b in s.body.clone() {
            let g = &l.geoms()[b as usize];
            assert!(
                l.hidden(b as usize) && g.h == 0.0 && g.content.is_none(),
                "{b}"
            );
        }
        let summary = &l.geoms()[s.summary.start as usize];
        assert!(summary.h > 0.0 && summary.content.is_some());
    }
    // Hidden blocks are not hit, and the triangle isn't text.
    let summary = doc.sections[0].summary.start as usize;
    let c = l.geoms()[summary].content.as_ref().unwrap();
    assert!(
        c.glyphs.len() > c.hits.len(),
        "the triangle has glyphs and no hits"
    );
}

/// Sol r1, finding 1: restoring the states of many sibling sections (a live reload of a
/// document whose sections lost their `open`, kept open by the window) is one pass, and one
/// toggle doesn't look at the siblings before it.
#[test]
fn many_sibling_sections_restore_and_toggle_in_linear_time() {
    let siblings =
        |n: usize| Doc::parse(&"<details><summary>S</summary>body</details>\n\n".repeat(n));
    crate::doc::assert_linear("restoring sibling sections", 10_000, |n| {
        let doc = siblings(n);
        let mut l = Layout::new(&doc, params(900.0, 1.0));
        l.set_sections(&doc, &vec![true; doc.sections.len()]);
        assert!(!l.hidden(doc.blocks.len() - 1));
    });
    crate::doc::assert_linear("toggling sibling sections", 5_000, |n| {
        let doc = siblings(n);
        let mut l = Layout::new(&doc, params(900.0, 1.0));
        let mut open = vec![false; doc.sections.len()];
        for s in 0..open.len() {
            open[s] = true;
            l.set_open(&doc, s, &open);
        }
        assert!(!l.hidden(doc.blocks.len() - 1));
    });
}

/// Opening a section shows its body below the summary: nothing above moves, the blocks after it
/// move down by the body's height; closing it again puts everything back. Each costs about
/// O(log n) per body block, however long the document.
#[test]
fn toggling_costs_the_body() {
    let fonts = Fonts::new().unwrap();
    let mut per_block = Vec::new();
    for n in [100, 400] {
        let doc = sections(n, 4);
        let mut l = laid_out(&doc, &fonts);
        let mut open = vec![false; doc.sections.len()];
        let s = n / 2;
        let sec = doc.sections[s].clone();
        let (above, after) = (l.y(sec.summary.end as usize), l.y(sec.body.end as usize));
        let before = l.position_ops();
        open[s] = true;
        l.set_open(&doc, s, &open);
        per_block.push((l.position_ops() - before) as f64 / sec.body.len() as f64);
        check_geometry(&l);
        assert_eq!(l.y(sec.summary.end as usize), above, "nothing above moves");
        assert!(l.y(sec.body.end as usize) > after, "the rest moves down");
        assert!(!l.hidden(sec.body.start as usize));
        // Measured where it shows, the same heights as a layout that starts open.
        l.ensure(&doc, &fonts, Anchor::TOP, 100_000.0);
        while !l.idle(&doc, &fonts, Instant::now() + Duration::from_secs(5)) {}
        let mut fresh = Layout::new(&doc, params(900.0, 1.0));
        fresh.set_sections(&doc, &open);
        while !fresh.idle(&doc, &fonts, Instant::now() + Duration::from_secs(5)) {}
        for b in sec.body.clone() {
            assert_eq!(l.geoms()[b as usize].h, fresh.geoms()[b as usize].h);
        }
        assert!((l.total_height() - fresh.total_height()).abs() < 0.01);
        open[s] = false;
        l.set_open(&doc, s, &open);
        assert_eq!(
            l.y(sec.body.end as usize),
            after,
            "closed again, back where it was"
        );
    }
    assert!(per_block[1] < per_block[0] * 2.0, "{per_block:?}");
}

/// Spacing is against the last block shown: after a closed body, against the summary.
#[test]
fn spacing_skips_hidden_blocks() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("<details><summary>S</summary>\n\n# Heading\n\n</details>\n\nAfter\n");
    let mut l = laid_out(&doc, &fonts);
    let gap = |l: &Layout, i: usize| from_quarters(l.meta[i].gap);
    // A paragraph after a paragraph (the summary), not after the heading.
    assert_eq!(gap(&l, 2), 1.0);
    l.set_open(&doc, 0, &[true]);
    assert_eq!((gap(&l, 1), gap(&l, 2)), (1.5, 1.0));
    // Review r1, finding 5: the outer section closed and an inner one opened at once (as a new
    // layout of the window's states does) give the spacing a fresh layout has.
    let doc = Doc::parse(
        "<details><summary>Outer</summary>\n\n<details open><summary>\n\n---\n\n</summary>\n\n\
         Inner body\n\n</details>\n</details>\n\nAfter\n",
    );
    let mut l = laid_out(&doc, &fonts);
    l.set_sections(&doc, &[true, false]);
    l.set_sections(&doc, &[false, true]);
    let mut fresh = Layout::new(&doc, params(900.0, 1.0));
    fresh.set_sections(&doc, &[false, true]);
    let gaps = |l: &Layout| (0..doc.blocks.len()).map(|i| gap(l, i)).collect::<Vec<_>>();
    assert_eq!(gaps(&l), gaps(&fresh));
}

/// Review r1, finding 4: a short row of an HTML table has no cells drawn where it has none.
#[test]
fn short_html_rows_end_with_their_cells() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(
        "<table><tr><td>a</td><td>b</td><td>c</td></tr><tr><td>d</td></tr></table>\n\n\
         | x | y | z |\n|---|---|---|\n| w |\n",
    );
    let l = laid_out(&doc, &fonts);
    let (full, _) = rules(&l, 0);
    let (short, bottom) = rules(&l, 1);
    assert_eq!(
        (full.len(), short.len()),
        (4, 2),
        "the left edge and its one cell's"
    );
    let right = bottom.iter().map(|(x, w)| x + w).fold(0.0, f32::max);
    assert!(
        (right - short[1]).abs() < 2.0,
        "the rule below ends with the cell"
    );
    // A Markdown table's rows have every column.
    let (md, _) = rules(&l, 3);
    assert_eq!(md.len(), 4);
    // Review r2, finding 2: a longer row under a shorter one has a rule above its own extra
    // cells (and none doubling the shorter row's rule below).
    let doc = Doc::parse("<table><tr><td>Logo</td></tr><tr><td>a</td><td>b</td></tr></table>\n");
    let l = laid_out(&doc, &fonts);
    let (v, _) = rules(&l, 1);
    let c = l.geoms()[1].content.as_ref().unwrap();
    let top = c.rects.iter().map(|r| r.y).fold(f32::INFINITY, f32::min);
    let tops: Vec<(f32, f32)> = (c.rects.iter())
        .filter(|r| r.role == Role::Border && r.h <= 1.0 && r.w > 1.0 && r.y == top)
        .map(|r| (r.x, r.x + r.w))
        .collect();
    assert_eq!(tops.len(), 1, "{tops:?}");
    assert!(
        (tops[0].0 - v[1]).abs() < 2.0 && (tops[0].1 - v[2]).abs() < 2.0,
        "{tops:?} {v:?}"
    );
}

/// A closed section inside an open one stays hidden; opening the outer one shows only the
/// blocks that aren't in it. New sizes and fonts keep the bodies hidden.
#[test]
fn nested_and_relaid_sections() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(
        "<details><summary>Outer</summary>\n\nA\n\n<details><summary>Inner</summary>\n\nB\n\n\
         </details>\n\nC\n\n</details>\n",
    );
    let mut l = laid_out(&doc, &fonts);
    let hidden = |l: &Layout| {
        (0..doc.blocks.len())
            .map(|i| l.hidden(i))
            .collect::<Vec<_>>()
    };
    assert_eq!(hidden(&l), [false, true, true, true, true]);
    l.set_open(&doc, 0, &[true, false]);
    assert_eq!(hidden(&l), [false, false, false, true, false]);
    l.set_params(params(500.0, 2.0));
    l.invalidate_all();
    while !l.idle(&doc, &fonts, Instant::now() + Duration::from_secs(5)) {}
    assert_eq!(hidden(&l), [false, false, false, true, false]);
    assert_eq!(l.geoms()[3].h, 0.0);
    check_geometry(&l);
}

/// The blocks a table row lays out cells for: a closed section's rows lay none out.
#[test]
fn hidden_rows_are_never_built() {
    let fonts = Fonts::new().unwrap();
    let rows: String = (0..50).map(|i| format!("| {i} | x |\n")).collect();
    let doc = Doc::parse(&format!(
        "<details><summary>S</summary>\n\n| a | b |\n|---|---|\n{rows}\n</details>\n"
    ));
    let mut l = laid_out(&doc, &fonts);
    // (The columns are measured, from the sampled rows; no row is laid out.)
    let laid = l.cells_laid;
    assert_eq!(laid, 0);
    l.set_open(&doc, 0, &[true]);
    l.ensure(&doc, &fonts, Anchor::TOP, 100_000.0);
    assert!(l.cells_laid > 50);
}

/// The x of the first glyph of the cell whose text starts with `text`, and its y, in a row.
fn glyph_at(doc: &Doc, l: &Layout, row: usize, text: &str) -> (f32, f32) {
    let c = l.geoms()[row].content.as_ref().unwrap();
    let off = doc.text.find(text).unwrap() as u32;
    let h = c.hits.iter().find(|h| h.doc == off).unwrap();
    let line = c
        .lines
        .iter()
        .find(|line| (line.hits.start..line.hits.end).any(|i| c.hits[i as usize].doc == off));
    (h.x0, line.unwrap().baseline)
}

/// As on GitHub: a header cell is centered unless its column is aligned, in Markdown tables
/// too, and a row's cells are centered vertically unless `valign` says otherwise.
#[test]
fn header_and_vertical_alignment() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("| head | left |\n|---|:--|\n| a much longer cell | b |\n");
    let l = laid_out(&doc, &fonts);
    let (x_head, _) = glyph_at(&doc, &l, 0, "head");
    let (x_long, _) = glyph_at(&doc, &l, 1, "a much");
    let (x_left, _) = glyph_at(&doc, &l, 0, "left");
    let (x_b, _) = glyph_at(&doc, &l, 1, "b");
    assert!(x_head > x_long + 10.0, "centered: {x_head} vs {x_long}");
    assert!(
        (x_left - x_b).abs() < 0.5,
        "an aligned column's header isn't centered"
    );
    let doc = Doc::parse(
        "<table><tr><td>mid</td><td valign=top>top</td><td valign=bottom>low</td>\
         <td>1<br>2<br>3</td></tr></table>\n",
    );
    let l = laid_out(&doc, &fonts);
    let y = |t| glyph_at(&doc, &l, 0, t).1;
    assert!(y("top") < y("mid") && y("mid") < y("low"));
    assert!((y("top") - y("1")).abs() < 0.5, "top: on the first line");
    assert!(
        (y("mid") - y("2")).abs() < 0.5,
        "middle: on the middle line"
    );
    assert!((y("low") - y("3")).abs() < 0.5, "bottom: on the last line");
}

/// The rules of a row: vertical ones (x of each), and the bottom one's runs (x, width).
fn rules(l: &Layout, row: usize) -> (Vec<f32>, Vec<(f32, f32)>) {
    let c = l.geoms()[row].content.as_ref().unwrap();
    let border = c.rects.iter().filter(|r| r.role == Role::Border);
    let (mut v, mut h) = (Vec::new(), Vec::new());
    let bottom = c.rects.iter().map(|r| r.y + r.h).fold(0.0, f32::max);
    for r in border {
        if r.w <= 1.0 && r.h > 1.0 {
            v.push(r.x);
        } else if (r.y + r.h - bottom).abs() < 0.01 {
            h.push((r.x, r.w));
        }
    }
    (v, h)
}

/// A cell spanning columns is as wide as they are, with no rule inside it; one spanning rows
/// leaves the rule under its first row out across its columns.
#[test]
fn spanning_cells() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(
        "<table><tr><td>one</td><td>two</td><td rowspan=2>tall</td></tr>\
         <tr><td colspan=2>both</td></tr><tr><td>x</td><td>y</td><td>z</td></tr></table>\n",
    );
    let l = laid_out(&doc, &fonts);
    let (v0, h0) = rules(&l, 0);
    let (v1, h1) = rules(&l, 1);
    assert_eq!(v0.len(), 4, "left edge and three columns: {v0:?}");
    assert_eq!(v1.len(), 3, "no rule inside the spanning cell: {v1:?}");
    assert_eq!(h0.len(), 1, "one run under the first row");
    assert!(
        h0[0].1 < h1[0].1,
        "not under the cell spanning rows: {h0:?} {h1:?}"
    );
    let (vx, _) = rules(&l, 2);
    assert_eq!(vx.len(), 4);
}

/// `<table align=center>` sits in the middle of the column; `width` sets a column's width.
#[test]
fn centered_tables_and_widths() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(
        "<table align=center><tr><td>a</td></tr></table>\n\n\
         <table><tr><td width=300>b</td><td>c</td></tr></table>\n",
    );
    let l = laid_out(&doc, &fonts);
    let (col_x, col_w) = l.column();
    let (v, _) = rules(&l, 0);
    let mid = (v[0] + v[1]) / 2.0;
    assert!(
        (mid - col_w / 2.0).abs() < 2.0,
        "{mid} vs {} ({col_x})",
        col_w / 2.0
    );
    let (v, _) = rules(&l, 1);
    let w = v[1] - v[0];
    assert!((w - (300.0 + 2.0 * CELL_PAD_X * 16.0)).abs() < 2.0, "{w}");
}

/// Fuzz findings (`layout` target): opening a section inside a closed one showed the block right
/// after its body, which only has its spacing looked at again; a section in a summary isn't in
/// the body. Any order of opening and closing ends with what a fresh layout of the same states
/// hides.
#[test]
fn any_order_of_toggles_ends_the_same() {
    let seeds: [&[u8]; 2] = [
        include_bytes!("../../../fuzz/seeds/layout/sections-reopened.bin"),
        include_bytes!("../../../fuzz/seeds/layout/section-in-a-summary.bin"),
    ];
    for data in seeds {
        let doc = Doc::parse(&String::from_utf8_lossy(&data[4..]));
        let n = doc.sections.len();
        let hidden = |l: &Layout| {
            (0..doc.blocks.len())
                .map(|i| l.hidden(i))
                .collect::<Vec<_>>()
        };
        for states in 0..1u32 << n.min(8) {
            let open: Vec<bool> = (0..n).map(|s| states >> (s % 8) & 1 != 0).collect();
            let mut l = Layout::new(&doc, params(900.0, 1.0));
            l.set_sections(&doc, &(0..n).map(|s| s % 2 == 0).collect::<Vec<_>>());
            l.set_sections(&doc, &open);
            let mut fresh = Layout::new(&doc, params(900.0, 1.0));
            fresh.set_sections(&doc, &open);
            assert_eq!(hidden(&l), hidden(&fresh), "{open:?}");
            let want: Vec<bool> = hidden_blocks(&doc, |s| open[s]).collect();
            assert_eq!(hidden(&l), want, "{open:?}");
            check_geometry(&l);
        }
    }
}
