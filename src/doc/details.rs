//! Collapsible sections (`<details>`), the builder's side: a section is a range of blocks, its
//! summary's and then its body's (see [`Section`]). The tags count in HTML blocks and inline
//! HTML alike, as an HTML parser has them: an inline `</details>` ends the section in the
//! middle of its paragraph.

use super::*;

/// A `<details>` being built.
pub(super) struct OpenSection {
    /// Its index in `Doc::sections`.
    index: usize,
    /// `Builder::depth` when it opened: the end of that container ends it.
    pub(super) depth: usize,
    /// Its blocks are a footnote definition's (indices into `Footnotes::blocks`).
    in_fn: bool,
    /// Its first block, and how long the lists were then whose entries may point into it (see
    /// `rotate`).
    start: usize,
    anchors: usize,
    headings: usize,
    fn_headings: usize,
    local: usize,
    /// Where its summary's blocks start, once `<summary>` came, and whether `</summary>` did.
    summary: Option<usize>,
    summary_done: bool,
}

/// How deep sections nest: a deeper `<details>` is shown open, as text. (Each level may move the
/// blocks of its body once, for a summary that comes late or none, so this bounds that work.)
const MAX_SECTIONS: usize = 32;

impl Builder {
    /// A `<details>` or `<summary>` tag (`closing`: its end tag). A summary counts only for the
    /// section open in the same container, and a second summary is ordinary text in the body;
    /// `</details>` ends the innermost section, from a list item or quote in it too (as GitHub's
    /// HTML parser has it: an end tag indented under the last item). Sections nest at most
    /// [`MAX_SECTIONS`] deep, and not in a summary: the tags of those are nothing.
    pub(super) fn section_tag(&mut self, details: bool, closing: bool, tag: &str) {
        // In a Markdown table's cell, which holds a line of text, the tags are nothing.
        if self.row.is_some() {
            return;
        }
        // (Counted apart for the text and for footnote definitions, which are moved out of it.)
        let ignored = &mut self.ignored_sections[usize::from(self.footnotes.active)];
        if *ignored > 0 {
            if details {
                *ignored += 1;
                if closing {
                    *ignored -= 2;
                }
            }
            return;
        }
        let (depth, active) = (self.depth, self.footnotes.active);
        let top = self.open_sections.last().filter(|s| s.in_fn == active);
        let here = top.filter(|s| s.depth == depth);
        match (details, closing) {
            // Too deep, or in a summary (where it could take the summary's only line, and with
            // it the click that opens the section): shown open, as text.
            (true, false)
                if (self.open_sections.iter())
                    .filter(|s| s.in_fn == active)
                    .count()
                    >= MAX_SECTIONS
                    || top.is_some_and(|s| s.summary.is_some() && !s.summary_done) =>
            {
                self.ignored_sections[usize::from(active)] = 1
            }
            (true, false) => self.open_section(has_attr(tag, "open")),
            (true, true) if top.is_some() => self.close_section(),
            (false, false) if here.is_some_and(|s| s.summary.is_none()) => {
                self.close();
                let at = self.block_count();
                if let Some(s) = self.open_sections.last_mut() {
                    s.summary = Some(at);
                }
            }
            (false, true) if here.is_some_and(|s| s.summary.is_some() && !s.summary_done) => {
                self.end_summary();
            }
            _ => {}
        }
    }

    fn open_section(&mut self, open: bool) {
        self.close();
        let start = self.block_count();
        let index = self.doc.sections.len();
        self.doc.sections.push(Section {
            summary: start as u32..start as u32,
            body: start as u32..start as u32,
            open,
            parent: u32::MAX,
        });
        self.section_fn.push(self.footnotes.active);
        self.open_sections.push(OpenSection {
            index,
            depth: self.depth,
            in_fn: self.footnotes.active,
            start,
            anchors: self.doc.anchors.len(),
            headings: self.doc.headings.len(),
            fn_headings: self.footnotes.headings.len(),
            local: self.footnotes.local.len(),
            summary: None,
            summary_done: false,
        });
        // (As in an HTML parser, the section is open whatever the value: `open="false"` too.)
    }

