use super::*;
use crate::fonts::Fonts;
use crate::text::fallback;

fn style(size: f32) -> TextStyle {
    TextStyle {
        bold: false,
        italic: false,
        mono: false,
        size,
        role: Role::Text,
        deco: 0,
        shift: 0.0,
        link: 0,
    }
}

fn para(text: &str) -> Para {
    let mut p = Para::default();
    p.push_text(text, 0, style(16.0));
    p
}

fn lay(text: &str, width: f32) -> ParaLayout {
    let fonts = Fonts::new().unwrap();
    TextCtx::default().layout(&fonts, &para(text), width, Align::Start, 1.5)
}

/// Doc offsets of a line's clusters in visual (left-to-right) order.
fn visual_docs(l: &ParaLayout, line: usize) -> Vec<u32> {
    let r = l.lines[line].hits.clone();
    l.hits[r.start as usize..r.end as usize]
        .iter()
        .map(|h| h.doc)
        .collect()
}

#[test]
fn wraps_within_width() {
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(8);
    for width in [120.0, 300.0, 640.0] {
        let l = lay(&text, width);
        assert!(l.lines.len() > 1);
        for line in &l.lines {
            assert!(line.width <= width + 0.01, "{} > {width}", line.width);
        }
        // Lines stack without gaps or overlap.
        for w in l.lines.windows(2) {
            assert!((w[0].bottom - w[1].top).abs() < 1e-3);
        }
        assert!((l.height - l.lines.last().unwrap().bottom).abs() < 1e-3);
        // Every line after the first starts after a soft wrap.
        assert!(l.lines[1..].iter().all(|l| l.cont));
    }
}

#[test]
fn line_height_is_size_times_factor() {
    let l = lay("Hello", 500.0);
    assert_eq!(l.lines.len(), 1);
    let h = l.lines[0].bottom - l.lines[0].top;
    assert!((h - 24.0).abs() < 0.5, "line height {h}");
}

#[test]
fn hard_breaks() {
    let l = lay("first\nsecond", 1000.0);
    assert_eq!(l.lines.len(), 2);
    assert!(!l.lines[1].cont);
    assert_eq!(visual_docs(&l, 1)[0], 6);
}

#[test]
fn emergency_break_long_word() {
    let l = lay(&"x".repeat(200), 100.0);
    assert!(l.lines.len() > 5);
    for line in &l.lines {
        assert!(line.width <= 100.01 && line.width > 0.0);
    }
}

#[test]
fn cjk_breaks_between_ideographs() {
    // Ideographs have break opportunities between them: no emergency splitting needed, and
    // every character is still laid out once.
    let text = "中文中文中文中文中文中文中文中文";
    let l = lay(text, 60.0);
    assert!(l.lines.len() > 1);
    let total: usize = l
        .lines
        .iter()
        .map(|ln| (ln.hits.end - ln.hits.start) as usize)
        .sum();
    assert_eq!(total, text.chars().count());
    assert_eq!(l.missing, vec!['中', '文']);
}

#[test]
fn rtl_reorders_visually() {
    // Hebrew renders right-to-left even with .notdef glyphs: order comes from bidi levels.
    let text = "abc שלום def";
    let l = lay(text, 1000.0);
    let docs = visual_docs(&l, 0);
    let heb: Vec<u32> = text
        .char_indices()
        .filter(|(_, c)| ('\u{5d0}'..='\u{5ea}').contains(c))
        .map(|(i, _)| i as u32)
        .collect();
    let pos = |d: u32| docs.iter().position(|&x| x == d).unwrap();
    // Latin stays left-to-right at both ends.
    assert!(pos(0) < pos(1) && pos(1) < pos(2));
    assert!(
        pos(*heb.last().unwrap()) < pos(heb[0]),
        "Hebrew must be reversed"
    );
    assert!(pos(2) < pos(*heb.last().unwrap()));
    assert!(pos(heb[0]) < pos(text.find("def").unwrap() as u32));
    // Hits are monotonic in x.
    let r = l.lines[0].hits.clone();
    let hits = &l.hits[r.start as usize..r.end as usize];
    assert!(hits.windows(2).all(|w| w[0].x1 <= w[1].x0 + 1e-3));
}

