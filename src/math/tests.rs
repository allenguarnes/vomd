use super::table::{Constant, Corner, MathTable, Part, Variant, coverage_index};

/// The embedded subset, whatever the build's features.
static STIX: &[u8] = include_bytes!("../../assets/fonts/STIXTwoMath-Regular.otf");

fn math_bytes() -> &'static [u8] {
    let font = swash::FontRef::from_index(STIX, 0).expect("font");
    font.table(swash::tag_from_bytes(b"MATH"))
        .expect("MATH table")
}

fn glyph(ch: char) -> u16 {
    let font = swash::FontRef::from_index(STIX, 0).expect("font");
    font.charmap().map(ch)
}

#[test]
fn stix_constants() {
    let t = MathTable::new(math_bytes()).expect("table");
    assert_eq!(t.constant(Constant::ScriptPercentScaleDown), Some(70));
    assert_eq!(t.constant(Constant::ScriptScriptPercentScaleDown), Some(55));
    assert_eq!(
        t.constant(Constant::DelimitedSubFormulaMinHeight),
        Some(1325)
    );
    assert_eq!(t.constant(Constant::DisplayOperatorMinHeight), Some(1800));
    assert_eq!(t.constant(Constant::MathLeading), Some(150));
    assert_eq!(t.constant(Constant::AxisHeight), Some(258));
    assert_eq!(t.constant(Constant::SubscriptShiftDown), Some(210));
    assert_eq!(t.constant(Constant::FractionRuleThickness), Some(68));
    assert_eq!(t.constant(Constant::RadicalKernAfterDegree), Some(-335));
    assert_eq!(
        t.constant(Constant::RadicalDegreeBottomRaisePercent),
        Some(55)
    );
}

#[test]
fn stix_glyph_info() {
    let t = MathTable::new(math_bytes()).expect("table");
    // Math italic f leans far right; the integral sign even more.
    assert_eq!(t.italics_correction(glyph('\u{1D453}')), Some(20));
    assert_eq!(t.italics_correction(glyph('∫')), Some(230));
    assert_eq!(t.italics_correction(glyph('f')), None, "upright: none");
    assert_eq!(t.top_accent_attachment(glyph('\u{1D44E}')), Some(340));
    assert_eq!(t.top_accent_attachment(glyph('x')), Some(250));
    assert_eq!(t.top_accent_attachment(glyph('\u{1D465}')), Some(345));
    assert!(t.is_extended_shape(glyph('|')));
    assert!(!t.is_extended_shape(glyph('a')));
    // A's top right corner: no cut-in low down, more of one higher up.
    let a = glyph('A');
    assert_eq!(t.kern(a, Corner::TopRight, 0), Some(0));
    assert_eq!(t.kern(a, Corner::TopRight, 300), Some(-18));
    assert_eq!(t.kern(a, Corner::TopRight, 352), Some(-66));
    assert_eq!(t.kern(a, Corner::TopRight, 1000), Some(-66));
    assert_eq!(t.kern(a, Corner::TopLeft, 300), None);
    assert_eq!(t.kern(glyph('+'), Corner::TopRight, 300), None);
}