    /// The summary of the innermost section ends: it is at least one block (an empty one is an
    /// empty paragraph), and it comes first, before any body blocks that came before it.
    fn end_summary(&mut self) {
        self.close();
        let Some(s) = self.open_sections.last() else {
            return;
        };
        let (start, from) = (s.summary.unwrap_or(s.start), s.start);
        if self.block_count() == start {
            let at = self.doc.spans.len() as u32;
            self.push_block(BlockKind::Paragraph, at..at);
        }
        let len = self.block_count() - start;
        if start > from {
            self.rotate(start);
        }
        let Some(s) = self.open_sections.last_mut() else {
            return;
        };
        s.summary_done = true;
        let sec = &mut self.doc.sections[s.index];
        sec.summary = from as u32..(from + len) as u32;
    }

    /// End the innermost section. One with no summary gets GitHub's: "Details".
    pub(super) fn close_section(&mut self) {
        self.close();
        let at = self.block_count();
        let Some(s) = self.open_sections.last_mut() else {
            return;
        };
        if s.summary.is_none() {
            s.summary = Some(at);
            self.begin(BlockKind::Paragraph);
            self.push_text("Details", 0, 0, 0);
            self.close();
        }
        if self.open_sections.last().is_some_and(|s| !s.summary_done) {
            self.end_summary();
        }
        let end = self.block_count() as u32;
        let Some(s) = self.open_sections.pop() else {
            return;
        };
        // (Sections too deep to keep that aren't closed yet end with it.)
        self.ignored_sections[usize::from(s.in_fn)] = 0;
        let sec = &mut self.doc.sections[s.index];
        sec.body = sec.summary.end..end;
    }

    /// Move the innermost section's blocks from `mid` on (its summary) before those from its
    /// start to `mid` (body blocks that came first), and what points at them with them: anchors,
    /// headings, and the sections in it.
    fn rotate(&mut self, mid: usize) {
        let Some(s) = self.open_sections.last() else {
            return;
        };
        let (from, in_fn) = (s.start, s.in_fn);
        let blocks = if in_fn {
            &mut self.footnotes.blocks
        } else {
            &mut self.doc.blocks
        };
        let end = blocks.len();
        blocks[from..].rotate_left(mid - from);
        let map = |b: u32| -> u32 {
            let b = b as usize;
            (match b {
                _ if b < from || b >= end => b,
                _ if b < mid => b + (end - mid),
                _ => b - (mid - from),
            }) as u32
        };
        let mut local = self.footnotes.local[s.local..].to_vec();
        local.sort_unstable();
        for a in s.anchors..self.doc.anchors.len() {
            if local.binary_search(&a).is_ok() == in_fn {
                self.doc.anchors[a].1 = map(self.doc.anchors[a].1);
            }
        }
        if in_fn {
            // (In block order, as the main text's: they turn as the blocks did.)
            let moved = &mut self.footnotes.headings[s.fn_headings..];
            let before = moved.iter().filter(|h| h.0 < mid).count();
            for h in moved.iter_mut() {
                h.0 = map(h.0 as u32) as usize;
            }
            moved.rotate_left(before);
        } else {
            // (They stay in document order, which the contents panel lists: they turn as the
            // blocks did.)
            let moved = &mut self.doc.headings[s.headings..];
            let before = moved.iter().filter(|h| (h.block as usize) < mid).count();
            for h in moved.iter_mut() {
                h.block = map(h.block);
            }
            moved.rotate_left(before);
        }
        let inner = (s.index + 1..self.doc.sections.len()).filter(|&t| self.section_fn[t] == in_fn);
        for t in inner {
            let sec = &mut self.doc.sections[t];
            let (summary, body) = (sec.summary.len() as u32, sec.body.len() as u32);
            let start = map(sec.summary.start);
            sec.summary = start..start + summary;
            sec.body = sec.summary.end..sec.summary.end + body;
        }
    }
}
