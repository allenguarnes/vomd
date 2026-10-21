//! GIF: the first frame, as a browser shows a still GIF and the first moment of an animated
//! one. Its size and cost come from the blocks before the first image ([`walk`]); the decoder is
//! given those blocks without their extensions (all but the frame's graphic control: no XMP,
//! ICC profile or comment is kept), and its rows are streamed to the resampler as RGBA.

use std::io::{Read, Seek};
use std::sync::atomic::AtomicBool;

use super::{Bitmap, Format, Header, MAX_HEADER_SCAN, Rows, Sampling, cancelled, resample_rows};

/// What [`walk`] finds before the first image of a GIF.
struct Blocks {
    /// The logical screen, made big enough for the first frame (see [`walk`]).
    screen: (u16, u16),
    /// The first frame: left, top, width and height.
    frame: [u16; 4],
    interlaced: bool,
    /// Bytes of the global color table, right after the screen descriptor.
    table: u64,
    /// Where the last graphic control extension before the image starts and ends: it holds the
    /// frame's transparent color.
    control: Option<(u64, u64)>,
    /// Where the image descriptor starts.
    image: u64,
}

/// The blocks of a GIF before its first image, `r` just past the first two bytes of the
/// signature: the screen descriptor and global color table, the extensions (skipped, by their
/// sub-blocks), and the image descriptor; within [`MAX_HEADER_SCAN`] bytes. A screen with a side
/// smaller than the first frame reaches (0, as some encoders write, or any other) is made big
/// enough for it, as browsers do. `None` for anything else, a frame with no pixels, or no image.
fn walk(r: &mut (impl Read + Seek)) -> Option<Blocks> {
    let mut head = [0u8; 11];
    r.read_exact(&mut head).ok()?;
    if !matches!(&head[..4], b"F87a" | b"F89a") {
        return None;
    }
    let le = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let flags = head[8];
    let table = if flags & 0x80 != 0 {
        3 << ((flags & 7) + 1)
    } else {
        0
    };
    r.seek_relative(table as i64).ok()?;
    let mut at = 13 + table;
    let mut control = None;
    let byte = |r: &mut _| {
        let mut b = [0u8; 1];
        Read::read_exact(r, &mut b).ok().map(|_| b[0])
    };
    loop {
        if at > MAX_HEADER_SCAN {
            return None;
        }
        match byte(r)? {
            0x21 => {
                let start = at;
                let label = byte(r)?;
                at += 2;
                // Sub-blocks, each its length and as many bytes, to one of length 0.
                loop {
                    let len = byte(r)?;
                    at += 1 + u64::from(len);
                    if len == 0 {
                        break;
                    }
                    if at > MAX_HEADER_SCAN {
                        return None;
                    }
                    r.seek_relative(i64::from(len)).ok()?;
                }
                if label == 0xF9 {
                    control = Some((start, at));
                }
            }
            0x2C => {
                let mut d = [0u8; 9];
                r.read_exact(&mut d).ok()?;
                let frame = [le(&d, 0), le(&d, 2), le(&d, 4), le(&d, 6)];
                if frame[2] == 0 || frame[3] == 0 {
                    return None;
                }
                let reach =
                    |offset: u16, side: u16| u16::try_from(u32::from(offset) + u32::from(side));
                let screen = (
                    le(&head, 4).max(reach(frame[0], frame[2]).ok()?),
                    le(&head, 6).max(reach(frame[1], frame[3]).ok()?),
                );
                return Some(Blocks {
                    screen,
                    frame,
                    interlaced: d[8] & 0x40 != 0,
                    table,
                    control,
                    image: at,
                });
            }
            _ => return None,
        }
    }
}

/// The [`Header`] of a GIF, `r` just past `GI` (see [`walk`]): the screen's size, and for the
/// cost whether the first frame is interlaced (read whole, a byte a pixel: [`Header::held`]).
pub(super) fn header(r: &mut (impl Read + Seek)) -> Option<Header> {
    let b = walk(r)?;
    let [.., fw, fh] = b.frame;
    Some(Header {
        format: Format::Gif,
        w: u32::from(b.screen.0),
        h: u32::from(b.screen.1),
        interlaced: b.interlaced,
        row: 0,
        out_channels: 0,
        coefficients: 0,
        mcu_rows: 0,
        held: if b.interlaced {
            usize::from(fw) * usize::from(fh)
        } else {
            0
        },
        orientation: 1,
        svg: super::SvgCost::default(),
    })
}

