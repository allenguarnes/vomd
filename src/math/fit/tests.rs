use super::*;

fn piece(x: f32, w: f32, breaks: Option<Break>) -> Piece {
    Piece {
        x,
        w,
        h: 0.7,
        d: 0.2,
        glyphs: 0,
        rules: 0,
        breaks,
        depth: 0,
    }
}

/// `a = b + c = d`, each atom 1 em wide with no space, as pieces.
fn row() -> Vec<Piece> {
    let kinds = [
        None,
        Some(Break::Rel),
        None,
        Some(Break::Bin),
        None,
        Some(Break::Rel),
        None,
    ];
    kinds
        .iter()
        .enumerate()
        .map(|(i, &b)| piece(i as f32, 1.0, b))
        .collect()
}

#[test]
fn lines_break_at_relations_first() {
    let p = row();
    assert_eq!(lines(&p, 100.0, Pass::any(Break::Rel)), [0]);
    // `a = b + c` fits in 5 em, `= d` goes on (2 em + its indent).
    assert_eq!(lines(&p, 5.0, Pass::any(Break::Rel)), [0, 5]);
    // In 4 em `a = b + c` doesn't: at relations alone a line is too wide…
    assert_eq!(lines(&p, 4.0, Pass::any(Break::Rel)), [0, 1, 5]);
    assert!(widest(&p, &[0, 1, 5]) > 4.0);
    // …and with binary operators too it isn't.
    assert_eq!(lines(&p, 4.0, Pass::any(Break::Bin)), [0, 3, 5]);
    assert!(widest(&p, &[0, 3, 5]) <= 4.0);
    // Room for nothing: a line at every break.
    assert_eq!(lines(&p, 0.5, Pass::any(Break::Bin)), [0, 1, 3, 5]);
}

#[test]
fn quantized_scales() {
    assert_eq!(quantize(1.5), 1.0);
    assert_eq!(quantize(0.51), 0.5);
    assert_eq!(quantize(0.03), 30.0 / 1024.0);
    assert_eq!(quantize(0.0), 1.0 / 1024.0);
    assert_eq!(quantize(f32::NAN), 1.0 / 1024.0);
    assert_eq!(quantize(-3.0), 1.0 / 1024.0);
    // The widest formula there is fits in a column an em wide.
    assert!(quantize(1.0 / crate::math::MAX_EXTENT) * crate::math::MAX_EXTENT <= 1.0);
}

fn formula(pieces: Vec<Piece>) -> Formula {
    let width = pieces.last().map_or(0.0, |p| p.x + p.w);
    Formula {
        width,
        height: 0.7,
        depth: 0.2,
        pieces,
        ..Formula::default()
    }
}

#[test]
fn a_formula_is_broken_at_full_size_then_shrunk() {
    let f = formula(row());
    // Fits: as it is.
    let (g, s) = fit(&f, 8.0);
    assert!(matches!(g, Cow::Borrowed(_)) && s == 1.0);
    // A little too wide: broken, not shrunk. `a = b + c` and `= d` (2 em + its indent).
    let (g, s) = fit(&f, 6.0);
    assert!(matches!(g, Cow::Owned(_)) && s == 1.0, "{s}");
    assert_eq!(g.width, 5.0);
    assert!(g.depth > f.depth + 1.0, "the lines go down");
    // Narrower: broken at the operators too, still full size.
    let (g, s) = fit(&f, 4.0);
    assert!(matches!(g, Cow::Owned(_)) && s == 1.0, "{s}");
    assert!(g.width <= 4.0);
    // Narrower than any line can be: broken everywhere, and shrunk to fit.
    let (g, s) = fit(&f, 2.5);
    assert!(matches!(g, Cow::Owned(_)) && s < 1.0);
    assert!(g.width * s <= 2.5, "{} × {s}", g.width);
    // Nothing to break at, even only slightly too wide: shrunk as far as it takes.
    let one = Formula {
        pieces: Vec::new(),
        ..f.clone()
    };
    let (g, s) = fit(&one, 6.9);
    assert!(matches!(g, Cow::Borrowed(_)) && s < 1.0 && g.width * s <= 6.9);
    let (g, s) = fit(&one, 1.0);
    assert!(matches!(g, Cow::Borrowed(_)) && g.width * s <= 1.0);
    let (g, s) = fit(&one, 0.01);
    assert!(g.width * s <= 0.01, "never cut off: {} × {s}", g.width);
}

