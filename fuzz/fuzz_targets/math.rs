//! Bytes → a formula, parsed, laid out, fitted to a column and emitted at two sizes: never
//! panics, an error's span is a range of the source, and what lays out stays within the limits
//! (every coordinate finite and inside the extent, at most `MAX_GLYPHS` glyphs and rules, a
//! fitted formula inside its column), and an input stays fast (see [`BUDGET`]). The
//! first byte's low bit picks display math, its other bits the column (1 to 64.5 em) and the
//! size drawn at.
#![no_main]

use std::cell::RefCell;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use vomd::fonts::{FaceId, Fonts};
use vomd::math::emit::{Emitted, emit};
use vomd::math::fit::{CONT_INDENT, fit};
use vomd::math::layout::{Engine, Formula};
use vomd::math::parse::parse;
use vomd::math::{MAX_EXTENT, MAX_GLYPHS, MAX_NODES, MAX_SOURCE};
use vomd::theme::Role;

/// Where formulas are emitted, in pixels.
const X: f32 = 3.5;
const Y: f32 = 100.0;
/// The most an input may take: parse, layout, fit and both emits. The real bound is 10 ms in a
/// release build (`examples/math_bench.rs` times the worst seeds there); this build's coverage
/// counters, comparison tracing, debug assertions and overflow checks make it about four times
/// slower (an 8 KiB formula of 8,000 atoms: 3.5 ms in the test profile, 14 ms here), and
/// AddressSanitizer on a shared CI runner slower still, so the bound here is loose: it is there
/// to catch work that grows faster than the source, not to time the engine.
const BUDGET: Duration = Duration::from_millis(500);

thread_local! {
    static FONTS: (Fonts, FaceId) = {
        let mut fonts = Fonts::new().expect("embedded fonts");
        let math = fonts.load_math().expect("the math face");
        (fonts, math)
    };
    static ENGINE: RefCell<Engine> = RefCell::new(Engine::new());
}

fn inside(v: f32, extent: f32) -> bool {
    v.is_finite() && v.abs() <= extent
}

/// A cut's end: inside the extent, or none (infinite: the end of a stretched delimiter).
fn edge(v: f32, extent: f32) -> bool {
    v.is_infinite() || inside(v, extent)
}

/// What every formula laid out must be (`extent`: how far its coordinates may reach, in em).
fn check(f: &Formula, extent: f32) {
    assert!(
        inside(f.width, extent) && f.width >= 0.0,
        "width {}",
        f.width
    );
    assert!(f.height >= 0.0 && f.depth >= 0.0);
    assert!(
        inside(f.height + f.depth, MAX_EXTENT),
        "{} + {}",
        f.height,
        f.depth
    );
    assert!(f.glyphs.len() + f.rules.len() <= MAX_GLYPHS);
    for g in &f.glyphs {
        assert!(
            inside(g.x, extent) && inside(g.y, extent),
            "glyph at {} {}",
            g.x,
            g.y
        );
        assert!(
            g.size.is_finite() && g.size > 0.0 && g.size <= 1.0,
            "size {}",
            g.size
        );
        if let Some(c) = g.cut {
            assert!(
                edge(c.lo, extent) && edge(c.hi, extent) && c.lo <= c.hi,
                "{c:?}"
            );
        }
    }
    for r in &f.rules {
        assert!(inside(r.x, extent) && inside(r.y, extent));
        assert!(inside(r.x + r.w, extent) && inside(r.y + r.h, extent));
        assert!(r.w >= 0.0 && r.h >= 0.0, "rule {} × {}", r.w, r.h);
    }
}

/// Emitted at `em` pixels per em, at (`X`, `Y`): finite pixels, a style for each glyph.
fn check_emitted(e: &Emitted, em: f32) {
    // As far as a formula reaches, from where it is, and a pixel of rounding.
    let px = (MAX_EXTENT + CONT_INDENT) * em + X.max(Y) + 1.0;
    assert!(e.glyphs.iter().all(|g| inside(g.x, px) && inside(g.y, px)));
    assert!(e.glyphs.iter().all(|g| (g.style as usize) < e.styles.len()));
    assert!(e.styles.iter().all(|s| s.size.is_finite() && s.size > 0.0));
    assert!(e.ink.iter().all(|r| inside(r.x, px)
        && inside(r.y, px)
        && inside(r.w, px)
        && r.h >= 1.0
        && inside(r.h, px)));
    assert!(e.cuts.iter().all(|c| (c.glyph as usize) < e.glyphs.len()
        && edge(c.lo, px)
        && edge(c.hi, px)
        && c.lo <= c.hi));
}

/// Parse, layout, fit and emit: the time it took.
fn run(src: &str, b: u8) -> Duration {
    let display = b & 1 != 0;
    let avail = 1.0 + f32::from(b >> 1) * 0.5;
    let t = Instant::now();
    match parse(src, display) {
        Ok(ast) => assert!(src.len() <= MAX_SOURCE && ast.nodes.len() <= MAX_NODES),
        Err(e) => {
            let s = e.span;
            assert!(
                s.start <= s.end && s.end <= src.len(),
                "{s:?} of {}",
                src.len()
            );
            assert!(src.is_char_boundary(s.start) && src.is_char_boundary(s.end));
        }
    }
    FONTS.with(|(fonts, math)| {
        let laid = ENGINE.with(|e| e.borrow_mut().layout(fonts, *math, src, display));
        let Ok(f) = laid else {
            return;
        };
        check(&f, MAX_EXTENT);
        if let Some(tag) = &f.tag {
            check(tag, MAX_EXTENT);
        }
        let (g, s) = fit(&f, avail);
        check(&g, MAX_EXTENT + CONT_INDENT);
        assert!(s > 0.0 && s <= 1.0, "scale {s}");
        assert!(g.width * s <= avail, "{} × {s} in {avail}", g.width);
        // Body text at 16 px, at 1× and at 2× (`b`'s high bit: at a 1/8 of that, a shrunk
        // formula's sizes).
        let small = if b & 0x80 != 0 { 0.125 } else { 1.0 };
        for em in [16.0 * 1.15 * s, 2.0 * 16.0 * 1.15 * s * small] {
            let mut e = Emitted::default();
            emit(&g, em, X, Y, Role::Text, &mut e);
            check_emitted(&e, em);
            assert_eq!(e.glyphs.len(), g.glyphs.len());
        }
    });
    t.elapsed()
}

fuzz_target!(|data: &[u8]| {
    let Some((&b, rest)) = data.split_first() else {
        return;
    };
    let src = String::from_utf8_lossy(rest);
    // Once more if too slow, its outlines measured by then: a slow first run can be the
    // machine's, or the first sight of a glyph.
    if run(&src, b) > BUDGET {
        let again = run(&src, b);
        assert!(again <= BUDGET, "{} ms", again.as_secs_f64() * 1e3);
    }
});
