use super::*;

/// A viewer of a `w` × `h` raster (or an SVG) in a 1000 × 800 window at scale 1.
fn viewer(w: u32, h: u32, vector: bool) -> Viewer {
    Viewer::new(1, (w, h), vector, (1000.0, 800.0), 1.0)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6 * a.abs().max(1.0)
}

/// Every place the transform may be in: a finite zoom within its range, the image centered
/// where it is smaller than the window and covering it where it is larger.
fn sound(v: &Viewer) {
    let (lo, hi) = v.range();
    assert!(v.tf.s.is_finite() && v.tf.s >= lo * (1.0 - 1e-9) && v.tf.s <= hi * (1.0 + 1e-9));
    let (w, h) = v.shown();
    for (pos, size, area) in [(v.tf.x, w, v.area.0), (v.tf.y, h, v.area.1)] {
        assert!(pos.is_finite());
        if size <= area {
            assert!(
                close(pos, (area - size) / 2.0),
                "centered: {pos} {size} {area}"
            );
        } else {
            assert!(
                pos <= 1e-6 && pos + size >= area - 1e-6,
                "no gutter: {pos} {size} {area}"
            );
        }
    }
}

/// Fit is the window less its margin, by the side that fills first; a raster opens at Fit or
/// 100%, whichever is smaller, an SVG at Fit; a window of one pixel, or of none, still fits.
#[test]
fn fit_follows_the_window_and_the_kind_of_image() {
    let wide = viewer(4000, 1000, false);
    assert!(close(wide.fit(), (1000.0 - 32.0) / 4000.0));
    assert!(close(wide.tf.s, wide.fit()) && wide.at_fit());
    let tall = viewer(1000, 4000, false);
    assert!(close(tall.fit(), (800.0 - 32.0) / 4000.0));
    let tiny = viewer(20, 10, false);
    assert!(close(tiny.tf.s, 1.0), "a small raster isn't blown up");
    assert!(tiny.at_fit());
    let svg = viewer(20, 10, true);
    assert!(
        close(svg.tf.s, (1000.0 - 32.0) / 20.0),
        "an SVG fills the window"
    );
    let huge = viewer(65_535, 65_535, false);
    assert!(close(huge.fit(), (800.0 - 32.0) / 65_535.0));
    for area in [(1.0, 1.0), (0.0, 0.0), (f64::NAN, -5.0)] {
        let mut v = viewer(300, 200, false);
        v.set_area(area, 1.0);
        assert!(v.fit() > 0.0 && v.fit().is_finite());
        sound(&v);
    }
    for v in [&wide, &tall, &tiny, &svg, &huge] {
        sound(v);
    }
}

/// 100% is the image's natural size in logical pixels: twice its pixels at scale 2.
#[test]
fn a_hundred_percent_follows_the_scale() {
    let mut v = Viewer::new(1, (300, 200), false, (2000.0, 1600.0), 2.0);
    assert!(close(v.hundred(), 2.0));
    assert!(close(v.tf.s, 2.0), "fits at 100%");
    assert_eq!(v.rect().2, 600);
    // A scale change keeps the zoom relative to 100%: at 400% it stays 400%.
    v.zoom_about((1000.0, 800.0), 2.0);
    assert!(close(v.tf.s, 4.0));
    v.set_area((2000.0, 1600.0), 1.0);
    assert!(close(v.tf.s, 2.0));
    assert_eq!(v.label(Instant::now(), None, None), "200%");
    sound(&v);
}

/// A zoom keeps the image point under the pointer under it, and in then out comes back; at
/// the range's ends it stops.
#[test]
fn a_zoom_keeps_the_point_under_the_pointer() {
    let mut v = viewer(4000, 3000, false);
    let p = (700.0, 300.0);
    let under = |v: &Viewer| ((p.0 - v.tf.x) / v.tf.s, (p.1 - v.tf.y) / v.tf.s);
    let before = under(&v);
    assert!(v.zoom_about(p, 3.0));
    let after = under(&v);
    assert!(close(before.0, after.0) && close(before.1, after.1));
    let start = v.tf;
    v.zoom_about(p, STEP);
    v.zoom_about(p, 1.0 / STEP);
    assert!(close(v.tf.s, start.s) && close(v.tf.x, start.x) && close(v.tf.y, start.y));
    // The ends: 16 × 100%, and Fit (smaller than 100% here).
    v.zoom_about(p, 1e9);
    assert!(close(v.tf.s, 16.0));
    assert!(!v.zoom_about(p, 2.0), "no further");
    v.zoom_about(p, 1e-9);
    assert!(close(v.tf.s, v.fit()));
    sound(&v);
    // A pinch's continuous delta, and nonsense from it.
    assert!(v.zoom_pixels(p, 400.0));
    assert!(close(v.tf.s, v.fit() * std::f64::consts::E));
    for bad in [f64::NAN, f64::INFINITY, -f64::INFINITY] {
        let tf = v.tf;
        assert!(!v.zoom_pixels(p, bad));
        assert!(!v.zoom_about((bad, 0.0), 2.0));
        assert!(!v.zoom_about(p, bad));
        assert_eq!(v.tf, tf);
    }
}

