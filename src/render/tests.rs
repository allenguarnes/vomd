use super::*;

const RED: Rgb = 0xff0000;
const BLUE: Rgb = 0x0000ff;
/// Fills the padding past `width` in each row, which painting must never touch.
const PAD: Rgb = 0x123456;

struct Buf {
    px: Vec<u32>,
    w: usize,
    h: usize,
    stride: usize,
}

impl Buf {
    fn new(w: usize, h: usize, stride: usize) -> Buf {
        Buf {
            px: vec![PAD; stride * h],
            w,
            h,
            stride,
        }
    }

    fn frame(&mut self) -> Frame<'_> {
        Frame::new(&mut self.px, self.w, self.h, self.stride)
    }

    fn at(&self, x: usize, y: usize) -> u32 {
        self.px[y * self.stride + x]
    }

    fn padding_untouched(&self) -> bool {
        (0..self.h).all(|y| (self.w..self.stride).all(|x| self.at(x, y) == PAD))
    }
}

#[test]
fn blend_endpoints() {
    assert_eq!(blend(BLUE, RED, 255), RED);
    assert_eq!(blend(BLUE, RED, 0), BLUE);
    assert_eq!(blend(0x000000, 0xffffff, 128), 0x808080);
    assert_eq!(blend(0xffffff, 0x000000, 128), 0x7f7f7f);
}

#[test]
fn premultiplied_over() {
    assert_eq!(over_premul(BLUE, 0xff, 0, 0, 255), RED);
    assert_eq!(over_premul(BLUE, 0, 0, 0, 0), BLUE);
    // Half-transparent red (premultiplied 128) over blue.
    assert_eq!(over_premul(BLUE, 128, 0, 0, 128), 0x80007f);
    // Additive "light" (alpha 0, color set) still adds and saturates.
    assert_eq!(over_premul(0xf00000, 0x40, 0, 0, 0), 0xff0000);
}

#[test]
fn fill_rect_clips_to_buffer_and_keeps_stride_padding() {
    let mut b = Buf::new(10, 6, 13);
    let mut f = b.frame();
    f.fill_rect(0, 0, 10, 6, BLUE);
    f.fill_rect(-5, 4, 100, 100, RED);
    f.hline(2, 4, 1, RED);
    assert!(b.padding_untouched());
    assert_eq!((b.at(0, 3), b.at(0, 4), b.at(9, 5)), (BLUE, RED, RED));
    assert_eq!(
        (b.at(1, 1), b.at(2, 1), b.at(3, 1), b.at(4, 1)),
        (BLUE, RED, RED, BLUE)
    );
}

#[test]
fn set_clip_limits_drawing() {
    let mut b = Buf::new(8, 8, 8);
    let mut f = b.frame();
    f.fill_rect(0, 0, 8, 8, BLUE);
    let prev = f.set_clip(Clip {
        x0: 2,
        y0: 2,
        x1: 100,
        y1: 4,
    });
    f.fill_rect(0, 0, 8, 8, RED);
    f.set_clip(prev);
    f.hline(0, 1, 7, RED);
    let red: Vec<(usize, usize)> = (0..8)
        .flat_map(|y| (0..8).map(move |x| (x, y)))
        .filter(|&(x, y)| b.at(x, y) == RED)
        .collect();
    assert_eq!(red.len(), 6 * 2 + 1);
    assert!(
        red.iter()
            .all(|&(x, y)| (2..4).contains(&y) && x >= 2 || (x, y) == (0, 7))
    );
}

#[test]
#[should_panic(expected = "too small")]
fn frame_rejects_short_buffer() {
    let mut px = vec![0; 10 * 4 - 1];
    Frame::new(&mut px, 10, 4, 10);
}

#[test]
#[should_panic(expected = "too small")]
fn frame_rejects_overflowing_size() {
    // (height - 1) * stride wraps to 0, which a short buffer would satisfy.
    let mut px = [0u32; 1];
    Frame::new(&mut px, 1, 3, usize::MAX / 2 + 1);
}

#[test]
#[should_panic(expected = "too small")]
fn frame_rejects_overflowing_row() {
    let mut px = [0u32; 4];
    Frame::new(&mut px, usize::MAX, 2, usize::MAX);
}

