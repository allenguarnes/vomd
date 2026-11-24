//! Composite-cluster layout for state diagrams: frame boxes around the
//! members of a `state X { … }` block, transition-endpoint clipping to those
//! boxes, and the attached state notes.

use std::collections::HashMap;

use crate::parse::{State, StateDiagram, StateKind, StateNote};
use crate::sugiyama::NodeId;

use crate::svg::builder::SvgBuilder;
use crate::svg::theme::Theme;

pub(super) const FRAME_PAD: f64 = 14.0;
/// Height of the filled title band drawn at the top of a composite frame.
const TITLE_BAND: f64 = 24.0;

/// Clip target for one end of a transition: the shape boundary a connector
/// stops at. `kind` is `None` for a composite cluster box (clipped as a rect).
pub(super) struct StateEndClip {
    pub(super) center: (f64, f64),
    pub(super) size: (f64, f64),
    pub(super) kind: Option<StateKind>,
}

/// Resolve a transition endpoint id to its clip target — a laid-out state's
/// boundary, or the bounding box of the composite it names.
pub(super) fn endpoint_clip(
    id: &str,
    id_to_u32: &HashMap<String, NodeId>,
    states: &[State],
    sizes: &[(f64, f64)],
    pos: &HashMap<NodeId, (f64, f64)>,
    boxes: &HashMap<String, (f64, f64, f64, f64)>,
) -> Option<StateEndClip> {
    if let Some(&u) = id_to_u32.get(id) {
        if let Some(&center) = pos.get(&u) {
            return Some(StateEndClip {
                center,
                size: sizes[u as usize],
                kind: Some(states[u as usize].kind),
            });
        }
    }
    let &(x0, y0, x1, y1) = boxes.get(id)?;
    Some(StateEndClip {
        center: ((x0 + x1) / 2.0, (y0 + y1) / 2.0),
        size: (x1 - x0, y1 - y0),
        kind: None,
    })
}

pub(super) fn draw_composites(
    svg: &mut SvgBuilder,
    d: &StateDiagram,
    boxes: &HashMap<String, (f64, f64, f64, f64)>,
    dividers: &HashMap<String, Vec<(f64, f64, f64, f64)>>,
    theme: &Theme,
) {
    let fg = &theme.fg;
    let border = &theme.flow_node_stroke;
    let band = &theme.flow_node_fill;
    for comp in &d.composites {
        let Some(&(x0, y0, x1, y1)) = boxes.get(&comp.id) else {
            continue;
        };
        // Solid purple-bordered rounded frame with a filled lavender title band
        // and a divider under the title, matching upstream's composite styling.
        svg.rect(
            x0,
            y0,
            x1 - x0,
            y1 - y0,
            &format!("fill=\"none\" stroke=\"{border}\" stroke-width=\"1\" rx=\"5\""),
        );
        svg.rect(
            x0,
            y0,
            x1 - x0,
            TITLE_BAND,
            &format!("fill=\"{band}\" stroke=\"{border}\" stroke-width=\"1\" rx=\"5\""),
        );
        let label = d
            .states
            .iter()
            .find(|s| s.id == comp.id)
            .map(|s| s.label.as_str())
            .filter(|l| !l.is_empty())
            .unwrap_or(comp.id.as_str());
        svg.text(
            (x0 + x1) / 2.0,
            y0 + 16.0,
            &format!("text-anchor=\"middle\" fill=\"{fg}\" font-size=\"12\" font-weight=\"bold\""),
            label,
        );
        svg.line(
            x0,
            y0 + TITLE_BAND,
            x1,
            y0 + TITLE_BAND,
            &format!("stroke=\"{border}\" stroke-width=\"1\""),
        );
        // Dashed dividers between stacked parallel regions.
        if let Some(ys) = dividers.get(&comp.id) {
            for &(ax, ay, bx, by) in ys {
                svg.line(
                    ax,
                    ay,
                    bx,
                    by,
                    "stroke=\"#999\" stroke-width=\"1\" stroke-dasharray=\"3 3\"",
                );
            }
        }
    }
}

/// A note's box size.
pub(super) fn note_size(note: &StateNote, font_size: f64) -> (f64, f64) {
    let nw = (crate::svg::metrics::text_width(&note.text, 7.0, font_size) + 20.0).max(80.0);
    (nw, 32.0)
}

/// VOMD: a note where the layout put it.
pub(super) fn draw_state_note(
    svg: &mut SvgBuilder,
    note: &StateNote,
    (cx, cy): (f64, f64),
    (nw, nh): (f64, f64),
    theme: &Theme,
) {
    let fg = &theme.fg;
    let (nx, ny) = (cx - nw / 2.0, cy - nh / 2.0);
    svg.rect(
        nx,
        ny,
        nw,
        nh,
        &format!(
            "fill=\"{}\" stroke=\"{}\" stroke-width=\"1\"",
            theme.note_fill, theme.note_stroke
        ),
    );
    svg.text(
        nx + nw / 2.0,
        ny + nh / 2.0 + 4.0,
        &format!("text-anchor=\"middle\" fill=\"{fg}\" font-size=\"12\""),
        &note.text,
    );
}
