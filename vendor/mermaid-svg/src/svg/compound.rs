//! VOMD: layout of a graph with groups (flowchart subgraphs, composite states), with dugong
//! (Dagre) as Mermaid does it.
//!
//! - A group is a compound node: Dagre keeps its members together and makes its box.
//! - An edge to or from a group goes to one of its nodes (its "anchor") and is cut where it
//!   crosses the group's box.
//! - A group with no edge crossing its border is laid out on its own, in its own direction
//!   (Mermaid's default for one without: across the parent's), and is then one node of its
//!   parent's graph. A group with such edges takes the parent's direction, as in Mermaid.

use std::collections::{HashMap, HashSet};

use crate::sugiyama::{layout_compound, CEdge, CGraph, CNode, Dir};

/// What to lay out. Ids are the source's: an edge's end names a node or a group.
#[derive(Debug, Default)]
pub(crate) struct Spec<'a> {
    pub(crate) dir: Dir,
    pub(crate) nodesep: f64,
    pub(crate) ranksep: f64,
    pub(crate) margin: f64,
    /// Room above a group's members for its title, and around a group laid out alone.
    pub(crate) title_h: f64,
    /// Room added above a compound group's members (Dagre's gap is too small for a title).
    pub(crate) title_lift: f64,
    pub(crate) pad: f64,
    pub(crate) nodes: Vec<SNode<'a>>,
    pub(crate) groups: Vec<SGroup<'a>>,
    pub(crate) edges: Vec<SEdge<'a>>,
}

#[derive(Debug)]
pub(crate) struct SNode<'a> {
    pub(crate) id: &'a str,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

#[derive(Debug)]
pub(crate) struct SGroup<'a> {
    pub(crate) id: &'a str,
    /// The width the title needs (the box is at least this wide).
    pub(crate) title_w: f64,
    pub(crate) dir: Option<Dir>,
    pub(crate) nodes: Vec<&'a str>,
    pub(crate) groups: Vec<&'a str>,
}

#[derive(Debug)]
pub(crate) struct SEdge<'a> {
    pub(crate) from: &'a str,
    pub(crate) to: &'a str,
    /// The label's size, `(0, 0)` for none.
    pub(crate) label: (f64, f64),
    pub(crate) minlen: usize,
}

/// One edge as laid out.
#[derive(Debug, Clone, Default)]
pub(crate) struct Route {
    /// Source to target. An end at a node is the node's centre (the drawing clips it to the
    /// shape), as is one at a group laid out alone; an end at another group is already on
    /// its box (`from_cut`, `to_cut`), as are a self-loop's.
    pub(crate) points: Vec<(f64, f64)>,
    pub(crate) label: Option<(f64, f64)>,
    pub(crate) from_cut: bool,
    pub(crate) to_cut: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Laid {
    /// Centre of each node, by index into `Spec::nodes` (a node named like a group: unused).
    pub(crate) pos: Vec<(f64, f64)>,
    /// Each group's box `(x0, y0, x1, y1)`, by index into `Spec::groups` (`None`: empty).
    pub(crate) boxes: Vec<Option<(f64, f64, f64, f64)>>,
    /// By index into `Spec::edges`; `None` for an edge left out (an unknown end).
    pub(crate) routes: Vec<Option<Route>>,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// What an item of one level's graph is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Item {
    Node(usize),
    Group(usize),
}

/// An edge's level and its ends there, and the groups its ends were cut at.
type EdgeLevel = Option<(Option<usize>, Item, Item, Option<usize>, Option<usize>)>;

struct Ctx<'s, 'a> {
    spec: &'s Spec<'a>,
    node_parent: Vec<Option<usize>>,
    group_parent: Vec<Option<usize>>,
    /// Laid out alone (no edge crosses its border).
    alone: Vec<bool>,
    /// The node an edge to the group goes to; `None` for an empty group.
    anchor: Vec<Option<usize>>,
    group_index: HashMap<&'a str, usize>,
    node_index: HashMap<&'a str, usize>,
    /// Nodes named like a group: the group stands for them.
    hidden: HashSet<usize>,
    edge_level: Vec<EdgeLevel>,
}

impl Ctx<'_, '_> {
    fn parent(&self, it: Item) -> Option<usize> {
        match it {
            Item::Node(n) => self.node_parent[n],
            Item::Group(g) => self.group_parent[g],
        }
    }