#[test]
fn frame_allows_short_last_row() {
    let mut px = vec![0; 16 * 3 + 10];
    let mut f = Frame::new(&mut px, 10, 4, 16);
    f.fill_rect(0, 0, 10, 4, RED);
    assert_eq!(px[16 * 3 + 9], RED);
}

#[test]
fn rounded_rect_corners_are_antialiased_and_symmetric() {
    let mut b = Buf::new(20, 20, 20);
    let mut f = b.frame();
    f.fill_rect(0, 0, 20, 20, 0);
    f.rounded_rect((0, 0, 20, 20), 6.0, None, 0xffffff, 255);
    // Center and edge midpoints are solid; the very corner is empty; the arc is partial.
    assert_eq!(b.at(10, 10), 0xffffff);
    assert_eq!(b.at(0, 10), 0xffffff);
    assert_eq!(b.at(10, 0), 0xffffff);
    assert_eq!(b.at(0, 0), 0);
    let arc = b.at(1, 2) & 0xff;
    assert!(arc > 0 && arc < 255, "{arc:#x}");
    for y in 0..20 {
        for x in 0..20 {
            let v = b.at(x, y);
            assert_eq!(v, b.at(19 - x, y), "mirror x at {x},{y}");
            assert_eq!(v, b.at(x, 19 - y), "mirror y at {x},{y}");
            assert_eq!(v, b.at(y, x), "transpose at {x},{y}");
        }
    }
}

#[test]
fn rounded_stroke_leaves_interior() {
    let mut b = Buf::new(16, 12, 16);
    let mut f = b.frame();
    f.fill_rect(0, 0, 16, 12, 0);
    f.rounded_rect((2, 2, 14, 10), 3.0, Some(1.0), 0xffffff, 255);
    assert_eq!(b.at(8, 2), 0xffffff, "top edge");
    assert_eq!(b.at(2, 6), 0xffffff, "left edge");
    assert_eq!(b.at(13, 6), 0xffffff, "right edge");
    assert_eq!(b.at(8, 9), 0xffffff, "bottom edge");
    assert_eq!(b.at(8, 6), 0, "interior");
    assert_eq!(b.at(3, 6), 0, "just inside the left edge");
    assert_eq!(b.at(1, 6), 0, "outside");
    assert_eq!(b.at(2, 2), 0, "rounded corner");
    // A stroke too wide for the box is a fill.
    f = b.frame();
    f.rounded_rect((0, 0, 4, 4), 0.0, Some(2.0), RED, 255);
    assert!((0..4).all(|y| (0..4).all(|x| b.at(x, y) == RED)));
}

#[test]
fn square_stroke_matches_four_fills() {
    let mut a = Buf::new(12, 12, 12);
    let mut b = Buf::new(12, 12, 12);
    a.frame().fill_rect(0, 0, 12, 12, 0);
    b.frame().fill_rect(0, 0, 12, 12, 0);
    a.frame()
        .rounded_rect((1, 2, 11, 10), 0.0, Some(2.0), RED, 255);
    let mut f = b.frame();
    f.fill_rect(1, 2, 11, 4, RED);
    f.fill_rect(1, 8, 11, 10, RED);
    f.fill_rect(1, 4, 3, 8, RED);
    f.fill_rect(9, 4, 11, 8, RED);
    assert_eq!(a.px, b.px);
}

#[test]
fn rounded_rect_alpha_and_clip() {
    let mut b = Buf::new(10, 10, 12);
    let mut f = b.frame();
    f.fill_rect(0, 0, 10, 10, 0);
    f.rounded_rect((-5, -5, 30, 30), 2.0, None, 0xffffff, 128);
    assert_eq!(b.at(5, 5), 0x808080);
    assert!(b.padding_untouched());
    let mut f = b.frame();
    f.rounded_rect((3, 3, 3, 9), 2.0, None, RED, 255);
    f.rounded_rect((3, 3, 9, 9), 2.0, None, RED, 0);
    assert_eq!(
        b.at(5, 5),
        0x808080,
        "empty and transparent rects draw nothing"
    );
}

