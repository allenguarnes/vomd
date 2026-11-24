//! Layered graph layout.
//!
//! VOMD: the crate's own Sugiyama layout is replaced by dugong (a port of Dagre, the layout
//! Mermaid itself uses). [`layout_with`] keeps the old interface for the diagrams that drew
//! with it; [`layout_compound`] adds compound nodes (clusters), edge-label room and the
//! direction of the ranks, for the flowchart and the state diagram.
//!
//! All layouts of one render share a work budget ([`set_budget`]); once it or a size limit is
//! exceeded, layouts return `None` (or an empty [`Layout`]) and [`failed`] says so.

use std::cell::Cell;
use std::collections::HashMap;

use dugong::graphlib::{Graph as DGraph, GraphOptions};
use dugong::{EdgeLabel, GraphLabel, NodeLabel, RankDir, WorkControl, WorkError};
use thiserror::Error;

pub type NodeId = u32;

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: Vec<NodeId>,
    pub edges: Vec<(NodeId, NodeId)>,
    pub node_size: HashMap<NodeId, (f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub node_pos: HashMap<NodeId, (f64, f64)>,
    /// Polyline per original edge, from the centre of its source to that of its target.
    pub edge_points: HashMap<(NodeId, NodeId), Vec<(f64, f64)>>,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // VOMD: the crossing and coordinate passes are Dagre's now.
pub struct LayoutConfig {
    pub layer_gap: f64,
    pub node_gap: f64,
    pub max_crossing_iter: usize,
    pub max_coord_iter: usize,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            layer_gap: 80.0,
            node_gap: 40.0,
            max_crossing_iter: 24,
            max_coord_iter: 24,
        }
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum LayoutError {
    #[error("unknown node id {0}")]
    UnknownNode(NodeId),
    #[error("missing size for node {0}")]
    MissingSize(NodeId),
    #[error("duplicate node id {0}")]
    DuplicateNode(NodeId),
}

/// The most nodes (clusters included) and edges one layout takes.
pub const MAX_NODES: usize = 2_000;
pub const MAX_EDGES: usize = 4_000;

thread_local! {
    static BUDGET: Cell<Option<usize>> = const { Cell::new(None) };
    static FAILED: Cell<bool> = const { Cell::new(false) };
}

/// Sets the work budget of the layouts that follow on this thread (`None`: unlimited), and
/// clears [`failed`].
pub fn set_budget(units: Option<usize>) {
    BUDGET.with(|b| b.set(units));
    FAILED.with(|f| f.set(false));
}

/// Whether a layout since [`set_budget`] ran out of budget or was over a size limit.
pub fn failed() -> bool {
    FAILED.with(|f| f.get())
}

fn fail() {
    FAILED.with(|f| f.set(true));
}

struct Budget;

impl WorkControl for Budget {
    fn charge(&mut self, units: usize) -> Result<(), WorkError> {
        BUDGET.with(|b| match b.get() {
            None => Ok(()),
            Some(left) if units <= left => {
                b.set(Some(left - units));
                Ok(())
            }
            Some(_) => {
                b.set(Some(0));
                Err(WorkError::Interrupted)
            }
        })
    }
}

/// The direction ranks run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dir {
    #[default]
    TB,
    BT,
    LR,
    RL,
}

impl Dir {
    pub fn horizontal(self) -> bool {
        matches!(self, Dir::LR | Dir::RL)
    }
}

/// A node of a compound graph: a node with children is a cluster, whose box the layout
/// makes around them (its own size is then ignored).
#[derive(Debug, Clone, Default)]
pub struct CNode {
    pub w: f64,
    pub h: f64,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct CEdge {
    pub v: usize,
    pub w: usize,
    /// The size of the edge's label, `(0, 0)` for none.
    pub label: (f64, f64),
    pub minlen: usize,
}

#[derive(Debug, Clone)]
pub struct CGraph {
    pub dir: Dir,
    pub nodesep: f64,
    pub ranksep: f64,
    pub margin: f64,
    pub nodes: Vec<CNode>,
    pub edges: Vec<CEdge>,
}

#[derive(Debug, Clone, Default)]
pub struct CEdgeOut {
    /// From the source's boundary to the target's, as Dagre routes it.
    pub points: Vec<(f64, f64)>,
    /// The centre of the label, if the edge has one.
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub struct CLayout {
    /// Centre, width and height of each node (of each cluster, its box).
    pub nodes: Vec<(f64, f64, f64, f64)>,
    pub edges: Vec<CEdgeOut>,
    pub width: f64,
    pub height: f64,
}

fn clean(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1e6)
    } else {
        0.0
    }
}

