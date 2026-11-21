//! Collapsible sections (`<details>`), the view's side: which are open (the window's, not the
//! document's: a reload keeps them), a click on a summary that opens or closes its section, and
//! the sections that open by themselves to show what find, a link, the contents panel or Back
//! goes to, as the HTML spec's "ancestor details revealing" has it.

use super::*;

/// How far the pointer may move between press and release for a click on a summary (logical
/// pixels): further, it is a drag that selects.
const SLOP: f64 = 4.0;

impl View {
    /// Whether each section of the document is open.
    pub fn sections_open(&self) -> &[bool] {
        &self.open
    }

    /// The section whose summary is under window point (`x`, `y`): anywhere across its lines,
    /// as a browser's summary is (`display: list-item`).
    pub(super) fn summary_at(&self, x: f64, y: f64) -> Option<usize> {
        if self.message.is_some() || !self.in_area(x, y) || self.doc.sections.is_empty() {
            return None;
        }
        let y = self.doc_y(y) as f32;
        if y < 0.0 || y >= self.layout.total_height() {
            return None;
        }
        self.doc.summary_of(self.layout.block_at(y) as u32)
    }

    /// A press on the document at (`x`, `y`) that no link, image or button took: on a summary,
    /// the start of a click on it.
    pub(super) fn press_summary(&mut self, x: f64, y: f64) {
        self.pressed_section = self.summary_at(x, y).map(|s| (s, (x, y)));
    }

    /// The button came up at (`x`, `y`): a click on the summary it went down on (not a drag)
    /// opens or closes its section. True if it did.
    pub(super) fn click_summary(&mut self, x: f64, y: f64) -> bool {
        let Some((s, (px, py))) = self.pressed_section.take() else {
            return false;
        };
        let slop = SLOP * self.hidpi;
        if (x - px).hypot(y - py) > slop || self.summary_at(x, y) != Some(s) {
            return false;
        }
        self.toggle_section(s);
        true
    }

    /// Open section `s` if it is closed, else close it. What is above it stays where it is.
    pub fn toggle_section(&mut self, s: usize) {
        let Some(open) = self.open.get_mut(s) else {
            return;
        };
        *open = !*open;
        self.layout.set_open(&self.doc, s, &self.open);
        self.keep_shown();
        self.ensure_content();
    }

    /// Open the closed sections around block `b`, outermost first: true if one was closed.
    pub(super) fn reveal(&mut self, b: u32) -> bool {
        let closed: Vec<usize> = (self.doc.sections_around(b).into_iter())
            .filter(|&s| !self.open[s])
            .collect();
        for &s in &closed {
            self.open[s] = true;
            self.layout.set_open(&self.doc, s, &self.open);
        }
        !closed.is_empty()
    }

    /// A reading position in a closed section moves to the summary of the outermost one closed.
    pub(super) fn keep_shown(&mut self) {
        let b = self.anchor.block;
        if !self.layout.hidden(b as usize) {
            return;
        }
        if let Some(s) = self
            .doc
            .sections_around(b)
            .into_iter()
            .find(|&s| !self.open[s])
        {
            self.anchor = Anchor {
                block: self.doc.sections[s].summary.start,
                frac: 0.0,
            };
        }
    }

    /// The blocks of the closed sections' bodies, as ascending ranges.
    #[cfg(any(feature = "clipboard", feature = "images"))]
    fn closed_bodies(&self) -> Vec<std::ops::Range<u32>> {
        let mut out: Vec<std::ops::Range<u32>> = Vec::new();
        for (s, &open) in self.doc.sections.iter().zip(&self.open) {
            if open || s.body.is_empty() {
                continue;
            }
            // (A section in a summary, `<summary><details>`, has its body before the outer
            // section's.) One in a closed body is hidden already.
            let at = out.partition_point(|r| r.start <= s.body.start);
            if at == 0 || out[at - 1].end < s.body.end {
                out.insert(at, s.body.clone());
            }
        }
        out
    }

    /// What a copy leaves out: the closed sections' bodies.
    #[cfg(feature = "clipboard")]
    pub(super) fn hidden_ranges(&self) -> crate::copy::Hidden {
        self.closed_bodies().into()
    }

    /// The images in closed sections (by id), which the viewer doesn't step to.
    #[cfg(feature = "images")]
    pub(super) fn hidden_images(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for r in self.closed_bodies() {
            for block in &self.doc.blocks[r.start as usize..r.end as usize] {
                let cells = self.doc.row_cells(block).map(|(_, spans)| spans);
                let spans = std::iter::once(self.doc.block_spans(block)).chain(cells);
                ids.extend(spans.flatten().map(|s| s.image).filter(|&id| id != 0));
            }
        }
        // (Ascending: blocks are in order, and an image is in one block.)
        ids.sort_unstable();
        ids
    }

    /// Each section's state for a new version of the document (see [`carry_sections`]): its
    /// key with the state in the key's lowest bit, sorted.
    pub(super) fn section_states(&self) -> Vec<(u32, u32)> {
        let keys = section_keys(&self.doc).into_iter().zip(&self.open);
        let mut states: Vec<(u32, u32)> = keys.map(|((h, n), &o)| (h, n << 1 | o as u32)).collect();
        states.sort_unstable();
        states
    }
}

/// Each section's key: a hash of its summary's text, and its ordinal among the sections whose
/// summaries have that hash.
fn section_keys(doc: &Doc) -> Vec<(u32, u32)> {
    // FNV-1a over the summary's text.
    let hash = |s: &crate::doc::Section| {
        let blocks = &doc.blocks[s.summary.start as usize..s.summary.end as usize];
        let spans = blocks.iter().flat_map(|b| doc.block_spans(b));
        (spans.flat_map(|span| doc.span_text(span).bytes())).fold(0x811c_9dc5u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        })
    };
    let mut keys: Vec<(u32, u32)> = (doc.sections.iter().enumerate())
        .map(|(i, s)| (hash(s), i as u32))
        .collect();
    // Sorted by hash then place, the ordinal is the place in its run of equal hashes.
    let mut by_hash = keys.clone();
    by_hash.sort_unstable();
    let (mut prev, mut n) = (None, 0);
    for &(h, i) in &by_hash {
        n = if prev == Some(h) { n + 1 } else { 0 };
        prev = Some(h);
        keys[i as usize].1 = n;
    }
    keys
}

/// Whether each section of `doc` is open, for a new version of a document whose sections were
/// `old` (see [`View::section_states`]): a section takes the state of the one with the same
/// summary text and the same ordinal among those with that text, as headings are found again
/// (an edit above a section doesn't open or close another); a new one starts as it says.
pub(super) fn carry_sections(old: &[(u32, u32)], doc: &Doc) -> Vec<bool> {
    (section_keys(doc).iter().zip(&doc.sections))
        .map(|(&(h, n), s)| {
            let at = old.partition_point(|&k| k < (h, n << 1));
            let found = old.get(at).filter(|k| k.0 == h && k.1 >> 1 == n);
            found.map_or(s.open, |k| k.1 & 1 != 0)
        })
        .collect()
}
