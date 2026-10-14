use std::time::{Duration, Instant};

use super::*;
use crate::text::fallback;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn params(width: f32, scale: f32) -> Params {
    Params {
        width: width * scale,
        scale,
        ..Params::default()
    }
}

/// Measure every block (idle until complete).
fn complete(doc: &Doc, fonts: &Fonts, p: Params) -> Layout {
    let mut l = Layout::new(doc, p);
    while !l.idle(doc, fonts, Instant::now() + Duration::from_secs(5)) {}
    assert!(l.complete());
    l
}

fn check_geometry(l: &Layout) {
    let mut y = 0.0;
    for g in l.geoms() {
        assert!((g.y - y).abs() < 1e-2, "blocks must be contiguous");
        assert!(g.h >= 0.0 && g.h.is_finite());
        y += g.h;
    }
    assert!((l.total_height() - y).abs() < 1e-2);
}

/// A stable fingerprint of all heights plus the content of blocks near the viewport.
fn signature(l: &Layout) -> Vec<String> {
    l.geoms()
        .iter()
        .map(|g| {
            let c = g.content.as_ref().map(|c| {
                let gl: f32 = c.glyphs.iter().map(|g| g.x * 3.0 + g.y).sum();
                format!(
                    "{} {} {} {gl:.3}",
                    c.glyphs.len(),
                    c.rects.len(),
                    c.lines.len()
                )
            });
            format!("{:.3} {c:?}", g.h)
        })
        .collect()
}

#[test]
fn spec_layout_is_deterministic_across_widths_and_scales() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("spec.md"));
    for width in [480.0, 900.0, 1600.0] {
        for scale in [1.0, 2.0] {
            let run = || {
                let mut l = complete(&doc, &fonts, params(width, scale));
                let mid = l.anchor_at(l.total_height() / 2.0);
                l.ensure(&doc, &fonts, mid, 1000.0 * scale);
                check_geometry(&l);
                signature(&l)
            };
            assert_eq!(run(), run(), "width {width} scale {scale}");
        }
    }
}

#[test]
fn content_stays_in_column() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("readme.md"));
    let mut l = complete(&doc, &fonts, params(900.0, 1.0));
    let (_, col_w) = l.column();
    let em = 16.0;
    let mut y = 0.0;
    while y < l.total_height() {
        y = l.ensure(&doc, &fonts, l.anchor_at(y), 1000.0) + 1000.0;
        for (i, g) in l.geoms().iter().enumerate() {
            let Some(c) = &g.content else { continue };
            let table = matches!(doc.blocks[i].kind, BlockKind::TableRow { .. });
            for gl in &c.glyphs {
                // Markers hang in the gutter, at most one list indent left of the column.
                assert!(
                    gl.x >= -LIST_INDENT * em - 1.0,
                    "block {i} glyph at {}",
                    gl.x
                );
                if !table {
                    assert!(gl.x <= col_w + 1.0, "block {i} glyph at {} > {col_w}", gl.x);
                }
                assert!(
                    gl.y >= 0.0 && gl.y <= g.h + 1.0,
                    "block {i} glyph y {} / {}",
                    gl.y,
                    g.h
                );
            }
        }
    }
}

#[test]
fn ensure_measures_near_viewport_and_keeps_anchor() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("spec.md"));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    let anchor = Anchor {
        block: 600,
        frac: 0.5,
    };
    let y = l.ensure(&doc, &fonts, anchor, 1000.0);
    assert!((y - l.y_of(anchor)).abs() < 1e-3);
    assert!(l.geoms()[600].content.is_some() && l.geoms()[600].measured);
    // Far-away blocks are neither shaped nor kept.
    assert!(l.geoms()[0].content.is_none() && !l.geoms()[0].measured);
    let live = l.geoms().iter().filter(|g| g.content.is_some()).count();
    assert!(live < 300, "{live} blocks kept");
    // A second call is a no-op.
    assert_eq!(l.ensure(&doc, &fonts, anchor, 1000.0), y);
    // Scrolling far away drops the old content.
    l.ensure(&doc, &fonts, Anchor::TOP, 1000.0);
    assert!(l.geoms()[600].content.is_none());
    assert!(l.geoms()[0].content.is_some());
}

#[test]
fn idle_heights_match_viewport_heights() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("readme.md"));
    let mut l = complete(&doc, &fonts, params(900.0, 1.0));
    let before: Vec<f32> = l.geoms().iter().map(|g| g.h).collect();
    let mid = l.anchor_at(l.total_height() / 2.0);
    l.ensure(&doc, &fonts, mid, 1000.0);
    let after: Vec<f32> = l.geoms().iter().map(|g| g.h).collect();
    assert_eq!(before, after);
}

#[test]
fn idle_respects_deadline() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("spec.md"));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    // An expired deadline still makes progress (one block) but stops immediately.
    assert!(!l.idle(&doc, &fonts, Instant::now()));
    assert_eq!(l.geoms().iter().filter(|g| g.measured).count(), 1);
}

#[test]
fn set_params_remeasures() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("readme.md"));
    let mut l = complete(&doc, &fonts, params(900.0, 1.0));
    let narrow_total = {
        let mut n = complete(&doc, &fonts, params(500.0, 1.0));
        n.ensure(&doc, &fonts, Anchor::TOP, 800.0);
        n.total_height()
    };
    l.set_params(params(500.0, 1.0));
    assert!(!l.complete());
    while !l.idle(&doc, &fonts, Instant::now() + Duration::from_secs(5)) {}
    assert!((l.total_height() - narrow_total).abs() < 1e-2);
    assert!(narrow_total > complete(&doc, &fonts, params(900.0, 1.0)).total_height());
}