#[test]
fn stix_variants_and_assemblies() {
    let t = MathTable::new(math_bytes()).expect("table");
    assert_eq!(t.min_connector_overlap(), Some(100));
    // `(`: itself and 12 bigger sizes, then an assembly of a bottom hook, an extender and a top
    // hook.
    let paren = glyph('(');
    let c = t.construction(paren, true).expect("vertical");
    assert_eq!(c.len(), 13);
    assert_eq!(
        c.variant(0),
        Some(Variant {
            glyph: paren,
            advance: 933
        })
    );
    assert_eq!(c.variants().last().map(|v| v.advance), Some(3821));
    let advances: Vec<u16> = c.variants().map(|v| v.advance).collect();
    assert!(advances.windows(2).all(|w| w[0] < w[1]), "{advances:?}");
    let a = c.assembly().expect("assembly");
    assert_eq!(a.italics_correction, 0);
    assert_eq!(a.len(), 3);
    let parts: Vec<Part> = a.parts().collect();
    assert_eq!(
        (
            parts[0].start_connector,
            parts[0].end_connector,
            parts[0].full_advance
        ),
        (0, 250, 1273)
    );
    assert_eq!(
        parts.iter().map(|p| p.extender).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert!(
        t.construction(paren, false).is_none(),
        "doesn't grow sideways"
    );
    // A wide hat: variants, no assembly; an overbrace: both; an arrow: one variant and parts.
    let hat = t.construction(glyph('\u{0302}'), false).expect("hat");
    assert_eq!((hat.len(), hat.assembly().is_none()), (6, true));
    let brace = t.construction(glyph('⏞'), false).expect("overbrace");
    assert_eq!(
        (brace.len(), brace.assembly().map(|a| a.len())),
        (6, Some(5))
    );
    let arrow = t.construction(glyph('→'), false).expect("arrow");
    assert_eq!(
        (arrow.len(), arrow.assembly().map(|a| a.len())),
        (1, Some(2))
    );
    assert!(t.construction(glyph('a'), true).is_none());
}

#[test]
fn coverage_formats() {
    // Format 1: glyphs 3, 7, 9.
    let one = [0, 1, 0, 3, 0, 3, 0, 7, 0, 9];
    assert_eq!(coverage_index(&one, 7), Some(1));
    assert_eq!(coverage_index(&one, 9), Some(2));
    assert_eq!(coverage_index(&one, 8), None);
    assert_eq!(coverage_index(&one, 0), None);
    // Format 2: glyphs 10..=12 from index 0, 20..=20 from index 3.
    let two = [0, 2, 0, 2, 0, 10, 0, 12, 0, 0, 0, 20, 0, 20, 0, 3];
    assert_eq!(coverage_index(&two, 11), Some(1));
    assert_eq!(coverage_index(&two, 20), Some(3));
    assert_eq!(coverage_index(&two, 13), None);
    // A count beyond the data, and an unknown format: nothing is covered.
    assert_eq!(coverage_index(&[0, 1, 0, 9, 0, 3], 3), None);
    assert_eq!(coverage_index(&[0, 3, 0, 1, 0, 3], 3), None);
    assert_eq!(coverage_index(&[], 3), None);
}

/// Every query the reader answers, one per entry, on a few glyphs that have data of each kind.
fn answers(t: &MathTable) -> Vec<String> {
    let mut out = Vec::new();
    for c in [
        Constant::ScriptPercentScaleDown,
        Constant::DisplayOperatorMinHeight,
        Constant::AxisHeight,
        Constant::RadicalDegreeBottomRaisePercent,
    ] {
        out.push(format!("{:?}", t.constant(c)));
    }
    out.push(format!("{:?}", t.min_connector_overlap()));
    for ch in ['(', 'A', '∫', '\u{1D453}', '\u{1D44E}', '|', '⏞', '→', '√'] {
        let g = glyph(ch);
        out.push(format!("{:?}", t.italics_correction(g)));
        out.push(format!("{:?}", t.top_accent_attachment(g)));
        out.push(format!("{}", t.is_extended_shape(g)));
        for corner in [Corner::TopRight, Corner::BottomLeft] {
            out.push(format!("{:?}", t.kern(g, corner, 300)));
        }
        for vertical in [true, false] {
            let c = t.construction(g, vertical);
            out.push(format!("{:?}", c.map(|c| c.variants().collect::<Vec<_>>())));
            let a = c.and_then(|c| c.assembly());
            out.push(format!(
                "{:?}",
                a.map(|a| (a.italics_correction, a.parts().collect::<Vec<_>>()))
            ));
        }
    }
    out
}

#[test]
fn a_truncated_table_answers_none_or_the_truth() {
    let full = math_bytes();
    let truth = answers(&MathTable::new(full).expect("table"));
    for len in 0..full.len() {
        let Some(t) = MathTable::new(&full[..len]) else {
            assert!(len < 10, "a header of {len} bytes was refused");
            continue;
        };
        // Bytes kept are the same, so whatever is read is right; what is cut off is no data.
        for (got, want) in answers(&t).iter().zip(&truth) {
            let none = got == "None" || got == "false";
            assert!(got == want || none, "{len} bytes: {got} (whole: {want})");
        }
    }
}

#[test]
fn garbage_never_panics() {
    let full = math_bytes();
    // Each byte in turn set to 0xff (offsets, counts and formats far out of range), then 0.
    let mut bytes = full.to_vec();
    for i in 0..bytes.len() {
        for v in [0xff, 0] {
            let old = bytes[i];
            bytes[i] = v;
            if let Some(t) = MathTable::new(&bytes) {
                answers(&t);
            }
            bytes[i] = old;
        }
    }
    // Random tables behind a valid header.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..2_000 {
        let len = 10 + (next() % 600) as usize;
        let mut b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        b[0] = 0;
        b[1] = 1;
        if round % 2 == 0 {
            // Small offsets, so they land inside.
            for at in [4, 6, 8] {
                b[at] = 0;
                b[at + 1] = (next() % len as u64) as u8;
            }
        }
        if let Some(t) = MathTable::new(&b) {
            answers(&t);
        }
    }
    assert!(
        MathTable::new(&[0, 2, 0, 0, 0, 10, 0, 0, 0, 0]).is_none(),
        "version 2"
    );
}

