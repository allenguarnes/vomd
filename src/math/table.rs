//! The OpenType MATH table, read in place.
//!
//! Everything is read from the table's bytes when it is asked for: nothing is copied or
//! decoded at load, and a glyph's entry in a coverage table is found by a binary search over the
//! bytes. The bytes may come from a font file on disk (without `embed-fonts`), so they are not
//! trusted: every read is checked, and a table that is cut short or points outside itself gives
//! `None` (no data), never a panic. The caller falls back to the plain glyph.
//!
//! Device tables (hinting adjustments of values) are not read. Values are in font design
//! units.

/// A value of the MathConstants table, in the table's order (OpenType 1.9, MATH, "MathConstants
/// table").

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constant {
    /// Percent of the size scripts are set in (70 for STIX Two).
    ScriptPercentScaleDown,
    ScriptScriptPercentScaleDown,
    DelimitedSubFormulaMinHeight,
    DisplayOperatorMinHeight,
    MathLeading,
    AxisHeight,
    AccentBaseHeight,
    FlattenedAccentBaseHeight,
    SubscriptShiftDown,
    SubscriptTopMax,
    SubscriptBaselineDropMin,
    SuperscriptShiftUp,
    SuperscriptShiftUpCramped,
    SuperscriptBottomMin,
    SuperscriptBaselineDropMax,
    SubSuperscriptGapMin,
    SuperscriptBottomMaxWithSubscript,
    SpaceAfterScript,
    UpperLimitGapMin,
    UpperLimitBaselineRiseMin,
    LowerLimitGapMin,
    LowerLimitBaselineDropMin,
    StackTopShiftUp,
    StackTopDisplayStyleShiftUp,
    StackBottomShiftDown,
    StackBottomDisplayStyleShiftDown,
    StackGapMin,
    StackDisplayStyleGapMin,
    StretchStackTopShiftUp,
    StretchStackBottomShiftDown,
    StretchStackGapAboveMin,
    StretchStackGapBelowMin,
    FractionNumeratorShiftUp,
    FractionNumeratorDisplayStyleShiftUp,
    FractionDenominatorShiftDown,
    FractionDenominatorDisplayStyleShiftDown,
    FractionNumeratorGapMin,
    FractionNumDisplayStyleGapMin,
    FractionRuleThickness,
    FractionDenominatorGapMin,
    FractionDenomDisplayStyleGapMin,
    SkewedFractionHorizontalGap,
    SkewedFractionVerticalGap,
    OverbarVerticalGap,
    OverbarRuleThickness,
    OverbarExtraAscender,
    UnderbarVerticalGap,
    UnderbarRuleThickness,
    UnderbarExtraDescender,
    RadicalVerticalGap,
    RadicalDisplayStyleVerticalGap,
    RadicalRuleThickness,
    RadicalExtraAscender,
    RadicalKernBeforeDegree,
    RadicalKernAfterDegree,
    /// Percent of the radical's height the degree's bottom is raised by.
    RadicalDegreeBottomRaisePercent,
}

impl Constant {
    /// Where the value is in the MathConstants table, and whether it is unsigned. The first
    /// four are bare 16-bit numbers, the last one too; the 51 in between are MathValueRecords
    /// (a value and a device table offset, 4 bytes).
    fn place(self) -> (usize, bool) {
        let i = self as usize;
        match self {
            Constant::ScriptPercentScaleDown | Constant::ScriptScriptPercentScaleDown => {
                (2 * i, false)
            }
            Constant::DelimitedSubFormulaMinHeight | Constant::DisplayOperatorMinHeight => {
                (2 * i, true)
            }
            Constant::RadicalDegreeBottomRaisePercent => (8 + 4 * 51, false),
            _ => (8 + 4 * (i - 4), false),
        }
    }
}

/// A corner of a glyph, for its math kerning (cut-ins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopRight = 0,
    TopLeft = 1,
    BottomRight = 2,
    BottomLeft = 3,
}

/// The MATH table of a font: `MathTable::new(font.table(b"MATH"))`.
#[derive(Clone, Copy, Debug)]
pub struct MathTable<'a> {
    data: &'a [u8],
    /// Offsets of the three subtables from the start of `data`, 0 for none.
    constants: usize,
    glyph_info: usize,
    variants: usize,
}

/// A size variant of a glyph: the glyph, and its advance in the direction it grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    pub glyph: u16,
    pub advance: u16,
}

/// A piece of a [`Assembly`]. Lengths are in the direction the assembly grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub glyph: u16,
    /// How much of the part's start (end) may overlap the part before (after) it.
    pub start_connector: u16,
    pub end_connector: u16,
    pub full_advance: u16,
    /// The part may be repeated any number of times (zero included) to grow the assembly.
    pub extender: bool,
}

