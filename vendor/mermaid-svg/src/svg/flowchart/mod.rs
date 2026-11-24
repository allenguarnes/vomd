//! Flowchart renderer: lays the AST out (VOMD: with `compound`, subgraphs as compound
//! nodes), then draws shapes, clipped polyline edges, and subgraph frames.

use std::collections::HashMap;

use crate::parse::{FlowDirection, FlowchartDiagram, Subgraph};
use crate::sugiyama::{Dir, NodeId};

use super::compound;

use super::builder::SvgBuilder;
use super::style::resolve_edge_style;
use super::theme::Theme;

mod edges;
mod nodes;
mod shapes;

use edges::*;
use nodes::*;

pub(super) const CHAR_W: f64 = 7.5;
pub(super) const LINE_H: f64 = 20.0;
pub(super) const PAD_X: f64 = 18.0;
pub(super) const PAD_Y: f64 = 12.0;
pub(super) const MIN_W: f64 = 60.0;
pub(super) const MIN_H: f64 = 40.0;
const CANVAS_PAD: f64 = 24.0;
/// Vertical space reserved above the graph for a frontmatter title.
const TITLE_BAND: f64 = 34.0;

pub(crate) fn render(d: &FlowchartDiagram, theme: &Theme) -> String {
    if d.nodes.is_empty() {
        let mut svg = SvgBuilder::new(40.0, 40.0).theme(theme);
        define_markers(&mut svg, theme);
        return svg.finish();
    }

    // VOMD: laid out by dugong (see svg/compound.rs), subgraphs as compound nodes.
    let title_h = if d.title.is_some() { TITLE_BAND } else { 0.0 };
    let node_sizes = node_sizes(d, theme);
    let sub_ids: std::collections::HashSet<&str> =
        d.subgraphs.iter().map(|s| s.id.as_str()).collect();
    let mut id_to_u32: HashMap<String, NodeId> = HashMap::new();
    for (i, n) in d.nodes.iter().enumerate() {
        if !sub_ids.contains(n.id.as_str()) {
            id_to_u32.entry(n.id.clone()).or_insert(i as NodeId);
        }
    }
    let Some(fl) = laid_out(d, theme, &node_sizes) else {
        return SvgBuilder::new(40.0, 40.0).theme(theme).finish();
    };
    let off = |(x, y): (f64, f64)| (x + CANVAS_PAD, y + CANVAS_PAD + title_h);
    let pos: HashMap<NodeId, (f64, f64)> = id_to_u32
        .values()
        .map(|&u| (u, off(fl.pos[u as usize])))
        .collect();
    let boxes: HashMap<String, (f64, f64, f64, f64)> = d
        .subgraphs
        .iter()
        .zip(&fl.boxes)
        .filter_map(|(s, b)| {
            let &(x0, y0, x1, y1) = b.as_ref()?;
            let (a, b) = off((x0, y0));
            let (c, e) = off((x1, y1));
            Some((s.id.clone(), (a, b, c, e)))
        })
        .collect();

    // Canvas: expand the global extent to include any locally moved nodes and
    // subgraph frames so nothing is clipped by the viewport.
    let mut max_x = 0.0_f64;
    let mut max_y = 0.0_f64;
    for (u, &(x, y)) in &pos {
        let (w, h) = node_sizes[*u as usize];
        max_x = max_x.max(x + w / 2.0);
        max_y = max_y.max(y + h / 2.0);
    }
    for &(_, _, bx1, by1) in boxes.values() {
        max_x = max_x.max(bx1);
        max_y = max_y.max(by1);
    }
    for r in fl.routes.iter().flatten() {
        for &p in r.points.iter().chain(r.label.iter()) {
            let (x, y) = off(p);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    let mut width = max_x + CANVAS_PAD;
    let height = max_y + CANVAS_PAD;

    // A long title can be wider than the graph itself; grow the canvas to fit.
    if let Some(t) = &d.title {
        let title_w =
            crate::svg::metrics::text_width(t, CHAR_W + 2.0, theme.font_size) + CANVAS_PAD * 2.0;
        width = width.max(title_w);
    }

    let mut svg = SvgBuilder::new(width, height).theme(theme);
    define_markers(&mut svg, theme);

    if let Some(t) = &d.title {
        let fg = &theme.fg;
        svg.text(
            width / 2.0,
            CANVAS_PAD + 6.0,
            &format!("text-anchor=\"middle\" fill=\"{fg}\" font-size=\"18\" font-weight=\"bold\""),
            t,
        );
    }

    // Subgraph frames (drawn first so they sit under nodes/edges).
    draw_subgraphs(&mut svg, d, &boxes, theme);

    // Edges.
    for (ei, fedge) in d.edges.iter().enumerate() {
        let edge_style = resolve_edge_style(&d.link_style_default, d.edge_styles.get(&ei));
        // Curve precedence: per-edge `@{ curve }`, then `linkStyle N interpolate`,
        // then `linkStyle default interpolate`, then the `config.flowchart.curve`
        // diagram default, else the default basis.
        let curve = fedge
            .curve
            .or_else(|| d.edge_interpolate.get(&ei).copied())
            .or(d.default_interpolate)
            .or(d.config_curve)
            .unwrap_or_default();
        let Some(route) = &fl.routes[ei] else {
            continue;
        };
        let pts: Vec<(f64, f64)> = route.points.iter().map(|&p| off(p)).collect();
        if pts.len() < 2 {
            continue;
        }
        let start = if route.from_cut {
            EndClip::point(pts[0])
        } else {
            match endpoint_clip(&fedge.from, &id_to_u32, &d.nodes, &node_sizes, &pos, &boxes) {
                Some(c) => c,
                None => continue,
            }
        };
        let end = if route.to_cut {
            EndClip::point(pts[pts.len() - 1])
        } else {
            match endpoint_clip(&fedge.to, &id_to_u32, &d.nodes, &node_sizes, &pos, &boxes) {
                Some(c) => c,
                None => continue,
            }
        };
        let label_at = route.label.map(off);
        draw_edge(
            &mut svg,
            &pts,
            fedge,
            curve,
            &edge_style,
            &start,
            &end,
            label_at,
            theme,
        );
    }

    draw_subgraph_titles(&mut svg, d, &boxes, theme);

    // Nodes.
    for (i, node) in d.nodes.iter().enumerate() {
        let Some(&center) = pos.get(&(i as NodeId)) else {
            continue;
        };
        let size = node_sizes[i];
        draw_node(&mut svg, center, size, node, &d.class_defs, theme);
    }

    svg.finish()
}

#[cfg(test)]
mod tests;

fn subgraph_title(s: &Subgraph) -> &str {
    if s.label.is_empty() {
        &s.id
    } else {
        &s.label
    }
}

/// VOMD: the flowchart laid out (see svg/compound.rs), its nodes `node_sizes` big.
pub(crate) fn laid_out(
    d: &FlowchartDiagram,
    theme: &Theme,
    node_sizes: &[(f64, f64)],
) -> Option<compound::Laid> {
    let font_size = theme.font_size;
    let label_size = |t: &str| {
        (
            crate::svg::metrics::text_width(t, 7.0, font_size) + 8.0,
            18.0,
        )
    };
    let dir_of = |d: FlowDirection| match d {
        FlowDirection::TopDown => Dir::TB,
        FlowDirection::BottomTop => Dir::BT,
        FlowDirection::LeftRight => Dir::LR,
        FlowDirection::RightLeft => Dir::RL,
    };
    let spec = compound::Spec {
        dir: dir_of(d.direction),
        nodesep: 50.0,
        ranksep: 50.0,
        margin: 8.0,
        title_h: 26.0,
        title_lift: 12.0,
        pad: 8.0,
        nodes: d
            .nodes
            .iter()
            .zip(node_sizes)
            .map(|(n, &(w, h))| compound::SNode { id: &n.id, w, h })
            .collect(),
        groups: d
            .subgraphs
            .iter()
            .map(|s| compound::SGroup {
                id: &s.id,
                title_w: label_size(subgraph_title(s)).0,
                dir: s.direction.map(dir_of),
                nodes: s.node_ids.iter().map(String::as_str).collect(),
                groups: s.child_subgraph_ids.iter().map(String::as_str).collect(),
            })
            .collect(),
        edges: d
            .edges
            .iter()
            .map(|e| compound::SEdge {
                from: &e.from,
                to: &e.to,
                label: e
                    .label
                    .as_deref()
                    .filter(|l| !l.is_empty())
                    .map_or((0.0, 0.0), label_size),
                minlen: 1,
            })
            .collect(),
    };
    compound::layout(&spec)
}

/// VOMD: the sizes the flowchart's nodes are drawn at.
pub(crate) fn node_sizes(d: &FlowchartDiagram, theme: &Theme) -> Vec<(f64, f64)> {
    d.nodes
        .iter()
        .map(|n| node_size(n, theme.font_size))
        .collect()
}
