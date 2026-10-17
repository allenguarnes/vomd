//! Images: where an image's file is, its size, decoding PNG and JPEG,
//! and the cache of decoded images with the worker thread that fills it.
//!
//! Layout only needs an image's size, which comes from its header (a few bytes, read when its
//! block is first measured), so the page doesn't jump when the pixels arrive. Pixels are decoded
//! lazily, for boxes within a screen of the viewport, on a worker thread, straight to the size
//! the box is shown at; the full-size pixels are dropped. Decoded images (and the bytes of
//! fetched remote ones, see `remote.rs`) share a ~32 MB budget; the least recently painted go
//! first, never one painted in the current frame. A fetched body gets in only if it fits, so
//! those for the images in view can't pile up undecoded ([`Images::admission`]).
// Without decoders nothing is ever queued: the worker and the cache are dead code.
#![cfg_attr(not(feature = "images"), allow(dead_code))]

use std::io::Read;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use crate::doc::{Doc, ImageRef};
use crate::layout::{ImageSize, Layout};
use crate::settings::RemoteImages;

/// Memory for decoded images (and fetched bytes), at most. Images painted in the
/// current frame are never evicted, so a window full of images may hold more.
pub const CACHE_BYTES: usize = 32 << 20;
/// Largest local image file read, in bytes.
pub const MAX_FILE: u64 = 64 << 20;
/// Images with more pixels than this aren't decoded: a few KB of PNG can claim gigapixels.
pub const MAX_PIXELS: u64 = 64_000_000;
/// Largest side of an image box, and so of a decoded (resampled) image: layout never makes a
/// box bigger, so what it asks for is what `decode` makes.
pub const MAX_SIDE: u32 = 16_384;
/// Largest side of an image decoded, at most: a JPEG's own limit. A PNG may claim 2³¹ − 1, which
/// even within [`MAX_PIXELS`] (64,000,000 × 1) makes rows of hundreds of MB.
pub const MAX_SOURCE_SIDE: u32 = 65_535;
/// Bytes of a JPEG looked through for its size, at most (segments before the frame header are
/// metadata: EXIF, color profiles, thumbnails).
const MAX_HEADER_SCAN: u64 = 4 << 20;
/// What a JPEG's decoder keeps of its metadata segments (EXIF, XMP, ICC profile, IPTC), at most,
/// as [`jpeg_metadata`] counts it: a JPEG with more has the segments cut out before it is
/// decoded ([`shed_jpeg_metadata`]). None of it is used, and none of it is in the file's header.
const MAX_JPEG_META: usize = 128 << 10;
/// What decoding a JPEG holds for its metadata: the segments kept and, for extended XMP, the
/// copy made of them put together.
const JPEG_META_COST: usize = 2 * MAX_JPEG_META;

/// Calls the event loop back when a result is waiting ([`Images::poll`]).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A fetched image's bytes: the buffer the body was read into, not copied to a slice.
pub(crate) type Bytes = Arc<Vec<u8>>;

/// A decoded image at the size it is shown: premultiplied `0xAARRGGBB`, rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
}

// ---- Headers ----------------------------------------------------------------------------

/// The format and natural size of the image `r` starts with, read from the PNG `IHDR` chunk
/// or the JPEG frame header (`SOFn`). `None` for anything else (GIF, WebP, SVG, ...), broken
/// data, a zero size or a side over [`MAX_SOURCE_SIDE`].
pub fn header(r: &mut impl Read) -> Option<(Format, u32, u32)> {
    header_info(r).map(|h| (h.format, h.w, h.h))
}

/// What [`header`] reads, and what decoding the image will hold that its size alone doesn't say
/// (see [`decode_cost`], which turns it into bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    format: Format,
    w: u32,
    h: u32,
    /// PNG: Adam7, so the whole frame is decoded, not a row at a time. (Whether the file is
    /// interlaced is the last byte of its `IHDR`.)
    interlaced: bool,
    /// PNG: bytes of a row of the data as stored, its filter byte included, the widest it can
    /// be (16-bit RGBA) if the `IHDR` is cut short. 0 for a JPEG.
    row: usize,
    /// PNG: bytes a pixel has after the decoder's transformations (`normalize_to_color8`: 8 bits
    /// a channel, and an alpha channel if a `tRNS` chunk follows, which isn't known yet, so
    /// always one for an RGB or gray image); the whole frame of an interlaced PNG is this many
    /// bytes a pixel. 0 for a JPEG.
    out_channels: usize,
    /// JPEG: the bytes of coefficients its decoder holds for the whole image (every
    /// component's samples as `i16`, padded to whole MCUs): for a progressive JPEG, and for a
    /// baseline one whose first scan doesn't hold all its components (the decoder then waits
    /// for the rest of the scans before it makes pixels). 0 for a baseline JPEG that is
    /// decoded an MCU row at a time, and for a PNG.
    coefficients: usize,
    /// JPEG: the bytes of what the decoder keeps per MCU row (see [`JPEG_ROW_BYTES`]).
    mcu_rows: usize,
}

fn header_info(r: &mut impl Read) -> Option<Header> {
    let mut start = [0u8; 2];
    r.read_exact(&mut start).ok()?;
    match start {
        [0x89, b'P'] => {
            // The rest of the signature, the first chunk's length and type, width, height.
            let mut b = [0u8; 22];
            r.read_exact(&mut b).ok()?;
            if &b[..6] != b"NG\r\n\x1a\n" || &b[10..14] != b"IHDR" {
                return None;
            }
            let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            let (w, h) = (be(14), be(18));
            let ok = (1..=MAX_SOURCE_SIDE).contains(&w) && (1..=MAX_SOURCE_SIDE).contains(&h);
            // Bit depth, color type, compression, filter and interlace method. A header cut
            // here isn't a PNG the decoder reads, but it has a size: it is costed as the worst.
            let mut rest = [16, 6, 0, 0, 1];
            let _ = r.read_exact(&mut rest);
            let (channels, out_channels) = match rest[1] {
                0 => (1, 2),
                2 => (3, 4),
                3 => (1, 4),
                4 => (2, 2),
                _ => (4, 4),
            };
            let bits = usize::from(rest[0].clamp(1, 16)) * channels;
            ok.then_some(Header {
                format: Format::Png,
                w,
                h,
                interlaced: rest[4] == 1,
                row: 1 + (w as usize * bits).div_ceil(8),
                out_channels,
                coefficients: 0,
                mcu_rows: 0,
            })
        }
        [0xFF, 0xD8] => jpeg_size(r).map(|j| Header {
            format: Format::Jpeg,
            w: j.w,
            h: j.h,
            interlaced: false,
            row: 0,
            out_channels: 0,
            coefficients: j.coefficients,
            mcu_rows: j.mcu_rows,
        }),
        _ => None,
    }
}

/// What [`jpeg_size`] finds in a JPEG's headers.
struct JpegSize {
    w: u32,
    h: u32,
    coefficients: usize,
    mcu_rows: usize,
}

/// A JPEG's segments read one after another from the start-of-image marker, within
/// [`MAX_HEADER_SCAN`] bytes.
struct JpegSegments<'a, R: Read> {
    r: &'a mut R,
    scanned: u64,
}

impl<R: Read> JpegSegments<'_, R> {
    fn byte(&mut self) -> Option<u8> {
        let mut b = [0u8; 1];
        self.r.read_exact(&mut b).ok().map(|_| b[0])
    }

    /// The marker of the next segment, and the bytes of its payload (what its length says, less
    /// the length itself: none for the end of the image, which has no length). Every marker is
    /// taken to have a length, as the decoder does (TEM and RSTn have no segment in the
    /// standard, but the decoder skips a length for them like for any marker it doesn't know:
    /// the two must agree on where the frame header is, see `decode`).
    fn next(&mut self) -> Option<(u8, u64)> {
        if self.byte()? != 0xFF {
            return None;
        }
        // Any number of fill bytes may come before the marker.
        let mut marker = 0xFF;
        while marker == 0xFF {
            marker = self.byte()?;
            self.scanned += 1;
            if self.scanned > MAX_HEADER_SCAN {
                return None;
            }
        }
        if marker == 0xD9 {
            return Some((marker, 0));
        }
        let len = u16::from_be_bytes([self.byte()?, self.byte()?]);
        (len >= 2).then_some((marker, u64::from(len - 2)))
    }

    /// Read past `n` bytes of a payload.
    fn skip(&mut self, n: u64) -> Option<()> {
        self.scanned += n + 2;
        if self.scanned > MAX_HEADER_SCAN {
            return None;
        }
        for _ in 0..n {
            self.byte()?;
        }
        Some(())
    }
}

/// The size in a JPEG's frame header, the segments after the start-of-image marker walked
/// (and skipped) until it, and what its decoder holds that the size doesn't say (see
/// [`Header`]), which takes the first scan's header too (a walk that doesn't get there is
/// taken to find a scan with fewer components, the larger cost).
fn jpeg_size(r: &mut impl Read) -> Option<JpegSize> {
    let mut seg = JpegSegments { r, scanned: 0 };
    let (w, h, progressive, sampling) = loop {
        let (marker, len) = seg.next()?;
        // The image ended, or its data began, before any frame header.
        if matches!(marker, 0xD9 | 0xDA) {
            return None;
        }
        // SOF0..SOF15, but not DHT (C4), JPG (C8) or DAC (CC).
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            // Precision, height, width, components; then id, sampling (H << 4 | V) and
            // quantization table of each component.
            let mut sof = [0u8; 6];
            for b in &mut sof {
                *b = seg.byte()?;
            }
            let h = u32::from(u16::from_be_bytes([sof[1], sof[2]]));
            let w = u32::from(u16::from_be_bytes([sof[3], sof[4]]));
            let mut sampling = Vec::with_capacity(usize::from(sof[5]));
            for _ in 0..sof[5] {
                let (_, hv, _) = (seg.byte()?, seg.byte()?, seg.byte()?);
                sampling.push((u64::from(hv >> 4).max(1), u64::from(hv & 15).max(1)));
            }
            // A height of 0 is defined later in the data (DNL): not worth supporting.
            if w == 0 || h == 0 {
                return None;
            }
            // SOF2, SOF6, SOF10 and SOF14 are progressive.
            let progressive = matches!(marker, 0xC2 | 0xC6 | 0xCA | 0xCE);
            // What the length says is left of the frame header.
            let read = 6 + 3 * u64::from(sof[5]);
            seg.scanned += read;
            let rest = len.saturating_sub(read);
            // (A frame header cut short here is the size all the same.)
            let _ = seg.skip(rest);
            break (w, h, progressive, sampling);
        }
        seg.skip(len)?;
    };
    // On to the first scan: its header starts with the number of components in it.
    let first_scan = loop {
        match seg.next() {
            Some((0xDA, len)) if len >= 1 => break seg.byte().map(usize::from),
            Some((0xDA | 0xD9, _)) | None => break None,
            Some((_, len)) => {
                if seg.skip(len).is_none() {
                    break None;
                }
            }
        }
    };
    Some(jpeg_info(w, h, progressive, &sampling, first_scan))
}

/// [`JpegSize`] of a frame of `w` × `h` with components sampled `sampling` (H, V), whose first
/// scan has `first_scan` of them (`None` if not known).
fn jpeg_info(
    w: u32,
    h: u32,
    progressive: bool,
    sampling: &[(u64, u64)],
    first_scan: Option<usize>,
) -> JpegSize {
    let hmax = sampling.iter().map(|s| s.0).max().unwrap_or(1);
    let vmax = sampling.iter().map(|s| s.1).max().unwrap_or(1);
    let (mw, mh) = (8 * hmax, 8 * vmax);
    let (pw, ph) = (
        u64::from(w).div_ceil(mw) * mw,
        u64::from(h).div_ceil(mh) * mh,
    );
    // The decoder holds every coefficient when it can't make pixels from the first scan.
    let all_in_first = first_scan.is_some_and(|n| n >= sampling.len());
    let coefficients = if progressive || !all_in_first {
        let samples: u64 = sampling
            .iter()
            .map(|&(hc, vc)| pw * hc / hmax * (ph * vc / vmax))
            .sum();
        usize::try_from(samples * 2).unwrap_or(usize::MAX)
    } else {
        0
    };
    let mcu_rows = usize::try_from(sampling.len() as u64 * pw * mh * JPEG_ROW_BYTES as u64)
        .unwrap_or(usize::MAX);
    JpegSize {
        w,
        h,
        coefficients,
        mcu_rows,
    }
}

// ---- Local files ------------------------------------------------------------------------

/// The local file an image's `src` names: a path, absolute or relative to `base` (the
/// document's directory), percent-decoded, without its `?query` or `#fragment`. `None` for URLs
/// (anything with a scheme), for network paths (see [`is_network`]) and for a relative path
/// without a base (a document read from stdin).
pub fn local_path(src: &str, base: Option<&Path>) -> Option<PathBuf> {
    let src = src.trim();
    if src.is_empty() || src.chars().any(char::is_control) {
        return None;
    }
    // A scheme is two characters or more: `C:\x.png` is a Windows path.
    if let Some(colon) = src.find(':')
        && colon >= 2
        && src[..colon]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && src.as_bytes()[0].is_ascii_alphabetic()
    {
        return None;
    }
    let path = src.split(['?', '#']).next().unwrap_or_default();
    let path = crate::nav::percent_decode(path);
    // Windows connects to a server just to look at its path (and sends credentials): no image
    // may name one, written as is or percent-encoded. A filename can't hold `?` or `*` there,
    // and the NT form `\??\UNC\host\share` (taken by the system as it is, with no parsing) has
    // two: a `?` that survives the query split must have been encoded, so it is refused (on
    // every system, which keeps this testable).
    if is_network(src) || is_network(&path) || path.contains(['?', '*']) {
        return None;
    }
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty() {
        None
    } else if path.is_absolute() || path.has_root() {
        Some(path)
    } else {
        Some(base?.join(path))
    }
}

/// Whether `path` names a file on another machine, or a device, for Windows: two separators
/// in a row at its start, in any mix of `/` and `\` (`\\host\share`, `//host/share`, the
/// `\\?\` and `\\.\` forms), a separator, `??` and a separator (the NT form
/// `\??\UNC\host\share`, which the system takes as it is: written with its `?` it would have
/// begun the query, so `local_path` also refuses any decoded `?`), or any prefix but a plain
/// drive letter. Plain drive-letter and rooted paths are the only absolute ones allowed.
/// (Elsewhere a mount like autofs's `/net/host` can reach a server the same way: a path is only
/// as local as the system makes it.)
fn is_network(path: &str) -> bool {
    let sep = |c: Option<char>| matches!(c, Some('/' | '\\'));
    let mut c = path.chars();
    let (a, b, x, y) = (c.next(), c.next(), c.next(), c.next());
    (sep(a) && (sep(b) || (b == Some('?') && x == Some('?') && sep(y))))
        || matches!(
            Path::new(path).components().next(),
            Some(Component::Prefix(p)) if !matches!(p.kind(), Prefix::Disk(_))
        )
}

