//! Mermaid syntax parser. Produces a [`Diagram`] AST from Mermaid source.
//!
//! Supports pie, sequence, flowchart (`flowchart`/`graph`/`flowchart-elk`), state
//! (`stateDiagram`/`stateDiagram-v2`), class, ER, and gantt diagrams.
//!
//! Implementation: hand-rolled line-oriented scanners (one per diagram type)
//! rather than a single PEG grammar — Mermaid's syntax is strongly
//! line-based and per-type scanners stay short and easy to extend.

pub mod ast;
mod class;
mod er;
mod flowchart;
mod gantt;
mod gitgraph;
mod journey;
mod mindmap;
mod pie;
mod preamble;
mod quadrant;
mod sequence;
mod state;
mod style;
mod timeline;
mod token;
mod xychart;
mod zenuml;

pub use ast::*;

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum ParseError {
    #[error("parse error at line {line}: {message}")]
    Syntax {
        /// The class of syntax failure, so callers can branch without
        /// string-matching `message`.
        kind: SyntaxKind,
        /// A human-readable description of the specific failure.
        message: String,
        /// 1-based source line the error was detected on.
        line: usize,
    },
    #[error("unknown diagram type: {0}")]
    UnknownDiagramType(String),
    /// VOMD: a diagram type Mermaid has and this crate doesn't draw (its name, as VOMD's note
    /// says it).
    #[error("unsupported diagram type: {0}")]
    Unsupported(&'static str),
    #[error("empty input")]
    Empty,
}

/// The class of a [`ParseError::Syntax`] failure. Lets callers distinguish the
/// recurring kinds of parse error (unknown statement vs bad number vs unclosed
/// block) without inspecting the free-form `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyntaxKind {
    /// The diagram's opening header keyword was missing or misspelled. A
    /// defensive per-parser check: [`parse`] pre-validates the header and
    /// reports an unrecognized one as [`ParseError::UnknownDiagramType`], so
    /// this surfaces only when a diagram parser is driven directly.
    MissingHeader,
    /// A line did not match any statement in the diagram's grammar.
    UnknownStatement,
    /// A numeric field could not be parsed (bad integer/float, NaN, or out of
    /// range).
    InvalidNumber,
    /// A bracket, brace, quote, or block delimiter was left unclosed.
    Unclosed,
    /// A recognized statement was otherwise malformed — a missing or empty
    /// required field, or a value that did not fit the expected shape.
    Malformed,
}

impl ParseError {
    /// [`SyntaxKind::MissingHeader`] error at `line`.
    pub(crate) fn header(line: usize, message: impl Into<String>) -> Self {
        Self::syntax(SyntaxKind::MissingHeader, line, message)
    }
    /// [`SyntaxKind::UnknownStatement`] error at `line`.
    pub(crate) fn unknown(line: usize, message: impl Into<String>) -> Self {
        Self::syntax(SyntaxKind::UnknownStatement, line, message)
    }
    /// [`SyntaxKind::InvalidNumber`] error at `line`.
    pub(crate) fn number(line: usize, message: impl Into<String>) -> Self {
        Self::syntax(SyntaxKind::InvalidNumber, line, message)
    }
    /// [`SyntaxKind::Unclosed`] error at `line`.
    pub(crate) fn unclosed(line: usize, message: impl Into<String>) -> Self {
        Self::syntax(SyntaxKind::Unclosed, line, message)
    }
    /// [`SyntaxKind::Malformed`] error at `line`.
    pub(crate) fn malformed(line: usize, message: impl Into<String>) -> Self {
        Self::syntax(SyntaxKind::Malformed, line, message)
    }

    fn syntax(kind: SyntaxKind, line: usize, message: impl Into<String>) -> Self {
        Self::Syntax {
            kind,
            message: message.into(),
            line,
        }
    }
}

pub fn parse(input: &str) -> Result<Diagram, ParseError> {
    parse_with_meta(input).map(|(d, _)| d)
}