mod symbols {
    use super::{STIX, glyph};
    use crate::math::symbols::{Class, Font, class_of, delimiter, font_chars, styled, symbol};

    #[test]
    fn names_are_found() {
        let s = symbol("alpha").expect("alpha");
        assert_eq!((s.ch, s.class), ('α', Class::Ord));
        assert_eq!(
            symbol("leq").map(|s| (s.ch, s.class)),
            Some(('≤', Class::Rel))
        );
        assert_eq!(symbol("sum").map(|s| s.limits), Some(true));
        assert_eq!(
            symbol("int").map(|s| (s.class, s.limits)),
            Some((Class::Op, false))
        );
        assert_eq!(symbol("{").map(|s| s.class), Some(Class::Open));
        assert_eq!(symbol("backslash").map(|s| s.ch), Some('\\'));
        // The first and last lines, and names that are prefixes of others.
        assert_eq!(symbol("#").map(|s| s.ch), Some('#'));
        assert_eq!(symbol("}").map(|s| s.ch), Some('}'));
        assert_eq!(symbol("le").map(|s| s.ch), Some('≤'));
        assert_eq!(symbol("l").map(|s| s.ch), None);
        assert_eq!(symbol("zzz"), None);
        assert_eq!(symbol(""), None);
        assert_eq!(symbol("frac"), None, "structure is the parser's");
    }

    /// Every line decodes and is found by the search, and the table is sorted.
    #[test]
    fn every_symbol_is_found() {
        let src = include_str!("symbols.rs");
        let start = src.find("static SYMBOLS: &str = \"\\\n").expect("table") + 26;
        let table = &src[start..start + src[start..].find("\";").expect("end")];
        let mut names = Vec::new();
        for line in table.lines() {
            let (name, _) = line.split_once(' ').expect("name");
            let name = name.replace("\\\\", "\\");
            assert!(symbol(&name).is_some(), "{name}");
            names.push(name);
        }
        assert!(names.len() > 480, "{}", names.len());
        assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted, no repeats");
    }

    #[test]
    fn typed_characters_have_classes() {
        assert_eq!(class_of('+'), Class::Bin);
        assert_eq!(class_of('='), Class::Rel);
        assert_eq!(class_of('('), Class::Open);
        assert_eq!(class_of(','), Class::Punct);
        assert_eq!(class_of('x'), Class::Ord);
        assert_eq!(class_of('≤'), Class::Rel);
        assert_eq!(class_of('∣'), Class::Rel, "\\mid before \\vert");
        assert_eq!(class_of('⋅'), Class::Bin);
        assert_eq!(class_of('∑'), Class::Op);
        assert_eq!(class_of('中'), Class::Ord);
    }

    #[test]
    fn delimiters() {
        assert_eq!(delimiter("."), Some(None));
        assert_eq!(delimiter("("), Some(Some('(')));
        assert_eq!(delimiter("langle"), Some(Some('⟨')));
        assert_eq!(delimiter("<"), Some(Some('⟨')));
        assert_eq!(delimiter("Vert"), Some(Some('‖')));
        assert_eq!(delimiter("uparrow"), Some(Some('↑')));
        assert_eq!(delimiter("alpha"), None);
        assert_eq!(delimiter("x"), None);
    }

    #[test]
    fn alphabets() {
        assert_eq!(styled('x', Font::Normal), '\u{1D465}');
        assert_eq!(styled('h', Font::Normal), 'ℎ');
        assert_eq!(styled('2', Font::Normal), '2');
        assert_eq!(styled('α', Font::Normal), '\u{1D6FC}');
        assert_eq!(styled('Γ', Font::Normal), 'Γ', "capitals upright");
        assert_eq!(styled('ϵ', Font::Normal), '\u{1D716}');
        assert_eq!(styled('ı', Font::Normal), '\u{1D6A4}');
        assert_eq!(styled('x', Font::Roman), 'x');
        assert_eq!(styled('R', Font::DoubleStruck), 'ℝ');
        assert_eq!(styled('A', Font::DoubleStruck), '\u{1D538}');
        assert_eq!(styled('1', Font::DoubleStruck), '\u{1D7D9}');
        assert_eq!(styled('L', Font::Script), 'ℒ');
        assert_eq!(styled('A', Font::Script), '\u{1D49C}');
        assert_eq!(styled('g', Font::Fraktur), '\u{1D524}');
        assert_eq!(styled('Z', Font::Fraktur), 'ℨ');
        assert_eq!(styled('v', Font::Bold), '\u{1D42F}');
        assert_eq!(styled('9', Font::Bold), '\u{1D7D7}');
        assert_eq!(styled('Ω', Font::Bold), '\u{1D6C0}');
        assert_eq!(styled('α', Font::BoldItalic), '\u{1D736}');
        assert_eq!(styled('ω', Font::Italic), '\u{1D714}');
        assert_eq!(styled('+', Font::Bold), '+');
        assert_eq!(styled('x', Font::Sans), 'x', "set in the text face");
    }