#[test]
fn breaking_that_doesnt_help_is_left_to_shrinking() {
    // `a = <10 em with no break in it>`: the lines would be as wide as the whole.
    let f = formula(vec![
        piece(0.0, 1.0, None),
        piece(1.0, 1.0, Some(Break::Rel)),
        piece(2.0, 10.0, None),
    ]);
    let (g, s) = fit(&f, 6.0);
    assert!(matches!(g, Cow::Borrowed(_)), "not broken");
    assert_eq!(s, quantize(6.0 / 12.0));
}

#[test]
fn lines_too_many_to_stack_are_shrunk_instead() {
    // 1,800 atoms of half an em, a relation before every other one: a line each would be
    // taller than a formula may be.
    let pieces = (0..1800)
        .map(|i| piece(i as f32 / 2.0, 0.5, (i % 2 == 1).then_some(Break::Rel)))
        .collect();
    let f = formula(pieces);
    let (g, s) = fit(&f, 1.5);
    assert!(matches!(g, Cow::Borrowed(_)));
    assert!(g.width * s <= 1.5);
}

/// `A + B + (x + y) + C`, an em a piece, the `x + y` in parentheses (their pieces one deep).
fn with_parentheses() -> Vec<Piece> {
    let kinds = [
        (None, 0),
        (Some(Break::Bin), 0),
        (None, 0),
        (Some(Break::Bin), 0),
        (None, 0),
        (None, 1),
        (Some(Break::Bin), 1),
        (None, 1),
        (None, 1),
        (Some(Break::Bin), 0),
        (None, 0),
    ];
    kinds
        .iter()
        .enumerate()
        .map(|(i, &(b, depth))| Piece {
            depth,
            ..piece(i as f32, 1.0, b)
        })
        .collect()
}

#[test]
fn a_break_outside_parentheses_is_preferred() {
    let f = formula(with_parentheses());
    // Anywhere, the first line would be `A + B + (x` (6 em, the most that fits in 8), with
    // `+ y) + C` under it.
    assert_eq!(
        lines(&f.pieces, 8.0, Pass::any(Break::Bin)),
        [0, 6],
        "breaks inside the parentheses"
    );
    // Outside them: `A + B` / `+ (x + y)` / `+ C`: a line is wider, none is cut in two.
    let (g, s) = fit(&f, 8.0);
    assert!(matches!(g, Cow::Owned(_)) && s == 1.0);
    assert_eq!(g.width, 8.0);
    assert!(g.glyphs.is_empty());
    assert_eq!(lines(&f.pieces, 8.0, PASSES[1]), [0, 3, 9]);
    // Where nothing outside fits, inside will do: 6 em, the lines are `A + B`, `+ (x`, `+ y) + C`.
    let (g, s) = fit(&f, 6.0);
    assert!(matches!(g, Cow::Owned(_)) && s == 1.0, "{s}");
    assert!(g.width <= 6.0);
}

#[test]
fn a_first_line_of_one_small_atom_is_not_made() {
    // `A = <3 em> + <3 em>`: a break before `=` alone would leave `A` on a line.
    let f = formula(vec![
        piece(0.0, 1.0, None),
        piece(1.0, 1.0, Some(Break::Rel)),
        piece(2.0, 3.0, None),
        piece(5.0, 1.0, Some(Break::Bin)),
        piece(6.0, 3.0, None),
    ]);
    let (g, s) = fit(&f, 7.0);
    assert!(matches!(g, Cow::Owned(_)) && s == 1.0, "{s}");
    // `A = <3>` and `+ <3>`: the first line has more than the `A`.
    assert!(g.depth > f.depth + 1.0);
    assert_eq!(lines(&f.pieces, 7.0, PASSES[1]), [0, 3]);
    assert_eq!(lines(&f.pieces, 7.0, Pass::any(Break::Rel)), [0, 1]);
    // A wide first piece is worth a line.
    let wide = formula(vec![
        piece(0.0, 5.0, None),
        piece(5.0, 1.0, Some(Break::Rel)),
        piece(6.0, 3.0, None),
    ]);
    assert_eq!(lines(&wide.pieces, 6.0, PASSES[0]), [0, 1]);
}