/// Parse `input`, also returning the cross-cutting [`DiagramMeta`] (title,
/// `accTitle`/`accDescr`, theme) extracted from the source preamble. The
/// diagram body is parsed from the source with the preamble removed.
pub fn parse_with_meta(input: &str) -> Result<(Diagram, DiagramMeta), ParseError> {
    // Strip a leading UTF-8 BOM (U+FEFF): Windows editors prepend one and Rust's
    // `trim` does not remove it, so it would otherwise glue onto the header token
    // (`﻿flowchart` → "unknown diagram type"). (#187)
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let (meta, cleaned) = preamble::strip(input);
    let mut diagram = dispatch(&cleaned)?;
    if let Some(title) = &meta.title {
        apply_title(&mut diagram, title);
    }
    if let (Diagram::Timeline(t), Some(true)) = (&mut diagram, meta.timeline_disable_multicolor) {
        t.disable_multicolor = true;
    }
    if let Diagram::GitGraph(g) = &mut diagram {
        apply_git_graph_config(&mut g.config, &meta.git_graph);
    }
    if let Diagram::Flowchart(f) = &mut diagram {
        if let Some(curve) = meta.config.get("flowchart.curve") {
            f.config_curve = Some(ast::EdgeCurve::from_name(curve));
        }
    }
    if let Diagram::XyChart(x) = &mut diagram {
        apply_xychart_config(x, &meta.config);
    }
    if let Diagram::Pie(p) = &mut diagram {
        if meta.pie_text_position.is_some() {
            p.text_position = meta.pie_text_position;
        }
        if meta.pie_donut_hole.is_some() {
            p.donut_hole = meta.pie_donut_hole;
        }
        if meta.pie_legend_position.is_some() {
            p.legend_position = meta.pie_legend_position.clone();
        }
    }
    if let Diagram::Quadrant(q) = &mut diagram {
        if meta.quadrant_chart_width.is_some() {
            q.chart_width = meta.quadrant_chart_width;
        }
        if meta.quadrant_chart_height.is_some() {
            q.chart_height = meta.quadrant_chart_height;
        }
        if meta.quadrant_point_radius.is_some() {
            q.point_radius = meta.quadrant_point_radius;
        }
    }
    Ok((diagram, meta))
}

/// Copy a frontmatter `title` onto the concrete diagram, but only for diagram
/// kinds that carry a title and only when the body did not set one itself.
fn apply_title(diagram: &mut Diagram, title: &str) {
    let slot: Option<&mut Option<String>> = match diagram {
        Diagram::Pie(d) => Some(&mut d.title),
        Diagram::Sequence(d) => Some(&mut d.title),
        Diagram::Flowchart(d) => Some(&mut d.title),
        Diagram::Gantt(d) => Some(&mut d.title),
        Diagram::Journey(d) => Some(&mut d.title),
        Diagram::Timeline(d) => Some(&mut d.title),
        Diagram::Quadrant(d) => Some(&mut d.title),
        Diagram::XyChart(d) => Some(&mut d.title),
        Diagram::GitGraph(d) => Some(&mut d.title),
        _ => None,
    };
    if let Some(slot) = slot {
        if slot.is_none() {
            *slot = Some(title.to_string());
        }
    }
}

/// Overlay the preamble's `config.gitGraph.*` keys onto the diagram's config,
/// leaving upstream defaults where the source set nothing.
fn apply_git_graph_config(cfg: &mut ast::GitGraphConfig, meta: &ast::GitGraphMeta) {
    if let Some(name) = &meta.main_branch_name {
        cfg.main_branch_name = name.clone();
    }
    if let Some(v) = meta.show_branches {
        cfg.show_branches = v;
    }
    if let Some(v) = meta.show_commit_label {
        cfg.show_commit_label = v;
    }
    if let Some(v) = meta.rotate_commit_label {
        cfg.rotate_commit_label = v;
    }
    if let Some(v) = meta.parallel_commits {
        cfg.parallel_commits = v;
    }
    if let Some(v) = meta.main_branch_order {
        cfg.main_branch_order = Some(v);
    }
}

