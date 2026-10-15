//! `cargo run --example render -- in.md out.png [width] [scale] [dark]`
//! Render a document headless to a PNG (the whole document, up to 6000 px tall), with the
//! embedded fonts plus a system fallback scan, for checking rendering by eye.
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
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
    render::paint(
        &mut Frame::new(&mut px, w, h, w),
        vp,
        &layout,
        &fonts,
        &mut GlyphCache::default(),
        theme,
    );
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
