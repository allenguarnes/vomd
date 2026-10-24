// Copyright 2023 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

/// Fits the current rect into the specified bounds.
pub fn fit_to_rect(
    r: tiny_skia::IntRect,
    bounds: tiny_skia::IntRect,
) -> Option<tiny_skia::IntRect> {
    let mut left = r.left();
    if left < bounds.left() {
        left = bounds.left();
    }

    let mut top = r.top();
    if top < bounds.top() {
        top = bounds.top();
    }

    let mut right = r.right();
    if right > bounds.right() {
        right = bounds.right();
    }

    let mut bottom = r.bottom();
    if bottom > bounds.bottom() {
        bottom = bounds.bottom();
    }

    tiny_skia::IntRect::from_ltrb(left, top, right, bottom)
}

/// `NonZeroRect::to_int_rect`, which unwraps the rectangle, as an option: a position or a size
/// far out leaves none (VOMD).
pub fn int_rect(r: tiny_skia::NonZeroRect) -> Option<tiny_skia::IntRect> {
    tiny_skia::IntRect::from_xywh(
        r.x().floor() as i32,
        r.y().floor() as i32,
        core::cmp::max(1, r.width().ceil() as u32),
        core::cmp::max(1, r.height().ceil() as u32),
    )
}

/// A shift in pixels, as `i32`, kept within what no pixmap reaches across: `draw_pixmap` unwraps
/// the rectangle it draws in, and a shift near the end of `i32` leaves none (VOMD).
pub fn shift(v: f32) -> i32 {
    v.clamp(-(1 << 28) as f32, (1 << 28) as f32) as i32
}

/// Coordinates (device pixels) within which tiny-skia scans a path without panicking: 2¹⁸. It
/// breaks beyond that: from about 2²⁴ (an `f32` stops holding half pixels) the edge clipper's
/// arithmetic puts an edge outside its clip, and `alpha_runs.rs` unwraps (`break_run`, about a
/// third of random anti-aliased triangles with a vertex at 2²⁵), and its cubic clipper unwraps
/// from about 2²¹ on a canvas drawn in tiles, and about 2²³ without anti-aliasing
/// (`edge_clipper.rs`, `mono_cubic_closest_t`). Random fills and strokes of lines, quads and
/// cubics up to this range, on canvases of 64 to 9,000 pixels a side, made no panic. A drawing
/// that real art needs isn't anywhere near it.
const SAFE: f32 = (1u32 << 18) as f32;

/// How far beyond the pixmap a path that is too far out is clipped, in pixels. A pixel's coverage
/// comes from the path within the pixel (the supersampling stays inside it), so the clipped path
/// draws the pixmap as the whole did; the rest is slack for the `f32` the clipped points end up as
/// (a few thousand pixels, where it holds a thousandth of one) and for the tiles of a big pixmap.
const MARGIN: f64 = 4096.0;

/// How far beyond the pixmap, in pixels, geometry still counts as touching it: a subpath that
/// stays further out is left out, and a curve that does is replaced by its chord. A pixel's
/// coverage comes from the path within the pixel, so the margin is only slack for rounding.
const NEAR: f64 = 2.0;

/// The most halvings a pass over a subpath's curves may make, at one flatness: a million is about
/// 16 MB of points and 10 ms. A subpath that needs more (tens of thousands of curves across the
/// pixmap) is flattened again four times as coarsely, until it fits, so that one that big keeps
/// its shape (to a flatness that grows with its size), and what else the path holds (each subpath
/// has its own count) doesn't change how it looks.
const MAX_SPLITS: usize = 1 << 20;

/// The flatness a curve is flattened to, in pixels: finer than tiny-skia's own.
const FLAT: f64 = 1.0 / 16.0;

/// What to draw for a path (VOMD).
pub enum Drawing {
    /// The path as it is, through tiny-skia's own transform.
    Whole,
    /// This path, in the pixmap's own pixels: draw it with the identity transform (and the
    /// paint's shader transformed by the one the path went through).
    Device(tiny_skia::Path),
    /// Nothing: a path tiny-skia can't draw.
    Nothing,
}

fn within(b: tiny_skia::Rect) -> bool {
    // The comparisons are false for a NaN.
    -SAFE <= b.left() && b.right() < SAFE && -SAFE <= b.top() && b.bottom() < SAFE
}

