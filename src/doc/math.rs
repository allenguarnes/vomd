//! Formulas in the document: what pulldown-cmark's math events leave to the builder (GitHub's
//! `` $`…`$ `` and the digit rule for prices, both decided by where the events are in the
//! source), and how the builder stores formulas.

use std::collections::VecDeque;
use std::ops::Range;

use pulldown_cmark::{CowStr, Event};

use super::{BlockKind, Builder, Hl, style};
use crate::math::MAX_SOURCE;

impl Builder {
    /// An inline formula (`display`: a `$$…$$` that can't be a block of its own) as a span of
    /// its own. One too long to lay out is its source in code style, delimiters and all.
    pub(super) fn formula(&mut self, src: &str, display: bool) {
        if src.len() > MAX_SOURCE {
            let d = if display { "$$" } else { "$" };
            self.text(&format!("{d}{src}{d}"), style::CODE | style::MATH_ERROR, 0);
        } else {
            let display = if display { style::DISPLAY } else { 0 };
            self.text(src, style::MATH | display, 0);
        }
    }

    /// `$$…$$`: a block of its own, as GitHub shows it, on a line of its own. The paragraph it
    /// is in ends before it (and is dropped if nothing came yet: the formula takes its list
    /// marker), and what follows it starts another. In a heading or a table cell, which hold
    /// one line, it stays inline. An empty one is nothing.
    pub(super) fn display_math(&mut self, src: &str) {
        // (The line breaks after `$$` and before the closing one aren't the formula's.)
        let src = src.trim();
        if src.is_empty() {
            return;
        }
        let in_paragraph = self
            .open
            .as_ref()
            .is_none_or(|open| open.kind == BlockKind::Paragraph);
        if !in_paragraph {
            self.formula(src, true);
            return;
        }
        self.end_paragraph();
        if src.len() > MAX_SOURCE {
            // Error text, cut into chunks like any long paragraph.
            self.begin(BlockKind::Paragraph);
            self.push_text(
                &format!("$${src}$$"),
                self.style() | style::CODE | style::MATH_ERROR,
                self.link(),
                0,
            );
            self.end_paragraph();
        } else {
            let at = self.doc.spans.len() as u32;
            let s = self.style() | style::MATH | style::DISPLAY;
            self.push_text(src, s, self.link(), 0);
            self.push_block(BlockKind::Math, at..at + 1);
        }
        self.after_display = true;
    }

    /// Close the open paragraph without the whitespace or break it ends in, keeping the open
    /// link and inline HTML styles for what follows (a formula in the middle of a link is part
    /// of it). A paragraph with nothing in it is dropped, its marker kept for the next block.
    fn end_paragraph(&mut self) {
        let Some(open) = &self.open else { return };
        let first = open.span_start as usize;
        if self.doc.spans.len() > first
            && let Some(last) = self.doc.spans.last_mut()
            && last.image == 0
            && last.style & style::CODE == 0
            && last.end as usize == self.doc.text.len()
        {
            let kept = self.doc.text[last.start as usize..]
                .trim_end_matches([' ', '\t', '\n'])
                .len();
            last.end = last.start + kept as u32;
            self.doc.text.truncate(last.end as usize);
            if last.start == last.end {
                self.doc.spans.pop();
            }
        }
        if self.doc.spans.len() == first {
            self.open = None;
            return;
        }
        let links = std::mem::take(&mut self.links);
        let html = std::mem::take(&mut self.html_inline);
        self.close();
        self.links = links;
        self.html_inline = html;
    }

    /// The end of a ```` ```math ```` block: a display formula of its text (without the line
    /// break that ends it). One too long to lay out is a code block of its source.
    pub(super) fn end_math_fence(&mut self) {
        let Some(mut src) = self.math_fence.take() else {
            return;
        };
        if src.ends_with('\n') {
            src.pop();
        }
        if src.trim().is_empty() {
            return;
        }
        if src.len() > MAX_SOURCE {
            self.open(BlockKind::Code(Hl::default()));
            self.text(&src, style::CODE | style::MATH_ERROR, 0);
            self.close();
        } else {
            let at = self.doc.spans.len() as u32;
            self.push_text(&src, style::MATH | style::DISPLAY, 0, 0);
            self.push_block(BlockKind::Math, at..at + 1);
        }
    }
}

