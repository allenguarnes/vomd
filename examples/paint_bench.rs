//! `cargo run --release --example paint_bench -- fixtures/readme.md [line_width] [--select-all]
//! [--damage] [--view [--animations]]`
//! A full repaint of a 4K window (3840×2160 at scale 2) takes ≤ 8 ms. The first paint
//! (cold glyph cache) is reported separately from scroll frames, each of which is timed with
//! the layout work that keeps ahead of it (`Layout::ensure`) plus the paint. `--select-all`
//! paints with the whole document selected. `--damage` also times an animation's tick at the
//! top: two frames of 640×360 (logical) painted again in their boxes only (the text under
//! them, the frame, the scrollbar over them) and made opaque there, against the whole window
//! painted and made opaque. `--view` times the scroll frames of the window's own view instead,
//! its images loaded from the document's directory (and with `--animations`, playing).
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

/// What paints the document: its fonts, and its selection.
struct Scene<'a> {
    fonts: &'a Fonts,
    #[cfg(feature = "clipboard")]
    select: Option<(Pos, Pos)>,
}

/// Paint the window (or only `limit` of it) with an animation's frame filling each of `frames`.
fn paint(
    px: &mut [u32],
    layout: &Layout,
    s: &Scene,
    cache: &mut GlyphCache,
    scroll: f64,
    (limit, frames): (Option<Clip>, &[Clip]),
) {
    let mut frame = Frame::new(px, W, H, W);
    let area = Clip {
        x0: 0,
        y0: 0,
        x1: W as i32,
        y1: H as i32,
    };
    let vp = Viewport { area, scroll };
    #[cfg(feature = "images")]
    if let Some(r) = limit {
        frame.set_limit(r);
    }
    #[cfg(not(feature = "images"))]
    let _ = limit;
    render::paint(
        &mut frame,
        vp,
        layout,
        s.fonts,
        cache,
        &LIGHT,
        #[cfg(feature = "clipboard")]
        s.select,
        #[cfg(feature = "find")]
        None,
    );
    for r in frames {
        let (w, h) = ((r.x1 - r.x0) as u32, (r.y1 - r.y0) as u32);
        let bm = vomd::image::Bitmap {
            width: w,
            height: h,
            data: vec![0xff80_4020; w as usize * h as usize],
        };
        frame.blit_bitmap(r.x0, r.y0, w, h, &bm);
    }
    render::paint_scrollbar(&mut frame, vp, layout.total_height(), 2.0, &LIGHT);
}

/// Scroll frames of the window's view of `path` (images, toolbar and all), its images given two
/// seconds to load first (and its animations to play, if `animations`): the frame times, sorted.
#[cfg(feature = "images")]
fn view_frames(path: &std::path::Path, src: &str, animations: bool) -> Vec<Duration> {
    use vomd::view::View;
    let base = path.parent().map(std::path::Path::to_path_buf);
    let size = (W as u32, H as u32);
    let mut v = View::with_base(
        Fonts::new().unwrap(),
        Doc::parse(src),
        None,
        size,
        2.0,
        base,
    );
    v.set_animations(animations);
    let mut px = vec![0u32; W * H];
    let frame = |v: &mut View, px: &mut [u32]| {
        let now = Instant::now();
        v.tick(now, Some(now + Duration::from_millis(4)));
        v.poll_images(now);
        v.paint(px, W, now);
    };
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        frame(&mut v, &mut px);
        std::thread::sleep(Duration::from_millis(16));
    }
    let mut times: Vec<Duration> = Vec::with_capacity(FRAMES);
    for i in 0..FRAMES {
        let t = Instant::now();
        v.set_scroll(i as f64 * 40.0);
        frame(&mut v, &mut px);
        times.push(t.elapsed());
    }
    times.sort();
    times
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let select_all = args.iter().any(|a| a == "--select-all");
    let damage = args.iter().any(|a| a == "--damage");
    let in_view = args.iter().any(|a| a == "--view");
    let animations = args.iter().any(|a| a == "--animations");
    args.retain(|a| !["--select-all", "--damage", "--view", "--animations"].contains(&a.as_str()));
    let path = args.first().expect("path");
    let src = std::fs::read_to_string(path).expect("read");
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    assert!(
        !in_view || cfg!(feature = "images"),
        "--view needs the images feature"
    );
    #[cfg(feature = "images")]
    if in_view {
        let times = view_frames(std::path::Path::new(path), &src, animations);
        println!(
            "{path}: {W}×{H} @2x, view{} | scroll frames: median {:.2} ms, p95 {:.2} ms, \
             worst {:.2} ms",
            if animations { ", animations on" } else { "" },
            ms(times[FRAMES / 2]),
            ms(times[FRAMES * 95 / 100]),
            ms(times[FRAMES - 1]),
        );
        println!(
            "paint-bench p95_ms={:.3} worst_ms={:.3}",
            ms(times[FRAMES * 95 / 100]),
            ms(times[FRAMES - 1])
        );
        return;
    }
    let _ = animations;
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
    assert!(
        !damage || cfg!(feature = "images"),
        "--damage needs the images feature"
    );
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

    let y = layout.ensure(&doc, &fonts, layout.anchor_at(0.0), H as f32);
    let t = Instant::now();
    let s = Scene {
        fonts: &fonts,
        #[cfg(feature = "clipboard")]
        select,
    };
    paint(&mut px, &layout, &s, &mut cache, y as f64, (None, &[]));
    let cold = t.elapsed();
    // Scroll frames: 40 px steps through the document, timed with the layout ahead of each.
    let mut times: Vec<Duration> = Vec::with_capacity(FRAMES);
    let max_scroll = (layout.total_height() - H as f32).max(0.0);
    for i in 0..FRAMES {
        let target = (i as f32 * 40.0).min(max_scroll);
        let t = Instant::now();
        let y = layout.ensure(&doc, &fonts, layout.anchor_at(target), H as f32);
        paint(&mut px, &layout, &s, &mut cache, y as f64, (None, &[]));
        times.push(t.elapsed());
    }
    times.sort();
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
    #[cfg(feature = "images")]
    if damage {
        let y = layout.ensure(&doc, &fonts, layout.anchor_at(0.0), H as f32) as f64;
        let boxes = [(400, 300), (400, 1200)].map(|(x0, y0)| Clip {
            x0,
            y0,
            x1: x0 + 1280,
            y1: y0 + 720,
        });
        let (mut parts, mut wholes) = (Vec::new(), Vec::new());
        for _ in 0..FRAMES {
            let t = Instant::now();
            for r in boxes {
                paint(&mut px, &layout, &s, &mut cache, y, (Some(r), &boxes));
            }
            render::opaque_rects(&mut px, W, &boxes);
            parts.push(t.elapsed());
            let t = Instant::now();
            paint(&mut px, &layout, &s, &mut cache, y, (None, &boxes));
            render::opaque(&mut px);
            wholes.push(t.elapsed());
        }
        parts.sort();
        wholes.sort();
        println!(
            "animation tick (two 1280×720 boxes): in part median {:.2} ms, p95 {:.2} ms | \
             whole median {:.2} ms, p95 {:.2} ms",
            ms(parts[FRAMES / 2]),
            ms(parts[FRAMES * 95 / 100]),
            ms(wholes[FRAMES / 2]),
            ms(wholes[FRAMES * 95 / 100]),
        );
    }
}
