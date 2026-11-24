//! VOMD: what VOMD calls. A diagram rendered at a fixed size (never the responsive
//! `width="100%"`), in Mermaid's `default` or `dark` theme unless the diagram names its own,
//! with text measured by the caller's font, and its layouts held to a work budget.

use std::cell::RefCell;

use crate::parse::{Diagram, DiagramMeta};
use crate::svg::Theme;

/// The width, in em, of one line of text.
pub type Measure = Box<dyn Fn(&str) -> f64>;

thread_local! {
    static MEASURE: RefCell<Option<Measure>> = const { RefCell::new(None) };
}

/// Measure text with `measure` on this thread from now on (`None`: the crate's estimate).
pub fn set_measure(measure: Option<Measure>) {
    MEASURE.with(|m| *m.borrow_mut() = measure);
}

/// The width of `s` in px at `font_size`, by the measure set, if one is.
pub(crate) fn measured(s: &str, font_size: f64) -> Option<f64> {
    MEASURE.with(|m| m.borrow().as_ref().map(|f| f(s) * font_size))
}

/// Why a diagram wasn't rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Its layout was over a limit or ran out of its work budget.
    TooComplex,
}

/// Renders `d` (parsed with its `meta`) for VOMD: the `dark` or `default` theme unless the
/// diagram names one, at a fixed size, its layouts within `budget` units of work.
pub fn render(
    d: &Diagram,
    meta: &DiagramMeta,
    dark: bool,
    budget: usize,
) -> Result<String, Failure> {
    let base = if dark {
        Theme::by_name("dark")
    } else {
        Theme::by_name("default")
    };
    let mut theme = crate::svg::theme_from_meta(&base.unwrap_or_default(), meta);
    theme.responsive = false;
    crate::sugiyama::set_budget(Some(budget));
    let svg = crate::svg::render_meta(d, meta, &theme);
    let failed = crate::sugiyama::failed();
    crate::sugiyama::set_budget(None);
    if failed {
        Err(Failure::TooComplex)
    } else {
        Ok(svg)
    }
}

/// A flowchart's layout, for tests of it: each node's box, each subgraph's box and the nodes
/// in it (its subgraphs' too), and each edge's route (`None`: left out), in the order of the
/// source.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct Geometry {
    pub nodes: Vec<(String, [f64; 4])>,
    pub groups: Vec<(String, [f64; 4], Vec<String>)>,
    pub edges: Vec<(String, String, Option<Vec<(f64, f64)>>)>,
}

/// The layout of the flowchart `src` (in the default theme, measured by the measure set).
#[doc(hidden)]
pub fn flowchart_geometry(src: &str) -> Option<Geometry> {
    let Ok((Diagram::Flowchart(d), meta)) = crate::parse::parse_with_meta(src) else {
        return None;
    };
    let theme = crate::svg::theme_from_meta(&Theme::default(), &meta);
    let sizes = crate::svg::flowchart::node_sizes(&d, &theme);
    crate::sugiyama::set_budget(None);
    let laid = crate::svg::flowchart::laid_out(&d, &theme, &sizes)?;
    let nodes = d
        .nodes
        .iter()
        .zip(&sizes)
        .zip(&laid.pos)
        .map(|((n, &(w, h)), &(x, y))| {
            (
                n.id.clone(),
                [x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0],
            )
        })
        .collect();
    let by_id: std::collections::HashMap<&str, &crate::parse::Subgraph> =
        d.subgraphs.iter().map(|s| (s.id.as_str(), s)).collect();
    let members = |s: &crate::parse::Subgraph| {
        let mut out = Vec::new();
        let mut stack = vec![s];
        let mut seen = 0;
        while let Some(s) = stack.pop() {
            seen += 1;
            if seen > d.subgraphs.len() + 1 {
                break;
            }
            out.extend(s.node_ids.iter().cloned());
            stack.extend(
                s.child_subgraph_ids
                    .iter()
                    .filter_map(|c| by_id.get(c.as_str())),
            );
        }
        out
    };
    let groups = d
        .subgraphs
        .iter()
        .zip(&laid.boxes)
        .filter_map(|(s, b)| {
            let b = (*b)?;
            Some((s.id.clone(), [b.0, b.1, b.2, b.3], members(s)))
        })
        .collect();
    let edges = d
        .edges
        .iter()
        .zip(&laid.routes)
        .map(|(e, r)| {
            (
                e.from.clone(),
                e.to.clone(),
                r.as_ref().map(|r| r.points.clone()),
            )
        })
        .collect();
    Some(Geometry {
        nodes,
        groups,
        edges,
    })
}

/// The notes of the state diagram `src` and their states, as boxes `(state, note)` in the
/// order of the source (for VOMD's tests of where notes go).
#[doc(hidden)]
pub fn state_note_boxes(src: &str) -> Vec<([f64; 4], [f64; 4])> {
    let Ok((d, meta)) = crate::parse::parse_with_meta(src) else {
        return Vec::new();
    };
    if !matches!(d, Diagram::State(_)) {
        return Vec::new();
    }
    crate::sugiyama::set_budget(None);
    let theme = crate::svg::theme_from_meta(&Theme::default(), &meta);
    let _ = crate::svg::render_meta(&d, &meta, &theme);
    crate::svg::state::last_notes()
}

/// The transitions of the state diagram `src` as drawn, as polylines (for VOMD's tests of where
/// edges go around notes).
#[doc(hidden)]
pub fn state_edges(src: &str) -> Vec<Vec<(f64, f64)>> {
    let _ = state_note_boxes(src);
    crate::svg::state::last_edges()
}

/// Whether a colour reads as light (the `autonumber` badge's digits follow it), for VOMD's
/// tests.
#[doc(hidden)]
pub fn is_light(color: &str) -> bool {
    crate::svg::sequence::light_color(color)
}