/// The image is centered along a side where it is smaller than the window, and leaves no
/// gutter where it is larger, whatever it is moved by.
#[test]
fn moving_the_image_keeps_it_in_the_window() {
    let mut v = viewer(4000, 300, false);
    v.toggle((500.0, 400.0));
    assert!(close(v.tf.s, 1.0));
    assert!(v.wider() && v.pannable());
    assert!(v.pan(1e9, 1e9), "to the left edge");
    assert!(close(v.tf.x, 0.0) && close(v.tf.y, (800.0 - 300.0) / 2.0));
    assert!(!v.pan(10.0, 0.0), "no further");
    v.pan(-1e9, 0.0);
    assert!(close(v.tf.x, 1000.0 - 4000.0));
    v.pan(f64::NAN, f64::INFINITY);
    sound(&v);
    // An arrow key moves the view, so the image the other way.
    let x = v.tf.x;
    v.pan_lines(-1.0, 0.0);
    assert!(close(v.tf.x, x + 40.0));
    let narrow = viewer(300, 200, false);
    assert!(!narrow.wider() && !narrow.pannable());
}

/// Fit ↔ 100%: about the pointer, back to Fit centered; one image whose Fit is 100% (a small
/// raster) has nothing to switch to, an SVG smaller than the window goes from Fit to 100%.
#[test]
fn a_click_switches_between_fit_and_a_hundred_percent() {
    let mut v = viewer(4000, 3000, false);
    assert!(v.toggle((100.0, 100.0)));
    assert!(close(v.tf.s, 1.0) && v.mode == Mode::Free);
    assert_eq!(v.label(Instant::now(), None, None), "100%");
    assert!(v.toggle((100.0, 100.0)));
    assert!(v.at_fit() && v.mode == Mode::Fit);
    assert_eq!(v.label(Instant::now(), Some((3, 7)), None), "Fit · 3 / 7");
    assert_eq!(v.label(Instant::now(), Some((1, 1)), None), "Fit");
    assert_eq!(
        v.label(Instant::now(), Some((3, 7)), Some("Space plays")),
        "Fit · 3 / 7 · Space plays"
    );
    let mut small = viewer(300, 200, false);
    assert!(!small.toggle((500.0, 400.0)));
    let mut svg = viewer(100, 50, true);
    assert!(svg.toggle((500.0, 400.0)));
    assert!(close(svg.tf.s, 1.0));
}

/// At Fit a resize fits again; zoomed, it keeps the zoom and the image point at the center.
#[test]
fn a_resize_follows_the_mode() {
    let mut v = viewer(4000, 3000, false);
    v.set_area((2000.0, 1600.0), 1.0);
    assert!(v.at_fit() && close(v.tf.s, (2000.0 - 32.0) / 4000.0));
    v.toggle((1000.0, 800.0));
    v.pan(-300.0, -200.0);
    let center = |v: &Viewer| {
        (
            (v.area.0 / 2.0 - v.tf.x) / v.tf.s,
            (v.area.1 / 2.0 - v.tf.y) / v.tf.s,
        )
    };
    let c = center(&v);
    v.set_area((1200.0, 900.0), 1.0);
    assert!(close(v.tf.s, 1.0));
    let after = center(&v);
    assert!(close(c.0, after.0) && close(c.1, after.1));
    sound(&v);
}

