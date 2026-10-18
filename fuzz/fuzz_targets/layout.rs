//! Bytes → Doc IR → layout at a width → paint of the first screen: never panics.
#![no_main]

use std::cell::RefCell;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::layout::{Layout, Params};
use vomd::render::{self, Clip, Frame, Viewport};
use vomd::text::cache::GlyphCache;
use vomd::theme::LIGHT;

const W: usize = 320;
const H: usize = 240;

thread_local! {
    static FONTS: Fonts = Fonts::new().expect("embedded fonts");
    static CACHE: RefCell<GlyphCache> = RefCell::new(GlyphCache::default());
}

fn paint(layout: &Layout, fonts: &Fonts, scroll: f32, scale: f32) {
    let mut px = vec![0u32; W * H];
    let mut frame = Frame::new(&mut px, W, H, W);
    let area = Clip {
        x0: 0,
        y0: 0,
        x1: W as i32,
        y1: H as i32,
    };
    let vp = Viewport {
        area,
        scroll: f64::from(scroll),
    };
    CACHE.with(|cache| {
        let cache = &mut cache.borrow_mut();
        render::paint(&mut frame, vp, layout, fonts, cache, &LIGHT);
    });
    render::paint_scrollbar(&mut frame, vp, layout.total_height(), scale, &LIGHT);
}

/// Page padding in em.
const PADS: [f32; 4] = [0.0, 0.5, 2.0, 4.0];

fuzz_target!(|data: &[u8]| {
    // The first four bytes pick the width (narrow ones wrap everything, wide ones pass the
    // line width's cap), the scale, the wide layout and the page padding, and the same again
    // for the second parameters the document is laid out in once only its first screen is
    // measured.
    let Some((&[b0, b1, b2, b3], rest)) = data.split_first_chunk() else {
        return;
    };
    let b = u16::from_le_bytes([b0, b1]);
    let src = String::from_utf8_lossy(rest);
    let doc = Doc::parse(&src);
    let params = Params {
        width: 8.0 + f32::from((b >> 2) % 3000),
        scale: if b & 1 == 0 { 1.0 } else { 2.0 },
        wide: b & 2 != 0,
        page_pad: PADS[usize::from(b3 >> 3) & 3],
        ..Params::default()
    };
    let again = Params {
        width: 8.0 + f32::from(b2 >> 1) * 24.0,
        scale: if b3 & 1 == 0 { 1.0 } else { 2.0 },
        wide: b2 & 1 != 0,
        page_pad: PADS[usize::from(b3 >> 1) & 3],
        ..params
    };
    FONTS.with(|fonts| {
        let mut layout = Layout::new(&doc, params);
        let scroll = layout.ensure(&doc, fonts, layout.anchor_at(0.0), H as f32);
        paint(&layout, fonts, scroll, params.scale);
        let unmeasured: Vec<bool> = layout.geoms().iter().map(|g| !g.measured).collect();
        layout.set_params(again);
        // A block never measured has the height a fresh layout estimates for it.
        let fresh = Layout::new(&doc, again);
        for (i, (g, f)) in layout.geoms().iter().zip(fresh.geoms()).enumerate() {
            assert!(!unmeasured[i] || g.h == f.h, "block {i}");
        }
        while !layout.idle(&doc, fonts, Instant::now() + Duration::from_secs(1)) {}
        let scroll = layout.ensure(&doc, fonts, layout.anchor_at(0.0), H as f32);
        paint(&layout, fonts, scroll, params.scale);
    });
});
