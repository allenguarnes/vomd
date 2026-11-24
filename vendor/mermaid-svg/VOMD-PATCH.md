# mermaid-svg 0.7.0, patched for VOMD

The library's sources (`src/`, without `bin/`) and license of mermaid-svg 0.7.0 from crates.io
(MIT, <https://github.com/xmiksay/mermaid>), used through `[patch.crates-io]` in VOMD's
`Cargo.toml` and `fuzz/Cargo.toml`. Its `Cargo.toml` drops the binary, the examples, the bench,
the dev-dependencies and the gallery (`lib.rs` no longer includes `assets/gallery/*.md` in its
documentation), and adds `dugong` (vendored too). The changes, marked `VOMD` in the code:

- **Graphs are laid out by dugong** (Dagre's port, `vendor/dugong`) instead of the crate's own
  Sugiyama layout, whose subgraphs overlapped other nodes, lost edges to a subgraph and ignored a
  subgraph's `direction`.
  - `sugiyama/mod.rs` is rewritten: `layout_with` keeps its interface (class, ER, requirement
    and architecture diagrams draw with it) over `layout_compound`, which hands a compound graph
    to `dugong::layout_controlled` (Mermaid's settings: edge weight 1, label sizes, `minlen`).
    The old `coord`, `cycle`, `layer`, `order`, `route` and `work` modules and their tests are
    gone. A work budget shared by every layout of a render (`set_budget`, `failed`) and limits of
    2,000 nodes and 4,000 edges a layout: past them a layout returns nothing and the render fails
    (`vomd::render` says so).
  - `svg/compound.rs` (new): Mermaid's handling of subgraphs on top of it. A subgraph is a
    compound node; an edge to or from one goes to its first node and is cut where it crosses its
    box; a subgraph with no edge crossing its border is laid out on its own in its own
    `direction` (across its parent's if it has none, as Mermaid does) and is one node of its
    parent's graph. Room for a title above a compound subgraph (`title_lift`), and the box
    widened for a long title. Edge labels sit where Dagre made room for them.
  - `svg/flowchart/mod.rs`, `edges.rs`, `nodes.rs`: the flowchart builds its graph for
    `compound` (`laid_out`, `node_sizes`), draws each edge on its route (`EndClip::point` for an
    end already on a box, `draw_edge` takes the label's place) and draws the subgraphs' titles
    after the edges, on the frame's fill (`draw_subgraph_titles`). The old local-direction
    transposition and box computation are gone.
  - `svg/state/mod.rs`, `composite.rs`, `edges.rs`: the state diagram too: composite states are
    compound nodes (top to bottom inside, as Mermaid's), parallel regions are groups inside them
    with dashed lines between them. A state with notes is laid out as one node with room for
    them, so each note is level with its state on the side it names (`over`: above it); a note
    on a composite state goes beside its box. A transition to or from such a state reaches it
    from above or below its room, round its notes, and a self-loop on it is drawn on a side
    without notes (the canvas grows for it). Notes take the theme's note colours (they were
    always light yellow). `state_note_boxes` (in `vomd.rs`) gives their boxes to VOMD's tests.
- **Nine diagram types left out** (VOMD doesn't draw them; about 230 KB of code): sankey,
  radar, packet, requirement, C4, block, architecture, kanban and treemap. Their parsers
  (`parse/`), renderers (`svg/`, with `sankey_layout.rs`), AST types and `Diagram` variants, the
  frontmatter settings applied to them (`apply_packet_config`, `apply_radar_config`) and the now
  unused `token::split_top_level` are deleted; `parse::dispatch` returns
  `ParseError::Unsupported(kind)` (new) for their header keywords, without reading the rest, and
  VOMD's note says "kind diagrams aren't supported". `DiagramMeta` still reads their
  frontmatter keys (unused). On an upgrade, delete the same modules again.
- **One sort for two** (size): `state::region_dividers` sorts its regions by y and by x with one
  `sort_by` (the key a function pointer), not two closures that were each a copy of std's sort.
- **Text is measured by VOMD's font** (`vomd::set_measure`; `svg/metrics.rs`, `text_width`):
  a renderer's estimate for wider text (bold) stays wider in proportion.
- **VOMD's entry point** (`src/vomd.rs`): `render` (a parsed diagram in Mermaid's `default` or
  `dark` theme unless the diagram names one, at a fixed size, within a work budget),
  `flowchart_geometry` (for VOMD's tests of the layout). `svg/mod.rs`: `theme_from_meta` is
  crate-visible and `render_meta` renders with a final theme.
- **Bounded work** (fuzzing): a group named twice is the first of the name (two composite
  states of one name tripped a Dagre assertion); an edge repeated without a label is laid out
  once, its copies drawn on its route (repeated edges between nested compound nodes left Dagre's
  ordering unsound: `vendor/dugong` now fails such a layout instead of asserting); a Gantt chart looks at most 100,000 days to
  stretch a task over excluded days (a duration of millions of days, or every day excluded, was
  work without end), draws at most about 500 axis ticks (a `0day` interval was a loop without
  end) shades excluded days only over spans of at most 20,000 days, and clamps the parts of a date
  and a day count before its calendar arithmetic (they overflowed).
- **Dark-theme colours**: a mind map's node text is dark on a light fill whatever the theme, a
  git graph's tag text is dark (its fill is light), a Gantt `vert` marker is light blue on a
  dark background (navy was invisible), and a sequence diagram's `autonumber` digits are black
  or white by their badge's lightness (white on the dark theme's light grey was invisible).
- **Margins**: a class diagram's namespace frame keeps the canvas margin (it touched the top
  edge); a git graph leaves room left of its first commit for the rotated commit id (it ran
  over the branch names); an XY chart has 20 px below its axis instead of 70.

The crate's own unit tests (`#[cfg(test)]` modules) are kept as they came and aren't run;
those of the flowchart's and the state diagram's layout describe the old layout. VOMD's tests
(`src/mermaid/tests.rs`, the `mermaid-*` render goldens, the `mermaid` fuzz target) cover what
VOMD uses.

To upgrade: copy the new version's `src/` (without `bin/`) and license here, apply the same
changes, and run VOMD's tests (`cargo test --lib mermaid`, `cargo test --test golden`) and the
`mermaid` fuzz target.