/// How to draw `path` filled, after `ts`, on a pixmap of `width` by `height` (VOMD).
///
/// tiny-skia's scan panics on paths far from the pixmap (see `SAFE`). A path within
/// `SAFE` is passed as it is. Any other is moved by `ts`, flattened and clipped to the pixmap and
/// `MARGIN` around it (`clip_to_pixmap`): only that part can be seen.
///
/// Not inlined, so that the drawing functions stay as small as they were: the clipping never
/// runs for an ordinary SVG.
#[inline(never)]
pub fn fill(
    path: &tiny_skia::Path,
    ts: tiny_skia::Transform,
    width: u32,
    height: u32,
) -> Drawing {
    // The bounds of the rectangle after `ts` hold those of the path: most paths are far inside.
    if path.bounds().transform(ts).is_some_and(within) {
        return Drawing::Whole;
    }
    fill_far(path, ts, width, height)
}

/// `fill` for a path whose bounds aren't within `SAFE` (out of line: the rare case isn't to slow
/// down the ordinary one).
#[cold]
#[inline(never)]
fn fill_far(
    path: &tiny_skia::Path,
    ts: tiny_skia::Transform,
    width: u32,
    height: u32,
) -> Drawing {
    // A path that can't be moved is not drawn by tiny-skia either.
    match path.clone().transform(ts) {
        Some(moved) => device(moved, width, height),
        None => Drawing::Nothing,
    }
}

/// `moved` as it is when it is within `SAFE`, else clipped.
fn device(moved: tiny_skia::Path, width: u32, height: u32) -> Drawing {
    if within(moved.bounds()) {
        return Drawing::Device(moved);
    }
    match clip_to_pixmap(&moved, width, height) {
        Some(clipped) => Drawing::Device(clipped),
        None => Drawing::Nothing,
    }
}

/// Whether tiny-skia strokes this width, under `ts`, as a hairline: `treat_as_hairline` of
/// `painter.rs`. A hairline is drawn along the path, each segment clipped to ±32767 in floating
/// point before it is made an integer (`scan/hairline.rs`, `hairline_aa.rs`).
fn is_hairline(width: f32, ts: tiny_skia::Transform, anti_alias: bool) -> bool {
    if width == 0.0 {
        return true;
    }
    if !anti_alias {
        return false;
    }
    let fast_len = |p: tiny_skia::Point| {
        let (x, y) = (p.x.abs(), p.y.abs());
        let (x, y) = if x < y { (y, x) } else { (x, y) };
        x + y * 0.5
    };
    let ts = tiny_skia::Transform {
        tx: 0.0,
        ty: 0.0,
        ..ts
    };
    let mut points = [
        tiny_skia::Point::from_xy(width, 0.0),
        tiny_skia::Point::from_xy(0.0, width),
    ];
    ts.map_points(&mut points);
    fast_len(points[0]) <= 1.0 && fast_len(points[1]) <= 1.0
}

/// How to draw the outline of `path`'s stroke (VOMD): made as `Pixmap::stroke_path` makes it,
/// dashed, then stroked in the path's own units with the transform's resolution scale, then moved
/// by `ts`, and handled as a fill (`fill`).
///
/// A hairline is drawn along the path with no outline, each segment clipped to the pixmap by the
/// hairline code, and is passed as it is.
#[inline(never)]
pub fn stroke(
    path: &tiny_skia::Path,
    stroke: &tiny_skia::Stroke,
    ts: tiny_skia::Transform,
    anti_alias: bool,
    width: u32,
    height: u32,
) -> Drawing {
    let hairline = is_hairline(stroke.width, ts, anti_alias);
    // A hairline is drawn whole: of a curve with points that far (1e19 and more, on either axis)
    // it asserts that its points are finite where debug assertions are on, and a release build
    // draws it. A debug build draws the outline of what `usvg::without_big_curves` leaves
    // instead.
    if hairline && !cfg!(debug_assertions) {
        return Drawing::Whole;
    }
    match usvg::without_big_curves(path, !hairline) {
        Some(std::borrow::Cow::Borrowed(_)) if hairline => Drawing::Whole,
        Some(kept) => stroke_outline(&kept, stroke, ts, width, height),
        None => Drawing::Nothing,
    }
}

