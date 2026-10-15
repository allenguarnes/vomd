//! `cargo run --example render -- in.md out.png [width] [scale] [dark]`
//! Render a document headless to a PNG (the whole document, up to 6000 px tall), with the
//! embedded fonts plus a system fallback scan, for checking rendering by eye. Local
//! images are decoded; remote ones show their alt text.
use std::path::Path;
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::image::Images;
use vomd::layout::{Anchor, Layout, Params};
use vomd::render::{self, Clip, Frame, Viewport};
use vomd::text::cache::GlyphCache;
use vomd::text::fallback;
use vomd::theme::{DARK, LIGHT};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (input, output) = (&args[0], &args[1]);
    let width: f32 = args.get(2).map_or(800.0, |w| w.parse().unwrap());
    let scale: f32 = args.get(3).map_or(1.0, |s| s.parse().unwrap());
    let theme = if args.get(4).is_some_and(|t| t == "dark") {
        &DARK
    } else {
        &LIGHT
    };
    let src = std::fs::read_to_string(input).unwrap();
    let doc = Doc::parse(&src);
    let mut fonts = Fonts::new().unwrap();
    let params = Params {
        width: (width * scale).round(),
        scale,
        page_pad: 2.0,
        ..Params::default()
    };
    let mut layout = Layout::new(&doc, params);
    let mut images = Images::default();
    let dir = Path::new(input)
        .parent()
        .filter(|p| !p.as_os_str().is_empty());
    images.set_base(Some(dir.unwrap_or(Path::new(".")).to_path_buf()));
    images.attach(&doc, &mut layout);
    let measure = |layout: &mut Layout, fonts: &Fonts| {
        while !layout.idle(&doc, fonts, Instant::now() + Duration::from_secs(1)) {}
    };
    measure(&mut layout, &fonts);
    let missing = layout.take_missing();
    if !missing.is_empty() {
        for f in fallback::find_fallbacks(&missing, &|_, _| false) {
            fonts.add_fallback(&f.path, f.index);
        }
        layout.invalidate_missing(&doc);
        measure(&mut layout, &fonts);
    }
    let w = params.width as usize;
    let h = (layout.total_height().ceil() as usize).clamp(1, 6000);
    layout.ensure(&doc, &fonts, Anchor::TOP, h as f32);
    // Decode every image box, then wait until nothing has arrived for a while.
    let boxes: Vec<(u32, u32, u32)> = layout
        .geoms()
        .iter()
        .filter_map(|g| g.content.as_deref())
        .flat_map(|c| c.boxes.iter())
        .map(|b| {
            let size = |v: f32| v.round().max(1.0) as u32;
            (b.b.id, size(b.b.width), size(b.b.ascent + b.b.descent))
        })
        .collect();
    images.request(&boxes, boxes.len());
    let mut quiet = Instant::now();
    while quiet.elapsed() < Duration::from_millis(500) {
        if images.poll(&doc, &mut layout) {
            quiet = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // An image that didn't decode shows its alt text instead.
    measure(&mut layout, &fonts);
    layout.ensure(&doc, &fonts, Anchor::TOP, h as f32);
    let mut px = vec![0u32; w * h];
    let vp = Viewport {
        area: Clip {
            x0: 0,
            y0: 0,
            x1: w as i32,
            y1: h as i32,
        },
        scroll: 0.0,
    };
    let mut frame = Frame::new(&mut px, w, h, w);
    render::paint(
        &mut frame,
        vp,
        &layout,
        &fonts,
        &mut GlyphCache::default(),
        theme,
    );
    images.begin_frame();
    render::paint_images(&mut frame, vp, &layout, &mut images, theme);
    let rgb: Vec<u8> = px
        .iter()
        .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
        .collect();
    let file = std::io::BufWriter::new(std::fs::File::create(output).unwrap());
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    println!(
        "{output}: {w}×{h} ({} blocks, {} missing chars)",
        doc.blocks.len(),
        missing.len()
    );
}
