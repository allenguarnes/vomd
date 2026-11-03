//! `cargo run --release --example math_bench -- fixtures/math.md`
//! Every formula of a document laid out (parse + layout), with an empty cache (cold) and again
//! from it (warm): `fixtures/math.md`'s 1,000 in ≤ 30 ms cold. Then the document's first
//! viewport and its full layout in idle slices, a sweep of window widths (which lays no
//! formula out again), and the slowest of the fuzz seeds (`fuzz/seeds/math/`, the adversarial
//! ones among them): parse, layout, fit and emit in ≤ 10 ms each.
use std::time::{Duration, Instant};

use vomd::doc::{Doc, style};
use vomd::fonts::Fonts;
use vomd::layout::{Anchor, Layout, Params};
use vomd::math::Cache;
use vomd::math::emit::{Emitted, emit};
use vomd::math::fit::fit;
use vomd::math::layout::Engine;
use vomd::theme::Role;

const RUNS: usize = 10;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/math.md".into());
    let src = std::fs::read_to_string(&path).expect("read");
    let mut fonts = Fonts::new().expect("fonts");
    let t = Instant::now();
    fonts.load_math().expect("the math face");
    let face = ms(t.elapsed());
    let doc = Doc::parse(&src);
    let formulas: Vec<(&str, bool)> = doc
        .spans
        .iter()
        .filter(|s| s.style & style::MATH != 0)
        .map(|s| (doc.span_text(s), s.style & style::DISPLAY != 0))
        .collect();

    // Cold: a new cache (and engine) each run; warm: the same formulas again from it.
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    let mut failed = 0;
    for _ in 0..RUNS {
        let mut cache = Cache::default();
        let t = Instant::now();
        failed = formulas
            .iter()
            .filter(|(f, d)| !cache.get(&fonts, f, *d).is_some_and(|l| l.is_ok()))
            .count();
        cold.push(ms(t.elapsed()));
        let t = Instant::now();
        for (f, d) in &formulas {
            std::hint::black_box(cache.get(&fonts, f, *d));
        }
        warm.push(ms(t.elapsed()));
    }
    let first_cold = cold[0];
    cold.sort_by(f64::total_cmp);
    warm.sort_by(f64::total_cmp);
    let (cold_med, warm_med) = (cold[RUNS / 2], warm[RUNS / 2]);

    // The document: its first viewport, then the rest in idle slices, at 1920 px.
    let shared = Cache::shared();
    let params = Params {
        width: 1920.0,
        scale: 1.0,
        ..Params::default()
    };
    let mut layout = Layout::new(&doc, params);
    layout.set_math(shared.clone());
    let t = Instant::now();
    layout.ensure(&doc, &fonts, Anchor::TOP, 1080.0);
    let first = ms(t.elapsed());
    let first_laid = shared.borrow().laid;
    let t = Instant::now();
    let mut worst_slice = Duration::ZERO;
    loop {
        let call = Instant::now();
        let done = layout.idle(&doc, &fonts, call + Duration::from_millis(4));
        worst_slice = worst_slice.max(call.elapsed());
        if done {
            break;
        }
    }
    let idle = ms(t.elapsed());
    let (laid, weight) = (shared.borrow().laid, shared.borrow().weight());
    // Resizing: every formula comes from the cache.
    let t = Instant::now();
    for w in (400..=1920).step_by(40) {
        layout.set_params(Params {
            width: w as f32,
            ..params
        });
        let top = layout.anchor_at(layout.total_height() / 2.0);
        layout.ensure(&doc, &fonts, top, 1080.0);
    }
    let sweep = ms(t.elapsed());
    let relaid = shared.borrow().laid - laid;

    // The fuzz seeds, each as the `math` fuzz target takes it (best of 3: the machine's noise).
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fuzz/seeds/math");
    let mut engine = Engine::new();
    let (mut worst, mut worst_name, mut seeds) = (0.0f64, String::new(), 0);
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    paths.sort();
    let math = fonts.math().expect("the math face");
    for p in paths {
        let Ok(data) = std::fs::read(&p) else {
            continue;
        };
        let Some((&b, rest)) = data.split_first() else {
            continue;
        };
        let s = String::from_utf8_lossy(rest);
        let avail = 1.0 + f32::from(b >> 1) * 0.5;
        let best = (0..3)
            .map(|_| {
                let t = Instant::now();
                if let Ok(f) = engine.layout(&fonts, math, &s, b & 1 != 0) {
                    let (g, sc) = fit(&f, avail);
                    for em in [18.4 * sc, 36.8 * sc] {
                        let mut e = Emitted::default();
                        emit(&g, em, 0.0, 0.0, Role::Text, &mut e);
                        std::hint::black_box(e);
                    }
                }
                ms(t.elapsed())
            })
            .fold(f64::MAX, f64::min);
        if best > worst {
            worst = best;
            worst_name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        seeds += 1;
    }

    println!(
        "{path}: {} formulas ({failed} errors) | math face {face:.2} ms | cold: first {first_cold:.1} ms, \
         median {cold_med:.1} ms of {RUNS} ({:.1} µs a formula) | warm {warm_med:.2} ms | \
         first viewport {first:.1} ms ({first_laid} laid out) | idle full {idle:.0} ms (worst slice \
         {:.1} ms, {laid} laid out, {weight} glyphs kept) | width sweep {sweep:.0} ms, {relaid} laid out again | \
         slowest of {seeds} seeds {worst:.2} ms ({worst_name})",
        formulas.len(),
        cold_med * 1e3 / formulas.len().max(1) as f64,
        ms(worst_slice),
    );
    println!(
        "math-bench formulas={} cold_ms={cold_med:.2} warm_ms={warm_med:.3} first_ms={first:.2} \
         worst_seed_ms={worst:.2}",
        formulas.len()
    );
}