/// `path` opened for reading if it's a regular file of at most [`MAX_FILE`] bytes. An image
/// path can name a FIFO or a device: it is refused without waiting on it or reading it (see
/// `view::read_input` for the same care with documents).
fn open_regular(path: &Path) -> Option<std::fs::File> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let file = crate::platform::open_nonblocking(path).ok()?;
    let meta = file.metadata().ok()?;
    (meta.is_file() && meta.len() <= MAX_FILE).then_some(file)
}

/// The natural size of a local PNG or JPEG, from its header (`None`: missing, unreadable, not
/// PNG or JPEG, or too large to decode: see [`MAX_PIXELS`] and [`MAX_SOURCE_SIDE`]).
pub fn probe_file(path: &Path) -> Option<(u32, u32)> {
    let file = open_regular(path)?;
    let (_, w, h) = header(&mut std::io::BufReader::with_capacity(4096, file))?;
    (u64::from(w) * u64::from(h) <= MAX_PIXELS).then_some((w, h))
}

/// The size layout gives a local image: its natural size, or its alt text.
fn probe(img: &ImageRef, base: Option<&Path>) -> ImageSize {
    if img.remote {
        return ImageSize::Alt;
    }
    match local_path(&img.src, base).and_then(|p| probe_file(&p)) {
        Some((w, h)) => ImageSize::Pixels(w, h),
        None => ImageSize::Alt,
    }
}

/// A file's bytes, read whole, if the file is at most `limit` bytes (what a decode of it was
/// charged for: a file that grew since its header was read is refused before anything is
/// allocated for it).
fn read_file(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let file = open_regular(path)?;
    let len = file.metadata().ok()?.len().min(MAX_FILE) as usize;
    if len > limit {
        return None;
    }
    // `take` has no size hint: without the length the buffer doubles its way up, leaving up to
    // twice the file in capacity.
    let mut bytes = Vec::with_capacity(len + 1);
    // One byte more than the file had says it grew while it was read.
    file.take(len as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= len).then_some(bytes)
}

/// What decoding an image to `w` × `h` holds at once while it runs, its result included but not
/// the bytes it reads (a file's are in [`FileInfo::cost`], a fetched body's are the cache's):
/// one formula for every path, read from the code of the decoders (`png` 0.18, `zune-jpeg`
/// 0.5) and of [`resample_rows`], each term exact or an upper bound, nothing in it twice. With
/// the source `sw` × `sh` pixels:
///
/// `cost = result + resample + source`
///
/// * `result = 4·w·h`: `resample_rows` makes its `Vec` of 4-byte pixels with exactly that
///   capacity.
/// * `resample = `[`resample_cost`]` = spans(sw, w) + spans(sh, h) + 32·w`: both axes' [`Spans`]
///   are two flat buffers made with exactly the capacity [`spans_cost`] charges (see
///   [`spans_weights`] for why the weights fit it), and `reduced` and `acc` are `w × 4` `f32`
///   each. Nothing else is allocated by it, a row of the source being borrowed.
/// * `source` for a PNG read by rows (not interlaced): [`PNG_FIXED`] plus [`PNG_ROW_FACTOR`]·`R`
///   plus [`PNG_WIDTH_FACTOR`]·`sw`, `R` the stored row (filter byte included): the decoder's
///   buffers, which [`PNG_ROW_FACTOR`] derives, and the chunk buffer, kept to `4·sw + 4 KB`
///   by the `Limits` [`decode_png`] sets. The decoder is given no other chunk than the critical
///   ones and `tRNS` ([`PngChunks`]).
/// * `source` for an interlaced PNG, which is decoded whole: `out_channels·sw·sh` (the buffer
///   `decode_png` makes of `output_buffer_size`) and the same working memory as for rows (an
///   Adam7 pass's row is never longer than `R`).
/// * `source` for a JPEG: `3·sw·sh` (the decoder's output, `output_buffer_size` of RGB,
///   allocated whole and kept until resampled), [`Header::coefficients`] (the whole image's
///   `i16` samples, padded to MCUs, when the decoder can't make pixels as the first scan is
///   decoded: progressive, or the first scan with only some of the components),
///   [`Header::mcu_rows`] (the buffers it keeps for one row of MCUs, by component: coefficients
///   of the row, the two rows and the destination of upsampling, the scratch of its width: at
///   most [`JPEG_ROW_BYTES`] for each pixel of a padded row and row of the MCU's height) and
///   [`JPEG_META_COST`] (the segments it copies, bounded by [`shed_jpeg_metadata`]).
///
/// Every term is checked against what the decoders allocate, counted, in `tests/image_memory.rs`
/// (every combination of color type, depth, interlacing, sampling, scans and shape tried there
/// is under its charge) and by its random mixes.
fn decode_cost(header: &Header, w: u32, h: u32) -> usize {
    let pixels = header.w as usize * header.h as usize;
    let source = match header.format {
        Format::Jpeg => pixels
            .saturating_mul(3)
            .saturating_add(header.coefficients)
            .saturating_add(header.mcu_rows)
            .saturating_add(JPEG_META_COST),
        Format::Png => {
            let frame = if header.interlaced {
                pixels.saturating_mul(header.out_channels)
            } else {
                0
            };
            frame
                .saturating_add(PNG_FIXED)
                .saturating_add(PNG_ROW_FACTOR.saturating_mul(header.row))
                .saturating_add(PNG_WIDTH_FACTOR.saturating_mul(header.w as usize))
        }
    };
    source
        .saturating_add(resample_cost(header.w, header.h, w, h))
        .saturating_add(w as usize * h as usize * 4)
}

/// What a PNG's decoder holds however small the image is, at most: its inflate state (a few
/// tables of 16 KB in all), its own structures, and the stream buffer's `Vec` growing to twice
/// 128 KB and the rest of what [`PNG_ROW_FACTOR`] doesn't say.
const PNG_FIXED: usize = 512 << 10;

/// What a PNG's decoder holds for each byte of a stored row `R`, at most: its stream buffer
/// (`UnfilteringBuffer`) is moved down when `max(128 KB, 4R)` bytes of rows have been used up and
/// holds, with the row being read and 8 KB more, at most `max(128 KB, 4R) + 2R + 8 KB`, and
/// the `Vec` is at most twice as big as it needs to be (`resize` doubles it): 8R + 4R beside
/// the fixed 256 KB + 16 KB; and the row before the one worked on, which is copied when the row
/// isn't whole yet (`R`), and one more for the allocator and what the estimate misses.
const PNG_ROW_FACTOR: usize = 14;

/// What a PNG's decoder holds for each pixel of the width: its output row (at most 4 bytes a
/// pixel after `normalize_to_color8`, whatever the stored row: a 1-bit palette image is
/// 1/8 byte a pixel stored, and 4 expanded), and the chunk buffer, which `Limits::bytes` (set
/// in `decode_png`) keeps to 4 bytes a pixel and 4 KB (the largest palette and transparency
/// chunks are 1 KB in all, anything longer is refused); and a row's worth more.
const PNG_WIDTH_FACTOR: usize = 12;

/// What the decoder of a JPEG holds per MCU row, per component and per pixel of the padded
/// width and row of the MCU's height: the coefficients, the upsampling rows and the like of
/// `zune-jpeg`, as `i16`s (see [`decode_cost`]).
const JPEG_ROW_BYTES: usize = 16;

/// A file's size, header and so what decoding it holds (`None`: see [`probe_file`]).
#[derive(Clone, Copy, Debug)]
struct FileInfo {
    /// The file's bytes: they are read whole.
    len: usize,
    header: Header,
}

impl FileInfo {
    /// What decoding the file to `w` × `h` holds all told, its own bytes included.
    fn cost(&self, w: u32, h: u32) -> usize {
        self.len.saturating_add(decode_cost(&self.header, w, h))
    }
}

/// The size and header of a local file, from its header (`None`: see [`probe_file`]).
fn file_info(path: &Path) -> Option<FileInfo> {
    let file = open_regular(path)?;
    let len = file.metadata().ok()?.len() as usize;
    let header = header_info(&mut std::io::BufReader::with_capacity(4096, file))?;
    Some(FileInfo { len, header })
}

/// What decoding an image already in memory to `w` × `h` holds, its bytes not included.
#[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
fn bytes_cost(bytes: &[u8], w: u32, h: u32) -> Option<usize> {
    Some(decode_cost(&header_info(&mut &bytes[..])?, w, h))
}

/// What decoding `bytes` to `w` × `h` (see [`clamp_box`]) holds at most besides the bytes and
/// the pixels it returns (`None`: not an image [`decode`] reads), for tests that count what the
/// decode allocates.
#[doc(hidden)]
pub fn decode_reservation(bytes: &[u8], w: u32, h: u32) -> Option<usize> {
    let (w, h) = clamp_box(w, h);
    Some(bytes_cost(bytes, w, h)?.saturating_sub(w as usize * h as usize * 4))
}

// ---- Decoding ---------------------------------------------------------------------------

/// The size [`decode`] makes an image of box size `w` × `h`: each side at least 1 and at most
/// [`MAX_SIDE`]. What the cache is asked for and told it has goes through this, so a size that
/// had to be clamped is still found.
pub fn clamp_box(w: u32, h: u32) -> (u32, u32) {
    (w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE))
}

/// Decode a PNG or JPEG and resample it to `w` × `h` (see [`clamp_box`]). `None` for anything
/// else, broken data, or more than [`MAX_PIXELS`].
#[cfg(feature = "images")]
pub fn decode(bytes: &[u8], w: u32, h: u32) -> Option<Bitmap> {
    let (format, sw, sh) = header(&mut &bytes[..])?;
    if u64::from(sw) * u64::from(sh) > MAX_PIXELS {
        return None;
    }
    let (w, h) = clamp_box(w, h);
    match format {
        Format::Png => decode_png(bytes, w, h, true),
        Format::Jpeg => {
            use zune_jpeg::zune_core::{
                bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
            };
            // The decoder keeps every EXIF, XMP and ICC segment it reads, wherever they are: more
            // than `MAX_JPEG_META` of them are cut out first. (A caller that owns the bytes has
            // done it already, in place, see `shed_jpeg_metadata`; this copy is for the rest.)
            let shed;
            let bytes = if jpeg_metadata(bytes) > MAX_JPEG_META {
                let mut copy = bytes.to_vec();
                if !shed_jpeg_metadata(&mut copy) {
                    return None;
                }
                shed = copy;
                &shed[..]
            } else {
                bytes
            };
            // The decoder may find a frame header other than the one `header` did (they read
            // odd markers differently): it gets the size we checked as its limit, and what it
            // reads must be that size, before it allocates anything.
            let options = DecoderOptions::default()
                .jpeg_set_out_colorspace(ColorSpace::RGB)
                .set_max_width(sw as usize)
                .set_max_height(sh as usize);
            let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
            d.decode_headers().ok()?;
            // Neither side is over the limit, so the product is the size only if both are it.
            // (`info` would copy all the metadata, to read what this says.)
            if d.output_buffer_size()? != sw as usize * sh as usize * 3 {
                return None;
            }
            let data = d.decode().ok()?;
            let frame = Frame {
                data: &data,
                width: sw,
                height: sh,
                stride: sw as usize * 3,
                channels: 3,
            };
            resample(frame, w, h)
        }
    }
}

/// The segments of a JPEG as its decoder (zune-jpeg 0.5) finds them: [`JpegWalk::next`] gives
/// the start, end and code of every `APP1` (EXIF, XMP, extended XMP), `APP2` (ICC profile, HDR
/// gain map, MPF) and `APP13` (IPTC) segment, the ones it keeps a copy of. A segment is a
/// marker, `FF` and a code, and the length that follows it (which counts itself), and what is
/// inside one, a comment holding `FF E1 FF FF` say, is its payload, skipped by the decoder and by
/// this. Before the first start of scan the decoder looks for `FF` and skips any `FF` and `00`
/// after it; in the scan data after it `FF 00` is a stuffed byte, and `FF`, fill and a code is a
/// marker (a restart marker has no segment; any other the decoder gives a segment, wherever it
/// is: even between and inside scans, where none should be). The walk ends at the end of the
/// image, or where the decoder gives up (a length under 2, one past the end of the file) having
/// kept what came before. Where this can't be sure of the decoder it takes the larger count: an
/// unknown marker in scan data is skipped by its length, as the decoder does between scans (in a
/// scan it fails).
///
/// It reads the bytes after the last segment it gave only: the ones before may be moved
/// (see [`shed_jpeg_metadata`]).
#[cfg(feature = "images")]
struct JpegWalk {
    /// Where to look next: 0 before the first call (the start of the image is checked).
    at: usize,
    scan: bool,
}

#[cfg(feature = "images")]
impl JpegWalk {
    fn new() -> JpegWalk {
        JpegWalk { at: 0, scan: false }
    }

    fn next(&mut self, bytes: &[u8]) -> Option<(usize, usize, u8)> {
        if self.at == 0 {
            if !bytes.starts_with(&[0xFF, 0xD8]) {
                return None;
            }
            self.at = 2;
        }
        let len = bytes.len();
        while let Some(at) = bytes
            .get(self.at..)
            .and_then(|b| b.iter().position(|&b| b == 0xFF))
        {
            let start = self.at + at;
            let mut q = start + 1;
            if self.scan {
                while bytes.get(q) == Some(&0xFF) {
                    q += 1;
                }
                if bytes.get(q) == Some(&0x00) {
                    self.at = q + 1;
                    continue;
                }
            } else {
                while matches!(bytes.get(q), Some(0x00 | 0xFF)) {
                    q += 1;
                }
            }
            let code = *bytes.get(q)?;
            match code {
                0xD9 => break,
                0xD0..=0xD7 if self.scan => {
                    self.at = q + 1;
                    continue;
                }
                _ => {}
            }
            let l = bytes.get(q + 1..q + 3)?;
            let n = usize::from(u16::from_be_bytes([l[0], l[1]]));
            if n < 2 {
                break;
            }
            let end = (q + 1 + n).min(len);
            self.scan |= code == 0xDA;
            self.at = end;
            if matches!(code, 0xE1 | 0xE2 | 0xED) {
                return Some((start, end, code));
            }
        }
        // The end, or where the decoder gives up: nothing more is read.
        self.at = len.max(2);
        None
    }
}

/// Upper bound on the bytes a JPEG decoder keeps of what is in `bytes`: for every metadata
/// segment ([`JpegWalk`]) the length it declares (what the decoder copies, as far as the
/// file goes) and [`JPEG_META_ENTRY`] for what it keeps beside it.
#[cfg(feature = "images")]
fn jpeg_metadata(bytes: &[u8]) -> usize {
    let mut total = 0usize;
    let mut walk = JpegWalk::new();
    while let Some((start, end, _)) = walk.next(bytes) {
        // The marker, its fill and the length are not copied.
        let payload = (end - start).saturating_sub(4);
        total = total.saturating_add(payload + JPEG_META_ENTRY);
    }
    total
}

/// What the decoder keeps besides a metadata segment's bytes: its entry in a `Vec`, the buffer's
/// allocator rounding and, for extended XMP, the segment's GUID.
#[cfg(feature = "images")]
const JPEG_META_ENTRY: usize = 256;

