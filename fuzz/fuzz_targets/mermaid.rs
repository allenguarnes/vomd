//! Bytes → a Mermaid diagram's SVG, as the process that makes diagrams makes it: never panics,
//! stays within its bounds (the source cap, the layouts' work budget: see [`BUDGET`]), and an
//! SVG made is one VOMD's SVG code reads, with a size. The first byte's low bit picks the dark
//! theme.
#![no_main]

use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use vomd::mermaid::{MAX_SOURCE, Metrics, make};

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
    let made = make(&src, b & 1 != 0, &Metrics::default());
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
