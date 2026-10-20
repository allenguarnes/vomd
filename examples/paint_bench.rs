//! `cargo run --release --example paint_bench -- fixtures/readme.md [line_width] [--select-all]`
//! A full repaint of a 4K window (3840×2160 at scale 2) takes ≤ 8 ms. The first paint
//! (cold glyph cache) is reported separately from scroll frames, each of which is timed with
//! the layout work that keeps ahead of it (`Layout::ensure`) plus the paint. `--select-all`
//! paints with the whole document selected.
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::layout::{Layout, Params};
use vomd::render::{self, Clip, Frame, Viewport};
#[cfg(feature = "clipboard")]
use vomd::select::Pos;
use vomd::text::cache::GlyphCache;
use vomd::theme::LIGHT;

const W: usize = 3840;
const H: usize = 2160;
const FRAMES: usize = 200;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let select_all = args.iter().any(|a| a == "--select-all");
    args.retain(|a| a != "--select-all");
    let path = args.first().expect("path");
    let src = std::fs::read_to_string(path).expect("read");
    // A wide line_width fills the whole window with text (the densest repaint).
    let line_width = args.get(1).map_or(Params::default().line_width, |w| {
        w.parse().expect("line_width")
    });
    let fonts = Fonts::new().unwrap();
    let doc = Doc::parse(&src);
    #[cfg(feature = "clipboard")]
    let select = select_all.then(|| (Pos::start(0), Pos::end(doc.blocks.len() as u32 - 1)));
    #[cfg(not(feature = "clipboard"))]
    assert!(!select_all, "--select-all needs the clipboard feature");
    let mut layout = Layout::new(
        &doc,
        Params {
            width: W as f32,
            scale: 2.0,
            line_width,
            ..Params::default()
        },
    );
    let mut cache = GlyphCache::default();
    let mut px = vec![0u32; W * H];
    let area = Clip {
        x0: 0,
        y0: 0,
        x1: W as i32,
        y1: H as i32,
    };
    let mut paint = |layout: &Layout, cache: &mut GlyphCache, scroll: f64| {
        let mut frame = Frame::new(&mut px, W, H, W);
        let vp = Viewport { area, scroll };
        render::paint(
            &mut frame,
            vp,
            layout,
            &fonts,
            cache,
            &LIGHT,
            #[cfg(feature = "clipboard")]
            select,
            #[cfg(feature = "find")]
            None,
        );
        render::paint_scrollbar(&mut frame, vp, layout.total_height(), 2.0, &LIGHT);
    };

    let y = layout.ensure(&doc, &fonts, layout.anchor_at(0.0), H as f32);
    let t = Instant::now();
    paint(&layout, &mut cache, y as f64);
    let cold = t.elapsed();
    // Scroll frames: 40 px steps through the document, timed with the layout ahead of each.
    let mut times: Vec<Duration> = Vec::with_capacity(FRAMES);
    let max_scroll = (layout.total_height() - H as f32).max(0.0);
    for i in 0..FRAMES {
        let target = (i as f32 * 40.0).min(max_scroll);
        let t = Instant::now();
        let y = layout.ensure(&doc, &fonts, layout.anchor_at(target), H as f32);
        paint(&layout, &mut cache, y as f64);
        times.push(t.elapsed());
    }
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    println!(
        "{path}: {W}×{H} @2x | first paint (cold cache, layout done) {:.2} ms | scroll frames: median {:.2} ms, \
         p95 {:.2} ms, worst {:.2} ms | glyph cache {} entries, {} KB",
        ms(cold),
        ms(times[FRAMES / 2]),
        ms(times[FRAMES * 95 / 100]),
        ms(times[FRAMES - 1]),
        cache.len(),
        cache.bytes() / 1024,
    );
    // For `cargo xtask budgets` (scroll frame time, 4K window).
    println!(
        "paint-bench p95_ms={:.3} worst_ms={:.3}",
        ms(times[FRAMES * 95 / 100]),
        ms(times[FRAMES - 1])
    );
}