/// Cut every `APP1`, `APP2` and `APP13` segment out of a JPEG in place (`FF D8 ...`: the
/// metadata its decoder keeps and nothing here uses; `APP0` and `APP14` stay, the decoder reads
/// the color transform from `APP14`), if there is more than [`MAX_JPEG_META`] of it, the segments
/// as [`JpegWalk`] finds them. `false` if what is left is still over the limit (cutting put
/// together bytes the decoder reads as a segment, which the walk of the bytes as they were
/// didn't find): the file is refused.
#[cfg(feature = "images")]
pub(crate) fn shed_jpeg_metadata(buf: &mut Vec<u8>) -> bool {
    if jpeg_metadata(buf) <= MAX_JPEG_META {
        return true;
    }
    // `buf[..kept]` is what stays; `buf[from..]` hasn't been looked at, or is to stay once it is
    // (the walk reads on from the end of the last segment, which is not moved: no list of
    // segments is kept, 450,000 of them being 7 MB).
    let (mut kept, mut from) = (0, 0);
    let mut walk = JpegWalk::new();
    while let Some((start, end, _)) = walk.next(buf) {
        buf.copy_within(from..start, kept);
        kept += start - from;
        from = end;
    }
    let len = buf.len();
    buf.copy_within(from..len, kept);
    buf.truncate(kept + len - from);
    buf.shrink_to_fit();
    jpeg_metadata(buf) <= MAX_JPEG_META
}

/// A PNG's pixels resampled to `w` × `h`. A PNG that isn't interlaced is read a row at a time
/// (`rows`; `false` reads the whole frame, as an interlaced one must be): its full-size pixels
/// can be hundreds of MB, and the memory the allocator then keeps would sit beside a full cache.
#[cfg(feature = "images")]
fn decode_png(bytes: &[u8], w: u32, h: u32, rows: bool) -> Option<Bitmap> {
    let sw = header(&mut &bytes[..])?.1;
    // What the decoder may allocate for itself, besides the buffers [`decode_cost`] counts by the
    // stored row: the output row (at most 4 bytes a pixel of the source's width) and the palette
    // and `tRNS` (the chunks it is given besides the pixels, see `PngChunks`: 768 and 256
    // bytes). A chunk longer than that is read into a buffer that has no more room than this,
    // and is refused. Not the pixels.
    let limits = png::Limits {
        bytes: sw as usize * 4 + 4096,
    };
    let mut d = png::Decoder::new_with_limits(PngChunks::new(bytes), limits);
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = d.read_info().ok()?;
    let (width, height) = reader.info().size();
    let channels = match reader.output_color_type().0 {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    if rows && !reader.info().interlaced {
        let mut frame = PngRows { reader, next: 0 };
        return resample_rows(&mut frame, (width, height), channels, w, h);
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    // The checksum of `IEND` too, as for rows (`Rows::finish`).
    reader.finish().ok()?;
    let frame = Frame {
        data: &buf,
        width: info.width,
        height: info.height,
        stride: info.line_size,
        channels,
    };
    resample(frame, w, h)
}

/// A PNG's bytes with the chunks nothing here uses left out: only the critical ones (`IHDR`,
/// `PLTE`, `IDAT`, `IEND`, and any other the decoder must refuse) and `tRNS` are read. The
/// decoder keeps ancillary chunks it is given (`eXIf` always, and `iCCP`, `tEXt` and the like
/// decompressed to up to its limit): a 100 KB PNG can hold a profile of 100 MB, or an `eXIf` of
/// the file's size, none of it in a decode's cost, none of it drawn. A chunk is skipped whole,
/// its checksum with it; an animated PNG is its default image, as in a browser's first frame.
#[cfg(feature = "images")]
struct PngChunks<'a> {
    /// The chunks not yet looked at.
    rest: &'a [u8],
    /// The part of the chunk being handed out that hasn't been.
    chunk: &'a [u8],
}

#[cfg(feature = "images")]
impl<'a> PngChunks<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        // The signature goes through as it is, like the first chunk (`IHDR`, critical).
        let at = bytes.len().min(8);
        PngChunks {
            rest: &bytes[at..],
            chunk: &bytes[..at],
        }
    }
}

#[cfg(feature = "images")]
impl std::io::BufRead for PngChunks<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        while self.chunk.is_empty() {
            let Some(head) = self.rest.get(..8) else {
                // What's left is no chunk: the decoder reports it.
                self.chunk = std::mem::take(&mut self.rest);
                break;
            };
            let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
            // Length, type, data and checksum; a chunk the data ends in is cut short.
            let end = len.saturating_add(12).min(self.rest.len());
            let (chunk, rest) = self.rest.split_at(end);
            self.rest = rest;
            // The type's first letter is upper case for a critical chunk.
            if head[4] & 0x20 == 0 || &head[4..8] == b"tRNS" {
                self.chunk = chunk;
            }
        }
        Ok(self.chunk)
    }

    fn consume(&mut self, amt: usize) {
        self.chunk = &self.chunk[amt.min(self.chunk.len())..];
    }
}

#[cfg(feature = "images")]
impl std::io::Read for PngChunks<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = {
            let chunk = std::io::BufRead::fill_buf(self)?;
            let n = buf.len().min(chunk.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            n
        };
        std::io::BufRead::consume(self, n);
        Ok(n)
    }
}

/// The decoder only needs this to be there: it never seeks.
#[cfg(feature = "images")]
impl std::io::Seek for PngChunks<'_> {
    fn seek(&mut self, _: std::io::SeekFrom) -> std::io::Result<u64> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// The source rows of an image, as [`resample_rows`] takes them: each at most once, in order.
trait Rows {
    /// Row `y` (`channels` bytes a pixel, at least a row's worth; more is ignored), valid until
    /// the next call, or `None` if it can't be had: the data ended, or `y` is before the row the
    /// source is at.
    fn row(&mut self, y: usize) -> Option<&[u8]>;

    /// Called once the last row is taken: whether what follows it is sound too (a PNG's final
    /// checksum), so a result isn't returned for data the decoder would refuse.
    fn finish(&mut self) -> bool {
        true
    }
}

/// 8-bit pixels in memory: gray, gray + alpha, RGB or RGBA (`channels` 1–4), rows `stride` bytes
/// apart.
struct Frame<'a> {
    data: &'a [u8],
    width: u32,
    height: u32,
    stride: usize,
    channels: usize,
}

impl Rows for Frame<'_> {
    fn row(&mut self, y: usize) -> Option<&[u8]> {
        self.data.get(y * self.stride..)
    }
}

/// A PNG decoded a row at a time, `next` the row the decoder is at.
#[cfg(feature = "images")]
struct PngRows<'a> {
    reader: png::Reader<PngChunks<'a>>,
    next: usize,
}

#[cfg(feature = "images")]
impl Rows for PngRows<'_> {
    fn row(&mut self, y: usize) -> Option<&[u8]> {
        if y < self.next {
            return None;
        }
        // Rows skipped are decoded and dropped; the one asked for is the decoder's own buffer.
        for _ in self.next..y {
            self.reader.next_row().ok()??;
        }
        self.next = y + 1;
        Some(self.reader.next_row().ok()??.data())
    }

    /// Reads through the end of the image data to `IEND`, which checks the checksum of the last
    /// `IDAT` (a row is only as good as the chunk it came in): a PNG cut short, or with a bit
    /// wrong anywhere in its pixel data, is refused whole, as a whole-frame decode refuses it.
    fn finish(&mut self) -> bool {
        self.reader.finish().is_ok()
    }
}

/// [`resample_rows`] for pixels in memory.
fn resample(mut frame: Frame, w: u32, h: u32) -> Option<Bitmap> {
    let size = (frame.width, frame.height);
    let channels = frame.channels;
    resample_rows(&mut frame, size, channels, w, h)
}

/// For each target pixel along one axis, the source pixels under it and their weights, in two
/// flat buffers made once, for the capacity [`spans_cost`] says.
struct Spans {
    /// For target pixel `t`: the first source pixel under it, and where its weights start in
    /// `weights` (they end where the next target pixel's start; one more entry marks the end).
    spans: Vec<(u32, u32)>,
    weights: Vec<f32>,
}

impl Spans {
    fn new(src: u32, dst: u32) -> Spans {
        let f = f64::from(src) / f64::from(dst);
        let mut spans = Vec::with_capacity(dst as usize + 1);
        let mut weights = Vec::with_capacity(spans_weights(src, dst));
        for t in 0..dst {
            let (a, b) = (f64::from(t) * f, f64::from(t + 1) * f);
            let first = a.floor() as usize;
            let last = (b.ceil() as usize).min(src as usize).max(first + 1);
            spans.push((first as u32, weights.len() as u32));
            weights.extend(
                (first..last)
                    .map(|s| ((b.min(s as f64 + 1.0) - a.max(s as f64)) / f).max(0.0) as f32),
            );
        }
        spans.push((src, weights.len() as u32));
        debug_assert!(weights.len() <= spans_weights(src, dst));
        Spans { spans, weights }
    }

    /// Target pixel `t`'s first source pixel and the weights from there on.
    fn get(&self, t: usize) -> (usize, &[f32]) {
        let ((first, from), (_, to)) = (self.spans[t], self.spans[t + 1]);
        (first as usize, &self.weights[from as usize..to as usize])
    }

    fn len(&self) -> usize {
        self.spans.len() - 1
    }
}

/// The weights [`Spans::new`] makes for `src` pixels averaged to `dst`, at most. Target pixel
/// `t` has the source pixels from `⌊t·f⌋` to `⌈(t+1)·f⌉` (`f` = `src / dst`, at least one, at most
/// as far as `src`), so with `⌈x⌉ ≤ ⌊x⌋ + 1` all of them together are at most the sum of
/// `⌊(t+1)·f⌋ − ⌊t·f⌋`, which is `⌊dst·f⌋ ≤ src` (plus rounding of the last, one), and
/// `dst` more. Two more for each target pixel are kept for what `max(first + 1)` may add.
fn spans_weights(src: u32, dst: u32) -> usize {
    src as usize + 2 * dst as usize
}

/// What the span tables of one axis hold (see [`Spans`]).
fn spans_cost(src: u32, dst: u32) -> usize {
    (dst as usize + 1) * size_of::<(u32, u32)>() + spans_weights(src, dst) * size_of::<f32>()
}

/// What [`resample_rows`] holds besides the pixels it makes: the span tables of both axes, and
/// the two rows of four floats a target pixel (`reduced` and `acc`).
fn resample_cost(sw: u32, sh: u32, w: u32, h: u32) -> usize {
    spans_cost(sw, w)
        .saturating_add(spans_cost(sh, h))
        .saturating_add(2 * w as usize * 4 * size_of::<f32>())
}

/// Resample an image of `sw` × `sh` pixels to `w` × `h` by averaging the source area under each
/// target pixel, premultiplied (transparent pixels don't bleed their color). Shrinking averages
/// many pixels; enlarging (a HiDPI window) blends the one or two under each edge. Each source row
/// is taken once, in order, and reduced to the target width at once, then summed into the target
/// row under it (the one row of the next target row it shares is kept): the extra memory is two
/// rows of the target width however wide the source is, and the span tables ([`resample_cost`]).
/// `None` if the source ends early or its end is bad ([`Rows::finish`]).
fn resample_rows(
    rows: &mut impl Rows,
    (sw, sh): (u32, u32),
    channels: usize,
    w: u32,
    h: u32,
) -> Option<Bitmap> {
    let sw_px = sw as usize;
    let (xs, ys) = (Spans::new(sw, w), Spans::new(sh, h));
    // The last source row, reduced to the target width: (its index, premultiplied sums).
    let mut reduced = (usize::MAX, vec![0f32; w as usize * 4]);
    let mut acc = vec![0f32; w as usize * 4];
    let mut data = Vec::with_capacity(w as usize * h as usize);
    for t in 0..ys.len() {
        let (y0, wy) = ys.get(t);
        acc.fill(0.0);
        for (k, &weight) in wy.iter().enumerate() {
            let y = y0 + k;
            if reduced.0 != y {
                let row = rows.row(y)?.get(..sw_px * channels)?;
                reduce_row(row, channels, &xs, &mut reduced.1);
                reduced.0 = y;
            }
            for (a, r) in acc.iter_mut().zip(&reduced.1) {
                *a += r * weight;
            }
        }
        for px in acc.chunks_exact(4) {
            // Color sums carry a factor 255 from the alpha they're premultiplied by.
            let ch = |v: f32, scale: f32| (v / scale).round().clamp(0.0, 255.0) as u32;
            let a = ch(px[3], 1.0);
            let (r, g, b) = (
                ch(px[0], 255.0).min(a),
                ch(px[1], 255.0).min(a),
                ch(px[2], 255.0).min(a),
            );
            data.push(a << 24 | r << 16 | g << 8 | b);
        }
    }
    rows.finish().then_some(Bitmap {
        width: w,
        height: h,
        data,
    })
}

/// One source row (`channels` bytes a pixel) averaged along its width into `out`, four floats
/// (premultiplied r, g, b and alpha) per target pixel, by the `xs` spans.
fn reduce_row(row: &[u8], channels: usize, xs: &Spans, out: &mut [f32]) {
    for (t, px) in out.chunks_exact_mut(4).enumerate() {
        let (x0, wx) = xs.get(t);
        px.fill(0.0);
        for (p, &weight) in row[x0 * channels..].chunks_exact(channels).zip(wx) {
            let (r, g, b, alpha) = match channels {
                1 => (p[0], p[0], p[0], 255),
                2 => (p[0], p[0], p[0], p[1]),
                3 => (p[0], p[1], p[2], 255),
                _ => (p[0], p[1], p[2], p[3]),
            };
            let al = f32::from(alpha) * weight;
            px[0] += f32::from(r) * al;
            px[1] += f32::from(g) * al;
            px[2] += f32::from(b) * al;
            px[3] += al;
        }
    }
}

// ---- The cache ------------------------------------------------------------------------------