/// The detail is the image at the zoom, never more pixels than a raster has, and within 16 MP
/// (and a decode's sides); it is decoded again only when it would be enlarged by more than a
/// tenth or shrunk by more than 1.5×.
#[test]
fn the_detail_is_capped_and_decoded_again_only_when_it_matters() {
    let mut v = viewer(4000, 3000, false);
    assert_eq!(v.target(), (968, 726));
    v.zoom_about((0.0, 0.0), 1e9);
    assert_eq!(v.target(), (4000, 3000), "a raster's own size at most");
    let mut big = viewer(6000, 4000, false);
    big.zoom_about((0.0, 0.0), 1e9);
    let (w, h) = big.target();
    assert!(f64::from(w) * f64::from(h) <= DETAIL_PIXELS + 2.0 * f64::from(w + h));
    assert!((f64::from(w) / f64::from(h) - 1.5).abs() < 0.01);
    assert!(big.over_cap() && !v.over_cap());
    let mut svg = viewer(1000, 1000, true);
    svg.zoom_about((0.0, 0.0), 1e9);
    let (w, h) = svg.target();
    assert!(w <= 4096 && h <= 4096 && w >= 4095, "{w} {h}");
    assert!(svg.over_cap(), "an SVG zoomed past the cap");
    let mut line = viewer(65_535, 2, true);
    line.zoom_about((0.0, 0.0), 1e9);
    assert!(line.target().0 <= 16_384);
    // A wide or tall raster zoomed past its own size asks for no side over a decode's, so the
    // decode gives what was asked and nothing is asked again (the review's 32,768 × 256 PNG).
    for (w, h) in [(32_768, 256), (256, 32_768), (20_000, 900)] {
        let mut r = viewer(w, h, false);
        for factor in [2.0, 1e9] {
            r.zoom_about((0.0, 0.0), factor);
            let t = r.target();
            assert!(t.0 <= 16_384 && t.1 <= 16_384, "{w}×{h}: {t:?}");
            let got = crate::image::clamp_box(t.0, t.1);
            assert!(
                !Viewer::redo(got, r.target()),
                "{w}×{h}: settles at {got:?}"
            );
        }
    }
    assert!(!Viewer::redo((1000, 750), (1090, 817)));
    assert!(Viewer::redo((1000, 750), (1120, 840)));
    assert!(!Viewer::redo((1000, 750), (700, 525)));
    assert!(Viewer::redo((1000, 750), (600, 450)));
}

/// Full resolution decodes the part of the image on screen at its own resolution, no more
/// pixels than the window, once the detail is there; off, the part goes.
#[test]
fn full_resolution_decodes_the_part_on_screen() {
    let now = Instant::now();
    let mut v = viewer(8000, 6000, false);
    v.set_full(true);
    assert_eq!(v.crop_wanted(), None, "at Fit the detail holds it");
    // The first decode is the detail, then (it came) the part.
    v.toggle((500.0, 400.0));
    let want = v.next_decode().unwrap();
    assert_eq!(want.1, None);
    v.asked_for(want, 1, now);
    assert_eq!(v.next_decode(), None, "asked for already");
    let detail = Bitmap {
        width: want.0.0,
        height: want.0.1,
        data: vec![0; (want.0.0 * want.0.1) as usize],
    };
    assert!(!v.took(9, None), "not the one asked for");
    assert!(v.took(1, Some(detail)));
    let (crop, size) = v.next_decode().map(|(s, c)| (c.unwrap(), s)).unwrap();
    assert_eq!(crop.full, (8000, 6000));
    // The window at 100% (and the pixel a fraction of one is cut from).
    assert!(
        (1000..=1001).contains(&size.0) && (800..=801).contains(&size.1),
        "{size:?}"
    );
    let x0 = (-v.tf.x).floor() as u32;
    assert_eq!(crop.at.0, x0);
    v.asked_for((size, Some(crop)), 2, now);
    let part = Bitmap {
        width: size.0,
        height: size.1,
        data: vec![0; (size.0 * size.1) as usize],
    };
    assert!(v.took(2, Some(part)));
    assert_eq!(v.next_decode(), None, "it is there");
    // Moved: another part. At the image's edge, the part is cut to it.
    v.pan(-1e9, -1e9);
    let (s, c) = v.next_decode().unwrap();
    let c = c.unwrap();
    assert_eq!((c.at.0 + s.0, c.at.1 + s.1), (8000, 6000));
    // Zoomed out a little: the part at that zoom, still within the window.
    v.zoom_about((500.0, 400.0), 0.6);
    let (s, c) = v.next_decode().unwrap();
    assert_eq!(c.unwrap().full, (4800, 3600));
    assert!(s.0 <= 1001 && s.1 <= 801);
    v.set_full(false);
    assert!(v.crop.is_none());
    assert_eq!(
        v.next_decode(),
        None,
        "the detail holds what this zoom needs"
    );
}

