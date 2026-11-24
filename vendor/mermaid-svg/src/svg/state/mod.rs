//! State diagram renderer. Layout via dugong (VOMD, see svg/compound.rs), custom shapes for
//! start/end/choice/fork/join pseudo-states.

use std::collections::{HashMap, HashSet};

use crate::parse::{FlowDirection, NotePosition, StateDiagram, StateKind};
use crate::sugiyama::{Dir, NodeId};

use super::compound;

use super::builder::SvgBuilder;
use super::interact::{close_click, open_click};
use super::theme::Theme;

mod composite;
use composite::*;
mod edges;
use edges::*;
mod shapes;
use shapes::*;
#[cfg(test)]
mod tests;

const CHAR_W: f64 = 7.5;
const LINE_H: f64 = 20.0;
const PAD_X: f64 = 18.0;
const PAD_Y: f64 = 12.0;
const MIN_W: f64 = 70.0;
const MIN_H: f64 = 40.0;
const PSEUDO_R: f64 = 10.0; // start/end radius
const CANVAS_PAD: f64 = 24.0;
const CHOICE_W: f64 = 60.0;
const CHOICE_H: f64 = 40.0;

pub(crate) fn render(d: &StateDiagram, theme: &Theme) -> String {
    if d.states.is_empty() {
        let mut svg = SvgBuilder::new(40.0, 40.0).theme(theme);
        define_marker(&mut svg, theme);
        return svg.finish();
    }

    // VOMD: laid out by dugong (see svg/compound.rs), composite states as compound nodes and
    // their parallel regions as groups inside them. A state with notes is laid out as one
    // node with room for them beside it (or above it, for `over`), so a note is level with its
    // state, on the side it names.
    let dir = d.direction;
    let horizontal = matches!(dir, FlowDirection::LeftRight | FlowDirection::RightLeft);
    let sizes: Vec<(f64, f64)> = d
        .states
        .iter()
        .map(|s| {
            let (w, h) = state_size(s, theme.font_size);
            match s.kind {
                StateKind::Fork | StateKind::Join if horizontal => (h, w),
                _ => (w, h),
            }
        })
        .collect();
    let composite_ids: HashSet<&str> = d.composites.iter().map(|c| c.id.as_str()).collect();
    let mut id_to_u32: HashMap<String, NodeId> = HashMap::new();
    for (i, s) in d.states.iter().enumerate() {
        if !composite_ids.contains(s.id.as_str()) {
            id_to_u32.entry(s.id.clone()).or_insert(i as NodeId);
        }
    }
    let note_sizes: Vec<(f64, f64)> = d
        .notes
        .iter()
        .map(|n| note_size(n, theme.font_size))
        .collect();
    let rooms = note_rooms(d, &id_to_u32, &sizes, &note_sizes);
    let region_ids: Vec<Vec<String>> = d
        .composites
        .iter()
        .map(|c| {
            if c.regions.len() < 2 {
                Vec::new()
            } else {
                (0..c.regions.len())
                    .map(|k| format!("{}\u{1}r{k}", c.id))
                    .collect()
            }
        })
        .collect();
    let font_size = theme.font_size;
    let label_size = |t: &str| {
        (
            crate::svg::metrics::text_width(t, 7.0, font_size) + 8.0,
            18.0,
        )
    };
    let mut spec = compound::Spec {
        dir: match dir {
            FlowDirection::TopDown => Dir::TB,
            FlowDirection::BottomTop => Dir::BT,
            FlowDirection::LeftRight => Dir::LR,
            FlowDirection::RightLeft => Dir::RL,
        },
        nodesep: 50.0,
        ranksep: 50.0,
        margin: 8.0,
        title_h: 30.0,
        title_lift: 8.0,
        pad: FRAME_PAD,
        ..compound::Spec::default()
    };
    for (i, (s, &(w, h))) in d.states.iter().zip(&sizes).enumerate() {
        let (w, h) = rooms[i].as_ref().map_or((w, h), |r| r.size);
        spec.nodes.push(compound::SNode { id: &s.id, w, h });
    }
    for (c, regions) in d.composites.iter().zip(&region_ids) {
        let title_w = label_size(composite_title(d, &c.id)).0;
        if regions.is_empty() {
            let (nodes, groups) = split(c.regions.iter().flatten(), &composite_ids);
            spec.groups.push(compound::SGroup {
                id: &c.id,
                title_w,
                dir: Some(Dir::TB),
                nodes,
                groups,
            });
        } else {
            spec.groups.push(compound::SGroup {
                id: &c.id,
                title_w,
                dir: Some(Dir::TB),
                nodes: Vec::new(),
                groups: regions.iter().map(String::as_str).collect(),
            });
            for (rid, region) in regions.iter().zip(&c.regions) {
                let (nodes, groups) = split(region.iter(), &composite_ids);
                spec.groups.push(compound::SGroup {
                    id: rid,
                    title_w: 0.0,
                    dir: Some(Dir::TB),
                    nodes,
                    groups,
                });
            }
        }
    }
    for t in &d.transitions {
        spec.edges.push(compound::SEdge {
            from: &t.from,
            to: &t.to,
            label: t
                .label
                .as_deref()
                .filter(|l| !l.is_empty())
                .map_or((0.0, 0.0), label_size),
            minlen: 1,
        });
    }
    let Some(laid) = compound::layout(&spec) else {
        return SvgBuilder::new(40.0, 40.0).theme(theme).finish();
    };
    let off = |(x, y): (f64, f64)| (x + CANVAS_PAD, y + CANVAS_PAD);
    // Each state's own centre in the room laid out for it and its notes, and the notes' places.
    let mut note_pos: Vec<Option<(f64, f64)>> = vec![None; d.notes.len()];
    // The box of each room, where edges must not go but to its state.
    let mut room_box: HashMap<NodeId, (f64, f64, f64, f64)> = HashMap::new();
    let pos: HashMap<NodeId, (f64, f64)> = id_to_u32
        .values()
        .map(|&u| {
            let (cx, cy) = off(laid.pos[u as usize]);
            let Some(r) = &rooms[u as usize] else {
                return (u, (cx, cy));
            };
            let (w, h) = r.size;
            room_box.insert(u, (cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0));
            let c = (cx + r.state.0, cy + r.state.1);
            for &(k, dx, dy) in &r.notes {
                note_pos[k] = Some((cx + dx, cy + dy));
            }
            (u, c)
        })
        .collect();
    let mut boxes: HashMap<String, (f64, f64, f64, f64)> = HashMap::new();
    let mut dividers: HashMap<String, Vec<(f64, f64, f64, f64)>> = HashMap::new();
    {
        let mut gi = 0;
        for (c, regions) in d.composites.iter().zip(&region_ids) {
            let shifted = |b: (f64, f64, f64, f64)| {
                let (x0, y0) = off((b.0, b.1));
                let (x1, y1) = off((b.2, b.3));
                (x0, y0, x1, y1)
            };
            let own = laid.boxes[gi].map(shifted);
            gi += 1;
            if let Some(b) = own {
                boxes.insert(c.id.clone(), b);
            }
            let rboxes: Vec<(f64, f64, f64, f64)> = regions
                .iter()
                .enumerate()
                .filter_map(|(k, _)| laid.boxes[gi + k].map(shifted))
                .collect();
            gi += regions.len();
            if let (Some(b), true) = (own, rboxes.len() > 1) {
                dividers.insert(c.id.clone(), region_dividers(b, &rboxes));
            }
        }
    }

    // A note on a composite state: beside its box, the canvas grown for it.
    let mut width = laid.width + 2.0 * CANVAS_PAD;
    let mut height = laid.height + 2.0 * CANVAS_PAD;
    for (k, note) in d.notes.iter().enumerate() {
        if note_pos[k].is_some() {
            continue;
        }
        let Some(&(x0, y0, x1, y1)) = boxes.get(&note.target) else {
            continue;
        };
        let (nw, nh) = note_sizes[k];
        let c = match note.position {
            NotePosition::LeftOf => ((x0 - NOTE_GAP - nw / 2.0).max(nw / 2.0), (y0 + y1) / 2.0),
            NotePosition::Over => ((x0 + x1) / 2.0, (y0 - NOTE_GAP - nh / 2.0).max(nh / 2.0)),
            _ => (x1 + NOTE_GAP + nw / 2.0, (y0 + y1) / 2.0),
        };
        width = width.max(c.0 + nw / 2.0 + CANVAS_PAD);
        height = height.max(c.1 + nh / 2.0 + CANVAS_PAD);
        note_pos[k] = Some(c);
    }
    NOTES.with(|n| {
        *n.borrow_mut() = d
            .notes
            .iter()
            .enumerate()
            .filter_map(|(k, note)| {
                let (x, y) = note_pos[k]?;
                let (nw, nh) = note_sizes[k];
                let &u = id_to_u32.get(&note.target)?;
                let (sx, sy) = pos[&u];
                let (sw, sh) = sizes[u as usize];
                Some((
                    [sx - sw / 2.0, sy - sh / 2.0, sx + sw / 2.0, sy + sh / 2.0],
                    [x - nw / 2.0, y - nh / 2.0, x + nw / 2.0, y + nh / 2.0],
                ))
            })
            .collect();
    });
    // The transitions' routes first: a self-loop of VOMD's may reach past the layout's
    // extent, which the canvas then takes in.
    let mut drawn_edges: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut prepared = Vec::new();
    for (ti, tr) in d.transitions.iter().enumerate() {
        let Some(route) = &laid.routes[ti] else {
            continue;
        };
        let mut pts: Vec<(f64, f64)> = route.points.iter().map(|&p| off(p)).collect();
        if pts.len() < 2 {
            continue;
        }
        let from = id_to_u32.get(&tr.from).copied();
        let to = id_to_u32.get(&tr.to).copied();
        let mut label_at = route.label.map(off);
        let mut cut = (route.from_cut, route.to_cut);
        // A self-loop on a state with notes: a loop of its own, on a side without notes.
        if let (Some(u), true) = (from, from == to) {
            if let (Some(r), Some(&c)) = (&rooms[u as usize], pos.get(&u)) {
                let (w, h) = sizes[u as usize];
                pts = self_loop(c, (w, h), r.free_side());
                label_at = Some(pts[pts.len() / 2]);
                cut = (true, true);
            }
        }
        if !cut.0 || !cut.1 {
            // (An end at a state with notes is the centre of its room: the state's own, reached
            // from above or below its notes, never through them.)
            if let (false, Some(u)) = (cut.0, from) {
                pts.reverse();
                into_room(&mut pts, &pos, &room_box, &rooms, u);
                pts.reverse();
            }
            if let (false, Some(u)) = (cut.1, to) {
                into_room(&mut pts, &pos, &room_box, &rooms, u);
            }
        }
        drawn_edges.push(pts.clone());
        for &(x, y) in pts.iter().chain(label_at.iter()) {
            let lw = tr.label.as_deref().map_or(0.0, |l| {
                crate::svg::metrics::text_width(l, 7.0, theme.font_size) / 2.0 + 4.0
            });
            width = width.max(x + lw + CANVAS_PAD / 2.0);
            height = height.max(y + 12.0 + CANVAS_PAD / 2.0);
        }
        prepared.push((tr, pts, label_at, cut));
    }
    let mut svg = SvgBuilder::new(width, height).theme(theme);
    define_marker(&mut svg, theme);

    // Cluster frames first (under nodes/edges) so labels stay legible.
    draw_composites(&mut svg, d, &boxes, &dividers, theme);

    for (tr, pts, label_at, cut) in prepared {
        let route_cut = cut;
        let point = |p: (f64, f64)| StateEndClip {
            center: p,
            size: (0.0, 0.0),
            kind: None,
        };
        let start = if route_cut.0 {
            point(pts[0])
        } else {
            match endpoint_clip(&tr.from, &id_to_u32, &d.states, &sizes, &pos, &boxes) {
                Some(c) => c,
                None => continue,
            }
        };
        let end = if route_cut.1 {
            point(pts[pts.len() - 1])
        } else {
            match endpoint_clip(&tr.to, &id_to_u32, &d.states, &sizes, &pos, &boxes) {
                Some(c) => c,
                None => continue,
            }
        };
        draw_transition(&mut svg, &pts, tr, &start, &end, label_at, theme);
    }
    EDGES.with(|e| *e.borrow_mut() = drawn_edges);

    for (i, state) in d.states.iter().enumerate() {
        let Some(&center) = pos.get(&(i as NodeId)) else {
            continue;
        };
        if let Some(action) = &state.click {
            open_click(&mut svg, action);
        }
        draw_state(&mut svg, center, sizes[i], state, &d.class_defs, theme);
        if let Some(action) = &state.click {
            close_click(&mut svg, action);
        }
    }

    // Notes attached to states, where the layout put them.
    for (note, (c, &sz)) in d.notes.iter().zip(note_pos.iter().zip(&note_sizes)) {
        if let Some(c) = *c {
            draw_state_note(&mut svg, note, c, sz, theme);
        }
    }

    svg.finish()
}

