//! Render [Mermaid](https://mermaid.js.org/) diagrams to SVG in pure Rust.
//!
//! Supported diagram types: pie, sequence, flowchart, state, class, ER,
//! gantt, journey, timeline, quadrantChart, xychart, mindmap, gitGraph, zenuml.
//! (VOMD: sankey, radar, packet, requirementDiagram, C4, block, architecture,
//! kanban and treemap are left out: they parse to `ParseError::Unsupported`.)
//!
//! ```
//! let svg = mermaid_svg::render("pie\n\"A\" : 1\n\"B\" : 2\n").unwrap();
//! assert!(svg.starts_with("<svg"));
//! ```
//!
//! (VOMD: the gallery of the original documentation is left out.)

mod parse;
mod sugiyama;
mod svg;
pub mod vomd;

pub use parse::ast;
pub use parse::ast::{Diagram, DiagramMeta};
pub use parse::{parse, parse_with_meta, ParseError, SyntaxKind};
pub use svg::{render, render_diagram, render_diagram_with, render_with, RenderError, Theme};