#[test]
fn hit_test_finds_links() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("Click [here](https://example.com) now.");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let (col_x, _) = l.column();
    let c = l.geoms()[0].content.as_ref().unwrap();
    let line = &c.lines[0];
    let link_hit = c
        .hits
        .iter()
        .find(|h| c.styles[h.style as usize].link == 1)
        .unwrap();
    let x = col_x + (link_hit.x0 + link_hit.x1) / 2.0;
    let y = (line.top + line.bottom) / 2.0;
    let hit = l.hit(x, y).unwrap();
    assert_eq!(hit.link, 1);
    assert_eq!(
        &doc.text[hit.doc as usize..hit.doc as usize + 1],
        &doc.text[link_hit.doc as usize..][..1]
    );
    assert_eq!(l.hit(col_x + c.hits[0].x0 + 1.0, y).unwrap().link, 0);
    assert!(l.hit(col_x - 50.0, y).is_none());
}

#[test]
fn list_markers_hang_in_gutter() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("- one\n- two\n  1. nested\n\n- [x] done\n- [ ] todo\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let em = 16.0;
    for (i, g) in l.geoms().iter().enumerate() {
        let c = g.content.as_ref().unwrap();
        let indent = doc.blocks[i].list as f32 * LIST_INDENT * em;
        match doc.blocks[i].marker {
            Marker::Bullet | Marker::Ordered(_) => {
                assert!(
                    c.glyphs.iter().any(|g| g.x < indent),
                    "block {i} marker in gutter"
                );
            }
            Marker::Task(checked) => {
                let boxes: Vec<_> = c.rects.iter().filter(|r| r.x < indent).collect();
                assert_eq!(boxes.len(), 1);
                assert_eq!(boxes[0].kind == RectKind::Fill, checked);
            }
            _ => {}
        }
    }
}

#[test]
fn code_blocks_wrap_with_continuation_markers() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&format!("```\nshort\n{}\n```\n", "word ".repeat(60)));
    let mut l = Layout::new(&doc, params(600.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let c = l.geoms()[0].content.as_ref().unwrap();
    let conts = c.lines.iter().filter(|l| l.cont).count();
    assert!(conts >= 2);
    // One marker glyph per continuation, in the left padding.
    let markers = c.glyphs.iter().filter(|g| g.x < 16.0).count();
    assert_eq!(markers, conts);
    assert_eq!(c.rects[0].role, Role::CodeBg);
}

#[test]
fn tabs_expand_to_four_columns() {
    let mut p = Para::default();
    let s = TextStyle {
        bold: false,
        italic: false,
        mono: true,
        size: 14.0,
        role: Role::Text,
        deco: 0,
        shift: 0.0,
        link: 0,
    };
    push_expanding_tabs(&mut p, "a\tb\n\tc\tdd", 10, s);
    assert_eq!(p.text, "a   b\n    c   dd");
    assert_eq!(p.doc_offset(1), 11); // expanded tab maps to the tab
    assert_eq!(p.doc_offset(3), 11);
    assert_eq!(p.doc_offset(4), 12); // 'b'
}

#[test]
fn tables_lay_out_columns() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("| a | long header |\n|---|---:|\n| 1 | 2 |\n| x | y |\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let cols = l.table_cols[0].clone().unwrap();
    assert_eq!(cols.len(), 2);
    assert!(cols[1] > cols[0]);
    // Rows are equal height and contiguous; the second data row is striped.
    let hs: Vec<f32> = l.geoms().iter().map(|g| g.h).collect();
    assert!((hs[1] - hs[2]).abs() < 1e-3);
    let striped = |i: usize| {
        l.geoms()[i]
            .content
            .as_ref()
            .unwrap()
            .rects
            .iter()
            .any(|r| r.role == Role::TableStripe)
    };
    assert!(!striped(1) && striped(2));
    // Right-aligned column: cell text ends at the column's right padding.
    let row = l.geoms()[1].content.as_ref().unwrap();
    let last = row.hits.last().unwrap();
    let right = cols[0] + cols[1] - CELL_PAD_X * 16.0;
    assert!((last.x1 - right).abs() < 1.0, "{} vs {right}", last.x1);
}

#[test]
fn wide_tables_share_space() {
    let fonts = Fonts::new().unwrap();
    let long = "lorem ipsum dolor sit amet ".repeat(10);
    let doc = Doc::parse(&format!("| a | b |\n|---|---|\n| {long} | {long} |\n"));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let (_, col_w) = l.column();
    let total: f32 = l.table_cols[0].as_ref().unwrap().iter().sum();
    assert!(total <= col_w + 2.0, "{total} > {col_w}");
}

#[test]
fn headings_quotes_and_alerts() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("# Title\n\n> outer\n> > inner\n\n> [!WARNING]\n> careful\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let content = |i: usize| l.geoms()[i].content.as_ref().unwrap();
    assert!(content(0).rects.iter().any(|r| r.role == Role::BorderMuted));
    let bars = |i: usize, role: Role| content(i).rects.iter().filter(|r| r.role == role).count();
    assert_eq!(bars(1, Role::QuoteBar), 1);
    assert_eq!(bars(2, Role::QuoteBar), 2);
    assert_eq!(bars(3, Role::AlertWarning), 1);
    assert_eq!(bars(4, Role::AlertWarning), 1);
    // Quote text is muted, alert text is not.
    assert_eq!(content(1).styles[0].role, Role::Muted);
    assert_eq!(content(4).styles[0].role, Role::Text);
}

#[test]
fn heading_slugs_and_footnotes_lay_out() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("Ref[^1].\n\n[^1]: Note.\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let refs: Vec<_> = l.geoms()[0]
        .content
        .as_ref()
        .unwrap()
        .styles
        .iter()
        .filter(|s| s.shift > 0.0)
        .collect();
    assert_eq!(refs.len(), 1);
    let note = l.geoms().last().unwrap().content.as_ref().unwrap();
    assert!(note.styles.iter().all(|s| s.size < 16.0));
}

#[test]
fn missing_characters_and_fallback() {
    let fonts_embedded = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("cjk.md"));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts_embedded, Anchor::TOP, 2000.0);
    let missing = l.take_missing();
    assert!(missing.contains(&'中'));
    assert!(l.take_missing().is_empty());
    assert!(l.geoms().iter().any(|g| g.missing));

    // With system fallbacks (when the machine has them), the blocks remeasure cleanly.
    let mut fonts = Fonts::new().unwrap();
    let found = fallback::find_fallbacks(&missing, &|p, i| fonts_embedded.has_source(p, i));
    if found.is_empty() {
        return;
    }
    for f in &found {
        fonts.add_fallback(&f.path, f.index);
    }
    l.invalidate_missing(&doc);
    assert!(l.geoms().iter().all(|g| !g.missing));
    l.ensure(&doc, &fonts, Anchor::TOP, 2000.0);
    let still = l.take_missing();
    assert!(
        !still.contains(&'中'),
        "fallback should cover CJK: {still:?}"
    );
}

