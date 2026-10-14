//! Our own text stack on swash.
//!
//! A [`Para`] is a paragraph of styled text items and atomic inline boxes. [`TextCtx::layout`]
//! shapes it (`shape.rs`), breaks it into lines in logical order (`lines.rs`), reorders each
//! line visually for bidi, and positions glyphs. The result is a [`ParaLayout`] in paragraph
//! coordinates (origin = top-left of the first line), which block layout offsets into place.

pub mod cache;
pub mod fallback;
mod lines;
mod shape;

use std::ops::Range;

use swash::shape::ShapeContext;

use crate::fonts::{FaceId, Fonts};
use crate::theme::Role;

pub use lines::LineBreak;
pub use shape::{Cluster, Glyph, Shaped};

/// Text decorations drawn by the painter from [`PRect`]s.
pub mod deco {
    /// Rounded background behind inline code.
    pub const CODE_BG: u8 = 1 << 0;
    pub const STRIKE: u8 = 1 << 1;
    /// Outline around an image placeholder's alt text.
    pub const IMAGE_BOX: u8 = 1 << 2;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    /// Font size in physical pixels.
    pub size: f32,
    pub role: Role,
    pub deco: u8,
    /// Baseline shift in physical pixels, positive = up (superscript).
    pub shift: f32,
    /// 1-based link id from the Doc; 0 = none.
    pub link: u16,
}

/// Atomic inline object (image, checkbox, future math) placed in a line like a glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineBox {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct ParaItem {
    /// Byte range in [`Para::text`].
    pub range: Range<u32>,
    /// Doc byte offset of `range.start`.
    pub doc: u32,
    /// Synthesized text (e.g. expanded tabs): every byte maps to `doc`.
    pub fixed_doc: bool,
    pub style: u16,
    pub inline_box: Option<InlineBox>,
}

/// A paragraph being assembled for layout.
#[derive(Clone, Debug, Default)]
pub struct Para {
    pub text: String,
    pub items: Vec<ParaItem>,
    pub styles: Vec<TextStyle>,
}

/// Object replacement character standing in for inline boxes in `Para::text`, so line
/// breaking sees them.
const OBJECT: char = '\u{FFFC}';

impl Para {
    pub fn clear(&mut self) {
        self.text.clear();
        self.items.clear();
        self.styles.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn style_index(&mut self, style: TextStyle) -> u16 {
        if let Some(i) = self.styles.iter().position(|s| *s == style) {
            return i as u16;
        }
        self.styles.push(style);
        (self.styles.len() - 1) as u16
    }

    fn push(&mut self, s: &str, doc: u32, fixed_doc: bool, style: TextStyle, b: Option<InlineBox>) {
        if s.is_empty() {
            return;
        }
        let style = self.style_index(style);
        let start = self.text.len() as u32;
        self.text.push_str(s);
        let range = start..self.text.len() as u32;
        self.items.push(ParaItem {
            range,
            doc,
            fixed_doc,
            style,
            inline_box: b,
        });
    }

    /// Text copied from the document at byte offset `doc`.
    pub fn push_text(&mut self, s: &str, doc: u32, style: TextStyle) {
        self.push(s, doc, false, style, None);
    }

    /// Text that doesn't exist in the document (tab expansion); maps to `doc`.
    pub fn push_synthetic(&mut self, s: &str, doc: u32, style: TextStyle) {
        self.push(s, doc, true, style, None);
    }

    pub fn push_box(&mut self, b: InlineBox, doc: u32, style: TextStyle) {
        let mut buf = [0u8; 4];
        self.push(OBJECT.encode_utf8(&mut buf), doc, true, style, Some(b));
    }

    /// Doc byte offset for a paragraph byte offset.
    pub fn doc_offset(&self, para_byte: u32) -> u32 {
        let i = self.items.partition_point(|it| it.range.end <= para_byte);
        match self.items.get(i) {
            Some(it) if it.fixed_doc => it.doc,
            Some(it) => it.doc + para_byte.saturating_sub(it.range.start),
            None => self.items.last().map_or(0, |it| it.doc),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// Left for LTR paragraphs, right for RTL paragraphs.
    Start,
    Center,
    Right,
}

/// A glyph positioned in paragraph (later block) coordinates; `y` is its baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PGlyph {
    pub x: f32,
    pub y: f32,
    pub id: u16,
    pub face: FaceId,
    /// Index into the owner's style table.
    pub style: u16,
    /// [`crate::fonts::synth`] bits.
    pub synth: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RectKind {
    Fill,
    /// Outline with the given stroke width.
    Stroke(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub role: Role,
    pub radius: f32,
    pub kind: RectKind,
}

/// One cluster's horizontal extent, for hit-testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PHit {
    pub x0: f32,
    pub x1: f32,
    pub doc: u32,
    pub style: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PBox {
    pub x: f32,
    /// Top of the box.
    pub y: f32,
    pub b: InlineBox,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PLine {
    pub top: f32,
    pub baseline: f32,
    pub bottom: f32,
    /// Width of the line's content (trailing whitespace excluded).
    pub width: f32,
    /// Starts after a soft wrap (code blocks draw a continuation marker).
    pub cont: bool,
    pub hits: Range<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParaLayout {
    pub glyphs: Vec<PGlyph>,
    pub lines: Vec<PLine>,
    pub hits: Vec<PHit>,
    pub rects: Vec<PRect>,
    pub boxes: Vec<PBox>,
    /// Widest line.
    pub width: f32,
    pub height: f32,
    /// Characters no loaded face covers (deduplicated, sorted).
    pub missing: Vec<char>,
}

/// Reusable shaping state; one per layout engine.
pub struct TextCtx {
    shape: ShapeContext,
}

impl Default for TextCtx {
    fn default() -> Self {
        TextCtx {
            shape: ShapeContext::new(),
        }
    }
}

impl TextCtx {
    pub fn shape(&mut self, fonts: &Fonts, para: &Para) -> Shaped {
        shape::shape(&mut self.shape, fonts, para)
    }

    /// Shape, break, and position a paragraph within `max_width`.
    pub fn layout(
        &mut self,
        fonts: &Fonts,
        para: &Para,
        max_width: f32,
        align: Align,
        line_height: f32,
    ) -> ParaLayout {
        let shaped = self.shape(fonts, para);
        lines::position(fonts, para, &shaped, max_width, align, line_height)
    }

    /// (minimum, natural) width: the widest unbreakable segment, and the unwrapped width.
    pub fn measure(&mut self, fonts: &Fonts, para: &Para) -> (f32, f32) {
        let shaped = self.shape(fonts, para);
        lines::measure(para, &shaped)
    }
}

#[cfg(all(test, feature = "embed-fonts"))]
mod tests;