/// `stroke` for a stroke that isn't a hairline.
fn stroke_outline(
    path: &tiny_skia::Path,
    stroke: &tiny_skia::Stroke,
    ts: tiny_skia::Transform,
    width: u32,
    height: u32,
) -> Drawing {
    let res_scale = tiny_skia::PathStroker::compute_resolution_scale(&ts);
    let dashed;
    let path = match &stroke.dash {
        Some(dash) => {
            let Some(p) = path.dash(dash, res_scale) else {
                return Drawing::Nothing;
            };
            dashed = p;
            &dashed
        }
        None => path,
    };
    // A path that can't be stroked or moved isn't drawn by tiny-skia.
    match path.stroke(stroke, res_scale).and_then(|o| o.transform(ts)) {
        Some(moved) => device(moved, width, height),
        None => Drawing::Nothing,
    }
}

type Pt = (f64, f64);

/// `path` (in pixmap pixels) cut to the pixmap and `MARGIN` around it, or `None` if nothing of it
/// is left.
///
/// Each subpath that reaches the pixmap (`Flattener::subpath`) is flattened to a polygon, curves
/// as far as they are near the pixmap, then cut at the four sides of the window one after another
/// (Sutherland–Hodgman: a polygon's points outside a side are replaced by its crossings of the
/// side, joined along it). What is cut away of a closed polygon, with the line along the side, is
/// a closed curve outside the side, which winds no point inside it: so the winding number of every
/// point of the window stays what it was, and the nonzero and even-odd fills of the window don't
/// change.
fn clip_to_pixmap(path: &tiny_skia::Path, width: u32, height: u32) -> Option<tiny_skia::Path> {
    let (w, h) = (f64::from(width), f64::from(height));
    let mut out = tiny_skia::PathBuilder::new();
    let mut flat = Flattener {
        win: [-MARGIN, -MARGIN, w + MARGIN, h + MARGIN],
        near: [-NEAR, -NEAR, w + NEAR, h + NEAR],
        poly: Vec::new(),
        splits: 0,
        tolerance: FLAT,
    };
    let mut segs = Vec::new();
    for seg in path.segments() {
        match seg {
            tiny_skia::PathSegment::MoveTo(_) => {
                flat.subpath(&segs, &mut out);
                segs.clear();
                segs.push(seg);
            }
            tiny_skia::PathSegment::Close => {
                flat.subpath(&segs, &mut out);
                segs.clear();
            }
            _ => segs.push(seg),
        }
    }
    flat.subpath(&segs, &mut out);
    out.finish()
}

fn pt(p: tiny_skia::Point) -> Pt {
    (f64::from(p.x), f64::from(p.y))
}

/// The deepest a curve is halved.
const MAX_DEPTH: u32 = 24;

/// A subpath being flattened to a polygon (`poly`) within a clip window `[left, top, right,
/// bottom]`.
struct Flattener {
    win: [f64; 4],
    /// The pixmap and `NEAR` around it, in the same order.
    near: [f64; 4],
    poly: Vec<Pt>,
    /// The halvings left for this pass over the subpath.
    splits: usize,
    /// The flatness of this pass, in pixels.
    tolerance: f64,
}

impl Flattener {
    /// Whether the box of `pts` is clear of the pixmap. A curve that is (inside its points' box)
    /// is replaced by its chord: the closed curve of both lies in the box, outside the pixmap,
    /// where it winds no point of the pixmap, as above.
    fn clear(&self, pts: &[Pt]) -> bool {
        let [l, t, r, b] = self.near;
        pts.iter().all(|p| p.0 < l)
            || pts.iter().all(|p| p.0 > r)
            || pts.iter().all(|p| p.1 < t)
            || pts.iter().all(|p| p.1 > b)
    }

    /// Whether a quad or cubic has to be halved (and counts the halving), else it is left as its
    /// chord.
    fn must_halve(&mut self, dev: f64, pts: &[Pt]) -> bool {
        if dev <= self.tolerance || self.clear(pts) {
            return false;
        }
        self.splits = self.splits.saturating_sub(1);
        true
    }