// Memory accounting (review rounds 6 to 8): every allocation of the image and remote
// code, who is charged for it, and until when. The rules are that a charge is released only after
// the allocation is freed (or handed to something that is charged for it), never before, that what
// is charged is what is held (the capacity of a buffer, not what is filled), and that nothing
// long-lived is charged for by a guess of its own: every container that outlives an operation is
// charged by its capacity, in one place ([`Images::bookkeeping_bytes`]), which comes out of the
// budget with every decision ([`Cache::held`]: room for a body, eviction, a decode's start), so
// that whatever a container keeps, it pays for with room.
//
// | allocation                          | charged by                                   | from -> to                                              |
// |-------------------------------------|----------------------------------------------|---------------------------------------------------------|
// | fetchers' read buffers (4 x 64 KB)  | `FETCH_RESERVE`, static (`Cache::fetching`)  | set aside while a fetch is queued, running or untaken;  |
// |                                     | (what they hold while held off and none is   | only what is held while they are held off and idle      |
// |                                     | being read: `Remote::reserve_idle`)          |                                                         |
// | body being read                     | pool credit (`Active::held`), 256 KB steps   | first grant -> `Remote::release`, after the cache has it |
// | copy made by `cut_to_size`          | `FETCH_RESERVE` (`FETCHER_SLACK`)            | transient, per fetcher                                   |
// | result untaken: channel message,    | credit (held = capacity); the rest in        | finish -> release; no fetch starts at `PENDING_MAX`      |
// | pool entry (`Active`), `waiting`    | `FETCH_RESERVE` (`PENDING_COST` each, which  |                                                         |
// | (the `Vec` of it)                   | covers 2 x the entries, ~100 B); `waiting`'s |                                                         |
// |                                     | buffer by capacity (bookkeeping)             |                                                         |
// | cached body                         | `Cache::bytes` (`Slot::cost`)                | insert -> removal, and removal only when no job holds    |
// |                                     |                                              | its `Arc` (`Slot::pinned`)                               |
// | `Arc` clones of a body in jobs      | the slot, pinned while they live             | `bytes_of` -> queue replaced or cleared, or the running  |
// |                                     |                                              | job's source dropped before its result is sent           |
// | decode: the decoder's working set,  | `Job::cost` = `FileInfo::cost` / `decode_cost` | request that queues it -> `Worker::taken` after the    |
// | the resampler's tables and rows,    | in `Cache::reserve` (the largest of queued   | result is in the cache; a local file is checked against  |
// | the frame, coefficients, metadata,  | and running, plus replaced workers'); the    | what it was charged for before it is read; one that does |
// | the result, a local file's bytes    | formulas are in `decode_cost`                | not fit beside the fetchers runs with them held off      |
// | bitmap in the channel / the cache   | the job's cost, until taken / `Slot::cost`   | insert -> eviction (paint only borrows it)               |
// | a slot's URL string, `Arc<Vec>`     | `Slot::cost`: `str_bytes` of the URL and     | with the slot                                            |
// | header                              | `SLOT_OVERHEAD`                              |                                                         |
// | `Cache::slots` (the list), `dropped` | bookkeeping: capacity x element size         | `Cache::trim` after every removal; `dropped` is drained  |
// | (URLs of removed slots)             |                                              | by `forget_evicted` at the end of every operation        |
// | `Remote::sizes` (table, strings)    | bookkeeping: `table_bytes` of its capacity   | pruned to the document's URLs and the cached ones at     |
// |                                     | and `sizes_str`, kept as it changes          | `attach`/`forget_remote`, and a URL's record goes when   |
// |                                     |                                              | its last slot is evicted (`forget_evicted`) or a late    |
// |                                     |                                              | body is dropped; `shrink_map` after                      |
// | `Images::deferred`, `urls`, `costs` | bookkeeping: `table_bytes` of the capacity   | replaced with the document; `deferred` pruned with it    |
// | `srcs` (list, URL strings, paths)   | bookkeeping: capacity and `srcs_heap`        | replaced by the next document, the old one freed first   |
// | `in_view`, `retired`, `waiting`     | bookkeeping: capacity                        | `shrink_vec` after what clears or prunes them            |
// | worker replaced by `set_wake`       | its decode in `retired_cost`; its queue and  | until its running job's result is taken                  |
// |                                     | `WORKER_FIXED` in bookkeeping                |                                                         |
// | the worker's queue (jobs, paths)    | bookkeeping: capacity, paths, `WORKER_FIXED` | replaced by the next request (the old one is freed after |
// |                                     |                                              | the new one is made: the request's allowance)            |
// | the pool's queue of URLs            | bookkeeping: capacity and `urls_str`         | replaced by the next request; `stop` and drop give the   |
// |                                     |                                              | buffer back (a `clear` kept 1.6 MB for 100,000 URLs)     |
// | pool's `active`, results in the     | `PENDING_COST` per entry (at most            | `Pool::done`; the list is cut to size when it empties    |
// | channel                             | `FETCHERS + PENDING_MAX` of them)            |                                                         |
// | what a request builds: the lists of | the allowance of the request: `REQUEST_PER_  | replaced by the next request                             |
// | URLs and the set that removes the   | REMOTE` for each remote image of the         |                                                         |
// | duplicates, jobs and their paths    | document, 2 x `Job` for each box, the paths  |                                                         |
// | `Cache::fetching` while held off    | `Remote::reserve_idle`: the bodies not       | `sync_pause`, and set back to the static reserve, and the |
// | and idle                            | released and their bookkeeping               | room made for it, before they go on                      |
//
// What is not charged: the page's own memory (the `Doc` and `Layout`, with each image's source and
// size in them, as for text), the `boxes` a caller passes to a request (the images within a screen
// of the viewport: the window's frame), the threads' stacks (not the allocator's), and the
// allocator's rounding beyond what each row allows for. `tests/image_memory.rs` runs random mixes
// of everything above and holds the live bytes of all of it to the budget and 256 KiB (`SLACK`
// there) for what is no image's: the page and the test's own.
//
// Fetchers and decodes against the budget: the fetchers are set aside `FETCH_RESERVE` whatever
// they hold (so they never have to ask for room), which leaves room for a decode of about 18 MiB
// beside them. A decode that doesn't fit beside it (a job: `request`; a body in view: `admit_alone`)
// runs with the fetchers held off (`Remote::pause`: nothing starts, nothing takes more credit, a
// transfer that needs more gives up and is queued again, one that has what it needs is waited
// for), when they hold nothing but the bodies that wait (`Images::alone`), and until its result is
// taken (`Images::exclusive`); then the budget holds beside it whatever it is, and a decode bigger
// than the cache goes over it by its own size, and nothing else. See `Images::sync_pause`.
//
// Only the main thread clones a body, so an `Arc` count of 1 can't go up: a slot is evictable
// exactly when it is unique.

/// What a cached image belongs to: an image of the current document (by id), or a remote URL,
/// which stays valid across documents for the session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Local(u32),
    Remote(Arc<str>),
}

enum Data {
    Pixels(Bitmap),
    /// A fetched remote image's bytes, to decode at whatever size its box needs.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    Bytes(Bytes),
}

struct Slot {
    key: Key,
    data: Data,
    /// The frame it was last painted (or stored) in.
    used: u64,
}

/// What a slot costs besides its buffer and its URL's string: the `Arc` and `Vec` headers of a
/// body and the allocator's rounding of the buffer, at most. (Its place in the list of slots, and
/// its entry in the table of sizes, are charged by capacity, with the rest of the bookkeeping:
/// [`Images::bookkeeping_bytes`].)
const SLOT_OVERHEAD: usize = 64;

/// What the string of an `Arc<str>` holds besides its bytes: the counts, and the allocator's
/// rounding, at most.
pub(crate) const STR_OVERHEAD: usize = 48;

/// What a string held by an `Arc<str>` holds.
pub(crate) fn str_bytes(s: &str) -> usize {
    s.len() + STR_OVERHEAD
}

impl Slot {
    /// What it holds: the buffer's allocated size (a `Vec` may hold more than it has filled, and
    /// all of it is memory), and [`SLOT_OVERHEAD`].
    fn cost(&self) -> usize {
        let buffer = match &self.data {
            Data::Pixels(b) => b.data.capacity() * 4,
            Data::Bytes(b) => b.capacity(),
        };
        let key = match &self.key {
            Key::Local(_) => 0,
            Key::Remote(url) => str_bytes(url),
        };
        buffer + SLOT_OVERHEAD + key
    }

    /// What a fetched body for `url` costs once it is a slot.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn bytes_cost(url: &str, bytes: &Bytes) -> usize {
        bytes.capacity() + SLOT_OVERHEAD + str_bytes(url)
    }

    fn pixels(&self) -> Option<&Bitmap> {
        match &self.data {
            Data::Pixels(b) => Some(b),
            Data::Bytes(_) => None,
        }
    }

    /// Whether a job of the worker, queued or running, holds this slot's bytes (an `Arc` clone):
    /// they stay allocated until the job lets go of them, whatever happens to the slot, so the
    /// slot keeps its charge, and isn't evicted or replaced, until then. Only the main thread
    /// clones a body, so a count of 1 can't go up again: the check can only be too careful.
    fn pinned(&self) -> bool {
        matches!(&self.data, Data::Bytes(b) if Arc::strong_count(b) > 1)
    }
}

/// Decoded images and fetched bytes under one memory budget, least recently painted first out.
struct Cache {
    slots: Vec<Slot>,
    bytes: usize,
    cap: usize,
    /// What the worker's decodes hold while they run, at most: it comes out of the budget.
    reserve: usize,
    /// What is set aside for the fetchers while any fetch is queued or running: all of
    /// `FETCH_RESERVE` (the most the fetchers can hold, whatever they hold now), or nothing. It
    /// comes out of the budget too, so the fetchers need no more than what they are given and
    /// the cache is made small before they start, not as they grow.
    fetching: usize,
    /// What the bookkeeping of the image code holds (the document's tables, the queues, the
    /// per-URL records, the slot list itself: [`Images::bookkeeping_bytes`]), which comes out of
    /// the budget too, so that whatever a container keeps is paid for by the room it takes.
    overhead: usize,
    /// The URLs of the remote slots removed since [`Images::forget_evicted`] last looked: what is
    /// kept of a URL that has no slot left, and isn't the document's, is forgotten.
    dropped: Vec<Arc<str>>,
    frame: u64,
}

impl Cache {
    fn new(cap: usize) -> Cache {
        Cache {
            slots: Vec::new(),
            bytes: 0,
            cap,
            reserve: 0,
            fetching: 0,
            overhead: 0,
            dropped: Vec::new(),
            frame: 0,
        }
    }

    /// What is set aside beside the slots: decoding, the fetchers, the bookkeeping.
    fn held(&self) -> usize {
        self.reserve
            .saturating_add(self.fetching)
            .saturating_add(self.overhead)
    }

    /// What the slot list holds, and the list of URLs waiting to be forgotten: the capacity of
    /// each buffer.
    fn bookkeeping(&self) -> usize {
        vec_bytes(&self.slots).saturating_add(vec_bytes(&self.dropped))
    }

    /// Whether any slot is for the remote image at `url`.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn has_url(&self, url: &Arc<str>) -> bool {
        self.slots
            .iter()
            .any(|s| matches!(&s.key, Key::Remote(u) if u == url))
    }

    /// Take the slot at `i` out: its charge goes, and its URL is noted.
    fn remove(&mut self, i: usize) {
        let s = self.slots.swap_remove(i);
        self.bytes -= s.cost();
        if let Key::Remote(url) = s.key {
            self.dropped.push(url);
        }
    }

    /// What stays however long things are waited for: the pixels painted in this frame. (The
    /// bodies waiting for their decodes, or held by them, are not here: each lets go of its body
    /// when its decode is done, and the decodes run one after another; [`Cache::would_fit`]
    /// counts them.)
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn kept(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.used >= self.frame && s.pixels().is_some())
            .map(Slot::cost)
            .sum()
    }

    fn has(&self, key: &Key, w: u32, h: u32) -> bool {
        self.slots
            .iter()
            .any(|s| s.key == *key && s.pixels().is_some_and(|b| (b.width, b.height) == (w, h)))
    }

    /// Mark the pixels for `key` at exactly `w` × `h`, if cached, as painted this frame.
    fn touch(&mut self, key: &Key, w: u32, h: u32) {
        let frame = self.frame;
        for s in &mut self.slots {
            if s.key == *key && s.pixels().is_some_and(|b| (b.width, b.height) == (w, h)) {
                s.used = frame;
            }
        }
    }

    /// The pixels for `key` at `w` × `h`, or at another size while that one is decoded (a zoom
    /// step); marked as painted this frame.
    fn pixels(&mut self, key: &Key, w: u32, h: u32) -> Option<&Bitmap> {
        let mut best: Option<usize> = None;
        for (i, s) in self.slots.iter().enumerate() {
            let Some(b) = s.pixels().filter(|_| s.key == *key) else {
                continue;
            };
            if (b.width, b.height) == (w, h) {
                best = Some(i);
                break;
            }
            best.get_or_insert(i);
        }
        let slot = &mut self.slots[best?];
        slot.used = self.frame;
        slot.pixels()
    }

    /// A URL's bytes without marking them used: looking at an image that won't be decoded now
    /// mustn't keep its bytes from being evicted.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn peek_bytes(&self, url: &Arc<str>) -> Option<&Bytes> {
        self.slots.iter().find_map(|s| match &s.data {
            Data::Bytes(b) if s.key == Key::Remote(url.clone()) => Some(b),
            _ => None,
        })
    }

    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn bytes_of(&mut self, url: &Arc<str>) -> Option<Bytes> {
        let frame = self.frame;
        self.slots.iter_mut().find_map(|s| match &s.data {
            Data::Bytes(b) if s.key == Key::Remote(url.clone()) => {
                s.used = frame;
                Some(b.clone())
            }
            _ => None,
        })
    }

    fn insert(&mut self, key: Key, data: Data) {
        // One slot per image and size (and one for a URL's bytes).
        let same = |s: &Slot| {
            s.key == key
                && match (&s.data, &data) {
                    (Data::Pixels(a), Data::Pixels(b)) => {
                        (a.width, a.height) == (b.width, b.height)
                    }
                    (Data::Bytes(_), Data::Bytes(_)) => true,
                    _ => false,
                }
        };
        // A body a job of the worker holds can't be replaced (its charge would go while it is
        // allocated): it is the same URL's, and so the same body, and the new one is let go.
        if matches!(data, Data::Bytes(_)) && self.slots.iter().any(|s| same(s) && s.pinned()) {
            return;
        }
        self.retain(|s| !same(s));
        // Pixels come in for a box that is wanted (and are as good as painted this frame). A
        // fetched body counts as used when something is made of it ([`Cache::bytes_of`]), not
        // before: it may belong to an image far from the viewport, and mustn't push out
        // what is being looked at.
        let used = match data {
            Data::Pixels(_) => self.frame,
            Data::Bytes(_) => 0,
        };
        let slot = Slot { key, data, used };
        // Room is made before the slot is in, so it can't be the one evicted.
        self.evict_for(slot.cost());
        self.bytes += slot.cost();
        self.slots.push(slot);
    }

    fn evict(&mut self) {
        self.evict_for(0);
    }

    /// Drop the least recently painted slots until the budget holds beside what decoding needs
    /// ([`Cache::reserve`]), what is set aside for the fetchers ([`Cache::fetching`]) and
    /// `extra` more bytes, never one painted in the current frame, nor one whose bytes a decode
    /// holds ([`Slot::pinned`]): a pinned slot is charged until its job is done with it.
    fn evict_for(&mut self, extra: usize) {
        let held = self.held().saturating_add(extra);
        while self.bytes.saturating_add(held) > self.cap {
            let Some((i, _)) = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.used < self.frame && !s.pinned())
                .min_by_key(|(_, s)| s.used)
            else {
                break;
            };
            self.remove(i);
        }
        self.trim();
    }

    /// Make room for a body of `cost` that will be decoded for `decode` more than what is kept
    /// free for decoding already, evicting what was not painted this frame, and say whether it
    /// fits. (A decode's memory is not held at the same time as another's: the larger counts.)
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn room_for(&mut self, cost: usize, decode: usize, fetching: usize) -> bool {
        // What the fetchers may take counts whole, whatever they hold now (see `admission`).
        let now = std::mem::replace(&mut self.fetching, fetching);
        let extra = cost.saturating_add(decode.saturating_sub(self.reserve));
        self.evict_for(extra);
        let fits = self.bytes.saturating_add(self.held()).saturating_add(extra) <= self.cap;
        self.fetching = now;
        fits
    }

    /// What [`Cache::room_for`] would say if `reserve` and `fetching` were what is set aside,
    /// without evicting anything: whatever was painted this frame, and the bodies decodes hold,
    /// is all that can't go, so the body fits exactly when it fits beside that.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn would_fit(&self, cost: usize, decode: usize, reserve: usize, fetching: usize) -> bool {
        let kept: usize = self
            .slots
            .iter()
            .filter(|s| s.used >= self.frame || s.pinned())
            .map(Slot::cost)
            .sum();
        self.fits(kept, cost, decode, reserve, fetching)
    }

    /// [`Cache::would_fit`] with `kept` ([`Cache::kept`]) already known.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn fits(
        &self,
        kept: usize,
        cost: usize,
        decode: usize,
        reserve: usize,
        fetching: usize,
    ) -> bool {
        kept.saturating_add(self.overhead)
            .saturating_add(reserve)
            .saturating_add(fetching)
            .saturating_add(cost)
            .saturating_add(decode.saturating_sub(reserve))
            <= self.cap
    }

    /// Whether a fetched body is held that was used this frame, which can't be evicted.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn holds_used_bytes(&self) -> bool {
        self.slots
            .iter()
            .any(|s| matches!(s.data, Data::Bytes(_)) && s.used >= self.frame)
    }

    /// What the bodies held by jobs (queued or running) cost: they stay while the jobs do.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn pinned_cost(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.pinned())
            .map(Slot::cost)
            .sum()
    }

    /// [`Cache::holds_used_bytes`], not counting the body of the image at `url`.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn holds_used_bytes_but(&self, url: Option<&Arc<str>>) -> bool {
        self.slots.iter().any(|s| {
            matches!(s.data, Data::Bytes(_))
                && s.used >= self.frame
                && !matches!((&s.key, url), (Key::Remote(u), Some(url)) if u == url)
        })
    }

    /// The decoded image for `url` has come: its fetched body is of no use until another size is
    /// wanted, and is the first to go (it was marked used for the frame it was decoded in).
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn release_bytes(&mut self, url: &Arc<str>) {
        for s in &mut self.slots {
            if matches!(s.data, Data::Bytes(_)) && matches!(&s.key, Key::Remote(u) if u == url) {
                s.used = 0;
            }
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(&Slot) -> bool) {
        let (bytes, dropped) = (&mut self.bytes, &mut self.dropped);
        self.slots.retain(|s| {
            let k = keep(s);
            if !k {
                debug_assert!(!s.pinned(), "a slot a decode holds was dropped");
                *bytes -= s.cost();
                if let Key::Remote(url) = &s.key {
                    dropped.push(url.clone());
                }
            }
            k
        });
        self.trim();
    }

    /// A `Vec` keeps its capacity when it loses elements: the list of slots is cut to size once
    /// it has twice what it needs and a little more, so what a slot costs ([`SLOT_OVERHEAD`]
    /// covers the room for as many again) is all it can hold, and not what it once did (100,000
    /// one-pixel images were 7 MB of empty list after a new document had dropped them, beside
    /// a cache that counted none of it).
    fn trim(&mut self) {
        shrink_vec(&mut self.slots);
    }

    /// The fetched body for `url` didn't decode: it is of no use, and is dropped, so that
    /// opening the document again fetches the image again. (One a decode still holds stays, first
    /// to go: the decode's failure comes here again.)
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn discard_bytes(&mut self, url: &Arc<str>) {
        let Some(i) = self.slots.iter().position(|s| {
            matches!(s.data, Data::Bytes(_)) && matches!(&s.key, Key::Remote(u) if u == url)
        }) else {
            return;
        };
        if self.slots[i].pinned() {
            self.slots[i].used = 0;
            return;
        }
        self.remove(i);
        self.trim();
    }
}