#[test]
fn empty_document() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("");
    let mut l = Layout::new(&doc, Params::default());
    assert_eq!(l.ensure(&doc, &fonts, Anchor::TOP, 800.0), 0.0);
    assert!(l.idle(&doc, &fonts, Instant::now()));
    assert_eq!(l.total_height(), 0.0);
    assert_eq!(l.range(0.0, 100.0), 0..0);
    assert!(l.hit(10.0, 10.0).is_none());
    assert_eq!(l.anchor_at(50.0), Anchor::TOP);
}

#[test]
fn narrow_and_tiny_windows_do_not_panic() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&fixture("tables.md"));
    for width in [1.0, 10.0, 50.0, 200.0] {
        let mut l = complete(&doc, &fonts, params(width, 1.0));
        l.ensure(&doc, &fonts, Anchor::TOP, 100.0);
        check_geometry(&l);
    }
}

/// x positions of a table row's vertical rules.
fn rules(c: &BlockBox) -> Vec<f32> {
    c.rects
        .iter()
        .filter(|r| r.role == Role::Border && r.w <= 1.0 && r.h > 1.0)
        .map(|r| r.x)
        .collect()
}

#[test]
fn fallback_invalidation_remeasures_whole_tables() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("before\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let rows = l.table_rows[0].clone();
    assert_eq!(rows.len(), 3);
    l.take_missing();
    l.cursor = l.geoms.len();
    l.geoms[rows.start as usize + 1].missing = true;
    l.invalidate_missing(&doc);
    for r in rows.start..rows.end {
        let g = &l.geoms[r as usize];
        assert!(!g.measured && g.content.is_none() && !g.missing, "row {r}");
    }
    assert!(l.geoms[0].measured && l.table_cols[0].is_none());
    assert_eq!(l.cursor, rows.start as usize);
    assert!(l.live.iter().all(|&i| !rows.contains(&i)));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let xs: Vec<_> = rows
        .map(|r| rules(l.geoms[r as usize].content.as_ref().unwrap()))
        .collect();
    assert_eq!(xs[0].len(), 3);
    assert!(xs.iter().all(|x| *x == xs[0]), "{xs:?}");
}

#[test]
fn long_ordered_markers_stay_on_one_line() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("100. hundred\n101. next\n\n10000. big");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let mut seen = 0;
    for (i, g) in l.geoms().iter().enumerate() {
        let c = g.content.as_ref().unwrap();
        if !matches!(doc.blocks[i].marker, Marker::Ordered(_)) {
            continue;
        }
        seen += 1;
        assert!(c.glyphs.iter().all(|gl| gl.y <= g.h), "block {i} overflows");
        // Marker glyphs (left of the text) share one baseline, right edge in the gutter.
        let indent = LIST_INDENT * 16.0;
        let marker: Vec<_> = c.glyphs.iter().filter(|gl| gl.x < indent - 8.0).collect();
        assert!(marker.len() >= 4);
        assert!(marker.iter().all(|gl| gl.y == marker[0].y));
    }
    assert_eq!(seen, 3);
}

#[test]
fn quote_after_alert_is_a_separate_quote() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("> [!NOTE]\n> n\n\n> plain quote\n\n> [!TIP]\n> t\n> > nested\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let content = |i: usize| l.geoms()[i].content.as_ref().unwrap();
    let bar = |i: usize, role: Role| content(i).rects.iter().find(|r| r.role == role);
    assert!(bar(1, Role::AlertNote).is_some());
    // The plain quote is a plain quote: its own bar starts below the gap and isn't an alert.
    assert!(bar(2, Role::AlertNote).is_none());
    let b = bar(2, Role::QuoteBar).unwrap();
    assert!(b.y > 0.0);
    assert_eq!(content(2).styles[0].role, Role::Muted);
    // A nested quote inside an alert stays part of the alert.
    assert!(bar(5, Role::AlertTip).is_some());
    assert_eq!(content(5).styles[0].role, Role::Text);
}

#[test]
fn decoration_text_is_not_hit_testable() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("- [x] item\n\n> [!NOTE]\n> n\n\n```\nshort\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let (col_x, _) = l.column();
    // The bullet's glyphs exist, but no hit or line maps to them (they aren't document text).
    let c = l.geoms()[0].content.as_ref().unwrap();
    assert!(c.glyphs.iter().any(|g| g.x < LIST_INDENT * 16.0 - 8.0));
    let y = l.geoms()[0].y + c.lines[0].baseline - 4.0;
    assert!(l.hit(col_x + LIST_INDENT * 16.0 - 12.0, y).is_none());
    // Real text still hits, at its own offset.
    let text = l.hit(col_x + LIST_INDENT * 16.0 + 2.0, y).unwrap();
    assert_eq!(text.doc as usize, doc.text.find("item").unwrap());
    // Alert titles have no hits either.
    let title = l.geoms()[1].content.as_ref().unwrap();
    assert!(title.hits.is_empty() && title.lines.is_empty());
}

