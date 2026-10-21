//! The text of the find bar's field: the query, the caret and the selection, with the motions and
//! edits of a one-line text field. Positions are byte offsets on `char` boundaries. Nothing here
//! knows how the text is drawn: the view turns pixels into offsets and back.

use crate::find::MAX_QUERY;

/// How the caret moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
}

/// The text, and the selection as the anchor where it began and the caret where it ends (equal
/// when nothing is selected, the caret then being a bar between two characters).
#[derive(Debug, Default)]
pub(super) struct Field {
    text: String,
    anchor: usize,
    caret: usize,
}

/// What a character is to a word: white space, letters, digits and the combining marks that
/// belong to them, or anything else (a run of punctuation is a word of its own).
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' || is_mark(c) {
        1
    } else {
        2
    }
}

/// A combining mark or variation selector, which stays with the character before it.
fn is_mark(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

impl Field {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected span, `lo..hi` (empty at the caret when nothing is selected).
    pub fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    pub fn selection(&self) -> &str {
        let (lo, hi) = self.range();
        &self.text[lo..hi]
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    /// Every character is selected (an empty field has none to select).
    pub fn all_selected(&self) -> bool {
        !self.text.is_empty() && self.range() == (0, self.text.len())
    }

    /// Make `text` the whole text, selected.
    #[cfg_attr(not(feature = "clipboard"), allow(dead_code))]
    pub fn set(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.select_all();
    }

    pub fn select_all(&mut self) {
        (self.anchor, self.caret) = (0, self.text.len());
    }

    /// The offset of the character boundary at or before `at`.
    fn snap(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Select `anchor..caret` (either may be the greater).
    pub fn select(&mut self, anchor: usize, caret: usize) {
        (self.anchor, self.caret) = (self.snap(anchor), self.snap(caret));
    }

    /// Put the caret at `at`, which `extend` makes the end of a selection from where the caret was.
    pub fn place(&mut self, at: usize, extend: bool) {
        self.caret = self.snap(at);
        if !extend {
            self.anchor = self.caret;
        }
    }

    fn before(&self, at: usize) -> Option<char> {
        self.text[..at].chars().next_back()
    }

    fn after(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    /// The offset of the character before `at` (`at` itself at the start); with `marks`, the
    /// character with the combining marks that follow it.
    fn prev(&self, at: usize, marks: bool) -> usize {
        let mut at = at;
        while let Some(c) = self.before(at) {
            at -= c.len_utf8();
            if !(marks && is_mark(c)) {
                break;
            }
        }
        at
    }

    /// The offset after the character at `at` and the combining marks that follow it.
    fn next(&self, at: usize, marks: bool) -> usize {
        let Some(c) = self.after(at) else { return at };
        let mut at = at + c.len_utf8();
        while marks && self.after(at).is_some_and(is_mark) {
            at += self.after(at).map_or(0, char::len_utf8);
        }
        at
    }

    /// The start of the word before `at`: back over white space, then over the run of letters or
    /// of punctuation before it.
    fn word_before(&self, at: usize) -> usize {
        let mut at = at;
        while self.before(at).is_some_and(|c| class(c) == 0) {
            at = self.prev(at, false);
        }
        if let Some(c) = self.before(at) {
            while self.before(at).is_some_and(|d| class(d) == class(c)) {
                at = self.prev(at, false);
            }
        }
        at
    }

    /// The end of the word after `at`: over white space, then over the run of letters or of
    /// punctuation.
    fn word_after(&self, at: usize) -> usize {
        let mut at = at;
        while self.after(at).is_some_and(|c| class(c) == 0) {
            at = self.next(at, false);
        }
        if let Some(c) = self.after(at) {
            while self.after(at).is_some_and(|d| class(d) == class(c)) {
                at = self.next(at, false);
            }
        }
        at
    }

    /// The span of the word (or of the white space, or of the punctuation) at `at`, the character
    /// after it or at the end the one before: what a double click selects.
    pub fn word_at(&self, at: usize) -> (usize, usize) {
        let at = self.snap(at);
        let Some(c) = self.after(at).or_else(|| self.before(at)) else {
            return (at, at);
        };
        let (mut lo, mut hi) = if self.after(at).is_some() {
            (at, at)
        } else {
            (self.prev(at, false), at)
        };
        while self.before(lo).is_some_and(|d| class(d) == class(c)) {
            lo = self.prev(lo, false);
        }
        while self.after(hi).is_some_and(|d| class(d) == class(c)) {
            hi = self.next(hi, false);
        }
        (lo, hi)
    }

    /// Move the caret; with `extend` the anchor stays, and the selection grows or shrinks to the
    /// caret. Without it a selection is given up: Left and Right go to its edge, the others on
    /// from the caret.
    pub fn go(&mut self, motion: Motion, extend: bool) {
        let (lo, hi) = self.range();
        let collapse = !extend && self.has_selection();
        let to = match motion {
            Motion::Left if collapse => lo,
            Motion::Right if collapse => hi,
            Motion::Left => self.prev(self.caret, true),
            Motion::Right => self.next(self.caret, true),
            Motion::WordLeft => self.word_before(self.caret),
            Motion::WordRight => self.word_after(self.caret),
            Motion::Home => 0,
            Motion::End => self.text.len(),
        };
        self.place(to, extend);
    }

    /// Replace the selection with `text` (at most [`MAX_QUERY`] characters stay in all), the caret
    /// after it. Returns true if the text changed.
    pub fn insert(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let (lo, hi) = self.range();
        let kept = self.text.chars().count() - self.text[lo..hi].chars().count();
        let room = MAX_QUERY.saturating_sub(kept);
        let end = text.char_indices().nth(room).map_or(text.len(), |(i, _)| i);
        let text = &text[..end];
        if text.is_empty() {
            return false;
        }
        self.text.replace_range(lo..hi, text);
        self.place(lo + text.len(), false);
        true
    }

    /// Delete the selection, or the character before the caret, or for `word` the word before it.
    /// Returns true if the text changed.
    pub fn backspace(&mut self, word: bool) -> bool {
        if !self.has_selection() {
            let from = if word {
                self.word_before(self.caret)
            } else {
                self.prev(self.caret, false)
            };
            self.anchor = from;
        }
        self.remove()
    }

    /// Delete the selection, or the character after the caret (with its combining marks), or for
    /// `word` the word after it. Returns true if the text changed.
    pub fn delete(&mut self, word: bool) -> bool {
        if !self.has_selection() {
            self.anchor = if word {
                self.word_after(self.caret)
            } else {
                self.next(self.caret, true)
            };
        }
        self.remove()
    }

    /// Remove the selected span, the caret where it was.
    fn remove(&mut self) -> bool {
        let (lo, hi) = self.range();
        self.text.replace_range(lo..hi, "");
        self.place(lo, false);
        lo != hi
    }

    /// Take the selected text out, if there is any.
    #[cfg_attr(not(feature = "clipboard"), allow(dead_code))]
    pub fn cut(&mut self) -> Option<String> {
        let text = self.has_selection().then(|| self.selection().to_owned())?;
        self.remove();
        Some(text)
    }
}

#[cfg(test)]
mod tests;