#[test]
fn rtl_paragraph_aligns_right() {
    let l = lay("שלום עולם", 500.0);
    let r = l.lines[0].hits.clone();
    let last = l.hits[r.end as usize - 1];
    assert!((last.x1 - 500.0).abs() < 0.5, "right edge {}", last.x1);
}

#[test]
fn inline_boxes() {
    let fonts = Fonts::new().unwrap();
    let mut p = para("before ");
    let b = InlineBox {
        width: 40.0,
        ascent: 50.0,
        descent: 10.0,
        id: 7,
    };
    p.push_box(b, 7, style(16.0));
    p.push_text(" after", 8, style(16.0));
    let l = TextCtx::default().layout(&fonts, &p, 1000.0, Align::Start, 1.5);
    assert_eq!(l.lines.len(), 1);
    assert_eq!(l.boxes.len(), 1);
    let pb = l.boxes[0];
    assert_eq!(pb.b.id, 7);
    // The box sits on the baseline and makes the line tall enough.
    assert!((pb.y + 50.0 - l.lines[0].baseline).abs() < 1e-3);
    assert!(l.lines[0].bottom - l.lines[0].top >= 60.0);
    assert!(pb.x > 0.0);
}

#[test]
fn alignment() {
    let fonts = Fonts::new().unwrap();
    let mut ctx = TextCtx::default();
    let p = para("mid");
    let left = ctx.layout(&fonts, &p, 300.0, Align::Start, 1.5);
    let center = ctx.layout(&fonts, &p, 300.0, Align::Center, 1.5);
    let right = ctx.layout(&fonts, &p, 300.0, Align::Right, 1.5);
    let w = left.width;
    assert!((center.hits[0].x0 - (300.0 - w) / 2.0).abs() < 0.01);
    assert!((right.hits[0].x0 - (300.0 - w)).abs() < 0.01);
}

#[test]
fn measure_min_max() {
    let fonts = Fonts::new().unwrap();
    let mut ctx = TextCtx::default();
    let (min, max) = ctx.measure(&fonts, &para("a extraordinarily b"));
    let (word, _) = ctx.measure(&fonts, &para("extraordinarily"));
    assert!((min - word).abs() < 0.01);
    assert!(max > min);
    let (_, natural) = ctx.measure(&fonts, &para("short\nlonger line here"));
    let (_, second) = ctx.measure(&fonts, &para("longer line here"));
    assert!((natural - second).abs() < 0.01);
}

#[test]
fn doc_offsets_map_through_items() {
    let mut p = Para::default();
    p.push_text("ab", 100, style(16.0));
    p.push_synthetic("    ", 102, style(16.0));
    p.push_text("cd", 103, style(16.0));
    assert_eq!(p.doc_offset(0), 100);
    assert_eq!(p.doc_offset(1), 101);
    assert_eq!(p.doc_offset(4), 102);
    assert_eq!(p.doc_offset(6), 103);
    assert_eq!(p.doc_offset(7), 104);
    // Identical styles share one entry.
    assert_eq!(p.styles.len(), 1);
}

#[test]
fn decorations() {
    let fonts = Fonts::new().unwrap();
    let mut p = Para::default();
    p.push_text("a ", 0, style(16.0));
    p.push_text(
        "code",
        2,
        TextStyle {
            mono: true,
            deco: deco::CODE_BG,
            ..style(14.0)
        },
    );
    p.push_text(
        " gone",
        6,
        TextStyle {
            deco: deco::STRIKE,
            ..style(16.0)
        },
    );
    let l = TextCtx::default().layout(&fonts, &p, 1000.0, Align::Start, 1.5);
    assert_eq!(l.rects.len(), 2);
    assert_eq!(l.rects[0].role, Role::CodeBg);
    assert!(l.rects[1].h >= 1.0 && l.rects[1].y < l.lines[0].baseline);
}

