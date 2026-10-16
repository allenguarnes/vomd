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

fuzz_target!(|data: &[u8]| {
    // The first byte picks the width (narrow ones wrap everything) and the scale.
    let Some((&b, rest)) = data.split_first() else {
        return;
    };
    let src = String::from_utf8_lossy(rest);
    let doc = Doc::parse(&src);
    let params = Params {
        width: 8.0 + f32::from(b >> 1) * 6.0,
        scale: if b & 1 == 0 { 1.0 } else { 2.0 },
        ..Params::default()
    };
    FONTS.with(|fonts| {
        let mut layout = Layout::new(&doc, params);
        while !layout.idle(&doc, fonts, Instant::now() + Duration::from_secs(1)) {}
        let scroll = layout.ensure(&doc, fonts, layout.anchor_at(0.0), H as f32);
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
            render::paint(&mut frame, vp, &layout, fonts, cache, &LIGHT);
        });
        render::paint_scrollbar(&mut frame, vp, layout.total_height(), params.scale, &LIGHT);
    });
});
