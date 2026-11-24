//! Bytes → a Mermaid diagram's SVG, as the process that makes diagrams makes it: never panics,
//! stays within its bounds (the source cap, the layouts' work budget: see [`BUDGET`]), and an
//! SVG made is one VOMD's SVG code reads, with a size. The first byte's low bit picks the dark
//! theme. Its text outside Latin is measured as the window measures it (`scan`), in VOMD's faces
//! and the tests' color fonts as fallbacks (`fixtures/fonts`).
#![no_main]

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use vomd::fonts::Fonts;
use vomd::mermaid::{MAX_SOURCE, Metrics, make, scan};

static FONTS: LazyLock<(Fonts, Metrics)> = LazyLock::new(|| {
    let mut fonts = Fonts::new().unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fonts");
    for name in ["test-emoji-colr.ttf", "test-emoji-cbdt.ttf"] {
        fonts.add_fallback(&dir.join(name), 0).expect(name);
    }
    let metrics = Metrics::of(&fonts);
    (fonts, metrics)
});

/// The most an input may take. A diagram is made in a few milliseconds in a release build, and
/// the layouts' work budget stops a dense one well short of a second there; this build's
/// coverage counters, comparison tracing and checks make it several times slower, so the bound
/// is loose: it catches work that grows faster than the source, not the layout's speed.
const BUDGET: Duration = Duration::from_secs(5);

fuzz_target!(|data: &[u8]| {
    let Some((&b, rest)) = data.split_first() else {
        return;
    };
    let src = String::from_utf8_lossy(rest);
    let t = Instant::now();
    let (fonts, base) = &*FONTS;
    let metrics = Metrics {
        seq: scan(&src, fonts).0,
        ..base.clone()
    };
    let made = make(&src, b & 1 != 0, &metrics);
    let took = t.elapsed();
    assert!(took <= BUDGET, "{} ms", took.as_secs_f64() * 1e3);
    match made {
        Ok(svg) => {
            assert!(src.len() <= MAX_SOURCE);
            let (_, w, h) = vomd::image::header_of(svg.as_bytes()).expect("an SVG VOMD reads");
            assert!(w > 0 && h > 0, "{w} × {h}");
        }
        Err(why) => assert!(!why.is_empty()),
    }
});
