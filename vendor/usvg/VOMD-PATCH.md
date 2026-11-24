# usvg 0.46.0, patched for VOMD

The library's sources (`src/`, without `main.rs`) and licenses of usvg 0.46.0 from crates.io
(git b8e58f5a, `crates/usvg`), used through `[patch.crates-io]` in VOMD's `Cargo.toml` and
`fuzz/Cargo.toml`. Its `Cargo.toml` drops the binary (and with it `pico-args`) and the
dev-dependencies. The changes, marked `VOMD` in the code, are of three kinds. All but the last
two are about numbers too large for an `f32` (a size, a position, a stroke width, a transform): they
made a rectangle or a size that upstream then unwraps, a panic in the process that draws the SVG
(the `svg` fuzz target found the first). Each now leaves out what it can't make, as upstream
does for the size it checks (the last of them is an assertion on such a number). The
next-to-last is about a pattern that paints with itself, which recursed until the stack
overflowed; the last is about text in a fallback face, which an emoji sequence or a character no
face has left undrawn.

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
- **A very wide stroke of a cubic** (`tree/mod.rs`, `stroke_path`, used by
  `calculate_stroke_bbox` and by resvg's `geom.rs` in place of `Path::stroke`): the stroker
  splits a cubic until its offset is within `1 / (4 * res_scale)`, checked in `f32`; the offset
  is the unit normal times half the width, and with half the width over about 2²² at the scale
  the normal's rounding alone is more than that, so it splits to its recursion limit, wherever
  the curve is: up to 16 million points for one cubic (1.8 s), and a path of 40 cubics 10⁹ wide
  took a second to stroke and a second more for its box (the `svg` fuzz target, a timeout). A
  subpath with a cubic whose half width times the scale is over 2¹⁸ (`STROKE_RADIUS`) is stroked
  on its own at the scale that brings it to 2¹⁸: a tolerance of a millionth of half the width
  (2⁻²⁰ of it) in place of a quarter of a pixel, so that edge may be off by up to that
  (`half width × res_scale / 2²⁰` pixels: a quarter of a pixel at the limit, 477 at 10⁹ wide,
  enough to leave a wedge out of an edge that shows). `stroke_error` gives that tolerance (nominal,
  not a proven distance: a shear can make it up to √2 more, and both outlines round) for a
  path, width and scale (0 where the stroke is tiny-skia's), and VOMD refuses an SVG with a
  stroke over a pixel off at the scale it is drawn (`MAX_STROKE_ERROR` in `src/image/svg.rs`).
  Every other path is stroked as it was, whole, to the same points, how far out it is or not;
  of 5,040 cubics of five shapes and every size, place, width and scale, those at the limit made
  at most 112,000 points (13 ms).
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

- **Fallback faces replace clusters, and pass over a character no face has**
  (`text/layout.rs`, `shape_text`): upstream copied a fallback's glyphs over the missing ones by
  index, and gave up on the whole span when the two shapings had different numbers of glyphs.
  An emoji sequence (❤️ with its variation selector, 👍🏽 with a skin tone, a joiner's family, a
  flag, a keycap) is one glyph in an emoji font and several in the face before it, so every
  emoji of such a span was drawn as a missing glyph (upstream's TODO: "Replace clusters and not
  glyphs"). Now each cluster of the fallback's shaping (its glyphs with the same start) whose
  glyphs are all found, and which overlaps a missing glyph, replaces every glyph of the span it
  overlaps, in visual order; a fallback that replaces nothing is passed over like one that
  covers nothing. And a character for which the fallback selector finds no face (`None`)
  stopped the fallback for the rest of the span (`break 'outer`): it is remembered (in a set)
  and passed over, and the next missing character is looked for from where the last search
  stopped, for at most 256 such characters a span; after them the rest of the span stays as it
  is, as upstream left it after the first. The rounds that shape are still at most one for each
  face, and the merge of a round is linear in the glyphs (with a binary search for the clusters
  that overlap), so a span of 4000 different characters no face has is drawn in tens of
  milliseconds, not seconds. The character looked for is the first of the missing glyph's
  cluster that no face used has (upstream took the cluster's first: a combining mark shapes into
  its base's cluster, so the base was looked for, and reported, while the mark was what no face
  had), or the first if each has one. A cluster's glyphs and characters are looked at once a
  search, where it was found (and how far its characters were checked) kept between lookups, and
  a cluster with nothing left to look for is passed over whole: one base with 16,000 of a mark no
  face has is one cluster of 16,000 missing glyphs, which looked through the whole cluster for
  each glyph took 5.7 s. Where debug assertions are on (tests), the search's steps are counted
  (`layout::vomd_work::fallback_steps`, hidden from the docs); a release build counts nothing.

To upgrade: copy the new version's `src/` (without `main.rs`) and licenses here, apply the same
changes, and run VOMD's SVG tests (`cargo test --lib image::svg`), whose
`extreme_numbers_in_shapes_are_drawn_or_refused` draws each of the cases above except the very
wide stroke, which `a_wide_stroke_of_cubics_far_out_is_cheap` and
`a_wide_stroke_whose_edge_shows_is_drawn_right_or_refused` draw, and the last,
which `a_pattern_that_paints_with_itself_by_inheritance_is_drawn_without_it` and
`a_cycle_of_patterns_that_fans_out_is_converted_once_each` draw, and the fallback's, which
`a_skin_tone_sequence_is_drawn_as_one_glyph`,
`an_uncovered_mark_on_a_covered_base_is_what_is_reported`,
`a_cluster_of_thousands_of_missing_marks_is_looked_through_once`,
`a_character_no_face_has_leaves_the_rest_of_the_span_drawn`,
`many_characters_no_face_has_are_looked_for_up_to_a_cap`,
`a_fallback_for_thousands_of_clusters_merges_them` and `a_bitmap_color_glyph_is_drawn` draw (with the fonts of `fixtures/fonts`).
