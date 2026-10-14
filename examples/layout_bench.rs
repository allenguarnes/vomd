//! `cargo run --release --example layout_bench -- fixtures/huge.md`
//! The first viewport is fast; full measurement runs in idle slices with flat memory.
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::layout::{Anchor, Layout, Params};

fn rss_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix(field))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let src = std::fs::read_to_string(&path).expect("read");
    let t = Instant::now();
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&src);
    let parsed = t.elapsed();
    let params = Params {
        width: 1920.0,
        scale: 1.0,
        ..Params::default()
    };
    let t = Instant::now();
    let mut layout = Layout::new(&doc, params);
    let created = t.elapsed();
    let t = Instant::now();
    layout.ensure(&doc, &fonts, Anchor::TOP, 1080.0);
    let first = t.elapsed();
    let t = Instant::now();
    let mut slices = 0;
    let mut worst = Duration::ZERO;
    loop {
        let call = Instant::now();
        let done = layout.idle(&doc, &fonts, call + Duration::from_millis(4));
        worst = worst.max(call.elapsed());
        if done {
            break;
        }
        slices += 1;
    }
    let idle = t.elapsed();
    let t = Instant::now();
    let mid = layout.anchor_at(layout.total_height() / 2.0);
    layout.ensure(&doc, &fonts, mid, 1080.0);
    let jump = t.elapsed();
    println!(
        "{path}: {} blocks | parse {:.1} ms | Layout::new {:.1} ms | first viewport {:.1} ms | \
         idle full {:.0} ms in {slices} slices (worst {:.1} ms) | jump to middle {:.1} ms | height {:.0} px | \
         RSS now {} KB, peak {} KB",
        doc.blocks.len(),
        parsed.as_secs_f64() * 1e3,
        created.as_secs_f64() * 1e3,
        first.as_secs_f64() * 1e3,
        idle.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3,
        jump.as_secs_f64() * 1e3,
        layout.total_height(),
        rss_kb("VmRSS:"),
        rss_kb("VmHWM:"),
    );
}