fn define_marker(svg: &mut SvgBuilder, theme: &Theme) {
    svg.def_arrow_marker("state-arrow", &theme.flow_edge_stroke, 10, 10);
}

/// A composite's members: its states, and its composites.
fn split<'a>(
    ids: impl Iterator<Item = &'a String>,
    composites: &HashSet<&str>,
) -> (Vec<&'a str>, Vec<&'a str>) {
    let (mut nodes, mut groups) = (Vec::new(), Vec::new());
    for id in ids {
        if composites.contains(id.as_str()) {
            groups.push(id.as_str());
        } else {
            nodes.push(id.as_str());
        }
    }
    (nodes, groups)
}

fn composite_title<'a>(d: &'a StateDiagram, id: &'a str) -> &'a str {
    d.states
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.label.as_str())
        .filter(|l| !l.is_empty())
        .unwrap_or(id)
}

/// Dashed lines between a composite's parallel regions: between each pair of regions next
/// to each other, across the composite.
fn region_dividers(
    b: (f64, f64, f64, f64),
    regions: &[(f64, f64, f64, f64)],
) -> Vec<(f64, f64, f64, f64)> {
    let mut out = Vec::new();
    // VOMD: one sort for both (the key a function pointer: one copy of std's sort, not two).
    let sorted = |key: fn(&(f64, f64, f64, f64)) -> f64| {
        let mut v = regions.to_vec();
        v.sort_by(|p, q| key(p).total_cmp(&key(q)));
        v
    };
    let by_y = sorted(|r| r.1);
    let by_x = sorted(|r| r.0);
    for w in by_y.windows(2) {
        if w[1].1 >= w[0].3 {
            let y = (w[0].3 + w[1].1) / 2.0;
            out.push((b.0, y, b.2, y));
        }
    }
    for w in by_x.windows(2) {
        if w[1].0 >= w[0].2 {
            let x = (w[0].2 + w[1].0) / 2.0;
            out.push((x, b.1 + 24.0, x, b.3));
        }
    }
    out
}