#[test]
fn wrapped_code_shares_one_marker_style() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&format!("```\n{}\n```\n", "word ".repeat(2000)));
    let mut l = Layout::new(&doc, params(300.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let c = l.geoms()[0].content.as_ref().unwrap();
    let conts = c.lines.iter().filter(|l| l.cont).count();
    assert!(conts > 100);
    assert!(c.styles.len() <= 3, "{} styles", c.styles.len());
    assert_eq!(c.glyphs.iter().filter(|g| g.x < 16.0).count(), conts);
    assert!(c.glyphs.iter().all(|g| (g.style as usize) < c.styles.len()));
}

#[test]
fn uncoverable_characters_are_reported_once() {
    let fonts = Fonts::new().unwrap();
    // Private use: no face covers it.
    let doc = Doc::parse("a \u{E000} b\n\n| x | \u{E000} |\n|---|---|\n| 1 | 2 |\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    assert_eq!(l.take_missing(), vec!['\u{E000}']);
    assert!(l.geoms().iter().any(|g| g.missing));
    // A fallback scan finds nothing; remeasuring must not ask for it again.
    l.invalidate_missing(&doc);
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    assert!(l.take_missing().is_empty());
}

#[test]
fn blocks_measured_during_the_fallback_scan_are_invalidated() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&"a \u{E000} b\n\n".repeat(60));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 100.0);
    assert_eq!(l.take_missing(), vec!['\u{E000}']);
    let early = l.geoms().iter().filter(|g| g.measured).count();
    // The scan is still running: the window grows and more blocks are measured.
    l.ensure(&doc, &fonts, Anchor::TOP, 1000.0);
    assert!(l.geoms().iter().filter(|g| g.measured).count() > early);
    assert!(l.take_missing().is_empty());
    // The scan finishes: every block laid out without the character is remeasured.
    l.invalidate_missing(&doc);
    assert!(l.geoms().iter().all(|g| !g.measured && g.content.is_none()));
}

#[test]
fn quote_bars_follow_nesting_order() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("- item\n\n  > quoted\n\n> - item\n>   > deep\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let em = 16.0;
    let bars = |i: usize| -> Vec<f32> {
        let c = l.geoms()[i].content.as_ref().unwrap();
        c.rects
            .iter()
            .filter(|r| r.role == Role::QuoteBar)
            .map(|r| r.x)
            .collect()
    };
    let text_x = |i: usize| {
        let c = l.geoms()[i].content.as_ref().unwrap();
        c.glyphs.iter().map(|g| g.x).fold(f32::MAX, f32::min)
    };
    // A quote inside a list item: the bar sits inside the list indent, text after the bar.
    assert_eq!(bars(1), vec![LIST_INDENT * em]);
    assert!(text_x(1) >= (LIST_INDENT + QUOTE_INDENT) * em - 0.5);
    // A list inside a quote: the bar is outside, at the margin.
    assert_eq!(bars(2), vec![0.0]);
    let (col_x, _) = l.column();
    let c = l.geoms()[2].content.as_ref().unwrap();
    let y = l.geoms()[2].y + c.lines[0].baseline - 4.0;
    let hit = l
        .hit(col_x + (QUOTE_INDENT + LIST_INDENT) * em + 2.0, y)
        .unwrap();
    assert_eq!(hit.doc as usize, doc.text.rfind("item").unwrap());
    // A quote inside that list: outer bar at the margin, inner one inside the list indent.
    assert_eq!(bars(3), vec![0.0, (QUOTE_INDENT + LIST_INDENT) * em]);
}

#[test]
fn paragraph_with_many_distinct_links_lays_out() {
    let fonts = Fonts::new().unwrap();
    let src: String = (0..20_000).map(|i| format!("[a](#{i}) ")).collect();
    let doc = Doc::parse(&src);
    // The paragraph is split into chunks, each laid out with its own style table.
    assert!(doc.blocks.len() > 1);
    let last = doc.blocks.len() - 1;
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    let anchor = Anchor {
        block: last as u32,
        frac: 0.5,
    };
    l.ensure(&doc, &fonts, anchor, 800.0);
    let c = l.geoms()[last].content.as_ref().unwrap();
    let links: Vec<u32> = c.styles.iter().map(|s| s.link).collect();
    assert_eq!(links.iter().max(), Some(&20_000));
    // One style per link in the chunk, plus the plain style of the spaces between them.
    let spans = doc.block_spans(&doc.blocks[last]);
    assert!((spans.len() / 2..=spans.len() / 2 + 2).contains(&c.styles.len()));
}

/// Right edge of a glyph, from its face's advance.
fn glyph_right(fonts: &Fonts, c: &BlockBox, g: &PGlyph) -> f32 {
    let size = c.styles[g.style as usize].size;
    let metrics = fonts.face(g.face).font().glyph_metrics(&[]).scale(size);
    g.x + metrics.advance_width(g.id)
}

fn quote_bar_xs(c: &BlockBox) -> Vec<f32> {
    c.rects
        .iter()
        .filter(|r| r.role == Role::QuoteBar)
        .map(|r| r.x)
        .collect()
}

#[test]
fn markers_stay_left_of_quote_bars_inside_items() {
    let fonts = Fonts::new().unwrap();
    let em = 16.0;
    for src in ["- > q\n", "10. > q\n", "- [ ]\n  > q\n", "- [x]\n  > q\n"] {
        let doc = Doc::parse(src);
        let mut l = Layout::new(&doc, params(900.0, 1.0));
        l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
        let c = l.geoms()[0].content.as_ref().unwrap();
        let block = &doc.blocks[0];
        assert!(block.quote == 1 && block.marker != Marker::None, "{src:?}");
        let bar = quote_bar_xs(c)[0];
        // Right edge of the marker: the checkbox rect, or the glyphs left of the bar.
        let edge = match block.marker {
            Marker::Task(_) => c
                .rects
                .iter()
                .filter(|r| r.role != Role::QuoteBar)
                .map(|r| r.x + r.w)
                .fold(f32::MIN, f32::max),
            _ => c
                .glyphs
                .iter()
                .filter(|g| g.x < bar)
                .map(|g| glyph_right(&fonts, c, g))
                .fold(f32::MIN, f32::max),
        };
        assert!(
            edge > f32::MIN && edge <= bar,
            "{src:?}: marker ends at {edge}, bar at {bar}"
        );
        // And it still hangs in the item's gutter, not the quote's text indent.
        assert!(edge >= bar - em, "{src:?}: marker at {edge}, bar at {bar}");
    }
}