#[test]
fn emoji_sequences_stay_together() {
    // ZWJ family + skin tone: the joiners/modifiers inherit the face of the base character,
    // so the sequence isn't split across faces.
    let text = "👨‍👩‍👧 👋🏽";
    let base = Fonts::new().unwrap();
    let shaped = TextCtx::default().shape(&base, &para(text));
    assert!(!shaped.missing.contains(&'\u{200D}'));
    assert!(!shaped.missing.contains(&'\u{1F3FD}'));

    // The embedded fonts have no emoji, so check the rule against system fallbacks.
    let found = fallback::find_fallbacks(&shaped.missing, &|p, i| base.has_source(p, i));
    if found.is_empty() {
        return;
    }
    let mut fonts = Fonts::new().unwrap();
    for f in &found {
        fonts.add_fallback(&f.path, f.index);
    }
    let shaped = TextCtx::default().shape(&fonts, &para(text));
    let face_at = |byte: usize| {
        let c = shaped
            .clusters
            .iter()
            .find(|c| (c.start as usize..c.end as usize).contains(&byte))
            .expect("every byte is in a cluster");
        c.face
    };
    let mut checked = 0;
    for (i, c) in text.char_indices() {
        if matches!(c, '\u{200D}' | '\u{1F3FD}') {
            let prev = text[..i].char_indices().next_back().unwrap().0;
            assert_eq!(
                face_at(i),
                face_at(prev),
                "{c:?} at {i} left its base's face"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 3);
}

#[test]
fn default_ignorables_are_invisible() {
    let text = "a\u{200F}b\u{00AD}c\u{FEFF}";
    let fonts = Fonts::new().unwrap();
    let shaped = TextCtx::default().shape(&fonts, &para(text));
    assert!(shaped.missing.is_empty(), "{:?}", shaped.missing);
    for c in &shaped.clusters {
        let src = &text[c.start as usize..c.end as usize];
        if src.chars().all(|c| c.is_alphanumeric()) {
            continue;
        }
        for g in &shaped.glyphs[c.glyphs.start as usize..c.glyphs.end as usize] {
            assert!(g.id != 0 || g.advance == 0.0, "{src:?} drew tofu");
        }
        assert_eq!(c.advance, 0.0, "{src:?} takes space");
    }
    // Only the three letters are drawn.
    let l = TextCtx::default().layout(&fonts, &para(text), 1000.0, Align::Start, 1.5);
    assert_eq!(l.glyphs.len(), 3);
}

#[test]
fn empty_para_has_one_line() {
    let fonts = Fonts::new().unwrap();
    let mut p = Para::default();
    p.styles.push(style(16.0));
    let l = TextCtx::default().layout(&fonts, &p, 100.0, Align::Start, 1.5);
    assert_eq!(l.lines.len(), 1);
    assert!(l.height > 0.0);
}

#[test]
fn deterministic() {
    let text = "Mixed text with **stuff**, 中文, שלום and emoji 🎉 wrapping around. ".repeat(5);
    assert_eq!(lay(&text, 333.0), lay(&text, 333.0));
}

#[test]
fn many_distinct_styles_are_deduplicated() {
    let mut p = Para::default();
    for round in 0..2 {
        for link in 0..20_000u32 {
            p.push_text(
                "a",
                0,
                TextStyle {
                    link,
                    ..style(16.0)
                },
            );
            let expect = if round == 0 {
                link as usize + 1
            } else {
                20_000
            };
            assert_eq!(p.styles.len(), expect);
        }
    }
    assert_eq!(p.items.len(), 40_000);
    assert!(
        p.items[20_000..]
            .iter()
            .enumerate()
            .all(|(i, it)| it.style as usize == i)
    );
    // Styles pushed directly are picked up by the index too.
    p.styles.push(style(99.0));
    p.push_text("b", 0, style(99.0));
    assert_eq!(p.styles.len(), 20_001);
    assert_eq!(p.items.last().unwrap().style, 20_000);
}

#[test]
fn style_indices_are_not_capped_at_16_bits() {
    const N: u32 = 70_000;
    let mut p = Para::default();
    for link in 0..N {
        p.push_text(
            "a ",
            link * 2,
            TextStyle {
                link: link + 1,
                ..style(16.0)
            },
        );
    }
    assert_eq!(p.styles.len(), N as usize);
    assert_eq!(p.items.last().unwrap().style, N - 1);
    let fonts = Fonts::new().unwrap();
    let shaped = TextCtx::default().shape(&fonts, &p);
    assert_eq!(shaped.clusters.last().unwrap().style, N - 1);
    let l = TextCtx::default().layout(&fonts, &p, 100_000.0, Align::Start, 1.5);
    // Every glyph and hit still resolves to its own style, not one wrapped past 65,535.
    let last = l.hits.last().unwrap();
    assert_eq!(p.styles[last.style as usize].link, N);
    assert_eq!(l.glyphs.last().unwrap().style, N - 1);
    for h in l.hits.iter().step_by(997) {
        assert_eq!(p.styles[h.style_index()].link, h.doc / 2 + 1);
    }
}

/// `text` shaped with `fonts`; every cluster's face and glyph ids.
fn shape_with(fonts: &Fonts, text: &str) -> Shaped {
    TextCtx::default().shape(fonts, &para(text))
}

#[test]
fn uncovered_combining_marks_are_reported_missing() {
    let fonts = Fonts::new().unwrap();
    // The embedded subset has no U+0301 glyph, and `q` + acute doesn't compose.
    let shaped = shape_with(&fonts, "q\u{0301}");
    assert_eq!(shaped.missing, vec!['\u{0301}']);
    assert!(shaped.glyphs.iter().any(|g| g.id == 0));
    // `e` + acute composes into `é`, which the face has: no fallback needed.
    let shaped = shape_with(&fonts, "e\u{0301}");
    assert!(shaped.missing.is_empty(), "{:?}", shaped.missing);
    assert!(shaped.glyphs.iter().all(|g| g.id != 0));
    assert_eq!(shaped.clusters[0].face, fonts.primary(false, false, false));
    // Several marks on one base: only the ones nothing draws are reported.
    let shaped = shape_with(&fonts, "e\u{0301}\u{0323}q\u{0308}");
    assert!(shaped.missing.contains(&'\u{0308}'));
    assert!(!shaped.missing.contains(&'q'));
}

#[test]
fn combining_marks_use_a_fallback_with_their_base() {
    let base = Fonts::new().unwrap();
    let found = fallback::find_fallbacks(&['\u{0301}'], &|p, i| base.has_source(p, i));
    let mut fonts = Fonts::new().unwrap();
    let ids: Vec<_> = found
        .iter()
        .filter_map(|f| fonts.add_fallback(&f.path, f.index))
        .collect();
    // Needs an installed face that has the base letter as well as the mark.
    let Some(&face) = ids.iter().find(|&&id| fonts.face(id).glyph('q') != 0) else {
        return;
    };
    let shaped = shape_with(&fonts, "q\u{0301}x");
    assert!(shaped.missing.is_empty(), "{:?}", shaped.missing);
    assert!(
        shaped.glyphs.iter().all(|g| g.id != 0),
        "{:?}",
        shaped.glyphs
    );
    // The base moved to the fallback together with its mark; the next letter stays on Inter.
    let cluster = &shaped.clusters[0];
    assert_eq!((cluster.start, cluster.end), (0, 3));
    assert_eq!(cluster.face, face);
    assert_eq!(
        shaped.clusters.last().unwrap().face,
        fonts.primary(false, false, false)
    );
    // A sequence the primary face composes by itself stays put.
    let shaped = shape_with(&fonts, "e\u{0301}");
    assert_eq!(shaped.clusters[0].face, fonts.primary(false, false, false));
}

#[test]
fn marks_after_cgj_and_whitespace_are_checked_for_coverage() {
    let fonts = Fonts::new().unwrap();
    // CGJ stops composition, and a space has nothing to compose with: Inter has no U+0301
    // glyph, so the mark is missing either way (not silently drawn as .notdef).
    for text in [
        "q\u{034F}\u{0301}",
        " \u{0301}",
        "e\u{034F}\u{0301}",
        "a\u{200B}\u{0301}",
    ] {
        let shaped = shape_with(&fonts, text);
        assert_eq!(shaped.missing, vec!['\u{0301}'], "{text:?}");
    }
    // Marks after a control character have no base to inherit a face from either.
    let shaped = shape_with(&fonts, "\n\u{0301}");
    assert_eq!(shaped.missing, vec!['\u{0301}']);
    // The invisible character itself is never reported, and a composable sequence still is
    // not: `e` + acute composes into `é`.
    let shaped = shape_with(&fonts, "e\u{0301}x\u{034F}");
    assert!(shaped.missing.is_empty(), "{:?}", shaped.missing);
    // Neither do joiners and variation selectors compose across: the mark needs its own glyph.
    for text in [
        "e\u{200D}\u{0301}",
        "e\u{FE0F}\u{0301}",
        "e\u{200C}\u{0301}",
    ] {
        let shaped = shape_with(&fonts, text);
        assert_eq!(shaped.missing, vec!['\u{0301}'], "{text:?}");
    }
}

#[test]
fn marks_after_cgj_and_whitespace_use_a_fallback() {
    let base = Fonts::new().unwrap();
    let found = fallback::find_fallbacks(&['\u{0301}'], &|p, i| base.has_source(p, i));
    let mut fonts = Fonts::new().unwrap();
    let ids: Vec<_> = found
        .iter()
        .filter_map(|f| fonts.add_fallback(&f.path, f.index))
        .collect();
    let covering = |ch: char| {
        ids.iter()
            .copied()
            .find(|&id| fonts.face(id).glyph(ch) != 0)
    };
    let (q, space) = (covering('q'), covering(' '));
    for (text, face) in [("q\u{034F}\u{0301}", q), (" \u{0301}", space)] {
        // Needs an installed face with the mark and the base.
        let Some(face) = face.filter(|&id| fonts.face(id).glyph('\u{0301}') != 0) else {
            continue;
        };
        let shaped = shape_with(&fonts, text);
        assert!(shaped.missing.is_empty(), "{text:?}: {:?}", shaped.missing);
        assert!(
            shaped.glyphs.iter().all(|g| g.id != 0),
            "{text:?}: {:?}",
            shaped.glyphs
        );
        // The base moved with its mark.
        assert_eq!(shaped.clusters[0].face, face, "{text:?}");
    }
}

#[test]
fn marks_after_joiners_and_selectors_use_a_fallback() {
    let base = Fonts::new().unwrap();
    let found = fallback::find_fallbacks(&['\u{0301}'], &|p, i| base.has_source(p, i));
    let mut fonts = Fonts::new().unwrap();
    let ids: Vec<_> = found
        .iter()
        .filter_map(|f| fonts.add_fallback(&f.path, f.index))
        .collect();
    // Needs an installed face with both the base letter and the mark.
    let Some(&face) = ids
        .iter()
        .find(|&&id| fonts.face(id).glyph('e') != 0 && fonts.face(id).glyph('\u{0301}') != 0)
    else {
        return;
    };
    for text in ["e\u{200D}\u{0301}", "e\u{FE0F}\u{0301}"] {
        let shaped = shape_with(&fonts, text);
        assert!(shaped.missing.is_empty(), "{text:?}: {:?}", shaped.missing);
        assert!(
            shaped.glyphs.iter().all(|g| g.id != 0),
            "{text:?}: {:?}",
            shaped.glyphs
        );
        // Inter composes nothing here, so the whole sequence moved to the fallback.
        assert!(
            shaped.clusters.iter().all(|c| c.face == face),
            "{text:?}: {:?}",
            shaped.clusters
        );
    }
}

/// Makes an item's style from the base style.
type Restyle<'a> = &'a dyn Fn(&mut TextStyle);

/// A paragraph of text items, each with the style `f` makes from the base style.
fn items_para(items: &[(&str, Restyle)]) -> Para {
    let mut p = Para::default();
    let mut doc = 0;
    for (text, f) in items {
        let mut s = style(16.0);
        f(&mut s);
        p.push_text(text, doc, s);
        doc += text.len() as u32;
    }
    p
}

#[test]
fn items_that_differ_only_in_color_link_or_decoration_share_a_run() {
    let fonts = Fonts::new().unwrap();
    let mut ctx = TextCtx::default();
    let link = |s: &mut TextStyle| {
        s.link = 1;
        s.role = Role::Link;
    };
    let strike = |s: &mut TextStyle| s.deco = deco::STRIKE;
    let plain = |_: &mut TextStyle| {};
    let p = items_para(&[("ab", &plain), ("cd", &link), ("ef", &strike)]);
    let whole = ctx.shape(&fonts, &para("abcdef"));
    let shaped = ctx.shape(&fonts, &p);
    assert_eq!(shaped.runs, 1);
    assert_eq!(whole.runs, 1);
    // Same glyphs, and every cluster has the style of its own item.
    let ids = |s: &Shaped| s.glyphs.iter().map(|g| g.id).collect::<Vec<_>>();
    assert_eq!(ids(&shaped), ids(&whole));
    let styles: Vec<u32> = shaped.clusters.iter().map(|c| c.style).collect();
    let items: Vec<u32> = (0..6).map(|i| p.items[i / 2].style).collect();
    assert_eq!(styles, items);
    assert_eq!(p.styles[styles[2] as usize].link, 1);
    // A different size, face (bold), an inline box or synthesized text each end the run.
    let big = |s: &mut TextStyle| s.size = 24.0;
    let bold = |s: &mut TextStyle| s.bold = true;
    for f in [&big as &dyn Fn(&mut TextStyle), &bold] {
        let p = items_para(&[("ab", &plain), ("cd", f), ("ef", &plain)]);
        assert_eq!(ctx.shape(&fonts, &p).runs, 3);
    }
    let mut p = items_para(&[("ab", &plain)]);
    p.push_box(
        InlineBox {
            width: 8.0,
            ascent: 8.0,
            descent: 0.0,
            id: 0,
        },
        2,
        style(16.0),
    );
    p.push_text("cd", 3, style(16.0));
    assert_eq!(ctx.shape(&fonts, &p).runs, 3);
    let mut p = items_para(&[("ab", &plain)]);
    p.push_synthetic("    ", 2, style(16.0));
    p.push_text("cd", 3, style(16.0));
    assert_eq!(ctx.shape(&fonts, &p).runs, 3);
}

#[test]
fn arabic_keeps_joining_across_style_boundaries() {
    let word = "سلام";
    let base = Fonts::new().unwrap();
    let chars: Vec<char> = word.chars().collect();
    let found = fallback::find_fallbacks(&chars, &|p, i| base.has_source(p, i));
    let mut fonts = Fonts::new().unwrap();
    for f in &found {
        fonts.add_fallback(&f.path, f.index);
    }
    let mut ctx = TextCtx::default();
    // The fallback the system picks, then every Arabic face on its own: some draw lam + alef
    // as one ligature cluster (DejaVu), others as two.
    arabic_joining(&mut ctx, &fonts, word);
    for fonts in arabic_fonts() {
        arabic_joining(&mut ctx, &fonts, word);
    }
}

fn arabic_joining(ctx: &mut TextCtx, fonts: &Fonts, word: &str) {
    let plain = ctx.shape(fonts, &para(word));
    // Needs an installed face that draws all of it.
    if !plain.missing.is_empty() || plain.glyphs.iter().any(|g| g.id == 0) {
        return;
    }
    let ids = |s: &Shaped| s.glyphs.iter().map(|g| g.id).collect::<Vec<_>>();
    let nolink = |_: &mut TextStyle| {};
    let link = |s: &mut TextStyle| {
        s.link = 1;
        s.role = Role::Link;
    };
    let colored = |s: &mut TextStyle| s.role = Role::Link;
    for split in [2, 4, 6] {
        let p = items_para(&[
            (&word[..split], &nolink as &dyn Fn(&mut TextStyle)),
            (&word[split..], &link),
        ]);
        let shaped = ctx.shape(fonts, &p);
        assert_eq!(ids(&shaped), ids(&plain), "split at {split}");
        assert_eq!(shaped.runs, 1, "split at {split}");
        // Each cluster has the style of the item it starts in: the link covers the tail only.
        for c in &shaped.clusters {
            let linked = p.styles[c.style as usize].link;
            assert_eq!(linked == 1, c.start as usize >= split, "{c:?}");
        }
        assert!(shaped.clusters.iter().all(|c| c.level == 1));
    }
    // Three items, the middle one differently colored.
    let p = items_para(&[
        (&word[..2], &nolink as &dyn Fn(&mut TextStyle)),
        (&word[2..6], &colored),
        (&word[6..], &nolink),
    ]);
    assert_eq!(ids(&ctx.shape(fonts, &p)), ids(&plain));
    // The hit map still resolves each cluster to the right document offset: where the plain
    // word's clusters start, so [0, 2, 6] in a face that ligates lam + alef.
    let l = ctx.layout(fonts, &p, 1000.0, Align::Start, 1.5);
    let mut docs = visual_docs(&l, 0);
    docs.sort_unstable();
    let mut starts: Vec<u32> = plain.clusters.iter().map(|c| c.start).collect();
    starts.sort_unstable();
    assert_eq!(docs, starts);
}

/// One `Fonts` per installed Arabic-capable file (DejaVu, Noto Naskh/Sans Arabic, ...), each
/// with just that face added as a fallback. A few at most.
fn arabic_fonts() -> Vec<Fonts> {
    let named = |p: &std::path::PathBuf| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase());
        n.is_some_and(|n| {
            ["arabic", "naskh", "dejavusans."]
                .iter()
                .any(|k| n.contains(k))
        })
    };
    fallback::list_files(&fallback::font_dirs())
        .iter()
        .filter(|p| named(p))
        .take(24)
        .filter_map(|p| {
            let mut fonts = Fonts::new().unwrap();
            fonts.add_fallback(p, 0)?;
            Some(fonts)
        })
        .collect()
}