/// The first frame of the GIF `bytes`, on a transparent screen (the background color is
/// ignored, as browsers do), resampled to `w` × `h`.
pub(super) fn decode(
    bytes: &[u8],
    w: u32,
    h: u32,
    sampling: Sampling,
    cancel: &AtomicBool,
) -> Option<Bitmap> {
    let b = walk(&mut std::io::Cursor::new(bytes.get(2..)?))?;
    // The signature and the screen as `walk` made it, the color table, the graphic control,
    // then the image on: no other extension.
    let mut head = [0u8; 13];
    head.copy_from_slice(bytes.get(..13)?);
    head[6..8].copy_from_slice(&b.screen.0.to_le_bytes());
    head[8..10].copy_from_slice(&b.screen.1.to_le_bytes());
    let at = |i: u64| usize::try_from(i).ok();
    let table = bytes.get(13..at(13 + b.table)?)?;
    let control = match b.control {
        Some((s, e)) => bytes.get(at(s)?..at(e)?)?,
        None => &[],
    };
    let rest = bytes.get(at(b.image)?..)?;
    let stream = (&head[..]).chain(table).chain(control).chain(rest);

    let [left, top, fw, fh] = b.frame.map(usize::from);
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::Indexed);
    // It holds nothing that grows but metadata, which it isn't given.
    let limit = std::num::NonZeroU64::new((fw * fh) as u64)?;
    options.set_memory_limit(gif::MemoryLimit::Bytes(limit));
    options.check_frame_consistency(true);
    let mut d = options.read_info(stream).ok()?;
    let frame = d.next_frame_info().ok()??;
    let (transparent, interlaced) = (frame.transparent, frame.interlaced);
    let rect = [frame.left, frame.top, frame.width, frame.height].map(usize::from);
    if rect != [left, top, fw, fh] || (d.width(), d.height()) != b.screen {
        return None;
    }
    // The palette as RGBA, the transparent index (and any past the palette's end) clear.
    let mut colors = [[0u8; 4]; 256];
    for (c, rgb) in colors.iter_mut().zip(d.palette().ok()?.as_chunks::<3>().0) {
        *c = [rgb[0], rgb[1], rgb[2], 255];
    }
    if let Some(t) = transparent {
        colors[usize::from(t)] = [0; 4];
    }
    let sw = usize::from(b.screen.0);
    let mut rows = GifRows {
        decoder: d,
        colors,
        frame: [left, top, fw, fh],
        whole: Vec::new(),
        indexed: vec![0; fw],
        rgba: vec![0; 4 * sw],
        next: 0,
    };
    if interlaced {
        // Rows come in four passes: every eighth from 0, every eighth from 4, every fourth from
        // 2, every second from 1.
        rows.whole = vec![0; fw * fh];
        for (start, step) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
            for y in (start..fh).step_by(step) {
                if cancelled(cancel) {
                    return None;
                }
                let row = &mut rows.whole[y * fw..(y + 1) * fw];
                if !rows.decoder.fill_buffer(row).ok()? {
                    return None;
                }
            }
        }
    }
    let size = (u32::from(b.screen.0), u32::from(b.screen.1));
    resample_rows(&mut rows, size, 4, (w, h), sampling, cancel)
}

/// A GIF's first frame as rows of its screen, RGBA: from the decoder a row at a time, or from
/// `whole` (an interlaced frame, read first).
struct GifRows<R: Read> {
    decoder: gif::Decoder<R>,
    colors: [[u8; 4]; 256],
    /// Left, top, width and height.
    frame: [usize; 4],
    whole: Vec<u8>,
    /// The frame's row being read.
    indexed: Vec<u8>,
    /// The screen's row given out.
    rgba: Vec<u8>,
    /// The frame's next row from the decoder.
    next: usize,
}

impl<R: Read> Rows for GifRows<R> {
    fn row(&mut self, y: usize) -> Option<&[u8]> {
        let [left, top, fw, fh] = self.frame;
        self.rgba.fill(0);
        if !(top..top + fh).contains(&y) {
            return Some(&self.rgba);
        }
        let fy = y - top;
        let indexed = if self.whole.is_empty() {
            if fy < self.next {
                return None;
            }
            while self.next <= fy {
                if !self.decoder.fill_buffer(&mut self.indexed).ok()? {
                    return None;
                }
                self.next += 1;
            }
            &self.indexed[..]
        } else {
            &self.whole[fy * fw..(fy + 1) * fw]
        };
        let out = &mut self.rgba[4 * left..4 * (left + fw)];
        for (px, &i) in out.as_chunks_mut::<4>().0.iter_mut().zip(indexed) {
            px.copy_from_slice(&self.colors[usize::from(i)]);
        }
        Some(&self.rgba)
    }
}

#[cfg(test)]
mod tests;