#[test]
fn footnote_quote_bars_sit_inside_the_footnote_indent() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("x[^1]\n\n[^1]: > note\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let em = 16.0;
    let (i, block) = doc
        .blocks
        .iter()
        .enumerate()
        .find(|(_, b)| b.quote == 1)
        .unwrap();
    assert!(matches!(block.marker, Marker::Footnote(1)));
    let c = l.geoms()[i].content.as_ref().unwrap();
    // The number hangs in the footnote gutter; the bar starts at the footnote's indent and the
    // text follows the bar.
    let bar = quote_bar_xs(c)[0];
    assert_eq!(bar, LIST_INDENT * em);
    let number = c.glyphs.iter().filter(|g| g.x < bar).collect::<Vec<_>>();
    assert!(!number.is_empty());
    assert!(number.iter().all(|g| glyph_right(&fonts, c, g) <= bar));
    let text = c
        .glyphs
        .iter()
        .filter(|g| g.x >= bar)
        .map(|g| g.x)
        .fold(f32::MAX, f32::min);
    assert!(text >= bar + (QUOTE_INDENT - 0.01) * em, "text at {text}");
}

#[test]
fn deep_quote_nesting_keeps_exact_list_depths() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse("> > > > - > note\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let em = 16.0;
    let c = l.geoms()[0].content.as_ref().unwrap();
    // Four quotes outside the list, then the list indent, then the fifth quote inside it.
    let expect: Vec<f32> = (0..4)
        .map(|q| q as f32 * QUOTE_INDENT * em)
        .chain([(4.0 * QUOTE_INDENT + LIST_INDENT) * em])
        .collect();
    assert_eq!(quote_bar_xs(c), expect);
}

/// Total text bytes of a block's spans.
fn block_bytes(doc: &Doc, block: &Block) -> usize {
    doc.block_spans(block)
        .iter()
        .map(|s| (s.end - s.start) as usize)
        .sum()
}

const BIG: usize = 2_500_000;

fn huge_code() -> String {
    format!("```\n{}\n```\n", "x ".repeat(BIG))
}

fn huge_paragraph() -> String {
    format!("{}\n", "word ".repeat(BIG / 2))
}

#[test]
fn huge_blocks_are_measured_in_bounded_chunks() {
    let fonts = Fonts::new().unwrap();
    for (name, src) in [("code", huge_code()), ("paragraph", huge_paragraph())] {
        let doc = Doc::parse(&src);
        assert!(
            doc.blocks.len() >= 300,
            "{name}: {} blocks",
            doc.blocks.len()
        );
        assert!(
            doc.blocks
                .iter()
                .all(|b| block_bytes(&doc, b) <= doc::MAX_BLOCK_BYTES)
        );
        let mut l = Layout::new(&doc, params(900.0, 1.0));
        // Every idle call with an expired deadline measures exactly one chunk, so one call
        // shapes at most MAX_BLOCK_BYTES of text however large the block is.
        for done in 1..=5 {
            assert!(!l.idle(&doc, &fonts, Instant::now()));
            assert_eq!(
                l.geoms().iter().filter(|g| g.measured).count(),
                done,
                "{name}"
            );
        }
        assert!(
            l.geoms()
                .iter()
                .filter(|g| g.measured)
                .all(|g| g.content.is_none())
        );
        // Viewing the middle builds only the chunks near it.
        let mid = l.anchor_at(l.total_height() / 2.0);
        l.ensure(&doc, &fonts, mid, 1000.0);
        let live = l.geoms().iter().filter(|g| g.content.is_some()).count();
        assert!((1..=8).contains(&live), "{name}: {live} chunks built");
        let at = mid.block as usize;
        assert!(l.geoms()[at].content.is_some());
        assert!(l.geoms()[0].content.is_none());
        check_geometry(&l);
    }
}

#[test]
fn chunk_estimates_match_measured_heights() {
    let fonts = Fonts::new().unwrap();
    // 4 chunks of code, plus a paragraph and a heading run, so every estimate path with
    // `joined` is exercised.
    let code: String = (0..12_000).map(|i| format!("l{}\n", i % 10)).collect();
    let words = "alpha beta gamma ".repeat(3000);
    let doc = Doc::parse(&format!("```\n{code}```\n\n# {words}\n\n{words}\n"));
    assert!(doc.blocks.iter().filter(|b| b.joined).count() >= 6);
    let est = Layout::new(&doc, params(900.0, 1.0));
    let est: Vec<f32> = est.geoms().iter().map(|g| g.h).collect();
    let l = complete(&doc, &fonts, params(900.0, 1.0));
    for (i, (e, g)) in est.iter().zip(l.geoms()).enumerate() {
        match doc.blocks[i].kind {
            // Code lines are exact, so are the pads on the run's outer edges.
            BlockKind::Code => assert!(
                (e - g.h).abs() < 1.0 + g.h * 1e-3,
                "block {i}: {e} vs {}",
                g.h
            ),
            _ => assert!(
                *e > 0.0 && g.h > 0.0 && e / g.h < 2.0 && g.h / e < 2.0,
                "block {i}"
            ),
        }
    }
}