    fn quad(&mut self, q: [Pt; 3], depth: u32) {
        let dev = (q[1].0 - (q[0].0 + q[2].0) * 0.5).hypot(q[1].1 - (q[0].1 + q[2].1) * 0.5);
        // The curve is within half the control point's distance from its chord's midpoint.
        if depth >= MAX_DEPTH || self.splits == 0 || !self.must_halve(dev, &q) {
            self.poly.push(q[2]);
            return;
        }
        let m01 = mid(q[0], q[1]);
        let m12 = mid(q[1], q[2]);
        let m = mid(m01, m12);
        self.quad([q[0], m01, m], depth + 1);
        self.quad([m, m12, q[2]], depth + 1);
    }

    fn cubic(&mut self, c: [Pt; 4], depth: u32) {
        // The curve is within 3/4 of the larger distance of its control points from the thirds
        // of its chord.
        let third = |t: f64| {
            (
                c[0].0 + (c[3].0 - c[0].0) * t,
                c[0].1 + (c[3].1 - c[0].1) * t,
            )
        };
        let (a, b) = (third(1.0 / 3.0), third(2.0 / 3.0));
        let dev = (c[1].0 - a.0)
            .hypot(c[1].1 - a.1)
            .max((c[2].0 - b.0).hypot(c[2].1 - b.1));
        if depth >= MAX_DEPTH || self.splits == 0 || !self.must_halve(dev, &c) {
            self.poly.push(c[3]);
            return;
        }
        let (m01, m12, m23) = (mid(c[0], c[1]), mid(c[1], c[2]), mid(c[2], c[3]));
        let (m012, m123) = (mid(m01, m12), mid(m12, m23));
        let m = mid(m012, m123);
        self.cubic([c[0], m01, m012, m], depth + 1);
        self.cubic([m, m123, m23, c[3]], depth + 1);
    }

    /// Whether the box of the subpath's points reaches the pixmap: a closed curve in a box that
    /// doesn't winds no point of the pixmap (the curves are inside their points' box).
    fn reaches(&self, segs: &[tiny_skia::PathSegment]) -> bool {
        let [l, t, r, b] = self.near;
        let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for seg in segs {
            let pts: &[tiny_skia::Point] = match seg {
                tiny_skia::PathSegment::MoveTo(p) | tiny_skia::PathSegment::LineTo(p) => {
                    core::slice::from_ref(p)
                }
                tiny_skia::PathSegment::QuadTo(c, p) => &[*c, *p],
                tiny_skia::PathSegment::CubicTo(c1, c2, p) => &[*c1, *c2, *p],
                tiny_skia::PathSegment::Close => &[],
            };
            for p in pts.iter().map(|&p| pt(p)) {
                bounds = [
                    bounds[0].min(p.0),
                    bounds[1].min(p.1),
                    bounds[2].max(p.0),
                    bounds[3].max(p.1),
                ];
            }
        }
        bounds[0] <= r && bounds[2] >= l && bounds[1] <= b && bounds[3] >= t
    }

    /// Adds the subpath `segs` (a move to, then lines and curves) to `out`, closed and cut to the
    /// window, if it reaches the pixmap.
    fn subpath(&mut self, segs: &[tiny_skia::PathSegment], out: &mut tiny_skia::PathBuilder) {
        if segs.is_empty() || !self.reaches(segs) {
            return;
        }
        // A subpath that takes too many halvings is flattened again, more coarsely (the points
        // the curves are halved to don't depend on the flatness, so nothing else is needed).
        self.tolerance = FLAT;
        loop {
            self.poly.clear();
            self.splits = MAX_SPLITS;
            for seg in segs {
                match *seg {
                    tiny_skia::PathSegment::MoveTo(p) | tiny_skia::PathSegment::LineTo(p) => {
                        self.poly.push(pt(p));
                    }
                    tiny_skia::PathSegment::QuadTo(c, p) => {
                        if let Some(&p0) = self.poly.last() {
                            self.quad([p0, pt(c), pt(p)], 0);
                        }
                    }
                    tiny_skia::PathSegment::CubicTo(c1, c2, p) => {
                        if let Some(&p0) = self.poly.last() {
                            self.cubic([p0, pt(c1), pt(c2), pt(p)], 0);
                        }
                    }
                    tiny_skia::PathSegment::Close => {}
                }
                if self.splits == 0 {
                    break;
                }
            }
            if self.splits > 0 || self.tolerance.is_infinite() {
                break;
            }
            self.tolerance *= 4.0;
        }
        self.clip(out);
    }