/// The ways a glyph can grow in one direction: bigger variants, smallest first, and possibly
/// an assembly of parts for sizes beyond the biggest.
#[derive(Clone, Copy, Debug)]
pub struct Construction<'a> {
    data: &'a [u8],
    /// Start of the MathGlyphConstruction table in `data`.
    at: usize,
    count: usize,
}

/// A glyph assembly: parts laid end to end, bottom to top or left to right.
#[derive(Clone, Copy, Debug)]
pub struct Assembly<'a> {
    /// The parts' records, `count` of 10 bytes each.
    parts: &'a [u8],
    count: usize,
    /// The italics correction of the assembled glyph.
    pub italics_correction: i16,
}

/// The `u16` at `at`, if `data` has it.
fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    u16_at(data, at).map(|v| v as i16)
}

/// The table `base + offset` points at (an offset of 0 is none), if it starts inside `data`.
fn sub(data: &[u8], base: usize, offset_at: usize) -> Option<usize> {
    let offset = u16_at(data, offset_at)?;
    if offset == 0 {
        return None;
    }
    let at = base.checked_add(offset as usize)?;
    (at < data.len()).then_some(at)
}

/// `count` records of `size` bytes from `at` all lie inside `data`.
fn fits(data: &[u8], at: usize, count: usize, size: usize) -> bool {
    count
        .checked_mul(size)
        .and_then(|n| n.checked_add(at))
        .is_some_and(|end| end <= data.len())
}

/// The index of `glyph` in the Coverage table at `at`, if it is covered. Both formats are
/// sorted by glyph id, so this is a binary search in place.
fn coverage(data: &[u8], at: usize, glyph: u16) -> Option<usize> {
    let format = u16_at(data, at)?;
    let count = u16_at(data, at + 2)? as usize;
    let records = at + 4;
    match format {
        1 => {
            if !fits(data, records, count, 2) {
                return None;
            }
            let (mut lo, mut hi) = (0, count);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let g = u16_at(data, records + 2 * mid)?;
                match g.cmp(&glyph) {
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                    std::cmp::Ordering::Equal => return Some(mid),
                }
            }
            None
        }
        2 => {
            if !fits(data, records, count, 6) {
                return None;
            }
            let (mut lo, mut hi) = (0, count);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let r = records + 6 * mid;
                let (start, end) = (u16_at(data, r)?, u16_at(data, r + 2)?);
                if glyph < start {
                    hi = mid;
                } else if glyph > end {
                    lo = mid + 1;
                } else {
                    let first = u16_at(data, r + 4)? as usize;
                    return Some(first + (glyph - start) as usize);
                }
            }
            None
        }
        _ => None,
    }
}

/// [`coverage`] of a Coverage table that starts at the start of `data`, for tests.
#[cfg(test)]
pub(super) fn coverage_index(data: &[u8], glyph: u16) -> Option<usize> {
    coverage(data, 0, glyph)
}

