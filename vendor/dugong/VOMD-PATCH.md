# dugong 0.11.0, patched for VOMD

The library's sources (`src/`) and licenses (MIT or Apache-2.0; Dagre's MIT license and its
notice in `THIRD_PARTY_LICENSES/` and `THIRD_PARTY_NOTICES.md`) of dugong 0.11.0 from crates.io
(<https://github.com/Latias94/merman>, a port of dagrejs/dagre 2.0.2), used through
`[patch.crates-io]` in VOMD's `Cargo.toml` and `fuzz/Cargo.toml` by `vendor/mermaid-svg`. Its
`Cargo.toml` drops the tests, the benches and the dev-dependencies, and `rust-version` is VOMD's
(1.89: it builds with it). The changes:

- No serde or serde_json (size): `data/` (a linked list used only by tests, printed through
  serde_json) is gone, and `EdgeLabel::extras` (a map of `serde_json::Value`s nothing in the
  layout reads), with `util::map_values_prop` and the test in `self_edges.rs` that set it.
- The check that the ordering phase left every layer with dense, unique orders
  (`pipeline/layout.rs`) is made in every build and fails the layout
  (`LayoutError::InvalidOrder`, new) instead of being a `debug_assert`: VOMD's fuzzing found
  it broken by repeated edges between nested compound nodes (`vendor/mermaid-svg` now lays such
  an edge out once).
- Only the options Mermaid uses (size): cycles are broken by DFS and ranks are made by network
  simplex, Dagre's defaults. `GraphLabel::ranker` and `acyclicer` (which Mermaid never sets) are
  gone, with `greedy_fas.rs` and the `tight-tree` and `longest-path` rankers' arms in
  `rank::rank_controlled` (`longest_path` and `feasible_tree` stay: network simplex uses them),
  and the unit tests of those options.
- One stable sort per element type (size): `src/dsort.rs` (new) has `dsort_by` and
  `dsort_by_key`, which hand the comparison to one `sort_by` per element type through
  `&mut dyn FnMut`; every `sort_by`/`sort_by_key` call (each closure was its own copy of std's
  sort, about 3.5 KB) uses them. The same std sort on the same elements and comparisons, so the
  same order. On an upgrade, rename new calls too (`grep -rn "\.sort_by" src`).

To upgrade: copy the new version's `src/` and licenses here, apply the same changes, and run
VOMD's Mermaid tests (`cargo test --lib mermaid`, `cargo test --test golden`).