    /// The level (`None`: the top) whose graph `it` is an item of: its nearest ancestor laid
    /// out alone.
    fn level(&self, it: Item) -> Option<usize> {
        let mut cur = self.parent(it);
        for _ in 0..=self.alone.len() {
            let s = cur?;
            if self.alone[s] {
                return Some(s);
            }
            cur = self.group_parent[s];
        }
        None
    }

    /// How many groups `g` is inside.
    fn depth(&self, g: usize) -> usize {
        let mut k = 0;
        bounded_walk(&self.group_parent, self.group_parent[g], |_| {
            k += 1;
            false
        });
        k
    }

    /// The levels `it` is inside, innermost first, the top (`None`) last.
    fn levels(&self, it: Item) -> Vec<Option<usize>> {
        let mut out = Vec::new();
        let mut cur = self.level(it);
        while let Some(s) = cur {
            out.push(Some(s));
            if out.len() > self.alone.len() {
                break;
            }
            cur = self.level(Item::Group(s));
        }
        out.push(None);
        out
    }

    /// Whether `it` is laid out in level `lv` or a level inside it.
    fn within(&self, it: Item, lv: Option<usize>) -> bool {
        lv.is_none() || self.levels(it).contains(&lv)
    }

    /// The item of level `lv`'s graph that holds `it`.
    fn item_in(&self, it: Item, lv: Option<usize>) -> Option<Item> {
        let mut cur = it;
        for _ in 0..=self.alone.len() + 1 {
            if self.level(cur) == lv {
                return Some(cur);
            }
            cur = Item::Group(self.level(cur)?);
        }
        None
    }

    /// An edge's end: a node, a group laid out alone, or the anchor of another group (and
    /// that group).
    fn end(&self, id: &str) -> Option<(Item, Option<usize>)> {
        if let Some(&g) = self.group_index.get(id) {
            if self.alone[g] {
                return Some((Item::Group(g), None));
            }
            return Some((Item::Node(self.anchor[g]?), Some(g)));
        }
        self.node_index.get(id).map(|&n| (Item::Node(n), None))
    }
}

fn bounded_walk(parent: &[Option<usize>], from: Option<usize>, mut f: impl FnMut(usize) -> bool) {
    let mut cur = from;
    for _ in 0..=parent.len() {
        let Some(c) = cur else { return };
        if f(c) {
            return;
        }
        cur = parent[c];
    }
}

/// Lays out `spec`. `None` when a limit or the work budget was hit.
pub(crate) fn layout(spec: &Spec) -> Option<Laid> {
    let n_groups = spec.groups.len();
    let mut group_index: HashMap<&str, usize> = HashMap::new();
    for (i, g) in spec.groups.iter().enumerate() {
        group_index.entry(g.id).or_insert(i);
    }
    let mut node_index: HashMap<&str, usize> = HashMap::new();
    for (i, n) in spec.nodes.iter().enumerate() {
        node_index.entry(n.id).or_insert(i);
    }
    // A group named again is the first of the name: the later ones are left out.
    let dup = |i: usize| group_index.get(spec.groups[i].id) != Some(&i);
    let mut group_parent: Vec<Option<usize>> = vec![None; n_groups];
    for (i, g) in spec.groups.iter().enumerate() {
        if dup(i) {
            continue;
        }
        for c in &g.groups {
            if let Some(&ci) = group_index.get(c) {
                if ci != i && group_parent[ci].is_none() {
                    group_parent[ci] = Some(i);
                }
            }
        }
    }
    // Break any cycle of parents (a malformed source).
    for i in 0..n_groups {
        let mut cyclic = false;
        bounded_walk(&group_parent, group_parent[i], |c| {
            cyclic = c == i;
            cyclic
        });
        if cyclic {
            group_parent[i] = None;
        }
    }
    let depth = |g: usize| {
        let mut k = 0;
        bounded_walk(&group_parent, group_parent[g], |_| {
            k += 1;
            false
        });
        k
    };
    // A node listed by several groups is in the deepest.
    let mut node_parent: Vec<Option<usize>> = vec![None; spec.nodes.len()];
    for (gi, g) in spec.groups.iter().enumerate() {
        if dup(gi) {
            continue;
        }
        for id in &g.nodes {
            if group_index.contains_key(id) {
                continue;
            }
            if let Some(&n) = node_index.get(id) {
                if node_parent[n].is_none_or(|p| depth(gi) >= depth(p)) {
                    node_parent[n] = Some(gi);
                }
            }
        }
    }
    let hidden: HashSet<usize> = spec
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| group_index.contains_key(n.id))
        .map(|(i, _)| i)
        .collect();