/// The parser's events over `src`, each with whether it is text that isn't a slice of the
/// source (see `marked` in the parent), with two changes:
///
/// - `` $`…`$ `` (GitHub's math in code-span delimiters) comes as text that ends in `$`, a code
///   span, and text that starts with `$`, the dollars right against the backtick runs. That is
///   made one inline formula of the code's text; the dollars go.
/// - An inline formula whose closing `$` is right before an ASCII digit is text, as pandoc
///   reads it: `$5-$10` is a price range, not math.
///
/// Where an event is in the source is where its text is: text and formulas borrow from `src`
/// (pulldown-cmark's offset iterator would say so for every event, but it can panic on input
/// its plain iterator takes). Text that doesn't, a decoded entity or a formula rebuilt from the
/// lines of a quote, is left as it is.
///
/// This is one adapter, the `decoded` one's too, and an event that is neither text ending in
/// `$` nor a formula goes through it with two checks: documents without math parse as fast as
/// they did.
pub(super) fn marked<'a>(
    src: &'a str,
    events: impl Iterator<Item = Event<'a>>,
) -> impl Iterator<Item = (Event<'a>, bool)> {
    let start = src.as_ptr() as usize;
    Marked {
        src,
        events,
        ahead: VecDeque::new(),
        source: start..start + src.len(),
    }
}

struct Marked<'a, I> {
    src: &'a str,
    events: I,
    /// Events read ahead, at most three: a `` $`…`$ `` is three events. Empty but around one.
    ahead: VecDeque<Event<'a>>,
    /// The source's addresses.
    source: Range<usize>,
}

impl<'a, I: Iterator<Item = Event<'a>>> Iterator for Marked<'a, I> {
    type Item = (Event<'a>, bool);

    fn next(&mut self) -> Option<(Event<'a>, bool)> {
        let mut event = match self.ahead.pop_front() {
            Some(event) => event,
            None => self.events.next()?,
        };
        // Only text that ends in `$` can be the start of a `` $`…`$ ``.
        if let Event::Text(CowStr::Borrowed(s)) = &event
            && s.ends_with('$')
        {
            self.ahead.push_front(event);
            while self.ahead.len() < 3 {
                match self.events.next() {
                    Some(event) => self.ahead.push_back(event),
                    None => break,
                }
            }
            self.code_math();
            event = self.ahead.pop_front()?;
        }
        if let Event::InlineMath(_) = &event {
            event = self.inline_math(event);
        }
        let decoded = super::decoded(&self.source, &event);
        Some((event, decoded))
    }
}

/// Where `s` is in `src`, if it is a slice of it.
fn place(src: &str, s: &CowStr) -> Option<Range<usize>> {
    let CowStr::Borrowed(s) = s else { return None };
    let start = (s.as_ptr() as usize).checked_sub(src.as_ptr() as usize)?;
    let end = start.checked_add(s.len())?;
    (end <= src.len()).then_some(start..end)
}