/// A decode that failed isn't asked for again; one taking long says "Loading…" until it is
/// done, with one deadline for it; settled, nothing is due.
#[test]
fn decodes_and_the_label_need_no_timer_once_settled() {
    let t = Instant::now();
    let mut v = viewer(4000, 3000, false);
    v.changed(t);
    assert_eq!(v.deadline(), Some(t + SETTLE));
    let want = v.next_decode().unwrap();
    v.asked_for(want, 5, t);
    assert!(!v.loading(t));
    let (repaint, settle) = v.expire(t + SETTLE);
    assert!(!repaint && settle);
    assert_eq!(v.deadline(), Some(t + SLOW));
    assert!(v.expire(t + SLOW).0, "Loading… shows");
    assert!(v.loading(t + SLOW) && v.label(t + SLOW, None, None) == "Loading…");
    assert_eq!(
        v.deadline(),
        Some(t + LABEL),
        "the label's time, not Loading…'s again"
    );
    assert!(v.took(5, None));
    assert_eq!(v.next_decode(), None, "failed: not again");
    assert!(v.expire(t + LABEL).0);
    assert_eq!(v.deadline(), None);
    assert!(!v.label_shows(t + LABEL));
}

/// A press that moves past the slop drags the image (and is no click); the second click of the
/// double click that opened the viewer is ignored.
#[test]
fn a_drag_moves_the_image_and_a_double_click_does_nothing() {
    let t = Instant::now();
    let mut v = viewer(4000, 3000, false);
    v.toggle((500.0, 400.0));
    let x = v.tf.x;
    v.press(500.0, 400.0, t);
    assert!(!v.drag(502.0, 401.0), "within the slop");
    assert!(!v.dragging());
    assert!(v.drag(450.0, 400.0));
    assert!(v.dragging() && close(v.tf.x, x - 50.0));
    v.press = None;
    v.opened = Some((t, (500.0, 400.0)));
    v.press(501.0, 400.0, t + Duration::from_millis(200));
    assert!(v.press.unwrap().ignored);
    v.press(501.0, 400.0, t + Duration::from_millis(400));
    assert!(!v.press.unwrap().ignored, "once");
    v.opened = Some((t, (500.0, 400.0)));
    v.press(501.0, 400.0, t + Duration::from_millis(900));
    assert!(!v.press.unwrap().ignored, "too late");
}

/// A click or `0` away from Fit goes back to Fit, even for a small raster (where Fit is 100%):
/// it doesn't close, or do nothing, while the label says 300%.
#[test]
fn a_zoomed_in_small_image_goes_back_to_fit() {
    let mut v = Viewer::new(1, (100, 80), false, (800.0, 600.0), 1.0);
    assert!(v.at_fit());
    assert!(!v.toggle((400.0, 300.0)), "Fit is 100%: nowhere to go");
    assert!(v.zoom_steps(5.0));
    assert!(!v.at_fit());
    assert!(v.toggle((400.0, 300.0)));
    assert!(v.at_fit() && v.mode == Mode::Fit);
    assert!(!v.toggle((400.0, 300.0)));
}

/// The part held goes once none is wanted (zoomed out to where the detail holds it), and a
/// part that comes back after that is not kept.
#[test]
fn the_part_goes_when_none_is_wanted() {
    let now = Instant::now();
    let bm = |(w, h): (u32, u32)| Bitmap {
        width: w,
        height: h,
        data: vec![0; (w * h) as usize],
    };
    let mut v = viewer(8000, 6000, false);
    v.set_full(true);
    v.toggle((500.0, 400.0));
    let want = v.next_decode().unwrap();
    v.asked_for(want, 1, now);
    assert!(v.took(1, Some(bm(want.0))));
    let (size, crop) = v.next_decode().map(|(s, c)| (s, c.unwrap())).unwrap();
    v.asked_for((size, Some(crop)), 2, now);
    assert!(v.took(2, Some(bm(size))));
    assert!(v.crop.is_some() && v.next_decode().is_none());
    // A part is asked for, and the zoom goes back to Fit before it comes.
    v.pan(-1e9, -1e9);
    let (size, crop) = v.next_decode().map(|(s, c)| (s, c.unwrap())).unwrap();
    v.asked_for((size, Some(crop)), 3, now);
    v.toggle((500.0, 400.0));
    assert!(v.crop_wanted().is_none());
    let _ = v.next_decode();
    assert!(v.crop.is_none(), "dropped");
    v.took(3, Some(bm(size)));
    assert!(v.crop.is_none(), "not kept");
}