    // Each group's nodes, its groups' included.
    let mut members: Vec<HashSet<usize>> = vec![HashSet::new(); n_groups];
    for (n, p) in node_parent.iter().enumerate() {
        if hidden.contains(&n) {
            continue;
        }
        bounded_walk(&group_parent, *p, |g| {
            members[g].insert(n);
            false
        });
    }
    let is_desc_group = |g: usize, of: usize| {
        let mut found = false;
        bounded_walk(&group_parent, group_parent[g], |c| {
            found = c == of;
            found
        });
        found
    };
    // Whether an edge's end is inside group `g` (not `g` itself).
    let inside = |id: &str, g: usize| -> bool {
        if let Some(&t) = group_index.get(id) {
            return is_desc_group(t, g);
        }
        node_index.get(id).is_some_and(|n| members[g].contains(n))
    };
    let mut alone = vec![false; n_groups];
    let mut anchor = vec![None; n_groups];
    for g in 0..n_groups {
        let crossing = spec
            .edges
            .iter()
            .any(|e| inside(e.from, g) != inside(e.to, g));
        // The first of its nodes, in the order of the source.
        anchor[g] = if dup(g) {
            None
        } else {
            members[g].iter().copied().min()
        };
        alone[g] = !crossing && anchor[g].is_some();
    }

    let mut ctx = Ctx {
        spec,
        node_parent,
        group_parent,
        alone,
        anchor,
        group_index,
        node_index,
        hidden,
        edge_level: Vec::new(),
    };
    // Which level each edge is laid out in, and its ends there.
    let mut edge_level = Vec::with_capacity(spec.edges.len());
    for e in &spec.edges {
        let (Some((a, a_cut)), Some((b, b_cut))) = (ctx.end(e.from), ctx.end(e.to)) else {
            edge_level.push(None);
            continue;
        };
        let la = ctx.levels(a);
        let lb = ctx.levels(b);
        let Some(&lv) = la.iter().find(|l| lb.contains(l)) else {
            edge_level.push(None);
            continue;
        };
        match (ctx.item_in(a, lv), ctx.item_in(b, lv)) {
            (Some(ia), Some(ib)) => edge_level.push(Some((lv, ia, ib, a_cut, b_cut))),
            _ => edge_level.push(None),
        }
    }
    ctx.edge_level = edge_level;

    let mut out = Laid {
        pos: vec![(0.0, 0.0); spec.nodes.len()],
        boxes: vec![None; n_groups],
        routes: vec![None; spec.edges.len()],
        ..Laid::default()
    };
    let (w, h) = lay_level(&ctx, None, spec.dir, &mut out)?;
    out.width = w;
    out.height = h;
    Some(out)
}

