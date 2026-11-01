# usvg 0.46.0, patched for VOMD

The library's sources (`src/`, without `main.rs`) and licenses of usvg 0.46.0 from crates.io
(git b8e58f5a, `crates/usvg`), used through `[patch.crates-io]` in VOMD's `Cargo.toml` and
`fuzz/Cargo.toml`. Its `Cargo.toml` drops the binary (and with it `pico-args`) and the
dev-dependencies. The changes, marked `VOMD` in the code, are of two kinds. All but the last
are about numbers too large for an `f32` (a size, a position, a stroke width, a transform): they
made a rectangle or a size that upstream then unwraps, a panic in the process that draws the SVG
(the `svg` fuzz target found the first). Each now leaves out what it can't make, as upstream
does for the size it checks (the last of them is an assertion on such a number). The final one
is about a pattern that paints with itself, which recursed until the stack overflowed.

- **An image far from zero** (`parser/image.rs`, `convert_inner`): an image much taller than it
  is wide, aligned inside a tall rectangle, sits at a position where an `f32` has no room for
  its size beside it, and `Size::to_non_zero_rect` unwrapped the empty rectangle. It is
  `NonZeroRect::from_xywh(..)?` now. `fit_view_box` makes its size as `Size::scale_to` and
  `expand_to` do (`size_scale_f64`), but returns `None` where these unwrap a size that
  overflowed (an `image` of `preserveAspectRatio="… slice"` and a side of 1e38).
- **A rectangle in the shape's units** (`parser/mod.rs`, `bbox_transform`, which replaces
  `NonZeroRect::bbox_transform` in `filter.rs`, `mask.rs` and `paint_server.rs`): the same
  arithmetic, an option instead of an unwrap. A filter, a filter primitive's region, a mask or a
  pattern whose rectangle can't be made is skipped as one of an invalid size is.
- **A marker of a stroke too wide** (`parser/marker.rs`, `draw_marker`): the marker's size
  (`Size::from_wh`) isn't unwrapped; no marker is drawn at that vertex.
- **A turbulence frequency** (`parser/filter.rs`, `convert_turbulence`): a `baseFrequency`
  past `f32::MAX` is infinite, which `PositiveF32` refuses; it stays at the default (0), as an
  invalid one does, not unwrapped (a panic in a release build too).
- **A morphology radius** (`parser/filter.rs`, `convert_morphology`): a radius that is infinite
  once scaled stays at the radius it had, not unwrapped.
- **An infinite miter limit** (`tree/mod.rs`, `StrokeMiterlimit::new`): `stroke-miterlimit` of
  `1e39` is infinite as an `f32`, and the `debug_assert!(n.is_finite())` stopped it where
  debug assertions are on (the fuzz target, tests). It isn't asserted; a release build never
  checked it, and an infinite limit means a join is never cut, as a very large one does.
- **A curve too large to stroke** (`tree/mod.rs`, `without_big_curves`, used by
  `calculate_stroke_bbox` and by resvg's `geom.rs`): a quad whose points are further apart than
  1.8e19 on both axes makes tiny-skia's stroker (`find_quad_max_curvature`) multiply to infinity
  twice and unwrap a `NaN`, a panic at the parse of the path (or, for a cubic, one of the same
  kind). The stroke's box is made of the path less its subpaths with a quad or cubic over 1e18
  on both axes (none left: it takes the bounding box, as when the stroke can't be made); resvg
  strokes the same remaining path.
- **An infinite value bounded** (`parser/mod.rs`, `f32_bound`): the saturation of `feColorMatrix`
  (`values="1e300"`) and a gradient stop's `offset` are infinite once an `f32`, and the
  `debug_assert!(val.is_finite())` stopped them where debug assertions are on. Only `NaN`
  (which no number of an SVG makes) is asserted against; an infinite value is bounded to the
  maximum, as a release build always did (the other caller, `feSpecularLighting`, tests its
  range first).
- **Orders whose product overflows** (`parser/filter.rs`, `convert_convolve_matrix`;
  `tree/filter.rs`, `ConvolveMatrixData::new`): `order="65536 65536"` is a `u32` product that
  overflows (a panic where overflow is checked, a wrapped 0 in a release build, which then took
  an empty kernel for a match). It is a checked product: an order whose kernel size doesn't fit,
  or doesn't match the kernel's length, is a primitive that isn't made, as a mismatch is.

- **A pattern that paints with itself through what it inherits** (`parser/paint_server.rs`,
  `convert`; `parser/converter.rs`, `Cache::patterns_in_progress`): `fix_recursive_patterns`
  only looks at the `fill` and `stroke` attributes of a pattern's own descendants (and of the
  elements they link to), so `<svg stroke="url(#p)">` with a `<pattern id="p">` whose `<rect>`
  takes that stroke made `convert_pattern` convert the rect, whose stroke converted the pattern
  again, until the stack overflowed (a `use`, or two patterns painting with each other
  through an ancestor's `fill`, did the same). The patterns being converted are kept in the
  `Cache`, and a pattern asked for while it is being converted isn't made: the reference
  becomes what a reference to nothing is (its fallback, or no paint), as the attribute-based
  pass makes of the references it finds, so everything it already caught is converted as
  before. A pattern converted with such a reference left out is part of a cycle, however
  deep the cut: it is cached like any other (upstream's cache), so each pattern is converted
  once, not once for each way round the cycle (a chain of patterns that each paint two shapes
  with the next, the last with the first, would take twice as long for each link).

To upgrade: copy the new version's `src/` (without `main.rs`) and licenses here, apply the same
changes, and run VOMD's SVG tests (`cargo test --lib image::svg`), whose
`extreme_numbers_in_shapes_are_drawn_or_refused` draws each of the cases above except the last,
which `a_pattern_that_paints_with_itself_by_inheritance_is_drawn_without_it` and
`a_cycle_of_patterns_that_fans_out_is_converted_once_each` draw.