    /// Clips the polygon that has been collected and adds it to `out`, closed.
    fn clip(&mut self, out: &mut tiny_skia::PathBuilder) {
        let mut poly = core::mem::take(&mut self.poly);
        if poly.len() < 3 {
            return;
        }
        let [l, t, r, b] = self.win;
        for side in 0..4 {
            poly = clip_side(&poly, |p| match side {
                0 => p.0 >= l,
                1 => p.0 <= r,
                2 => p.1 >= t,
                _ => p.1 <= b,
            }, |p, q| match side {
                0 => cut_x(p, q, l),
                1 => cut_x(p, q, r),
                2 => cut_y(p, q, t),
                _ => cut_y(p, q, b),
            });
            if poly.len() < 3 {
                return;
            }
        }
        let mut points = poly.iter();
        if let Some(first) = points.next() {
            out.move_to(first.0 as f32, first.1 as f32);
        }
        for p in points {
            out.line_to(p.0 as f32, p.1 as f32);
        }
        out.close();
    }
}

fn mid(a: Pt, b: Pt) -> Pt {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

/// `a + b` exactly, as the rounded sum and what rounding left out.
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let v = s - a;
    (s, (a - (s - v)) + (b - v))
}

/// `a * b` exactly, as the rounded product and what rounding left out.
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    (p, a.mul_add(b, -p))
}

/// `po + (qo - po) * (s - ps) / (qs - ps)`: where the segment from `p` to `q` is at `s` along
/// one axis, on the other axis (`po` and `qo` are the points' coordinates on it).
///
/// When both ends are far from `s` (1e20 away, where the crossing is a few pixels), the terms of
/// this sum are as large as that and cancel: any rounding of them is far more than the pixels. So
/// the numerator `po * (qs - ps) + (qo - po) * (s - ps)` is added up without rounding (as
/// Shewchuk's expansions: a sum of `f64`s kept as the parts that were left out of each rounding),
/// and only its value is rounded, before the division.
fn cross(ps: f64, qs: f64, po: f64, qo: f64, s: f64) -> f64 {
    let (dx, dy, ex) = (two_sum(qs, -ps), two_sum(qo, -po), two_sum(s, -ps));
    // The parts grow in size, and none overlaps another: `Grow-Expansion`.
    let mut parts = [0.0; 12];
    let mut len = 0;
    for (a, b) in [
        (po, dx.0),
        (po, dx.1),
        (dy.0, ex.0),
        (dy.0, ex.1),
        (dy.1, ex.0),
        (dy.1, ex.1),
    ] {
        let (hi, lo) = two_product(a, b);
        for mut q in [lo, hi] {
            for part in &mut parts[..len] {
                (q, *part) = two_sum(q, *part);
            }
            parts[len] = q;
            len += 1;
        }
    }
    parts.iter().sum::<f64>() / (dx.0 + dx.1)
}

/// The crossing of the segment `p`–`q` with the vertical line at `x`, from the end nearer the
/// line (its distance is the smaller part of the segment to interpolate by).
fn cut_x(p: Pt, q: Pt, x: f64) -> Pt {
    let (p, q) = if (p.0 - x).abs() <= (q.0 - x).abs() { (p, q) } else { (q, p) };
    (x, cross(p.0, q.0, p.1, q.1, x))
}

/// The crossing of the segment `p`–`q` with the horizontal line at `y`.
fn cut_y(p: Pt, q: Pt, y: f64) -> Pt {
    let (p, q) = if (p.1 - y).abs() <= (q.1 - y).abs() { (p, q) } else { (q, p) };
    (cross(p.1, q.1, p.0, q.0, y), y)
}

/// One step of Sutherland–Hodgman: the closed polygon `poly` cut to the side `keep` is true on.
fn clip_side(
    poly: &[Pt],
    keep: impl Fn(Pt) -> bool,
    cut: impl Fn(Pt, Pt) -> Pt,
) -> Vec<Pt> {
    let mut out = Vec::with_capacity(poly.len() + 4);
    let Some(&last) = poly.last() else {
        return out;
    };
    let mut prev = last;
    for &p in poly {
        let (was_in, is_in) = (keep(prev), keep(p));
        if was_in != is_in {
            out.push(cut(prev, p));
        }
        if is_in {
            out.push(p);
        }
        prev = p;
    }
    out
}