/// Lays out a compound graph with Dagre. `None` when the graph is over a limit or the budget
/// runs out (and [`failed`] is then true).
pub fn layout_compound(cg: &CGraph) -> Option<CLayout> {
    if failed() || cg.nodes.len() > MAX_NODES || cg.edges.len() > MAX_EDGES {
        fail();
        return None;
    }
    let mut g: DGraph<NodeLabel, EdgeLabel, GraphLabel> = DGraph::new(GraphOptions {
        multigraph: true,
        compound: true,
        directed: true,
    });
    g.set_graph(GraphLabel {
        rankdir: match cg.dir {
            Dir::TB => RankDir::TB,
            Dir::BT => RankDir::BT,
            Dir::LR => RankDir::LR,
            Dir::RL => RankDir::RL,
        },
        nodesep: clean(cg.nodesep),
        ranksep: clean(cg.ranksep),
        marginx: clean(cg.margin),
        marginy: clean(cg.margin),
        ..GraphLabel::default()
    });
    let id = |i: usize| format!("n{i}");
    for (i, n) in cg.nodes.iter().enumerate() {
        g.set_node(
            id(i),
            NodeLabel {
                width: clean(n.w),
                height: clean(n.h),
                ..NodeLabel::default()
            },
        );
    }
    for (i, n) in cg.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            // A parent must be another node, and no node its own ancestor.
            if p >= cg.nodes.len() || p == i || is_ancestor(cg, i, p) {
                continue;
            }
            g.set_parent(id(i), id(p));
        }
    }
    // An edge repeated without a label is laid out once (its copies take its route): repeated
    // edges between nested compound nodes left Dagre's ordering unsound.
    let mut seen = std::collections::HashSet::new();
    let mut first_of: Vec<usize> = (0..cg.edges.len()).collect();
    let mut firsts: HashMap<(usize, usize), usize> = HashMap::new();
    for (i, e) in cg.edges.iter().enumerate() {
        if e.v >= cg.nodes.len() || e.w >= cg.nodes.len() {
            continue;
        }
        if e.label == (0.0, 0.0) {
            let f = *firsts.entry((e.v, e.w)).or_insert(i);
            first_of[i] = f;
            if !seen.insert((e.v, e.w)) {
                continue;
            }
        }
        let (lw, lh) = (clean(e.label.0), clean(e.label.1));
        g.set_edge_named(
            id(e.v),
            id(e.w),
            Some(format!("e{i}")),
            Some(EdgeLabel {
                width: lw,
                height: lh,
                minlen: e.minlen.min(8),
                weight: 1.0,
                ..EdgeLabel::default()
            }),
        );
    }
    if dugong::layout_controlled(&mut g, &mut Budget).is_err() {
        fail();
        return None;
    }
    let nodes = (0..cg.nodes.len())
        .map(|i| {
            let n = g.node(&id(i));
            n.map_or((0.0, 0.0, 0.0, 0.0), |n| {
                (n.x.unwrap_or(0.0), n.y.unwrap_or(0.0), n.width, n.height)
            })
        })
        .collect();
    let edges = cg
        .edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let name = format!("e{}", first_of[i]);
            let Some(l) = g.edge(&id(e.v), &id(e.w), Some(&name)) else {
                return CEdgeOut::default();
            };
            CEdgeOut {
                points: l.points.iter().map(|p| (p.x, p.y)).collect(),
                label: match (l.x, l.y) {
                    (Some(x), Some(y)) if e.label.0 > 0.0 => Some((x, y)),
                    _ => None,
                },
            }
        })
        .collect();
    let gl = g.graph();
    Some(CLayout {
        nodes,
        edges,
        width: gl.width,
        height: gl.height,
    })
}

/// Whether `a` is an ancestor of `n` through the parents given (cycle-safe).
fn is_ancestor(cg: &CGraph, a: usize, n: usize) -> bool {
    let mut cur = cg.nodes.get(n).and_then(|x| x.parent);
    let mut steps = 0;
    while let Some(c) = cur {
        if c == a {
            return true;
        }
        steps += 1;
        if steps > cg.nodes.len() {
            return true;
        }
        cur = cg.nodes.get(c).and_then(|x| x.parent);
    }
    false
}

/// The old interface: ranks top to bottom (callers turn the picture for other directions),
/// each edge's polyline from centre to centre. Over a limit or the budget, every node is at
/// the origin (and [`failed`] is true): the caller draws nothing of it.
pub fn layout_with(g: &Graph, cfg: &LayoutConfig) -> Result<Layout, LayoutError> {
    let mut index: HashMap<NodeId, usize> = HashMap::new();
    let mut nodes = Vec::with_capacity(g.nodes.len());
    for &u in &g.nodes {
        if index.insert(u, nodes.len()).is_some() {
            return Err(LayoutError::DuplicateNode(u));
        }
        let &(w, h) = g.node_size.get(&u).ok_or(LayoutError::MissingSize(u))?;
        nodes.push(CNode { w, h, parent: None });
    }
    let mut edges = Vec::with_capacity(g.edges.len());
    for &(u, v) in &g.edges {
        let a = *index.get(&u).ok_or(LayoutError::UnknownNode(u))?;
        let b = *index.get(&v).ok_or(LayoutError::UnknownNode(v))?;
        edges.push(CEdge {
            v: a,
            w: b,
            label: (0.0, 0.0),
            minlen: 1,
        });
    }
    let cg = CGraph {
        dir: Dir::TB,
        nodesep: cfg.node_gap,
        ranksep: cfg.layer_gap,
        margin: 0.0,
        nodes,
        edges,
    };
    let Some(l) = layout_compound(&cg) else {
        let node_pos = g.nodes.iter().map(|&u| (u, (0.0, 0.0))).collect();
        return Ok(Layout {
            node_pos,
            ..Layout::default()
        });
    };
    let node_pos: HashMap<NodeId, (f64, f64)> = g
        .nodes
        .iter()
        .enumerate()
        .map(|(i, &u)| (u, (l.nodes[i].0, l.nodes[i].1)))
        .collect();
    let mut edge_points = HashMap::new();
    for (i, &(u, v)) in g.edges.iter().enumerate() {
        let (a, b) = (node_pos[&u], node_pos[&v]);
        let pts = &l.edges[i].points;
        let mut out = Vec::with_capacity(pts.len().max(2));
        out.push(a);
        if u == v {
            // A self-loop: Dagre's route around the node, ending where it starts.
            out.extend(pts.iter().copied());
            out.push(a);
        } else if pts.len() > 2 {
            out.extend(pts[1..pts.len() - 1].iter().copied());
            out.push(b);
        } else {
            out.push(b);
        }
        edge_points.entry((u, v)).or_insert(out);
    }
    Ok(Layout {
        node_pos,
        edge_points,
        width: l.width,
        height: l.height,
    })
}