#[test]
fn chunked_code_is_one_continuous_background() {
    let fonts = Fonts::new().unwrap();
    let code: String = (0..12_000).map(|i| format!("l{}\n", i % 10)).collect();
    let doc = Doc::parse(&format!("```\n{code}```\n\ntail\n"));
    let n = doc.blocks.len() - 1;
    assert!(n >= 3);
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 1.0e9);
    let pad = 16.0;
    for i in 0..n {
        let g = &l.geoms()[i];
        let c = g.content.as_ref().unwrap();
        let bg = c.rects.iter().find(|r| r.role == Role::CodeBg).unwrap();
        // No spacing between chunks; padding only above the first and below the last.
        assert_eq!(bg.y, 0.0);
        assert_eq!(bg.h, g.h, "chunk {i} background covers it");
        assert_eq!(bg.radius, 0.0);
        let top = c.lines.first().unwrap().top;
        assert_eq!(top, if i == 0 { pad } else { 0.0 }, "chunk {i}");
        let bottom = g.h - c.lines.last().unwrap().bottom;
        assert_eq!(bottom, if i == n - 1 { pad } else { 0.0 }, "chunk {i}");
        if i > 0 {
            assert_eq!(g.y, l.geoms()[i - 1].y + l.geoms()[i - 1].h);
        }
    }
    // A code block that fits in one block keeps its rounded corners and both pads.
    let doc = Doc::parse("```\nshort\n```\n");
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let bg = l.geoms()[0].content.as_ref().unwrap().rects[0];
    assert!(bg.radius > 0.0);
}

#[test]
fn chunked_quotes_and_headings_stay_whole() {
    let fonts = Fonts::new().unwrap();
    let words = "alpha beta gamma ".repeat(3000);
    let doc = Doc::parse(&format!("> {words}\n\n# {words}\n\nend\n"));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 1.0e9);
    let quote: Vec<usize> = (0..doc.blocks.len())
        .filter(|&i| doc.blocks[i].quote > 0)
        .collect();
    assert!(quote.len() >= 3);
    for &i in &quote {
        let (g, c) = (&l.geoms()[i], l.geoms()[i].content.as_ref().unwrap());
        // One bar per chunk, spanning it entirely: it joins its neighbors.
        let bars: Vec<_> = c
            .rects
            .iter()
            .filter(|r| r.role == Role::QuoteBar)
            .collect();
        assert_eq!(bars.len(), 1);
        assert_eq!((bars[0].y, bars[0].h), (0.0, g.h), "chunk {i}");
    }
    // The heading's rule is drawn once, under the last chunk.
    let heading: Vec<usize> = (0..doc.blocks.len())
        .filter(|&i| matches!(doc.blocks[i].kind, BlockKind::Heading(1)))
        .collect();
    assert!(heading.len() >= 3);
    for &i in &heading {
        let c = l.geoms()[i].content.as_ref().unwrap();
        let rules = c
            .rects
            .iter()
            .filter(|r| r.role == Role::BorderMuted)
            .count();
        assert_eq!(rules, (i == *heading.last().unwrap()) as usize, "chunk {i}");
    }
    // Chunks of a run have no gap between them.
    for &i in &heading[1..] {
        let prev = &l.geoms()[i - 1];
        assert_eq!(l.geoms()[i].y, prev.y + prev.h);
    }
}

#[test]
fn table_rows_with_over_65535_links_keep_them_across_fragments() {
    let fonts = Fonts::new().unwrap();
    const N: usize = 20_000;
    let cell = |from: usize| -> String {
        (from..from + N)
            .map(|i| format!("[a **b**](#{i}) "))
            .collect()
    };
    let doc = Doc::parse(&format!(
        "| h | h |\n|---|---|\n| {} | {} |\n",
        cell(0),
        cell(N)
    ));
    assert_eq!(doc.links.len(), 2 * N);
    // The row is split, so no block holds more styles than 16 KB of text can carry.
    assert!(doc.blocks.len() > 10);
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 1.0e9);
    let mut links = Vec::new();
    for (i, g) in l.geoms().iter().enumerate().skip(1) {
        let c = g.content.as_ref().unwrap();
        // Each fragment's links index its own styles, and point at the right document text.
        for h in &c.hits {
            let link = c.styles[h.style as usize].link;
            if link != 0 {
                assert!(
                    b"a b".contains(&doc.text.as_bytes()[h.doc as usize]),
                    "block {i}"
                );
                links.push(link);
            }
        }
    }
    links.sort_unstable();
    links.dedup();
    assert_eq!(links, (1..=2 * N as u32).collect::<Vec<_>>());
    assert_eq!(doc.links[2 * N - 1], format!("#{}", 2 * N - 1));
    // Hits resolve to the link under them, in the last fragment too.
    let last = l.geoms().len() - 1;
    let c = l.geoms()[last].content.as_ref().unwrap();
    let (hi, h) = c
        .hits
        .iter()
        .enumerate()
        .rev()
        .find(|(_, h)| c.styles[h.style as usize].link != 0)
        .unwrap();
    let line = c
        .lines
        .iter()
        .find(|l| l.hits.contains(&(hi as u32)))
        .unwrap();
    let (x, y) = (
        l.column().0 + (h.x0 + h.x1) / 2.0,
        l.geoms()[last].y + (line.top + line.bottom) / 2.0,
    );
    let hit = l.hit(x, y).expect("link under the cursor");
    assert_eq!(
        (hit.block as usize, hit.link),
        (last, c.styles[h.style as usize].link)
    );
}

#[test]
fn tables_past_65535_lay_out_all_cells() {
    let fonts = Fonts::new().unwrap();
    let mut src: String = (0..65_536).map(|_| "|a|\n|-|\n|b|\n\n").collect();
    src.push_str("|x|y|\n|-|-|\n|1|2|\n");
    let doc = Doc::parse(&src);
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    let last = doc.blocks.len() as u32 - 1;
    l.ensure(
        &doc,
        &fonts,
        Anchor {
            block: last,
            frac: 0.0,
        },
        800.0,
    );
    let c = l.geoms()[last as usize].content.as_ref().unwrap();
    // Both cells of the final two-column table are laid out ("1" and "2").
    let docs: Vec<u32> = c.hits.iter().map(|h| h.doc).collect();
    let (one, two) = (
        doc.text.rfind('1').unwrap() as u32,
        doc.text.rfind('2').unwrap() as u32,
    );
    assert!(docs.contains(&one) && docs.contains(&two), "{docs:?}");
}