/// Space between a state and its note.
const NOTE_GAP: f64 = 14.0;

thread_local! {
    /// The last render's notes and their states, as boxes (for VOMD's tests).
    static NOTES: std::cell::RefCell<Vec<([f64; 4], [f64; 4])>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// VOMD: the boxes of the last state diagram's notes on states and of their states, on this
/// thread: `(state, note)` in the order of the source.
pub(crate) fn last_notes() -> Vec<([f64; 4], [f64; 4])> {
    NOTES.with(|n| n.borrow().clone())
}

/// A state with notes, laid out as one node: its size, its own centre and each note's centre
/// (by index into `d.notes`) from the room's centre.
struct Room {
    size: (f64, f64),
    state: (f64, f64),
    notes: Vec<(usize, f64, f64)>,
}

/// The room of each state with notes (by index into `d.states`): notes `right of` stacked to
/// its right, `left of` to its left, both level with it, `over` stacked above it.
fn note_rooms(
    d: &StateDiagram,
    id_to_u32: &HashMap<String, NodeId>,
    sizes: &[(f64, f64)],
    note_sizes: &[(f64, f64)],
) -> Vec<Option<Room>> {
    let mut by_state: HashMap<usize, [Vec<usize>; 3]> = HashMap::new();
    for (k, note) in d.notes.iter().enumerate() {
        let Some(&u) = id_to_u32.get(&note.target) else {
            continue;
        };
        let side = match note.position {
            NotePosition::LeftOf => 0,
            NotePosition::Over => 2,
            _ => 1,
        };
        by_state.entry(u as usize).or_default()[side].push(k);
    }
    let mut rooms: Vec<Option<Room>> = (0..d.states.len()).map(|_| None).collect();
    for (u, [left, right, over]) in by_state {
        let (w, h) = sizes[u];
        let stack_h = |ks: &[usize]| {
            ks.iter().map(|&k| note_sizes[k].1).sum::<f64>()
                + NOTE_GAP / 2.0 * ks.len().saturating_sub(1) as f64
        };
        let widest = |ks: &[usize]| ks.iter().map(|&k| note_sizes[k].0).fold(0.0, f64::max);
        let side_w = |ks: &[usize]| {
            if ks.is_empty() {
                0.0
            } else {
                NOTE_GAP + widest(ks)
            }
        };
        let (lw, rw) = (side_w(&left), side_w(&right));
        let over_h = if over.is_empty() {
            0.0
        } else {
            stack_h(&over) + NOTE_GAP
        };
        let body_h = h.max(stack_h(&left)).max(stack_h(&right));
        let width = (lw + w + rw).max(widest(&over));
        let height = over_h + body_h;
        // From the room's top left corner, then from its centre.
        let sx = (width - (lw + w + rw)) / 2.0 + lw + w / 2.0;
        let sy = over_h + body_h / 2.0;
        let mut notes = Vec::new();
        let mut column = |ks: &[usize], x_of: &dyn Fn(f64) -> f64| {
            let mut y = sy - stack_h(ks) / 2.0;
            for &k in ks {
                let (nw, nh) = note_sizes[k];
                notes.push((k, x_of(nw) - width / 2.0, y + nh / 2.0 - height / 2.0));
                y += nh + NOTE_GAP / 2.0;
            }
        };
        column(&left, &|nw| sx - w / 2.0 - NOTE_GAP - nw / 2.0);
        column(&right, &|nw| sx + w / 2.0 + NOTE_GAP + nw / 2.0);
        let mut y = 0.0;
        for &k in &over {
            let (_, nh) = note_sizes[k];
            notes.push((k, sx - width / 2.0, y + nh / 2.0 - height / 2.0));
            y += nh + NOTE_GAP / 2.0;
        }
        rooms[u] = Some(Room {
            size: (width, height),
            state: (sx - width / 2.0, sy - height / 2.0),
            notes,
        });
    }
    rooms
}

thread_local! {
    /// The last render's transitions as drawn (before their ends are clipped to the shapes),
    /// for VOMD's tests.
    static EDGES: std::cell::RefCell<Vec<Vec<(f64, f64)>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// VOMD: the last state diagram's transitions as polylines, on this thread.
pub(crate) fn last_edges() -> Vec<Vec<(f64, f64)>> {
    EDGES.with(|e| e.borrow().clone())
}

impl Room {
    /// Where a self-loop goes: right, left or above (`1`, `-1`, `0`), a side without notes.
    fn free_side(&self) -> i8 {
        let side = |sign: f64| {
            self.notes
                .iter()
                .any(|&(_, dx, _)| (dx - self.state.0) * sign > 0.0)
        };
        if !side(1.0) {
            1
        } else if !side(-1.0) {
            -1
        } else {
            0
        }
    }

    fn has_over(&self) -> bool {
        self.notes
            .iter()
            .any(|&(_, dx, dy)| (dx - self.state.0).abs() < 1e-6 && dy < self.state.1)
    }
}

/// A self-loop of a state at `c`, `(w, h)` big, on side `side` (see [`Room::free_side`]):
/// from the state's border out and back.
fn self_loop((cx, cy): (f64, f64), (w, h): (f64, f64), side: i8) -> Vec<(f64, f64)> {
    const OUT: f64 = 28.0;
    if side == 0 {
        let top = cy - h / 2.0;
        let dx = w / 4.0;
        return vec![
            (cx + dx, top),
            (cx + dx + 8.0, top - OUT),
            (cx - dx - 8.0, top - OUT),
            (cx - dx, top),
        ];
    }
    let s = f64::from(side);
    let x = cx + s * w / 2.0;
    let dy = h / 4.0;
    vec![
        (x, cy - dy),
        (x + s * OUT, cy - dy - 8.0),
        (x + s * OUT, cy + dy + 8.0),
        (x, cy + dy),
    ]
}

/// The end of `pts` at state `u`: if `u` has notes, the points inside its room go, and the
/// route reaches the state's centre straight down from above the room (or up from below, when
/// it comes from below, or when the state has notes above it).
fn into_room(
    pts: &mut Vec<(f64, f64)>,
    pos: &HashMap<NodeId, (f64, f64)>,
    room_box: &HashMap<NodeId, (f64, f64, f64, f64)>,
    rooms: &[Option<Room>],
    u: NodeId,
) {
    let (Some(&(x0, y0, x1, y1)), Some(&c), Some(Some(r))) =
        (room_box.get(&u), pos.get(&u), rooms.get(u as usize))
    else {
        return;
    };
    if pts.len() < 2 {
        return;
    }
    let inside = |p: (f64, f64)| p.0 > x0 && p.0 < x1 && p.1 > y0 && p.1 < y1;
    let last = pts.len() - 1;
    let mut keep: Vec<(f64, f64)> = pts[..last]
        .iter()
        .enumerate()
        .filter(|&(i, p)| i == 0 || !inside(*p))
        .map(|(_, p)| *p)
        .collect();
    let q = *keep.last().unwrap_or(&c);
    let from_above = q.1 < c.1 && !r.has_over();
    let y = if from_above { y0 - 10.0 } else { y1 + 10.0 };
    if !inside(q) {
        // Round the room, never through it: to the level above (or below) it first.
        if (from_above && q.1 > y) || (!from_above && q.1 < y) {
            keep.push((q.0, y));
        }
        keep.push((c.0, y));
    }
    keep.push(c);
    *pts = keep;
}