    /// Every character of the table, the accents, the layout and the alphabets is in the
    /// embedded font.
    #[test]
    fn the_font_has_every_character() {
        let missing: Vec<String> = font_chars()
            .into_iter()
            .filter(|&c| glyph(c) == 0)
            .map(|c| format!("U+{:04X}", c as u32))
            .collect();
        assert!(missing.is_empty(), "not in the font: {missing:?}");
        assert!(STIX.len() > 100_000);
    }

    /// `assets/fonts/math-codepoints.txt` is what `font_chars` says; `VOMD_BLESS=1` writes it.
    #[test]
    fn the_codepoint_list_matches_the_table() {
        let mut want = String::from(
            "# The code points kept in assets/fonts/STIXTwoMath-Regular.otf\n\
             # (scripts/subset-math-font.sh): src/math/symbols.rs's `font_chars`, the symbol\n\
             # table's characters and the alphabets'. Written by `VOMD_BLESS=1 cargo test\n\
             # the_codepoint_list` (its test checks it); don't edit it by hand.\n",
        );
        let chars = font_chars();
        let mut i = 0;
        while i < chars.len() {
            let mut j = i;
            while j + 1 < chars.len() && chars[j + 1] as u32 == chars[j] as u32 + 1 {
                j += 1;
            }
            if i == j {
                want.push_str(&format!("U+{:04X}\n", chars[i] as u32));
            } else {
                want.push_str(&format!(
                    "U+{:04X}-{:04X}\n",
                    chars[i] as u32, chars[j] as u32
                ));
            }
            i = j + 1;
        }
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/math-codepoints.txt"
        );
        if std::env::var_os("VOMD_BLESS").is_some() {
            std::fs::write(path, &want).expect("write");
        }
        let have = std::fs::read_to_string(path).expect("read");
        assert!(
            have == want,
            "math-codepoints.txt is stale: VOMD_BLESS=1 to rewrite it"
        );
    }
}

#[cfg(feature = "embed-fonts")]
mod cache {
    use crate::fonts::Fonts;
    use crate::math::{CACHE_CAP, CACHE_WEIGHT, Cache};

    fn fonts() -> Fonts {
        let mut fonts = Fonts::new().expect("fonts");
        fonts.load_math().expect("math face");
        fonts
    }

    #[test]
    fn a_formula_is_laid_out_once() {
        let fonts = fonts();
        let mut cache = Cache::default();
        for _ in 0..3 {
            assert!(cache.get(&fonts, r"\frac{a}{b}", false).is_some());
            assert!(cache.get(&fonts, r"\frac{a}{b}", true).is_some());
        }
        assert_eq!((cache.laid, cache.len()), (2, 2));
    }

    /// Past the cap the formulas not used lately go, and the ones in use stay.
    #[test]
    fn the_least_recently_used_go_first() {
        let fonts = fonts();
        let mut cache = Cache::default();
        let hot = |i: usize| format!("h_{{{i}}}");
        for i in 0..CACHE_CAP + 500 {
            cache.get(&fonts, &format!("x_{{{i}}}"), false);
            // A few formulas on screen all along: each is found again.
            for k in 0..4 {
                cache.get(&fonts, &hot(k), false);
            }
        }
        assert_eq!(cache.len(), CACHE_CAP);
        let laid = cache.laid;
        for k in 0..4 {
            cache.get(&fonts, &hot(k), false);
        }
        assert_eq!(cache.laid, laid, "the formulas in use were kept");
    }

    /// Big formulas are dropped by their weight long before the count's cap.
    #[test]
    fn the_cache_holds_so_many_glyphs() {
        let fonts = fonts();
        let mut cache = Cache::default();
        // 3,000 glyphs each: 30 columns, 100 rows.
        let row = format!("{}x", "x&".repeat(29));
        let big = |i: usize| {
            format!(
                r"\begin{{matrix}}{}\end{{matrix}}{i}",
                [row.as_str(); 100].join(r"\\")
            )
        };
        for i in 0..100 {
            let laid = cache.get(&fonts, &big(i), true).expect("laid");
            assert!(laid.as_ref().as_ref().is_ok_and(|f| f.glyphs.len() >= 3000));
            assert!(cache.weight() <= CACHE_WEIGHT, "{}", cache.weight());
        }
        let n = cache.len();
        assert!((CACHE_WEIGHT / 3200..100).contains(&n), "{n}");
    }
}