/// Cut a `Vec` to the room it needs if it has twice that and 16 elements more (see
/// [`Cache::trim`]): what a container left from removals is held until it grows again, so
/// every long-lived one of the image code is trimmed after what removes from it.
pub(crate) fn shrink_vec<T>(v: &mut Vec<T>) {
    if v.capacity() > 2 * v.len() + 16 {
        v.shrink_to(v.len());
    }
}

/// What a `Vec`'s buffer holds: its capacity, not what is in it.
pub(crate) fn vec_bytes<T>(v: &Vec<T>) -> usize {
    v.capacity().saturating_mul(size_of::<T>())
}

/// What a hash table that says it has room for `capacity` entries of `entry` bytes holds (it has
/// the next power of two of eight sevenths of that many buckets, a control byte each, and 16
/// more; and 32 bytes for alignment and the allocator's rounding).
pub(crate) fn table_bytes(capacity: usize, entry: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    let buckets = (capacity.saturating_mul(8)).div_ceil(7).next_power_of_two();
    buckets.saturating_mul(entry + 1).saturating_add(16 + 32)
}

/// [`shrink_vec`] for a map: [`Remote::sizes`] and [`Images::deferred`] are pruned to the
/// document's URLs, which leaves their tables as big as they were.
#[cfg(feature = "remote-images")]
pub(crate) fn shrink_map<K: Eq + std::hash::Hash, V>(m: &mut std::collections::HashMap<K, V>) {
    if m.capacity() > 2 * m.len() + 16 {
        m.shrink_to_fit();
    }
}

// ---- The decode worker ------------------------------------------------------------------

pub(crate) enum Source {
    File(PathBuf),
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    Bytes(Bytes),
}

/// Decode `source` to `w` × `h` for `key` (of document generation `doc_gen`).
pub(crate) struct Job {
    doc_gen: u64,
    key: Key,
    source: Source,
    w: u32,
    h: u32,
    /// What it holds while it runs, its result included: see [`decode_cost`].
    cost: usize,
}

/// What the worker threads send back.
pub(crate) enum Done {
    Decoded {
        doc_gen: u64,
        key: Key,
        bitmap: Option<Bitmap>,
        /// The worker that made it, which waits until the result is taken ([`Worker::taken`]).
        worker: Shared,
    },
    /// A remote image was fetched: its bytes and natural size, or why not.
    #[cfg(feature = "remote-images")]
    Fetched {
        url: Arc<str>,
        result: crate::remote::Fetched,
    },
}

/// The worker's queue and its condition variable, shared with the main thread.
pub(crate) type Shared = Arc<(Mutex<Queue>, Condvar)>;

#[derive(Default)]
pub(crate) struct Queue {
    jobs: Vec<Job>,
    /// The job being decoded, and then its result until the main thread takes it
    /// ([`Worker::taken`]): (key, size, cost). The worker starts no other meanwhile, so what is
    /// held beside the cache is one job's cost (result included) from its start until its result
    /// is in the cache, and not queued again while it is here.
    busy: Option<(Key, u32, u32, usize)>,
    quit: bool,
}

/// What a worker holds besides its jobs: its queue and condition variable, the thread's own
/// bookkeeping (its stack is not the allocator's).
const WORKER_FIXED: usize = 512;

/// One thread decoding the most wanted image first; the queue is replaced as the viewport
/// moves, so images scrolled past are never decoded.
struct Worker {
    shared: Shared,
}

impl Worker {
    fn spawn(tx: Sender<Done>, wake: Wake) -> Worker {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let theirs = shared.clone();
        std::thread::spawn(move || {
            let (lock, cv) = &*theirs;
            loop {
                let job = {
                    let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                    // The last result still waits to be taken: its memory isn't free yet.
                    while (q.jobs.is_empty() || q.busy.is_some()) && !q.quit {
                        q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                    if q.quit {
                        return;
                    }
                    let job = q.jobs.remove(0);
                    q.busy = Some((job.key.clone(), job.w, job.h, job.cost));
                    job
                };
                let bitmap = decode_job(&job);
                // The job's hold on a body (an `Arc` clone) ends before the result is sent: once
                // the main thread has the result the body's slot may be evicted, and its charge
                // with it ([`Slot::pinned`]).
                let Job {
                    doc_gen,
                    key,
                    source,
                    ..
                } = job;
                drop(source);
                let done = Done::Decoded {
                    doc_gen,
                    key,
                    bitmap,
                    worker: theirs.clone(),
                };
                if tx.send(done).is_err() {
                    return;
                }
                wake();
            }
        });
        Worker { shared }
    }

    /// Replace the queue (most wanted first), skipping the job being decoded.
    fn set(&self, mut jobs: Vec<Job>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((key, w, h, _)) = &q.busy {
            jobs.retain(|j| (&j.key, j.w, j.h) != (key, *w, *h));
        }
        q.jobs = jobs;
        cv.notify_one();
    }

    /// The main thread took the result of the job the worker `shared` ran (it is in the cache,
    /// or dropped): that worker may go on to the next. It goes by the result, not by the current
    /// worker, which may be another one ([`Images::set_wake`]).
    fn taken(shared: &Shared) {
        let (lock, cv) = &**shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).busy = None;
        cv.notify_one();
    }

    /// Whether the worker has a job for `key`, queued, being decoded, or decoded and not taken.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn has(&self, key: &Key) -> bool {
        let (lock, _) = &*self.shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.jobs.iter().any(|j| j.key == *key) || q.busy.as_ref().is_some_and(|b| b.0 == *key)
    }

    /// What the queue of the worker `shared` holds besides the jobs' bodies (the cache's): the
    /// buffer, as allocated, the path each job copied, and [`WORKER_FIXED`].
    fn bookkeeping_of(shared: &Shared) -> usize {
        let (lock, _) = &**shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let paths: usize = q
            .jobs
            .iter()
            .map(|j| match &j.source {
                Source::File(path) => path.as_os_str().len() + STR_OVERHEAD,
                Source::Bytes(_) => 0,
            })
            .sum();
        WORKER_FIXED
            .saturating_add(vec_bytes(&q.jobs))
            .saturating_add(paths)
    }

    /// Whether nothing is queued, being decoded, or decoded and not taken.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn idle(&self) -> bool {
        Worker::idle_of(&self.shared)
    }

    fn idle_of(shared: &Shared) -> bool {
        let (lock, _) = &**shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.jobs.is_empty() && q.busy.is_none()
    }

    /// What the largest job of this worker holds, queued, being decoded, or decoded and not taken.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn pending_cost(&self) -> usize {
        let (lock, _) = &*self.shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let queued = q.jobs.iter().map(|j| j.cost).max().unwrap_or(0);
        queued.max(q.busy.as_ref().map_or(0, |b| b.3))
    }

    /// What the job being decoded (or whose result isn't taken) holds, if any.
    fn busy_cost(&self) -> usize {
        Worker::busy_cost_of(&self.shared)
    }

    fn busy_cost_of(shared: &Shared) -> usize {
        let (lock, _) = &**shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.busy.as_ref().map_or(0, |b| b.3)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.quit = true;
        // What was queued will not run: its hold on bodies ends now, not when the thread is gone.
        q.jobs = Vec::new();
        cv.notify_one();
    }
}

#[cfg(feature = "images")]
fn decode_job(job: &Job) -> Option<Bitmap> {
    match &job.source {
        Source::File(path) => {
            // What the job is charged for is what the file was when its header was read: a file
            // that is bigger now, or has another header, is refused (it shows its alt text)
            // instead of decoded with memory nothing counted.
            let held = job.cost.saturating_sub(job.w as usize * job.h as usize * 4);
            let mut bytes = read_file(path, held)?;
            if bytes
                .len()
                .saturating_add(bytes_cost(&bytes, job.w, job.h)?)
                > job.cost
            {
                return None;
            }
            // The bytes are ours: a JPEG's metadata is cut out of them where they are.
            if !shed_jpeg_metadata(&mut bytes) {
                return None;
            }
            decode(&bytes, job.w, job.h)
        }
        Source::Bytes(bytes) => decode(bytes, job.w, job.h),
    }
}

#[cfg(not(feature = "images"))]
fn decode_job(_: &Job) -> Option<Bitmap> {
    None
}

// ---- A document's images ----------------------------------------------------------------

/// Where one of the document's images comes from.
#[derive(Clone, Debug)]
enum Src {
    /// Nothing loadable (not found, failed to decode, not PNG or JPEG, a `data:` URL, ...).
    None,
    File(PathBuf),
    Remote(Arc<str>),
}

/// What has been loaded so far (the RSS examples and tests check that the images they measure
/// were).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Images decoded and in the cache (a size of an image is one).
    pub decoded: usize,
    /// Images decoded since the start, those evicted since too.
    pub decodes: usize,
    /// Images that couldn't be loaded or decoded: they show their alt text.
    pub failed: usize,
    /// Remote images fetched (their size is known).
    pub fetched: usize,
    /// Fetched bodies held for room in the cache.
    pub waiting: usize,
}

/// What [`Images::admission`] says of a fetched body.
#[cfg(feature = "remote-images")]
enum Admit {
    /// Into the cache.
    Yes,
    /// Not kept: there is no room, and its image isn't in view.
    Drop,
    /// Held by the fetchers until there is room: its image is in view.
    Wait,
}