impl<'a> MathTable<'a> {
    /// The table in `data`, if it has a version 1 header. Its subtables are checked when they
    /// are read.
    pub fn new(data: &'a [u8]) -> Option<MathTable<'a>> {
        if u16_at(data, 0)? != 1 || data.len() < 10 {
            return None;
        }
        Some(MathTable {
            data,
            constants: sub(data, 0, 4).unwrap_or(0),
            glyph_info: sub(data, 0, 6).unwrap_or(0),
            variants: sub(data, 0, 8).unwrap_or(0),
        })
    }

    /// A value of the MathConstants table.
    pub fn constant(&self, c: Constant) -> Option<i32> {
        if self.constants == 0 {
            return None;
        }
        let (at, unsigned) = c.place();
        let at = self.constants.checked_add(at)?;
        let v = u16_at(self.data, at)?;
        Some(if unsigned { v as i32 } else { v as i16 as i32 })
    }

    /// The MathGlyphInfo subtable at `slot` (0 italics correction, 1 top accent attachment,
    /// 2 extended shapes, 3 kerning).
    fn glyph_info(&self, slot: usize) -> Option<usize> {
        if self.glyph_info == 0 {
            return None;
        }
        sub(self.data, self.glyph_info, self.glyph_info + 2 * slot)
    }

    /// A value of a table of a coverage and one MathValueRecord per covered glyph, as the
    /// italics correction and top accent attachment tables are.
    fn per_glyph_value(&self, at: usize, glyph: u16) -> Option<i16> {
        let count = u16_at(self.data, at + 2)? as usize;
        let cov = sub(self.data, at, at)?;
        let i = coverage(self.data, cov, glyph)?;
        if i >= count {
            return None;
        }
        i16_at(self.data, at + 4 + 4 * i)
    }

    /// How far to move a superscript right, past the glyph's slant.
    pub fn italics_correction(&self, glyph: u16) -> Option<i16> {
        self.per_glyph_value(self.glyph_info(0)?, glyph)
    }

    /// Where an accent over the glyph is centred, from its left edge (the middle of its
    /// advance if `None`).
    pub fn top_accent_attachment(&self, glyph: u16) -> Option<i16> {
        self.per_glyph_value(self.glyph_info(1)?, glyph)
    }

    /// The glyph is an extended shape (a tall variant, an integral): scripts attach to its
    /// ink, not to its line metrics.
    pub fn is_extended_shape(&self, glyph: u16) -> bool {
        self.glyph_info(2)
            .and_then(|at| coverage(self.data, at, glyph))
            .is_some()
    }

    /// The kerning at `corner` of the glyph for a script whose nearest edge is at `height`
    /// (from the baseline). `None` if the glyph has none there.
    pub fn kern(&self, glyph: u16, corner: Corner, height: i32) -> Option<i16> {
        let info = self.glyph_info(3)?;
        let count = u16_at(self.data, info + 2)? as usize;
        let cov = sub(self.data, info, info)?;
        let i = coverage(self.data, cov, glyph)?;
        if i >= count {
            return None;
        }
        let record = info.checked_add(4 + 8 * i)?;
        let kern = sub(self.data, info, record + 2 * corner as usize)?;
        // heightCount heights, then heightCount + 1 kerns, all MathValueRecords: the kern
        // below the first height, between each two, and above the last.
        let heights = u16_at(self.data, kern)? as usize;
        if !fits(self.data, kern + 2, 2 * heights + 1, 4) {
            return None;
        }
        let mut band = heights;
        for k in 0..heights {
            if height < i16_at(self.data, kern + 2 + 4 * k)? as i32 {
                band = k;
                break;
            }
        }
        i16_at(self.data, kern + 2 + 4 * (heights + band))
    }

    /// How much two parts of an assembly overlap at least.
    pub fn min_connector_overlap(&self) -> Option<u16> {
        if self.variants == 0 {
            return None;
        }
        u16_at(self.data, self.variants)
    }

    /// How the glyph grows vertically (`vertical`) or horizontally, if it does.
    pub fn construction(&self, glyph: u16, vertical: bool) -> Option<Construction<'a>> {
        let v = self.variants;
        if v == 0 {
            return None;
        }
        let vert_count = u16_at(self.data, v + 6)? as usize;
        let horiz_count = u16_at(self.data, v + 8)? as usize;
        let (cov_slot, count, offsets) = if vertical {
            (v + 2, vert_count, v + 10)
        } else {
            (v + 4, horiz_count, v + 10 + 2 * vert_count)
        };
        let cov = sub(self.data, v, cov_slot)?;
        let i = coverage(self.data, cov, glyph)?;
        if i >= count {
            return None;
        }
        let at = sub(self.data, v, offsets + 2 * i)?;
        let n = u16_at(self.data, at + 2)? as usize;
        fits(self.data, at + 4, n, 4).then_some(Construction {
            data: self.data,
            at,
            count: n,
        })
    }
}

impl<'a> Construction<'a> {
    /// How many size variants there are; the first is usually the glyph itself.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Variant `i`, smallest first.
    pub fn variant(&self, i: usize) -> Option<Variant> {
        if i >= self.count {
            return None;
        }
        let r = self.at + 4 + 4 * i;
        Some(Variant {
            glyph: u16_at(self.data, r)?,
            advance: u16_at(self.data, r + 2)?,
        })
    }

    pub fn variants(&self) -> impl Iterator<Item = Variant> + '_ {
        (0..self.count).filter_map(|i| self.variant(i))
    }

    /// The assembly for sizes beyond the variants, if there is one and all its parts are in
    /// the table.
    pub fn assembly(&self) -> Option<Assembly<'a>> {
        let at = sub(self.data, self.at, self.at)?;
        let italics_correction = i16_at(self.data, at)?;
        let count = u16_at(self.data, at + 4)? as usize;
        let start = at + 6;
        if !fits(self.data, start, count, 10) {
            return None;
        }
        Some(Assembly {
            parts: self.data.get(start..start + 10 * count)?,
            count,
            italics_correction,
        })
    }
}

impl Assembly<'_> {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Part `i`, from the bottom (or left).
    pub fn part(&self, i: usize) -> Option<Part> {
        if i >= self.count {
            return None;
        }
        let r = 10 * i;
        Some(Part {
            glyph: u16_at(self.parts, r)?,
            start_connector: u16_at(self.parts, r + 2)?,
            end_connector: u16_at(self.parts, r + 4)?,
            full_advance: u16_at(self.parts, r + 6)?,
            extender: u16_at(self.parts, r + 8)? & 1 != 0,
        })
    }

    pub fn parts(&self) -> impl Iterator<Item = Part> + '_ {
        (0..self.count).filter_map(|i| self.part(i))
    }
}