/// Drawing far outside the frame, or with extents past i32 when subtracted, neither panics
/// nor changes the pixels.
#[test]
fn extreme_coordinates_are_clipped_not_overflowed() {
    let mut b = Buf::new(10, 10, 12);
    b.frame().fill_rect(0, 0, 10, 10, 0);
    let before = b.px.clone();
    let mut f = b.frame();
    f.hline(0, 10, i32::MAX, RED);
    f.hline(i32::MIN, i32::MAX, i32::MIN, RED);
    f.fill_rect(i32::MIN, i32::MIN, i32::MAX, i32::MIN + 1, RED);
    f.rounded_rect((i32::MIN, 20, i32::MAX, 30), 2.0, None, RED, 255);
    f.rounded_rect((0, i32::MAX - 5, 10, i32::MAX), 2.0, Some(1.0), RED, 255);
    f.rounded_rect((i32::MAX - 5, 0, i32::MAX, 10), 2.0, None, RED, 128);
    f.rounded_rect((i32::MIN, i32::MIN, -5, -5), 1e9, Some(3.0), RED, 255);
    f.blit_mask(i32::MIN, 0, 2, 2, &[255; 4], RED);
    f.blit_rgba(0, i32::MIN, 1, 1, &[255; 4]);
    f.blit_mask(i32::MAX, i32::MAX, 1, 1, &[255], RED);
    assert!(b.px == before);

    // A box spanning the whole i32 range still paints what it covers: a fill, and a rounded
    // one whose corners are miles away.
    let mut f = b.frame();
    f.rounded_rect((i32::MIN, 0, i32::MAX, 10), 2.0, None, RED, 255);
    assert!((0..10).all(|y| (0..10).all(|x| b.at(x, y) == RED)));
    b.frame().fill_rect(0, 0, 10, 10, 0);
    b.frame().rounded_rect(
        (i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        1e9,
        None,
        RED,
        255,
    );
    assert!((0..10).all(|y| (0..10).all(|x| b.at(x, y) == RED)));
    assert!(b.padding_untouched());
}

/// A stroke on an edge of a box that begins far outside the frame keeps full precision.
#[test]
fn stroke_survives_a_far_off_edge() {
    let mut b = Buf::new(10, 10, 10);
    b.frame().fill_rect(0, 0, 10, 10, 0);
    b.frame()
        .rounded_rect((-100_000_000, 0, 6, 10), 0.0, Some(1.0), 0xffffff, 255);
    // The right border is column 5; the interior and the rest are untouched.
    assert!(
        (0..10).all(|y| b.at(5, y) == 0xffffff || y == 0 || y == 9),
        "right border"
    );
    assert!((1..9).all(|y| (0..5).all(|x| b.at(x, y) == 0)), "interior");
    assert!((0..10).all(|y| (6..10).all(|x| b.at(x, y) == 0)), "outside");
    assert!((0..6).all(|x| b.at(x, 0) == 0xffffff), "top border");
    // The same stroke with the box near the frame paints identically.
    let mut near = Buf::new(10, 10, 10);
    near.frame().fill_rect(0, 0, 10, 10, 0);
    near.frame()
        .rounded_rect((-100, 0, 6, 10), 0.0, Some(1.0), 0xffffff, 255);
    assert!(b.px == near.px);
}

#[test]
fn blit_mask_clips_and_tints() {
    let mut b = Buf::new(6, 5, 8);
    b.frame().fill_rect(0, 0, 6, 5, 0);
    // 3×3 mask: full, half and empty coverage.
    let mask = [255, 128, 0, 255, 128, 0, 255, 128, 0];
    let mut f = b.frame();
    f.blit_mask(-1, 3, 3, 3, &mask, 0xffffff);
    assert_eq!(
        (b.at(0, 3), b.at(1, 3), b.at(0, 4)),
        (0x808080, 0, 0x808080)
    );
    let mut f = b.frame();
    f.blit_mask(4, 0, 3, 3, &mask, RED);
    assert_eq!((b.at(4, 0), b.at(5, 0)), (RED, 0x800000));
    // Entirely outside: nothing happens (and nothing panics).
    let mut f = b.frame();
    f.blit_mask(100, 100, 3, 3, &mask, RED);
    f.blit_mask(-3, -3, 3, 3, &mask, RED);
    f.blit_mask(i32::MAX - 1, 0, 3, 3, &mask, RED);
    assert!(b.padding_untouched());
}

#[test]
fn blit_rgba_composites_premultiplied() {
    let mut b = Buf::new(4, 1, 4);
    b.frame().fill_rect(0, 0, 4, 1, BLUE);
    let px = [255, 0, 0, 255, 128, 0, 0, 128, 0, 0, 0, 0];
    b.frame().blit_rgba(1, 0, 3, 1, &px);
    assert_eq!(&b.px, &[BLUE, RED, 0x80007f, BLUE]);
}

fn vp(h: i32, scroll: f64) -> Viewport {
    Viewport {
        area: Clip {
            x0: 0,
            y0: 0,
            x1: 400,
            y1: h,
        },
        scroll,
    }
}

#[test]
fn thumb_geometry() {
    assert_eq!(thumb(vp(500, 0.0), 400.0, 1.0), None, "fits");
    assert_eq!(thumb(vp(500, 0.0), 500.0, 1.0), None, "exactly fits");
    let top = thumb(vp(500, 0.0), 1000.0, 1.0).unwrap();
    assert_eq!((top.x0, top.x1, top.y0), (392, 398, 2));
    assert_eq!(top.y1 - top.y0, 248, "half the track for half the document");
    let bottom = thumb(vp(500, 500.0), 1000.0, 1.0).unwrap();
    assert_eq!(bottom.y1, 498);
    let over = thumb(vp(500, 9e9), 1000.0, 1.0).unwrap();
    assert_eq!(over, bottom, "scroll past the end clamps");
    // A huge document keeps a grabbable thumb, scaled for HiDPI.
    let tiny = thumb(vp(500, 0.0), 1e9, 2.0).unwrap();
    assert_eq!((tiny.y1 - tiny.y0, tiny.x1 - tiny.x0), (48, 12));
    let end = thumb(vp(500, 1e9 - 500.0), 1e9, 2.0).unwrap();
    assert_eq!(end.y1, 496);
}

/// Rounding the top and the length separately put the thumb a pixel past the margin when both
/// fractions were .5: a 100 px area, a 256 px document (length 37.5, position 58.5).
#[test]
fn thumb_with_half_pixel_fractions_stays_inside_margin() {
    let bottom = thumb(vp(100, 156.0), 256.0, 1.0).unwrap();
    assert_eq!(bottom.y1, 98, "ends at the margin, not one pixel past it");
    assert!(bottom.y0 >= 2 && bottom.y1 - bottom.y0 >= 37);
    let top = thumb(vp(100, 0.0), 256.0, 1.0).unwrap();
    assert_eq!(top.y0, 2);
}

/// For many window heights, document lengths, scales and scroll positions the thumb stays
/// between the margins, never collapses, and ends exactly at the bottom margin when scrolled to
/// the end.
#[test]
fn thumb_stays_inside_margins() {
    for scale in [1.0f32, 1.25, 1.5, 2.0, 3.0] {
        let margin = (THUMB_MARGIN * scale).round() as i32;
        for h in (40..800).step_by(7) {
            for k in [1.01, 1.5, 2.56, 3.0, 7.7, 20.0, 1000.0] {
                let total = (h as f32 * k).ceil();
                let max_scroll = f64::from(total) - f64::from(h);
                for f in [0.0, 0.1, 0.25, 0.333, 0.5, 0.75, 0.999, 1.0, 2.0] {
                    let t = thumb(vp(h, max_scroll * f), total, scale).unwrap();
                    let ctx = format!("h {h} total {total} scale {scale} at {f}: {t:?}");
                    assert!(t.y0 >= margin, "{ctx}");
                    assert!(t.y1 <= h - margin, "{ctx}");
                    assert!(t.y1 > t.y0, "{ctx}");
                }
                let end = thumb(vp(h, max_scroll), total, scale).unwrap();
                assert_eq!(end.y1, h - margin, "h {h} total {total} scale {scale}");
                let start = thumb(vp(h, 0.0), total, scale).unwrap();
                assert_eq!(start.y0, margin, "h {h} total {total} scale {scale}");
            }
        }
    }
}

#[test]
fn scrollbar_paints_inside_area() {
    let mut b = Buf::new(400, 100, 400);
    b.frame().fill_rect(0, 0, 400, 100, 0);
    let mut v = vp(100, 0.0);
    v.area.y0 = 20;
    paint_scrollbar(&mut b.frame(), v, 1000.0, 1.0, &crate::theme::LIGHT);
    assert!((0..20).all(|y| (0..400).all(|x| b.at(x, y) == 0)));
    assert!(b.at(395, 40) != 0);
    assert!((0..100).all(|y| b.at(390, y) == 0 && b.at(399, y) == 0));
}

fn area(x0: i32, y0: i32, x1: i32, y1: i32) -> Viewport {
    Viewport {
        area: Clip { x0, y0, x1, y1 },
        scroll: 0.0,
    }
}

/// Areas reaching past i32 when subtracted still report their true height.
#[test]
fn viewport_height_does_not_overflow() {
    assert_eq!(area(0, i32::MIN, 1, i32::MAX).height(), 4_294_967_295.0);
    assert_eq!(area(0, i32::MAX, 1, i32::MIN).height(), 0.0, "inverted");
    assert_eq!(area(0, -5, 1, 5).height(), 10.0);
    assert_eq!(area(0, 7, 1, 7).height(), 0.0);
}

/// The thumb of an area at the i32 limits saturates instead of overflowing, and what lies in
/// the frame is placed as it would be near the origin.
#[test]
fn thumb_survives_extreme_areas() {
    // Entirely off to the left, one pixel wide.
    let t = thumb(area(i32::MIN, 0, i32::MIN + 1, 100), 1000.0, 1.0).unwrap();
    assert!(t.x0 <= t.x1 && t.x1 < 0, "{t:?}");
    // Off to the right, and a tall area at the bottom limit.
    let t = thumb(area(i32::MAX - 1, 0, i32::MAX, 100), 1000.0, 1.0).unwrap();
    assert_eq!(t.x1, i32::MAX - 2);
    let t = thumb(area(0, i32::MAX - 100, 400, i32::MAX), 1000.0, 1.0).unwrap();
    assert_eq!(t.y0, i32::MAX - 98);
    assert!(t.y0 <= t.y1);
    // A huge scale or document, and an area spanning the whole range.
    let wide = thumb(area(0, 0, 1000, 1000), 1e30, 1e9);
    assert!(wide.is_none(), "margins swallow the track");
    let t = thumb(area(i32::MIN, i32::MIN, i32::MAX, i32::MAX), 1e12, 1.0).unwrap();
    assert!(t.y0 < t.y1 && t.x0 < t.x1);
    let t = thumb(area(0, 0, 400, 100), 1000.0, 3e7);
    assert!(t.is_none_or(|t| t.x0 <= t.x1));
    // Garbage in a float: no thumb for a NaN document, the top for a NaN scroll.
    assert_eq!(thumb(vp(100, 0.0), f32::NAN, 1.0), None);
    let v = Viewport {
        scroll: f64::NAN,
        ..vp(100, 0.0)
    };
    assert_eq!(thumb(v, 1000.0, 1.0), thumb(vp(100, 0.0), 1000.0, 1.0));
    let v = Viewport {
        scroll: f64::INFINITY,
        ..vp(100, 0.0)
    };
    assert_eq!(
        thumb(v, f32::INFINITY, 1.0),
        thumb(vp(100, 0.0), f32::INFINITY, 1.0)
    );
}

/// Scrollbars of areas far off the frame, or covering it, neither panic nor stray.
#[test]
fn scrollbar_survives_extreme_areas() {
    let mut b = Buf::new(20, 20, 20);
    b.frame().fill_rect(0, 0, 20, 20, 0);
    let before = b.px.clone();
    let theme = &crate::theme::LIGHT;
    for (x0, y0, x1, y1) in [
        (i32::MIN, 0, i32::MIN + 1, 100),
        (i32::MAX - 1, 0, i32::MAX, 100),
        (0, i32::MIN, 20, i32::MIN + 100),
        (0, i32::MAX - 100, 20, i32::MAX),
    ] {
        paint_scrollbar(&mut b.frame(), area(x0, y0, x1, y1), 1000.0, 1.0, theme);
    }
    assert!(b.px == before);
    // Covering the whole i32 range, the thumb sits at the far right edge: off the frame.
    paint_scrollbar(
        &mut b.frame(),
        area(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        1e12,
        1.0,
        theme,
    );
    assert!(b.px == before);
}

#[test]
fn scrollbar_fade_survives_the_end_of_time() {
    // The latest instant this platform can represent, found by bisecting `checked_add`.
    let t0 = Instant::now();
    let (mut lo, mut hi) = (0u64, u64::MAX);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if t0.checked_add(Duration::from_secs(mid)).is_some() {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let end = t0 + Duration::from_secs(lo);
    let mut f = ScrollbarFade::default();
    f.poke(end);
    assert!(!f.visible(end), "no room for a deadline");
    assert!(f.expire(end));
}

#[test]
fn scrollbar_fade_is_one_deadline() {
    let t0 = Instant::now();
    let mut f = ScrollbarFade::default();
    assert!(!f.visible(t0) && f.deadline().is_none());
    f.poke(t0);
    assert!(f.visible(t0 + Duration::from_millis(999)));
    assert!(!f.expire(t0 + Duration::from_millis(500)));
    f.poke(t0 + Duration::from_millis(500));
    assert_eq!(f.deadline(), Some(t0 + Duration::from_millis(1500)));
    assert!(f.visible(t0 + Duration::from_millis(1200)));
    assert!(f.expire(t0 + Duration::from_millis(1500)));
    assert!(!f.visible(t0 + Duration::from_millis(1500)));
    assert!(f.deadline().is_none(), "no more wakeups once hidden");
    assert!(!f.expire(t0 + Duration::from_secs(5)));
}

#[cfg(feature = "embed-fonts")]
mod doc {
    use super::*;
    use crate::doc::Doc;
    use crate::layout::{Anchor, Params};
    use crate::theme::{DARK, LIGHT};

    /// Render `src` scrolled to `off` pixels below the top of block `at`.
    fn render(src: &str, w: usize, h: usize, (at, off): (usize, f64), theme: &Theme) -> Buf {
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse(src);
        let mut layout = Layout::new(
            &doc,
            Params {
                width: w as f32,
                ..Params::default()
            },
        );
        let anchor = Anchor {
            block: at as u32,
            frac: 0.0,
        };
        layout.ensure(&doc, &fonts, anchor, h as f32);
        let s = layout.top(at) + off;
        let mut b = Buf::new(w, h + 10, w + 3);
        let v = Viewport {
            area: Clip {
                x0: 0,
                y0: 10,
                x1: w as i32,
                y1: h as i32 + 10,
            },
            scroll: s,
        };
        paint(
            &mut b.frame(),
            v,
            &layout,
            &fonts,
            &mut GlyphCache::default(),
            theme,
        );
        b
    }

    fn colors(b: &Buf, rows: std::ops::Range<usize>) -> std::collections::HashSet<u32> {
        rows.flat_map(|y| (0..b.w).map(move |x| (x, y)))
            .map(|(x, y)| b.at(x, y))
            .collect()
    }

    #[test]
    fn paints_text_and_keeps_outside_untouched() {
        let b = render(
            "# Title\n\nSome *text* and `code`.",
            600,
            200,
            (0, 0.0),
            &LIGHT,
        );
        assert!(b.padding_untouched());
        assert!(
            (0..10).all(|y| (0..600).all(|x| b.at(x, y) == PAD)),
            "above the area"
        );
        let c = colors(&b, 10..210);
        assert!(c.contains(&LIGHT.bg));
        assert!(c.contains(&LIGHT.text), "solid glyph pixels");
        assert!(c.contains(&LIGHT.code_bg));
        assert!(c.len() > 10, "anti-aliased edges");
    }

    #[test]
    fn dark_theme_resolves_roles() {
        let b = render("[link](x) text\n\n---", 600, 200, (0, 0.0), &DARK);
        let c = colors(&b, 10..210);
        assert!(c.contains(&DARK.bg) && c.contains(&DARK.text) && c.contains(&DARK.link));
        assert!(c.contains(&DARK.border));
        assert!(!c.contains(&LIGHT.bg));
    }

    /// Identical blocks look identical however far down the document they are: block tops and
    /// the scroll position stay f64 until painting.
    #[test]
    fn deep_scroll_paints_like_shallow() {
        let src = "Paragraph with a few words.\n\n".repeat(400_000);
        let near = render(&src, 500, 150, (10, 3.0), &LIGHT);
        let far = render(&src, 500, 150, (399_900, 3.0), &LIGHT);
        assert!(colors(&near, 10..160).contains(&LIGHT.text));
        assert!(near.px == far.px, "far blocks drift against near ones");
    }

    /// Blocks straddling the top of the viewport stay painted at fractional offsets far down a
    /// tall document, where an f32 scroll position can't tell them from their neighbors.
    #[test]
    fn deep_scroll_keeps_block_at_the_top_edge() {
        let src = "```\na\n```\n\n".repeat(60_000);
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse(&src);
        let mut layout = Layout::new(
            &doc,
            Params {
                width: 600.0,
                scale: 6.0,
                ..Params::default()
            },
        );
        let mut cache = GlyphCache::default();
        for k in [59_980, 59_981, 59_990] {
            // The first rows of the viewport show the end of block k - 1.
            let anchor = Anchor {
                block: k as u32 - 1,
                frac: 0.0,
            };
            layout.ensure(&doc, &fonts, anchor, 150.0);
            let scroll = layout.top(k) - 0.75;
            let area = Clip {
                x0: 0,
                y0: 0,
                x1: 600,
                y1: 150,
            };
            let range = layout.range_exact(scroll, scroll + 150.0);
            assert_eq!(range.start, k - 1, "first overlapping block");
            let mut b = Buf::new(600, 150, 600);
            let v = Viewport { area, scroll };
            paint(&mut b.frame(), v, &layout, &fonts, &mut cache, &LIGHT);
            assert_eq!(
                b.at(300, 0),
                LIGHT.code_bg,
                "block {} at the top row",
                k - 1
            );
        }
    }

    /// An area reaching past i32 on either side paints like the same frame rows of a normal one.
    #[test]
    fn huge_area_paints_like_a_normal_one() {
        let src = "# Title\n\nSome *text* and `code`.\n\n---\n\nMore text.";
        let fonts = Fonts::new().unwrap();
        let doc = Doc::parse(src);
        let mut layout = Layout::new(
            &doc,
            Params {
                width: 300.0,
                ..Params::default()
            },
        );
        layout.ensure(&doc, &fonts, Anchor::TOP, 120.0);
        let mut cache = GlyphCache::default();
        let mut paint_with = |area: Clip, scroll: f64| {
            let mut b = Buf::new(300, 120, 300);
            let v = Viewport { area, scroll };
            paint(&mut b.frame(), v, &layout, &fonts, &mut cache, &LIGHT);
            b
        };
        let normal = paint_with(
            Clip {
                x0: 0,
                y0: 0,
                x1: 300,
                y1: 120,
            },
            0.0,
        );
        assert!(colors(&normal, 0..120).contains(&LIGHT.text));
        // The document's top is at frame row 0 when `scroll` is `area.y0` less than it.
        let min = f64::from(i32::MIN);
        let tall = paint_with(
            Clip {
                x0: 0,
                y0: i32::MIN,
                x1: 300,
                y1: i32::MAX,
            },
            min,
        );
        assert!(tall.px == normal.px, "area spanning i32");
        let below = paint_with(
            Clip {
                x0: 0,
                y0: 0,
                x1: 300,
                y1: i32::MAX,
            },
            0.0,
        );
        assert!(below.px == normal.px, "area extending past the frame");
        // Entirely off screen, or garbage scrolls: nothing to paint, nothing to panic.
        let blank = paint_with(
            Clip {
                x0: 0,
                y0: i32::MIN,
                x1: 1,
                y1: i32::MIN + 5,
            },
            0.0,
        );
        assert!(blank.px.iter().all(|&p| p == PAD));
        for scroll in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
            let mut b = Buf::new(300, 120, 300);
            let area = Clip {
                x0: 0,
                y0: 0,
                x1: 300,
                y1: 120,
            };
            let v = Viewport { area, scroll };
            paint(&mut b.frame(), v, &layout, &fonts, &mut cache, &LIGHT);
            paint_scrollbar(&mut b.frame(), v, 1000.0, 1.0, &LIGHT);
        }
    }

    #[test]
    fn scroll_moves_content_up() {
        let src = "Line one\n\nLine two\n\nLine three\n\nLine four";
        let a = render(src, 400, 300, (0, 0.0), &LIGHT);
        let b = render(src, 400, 300, (0, 20.0), &LIGHT);
        // Row y + 20 before scrolling equals row y after.
        for y in 10..250 {
            for x in 0..400 {
                assert_eq!(a.at(x, y + 20), b.at(x, y), "at {x},{y}");
            }
        }
    }
}
