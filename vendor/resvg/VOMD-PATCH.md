# resvg 0.46.0, patched for VOMD

The library's sources (`src/`, without `main.rs`) and licenses of resvg 0.46.0 from crates.io
(git b8e58f5a, `crates/resvg`), used through `[patch.crates-io]` in VOMD's `Cargo.toml` and
`fuzz/Cargo.toml`. Its `Cargo.toml` drops the binary, the examples, the dev-dependencies and
the `raster-images` feature (and with it the `gif`, `image-webp`, `zune-jpeg` and `pico-args`
dependencies), and allows two lints upstream's code trips with the newer `rgb` in VOMD's lock
file. The changes, marked `VOMD` in the code:

- **Raster images go to a hook** instead of resvg's own decoders, which would add a second PNG
  decoder and decode every image at its full size on every render.
  - `render.rs`: `pub struct Hooks { raster, cancel }`, and `Context` holds `&Hooks`.
    `raster: &dyn Fn(&usvg::ImageKind, Transform, ImageRendering) -> Option<Pixmap>` is given
    the image, the transform from its own pixels to the canvas and the image's
    `image-rendering` (a hook that reduces the image must sample as it says: pixels averaged
    away can't be picked later), and returns pixels of any size. `Hooks::NONE`
    draws no raster image and is never cancelled; `render` and `render_node` use it.
  - `lib.rs`: `pub fn render_with(tree, transform, pixmap, &Hooks)`, and `pub use
    render::Hooks`.
  - `image.rs`: `render` and `render_inner` take the `Context` and the image's size; a raster
    image's pixels come from the hook, and are drawn with upstream's pattern code, the pattern
    scaled from the hook's pixels to the image's size. The decoders (`decode_raster`,
    `decode_png`/`jpeg`/`gif`/`webp`, `rgb_to_pixmap`, `rgba_to_pixmap`) are gone. A nested SVG
    image is rendered with `render_with` and the same hooks.
  - `filter/mod.rs`: `apply` takes the `Context`; `feImage` renders with its hooks. `clip.rs`
    draws only paths (with `Hooks::NONE`, as a clip path has no image), and `apply` takes the
    hooks to check the cancel flag before each child.
- **A cancel flag** (`Hooks::cancel`, an `AtomicBool` another thread may set). Rendering checks
  it before each node (`render_node`), before each child of a clip path (`clip.rs`), before
  each filter primitive (`apply_inner`, which then fails, clearing the layer), and in the
  filters' loops: once a pass of the box blur, a row of the IIR blur and of lighting, a pixel
  of morphology, convolution and turbulence (whose pixels can each take long), and a row of the
  displacement map. A cancelled render leaves the pixmap half drawn: the caller throws it away.

- **Turbulence's lattice arithmetic wraps** (`filter/turbulence.rs`, `noise2`): a coordinate
  past `i32::MAX` (many octaves double it) saturates when cast, and the `+ 1` after it
  overflowed, a panic where overflow is checked (tests, fuzzing). It wraps now, as a release
  build did.
- **Extreme numbers don't panic** (the same kind as in `vendor/usvg`): a group with filters
  makes its layer's `IntRect` as the one without does, with an option (`geom::int_rect`, for
  `render.rs` and the filter and primitive regions in `filter/mod.rs`, where an out-of-range
  region is `Error::InvalidRegion`); the shift of a drop shadow and of `feOffset` is kept
  within ±2²⁸ pixels (`geom::shift`), as `draw_pixmap` unwraps the rectangle it
  draws in; and `f32_bound` no longer asserts its value or its maximum is finite (a
  convolution, a transfer function or an arithmetic composite of extreme numbers makes
  infinities, which it bounds, as a release build does). Two more where overflow is checked
  (fuzzing): the box blur's `wl + 2` (`filter/box_blur.rs`, `create_box_gauss`: a
  `sigma` past 1e9 saturates `wl` at `i32::MAX`, and a release build made a negative width of
  it) is kept from overflowing by a width two under `i32::MAX`, and turbulence's `-seed`
  (`filter/turbulence.rs`, `init`: a seed of `i32::MIN`) wraps, as a release build did.
