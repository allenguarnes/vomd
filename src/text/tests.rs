use super::*;
use crate::fonts::Fonts;

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
    let fonts = Fonts::new().unwrap();
    let shaped = TextCtx::default().shape(&fonts, &para("👨‍👩‍👧 👋🏽"));
    let faces: Vec<_> = shaped.clusters.iter().map(|c| c.face).collect();
    assert!(faces.windows(2).all(|w| w[0] == w[1]) || faces.len() <= 3);
    assert!(!shaped.missing.contains(&'\u{200D}'));
    assert!(!shaped.missing.contains(&'\u{1F3FD}'));
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
