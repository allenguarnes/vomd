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

To upgrade: copy the new version's `src/` (without `main.rs`) and licenses here, apply the same
changes, and run VOMD's SVG tests (`cargo test --lib image::svg`) and image memory tests
(`cargo xtask images`): the SVG charges are measured against this version.
