# swash 0.2.10, patched for VOMD

The library's sources (`src/`) and licenses (Apache-2.0 or MIT) of swash 0.2.10 from crates.io
(<https://github.com/dfrg/swash>), used through `[patch.crates-io]` in VOMD's `Cargo.toml` and
`fuzz/Cargo.toml`. Its `Cargo.toml` is the published one's source (`Cargo.toml.orig`). The
change, marked `VOMD` in the code:

- **No hinting** (size, about 145 KB): VOMD builds every scaler with `.hint(false)`
  (`src/text/cache.rs`, `src/math/layout.rs`), but the flag is a runtime value, so skrifa's
  TrueType bytecode interpreter, autohinter and CFF hinter were linked all the same. In
  `scale/mod.rs`, `ScalerBuilder::hint` does nothing and `scale_outline_impl` always draws the
  unhinted outline; `scale/hinting_cache.rs` and the context's cache are gone. Output is as
  before: the removed branch was never taken.

To upgrade: copy the new version's `src/`, licenses and `Cargo.toml.orig` (as `Cargo.toml`)
here, apply the same change, and run the golden tests (`cargo test --test golden`) and the math
tests (`cargo test --lib math`).