- **A path far from the pixmap is clipped to it** (`geom.rs`, `fill` and `stroke`, called by
  `path.rs`, `fill_path` and `stroke_path`). tiny-skia isn't vendored, and its anti-aliased scan
  panics on coordinates far from the pixmap, in several places (all `unwrap`s, or `debug_assert`s
  where those are on):
  - from about 2²⁴ px (where an `f32` stops holding half pixels), the edge clipper's `f32`
    arithmetic puts an edge outside its clip: `alpha_runs.rs`, `break_run`. About a third of
    random triangles with one vertex at 2²⁵ px panic, `M-6e8 80 L200 290 L100 110 Z` on a 300 px
    canvas among them (the point is far off in `x`, `y` or both);
  - the cubic clipper, from about 2²¹ px on a pixmap drawn in tiles (a side over 8,191 px,
    `DrawTiler`), and about 2²³ px on one that isn't, without anti-aliasing: `edge_clipper.rs`,
    `mono_cubic_closest_t`;
  - the scan keeps the rows of a path, four times as many for the supersampling, in an `i32`
    (`scan/path.rs`, `fill_path_impl`: `start_y <<= shift_edges_up`), which wraps for a row
    beyond 2²⁹ px, and the walk of the edges then meets an edge that ends before its first row
    (`debug_assert!(last_y >= curr_y)` in `walk_edges`, `break_run`'s `unwrap` in a release).

  A path whose device bounds (after the transform; for a stroke, those of its outline) are within
  `SAFE` (2¹⁸ px) is passed to tiny-skia as it is: it was 0 panics in random fills and strokes of
  lines, quads and cubics up to that range, on pixmaps of 64 to 9,000 px a side (300,000
  cubic fills of a 9,000 px wide pixmap up to 2²⁰ px: none; the cubic clipper above shows from
  2²¹, which is why the range is not wider), and an SVG's coordinates don't go there. Any other is moved by the transform (as tiny-skia does, in `f32`), flattened
  and clipped to the pixmap and 4,096 px around it, and filled with the identity transform (the
  paint's shader moved by the transform, as tiny-skia does): only that part can be seen. Where
  it differs from the build without it, the old one panicked, or drew a path of coordinates that
  an `f32` doesn't hold to a pixel, wrongly or not at all.
  - The clipping is `geom::clip_to_pixmap` in `f64`: each subpath is flattened to a polygon and
    cut at the four sides of the window one after the other (Sutherland–Hodgman). What is cut
    away, with the line along the side, is a closed curve outside the side, which winds no point
    inside it, so the winding number of every point of the window is unchanged and the nonzero
    and even-odd fills with it. The 4,096 px of margin are far more than a pixel's coverage needs
    (it comes from the path within the pixel; a filter works on the pixmap after the path was
    drawn); it keeps the clipped points' `f32` rounding a thousandth of a pixel.
  - What is flattened, and how finely, depends on the subpath alone. A subpath whose points' box
    doesn't reach the pixmap (and 2 px around it) is left out without being flattened: a closed
    curve inside a box winds no point outside it. Of the curves of one that does, a piece whose
    points' box is clear of the pixmap is replaced by its chord (the closed curve of both is in
    that box), and the rest is flattened to 1/16 px, finer than tiny-skia's own. A subpath is
    halved at most 2²⁰ times in a pass (about 16 MB of points and 10 ms), and one that needs more
    (tens of thousands of curves across the pixmap) is flattened again four times as coarsely,
    until it fits: its shape holds to a flatness that grows with its size, and nothing else in
    the path (each subpath has its own limit) changes how it looks. This replaced a budget the
    whole path shared, which once spent on curves off the canvas left the visible ones as chords
    (a shape gone). The most a path costs is in proportion to its size, and an SVG is at most 4
    MiB of XML. A curve is halved at most 24 times, so one wider than about 1e13 px is flattened
    coarsely where it crosses the pixmap.
  - A cut's crossing is worked out from the end of the segment nearer the side (`cut_x`,
    `cut_y`), and without rounding where both ends are far from the side (`geom::cross`): the
    numerator `po * (qs - ps) + (qo - po) * (s - ps)` is added up as an expansion (Shewchuk), and
    only its value is rounded. With the ends' distance from the side the terms are as large as
    1e20 and more and cancel; an `f64` (or a double-double: 1e-32 relative, short of coordinates
    over 1e31) left the line `y = x` through ends 1e20 apart as a canvas of one colour. Checked
    on random triangles with two vertices on a line through the canvas, coordinates to 1.5e38
    (1,479 of them): every pixel centre that isn't on an edge is where exact arithmetic puts it.
    The crossings are exact for the path's own points; one cut gives the next a point that is
    already rounded to an `f64` (relative 1e-16 on its larger coordinate, which is out of the
    window anyway). A path whose box is wider or taller than `f32::MAX` isn't reached: usvg can't
    measure it, and it is drawn blank as before.
  - A stroke is made first, as `Pixmap::stroke_path` does (dashed, then stroked in the path's own
    units with the transform's resolution scale; the joins and curves of a wide stroke reach much
    further than its width says, so its outline is what is measured), then moved, then handled as
    a fill. Cost: none that shows: that is the work tiny-skia does (it clones the outline once
    more). A stroke at most a pixel wide after the transform (a width of 0 too; an
    anti-aliased one only, bar the width of 0) is a hairline: tiny-skia draws it along the path,
    each segment clipped to ±32767 in floating point first, and `geom::is_hairline` repeats its
    `treat_as_hairline`; it is passed as it is.
  - A raster image and the region of a mask are rectangles drawn through a transform, which
    tiny-skia scans as a path (`fill_rect` with a transform is `fill_path` of the rectangle;
    `Mask::fill_path`): `image.rs` (`raster_images::draw`) and `mask.rs` (`apply`) go through
    `geom::fill` as `path.rs` does, so an image 1.9e9 px wide turned 18° or a mask region 4.4e8 px
    tall on a rotated group are clipped like any far path (they were the same panics, in
    `alpha_runs.rs` and `pipeline/mod.rs`). The other `fill_rect`s and `draw_pixmap`s are drawn
    with the identity transform or a whole-pixel shift on rectangles within the pixmap
    (`geom::shift`, `int_rect`), and clip paths, patterns and text paths go through `path.rs`.
  - The stroker's arithmetic has a limit of its own: a quad whose points are over 1.8e19 apart
    on both axes overflows `find_quad_max_curvature` (two infinite products, subtracted, an
    `inf - inf` unwrapped as a `NaN`; a panic whatever the transform). Of 6,000 random curves of
    1e17 to 1e22 per axis, tiny-skia panicked on 104 (release), none under 1.4e19 on both axes
    (1,500 between 1e18 and 1e19) and none long on one axis only. `usvg::without_big_curves`
    leaves out the **subpaths** of a path to stroke that have a quad or cubic over 1e18 on both
    axes; the others are stroked as before. `vendor/usvg` uses it for the stroke's box. A hairline of a
    curve that far on one axis (`hairline.rs`) asserts, where debug assertions are on (tests,
    fuzzing), that its points are finite, which they aren't at 1e19: there the hairline's path is
    taken less its subpaths with such a curve (on either axis) and drawn as an outline, and a
    release build draws it whole, as before.

To upgrade: copy the new version's `src/` (without `main.rs`) and licenses here, apply the same
changes, and run VOMD's SVG tests (`cargo test --lib image::svg`) and image memory tests
(`cargo xtask images`): the SVG charges are measured against this version.
