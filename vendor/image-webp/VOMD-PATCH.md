# image-webp 0.2.4, patched for VOMD

The library's sources (`src/`) and licenses of image-webp 0.2.4 from crates.io (git
08b57d47), used through `[patch.crates-io]` in VOMD's `Cargo.toml` and `fuzz/Cargo.toml`. Its
`Cargo.toml` drops the dev-dependencies; `src/mod.rs` is upstream's, unused there too. The
patch, about 100 lines, marked `VOMD` in the code:

- **A limit on a lossless image's Huffman codes.** The format lets an image name up to 65,536
  groups of five codes, each with a table of up to 1,024 entries, whatever the image's size:
  a file of 1 MB could make the decoder allocate about 600 MB, and one of 13 MB about 8 GB.
  - `WebPDecoder::set_huffman_limit(bytes)`: the bytes the codes of one lossless image may take
    (a `VP8L` image, an `ALPH` plane, an animation frame). The default is no limit, as upstream.
  - `lossless.rs`: `LosslessDecoder` has `huffman_limit` and `huffman_bytes` (set with
    `with_huffman_limit`), and `read_huffman_codes` counts the groups (`size_of` a group
    times their number) before reading any, then each code's table and nodes once built
    (`HuffmanTree::heap_bytes`, new in `huffman.rs`). Past the limit it fails with
    `DecodingError::MemoryLimitExceeded`. The groups' `Vec` is made with its capacity, so what
    is counted is what it holds.
  - `decoder.rs` gives the limit to every `LosslessDecoder` it makes; `extended.rs`'s
    `read_alpha_chunk` takes it as a parameter.
- **A lossy frame of another size than its file says is refused before anything is
  allocated from it.** `Vp8Decoder::decode_frame` read the frame's own width and height (up to
  16383 × 16383: 384 MB of planes, and seconds of decoding) and only afterwards did the
  decoder compare them with the canvas or the `ANMF`'s frame.
  - `vp8.rs`: `Vp8Decoder::decode_frame_sized(r, width, height)` sets `expected`, which
    `read_frame_header` compares with the frame's size, failing with `InconsistentImageSizes`,
    before its planes are made. `decode_frame` is as upstream.
  - `decoder.rs` uses it for a still (the canvas's size), and for the frame of an animation
    with and without an `ALPH` chunk (the frame's size); the comparisons after the decode are gone.
- **A lossy frame's partitions are read as their bytes arrive.** `init_partitions` and
  `read_frame_header` made a zeroed `Vec` of each partition's claimed length (24 bits, 16 MiB,
  for the entropy partitions; 19 bits for the first) before reading it: a 38-byte file made the
  decoder allocate 16 MB before it found the bytes missing.
  - `vp8.rs`: `read_partition(r, size)` reads in steps (4 KiB, then doubling), so a file that
    stops short fails having allocated at most twice what it held. Used for the first partition
    and every entropy partition but the last (which is read to the end, as upstream).
- **An animation frame with alpha of another size than its `ANMF` says** (`decoder.rs`,
  `read_frame`): fails with `InconsistentImageSizes`, as a frame without alpha does, instead
  of panicking on a slice of the wrong length (now by the check above).

To upgrade: copy the new version's `src/` and licenses here, apply the same changes, and run
`cargo test --lib image::webp` and the image memory tests (`cargo xtask images`).