impl<'a, I> Marked<'a, I> {
    /// If the next three events are `` $`…`$ ``, make them (what is left of the text before)
    /// the formula and (what is left of the text after).
    fn code_math(&mut self) {
        let src = self.src.as_bytes();
        let (Some(Event::Text(before)), Some(Event::Code(_)), Some(Event::Text(after))) =
            (self.ahead.front(), self.ahead.get(1), self.ahead.get(2))
        else {
            return;
        };
        let (Some(b), Some(a)) = (place(self.src, before), place(self.src, after)) else {
            return;
        };
        // The code span is all that is between the two texts: it starts and ends with a
        // backtick. The dollar before it isn't escaped (`\$` is a dollar of text).
        let dollar_before = b.end > b.start
            && src.get(b.end - 1) == Some(&b'$')
            && src.get(b.end) == Some(&b'`')
            && src[..b.end - 1]
                .iter()
                .rev()
                .take_while(|&&c| c == b'\\')
                .count()
                % 2
                == 0;
        let dollar_after = a.start > b.end
            && src.get(a.start - 1) == Some(&b'`')
            && src.get(a.start) == Some(&b'$');
        if !(dollar_before && dollar_after) {
            return;
        }
        let (Some(Event::Text(before)), Some(Event::Code(code)), Some(Event::Text(after))) = (
            self.ahead.pop_front(),
            self.ahead.pop_front(),
            self.ahead.pop_front(),
        ) else {
            return;
        };
        let after = trim(after, 1, 0);
        if !after.is_empty() {
            self.ahead.push_front(Event::Text(after));
        }
        // (Not borrowed from where `$…$` is: the digit rule leaves it alone.)
        self.ahead
            .push_front(Event::InlineMath(CowStr::from(code.into_string())));
        let before = trim(before, 0, 1);
        if !before.is_empty() {
            self.ahead.push_front(Event::Text(before));
        }
    }

    /// pulldown-cmark reads `` $`…`$ `` as `$…$` around a code span, which it is: the formula
    /// is the code's text. Any other `$…$` closed right before a digit is the text it was
    /// written as.
    fn inline_math(&self, event: Event<'a>) -> Event<'a> {
        let src = self.src;
        if let Event::InlineMath(s) = &event
            && let Some(inner) = code_span(s)
        {
            let inner = match s {
                CowStr::Borrowed(b) => CowStr::Borrowed(b.get(inner).unwrap_or("")),
                s => CowStr::from(s.get(inner).unwrap_or("").to_owned()),
            };
            return Event::InlineMath(inner);
        }
        if let Event::InlineMath(s) = &event
            && let Some(at) = place(src, s)
            && at.start > 0
            && src.as_bytes().get(at.start - 1) == Some(&b'$')
            && src.as_bytes().get(at.end) == Some(&b'$')
            && src
                .as_bytes()
                .get(at.end + 1)
                .is_some_and(u8::is_ascii_digit)
            && let Some(written) = src.get(at.start - 1..at.end + 1)
        {
            return Event::Text(CowStr::Borrowed(written));
        }
        event
    }
}

/// The text of the code span that `s` is, if it is one: it starts with a run of backticks and
/// ends with the first run as long. As in CommonMark, one space is taken off each end when
/// both have one and there is more than spaces.
fn code_span(s: &str) -> Option<Range<usize>> {
    let b = s.as_bytes();
    let n = b.iter().take_while(|&&c| c == b'`').count();
    if n == 0 {
        return None;
    }
    // The first run of exactly `n` backticks after the opening one.
    let mut at = n;
    let close = loop {
        let run_start = at + b.get(at..)?.iter().position(|&c| c == b'`')?;
        let len = b[run_start..].iter().take_while(|&&c| c == b'`').count();
        if len == n {
            break run_start;
        }
        at = run_start + len;
    };
    if close + n != b.len() || close == n {
        return None;
    }
    let (mut start, mut end) = (n, close);
    if b[start] == b' ' && b[end - 1] == b' ' && b[start..end].iter().any(|&c| c != b' ') {
        start += 1;
        end -= 1;
    }
    Some(start..end)
}

/// `s` without `head` bytes at its start and `tail` at its end (ASCII dollars, so these are
/// char boundaries).
fn trim(s: CowStr<'_>, head: usize, tail: usize) -> CowStr<'_> {
    let end = s.len().saturating_sub(tail).max(head);
    match s {
        CowStr::Borrowed(b) => CowStr::Borrowed(b.get(head..end).unwrap_or("")),
        s => CowStr::from(s.get(head..end).unwrap_or("").to_owned()),
    }
}