/// Lines laid out across all blocks of a code run.
fn code_lines(fonts: &Fonts, code: &str) -> (usize, usize) {
    let doc = Doc::parse(&format!("```\n{code}\n```\n"));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, fonts, Anchor::TOP, 1.0e9);
    let lines = l
        .geoms()
        .iter()
        .map(|g| g.content.as_ref().unwrap().lines.len())
        .sum();
    (doc.blocks.len(), lines)
}

#[test]
fn chunk_boundaries_keep_every_line_and_blank_line() {
    let fonts = Fonts::new().unwrap();
    // The cut lands on the last newline within 16 KB: first the only one, then inside runs of
    // two and three newlines.
    let pairs = "a\n".repeat(8191);
    for (tail, lines) in [("a\nb", 8193), ("\n\nb", 8194), ("\n\n\nb", 8195)] {
        let code = format!("{pairs}{tail}");
        let (blocks, got) = code_lines(&fonts, &code);
        assert!(blocks > 1, "{tail:?}");
        let want = code.matches('\n').count() + 1;
        assert_eq!(want, lines);
        assert_eq!(got, lines, "{tail:?}");
    }
    // A chunk ending in a newline has no empty line after it, and the estimate agrees.
    let doc = Doc::parse(&format!("```\n{pairs}{}\n```\n", "x\n".repeat(100)));
    let est = Layout::new(&doc, params(900.0, 1.0));
    let l = complete(&doc, &fonts, params(900.0, 1.0));
    assert!(doc.blocks.len() > 1);
    for (i, (e, g)) in est.geoms().iter().zip(l.geoms()).enumerate() {
        // Within a fraction of one line (19.7 px): an extra or missing line would show.
        assert!((e.h - g.h).abs() < 5.0, "block {i}: {} vs {}", e.h, g.h);
    }
}

/// A table with one row whose second cell is `cell`.
fn table_with_cell(cell: &str) -> String {
    format!("| a | b |\n|---|---|\n| x | {cell} |\n")
}

#[test]
fn huge_table_cells_are_measured_in_bounded_row_fragments() {
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&table_with_cell(&"word ".repeat(BIG / 2)));
    assert!(doc.blocks.len() >= 300, "{} blocks", doc.blocks.len());
    assert!(
        doc.blocks
            .iter()
            .all(|b| block_bytes(&doc, b) <= doc::MAX_BLOCK_BYTES)
    );
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    // Every idle call with an expired deadline measures exactly one fragment (the first also
    // measures the sampled rows for the column widths).
    for done in 1..=5 {
        assert!(!l.idle(&doc, &fonts, Instant::now()));
        assert_eq!(l.geoms().iter().filter(|g| g.measured).count(), done);
    }
    // At the top, only the fragments near the viewport are built.
    l.ensure(&doc, &fonts, Anchor::TOP, 1000.0);
    let live = l.geoms().iter().filter(|g| g.content.is_some()).count();
    assert!((2..=8).contains(&live), "{live} fragments built");
    assert!(l.geoms().last().unwrap().content.is_none());
    // And so are the ones in the middle.
    let mid = l.anchor_at(l.total_height() / 2.0);
    l.ensure(&doc, &fonts, mid, 1000.0);
    let live = l.geoms().iter().filter(|g| g.content.is_some()).count();
    assert!((1..=8).contains(&live), "{live} fragments built");
    assert!(l.geoms()[mid.block as usize].content.is_some());
    check_geometry(&l);
}

/// Horizontal rules of a row block (full-width borders).
fn horizontal_rules(c: &BlockBox) -> usize {
    c.rects
        .iter()
        .filter(|r| r.role == Role::Border && r.h <= 1.0 && r.w > 1.0)
        .count()
}

#[test]
fn row_fragments_share_stripes_borders_and_parity() {
    let fonts = Fonts::new().unwrap();
    let words = "alpha beta gamma ".repeat(2500);
    let src =
        format!("| a | b |\n|---|---|\n| 1 | 2 |\n| x | {words} |\n| 3 | 4 |\n| y | {words} |\n");
    let doc = Doc::parse(&src);
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 1.0e9);
    let n = doc.blocks.len();
    assert!(doc.blocks.iter().filter(|b| b.joined).count() >= 4);
    let py = CELL_PAD_Y * 16.0;
    // Row index (header = 0) of every block; fragments of one row count once.
    let mut row = 0;
    for i in 0..n {
        let (b, g) = (&doc.blocks[i], &l.geoms()[i]);
        let c = g.content.as_ref().unwrap();
        let header = matches!(b.kind, BlockKind::TableRow { header: true, .. });
        if !b.joined && !header {
            row += 1;
        }
        let striped = c.rects.iter().any(|r| r.role == Role::TableStripe);
        assert_eq!(striped, row % 2 == 0 && !header, "block {i} (row {row})");
        let last = i + 1 == n || !doc.blocks[i + 1].joined;
        // Rules: the table's top above the header only, a bottom rule under each row's last
        // fragment only, and the vertical rules run the whole fragment.
        let want = (i == 0) as usize + last as usize;
        assert_eq!(horizontal_rules(c), want, "block {i}");
        let verticals: Vec<_> = c
            .rects
            .iter()
            .filter(|r| r.role == Role::Border && r.w <= 1.0 && r.h > 1.0)
            .collect();
        assert_eq!(verticals.len(), 3, "block {i}");
        assert!(
            verticals.iter().all(|r| r.y == 0.0 && r.h == g.h),
            "block {i}"
        );
        if let Some(s) = c.rects.iter().find(|r| r.role == Role::TableStripe) {
            assert_eq!((s.y, s.h), (0.0, g.h), "block {i}");
        }
        // Padding only above the row's first fragment and below its last.
        let top = c.lines.iter().map(|l| l.top).fold(f32::MAX, f32::min);
        assert_eq!(top, if b.joined { 0.0 } else { py }, "block {i}");
        let bottom = g.h - c.lines.iter().map(|l| l.bottom).fold(0.0, f32::max);
        let want = if last { py } else { 0.0 };
        assert!(
            (bottom - want).abs() < 1e-3,
            "block {i}: {bottom} vs {want}"
        );
        assert!(g.h > py);
    }
    // Fragments counted as one row each: four data rows.
    assert_eq!(row, 4);
}