/// The current document's images: where each comes from, the cache, and the threads that
/// decode (and, with `remote-images`, fetch) them.
pub struct Images {
    /// The document's directory, which relative paths resolve against.
    base: Option<PathBuf>,
    /// Bumped for every document: results for an older one are dropped.
    doc_gen: u64,
    srcs: Vec<Src>,
    /// What the strings of `srcs` hold ([`str_bytes`] of each URL), and what the paths of its
    /// files hold (a job copies one): kept from when `srcs` is made.
    srcs_heap: usize,
    paths_heap: usize,
    /// What each local image is (its file's size and header, for what decoding it holds: see
    /// [`decode_cost`]), by id, once its header is read: `None` if it can't be. Only the images
    /// that were asked for have an entry.
    costs: std::collections::HashMap<u32, Option<FileInfo>>,
    cache: Cache,
    /// Results with pixels taken so far ([`Stats::decodes`]).
    decodes: usize,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    wake: Wake,
    worker: Option<Worker>,
    /// Workers replaced by [`Images::set_wake`] that may still be decoding (or hold a result the
    /// main thread hasn't taken): what they hold is counted until they are idle.
    retired: Vec<Shared>,
    /// The remote images in view when images were last asked for, with their boxes: a body that
    /// arrives for one is kept while the cache has room for it (and is decoded next), any other
    /// may be evicted at once.
    #[cfg(feature = "remote-images")]
    in_view: Vec<(Arc<str>, u32, u32)>,
    /// Bodies fetched for images in view that the cache had no room for ([`Images::admission`]):
    /// still held by the fetchers' credit, so they can't pile up while the decoder is behind.
    #[cfg(feature = "remote-images")]
    waiting: Vec<(Arc<str>, crate::remote::Fetched)>,
    /// The URLs of the current document's remote images: what is kept per URL beyond it is
    /// pruned ([`Images::attach`]).
    #[cfg(feature = "remote-images")]
    urls: std::collections::HashSet<Arc<str>>,
    /// Bodies dropped for lack of room (their images weren't in view), by URL, for the current
    /// document's URLs only: what keeping each would have cost in the cache and what decoding it
    /// needs. Such a URL is fetched again only when its image is in view or there would be room
    /// for it now ([`Cache::would_fit`]), not just because it is wanted: see [`Images::request`].
    /// Repainting doesn't touch it (that would bring the refetch loop back); another document
    /// replaces it ([`Images::attach`]).
    #[cfg(feature = "remote-images")]
    deferred: std::collections::HashMap<Arc<str>, (usize, usize)>,
    /// How many images of the document are remote: what a request builds per one of them is in
    /// [`REQUEST_PER_REMOTE`].
    #[cfg(feature = "remote-images")]
    remote_images: usize,
    /// A decode that doesn't fit beside what the fetchers may hold runs with them held off
    /// ([`Images::alone`]): its key, from when its body is let in or its job is handed to the
    /// worker until its result is taken.
    #[cfg(feature = "remote-images")]
    exclusive: Option<Key>,
    /// `exclusive` is a body just let in, whose job hasn't been asked for yet.
    #[cfg(feature = "remote-images")]
    awaiting: bool,
    /// The last request had an image in view to decode that doesn't fit beside the fetchers.
    #[cfg(feature = "remote-images")]
    heavy_wanted: bool,
    #[cfg(feature = "remote-images")]
    pub(crate) remote: crate::remote::Remote,
}

/// The URL of a remote key (the empty string for a local one).
#[cfg(feature = "remote-images")]
fn key_url(key: &Key) -> &str {
    match key {
        Key::Remote(url) => url,
        Key::Local(_) => "",
    }
}

/// What a request builds per remote image of the document, besides what stays: the list of URLs
/// to fetch (it doubles as it grows), the list of those dropped for room, and the set that takes
/// the duplicates out (see `Remote::fetch`), and the old queue as the new one replaces it.
#[cfg(feature = "remote-images")]
const REQUEST_PER_REMOTE: usize = 128;

impl Default for Images {
    fn default() -> Self {
        let (tx, rx) = channel();
        Images {
            base: None,
            doc_gen: 0,
            srcs: Vec::new(),
            srcs_heap: 0,
            paths_heap: 0,
            costs: std::collections::HashMap::new(),
            cache: Cache::new(CACHE_BYTES),
            decodes: 0,
            tx,
            rx,
            wake: Arc::new(|| {}),
            worker: None,
            retired: Vec::new(),
            #[cfg(feature = "remote-images")]
            in_view: Vec::new(),
            #[cfg(feature = "remote-images")]
            waiting: Vec::new(),
            #[cfg(feature = "remote-images")]
            urls: std::collections::HashSet::new(),
            #[cfg(feature = "remote-images")]
            deferred: std::collections::HashMap::new(),
            #[cfg(feature = "remote-images")]
            remote_images: 0,
            #[cfg(feature = "remote-images")]
            exclusive: None,
            #[cfg(feature = "remote-images")]
            awaiting: false,
            #[cfg(feature = "remote-images")]
            heavy_wanted: false,
            #[cfg(feature = "remote-images")]
            remote: crate::remote::Remote::default(),
        }
    }
}

impl Images {
    pub fn set_wake(&mut self, wake: Wake) {
        // The decoder thread keeps the old one: it is started again with the new one. It may be
        // decoding: what it holds stays counted until its result is taken.
        if let Some(old) = self.worker.take() {
            self.retired.push(old.shared.clone());
        }
        #[cfg(feature = "remote-images")]
        self.remote.set_wake(wake.clone());
        self.wake = wake;
    }

    /// Fetch remote images from `body` instead of `curl` (see [`Remote::set_test_body`]).
    #[cfg(feature = "remote-images")]
    #[doc(hidden)]
    pub fn set_test_body(&mut self, body: crate::remote::TestBody) {
        self.remote.set_test_body(body);
    }

    pub fn set_base(&mut self, base: Option<PathBuf>) {
        self.base = base;
    }

    /// Start on a document (opened or reloaded) and its new layout: local images are decoded
    /// again (the files may have changed); remote ones stay cached by URL.
    pub fn attach(&mut self, doc: &Doc, layout: &mut Layout) {
        self.doc_gen += 1;
        self.cache.retain(|s| matches!(s.key, Key::Remote(_)));
        // The last document's tables go before the new one's are made: they are never both held.
        self.srcs = Vec::new();
        self.costs = std::collections::HashMap::new();
        #[cfg(feature = "remote-images")]
        {
            self.urls = std::collections::HashSet::new();
        }
        self.srcs = doc
            .images
            .iter()
            .map(|img| {
                if img.remote {
                    Src::Remote(img.src.as_str().into())
                } else if cfg!(feature = "images") {
                    local_path(&img.src, self.base.as_deref()).map_or(Src::None, Src::File)
                } else {
                    Src::None
                }
            })
            .collect();
        self.srcs_heap = 0;
        self.paths_heap = 0;
        for src in &self.srcs {
            match src {
                Src::File(path) => {
                    self.paths_heap += path.as_os_str().len() + STR_OVERHEAD;
                    self.srcs_heap += path.as_os_str().len() + STR_OVERHEAD;
                }
                Src::Remote(url) => self.srcs_heap += str_bytes(url),
                Src::None => {}
            }
        }
        if let Some(w) = &self.worker {
            w.set(Vec::new());
        }
        if cfg!(feature = "images") {
            let base = self.base.clone();
            layout.set_image_probe(
                doc,
                Some(Box::new(move |img: &ImageRef| probe(img, base.as_deref()))),
            );
        }
        #[cfg(feature = "remote-images")]
        {
            // Nothing is in view of the new document yet: the bodies that waited are let go.
            self.in_view.clear();
            self.urls = self
                .srcs
                .iter()
                .filter_map(|s| match s {
                    Src::Remote(url) => Some(url.clone()),
                    _ => None,
                })
                .collect();
            self.remote_images = self.urls.len();
            // Nothing is wanted of the new document yet: the first request says.
            self.heavy_wanted = false;
            self.admit_waiting();
            self.prune_urls();
            self.sync_remote(doc, layout);
            self.settle_exclusive();
            self.sync_pause(false);
        }
        self.charge_bookkeeping(0);
    }

    /// Where the budget is: what the cache holds and what is set aside beside it (the memory
    /// tests print this when they fail).
    #[doc(hidden)]
    pub fn memory_report(&self) -> String {
        let c = &self.cache;
        #[cfg(feature = "remote-images")]
        let remote = format!(
            "paused {}, exclusive {:?}, held {}, waiting {}, in flight {:?}",
            self.remote.paused(),
            self.exclusive,
            self.remote.held_bytes(),
            self.waiting.len(),
            self.remote.transfers(),
        );
        #[cfg(not(feature = "remote-images"))]
        let remote = String::new();
        let slots: Vec<String> = c
            .slots
            .iter()
            .map(|s| {
                format!(
                    "{}{}{} {}",
                    match &s.data {
                        Data::Pixels(_) => "pixels",
                        Data::Bytes(_) => "bytes",
                    },
                    if s.used >= c.frame { " used" } else { "" },
                    if s.pinned() { " pinned" } else { "" },
                    s.cost()
                )
            })
            .collect();
        format!(
            "cache {} in {} slots ({}), decode {}, fetchers {}, bookkeeping {}, retired {}; {remote}",
            c.bytes,
            c.slots.len(),
            slots.join(", "),
            c.reserve,
            c.fetching,
            c.overhead,
            self.retired.len(),
        )
    }

    /// Bytes of fetched bodies the fetchers hold (being read, or fetched and not taken yet): the
    /// tests that wait for transfers to have read what they will.
    #[cfg(feature = "remote-images")]
    #[doc(hidden)]
    pub fn fetch_held(&self) -> usize {
        self.remote.held_bytes()
    }

    /// Whether nothing is queued, being fetched or decoded, or waiting for room: what is wanted
    /// has been done (the tests that look for a state nothing leaves).
    #[doc(hidden)]
    pub fn is_idle(&self) -> bool {
        let worker = self.worker.as_ref().is_none_or(Worker::idle);
        #[cfg(feature = "remote-images")]
        let remote = !self.remote.busy() && self.waiting.is_empty();
        #[cfg(not(feature = "remote-images"))]
        let remote = true;
        worker && remote && self.retired.iter().all(Worker::idle_of)
    }

    /// What has been loaded (see [`Stats`]).
    pub fn stats(&self) -> Stats {
        let decoded = self
            .cache
            .slots
            .iter()
            .filter(|s| s.pixels().is_some())
            .count();
        #[cfg(feature = "remote-images")]
        let (fetched, failed, waiting) = {
            let (fetched, failed) = self.remote.counts();
            let local = self.srcs.iter().filter(|s| matches!(s, Src::None)).count();
            (fetched, failed + local, self.waiting.len())
        };
        #[cfg(not(feature = "remote-images"))]
        let (fetched, failed, waiting) = (
            0,
            self.srcs.iter().filter(|s| matches!(s, Src::None)).count(),
            0,
        );
        Stats {
            decoded,
            decodes: self.decodes,
            failed,
            fetched,
            waiting,
        }
    }

    /// The directory relative paths resolve against (tests).
    #[cfg(all(test, feature = "embed-fonts"))]
    pub(crate) fn base(&self) -> Option<&Path> {
        self.base.as_deref()
    }

    /// How many documents were attached (tests).
    #[cfg(all(test, feature = "embed-fonts"))]
    pub(crate) fn generation(&self) -> u64 {
        self.doc_gen
    }

    /// The source of image `id` (1-based).
    fn src(&self, id: u32) -> &Src {
        id.checked_sub(1)
            .and_then(|i| self.srcs.get(i as usize))
            .unwrap_or(&Src::None)
    }

    fn key(&self, id: u32) -> Option<Key> {
        match self.src(id) {
            Src::None => None,
            Src::File(_) => Some(Key::Local(id)),
            Src::Remote(url) => Some(Key::Remote(url.clone())),
        }
    }

    /// What decoding image `id` to `w` × `h` holds while it runs (see [`decode_cost`]), the bytes
    /// of a local file and the result included; the bytes of a remote image are the cache's, and
    /// not known until it is fetched (the result alone is counted till then). Doesn't mark
    /// anything used.
    fn cost(&mut self, id: u32, w: u32, h: u32) -> usize {
        let result = w as usize * h as usize * 4;
        match self.src(id).clone() {
            Src::File(path) => self
                .costs
                .entry(id)
                .or_insert_with(|| file_info(&path))
                .map_or(result, |info| info.cost(w, h)),
            #[cfg(feature = "remote-images")]
            Src::Remote(url) if self.remote.on => self
                .cache
                .peek_bytes(&url)
                .and_then(|b| bytes_cost(b, w, h))
                .unwrap_or(result),
            _ => result,
        }
    }

    /// A new frame is being painted: what it paints is the most recently used.
    pub fn begin_frame(&mut self) {
        self.cache.frame += 1;
    }