#[test]
fn ligatures_across_link_boundaries_keep_every_link_hittable() {
    // Lam + alef is one mandatory ligature glyph in the Arabic faces that have it.
    let mut ctx = TextCtx::default();
    for fonts in arabic_fonts() {
        let plain = ctx.shape(&fonts, &para("لا"));
        // Needs a face that ligates the pair into one cluster.
        if !plain.missing.is_empty() || plain.clusters.len() != 1 || plain.glyphs.len() != 1 {
            continue;
        }
        lam_alef_links(&mut ctx, &fonts);
    }
}

fn lam_alef_links(ctx: &mut TextCtx, fonts: &Fonts) {
    let link = |n: u32| {
        move |s: &mut TextStyle| {
            s.link = n;
            s.role = Role::Link;
        }
    };
    let nolink = |_: &mut TextStyle| {};
    let (l1, l2) = (link(1), link(2));
    let hit_of = |l: &ParaLayout, n: u32, p: &Para| {
        let h: Vec<&PHit> = l
            .hits
            .iter()
            .filter(|h| p.styles[h.style_index()].link == n)
            .collect();
        assert_eq!(h.len(), 1, "link {n}: {:?}", l.hits);
        *h[0]
    };
    // Linked tail: the unlinked first item owns the cluster's glyph, but the link still has a
    // hit of its own.
    let p = items_para(&[("ل", &nolink as &dyn Fn(&mut TextStyle)), ("ا", &l1)]);
    let shaped = ctx.shape(fonts, &p);
    assert_eq!(shaped.runs, 1);
    assert_eq!(shaped.clusters.len(), 1, "the pair must stay one cluster");
    assert_eq!(shaped.clusters[0].segs, 0..2);
    let l = ctx.layout(fonts, &p, 1000.0, Align::Start, 1.5);
    let (tail, head) = (hit_of(&l, 1, &p), hit_of(&l, 0, &p));
    assert!(tail.x1 > tail.x0 && head.x1 > head.x0, "{:?}", l.hits);
    // RTL: the later letter is on the left.
    assert_eq!(tail.x1, head.x0);
    assert_eq!((tail.doc, head.doc), (2, 0));
    // Two links: both activate, each over its own half, in visual order.
    let p = items_para(&[("ل", &l1 as &dyn Fn(&mut TextStyle)), ("ا", &l2)]);
    let l = ctx.layout(fonts, &p, 1000.0, Align::Start, 1.5);
    let (second, first) = (hit_of(&l, 2, &p), hit_of(&l, 1, &p));
    assert!(second.x1 > second.x0 && first.x1 > first.x0, "{:?}", l.hits);
    assert_eq!(second.x1, first.x0);
    assert_eq!((second.doc, first.doc), (2, 0));
    // The glyph itself is drawn once, in the first segment's style.
    assert_eq!(l.glyphs.len(), 1);
    assert_eq!(p.styles[l.glyphs[0].style as usize].link, 1);
}