/// Overlay `config.xyChart.*` and the `themeVariables.xyChart.plotColorPalette`
/// theme variable onto an [`ast::XyChartDiagram`].
fn apply_xychart_config(
    d: &mut ast::XyChartDiagram,
    config: &std::collections::BTreeMap<String, String>,
) {
    if let Some(w) = config.get("xyChart.width").and_then(|v| v.parse().ok()) {
        d.width = Some(w);
    }
    if let Some(h) = config.get("xyChart.height").and_then(|v| v.parse().ok()) {
        d.height = Some(h);
    }
    if let Some(v) = config.get("xyChart.showLegend") {
        d.show_legend = match v.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => d.show_legend,
        };
    }
    if let Some(p) = config.get("themeVariables.xyChart.plotColorPalette") {
        d.plot_color_palette = p
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
}

fn dispatch(input: &str) -> Result<Diagram, ParseError> {
    let header_line = input
        .lines()
        .map(strip_comment)
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or(ParseError::Empty)?;

    let head_token = header_line
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
        // Upstream's grammar accepts a trailing colon on the header (`gitGraph:`).
        .trim_end_matches(':');
    match head_token {
        "pie" => pie::parse(input).map(Diagram::Pie),
        "sequenceDiagram" => sequence::parse(input).map(Diagram::Sequence),
        // `flowchart-elk` selects the ELK layouter upstream; we lay it out with
        // sugiyama like the `layout: elk` config, matching our layout-deviation policy.
        "flowchart" | "graph" | "flowchart-elk" => flowchart::parse(input).map(Diagram::Flowchart),
        "stateDiagram" | "stateDiagram-v2" => state::parse(input).map(Diagram::State),
        "classDiagram" | "classDiagram-v2" => class::parse(input).map(Diagram::Class),
        "erDiagram" => er::parse(input).map(Diagram::Er),
        "gantt" => gantt::parse(input).map(Diagram::Gantt),
        "journey" => journey::parse(input).map(Diagram::Journey),
        "timeline" => timeline::parse(input).map(Diagram::Timeline),
        "quadrantChart" => quadrant::parse(input).map(Diagram::Quadrant),
        "xychart-beta" | "xychart" => xychart::parse(input).map(Diagram::XyChart),
        "mindmap" => mindmap::parse(input).map(Diagram::Mindmap),
        "gitGraph" => gitgraph::parse(input).map(Diagram::GitGraph),
        "zenuml" => zenuml::parse(input).map(Diagram::Sequence),
        // VOMD: diagram types it doesn't draw are not parsed (their parsers and renderers are
        // left out of the crate).
        "sankey-beta" | "sankey" => Err(ParseError::Unsupported("sankey")),
        "radar-beta" | "radar" => Err(ParseError::Unsupported("radar")),
        "packet-beta" | "packet" => Err(ParseError::Unsupported("packet")),
        "requirementDiagram" => Err(ParseError::Unsupported("requirement")),
        "C4Context" | "C4Container" | "C4Component" | "C4Dynamic" | "C4Deployment" => {
            Err(ParseError::Unsupported("C4"))
        }
        "block-beta" | "block" => Err(ParseError::Unsupported("block")),
        "architecture-beta" | "architecture" => Err(ParseError::Unsupported("architecture")),
        "kanban" => Err(ParseError::Unsupported("kanban")),
        "treemap-beta" | "treemap" => Err(ParseError::Unsupported("treemap")),
        other => Err(ParseError::UnknownDiagramType(other.to_string())),
    }
}

pub(crate) fn strip_comment(line: &str) -> &str {
    // A `%%` inside a `"…"` quoted label is content, not a comment start
    // (`A["100%% sure"]`), so scan for the marker only outside quoted runs.
    if let Some(pos) = token::find_unquoted(line, "%%") {
        &line[..pos]
    } else {
        line
    }
}

#[cfg(test)]
mod tests;