#[test]
fn row_fragment_estimates_match_measured_heights() {
    let fonts = Fonts::new().unwrap();
    let words = "alpha beta gamma ".repeat(3000);
    let doc = Doc::parse(&format!(
        "| a | b |\n|---|---|\n| 1 | 2 |\n| x | {words} |\n| 3 | 4 |\n"
    ));
    assert!(doc.blocks.iter().filter(|b| b.joined).count() >= 4);
    let est = Layout::new(&doc, params(900.0, 1.0));
    let est: Vec<f32> = est.geoms().iter().map(|g| g.h).collect();
    let l = complete(&doc, &fonts, params(900.0, 1.0));
    for (i, (e, g)) in est.iter().zip(l.geoms()).enumerate() {
        assert!(
            *e > 0.0 && g.h > 0.0 && e / g.h < 2.0 && g.h / e < 2.0,
            "block {i}: {e} vs {}",
            g.h
        );
    }
    // Short rows keep their exact one-line estimate.
    assert!((est[1] - l.geoms()[1].h).abs() < 1e-3);
}

/// `rows` short data rows after a header; row `at` (if any) has `wide` in its second cell.
fn long_table(rows: usize, at: usize, wide: &str) -> String {
    let mut src = String::from("| key | value |\n|---|---|\n");
    for r in 0..rows {
        let cell = if r == at { wide } else { "v" };
        src.push_str(&format!("| k{r} | {cell} |\n"));
    }
    src
}

#[test]
fn table_columns_are_sized_from_the_first_rows_only() {
    let fonts = Fonts::new().unwrap();
    // 96k rows: the sample is a small prefix, so the first frame doesn't shape every row.
    let doc = Doc::parse(&long_table(96_000, usize::MAX, ""));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    let sample = l.table_sample(&doc, 0);
    assert_eq!(sample.start, 0);
    assert!(sample.len() <= TABLE_SAMPLE_ROWS, "{sample:?}");
    assert!(sample.len() > 1);
    l.ensure(&doc, &fonts, Anchor::TOP, 1000.0);
    assert!(l.table_cols[0].is_some());
    // A table of text-heavy rows is bounded by bytes instead.
    let words = "alpha ".repeat(1000);
    let heavy = Doc::parse(&format!(
        "| a |\n|---|\n{}",
        (0..200)
            .map(|_| format!("| {words} |\n"))
            .collect::<String>()
    ));
    let l = Layout::new(&heavy, params(900.0, 1.0));
    let bytes: usize = l
        .table_sample(&heavy, 0)
        .map(|r| text_bytes(&heavy, &heavy.blocks[r as usize]))
        .sum();
    assert!(
        bytes < TABLE_SAMPLE_BYTES + 6000 + 1,
        "{bytes} bytes sampled"
    );
    assert!(l.table_sample(&heavy, 0).len() < 20);

    // A cell far past the sample doesn't change the columns: the table sizes like one that
    // stops at the sample.
    let wide = "wideword".repeat(12);
    let long = Doc::parse(&long_table(1000, 600, &wide));
    let short = Doc::parse(&long_table(300, usize::MAX, ""));
    let cols = |doc: &Doc| {
        let mut l = Layout::new(doc, params(900.0, 1.0));
        l.ensure(doc, &fonts, Anchor::TOP, 800.0);
        l.table_cols[0].clone().unwrap()
    };
    assert_eq!(cols(&long), cols(&short));

    // The wide row still lays out completely, wrapped inside the sampled columns.
    let mut l = Layout::new(&long, params(900.0, 1.0));
    let row = 1 + 600;
    let widths = cols(&long);
    l.ensure(
        &long,
        &fonts,
        Anchor {
            block: row,
            frac: 0.0,
        },
        800.0,
    );
    let c = l.geoms()[row as usize].content.as_ref().unwrap();
    assert!(c.lines.len() > 1, "the cell wraps");
    let start = long.text.find(&wide).unwrap() as u32;
    let mut seen: Vec<u32> = c
        .hits
        .iter()
        .map(|h| h.doc)
        .filter(|d| (start..start + wide.len() as u32).contains(d))
        .collect();
    seen.sort_unstable();
    seen.dedup();
    assert!(
        seen.len() >= wide.len() - 4,
        "{} of {}",
        seen.len(),
        wide.len()
    );
    assert_eq!(seen[0], start);
    let right = widths[0] + widths[1] - CELL_PAD_X * 16.0;
    for g in &c.glyphs {
        assert!(
            g.x <= right + 1.0,
            "glyph at {} past column edge {right}",
            g.x
        );
    }
}

#[test]
fn rows_past_the_sample_get_slack_instead_of_wrapping() {
    let fonts = Fonts::new().unwrap();
    // The sampled rows are narrow; a later row is a bit wider than any of them.
    let doc = Doc::parse(&long_table(1000, 600, "vvvvvvvvvv"));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    let row = 1 + 600;
    l.ensure(
        &doc,
        &fonts,
        Anchor {
            block: row,
            frac: 0.0,
        },
        800.0,
    );
    let c = l.geoms()[row as usize].content.as_ref().unwrap();
    // One line per cell: neither wrapped.
    assert_eq!(c.lines.len(), 2, "fits on one line");
    let (_, col_w) = l.column();
    let total: f32 = l.table_cols[0].as_ref().unwrap().iter().sum();
    assert!(
        total <= col_w + 2.0 && total > col_w * 0.9,
        "{total} of {col_w}"
    );
    // A table that ends inside the sample keeps its natural widths.
    let doc = Doc::parse(&long_table(10, usize::MAX, ""));
    let mut l = Layout::new(&doc, params(900.0, 1.0));
    l.ensure(&doc, &fonts, Anchor::TOP, 800.0);
    let total: f32 = l.table_cols[0].as_ref().unwrap().iter().sum();
    assert!(total < col_w * 0.5, "{total}");
}