    /// Ask for the boxes near the viewport, most wanted first: (image id, width, height) in
    /// physical pixels, the first `on_screen` of them in view. Images not decoded at that size
    /// are queued for the worker, replacing what it hadn't started. With remote images on, URLs
    /// not fetched yet are queued for the fetchers: these boxes first, then the rest of the
    /// document's in order.
    ///
    /// The ones in view are always wanted; of the rest only as many as fit the cache beside
    /// them are, and those already decoded are kept (as if painted) while they are. Otherwise
    /// a screen of images bigger than the cache would evict each other, decoded again and
    /// again.
    ///
    /// Decoding holds memory of its own (the whole frame of a JPEG, see [`decode_cost`]): the
    /// most any job queued or running holds is kept free of the cache, the slots not painted
    /// evicted to make it, so the cache and a decode together stay within the budget. (Slots
    /// painted in the current frame are never evicted: a window of them is over it, as above.)
    /// A job that doesn't fit beside what the fetchers may hold is not handed to the worker
    /// until they are held off and hold nothing ([`Images::alone`]): this asks for that, and the
    /// window's next request, woken by the last of them, finds it so.
    pub fn request(&mut self, boxes: &[(u32, u32, u32)], on_screen: usize) {
        // Without decoders there's nothing to ask for (and the worker isn't built in).
        if !cfg!(feature = "images") {
            return;
        }
        // The bookkeeping is paid for out of the same budget, what this request builds beside it
        // too: the jobs (a `Vec` doubles as it grows) and the paths they copy.
        #[cfg(feature = "remote-images")]
        self.forget_evicted();
        self.charge_bookkeeping(
            boxes
                .len()
                .saturating_mul(2 * size_of::<Job>())
                .saturating_add(self.paths_heap),
        );
        let mut jobs = Vec::new();
        #[cfg(feature = "remote-images")]
        let mut fetch: Vec<Arc<str>> = Vec::new();
        #[cfg(feature = "remote-images")]
        let mut deferred: Vec<Arc<str>> = Vec::new();
        let mut wanted = 0usize;
        // Decodes of replaced workers still running hold memory beside the current worker's.
        let retired = self.retired_cost();
        let mut reserve = retired + self.worker.as_ref().map_or(0, Worker::busy_cost);
        // What is set aside for the fetchers, if any are (or are about to be) running.
        #[cfg(feature = "remote-images")]
        let mut starting = self.unfetched();
        #[cfg(feature = "remote-images")]
        let mut fetching = self.fetch_reserve(starting);
        #[cfg(not(feature = "remote-images"))]
        let fetching = 0;
        #[cfg(feature = "remote-images")]
        self.in_view.clear();
        // What can't be evicted, and what the fetchers would be given if they weren't held off:
        // a job that doesn't fit beside them can only run with them held off.
        #[cfg(feature = "remote-images")]
        let (kept, fetch_static) = (
            self.cache.kept(),
            self.fetch_static(self.remote.busy(), starting),
        );
        #[cfg(feature = "remote-images")]
        let (mut heavy_wanted, mut heavy_chosen) = (false, false);
        for (n, &(id, w, h)) in boxes.iter().enumerate() {
            let Some(key) = self.key(id) else {
                continue;
            };
            #[cfg(feature = "remote-images")]
            if let (true, Key::Remote(url)) = (n < on_screen, &key) {
                let (w, h) = clamp_box(w, h);
                self.in_view.push((url.clone(), w, h));
            }
            let (w, h) = clamp_box(w, h);
            let cached = self.cache.has(&key, w, h);
            // The result is held beside the cache too, until it goes in.
            let cost = if cached { 0 } else { self.cost(id, w, h) };
            let room = (self.cache.cap.saturating_sub(fetching))
                .saturating_sub(self.cache.overhead)
                .saturating_sub(reserve.max(retired.saturating_add(cost)));
            let over = n >= on_screen && wanted >= room;
            wanted = wanted.saturating_add(w as usize * h as usize * 4);
            if over {
                continue;
            }
            if cached {
                self.cache.touch(&key, w, h);
                continue;
            }
            if jobs
                .iter()
                .any(|j: &Job| (&j.key, j.w, j.h) == (&key, w, h))
            {
                continue;
            }
            // A decode that runs alone is the only job there is (see below).
            #[cfg(feature = "remote-images")]
            if heavy_chosen {
                continue;
            }
            let source = match self.src(id).clone() {
                Src::File(path) => Source::File(path),
                #[cfg(feature = "remote-images")]
                Src::Remote(url) if self.remote.on => match self.cache.peek_bytes(&url).cloned() {
                    Some(bytes) => Source::Bytes(bytes),
                    None => {
                        // Not fetched yet, or its bytes were evicted: fetch it (again). One
                        // dropped for lack of room, whose image isn't in view, waits to see
                        // if there is room once what is wanted this frame is known.
                        if self.remote.size(&url) != Some(None) {
                            if n >= on_screen && self.deferred.contains_key(&url) {
                                deferred.push(url);
                            } else {
                                fetch.push(url);
                                starting = true;
                                fetching = self.fetch_reserve(true);
                            }
                        }
                        continue;
                    }
                },
                _ => continue,
            };
            // A decode that doesn't fit beside what the fetchers may hold is run when they are
            // held off and hold nothing: the fetchers are told to stop now, and the job is
            // handed over by a request that finds them done (their last result wakes the window).
            // It is run alone: no other body is held for a decode (those wait, and decode first),
            // and no job behind it is handed over (a queued job holds its body).
            #[cfg(feature = "remote-images")]
            let need = retired.saturating_add(cost).saturating_add(match &source {
                Source::Bytes(body) => Slot::bytes_cost(key_url(&key), body),
                Source::File(_) => 0,
            });
            #[cfg(feature = "remote-images")]
            if !self.cache.fits(kept, 0, need, 0, fetch_static) {
                heavy_wanted = true;
                self.remote.pause(true);
                let own = match &key {
                    Key::Remote(url) => Some(url),
                    Key::Local(_) => None,
                };
                // (A replaced worker's decode that is still running is another decode beside it:
                // waited for, its result is taken by the next poll.)
                if retired > 0
                    || self.exclusive.as_ref().is_some_and(|k| *k != key)
                    || self.cache.holds_used_bytes_but(own)
                    || !self.alone(kept, need, None)
                {
                    continue;
                }
                self.exclusive = Some(key.clone());
                heavy_chosen = true;
            }
            // What the job holds is held for it from now on: the body is in use.
            #[cfg(feature = "remote-images")]
            if let Key::Remote(url) = &key {
                self.cache.bytes_of(url);
            }
            reserve = reserve.max(retired.saturating_add(cost));
            jobs.push(Job {
                doc_gen: self.doc_gen,
                key,
                source,
                w,
                h,
                cost,
            });
        }
        // A body dropped for lack of room is fetched again only if it would fit now, beside
        // what this frame keeps (the decoded images and bodies painted or wanted, which can't
        // be evicted), what decoding holds and what the fetchers are given: the test the body
        // meets when it arrives (`Cache::room_for`), so if the state doesn't change it is
        // admitted and the loop is over, and if it does not fit nothing is fetched until
        // something changes. (Its image coming into view is the other way: handled above.)
        #[cfg(feature = "remote-images")]
        for url in deferred {
            let fits = self.deferred.get(&url).is_some_and(|&(cost, decode)| {
                self.cache
                    .would_fit(cost, decode, reserve, self.fetch_reserve(true))
            });
            if fits {
                fetch.push(url);
                starting = true;
            }
        }
        #[cfg(feature = "remote-images")]
        {
            shrink_vec(&mut self.in_view);
            self.heavy_wanted = heavy_wanted;
            fetching = self.fetch_reserve(starting);
        }
        self.cache.reserve = reserve;
        // The cache is made room for them before they are let start: they can't take more.
        self.cache.fetching = fetching;
        self.cache.evict();
        #[cfg(feature = "remote-images")]
        self.forget_evicted();
        if !jobs.is_empty() && self.worker.is_none() {
            self.worker = Some(Worker::spawn(self.tx.clone(), self.wake.clone()));
        }
        if let Some(w) = &self.worker {
            w.set(jobs);
        }
        #[cfg(feature = "remote-images")]
        {
            // The job a body let in was waiting for has been asked for, or isn't wanted now.
            self.awaiting = false;
            self.settle_exclusive();
            self.sync_pause(starting);
        }
        #[cfg(feature = "remote-images")]
        if self.remote.on {
            for src in &self.srcs {
                if let Src::Remote(url) = src
                    && self.remote.size(url).is_none()
                {
                    fetch.push(url.clone());
                }
            }
            self.remote.fetch(fetch, self.tx.clone(), self.wake.clone());
        }
        // Room was made (a frame went by, what was decoded is released): the window takes the
        // bodies that wait, as it does when a result comes.
        #[cfg(feature = "remote-images")]
        if self.waiting_ready() {
            (self.wake)();
        }
        self.charge_bookkeeping(0);
    }

    /// What the replaced workers' decodes hold (see [`Images::retired`]); the idle ones are
    /// forgotten.
    fn retired_cost(&mut self) -> usize {
        self.retired.retain(|w| !Worker::idle_of(w));
        shrink_vec(&mut self.retired);
        self.retired.iter().map(Worker::busy_cost_of).sum()
    }

    /// Take the results that arrived: decoded images go in the cache; an image that turned out
    /// not to decode shows its alt text, and a fetched one gets its size (its block is laid out
    /// again, the reading position kept by the caller's anchor). Returns true if anything
    /// arrived (the window needs a repaint).
    pub fn poll(&mut self, doc: &Doc, layout: &mut Layout) -> bool {
        if !cfg!(feature = "images") {
            return false;
        }
        self.charge_bookkeeping(0);
        let mut any = false;
        while let Ok(done) = self.rx.try_recv() {
            any = true;
            match done {
                Done::Decoded {
                    doc_gen,
                    key,
                    bitmap,
                    worker,
                } => {
                    // The result is in the cache (or dropped) before the worker is let go on,
                    // so what it holds is never uncounted: see `Queue::busy`.
                    if matches!(key, Key::Local(_)) && doc_gen != self.doc_gen {
                        Worker::taken(&worker);
                        continue;
                    }
                    match (bitmap, key) {
                        (Some(b), key) => {
                            #[cfg(feature = "remote-images")]
                            if let Key::Remote(url) = &key {
                                self.cache.release_bytes(url);
                            }
                            self.decodes += 1;
                            self.charge_bookkeeping(0);
                            self.cache.insert(key, Data::Pixels(b));
                        }
                        (None, Key::Local(id)) => {
                            if let Some(s) = id
                                .checked_sub(1)
                                .and_then(|i| self.srcs.get_mut(i as usize))
                            {
                                *s = Src::None;
                            }
                            layout.set_image(doc, id, ImageSize::Alt);
                        }
                        #[cfg(feature = "remote-images")]
                        (None, Key::Remote(url)) => {
                            self.remote.failed(&url);
                            // The body that didn't decode is of no use: with it gone, the
                            // image is fetched again when a document with it is opened again
                            // (the failure is kept while this one is open).
                            self.cache.discard_bytes(&url);
                            self.sync_remote(doc, layout);
                        }
                        #[cfg(not(feature = "remote-images"))]
                        (None, Key::Remote(_)) => {}
                    }
                    Worker::taken(&worker);
                }
                #[cfg(feature = "remote-images")]
                Done::Fetched { url, result } => self.arrived(url, result),
            }
        }
        #[cfg(feature = "remote-images")]
        {
            // What the decoder made room for (its results are taken, a frame has passed) goes to
            // the bodies that waited.
            any |= self.admit_waiting();
            // What was decoded, and the bodies that went by, may have been the last of a decode
            // that ran with the fetchers held off.
            self.settle_exclusive();
            self.sync_pause(false);
            // A body that can only go in alone goes in once the fetchers are held off (which
            // was just asked for) and have nothing in the way.
            if self.admit_alone() {
                any = true;
                self.sync_pause(false);
            }
            self.forget_evicted();
            if any {
                self.sync_remote(doc, layout);
            }
        }
        any
    }

    /// A fetch finished. Its body is admitted to the cache only if the cache has room for it
    /// beside what is set aside already (the budget, less what decoding and the fetchers
    /// hold), after evicting what was not painted this frame: a body for an image in view is
    /// protected from eviction (it is decoded next), so without this the bodies of a screen of
    /// images could pile up, as many as the fetchers bring while the decoder works through them
    /// one at a time. A body that doesn't fit waits for room if its image is in view (see
    /// [`Images::admission`]), still holding the fetchers' credit, which stops them. Any other is
    /// dropped (its size is kept, it is fetched again when it is wanted).
    #[cfg(feature = "remote-images")]
    fn arrived(&mut self, url: Arc<str>, result: crate::remote::Fetched) {
        self.charge_bookkeeping(0);
        let admit = match &result {
            Ok((bytes, ..)) => self.admission(&url, bytes),
            Err(_) => Admit::Drop,
        };
        match admit {
            Admit::Wait => self.waiting.push((url, result)),
            Admit::Yes => self.take_fetched(url, result, true),
            Admit::Drop => self.drop_fetched(url, result),
        }
    }

    /// Forget what is kept per URL that the current document doesn't have and nothing holds:
    /// the records of bodies dropped for room ([`Images::deferred`]), and what fetching found
    /// ([`Remote::size`](crate::remote::Remote::size)) unless a body or pixels for the URL are
    /// cached (the slot is charged for the entry). Without this, a session through many documents
    /// would collect an entry for every URL ever seen. A failed URL of a document left is tried
    /// again when it is opened again: its record goes whatever is cached, and so does a body of
    /// it that didn't decode (a cached body is taken for good, and would fail again).
    #[cfg(feature = "remote-images")]
    fn prune_urls(&mut self) {
        let urls = &self.urls;
        self.deferred.retain(|u, _| urls.contains(u));
        shrink_map(&mut self.deferred);
        let gone: Vec<Arc<str>> = self
            .remote
            .failed_urls()
            .filter(|u| !urls.contains(*u))
            .cloned()
            .collect();
        for url in &gone {
            self.cache.discard_bytes(url);
        }
        let cached: std::collections::HashSet<&Arc<str>> = self
            .cache
            .slots
            .iter()
            .filter_map(|s| match &s.key {
                Key::Remote(url) => Some(url),
                Key::Local(_) => None,
            })
            .collect();
        self.remote
            .retain_sizes(|u, size| urls.contains(u) || (size.is_some() && cached.contains(u)));
    }

    /// Take a body that is not kept (no room, its image not in view), remembering what it would
    /// have cost so it is fetched again only when there is room for that (if the document has
    /// the URL: a body that comes late for another one is simply let go).
    #[cfg(feature = "remote-images")]
    fn drop_fetched(&mut self, url: Arc<str>, result: crate::remote::Fetched) {
        if let (Ok((bytes, ..)), true) = (&result, self.urls.contains(&url)) {
            let need = (
                Slot::bytes_cost(&url, bytes),
                self.decode_estimate(&url, bytes),
            );
            self.deferred.insert(url.clone(), need);
        }
        self.take_fetched(url, result, false);
    }

    /// What decoding the body fetched for `url` will hold (see [`decode_cost`]), to see if there
    /// is room for it: to the box of its image if that is in view, else to the size it has
    /// (the box is known when it is asked for, and the reserve is made then).
    #[cfg(feature = "remote-images")]
    fn decode_estimate(&self, url: &Arc<str>, bytes: &Bytes) -> usize {
        let Some(header) = header_info(&mut &bytes[..]) else {
            return 0;
        };
        let (w, h) = match self.in_view.iter().find(|(u, ..)| u == url) {
            Some(&(_, w, h)) => (w, h),
            None => clamp_box(header.w, header.h),
        };
        decode_cost(&header, w, h)
    }

    /// Whether the body fetched for `url` can go to the cache now, beside what is set aside
    /// already after evicting what was not painted this frame. One in view that doesn't fit
    /// waits: for the decoder, which holds what it is given until its result is taken, or, if it
    /// would not fit even with the decoder idle, for the fetchers to be held off and hold nothing
    /// ([`Images::admit_alone`]).
    #[cfg(feature = "remote-images")]
    fn admission(&mut self, url: &Arc<str>, bytes: &Bytes) -> Admit {
        let cost = Slot::bytes_cost(url, bytes);
        let decode = self.decode_estimate(url, bytes);
        // Whatever the fetchers hold now (they may be held off, and hold nothing): a body let in
        // is protected, and stays when they are let go on, so it has to fit beside all they may take.
        if self
            .cache
            .room_for(cost, decode, self.fetch_static(true, true))
        {
            Admit::Yes
        } else if !self.in_view.iter().any(|(u, ..)| u == url) {
            Admit::Drop
        } else {
            Admit::Wait
        }
    }

    /// Whether the body for `url` in view is one that never fits beside what the fetchers may
    /// hold (`kept` is what can't be evicted, [`Cache::kept`]): not even with the decoder idle and
    /// everything else evicted.
    #[cfg(feature = "remote-images")]
    fn is_heavy(&self, kept: usize, url: &Arc<str>, bytes: &Bytes) -> bool {
        let cost = Slot::bytes_cost(url, bytes);
        let decode = self.decode_estimate(url, bytes);
        let fetch = self.fetch_static(true, true);
        self.in_view.iter().any(|(u, ..)| u == url)
            && !self.cache.fits(kept, cost, decode, 0, fetch)
    }

    /// Whether the fetchers are held off and hold nothing but the bodies that wait (in
    /// `waiting`): no transfer is being read, and no result is in the channel or being looked at.
    #[cfg(feature = "remote-images")]
    fn fetchers_idle(&self) -> bool {
        let (reading, untaken) = self.remote.transfers();
        self.remote.paused() && reading == 0 && untaken <= self.waiting.len()
    }

    /// The fetchers are held off ([`Remote::pause`]) and have nothing but the bodies that wait:
    /// whether `need` bytes (a decode and what goes with it) fit beside what is kept and what
    /// those bodies hold; if they don't, those bodies, which are in the way, are let go (they are
    /// fetched again when wanted; `except` is the one that is let in). `false` while a transfer
    /// is still being read or a result is not taken yet, which nothing here can hurry: the
    /// transfers end by themselves (`curl` is limited in time, and a fetch that needs more
    /// credit gives up at once), their results are taken by [`Images::poll`], and nothing new
    /// starts, so this is true in the end. If nothing fits, nothing more can be done: that is a
    /// decode bigger than the cache, run alone.
    #[cfg(feature = "remote-images")]
    fn alone(&mut self, kept: usize, need: usize, except: Option<&Arc<str>>) -> bool {
        if !self.fetchers_idle() {
            return false;
        }
        let others = |s: &Self| s.waiting.iter().filter(|(u, _)| except != Some(u)).count();
        if others(self) > 0
            && !self
                .cache
                .fits(kept, 0, need, 0, self.remote.reserve_idle())
        {
            let (keep, drop): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
                .into_iter()
                .partition(|(u, _)| except == Some(u));
            self.waiting = keep;
            for (url, result) in drop {
                self.drop_fetched(url, result);
            }
        }
        true
    }