/// Lays out level `lv` (a group laid out alone, or the top), its own alone groups first,
/// writing positions relative to the level's top left corner. Returns its size.
fn lay_level(ctx: &Ctx, lv: Option<usize>, dir: Dir, out: &mut Laid) -> Option<(f64, f64)> {
    let spec = ctx.spec;
    // The items of this level, groups first (as Mermaid adds them), then nodes.
    let mut items: Vec<Item> = Vec::new();
    for g in 0..spec.groups.len() {
        if ctx.anchor[g].is_some() && ctx.level(Item::Group(g)) == lv {
            items.push(Item::Group(g));
        }
    }
    for n in 0..spec.nodes.len() {
        if !ctx.hidden.contains(&n) && ctx.level(Item::Node(n)) == lv {
            items.push(Item::Node(n));
        }
    }
    let index: HashMap<Item, usize> = items.iter().enumerate().map(|(i, &it)| (it, i)).collect();

    // Groups laid out alone, inside this level: their size first.
    let mut alone_size: HashMap<usize, (f64, f64)> = HashMap::new();
    for &it in &items {
        if let Item::Group(g) = it {
            if ctx.alone[g] {
                let inner = match spec.groups[g].dir {
                    Some(d) => d,
                    None if dir.horizontal() => Dir::TB,
                    None => Dir::LR,
                };
                let (w, h) = lay_level(ctx, Some(g), inner, out)?;
                let title_w = spec.groups[g].title_w + 2.0 * spec.pad;
                alone_size.insert(
                    g,
                    (
                        (w + 2.0 * spec.pad).max(title_w),
                        h + 2.0 * spec.pad + title_h(spec, g),
                    ),
                );
            }
        }
    }

    let nodes: Vec<CNode> = items
        .iter()
        .map(|&it| {
            let (w, h) = match it {
                Item::Node(n) => (spec.nodes[n].w, spec.nodes[n].h),
                Item::Group(g) => alone_size.get(&g).copied().unwrap_or((0.0, 0.0)),
            };
            let parent = match ctx.parent(it) {
                Some(p) if Some(p) != lv => index.get(&Item::Group(p)).copied(),
                _ => None,
            };
            CNode { w, h, parent }
        })
        .collect();
    let mut edges = Vec::new();
    let mut edge_ids = Vec::new();
    for (ei, el) in ctx.edge_level.iter().enumerate() {
        let Some((elv, a, b, _, _)) = el else {
            continue;
        };
        if *elv != lv {
            continue;
        }
        let (Some(&ia), Some(&ib)) = (index.get(a), index.get(b)) else {
            continue;
        };
        edges.push(CEdge {
            v: ia,
            w: ib,
            label: spec.edges[ei].label,
            minlen: spec.edges[ei].minlen,
        });
        edge_ids.push(ei);
    }
    let cg = CGraph {
        dir,
        nodesep: spec.nodesep,
        ranksep: spec.ranksep,
        margin: spec.margin,
        nodes,
        edges,
    };
    let l = layout_compound(&cg)?;

    // Boxes of this level's groups, and its nodes. A compound group's title: Dagre leaves a
    // rank's gap above its members; the box is widened for a long title.
    let mut min_x = 0.0_f64;
    let mut max_x = l.width;
    let max_y = l.height;
    for (i, &it) in items.iter().enumerate() {
        let (cx, cy, w, h) = l.nodes[i];
        match it {
            Item::Node(n) => out.pos[n] = (cx, cy),
            Item::Group(g) if ctx.alone[g] => {
                let (w, h) = alone_size[&g];
                let (x0, y0) = (cx - w / 2.0, cy - h / 2.0);
                out.boxes[g] = Some((x0, y0, x0 + w, y0 + h));
                shift_level(
                    ctx,
                    Some(g),
                    (x0 + spec.pad, y0 + spec.pad + title_h(spec, g)),
                    out,
                );
            }
            Item::Group(g) => {
                let w = w.max(spec.groups[g].title_w + 2.0 * spec.pad);
                let (x0, x1) = (cx - w / 2.0, cx + w / 2.0);
                min_x = min_x.min(x0);
                max_x = max_x.max(x1);
                out.boxes[g] = Some((x0, cy - h / 2.0, x1, cy + h / 2.0));
            }
        }
    }
    // Dagre leaves half a rank's gap above a compound group's members: its title is given
    // `title_lift` more, the innermost first, each group's top above its groups' tops.
    if spec.title_lift > 0.0 {
        let mut groups: Vec<(usize, usize)> = items
            .iter()
            .filter_map(|&it| match it {
                Item::Group(g) if !ctx.alone[g] => Some((ctx.depth(g), g)),
                _ => None,
            })
            .collect();
        groups.sort_by(|a, b| b.cmp(a));
        for &(_, g) in &groups {
            let Some(mut b) = out.boxes[g] else { continue };
            if spec.groups[g].title_w > 0.0 {
                b.1 -= spec.title_lift;
            }
            for (c, &p) in ctx.group_parent.iter().enumerate() {
                if let (true, Some(cb)) = (p == Some(g), out.boxes[c]) {
                    b.1 = b.1.min(cb.1 - spec.pad);
                }
            }
            out.boxes[g] = Some(b);
        }
    }
    for (k, &ei) in edge_ids.iter().enumerate() {
        let Some((_, a, b, a_cut, b_cut)) = ctx.edge_level[ei] else {
            continue;
        };
        let e = &l.edges[k];
        if e.points.len() < 2 {
            continue;
        }
        let mut pts = e.points.clone();
        let mut from_cut = false;
        let mut to_cut = false;
        let centre = |it: Item, out: &Laid| match it {
            Item::Node(n) => out.pos[n],
            Item::Group(_) => index
                .get(&it)
                .map_or((0.0, 0.0), |&i| (l.nodes[i].0, l.nodes[i].1)),
        };
        if a == b {
            // A self-loop: Dagre's route around the node, from its side and back.
            from_cut = true;
            to_cut = true;
        } else {
            // An end at a group is cut at its box; one at a node starts at its centre.
            match a_cut.and_then(|g| out.boxes[g]) {
                Some(bx) => {
                    pts = cut_from(&pts, bx);
                    from_cut = true;
                }
                None => pts[0] = centre(a, out),
            }
            match b_cut.and_then(|g| out.boxes[g]) {
                Some(bx) => {
                    pts = cut_to(&pts, bx);
                    to_cut = true;
                }
                None => {
                    let c = centre(b, out);
                    if let Some(p) = pts.last_mut() {
                        *p = c;
                    }
                }
            }
        }
        out.routes[ei] = Some(Route {
            points: pts,
            label: e.label,
            from_cut,
            to_cut,
        });
    }
    // A widened title box may reach left of the graph, a lifted one above it: move
    // everything right and down.
    let min_y = items
        .iter()
        .filter_map(|&it| match it {
            Item::Group(g) => out.boxes[g].map(|b| b.1),
            Item::Node(_) => None,
        })
        .fold(0.0_f64, f64::min);
    if min_x < 0.0 || min_y < 0.0 {
        shift_level(ctx, lv, (-min_x, -min_y), out);
        max_x -= min_x;
        min_x = 0.0;
    }
    Some((max_x - min_x, max_y - min_y))
}

