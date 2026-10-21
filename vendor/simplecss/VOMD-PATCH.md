# simplecss 0.2.2, patched for VOMD

The library's sources (`src/`) and licenses of simplecss 0.2.2 from crates.io, used through
`[patch.crates-io]` in VOMD's `Cargo.toml` and `fuzz/Cargo.toml` (usvg reads an SVG's style
sheets with it). The changes, marked `VOMD` in the code:

- **An error's position is made in constant time** (`stream.rs`, `gen_text_pos`): upstream counts
  the lines and columns from the start of the text each time the parser meets something it
  doesn't take (every declaration's end, for one), so a style sheet of *n* declarations or rules
  took *n* times its length (40,000 rules of `a{fill:red}` took eight seconds, in `usvg`'s
  `from_xmltree`, which can't be stopped). The position is the line 1 and the byte offset as
  the column; nothing but the log reads it.
- **The rules of a selector list share their declarations** (`lib.rs`, `Rule::declarations` is a
  `Declarations`, an `Rc` of the list that dereferences to a slice and iterates by reference, so
  usvg's code reads it as it did): upstream copies the list for each selector, so a list of *n*
  selectors (`a,a,a,…{…}`) with *d* declarations made *n* × *d*, gigabytes from a megabyte.

To upgrade: copy the new version's `src/` and licenses here, apply the same change, and run
VOMD's SVG tests (`cargo test --lib image::svg`).