    /// Let in the body of an image in view that doesn't fit beside what the fetchers may hold,
    /// and so can only be decoded alone: the pool is held off (by [`Images::sync_pause`], as soon
    /// as such a body waits), and it goes in when the decoder is idle and no body is kept for a
    /// decode, and [`Images::alone`] says the fetchers have nothing in the way. It is then the
    /// one decode, with the fetchers still held off until its result is taken
    /// ([`Images::settle_exclusive`]). Returns true if a body was let in.
    #[cfg(feature = "remote-images")]
    fn admit_alone(&mut self) -> bool {
        if self.exclusive.is_some()
            || self.waiting.is_empty()
            || !self.worker.as_ref().is_none_or(Worker::idle)
            || self.cache.holds_used_bytes()
        {
            return false;
        }
        self.retired_cost();
        if !self.retired.is_empty() {
            return false;
        }
        let kept = self.cache.kept();
        let Some(url) = self.waiting.iter().find_map(|(u, r)| match r {
            Ok((bytes, ..)) if self.is_heavy(kept, u, bytes) => Some(u.clone()),
            _ => None,
        }) else {
            return false;
        };
        let need = match self.waiting.iter().find(|(u, _)| *u == url) {
            Some((_, Ok((bytes, ..)))) => {
                Slot::bytes_cost(&url, bytes).saturating_add(self.decode_estimate(&url, bytes))
            }
            _ => return false,
        };
        if !self.alone(kept, need, Some(&url)) {
            return false;
        }
        let Some(i) = self.waiting.iter().position(|(u, _)| *u == url) else {
            return false;
        };
        let (url, result) = self.waiting.remove(i);
        self.take_fetched(url.clone(), result, true);
        self.exclusive = Some(Key::Remote(url));
        self.awaiting = true;
        true
    }

    /// Whether a decode that ran with the fetchers held off is over: its job isn't queued, running
    /// or waiting for its result to be taken (and no replaced worker is decoding either, whose
    /// job this can't see), nor is it still to be asked for ([`Images::awaiting`]).
    #[cfg(feature = "remote-images")]
    fn settle_exclusive(&mut self) {
        let Some(key) = self.exclusive.clone() else {
            return;
        };
        self.retired_cost();
        if !self.awaiting
            && self.retired.is_empty()
            && !self.worker.as_ref().is_some_and(|w| w.has(&key))
        {
            self.exclusive = None;
            // A decode held back for it (see `request`) can be asked for now.
            if self.heavy_wanted {
                (self.wake)();
            }
        }
    }

    /// Hold the fetchers off while a decode runs that doesn't fit beside what they may hold, or
    /// one is wanted (an image in view whose body waits for it, or whose job was held back), and
    /// let them go on when none is. Letting them go on first sets aside what they are given, and
    /// makes the room for it, so they can't take it before it is there.
    #[cfg(feature = "remote-images")]
    fn sync_pause(&mut self, starting: bool) {
        // A decode that is queued or running and doesn't fit beside what the fetchers may hold
        // holds them off, however it came to be handed over (fetchers that weren't wanted then, a
        // document opened or remote images turned on since): they must not start beside it.
        let decoding = self
            .retired_cost()
            .saturating_add(self.worker.as_ref().map_or(0, Worker::pending_cost));
        let fetch_static = self.fetch_static(self.remote.busy(), starting);
        let conflict = decoding > 0 && fetch_static > 0 && {
            // The bodies the decodes are reading stay as long as they run.
            let kept = self.cache.kept().saturating_add(self.cache.pinned_cost());
            !self.cache.fits(kept, 0, decoding, 0, fetch_static)
        };
        let want = self.exclusive.is_some()
            || conflict
            || self.heavy_wanted
            || (!self.waiting.is_empty() && {
                let kept = self.cache.kept();
                self.waiting
                    .iter()
                    .any(|(u, r)| matches!(r, Ok((bytes, ..)) if self.is_heavy(kept, u, bytes)))
            });
        if want != self.remote.paused() {
            if !want {
                self.cache.fetching = self.fetch_static(self.remote.busy(), starting);
                self.cache.evict();
                self.forget_evicted();
            }
            self.remote.pause(want);
        }
        self.cache.fetching = self.fetch_reserve(starting);
    }

    /// The body (or failure) of a finished fetch is taken from the fetchers (their credit is
    /// free) and, if `keep`, goes to the cache, before the fetchers can use what they are given.
    #[cfg(feature = "remote-images")]
    fn take_fetched(&mut self, url: Arc<str>, result: crate::remote::Fetched, keep: bool) {
        self.charge_bookkeeping(0);
        let taken = self.remote.take_fetched(&url, result);
        if keep {
            self.deferred.remove(&url);
        }
        match (keep, taken) {
            (true, Some(bytes)) => {
                self.cache
                    .insert(Key::Remote(url.clone()), Data::Bytes(bytes));
                if self.in_view.iter().any(|(u, ..)| *u == url) {
                    self.cache.bytes_of(&url);
                }
            }
            (_, taken) => drop(taken),
        }
        // The fetchers' credit goes last: the body is in the cache, charged there, or freed.
        self.remote.release(&url);
        // A late body (its URL is not the document's, and nothing keeps it) leaves no record.
        if !keep && !self.urls.contains(&url) && !self.cache.has_url(&url) {
            self.remote.forget_sizes([&url]);
        }
        self.cache.fetching = self.fetch_reserve(false);
        self.forget_evicted();
    }

    /// Admit the bodies that waited, as far as there is room now. Returns true if any was
    /// taken.
    #[cfg(feature = "remote-images")]
    fn admit_waiting(&mut self) -> bool {
        self.charge_bookkeeping(0);
        let mut any = false;
        for (url, result) in std::mem::take(&mut self.waiting) {
            let admit = match &result {
                Ok((bytes, ..)) => self.admission(&url, bytes),
                Err(_) => Admit::Drop,
            };
            match admit {
                Admit::Wait => self.waiting.push((url, result)),
                Admit::Yes => {
                    self.take_fetched(url, result, true);
                    any = true;
                }
                Admit::Drop => {
                    self.drop_fetched(url, result);
                    any = true;
                }
            }
        }
        any
    }

    /// Whether a body that waited can go on now: its image is no longer in view, or there is room.
    #[cfg(feature = "remote-images")]
    fn waiting_ready(&mut self) -> bool {
        self.charge_bookkeeping(0);
        let waiting = std::mem::take(&mut self.waiting);
        let ready = waiting.iter().any(|(url, result)| match result {
            Ok((bytes, ..)) => !matches!(self.admission(url, bytes), Admit::Wait),
            Err(_) => true,
        });
        self.waiting = waiting;
        ready || self.alone_ready()
    }

    /// Whether [`Images::admit_alone`] would let a body in now (nothing is changed).
    #[cfg(feature = "remote-images")]
    fn alone_ready(&mut self) -> bool {
        if self.exclusive.is_some()
            || self.waiting.is_empty()
            || !self.worker.as_ref().is_none_or(Worker::idle)
            || self.cache.holds_used_bytes()
            || !self.fetchers_idle()
        {
            return false;
        }
        self.retired_cost();
        let kept = self.cache.kept();
        self.retired.is_empty()
            && self
                .waiting
                .iter()
                .any(|(u, r)| matches!(r, Ok((b, ..)) if self.is_heavy(kept, u, b)))
    }

    /// What the cache sets aside for the fetchers: [`remote::FETCH_RESERVE`] if any are running or
    /// queued (`busy`), or about to be (`starting`), else nothing.
    #[cfg(feature = "remote-images")]
    fn fetch_static(&self, busy: bool, starting: bool) -> usize {
        if busy || (starting && self.remote.on && !self.remote.no_curl) {
            crate::remote::FETCH_RESERVE
        } else {
            0
        }
    }

    /// What the cache sets aside for the fetchers now: [`Images::fetch_static`], or, while they
    /// are held off and no transfer is being read, only what they hold (the bodies of results
    /// not released), since they can't take any more.
    #[cfg(feature = "remote-images")]
    fn fetch_reserve(&self, starting: bool) -> usize {
        if self.remote.paused() && self.remote.transfers().0 == 0 {
            self.remote.reserve_idle()
        } else {
            self.fetch_static(self.remote.busy(), starting)
        }
    }

    /// Forget what is kept for the URLs whose last slot in the cache was evicted since this last
    /// ran, unless the document has them: their size and its string, and the room the table has
    /// left. (What a slot is charged for ends with it: nothing a URL had is kept for it.)
    #[cfg(feature = "remote-images")]
    fn forget_evicted(&mut self) {
        if self.cache.dropped.is_empty() {
            return;
        }
        let mut gone: std::collections::HashSet<Arc<str>> = std::mem::take(&mut self.cache.dropped)
            .into_iter()
            .collect();
        for s in &self.cache.slots {
            if let Key::Remote(url) = &s.key {
                gone.remove(url);
            }
        }
        gone.retain(|u| !self.urls.contains(u));
        self.remote.forget_sizes(&gone);
    }

    /// What the bookkeeping of the image code holds beside the cache's slots, the decodes and the
    /// fetchers' reserve: every long-lived container by its capacity (what its buffer holds, not
    /// what is in it) and the strings it owns. The document's tables (`srcs`, `urls`), the lists
    /// that are made and replaced by a request (`in_view`, `waiting`, the worker's queue and the
    /// pool's), the tables of per-URL records (`sizes`, `deferred`), the list of slots itself,
    /// the workers replaced and not yet idle. Everything that comes into the budget's arithmetic
    /// ([`Cache::held`]) goes through this, every time, so whatever a container keeps is paid for
    /// by the room it takes, and nothing needs a charge of its own. A URL's string is counted by
    /// each container that may outlive the others (the document's list of sources, a slot, the
    /// table of sizes, the queue of the pool), so one of the document's, which they share, is
    /// counted up to three times: an over-count of about a hundred bytes a URL, which is the price
    /// of not following who owns it.
    fn bookkeeping_bytes(&self) -> usize {
        let mut n = self
            .cache
            .bookkeeping()
            .saturating_add(vec_bytes(&self.srcs))
            .saturating_add(self.srcs_heap)
            .saturating_add(table_bytes(
                self.costs.capacity(),
                size_of::<(u32, Option<FileInfo>)>(),
            ))
            .saturating_add(vec_bytes(&self.retired));
        for shared in self
            .retired
            .iter()
            .chain(self.worker.iter().map(|w| &w.shared))
        {
            n = n.saturating_add(Worker::bookkeeping_of(shared));
        }
        #[cfg(feature = "remote-images")]
        {
            n = n
                .saturating_add(table_bytes(self.urls.capacity(), size_of::<Arc<str>>()))
                .saturating_add(table_bytes(
                    self.deferred.capacity(),
                    size_of::<(Arc<str>, (usize, usize))>(),
                ))
                .saturating_add(vec_bytes(&self.in_view))
                .saturating_add(vec_bytes(&self.waiting))
                .saturating_add(self.remote.bookkeeping_bytes())
                .saturating_add(self.remote_images.saturating_mul(REQUEST_PER_REMOTE));
        }
        n
    }

    /// Set what the cache holds back for the bookkeeping to what it is now, and `transient` more
    /// (what a request is building).
    fn charge_bookkeeping(&mut self, transient: usize) {
        self.cache.overhead = self.bookkeeping_bytes().saturating_add(transient);
    }

    /// Whether the document has a remote image nothing is known about yet.
    #[cfg(feature = "remote-images")]
    fn unfetched(&self) -> bool {
        self.srcs
            .iter()
            .any(|s| matches!(s, Src::Remote(url) if self.remote.size(url).is_none()))
    }

    /// Give every remote image of the document the size layout should use now: its alt text
    /// while remote images are off (or it failed), a loading box until fetched, then its size.
    #[cfg(feature = "remote-images")]
    pub(crate) fn sync_remote(&mut self, doc: &Doc, layout: &mut Layout) {
        for (i, src) in self.srcs.iter().enumerate() {
            if let Src::Remote(url) = src {
                layout.set_image(doc, i as u32 + 1, self.remote.layout_size(url));
            }
        }
    }

    /// Whether the remote image `id` is blocked (remote images are off but could be turned on):
    /// its placeholder says how to load it.
    pub fn blocked(&self, id: u32) -> bool {
        matches!(self.src(id), Src::Remote(_))
            && !self.remote_on()
            && self.policy() != RemoteImages::Never
            && !self.no_curl()
    }

    pub fn remote_on(&self) -> bool {
        #[cfg(feature = "remote-images")]
        return self.remote.on;
        #[cfg(not(feature = "remote-images"))]
        false
    }

    /// `curl` couldn't be run (remote images can't load).
    pub fn no_curl(&self) -> bool {
        #[cfg(feature = "remote-images")]
        return self.remote.no_curl;
        #[cfg(not(feature = "remote-images"))]
        false
    }

    /// The `remote_images` setting; without the `remote-images` feature, never.
    pub fn policy(&self) -> RemoteImages {
        #[cfg(feature = "remote-images")]
        return self.remote.policy;
        #[cfg(not(feature = "remote-images"))]
        RemoteImages::Never
    }

    pub fn set_policy(&mut self, policy: RemoteImages) {
        #[cfg(feature = "remote-images")]
        {
            self.remote.policy = policy;
        }
        #[cfg(not(feature = "remote-images"))]
        let _ = policy;
    }

    /// Another document: remote images are off again, before [`Images::attach`].
    pub fn forget_remote(&mut self) {
        #[cfg(feature = "remote-images")]
        {
            self.remote.on = false;
            self.remote.stop();
            // Nothing is wanted of the old document's URLs now (`attach` follows with the new
            // one's).
            self.urls = std::collections::HashSet::new();
            self.prune_urls();
        }
    }

    /// Turn remote images on or off for the document; their blocks are laid out
    /// again. Off drops the fetches not started yet; what was fetched stays cached.
    pub fn set_remote(&mut self, on: bool, doc: &Doc, layout: &mut Layout) {
        #[cfg(feature = "remote-images")]
        {
            self.remote.on = on && self.remote.policy != RemoteImages::Never;
            if !self.remote.on {
                self.remote.stop();
            }
            self.sync_remote(doc, layout);
        }
        #[cfg(not(feature = "remote-images"))]
        let _ = (on, doc, layout);
    }
}

impl crate::render::Pixels for Images {
    fn pixels(&mut self, id: u32, w: u32, h: u32) -> Option<&Bitmap> {
        let key = self.key(id)?;
        let (w, h) = clamp_box(w, h);
        self.cache.pixels(&key, w, h)
    }
}

#[cfg(all(test, feature = "images"))]
mod tests;