/// Moves everything laid out in level `lv` (and the levels inside it) by `(dx, dy)`.
fn shift_level(ctx: &Ctx, lv: Option<usize>, (dx, dy): (f64, f64), out: &mut Laid) {
    for n in 0..ctx.spec.nodes.len() {
        if !ctx.hidden.contains(&n) && ctx.within(Item::Node(n), lv) {
            out.pos[n].0 += dx;
            out.pos[n].1 += dy;
        }
    }
    for g in 0..ctx.spec.groups.len() {
        if Some(g) != lv && ctx.within(Item::Group(g), lv) {
            if let Some(b) = out.boxes[g].as_mut() {
                *b = (b.0 + dx, b.1 + dy, b.2 + dx, b.3 + dy);
            }
        }
    }
    for (ei, el) in ctx.edge_level.iter().enumerate() {
        let Some((elv, ..)) = el else { continue };
        let inner = match elv {
            Some(g) => *elv == lv || ctx.within(Item::Group(*g), lv),
            None => lv.is_none(),
        };
        if !inner {
            continue;
        }
        if let Some(r) = out.routes[ei].as_mut() {
            for p in &mut r.points {
                p.0 += dx;
                p.1 += dy;
            }
            if let Some(p) = r.label.as_mut() {
                p.0 += dx;
                p.1 += dy;
            }
        }
    }
}

fn in_box((x, y): (f64, f64), (x0, y0, x1, y1): (f64, f64, f64, f64)) -> bool {
    x > x0 && x < x1 && y > y0 && y < y1
}

/// Where the segment from `p` (outside) to `q` (inside) crosses the box.
fn crossing(p: (f64, f64), q: (f64, f64), b: (f64, f64, f64, f64)) -> (f64, f64) {
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..40 {
        let m = (lo + hi) / 2.0;
        let pt = (p.0 + (q.0 - p.0) * m, p.1 + (q.1 - p.1) * m);
        if in_box(pt, b) {
            hi = m;
        } else {
            lo = m;
        }
    }
    (p.0 + (q.0 - p.0) * hi, p.1 + (q.1 - p.1) * hi)
}

/// The route up to where it first enters the box (all of it if it never does).
fn cut_to(pts: &[(f64, f64)], b: (f64, f64, f64, f64)) -> Vec<(f64, f64)> {
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        if !in_box(w[0], b) && in_box(w[1], b) {
            out.push(crossing(w[0], w[1], b));
            return out;
        }
        out.push(w[1]);
    }
    out
}

/// The route from where it last leaves the box.
fn cut_from(pts: &[(f64, f64)], b: (f64, f64, f64, f64)) -> Vec<(f64, f64)> {
    let rev: Vec<(f64, f64)> = pts.iter().rev().copied().collect();
    let mut out = cut_to(&rev, b);
    out.reverse();
    out
}

/// The room above group `g`'s members for its title (none for an untitled one).
fn title_h(spec: &Spec, g: usize) -> f64 {
    if spec.groups[g].title_w > 0.0 {
        spec.title_h
    } else {
        0.0
    }
}
