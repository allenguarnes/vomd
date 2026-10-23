//! Images: where an image's file is, its size, decoding PNG, JPEG, GIF (the first frame) and
//! WebP, turned upright by their EXIF orientation, and drawing SVG (`svg.rs`), and the cache of
//! decoded images with the worker thread that fills it.
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

use std::io::{Cursor, Read, Seek};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use crate::doc::{Doc, ImageRef};
use crate::layout::{ImageSize, Layout, Refusal};
use crate::settings::RemoteImages;

#[cfg(feature = "images")]
mod anim;
mod exif;
#[cfg(feature = "images")]
mod forced;
#[cfg(feature = "images")]
mod gif;
#[cfg(feature = "svg")]
mod svg;
#[cfg(feature = "images")]
mod webp;

#[doc(hidden)]
pub use exif::orientation as exif_orientation;
/// The binary's allocator on Windows (see `svg::process::windows`).
#[cfg(all(feature = "svg", windows))]
#[doc(hidden)]
pub use svg::process::SvgChildAllocator;
/// SVGs drawn in a process of their own (see `svg::process`).
#[cfg(all(feature = "svg", any(target_os = "linux", windows)))]
pub use svg::process::{
    ARG as SVG_PROCESS_ARG, child as svg_child, running as svg_children,
    set_program as set_svg_program,
};
#[cfg(all(feature = "svg", any(target_os = "linux", windows)))]
#[doc(hidden)]
pub use svg::process::{
    EAT as SVG_TEST_EAT, SPIN as SVG_TEST_SPIN, set_cpu_limit as set_svg_cpu_limit,
    set_test as set_svg_test,
};

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
/// Bytes of an image looked through for its size and orientation, at most: a JPEG's segments
/// before the frame header (metadata: EXIF, color profiles, thumbnails), a PNG's chunks before
/// its data, a GIF's blocks before its first image, a WebP's chunks.
const MAX_HEADER_SCAN: u64 = 4 << 20;
/// What a JPEG's decoder keeps of its metadata segments (EXIF, XMP, ICC profile, IPTC), at most,
/// as [`jpeg_metadata`] counts it: a JPEG with more has the segments cut out before it is
/// decoded ([`shed_jpeg_metadata`]). None of it is used but the EXIF orientation, which is put
/// back.
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

/// How many times an animation plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loops {
    Forever,
    /// Plays in all, at least one.
    Plays(u32),
}

/// An animated GIF or WebP, as the worker finds it when it decodes the first frame: two frames
/// or more. Its canvas (the size of its first frame's bitmap before resampling) and loop count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimInfo {
    pub size: (u32, u32),
    pub loops: Loops,
}

/// What an animated image does, as its badge, its hover note and a click see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimState {
    /// It plays (or will, once its frames are made).
    Playing,
    /// The window's animations are off, the reader paused it, or it has played to its end.
    Paused,
    /// It doesn't fit beside the animations above it on screen: it plays once they scroll off.
    Waiting,
    /// It is over VOMD's limits for one animation: only a click plays it.
    Over,
    /// Its frames couldn't be fetched (its still is shown): a click tries again.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
    #[cfg(feature = "svg")]
    Svg,
}

/// What decoding an SVG holds, as far as it is known: from its header (`parse` 0: a guess is
/// charged), then as a job measured it (see `svg::render`), kept with the image's record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SvgCost {
    /// Bytes of its XML (inflated, for an svgz).
    xml: u32,
    /// It is an svgz: its XML is inflated into a copy.
    gz: bool,
    /// What parsing it holds, as a job counted; 0 until one has.
    parse: usize,
    /// What drawing its largest path holds, whatever the box.
    draw: usize,
    /// The pixmaps rendering holds at once beside its canvas and result, in sixteenths of the
    /// canvas (`svg::LAYER_UNITS`), their margins aside: those are for each of them
    /// (`pixmaps`), by the box's sides.
    layers: u32,
    pixmaps: u32,
    /// What decoding its largest embedded raster image holds besides its pixels.
    raster: usize,
}

/// What a decode is allowed: the limits of [`Limits::normal`] keep every image within the
/// budget of the cache ([`decode_cost`] says what one holds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Images with more pixels than this aren't decoded.
    pub max_pixels: u64,
    /// What decoding an SVG may hold all told, at most.
    pub memory: usize,
    /// A load the reader asked for past VOMD's limits ([`Limits::forced`]).
    pub forced: bool,
}

impl Limits {
    /// The limits of the images the cache holds.
    pub const fn normal() -> Limits {
        Limits {
            max_pixels: MAX_PIXELS,
            memory: CACHE_BYTES,
            forced: false,
        }
    }

    /// The limits of a load forced past the normal ones (a click on its placeholder): only
    /// `memory` (see [`forced_ceiling`]), the sides a decoder reads, and an SVG's hard caps.
    pub const fn forced(memory: usize) -> Limits {
        Limits {
            max_pixels: u64::MAX,
            memory,
            forced: true,
        }
    }
}

/// Why the image of `h` is over `limits` (VOMD's own), if it is: a raster's pixels, an SVG's
/// bytes of XML.
fn refusal(h: &Header, limits: &Limits) -> Option<Refusal> {
    #[cfg(feature = "svg")]
    if h.format == Format::Svg {
        let xml = h.svg.xml as usize;
        return (xml > svg::Caps::of(limits).xml).then_some(Refusal::Bytes(xml as u64));
    }
    let (w, h2) = h.shown();
    (u64::from(h.w) * u64::from(h.h) > limits.max_pixels).then_some(Refusal::Pixels(w, h2))
}

/// Whether the image of `h` is drawn (an SVG), not decoded.
fn is_svg(h: &Header) -> bool {
    #[cfg(feature = "svg")]
    return h.format == Format::Svg;
    #[cfg(not(feature = "svg"))]
    {
        let _ = h;
        false
    }
}

/// [`refusal`] of the image `bytes` start with, within the normal limits; `None` also for what
/// isn't an image.
#[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
pub(crate) fn refusal_of(bytes: &[u8]) -> Option<Refusal> {
    refusal(&info_of(bytes)?, &Limits::normal())
}

/// A cancel flag never set: for a decode nothing stops.
pub static NO_CANCEL: AtomicBool = AtomicBool::new(false);

/// Whether a decode was told to stop (checked once a row).
fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

// ---- Headers ----------------------------------------------------------------------------

/// The format and natural size of the image `r` starts with, read from its header (a PNG's
/// `IHDR` chunk, a JPEG's frame header, a GIF's screen, a WebP's first chunk, an SVG's root
/// tag): the size it is shown at, turned by its EXIF orientation. `None` for anything else
/// (AVIF, BMP, an SVG in a build without `svg`, ...), broken data, a zero size or a side over
/// [`MAX_SOURCE_SIDE`].
pub fn header(r: &mut (impl Read + Seek)) -> Option<(Format, u32, u32)> {
    header_info(r).map(|h| {
        let (w, h2) = h.shown();
        (h.format, w, h2)
    })
}

/// Plays the animated GIF or WebP `bytes` from its first frame for `steps` frames at most, at its
/// own size, giving `each` the frame drawn, the canvas (premultiplied `0xAARRGGBB`) and what of
/// it changed (left, top, width, height), for tests and the fuzzer, which count what it
/// allocates. Returns the canvas's size and what playing it is charged beside `bytes` and what is
/// made of the canvas; `None` if it isn't an animation, or that is over `room` (checked once the
/// index is made, which is small, before the canvas is).
#[cfg(feature = "images")]
#[doc(hidden)]
pub fn play_frames(
    bytes: &[u8],
    steps: usize,
    room: usize,
    mut each: impl FnMut(usize, &[u32], [u32; 4]),
) -> Option<((u32, u32), usize)> {
    let mut index = anim::Index::new(bytes)?;
    let held = index.held();
    if held > room {
        return None;
    }
    let mut canvas = anim::Canvas::new(&index);
    for _ in 0..steps {
        match canvas.step(&mut index, bytes, &NO_CANCEL) {
            anim::Advance::Drawn { frame, dirty, .. } => {
                each(frame, &canvas.px, [dirty.x, dirty.y, dirty.w, dirty.h]);
            }
            _ => break,
        }
    }
    Some((index.size, held))
}

/// Plays the animated GIF or WebP `bytes` resampled to `size` until `shows` frames were shown,
/// as the window and the anim thread play it ([`anim::play_box`]), for tests that count what it
/// allocates or look at its frames. Returns what it is charged beside `bytes`; `None` if it isn't
/// an animation, or that is over `room`.
#[cfg(feature = "images")]
#[doc(hidden)]
pub fn play_box(
    bytes: Arc<Vec<u8>>,
    size: (u32, u32),
    shows: usize,
    room: usize,
    each: impl FnMut(&Bitmap),
) -> Option<usize> {
    anim::play_box(bytes, size, shows, room, each)
}

/// [`header`] of an image in memory.
pub fn header_of(bytes: &[u8]) -> Option<(Format, u32, u32)> {
    header(&mut Cursor::new(bytes))
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
    /// bytes a pixel. WebP: the bytes a pixel of its output has (4 with alpha, else 3). 0 for
    /// a JPEG or a GIF.
    out_channels: usize,
    /// JPEG: the bytes of coefficients its decoder holds for the whole image (every
    /// component's samples as `i16`, padded to whole MCUs): for a progressive JPEG, and for a
    /// baseline one whose first scan doesn't hold all its components (the decoder then waits
    /// for the rest of the scans before it makes pixels). 0 for a baseline JPEG that is
    /// decoded an MCU row at a time, and for a PNG.
    coefficients: usize,
    /// JPEG: the bytes of what the decoder keeps per MCU row (see [`JPEG_ROW_BYTES`]).
    mcu_rows: usize,
    /// GIF: the bytes of an interlaced first frame, which is read whole (a byte a pixel); 0 for
    /// one read a row at a time. WebP: what its decoder works in besides its output (see
    /// `webp::working`). 0 for the other formats.
    held: usize,
    /// The EXIF orientation, 1 to 8: 1 as stored, 2 mirrored, 3 turned half round, 4 upside
    /// down; 5 to 8 swap the sides (the image is turned a quarter, mirrored or not). `w` and `h`
    /// are the size as stored.
    orientation: u8,
    /// SVG: what decoding it holds, as far as it is known. Nothing for the other formats.
    svg: SvgCost,
}

impl Header {
    /// The size the image is shown at: its stored size, its sides swapped for orientations 5–8.
    fn shown(&self) -> (u32, u32) {
        if self.transposed() {
            (self.h, self.w)
        } else {
            (self.w, self.h)
        }
    }

    /// Whether the orientation swaps the sides.
    fn transposed(&self) -> bool {
        self.orientation >= 5
    }
}

/// [`header_info`] of an image in memory.
fn info_of(bytes: &[u8]) -> Option<Header> {
    header_info(&mut Cursor::new(bytes))
}

fn header_info(r: &mut (impl Read + Seek)) -> Option<Header> {
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
            let whole = r.read_exact(&mut rest).is_ok();
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
                held: 0,
                orientation: if ok && whole { png_orientation(r) } else { 1 },
                svg: SvgCost::default(),
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
            held: 0,
            orientation: j.orientation,
            svg: SvgCost::default(),
        }),
        #[cfg(feature = "images")]
        [b'G', b'I'] => gif::header(r),
        #[cfg(feature = "images")]
        [b'R', b'I'] => webp::header(r),
        #[cfg(feature = "svg")]
        _ => svg::header(start, r),
        #[cfg(not(feature = "svg"))]
        _ => None,
    }
}

/// The orientation in a PNG's `eXIf` chunk, `r` just past the data of its `IHDR`: the chunks
/// after it are walked by their headers, their data skipped, as far as the first `IDAT` and
/// within [`MAX_HEADER_SCAN`] bytes. An `eXIf` after the image data isn't read, as the format
/// says and browsers do. 1 if there is none.
fn png_orientation(r: &mut (impl Read + Seek)) -> u8 {
    // `IHDR`'s checksum.
    let mut skip = 4;
    let mut scanned = 0u64;
    loop {
        let mut head = [0u8; 8];
        if r.seek_relative(skip).is_err() || r.read_exact(&mut head).is_err() {
            return 1;
        }
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        scanned += 12 + u64::from(len);
        if &head[4..] == b"IDAT" || scanned > MAX_HEADER_SCAN {
            return 1;
        }
        if &head[4..] == b"eXIf" {
            let mut tiff = [0u8; exif::HEAD];
            let tiff = &mut tiff[..(len as usize).min(exif::HEAD)];
            return match r.read_exact(tiff) {
                Ok(()) => exif::orientation(tiff),
                Err(_) => 1,
            };
        }
        skip = i64::from(len) + 4;
    }
}

/// What [`jpeg_size`] finds in a JPEG's headers.
struct JpegSize {
    w: u32,
    h: u32,
    coefficients: usize,
    mcu_rows: usize,
    orientation: u8,
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
        (len >= 2).then(|| (marker, u64::from(len - 2)))
    }

    /// Read the first bytes of a payload into `buf`.
    fn read(&mut self, buf: &mut [u8]) -> Option<()> {
        self.scanned += buf.len() as u64;
        if self.scanned > MAX_HEADER_SCAN {
            return None;
        }
        self.r.read_exact(buf).ok()
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
/// taken to find a scan with fewer components, the larger cost). The orientation is the first
/// EXIF `APP1`'s before the frame header (the start of it: [`exif::HEAD`]).
fn jpeg_size(r: &mut impl Read) -> Option<JpegSize> {
    let mut seg = JpegSegments { r, scanned: 0 };
    let mut orientation = None;
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
        if marker == 0xE1 && orientation.is_none() && len >= 6 {
            let mut tiff = [0u8; exif::HEAD];
            seg.read(&mut tiff[..6])?;
            let mut read = 6;
            if tiff.starts_with(b"Exif\0\0") {
                read = (len as usize).min(exif::HEAD);
                seg.read(&mut tiff[6..read])?;
                orientation = Some(exif::orientation(&tiff[..read]));
            }
            // The length's two bytes are counted by `skip`.
            seg.skip(len - read as u64)?;
            continue;
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
    let mut size = jpeg_info(w, h, progressive, &sampling, first_scan);
    size.orientation = orientation.unwrap_or(1);
    Some(size)
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
        orientation: 1,
    }
}

// ---- Local files ------------------------------------------------------------------------

/// Longest `src` of a local image, in bytes as written (percent-encoded): a path of a few KB is
/// longer than any system takes (`PATH_MAX` is 4096 on Linux, 1024 on macOS), and a longer string
/// would make the copies a lookup makes cost what the document's own does. Longer is refused.
pub const MAX_SRC: usize = 16 << 10;

/// The local file an image's `src` names: a path, absolute or relative to `base` (the
/// document's directory), percent-decoded, without its `?query` or `#fragment`. `None` for URLs
/// (anything with a scheme), for network paths (see [`is_network`]), for a relative path
/// without a base (a document read from stdin) and for a `src` longer than [`MAX_SRC`].
pub fn local_path(src: &str, base: Option<&Path>) -> Option<PathBuf> {
    let src = src.trim();
    if src.is_empty() || src.len() > MAX_SRC || src.chars().any(char::is_control) {
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

/// The natural size of a local image, from its header (`None`: missing, unreadable, not a
/// format [`decode`] reads, or too large to decode: see [`MAX_PIXELS`] and [`MAX_SOURCE_SIDE`]).
pub fn probe_file(path: &Path) -> Option<(u32, u32)> {
    match probe_size(path) {
        ImageSize::Pixels(w, h) => Some((w, h)),
        _ => None,
    }
}

/// The size layout gives the local image at `path`: its natural size, why it's over VOMD's
/// limits, or its alt text (missing, unreadable, not a format [`decode`] reads).
fn probe_size(path: &Path) -> ImageSize {
    let Some(h) = open_regular(path)
        .and_then(|f| header_info(&mut std::io::BufReader::with_capacity(4096, f)))
    else {
        return ImageSize::Alt;
    };
    match refusal(&h, &Limits::normal()) {
        Some(why) => ImageSize::Refused(why),
        None => {
            let (w, h) = h.shown();
            ImageSize::Pixels(w, h)
        }
    }
}

/// The size layout gives a local image: its natural size, or its alt text.
fn probe(img: &ImageRef, base: Option<&Path>) -> ImageSize {
    if img.remote {
        return ImageSize::Alt;
    }
    local_path(&img.src, base).map_or(ImageSize::Alt, |p| probe_size(&p))
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
///   capacity. For an EXIF orientation that swaps the sides (5–8) it is resampled to `h × w`,
///   and [`orient`] copies it into another `4·w·h` as it turns it.
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
/// * `source` for a GIF: [`GIF_FIXED`], the first frame's indexed row and the screen's RGBA
///   row (`5·sw`: the frame is no wider than the screen), and [`Header::held`] (an interlaced
///   frame, read whole). The decoder is given no metadata to keep (`gif.rs`).
/// * `source` for a WebP: its output, `out_channels·sw·sh` (RGB, or RGBA with alpha, decoded
///   whole), [`Header::held`] (what the decoder works in, by kind of image: `webp::working`)
///   and [`WEBP_FIXED`].
///
/// Every term is checked against what the decoders allocate, counted, in `tests/image_memory.rs`
/// (every combination of color type, depth, interlacing, sampling, scans and shape tried there
/// is under its charge) and by its random mixes.
fn decode_cost(header: &Header, w: u32, h: u32) -> usize {
    #[cfg(feature = "svg")]
    if header.format == Format::Svg {
        return svg::cost(&header.svg, w, h);
    }
    let result = w as usize * h as usize * 4;
    // Resampled as stored; turned, for 5–8, into a copy beside it.
    let (resample, turned) = if header.transposed() {
        (resample_cost(header.w, header.h, h, w), result)
    } else {
        (resample_cost(header.w, header.h, w, h), 0)
    };
    header_cost(header)
        .saturating_add(resample)
        .saturating_add(result)
        .saturating_add(turned)
}

/// The `source` term of [`decode_cost`]: what the decoder holds, whatever the box.
fn header_cost(header: &Header) -> usize {
    let pixels = header.w as usize * header.h as usize;
    match header.format {
        Format::Jpeg => pixels
            .saturating_mul(3)
            .saturating_add(header.coefficients)
            .saturating_add(header.mcu_rows)
            .saturating_add(JPEG_META_COST),
        Format::Gif => GIF_FIXED
            .saturating_add(5 * header.w as usize)
            .saturating_add(header.held),
        Format::Webp => pixels
            .saturating_mul(header.out_channels)
            .saturating_add(header.held)
            .saturating_add(WEBP_FIXED),
        // What it holds whatever the box: [`svg::cost`] with a box of no pixels.
        #[cfg(feature = "svg")]
        Format::Svg => svg::cost(&header.svg, 0, 0),
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
    }
}

/// What decoding the image `bytes` holds whatever the box ([`header_cost`]); 0 if it isn't one.
#[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
pub(crate) fn source_cost(bytes: &[u8]) -> usize {
    info_of(bytes).map_or(0, |h| header_cost(&h))
}

/// What decoding the image `bytes` holds for each pixel of its box besides its result twice
/// (which [`unfetched_cost`] counts), and for each pixel of its sides: an SVG's layers and their
/// margins (see [`svg::cost`]); nothing for a raster image, or if it isn't one.
#[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
pub(crate) fn box_costs(bytes: &[u8]) -> (u32, u32) {
    #[cfg(feature = "svg")]
    if let Some(h) = info_of(bytes).filter(|h| h.format == Format::Svg) {
        return svg::box_costs(&h.svg);
    }
    let _ = bytes;
    (0, 0)
}

/// What a GIF's decoder holds however small the image: its LZW tables and buffers, the read
/// buffer of 8 KB, the palettes, and its own structures.
const GIF_FIXED: usize = 64 << 10;

/// What a WebP's decoder holds however small the image: its tables, the lossless decoder's
/// color cache and the code it may build past `webp::huffman_limit` (the limit's own bytes are
/// in `Header::held`), and the map of the file's chunks.
const WEBP_FIXED: usize = 256 << 10;

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
    Some(decode_cost(&info_of(bytes)?, w, h))
}

/// What decoding `bytes` to `w` × `h` (see [`clamp_box`]) holds at most besides the bytes and
/// the pixels it returns (`None`: not an image [`decode`] reads), for tests that count what the
/// decode allocates.
#[doc(hidden)]
pub fn decode_reservation(bytes: &[u8], w: u32, h: u32) -> Option<usize> {
    let (w, h) = clamp_box(w, h);
    Some(bytes_cost(bytes, w, h)?.saturating_sub(w as usize * h as usize * 4))
}

/// [`decode`] as a load past the limits does, holding no more than `room` bytes in all (the
/// result's too), for tests that count what it allocates.
#[cfg(feature = "images")]
#[doc(hidden)]
pub fn decode_forced(bytes: &[u8], w: u32, h: u32, room: usize) -> Option<Bitmap> {
    decode_with(bytes, w, h, &Limits::forced(room), &NO_CANCEL)
}

/// An SVG's job as the worker runs it, for tests that count what it allocates: charged what its
/// record says ([`SvgJob::reservation`]), and run ([`SvgJob::run`]), which may find it needs more
/// and record that, as the main thread does for a job that says so.
#[cfg(feature = "svg")]
#[doc(hidden)]
pub struct SvgJob(Header, svg::Fonts);

#[cfg(feature = "svg")]
impl SvgJob {
    /// The job of `bytes`, if it is an SVG.
    pub fn of(bytes: &[u8]) -> Option<SvgJob> {
        info_of(bytes)
            .filter(|h| h.format == Format::Svg)
            .map(|h| SvgJob(h, svg::Fonts::default()))
    }

    /// Its text set in `fonts`' faces (it is drawn without text otherwise).
    pub fn with_fonts(mut self, fonts: &crate::fonts::Fonts) -> SvgJob {
        self.1.set(Some(Arc::new(fonts.svg_sources())));
        self
    }

    /// What its job to `w` × `h` is charged besides its result (as [`decode_reservation`]).
    pub fn reservation(&self, w: u32, h: u32) -> usize {
        decode_cost(&self.0, w, h).saturating_sub(w as usize * h as usize * 4)
    }

    /// Run its job to `w` × `h` within what it is charged: `Some` of the result, or `None` if it
    /// needed more, which it is charged from now on.
    pub fn run(&mut self, bytes: &[u8], w: u32, h: u32) -> Option<Option<Bitmap>> {
        let charge = decode_cost(&self.0, w, h);
        match svg::render(bytes, &self.0.svg, (w, h), charge, &NO_CANCEL, &mut self.1) {
            svg::Rendered::Pixels(b, _) => Some(Some(b)),
            svg::Rendered::Need(need) => {
                self.0.svg = need;
                None
            }
            _ => Some(None),
        }
    }
}

// ---- Decoding ---------------------------------------------------------------------------

/// The size [`decode`] makes an image of box size `w` × `h`: each side at least 1 and at most
/// [`MAX_SIDE`]. What the cache is asked for and told it has goes through this, so a size that
/// had to be clamped is still found.
pub fn clamp_box(w: u32, h: u32) -> (u32, u32) {
    (w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE))
}

/// Decode a PNG, JPEG, GIF or WebP and resample it to `w` × `h` (see [`clamp_box`]), upright,
/// or draw an SVG at that size. `None` for anything else, broken data, or more than
/// [`MAX_PIXELS`] (an SVG: more than the cache can hold, or too slow to draw).
#[cfg(feature = "images")]
pub fn decode(bytes: &[u8], w: u32, h: u32) -> Option<Bitmap> {
    decode_with(bytes, w, h, &Limits::normal(), &NO_CANCEL)
}

/// [`decode`] within `limits`, given up (`None`) once `cancel` is set.
#[cfg(feature = "images")]
pub fn decode_with(
    bytes: &[u8],
    w: u32,
    h: u32,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Option<Bitmap> {
    #[cfg(feature = "svg")]
    let fonts = &mut svg::Fonts::default();
    #[cfg(not(feature = "svg"))]
    let fonts = &mut ();
    decode_why(bytes, w, h, Sampling::Area, limits, cancel, fonts).ok()
}

/// How a decode reduces an image to its box: by averaging the source area under each target
/// pixel (the resampler's own), or, for an SVG's `image-rendering` of `optimizeSpeed`,
/// `crisp-edges` or `pixelated`, by taking the one under the target pixel's center, as resvg's
/// nearest-neighbor drawing does (pixels averaged away can't be picked afterwards).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "svg"), allow(dead_code))]
pub(crate) enum Sampling {
    Area,
    Nearest,
}

/// What SVG text is set in, for [`decode_why`].
#[cfg(feature = "svg")]
type SvgText = svg::Fonts;
#[cfg(not(feature = "svg"))]
type SvgText = ();

/// [`decode_with`], an SVG's text set in `fonts`, and why it made nothing: `Some` refusal for
/// `limits` (a click loads it anyway), `None` for broken data or a decode cancelled.
#[cfg(feature = "images")]
fn decode_why(
    bytes: &[u8],
    w: u32,
    h: u32,
    sampling: Sampling,
    limits: &Limits,
    cancel: &AtomicBool,
    fonts: &mut SvgText,
) -> Result<Bitmap, Option<Refusal>> {
    let header = info_of(bytes).ok_or(None)?;
    #[cfg(feature = "svg")]
    if header.format == Format::Svg {
        let (w, h) = clamp_box(w, h);
        return match svg::draw(
            bytes,
            &header.svg,
            (w, h),
            limits.memory,
            limits,
            cancel,
            &AtomicBool::new(false),
            fonts,
        ) {
            svg::Rendered::Pixels(b, _) => Ok(b),
            svg::Rendered::Need(need) => Err(Some(Refusal::Memory(svg::cost(&need, w, h) as u64))),
            svg::Rendered::Refused(why) => Err(svg_refusal(why)),
            svg::Rendered::Cancelled => Err(None),
        };
    }
    let _ = fonts;
    if let Some(why) = refusal(&header, limits) {
        return Err(Some(why));
    }
    let (sw, sh) = (header.w, header.h);
    // The box is the image as shown: it is resampled as stored, and turned after.
    let (w, h) = clamp_box(w, h);
    let (w, h) = if header.transposed() { (h, w) } else { (w, h) };
    let bitmap = match header.format {
        Format::Png => decode_png(bytes, w, h, true, sampling, cancel),
        Format::Gif => gif::decode(bytes, w, h, sampling, cancel),
        Format::Webp => webp::decode(bytes, &header, w, h, sampling, limits, cancel)?,
        // Drawn above, at the box's size.
        #[cfg(feature = "svg")]
        Format::Svg => None,
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
                    return Err(None);
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
            d.decode_headers().map_err(|_| None)?;
            // Neither side is over the limit, so the product is the size only if both are it.
            // (`info` would copy all the metadata, to read what this says.)
            if d.output_buffer_size() != Some(sw as usize * sh as usize * 3) {
                return Err(None);
            }
            let data = d.decode().map_err(|_| None)?;
            let frame = Frame {
                data: &data,
                width: sw,
                height: sh,
                stride: sw as usize * 3,
                channels: 3,
            };
            resample(frame, w, h, sampling, cancel)
        }
    }
    .ok_or(None)?;
    orient(bitmap, header.orientation, cancel).ok_or(None)
}

/// The refusal of an SVG's job for the placeholder: broken data is none.
#[cfg(feature = "svg")]
fn svg_refusal(why: svg::Refusal) -> Option<Refusal> {
    match why {
        svg::Refusal::Broken => None,
        svg::Refusal::Complex => Some(Refusal::Complex),
        svg::Refusal::Slow => Some(Refusal::Slow),
        // (How much it needs isn't known.)
        svg::Refusal::Memory => Some(Refusal::Memory(0)),
    }
}

/// `b`, resampled as its image is stored, turned the way its EXIF orientation says (see
/// [`Header::orientation`]): flipped in place, or for 5–8, whose sides swap, copied (which
/// [`decode_cost`] counts). Resampling first is the same as turning first: the resampler's
/// box filter treats both axes alike.
fn orient(mut b: Bitmap, orientation: u8, cancel: &AtomicBool) -> Option<Bitmap> {
    let (w, h) = (b.width as usize, b.height as usize);
    match orientation {
        2 => b.data.chunks_exact_mut(w).for_each(<[u32]>::reverse),
        3 => b.data.reverse(),
        4 => {
            let (top, bottom) = b.data.split_at_mut(w * (h / 2));
            for (a, z) in top.chunks_exact_mut(w).zip(bottom.rchunks_exact_mut(w)) {
                a.swap_with_slice(z);
            }
        }
        5..=8 => {
            // Shown, the image is `h` wide and `w` high: its row `y` is column `y` of `b` (5, 6)
            // or `w − 1 − y` (7, 8), read from the top (5, 8) or from the bottom (6, 7).
            let down = matches!(orientation, 5 | 8);
            let mut data = Vec::with_capacity(w * h);
            for y in 0..w {
                if cancelled(cancel) {
                    return None;
                }
                let sx = if orientation <= 6 { y } else { w - 1 - y };
                data.extend((0..h).map(|x| {
                    let sy = if down { x } else { h - 1 - x };
                    b.data[sy * w + sx]
                }));
            }
            return Some(Bitmap {
                width: h as u32,
                height: w as u32,
                data,
            });
        }
        _ => {}
    }
    Some(b)
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
/// the color transform from `APP14`; the EXIF orientation is put back, in an `APP1` of its own,
/// [`exif::app1`]), if there is more than [`MAX_JPEG_META`] of it, the segments as [`JpegWalk`]
/// finds them. `false` if what is left is still over the limit (cutting put together bytes the
/// decoder reads as a segment, which the walk of the bytes as they were didn't find): the file
/// is refused.
#[cfg(feature = "images")]
pub(crate) fn shed_jpeg_metadata(buf: &mut Vec<u8>) -> bool {
    if jpeg_metadata(buf) <= MAX_JPEG_META {
        return true;
    }
    // The orientation goes, with the rest of the EXIF data: it is put back on its own.
    let orientation = info_of(buf).map_or(1, |h| h.orientation);
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
    if orientation != 1 {
        // After the start-of-image marker, where `header` finds it before the frame header.
        buf.splice(2..2, exif::app1(orientation));
    }
    buf.shrink_to_fit();
    jpeg_metadata(buf) <= MAX_JPEG_META
}

/// A PNG's pixels resampled to `w` × `h`. A PNG that isn't interlaced is read a row at a time
/// (`rows`; `false` reads the whole frame, as an interlaced one must be): its full-size pixels
/// can be hundreds of MB, and the memory the allocator then keeps would sit beside a full cache.
#[cfg(feature = "images")]
fn decode_png(
    bytes: &[u8],
    w: u32,
    h: u32,
    rows: bool,
    sampling: Sampling,
    cancel: &AtomicBool,
) -> Option<Bitmap> {
    let sw = info_of(bytes)?.w;
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
        return resample_rows(
            &mut frame,
            (width, height),
            channels,
            (w, h),
            sampling,
            cancel,
        );
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
    resample(frame, w, h, sampling, cancel)
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
fn resample(
    mut frame: Frame,
    w: u32,
    h: u32,
    sampling: Sampling,
    cancel: &AtomicBool,
) -> Option<Bitmap> {
    let size = (frame.width, frame.height);
    let channels = frame.channels;
    resample_rows(&mut frame, size, channels, (w, h), sampling, cancel)
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

    /// [`Spans::new`] for [`Sampling::Nearest`]: the one source pixel under the center of each
    /// target pixel, weight one (in the room of the same tables: it is never more than a
    /// weight a target pixel).
    fn nearest(src: u32, dst: u32) -> Spans {
        let mut spans = Vec::with_capacity(dst as usize + 1);
        let mut weights = Vec::with_capacity(spans_weights(src, dst));
        for t in 0..dst {
            // ⌊(t + ½) · src / dst⌋, exactly.
            let at = ((2 * u64::from(t) + 1) * u64::from(src) / (2 * u64::from(dst))) as u32;
            spans.push((at.min(src - 1), weights.len() as u32));
            weights.push(1.0);
        }
        spans.push((src, weights.len() as u32));
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
/// `None` if the source ends early or its end is bad ([`Rows::finish`]), or `cancel` is set.
fn resample_rows(
    rows: &mut impl Rows,
    (sw, sh): (u32, u32),
    channels: usize,
    (w, h): (u32, u32),
    sampling: Sampling,
    cancel: &AtomicBool,
) -> Option<Bitmap> {
    let sw_px = sw as usize;
    let spans = match sampling {
        Sampling::Area => Spans::new,
        Sampling::Nearest => Spans::nearest,
    };
    let (xs, ys) = (spans(sw, w), spans(sh, h));
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
                if cancelled(cancel) {
                    return None;
                }
                let row = rows.row(y)?.get(..sw_px * channels)?;
                reduce_row(row, channels, &xs, &mut reduced.1);
                reduced.0 = y;
            }
            for (a, r) in acc.iter_mut().zip(&reduced.1) {
                *a += r * weight;
            }
        }
        data.extend(acc.as_chunks::<4>().0.iter().map(pack));
    }
    rows.finish().then_some(Bitmap {
        width: w,
        height: h,
        data,
    })
}

/// A target pixel's sums (premultiplied r, g, b and alpha, as [`reduce_row`] makes them) as
/// premultiplied `0xAARRGGBB`.
fn pack(px: &[f32; 4]) -> u32 {
    // Color sums carry a factor 255 from the alpha they're premultiplied by.
    let ch = |v: f32, scale: f32| (v / scale).round().clamp(0.0, 255.0) as u32;
    let a = ch(px[3], 1.0);
    let (r, g, b) = (
        ch(px[0], 255.0).min(a),
        ch(px[1], 255.0).min(a),
        ch(px[2], 255.0).min(a),
    );
    a << 24 | r << 16 | g << 8 | b
}

/// One source row (`channels` bytes a pixel) averaged along its width into `out`, four floats
/// (premultiplied r, g, b and alpha) per target pixel, by the `xs` spans.
fn reduce_row(row: &[u8], channels: usize, xs: &Spans, out: &mut [f32]) {
    for (t, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
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
// |                                     | and `sizes_str`, kept as it changes; given   | `attach`/`forget_remote`, and a URL's record goes when   |
// |                                     | the room for the document's URLs at attach   | its last slot is evicted (`forget_evicted`) or a late    |
// |                                     | (`fit_sizes`), and only those are recorded   | body is let go (none is made for it); cut to size and    |
// |                                     |                                              | given room again at the next `attach` (`fit_map`)        |
// | `Images::deferred`, `urls`          | bookkeeping: `table_bytes` of the capacity;  | replaced with the document; `deferred` given the room    |
// |                                     | `deferred` is given the room for the         | for its URLs at `attach`, and an entry is never removed  |
// |                                     | document's URLs at attach, like `sizes`      | while it is open (`undefer`: its value says none)        |
// | `costs` (sizes of local files)      | bookkeeping: `table_bytes` of the capacity;  | cut to the images of the request once it has twice as    |
// |                                     | its growth is in the request's allowance     | many (`request`), and replaced with the document         |
// | `srcs` (one `Src` per image),       | bookkeeping: capacity; charged, and the cache| replaced by the next document, the old one freed first   |
// | `urls` (a place per remote one) and | made room for, before any is made            |                                                         |
// | the room of `sizes`, `deferred`     | (`Images::tables_bytes`, `Images::attach`)   |                                                         |
// | the string in a `Src`               | not counted: the document's own, shared      | the document's                                           |
// |                                     | (`ImageRef::src` is an `Arc<str>`)           |                                                         |
// | a local image's path                | not kept: worked out from the string when a  | a lookup: dropped when it is done (`path_scratch` is    |
// |                                     | file's header or a job needs it; a job's copy| taken off the room before it is made); a job: with the   |
// |                                     | is charged as it is queued (`path_cost`)     | queue                                                    |
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
// | duplicates, jobs and their paths    | document while remote images are on (made    |                                                         |
// |                                     | room for at `attach`), `REQUEST_PER_BOX` and |                                                         |
// |                                     | `REQUEST_PER_REMOTE_BOX` for each box, the   |                                                         |
// |                                     | table of sizes grown to the local boxes, each|                                                         |
// |                                     | job's path, a path and a lookup in progress: |                                                         |
// |                                     | `Images::extra`, worked out by `request_fit` |                                                         |
// |                                     | and made room for first (`make_room`)        |                                                         |
// | `Cache::fetching` while held off    | `Remote::reserve_idle`: the bodies not       | `sync_pause`, and set back to the static reserve, and the |
// | and idle                            | released and their bookkeeping               | room made for it, before they go on                      |
//
// What is not charged: the page's own memory (the `Doc` and `Layout`, with each image's source and
// size in them, as for text), the `boxes` a caller passes to a request (the images within a screen
// of the viewport: the window's frame), the threads' stacks (not the allocator's), and the
// allocator's rounding beyond what each row allows for. What grows with a document's images is
// one `Src` (24 bytes) each and, for a remote image, a place in `urls` (about 20 to 40 bytes), and
// nothing else that stays: all of it is known from the numbers of images and is charged, and the
// cache made room for it, before any of it is made; the rest of what is per image is per box of
// a request, and goes with the request. `tests/image_memory.rs` runs random mixes of everything
// above with an allocator that counts what the image code has live apart from the page's and the
// test's own, and holds it to the budget and a small allowance for the allocator and the threads
// (`SLACK` there), the pixels painted in a frame being the one exception, by exactly their amount.
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
//
// Room is made in one place (review round 11). Every allocation of the image and remote code that
// is not covered by a reservation is preceded by a call of `Images::make_room(site, bytes)` with
// its size, at most, whose arithmetic holds everything that can't be evicted (the bookkeeping and
// what the operation in progress builds, `Images::extra`; what decodes hold, a result nobody has
// taken included; the fetchers' reserve; the bodies decodes read; the pixels painted in the frame)
// and which evicts the rest, and forgets what it evicted, to make `bytes` fit. A container that
// outlives an operation is never grown by an insert whose room is not made; and the answer, when
// it is no, has a defined meaning at every site. Debug builds tell a probe of every call
// (`set_room_probe`), so that the tests' allocator holds the live bytes to the budget when each is
// made, and when it says the room is there. The sites, and what guards each (`<` is a
// reservation made by the guard named; `FREE` a constant bounded by the allowance in the tests'
// `SLACK`, 64 KiB):
//
// | site                                       | guard                                      | if there is no room                         |
// |--------------------------------------------|--------------------------------------------|---------------------------------------------|
// | `attach`: `srcs`, `urls`, `sizes` and      | `make_room("attach: tables")` for          | none ever (`Never`, over the budget): every |
// | `deferred` made room for (`fit_map`),      | `tables_bytes` and the longest path, in    | image shows its alt text; not now but when  |
// | the paths worked out for `srcs`, the       | `admit_tables`, after the results nobody   | nothing decodes (`Later`): the tables are   |
// | layout probe's closure (the directory is   | has taken are taken (`drain_results`)      | made by the poll that takes the decode's    |
// | an `Arc`, shared)                          |                                            | result (`build_pending`)                    |
// | the layout's probe: a path and a header    | not the images': layout calls it. One at a | -                                           |
// | (`probe`, `probe_file`)                    | time, at most 2 x `MAX_SRC` + the directory| (FREE)                                      |
// |                                            | + 4 KiB                                    |                                             |
// | `take_done` / `drain_results`              | nothing is made: a result moves from the   | -                                           |
// |                                            | channel to the cache (`store`) or is freed |                                             |
// | `Cache::insert`: the slot list grows       | a request makes the room for the results it| a growth of 16 KiB (`SLOTS_FREE`) at most goes|
// | (the places of the results of jobs)        | asks for before it asks (`reserve_slots`,   | in anyway (painted slots are the exception to|
// |                                            | in its allowance; `Cache::expect` keeps it  | the budget); more: the request asks for fewer|
// |                                            | from `trim`); `store`: `make_room("slot     | boxes (no room *now* fails no image), a      |
// |                                            | list")` for what came beside them           | result with no place is dropped and asked for|
// |                                            |                                            | again, a body waits (in view) or is deferred |
// | the viewport (round 14): `in_view` (the    | `note_viewport`: `make_room("request: the  | made whatever the answer (FREE: at most     |
// | remote images in view, whole) and the      | viewport")` for both, and then `set_floor` | `VIEW_MAX` = 4,096 entries: 128 KB and      |
// | places of the images in view in the list   | (`Cache::floor`, kept by `trim`): what the | 360 KB, the size of what a window paints,   |
// | of slots                                   | window shows is wanted, and its places are | which is the exception to the budget)       |
// |                                            | there when a result or a body comes        |                                             |
// | `Cache::dropped`, `Cache::remove`          | the slot freed first, a URL is 16 bytes of | - (net memory only falls)                   |
// |                                            | the 112 or more that it was                |                                             |
// | `Cache::trim`, `shrink_vec`                | a realloc to less allocates nothing        | -                                           |
// | `forget_evicted`: a set of removed URLs    | 512 at a time (at most 18 KiB)             | - (FREE)                                    |
// | `prune_urls`: the set of cached URLs       | `make_room("prune: the cached URLs")`      | the records of other documents stay (they   |
// |                                            |                                            | are charged), and are looked at again       |
// | `request`: `costs` table, `jobs`, `refused`| `request_fit` works out all of it, boxes   | the boxes that don't fit are not asked for  |
// | `in_view`, the lists of URLs, the paths of | most wanted first; `make_room("request")`  | (a later request asks); `REQUEST_FREE` +    |
// | jobs, a path and a file's header in        | before any is made, the images it is about | `SLOTS_FREE` (32 KiB) is let whatever the   |
// | progress, `Remote::fetch`'s set and queue  | to keep protected first (`protect_wanted`) | room is; refused: asked again for fewer     |
// | the decodes a request queues (worker)      | `make_room("request: decodes and           | each decode was judged to fit (`fits_       |
// |                                            | fetchers")` with their reserve set, before | bookkeeping, `alone`): one that doesn't is  |
// |                                            | `Worker::set`                              | refused or run alone with the fetchers held |
// | the fetchers starting (`Remote::fetch`)    | the same call (the reserve is `fetching`)  | -                                           |
// | the fetchers released from a pause         | `make_room("pause released")`              | -                                           |
// | `Remote::record` (`sizes`), `defer`        | prepaid at attach; only the document's     | -                                           |
// | (`deferred`)                               | URLs, an entry is never removed (asserted) |                                             |
// | `Pool` requeue (`Error::Yielded`)          | its room for `FETCHERS` is made with the   | -                                           |
// |                                            | pool; the queue itself never grows; a      |                                             |
// |                                            | replaced queue keeps what it still wants   |                                             |
// | `waiting`, the lists `alone` and           | `FETCH_RESERVE` (`PENDING_COST` covers two | - (FREE: at most 260 entries, 25 KiB)       |
// | `admit_waiting` make from it               | entries of each of them)                   |                                             |
// | `retired` (`set_wake`)                     | the idle ones go first: a worker is a      | - (FREE: a kilobyte)                        |
// |                                            | decode's fixed costs, a hundred are the    |                                             |
// |                                            | budget                                     |                                             |
// | fetchers: read buffer, the body's credit,  | `FETCH_RESERVE`; the URL is at most        | -                                           |
// | `cut_to_size`'s copy, the copies of the    | `MAX_SRC` (`fetchable`)                    |                                             |
// | URL `Command` makes, results in channel   |                                            |                                             |
// | the decode's own allocations               | the job's cost, `Cache::reserve`           | -                                           |
// | `Worker::spawn`, `Pool::spawn`, watchdog   | constants (`WORKER_FIXED`, `PENDING_COST`);| -                                           |
// | threads, the channel                       | stacks are not the allocator's             |                                             |
// | anything made from a document's `src` or   | the length is judged first (`MAX_SRC`):    | -                                           |
// | URL (round 12: a lowercased scheme, a      | `fetchable` and `local_path` copy nothing  | a longer one is refused: alt text           |
// | decoded or joined path, a `format!`)       | of what is longer, and read the scheme's   | (not fetched, not a file)                   |
// |                                            | bytes (`has_http_scheme`); a path is made  |                                             |
// |                                            | only beside the room `path_scratch` has    |                                             |
// | `memory_report`                            | diagnostics for failing tests only         | -                                           |
//
// What keeps a slot from being evicted (review round 14: "painted" means a frame was drawn). Every
// place that says a slot is used, or kept, and what it stands for ([`Slot::unevictable`]):
// | place                                      | marks                                      | stands for                                  |
// |--------------------------------------------|--------------------------------------------|---------------------------------------------|
// | `Cache::pixels` (`Pixels::pixels`, the     | `used` and `painted`: the frame            | a frame that was drawn took the pixels: the |
// | paint of a frame)                          |                                            | exception to the budget, until              |
// |                                            |                                            | the next frame begins                       |
// | `Cache::insert_as` (a result stored)       | `used` only (the most recent)              | nothing: the room goes to the oldest first, |
// |                                            |                                            | and the result is as evictable as any other |
// | `Cache::touch` (cached boxes the request   | `used` only                                | nothing: recency                            |
// | keeps near the viewport)                   |                                            |                                             |
// | `Cache::protect` / `unprotect`             | `protect`, within `request` and `poll`     | what the operation in progress is about to  |
// | (`protect_wanted`; the results a poll took)| (cleared when it ends)                     | use: no longer than the call                |
// | `Cache::guard_admitted` (the bodies a poll | `protect` of them, while a body is let in  | what the next body's room is not made from: |
// | let in, around the next body's admission)  | (and `admitted`, until the poll ends)      | a decoded result evicts them like any other |
// | `Cache::bytes_of`                          | `used` of a body: the frame                | a decode input: an image in view had its    |
// | (`take_fetched` for an image in view; a    |                                            | body admitted, and its job holds it from the|
// | job made)                                  |                                            | next request on (`Slot::pinned`)            |
// | `Slot::pinned` (an `Arc` count over 1)     | nothing is stored                          | a decode input in flight                    |
// | `release_bytes`, `discard_bytes`           | `used` = 0 of a body                       | the decode is done: first to go             |

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
    /// An animation playing (or paused) in a box ([`anim::Playing`]): the frame it shows, and
    /// what makes the next. It is charged [`anim::Playing::cost`], what it holds at most, also
    /// while a step runs on the anim thread with its engine.
    #[cfg(feature = "images")]
    Anim(Box<anim::Playing>),
}

struct Slot {
    key: Key,
    data: Data,
    /// The frame it was last used in (stored, asked for, painted): which slot goes first when the
    /// room is made. For a body, the frame it was last taken for a decode in, which keeps it
    /// ([`Slot::unevictable`]); for pixels it keeps nothing.
    used: u64,
    /// One more than the frame the pixels were last painted in (0: never): what a frame that was
    /// drawn took from the cache ([`Cache::pixels`]). It is the only thing that keeps pixels from
    /// being evicted, besides a request's own wishes ([`Slot::protect`]): a result stored and not
    /// painted yet, or asked for and kept near, is as good as any other slot to the room that is
    /// made.
    painted: u64,
    /// Kept for the request being made ([`Cache::protect`]), and cleared when it is done.
    protect: bool,
    /// A body let in by the poll in progress ([`Cache::admitted`]): kept from the room that
    /// the next body's admission makes ([`Cache::guard_admitted`]), and from no other.
    admitted: bool,
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
            #[cfg(feature = "images")]
            Data::Anim(p) => p.cost,
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
            _ => None,
        }
    }

    /// Whether the pixels were painted in the frame `frame`: a frame that was drawn took them.
    fn painted_in(&self, frame: u64) -> bool {
        self.painted == frame.saturating_add(1)
    }

    /// What no eviction can free (see [`Cache::stuck`]): the pixels a frame was painted with
    /// (the one exception to the budget), the slots a request keeps while it works
    /// ([`Cache::protect`]) and the bodies decodes read, or are about to (an image in view had its
    /// body admitted, [`Cache::bytes_of`], in this frame: its job is asked for by the next request
    /// and holds it from then on, [`Slot::pinned`]). Nothing else: pixels that were stored, or
    /// asked for, or are kept as the images near the viewport, are evicted like any other when
    /// room is wanted, since nothing was painted with them.
    fn unevictable(&self, frame: u64) -> bool {
        self.protect
            || self.pinned()
            || match &self.data {
                Data::Pixels(_) => self.painted_in(frame),
                Data::Bytes(_) => self.used >= frame,
                // (Its engine is on the anim thread while a step runs: what that holds stays
                // charged until it is back.)
                #[cfg(feature = "images")]
                Data::Anim(p) => self.painted_in(frame) || p.running,
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
    /// What the anim thread's jobs hold outside the slots: an animation's file and index while
    /// it is read, and the steps of animations whose slots went while they ran
    /// ([`anim::Anims::out`]).
    #[cfg(feature = "images")]
    anims: usize,
    /// The URLs of the remote slots removed since [`Images::forget_evicted`] last looked: what is
    /// kept of a URL that has no slot left, and isn't the document's, is forgotten.
    dropped: Vec<Arc<str>>,
    /// Results the worker may still send back (jobs queued or running): each takes a slot, and
    /// the list of slots was given the room for them when they were asked for
    /// ([`Images::request`]), so a result is never turned away for want of a place to go.
    /// [`Cache::trim`] keeps that room.
    expect: usize,
    /// The places the list of slots keeps beside the slots it holds: one for each image in view
    /// ([`Images::note_viewport`]), so a result is never turned away, or its image asked for again,
    /// for want of a place; [`Cache::trim`] keeps them.
    floor: usize,
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
            #[cfg(feature = "images")]
            anims: 0,
            dropped: Vec::new(),
            expect: 0,
            floor: 0,
            frame: 0,
        }
    }

    /// What is set aside beside the slots: decoding, the fetchers, the bookkeeping.
    fn held(&self) -> usize {
        let held = self
            .reserve
            .saturating_add(self.fetching)
            .saturating_add(self.overhead);
        #[cfg(feature = "images")]
        let held = held.saturating_add(self.anims);
        held
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

    /// What stays however long things are waited for: the pixels painted in this frame (by a frame
    /// that was drawn: [`Cache::pixels`]). (The bodies waiting for their decodes, or held by them,
    /// are not here: each lets go of its body when its decode is done, and the decodes run one
    /// after another; [`Cache::would_fit`] counts them.)
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn kept(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.painted_in(self.frame))
            .map(Slot::cost)
            .sum()
    }

    fn has(&self, key: &Key, w: u32, h: u32) -> bool {
        // (The size first: it is a comparison of two numbers, and a URL's is of a string.)
        self.slots
            .iter()
            .any(|s| s.pixels().is_some_and(|b| (b.width, b.height) == (w, h)) && s.key == *key)
    }

    /// Mark the pixels for `key` at exactly `w` × `h`, if cached, as used this frame: the last to
    /// go when room is made, and no more than that (nothing was painted).
    fn touch(&mut self, key: &Key, w: u32, h: u32) {
        let frame = self.frame;
        for s in &mut self.slots {
            if s.pixels().is_some_and(|b| (b.width, b.height) == (w, h)) && s.key == *key {
                s.used = frame;
            }
        }
    }

    /// The pixels for `key` at `w` × `h`, or at another size while that one is decoded (a zoom
    /// step), for a frame that is being painted: marked as painted this frame, which is what keeps
    /// them from being evicted until the next one begins. The only place that is said.
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
        slot.painted = self.frame.saturating_add(1);
        slot.pixels()
    }

    /// A URL's bytes without marking them used: looking at an image that won't be decoded now
    /// mustn't keep its bytes from being evicted.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn peek_bytes(&self, url: &Arc<str>) -> Option<&Bytes> {
        self.slots.iter().find_map(|s| match &s.data {
            Data::Bytes(b) if matches!(&s.key, Key::Remote(u) if u == url) => Some(b),
            _ => None,
        })
    }

    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn bytes_of(&mut self, url: &Arc<str>) -> Option<Bytes> {
        let frame = self.frame;
        self.slots.iter_mut().find_map(|s| match &s.data {
            Data::Bytes(b) if matches!(&s.key, Key::Remote(u) if u == url) => {
                s.used = frame;
                Some(b.clone())
            }
            _ => None,
        })
    }

    #[cfg(test)]
    fn insert(&mut self, key: Key, data: Data) {
        self.insert_as(key, data, true);
    }

    /// [`Cache::insert`]; `current`: whether the pixels are for the current document (a result for
    /// one that has gone is cached as not painted).
    fn insert_as(&mut self, key: Key, data: Data, current: bool) {
        // One slot per image and size (and one for a URL's bytes).
        let same = |s: &Slot| {
            s.key == key
                && match (&s.data, &data) {
                    (Data::Pixels(a), Data::Pixels(b)) => {
                        (a.width, a.height) == (b.width, b.height)
                    }
                    (Data::Bytes(_), Data::Bytes(_)) => true,
                    #[cfg(feature = "images")]
                    (Data::Anim(_), Data::Anim(_)) => true,
                    _ => false,
                }
        };
        // A body a job of the worker holds can't be replaced (its charge would go while it is
        // allocated): it is the same URL's, and so the same body, and the new one is let go.
        if matches!(data, Data::Bytes(_)) && self.slots.iter().any(|s| same(s) && s.pinned()) {
            return;
        }
        // (Nothing is cut to size before the push: the list's room, made for by the caller
        // ([`Images::store`]), is the room the slot goes in, and a list cut to what it holds
        // would have to grow again for it.)
        self.retain_untrimmed(|s| !same(s));
        // Pixels come in for a box that is wanted: the most recently used, and no more than that
        // (nothing painted them: only a frame does, [`Cache::pixels`]). A fetched body counts as
        // used when something is made of it ([`Cache::bytes_of`]), not before: it may belong to
        // an image far from the viewport, and mustn't push out what is being looked at.
        // (Pixels decoded for a document that has gone are wanted by nothing yet: the next request
        // for one that has the image keeps them, and until then anything may have the room.)
        let used = match data {
            Data::Pixels(_) if current => self.frame,
            #[cfg(feature = "images")]
            Data::Anim(_) => self.frame,
            _ => 0,
        };
        let slot = Slot {
            key,
            data,
            used,
            painted: 0,
            protect: false,
            admitted: false,
        };
        // Room is made before the slot is in, so it can't be the one evicted.
        self.evict_to(slot.cost());
        self.bytes += slot.cost();
        self.slots.push(slot);
        self.trim();
    }

    /// What the next slot's place in the list allocates, if the list is full: its buffer made
    /// larger (the old one is charged already, as the bookkeeping, until it is freed).
    fn growth(&self) -> usize {
        vec_growth(&self.slots)
    }

    /// Keep what a request is about to ask for from the eviction that makes room for what the
    /// request allocates ([`Images::request`]): the pixels cached for `key` at `w` × `h` and the
    /// body of a remote image, until [`Cache::unprotect`], which ends the request. Nothing outlives
    /// it: what the request wants of the cache beyond its own run is what a frame painted.
    fn protect(&mut self, key: &Key, w: u32, h: u32) {
        for s in &mut self.slots {
            if match &s.data {
                Data::Pixels(b) => (b.width, b.height) == (w, h),
                Data::Bytes(_) => true,
                #[cfg(feature = "images")]
                Data::Anim(_) => false,
            } && s.key == *key
            {
                s.protect = true;
            }
        }
    }

    /// What [`Cache::protect`] set is cleared: every slot is as it was.
    fn unprotect(&mut self) {
        for s in &mut self.slots {
            s.protect = false;
            s.admitted = false;
        }
    }

    /// The body of `url` was let in by the poll in progress, until [`Cache::unprotect`].
    #[cfg(feature = "remote-images")]
    fn admitted(&mut self, url: &Arc<str>) {
        for s in &mut self.slots {
            if matches!(&s.key, Key::Remote(u) if u == url) && matches!(s.data, Data::Bytes(_)) {
                s.admitted = true;
            }
        }
    }

    /// Keep the bodies the poll in progress let in ([`Cache::admitted`]) from eviction, or not:
    /// for the admission of the next body, whose room is not made out of them. Not for a decoded
    /// result, which is stored whatever there is, and must leave the budget held beside the job
    /// that starts next: it evicts them like any slot not in use.
    #[cfg(feature = "remote-images")]
    fn guard_admitted(&mut self, on: bool) {
        for s in self.slots.iter_mut().filter(|s| s.admitted) {
            s.protect = on;
        }
    }

    /// What no eviction can free: the pixels painted in this frame and the bodies decodes hold
    /// ([`Slot::unevictable`]).
    fn stuck(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.unevictable(self.frame))
            .map(Slot::cost)
            .sum()
    }

    #[cfg(test)]
    fn evict(&mut self) {
        self.evict_for(0);
    }

    /// Drop the least recently used slots until the budget holds beside what decoding needs
    /// ([`Cache::reserve`]), what is set aside for the fetchers ([`Cache::fetching`]) and
    /// `extra` more bytes, never one painted in the current frame, nor one whose bytes a decode
    /// holds ([`Slot::unevictable`]): a pinned slot is charged until its job is done with it.
    fn evict_for(&mut self, extra: usize) {
        self.evict_to(extra);
        self.trim();
    }

    /// [`Cache::evict_for`] without cutting the list to size.
    fn evict_to(&mut self, extra: usize) {
        let held = self.held().saturating_add(extra);
        while self.bytes.saturating_add(held) > self.cap {
            let Some((i, _)) = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.unevictable(self.frame))
                .min_by_key(|(_, s)| s.used)
            else {
                break;
            };
            self.remove(i);
        }
    }

    /// Make room for a body of `cost` that will be decoded for `decode` more than what is kept
    /// free for decoding already, evicting what was not painted this frame, and say whether it
    /// fits. (A decode's memory is not held at the same time as another's: the larger counts.)
    /// Nothing is evicted when it can't: whatever the others hold, everything that could be taken
    /// out would be gone for nothing (a result nobody has painted yet was taken out by the
    /// admission of a body that then waited, and its image was decoded again).
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn room_for(&mut self, cost: usize, decode: usize, fetching: usize) -> bool {
        // What the fetchers may take counts whole, whatever they hold now (see `admission`).
        let now = std::mem::replace(&mut self.fetching, fetching);
        let extra = cost.saturating_add(decode.saturating_sub(self.reserve));
        let fits = self.can_fit(extra);
        if fits {
            self.evict_for(extra);
        }
        self.fetching = now;
        fits
    }

    /// Whether `extra` more bytes would fit the budget beside what can't be evicted
    /// ([`Cache::stuck`]) and what is set aside ([`Cache::held`]), once everything that can be is:
    /// nothing is evicted to find out.
    fn can_fit(&self, extra: usize) -> bool {
        self.can_fit_without(extra, 0)
    }

    /// [`Cache::can_fit`] with `freed` of what can't be evicted let go.
    fn can_fit_without(&self, extra: usize, freed: usize) -> bool {
        self.stuck()
            .saturating_sub(freed)
            .saturating_add(self.held())
            .saturating_add(extra)
            <= self.cap
    }

    /// What [`Cache::room_for`] would say if `reserve` and `fetching` were what is set aside,
    /// without evicting anything: whatever was painted this frame, and the bodies decodes hold,
    /// is all that can't go, so the body fits exactly when it fits beside that.
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn would_fit(&self, cost: usize, decode: usize, reserve: usize, fetching: usize) -> bool {
        self.fits(self.stuck(), cost, decode, reserve, fetching)
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
        #[cfg(feature = "images")]
        let kept = kept.saturating_add(self.anims);
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

    fn retain(&mut self, keep: impl FnMut(&Slot) -> bool) {
        self.retain_untrimmed(keep);
        self.trim();
    }

    /// [`Cache::retain`] without cutting the list to size. (The URLs of the slots dropped go on a
    /// list that grows as they go: each slot is at least [`SLOT_OVERHEAD`] and a URL's string
    /// freed, and an entry is 16 bytes, so what the removal frees pays for the list, even if it
    /// is doubled and copied.)
    fn retain_untrimmed(&mut self, mut keep: impl FnMut(&Slot) -> bool) {
        let (bytes, dropped) = (&mut self.bytes, &mut self.dropped);
        self.slots.retain(|s| {
            let k = keep(s);
            if !k {
                debug_assert!(!s.pinned(), "a slot a decode holds was dropped");
                #[cfg(feature = "images")]
                debug_assert!(
                    !matches!(&s.data, Data::Anim(p) if p.running),
                    "an animation whose step runs was dropped"
                );
                *bytes -= s.cost();
                if let Key::Remote(url) = &s.key {
                    dropped.push(url.clone());
                }
            }
            k
        });
    }

    /// A `Vec` keeps its capacity when it loses elements: the list of slots is cut to size once
    /// it has twice what it needs and a little more, so what a slot costs ([`SLOT_OVERHEAD`]
    /// covers the room for as many again) is all it can hold, and not what it once did (100,000
    /// one-pixel images were 7 MB of empty list after a new document had dropped them, beside
    /// a cache that counted none of it).
    fn trim(&mut self) {
        // (The places of the results to come are kept: see [`Cache::expect`], and the viewport's:
        // [`Cache::floor`].)
        let keep = self.slots.len().saturating_add(self.expect.max(self.floor));
        if self.slots.capacity() > 2 * keep + 16 {
            self.slots.shrink_to(keep);
        }
    }

    /// Give the list of slots the room for `results` more slots at once, if it has not got it:
    /// the buffer is made as large as it will be needed, beside the old one, which is charged
    /// until it is freed (the room for it is the request's allowance, [`Images::request_fit`]).
    fn reserve_slots(&mut self, results: usize) {
        let need = self.slots.len().saturating_add(results);
        if results > 0 && need > self.slots.capacity() {
            self.slots.reserve_exact(results);
        }
    }

    /// Give the list of slots `places` places beside the slots it holds, and keep them
    /// ([`Cache::trim`]): the viewport's.
    fn set_floor(&mut self, places: usize) {
        self.floor = places;
        self.reserve_slots(places);
    }

    /// What [`Cache::reserve_slots`] allocates for `results`: the new buffer, or nothing.
    fn reserve_bytes(&self, results: usize) -> usize {
        let need = self.slots.len().saturating_add(results);
        if results > 0 && need > self.slots.capacity() {
            need.saturating_mul(size_of::<Slot>())
        } else {
            0
        }
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

/// What the next push on `v` allocates: nothing, if its buffer has room, else the buffer it is
/// moved to (twice the old one, at least four elements), which is made beside the old one, itself
/// charged ([`vec_bytes`]) until it is freed.
pub(crate) fn vec_growth<T>(v: &Vec<T>) -> usize {
    if v.len() < v.capacity() {
        0
    } else {
        v.capacity()
            .saturating_mul(2)
            .max(4)
            .saturating_mul(size_of::<T>())
    }
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

/// A map made to have room for what it holds and `extra` more, and no more than twice that and 16
/// entries: [`Remote::sizes`] and [`Images::deferred`] are pruned to the document's URLs, which
/// leaves their tables as big as they were, and what a document may add to them is known from its
/// numbers, so they are given the room for it at once ([`Images::attach`]) and never grow after
/// (what is put in them is the document's URLs only, [`Remote::record`]). Allocates a table of at
/// most `table_bytes(len + extra)` beside the old one, once: the caller made room for that.
#[cfg(feature = "remote-images")]
pub(crate) fn fit_map<K: Eq + std::hash::Hash, V>(
    m: &mut std::collections::HashMap<K, V>,
    extra: usize,
) {
    let want = m.len().saturating_add(extra);
    if m.capacity() > 2 * want + 16 {
        m.shrink_to(want);
    }
    m.reserve(extra);
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
        /// An SVG's job was charged less than it needs, and decoded nothing: what it needs, for
        /// the image's record, and the image is asked for again ([`Images::remeasure`]).
        need: Option<SvgCost>,
        /// No pixels for VOMD's limits (not for broken data): the placeholder says why.
        refused: Option<Refusal>,
        /// An SVG drawn without some of its embedded images, for VOMD's limits.
        skipped: bool,
        /// The pixels are an animation's first frame ([`anim::detect`]).
        #[cfg(feature = "images")]
        anim: Option<AnimInfo>,
        /// The job was told to stop and made nothing: its image is given up already.
        cancelled: bool,
        /// The worker that made it, which waits until the result is taken ([`Worker::taken`]).
        worker: Shared,
    },
    /// A load past the limits ([`forced`]) of `origin` at `size` is done.
    #[cfg(feature = "images")]
    Forced {
        doc_gen: u64,
        origin: forced::Origin,
        result: Result<forced::Loaded, Refusal>,
    },
    /// The anim thread did job `serial` ([`anim::Job`]).
    #[cfg(feature = "images")]
    Anim { serial: u64, output: anim::Output },
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
    /// The faces SVG text is set in ([`Images::set_svg_fonts`]).
    #[cfg(feature = "svg")]
    fonts: Option<Arc<crate::fonts::SvgFonts>>,
    /// When the worker started the job in `busy`, while it runs.
    since: Option<std::time::Instant>,
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
/// The decode worker's stack.
const WORKER_STACK: usize = 8 << 20;
/// How long an SVG may take to draw before it's given up ([`Images::expire`]): its estimated
/// work is about a tenth of it, at most.
#[cfg_attr(not(feature = "svg"), allow(dead_code))]
const SVG_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

/// One thread decoding the most wanted image first; the queue is replaced as the viewport
/// moves, so images scrolled past are never decoded.
struct Worker {
    shared: Shared,
    /// Set, the job it runs stops (an SVG's drawing, as soon as it can: [`Images::expire`]).
    #[cfg_attr(not(feature = "svg"), allow(dead_code))]
    cancel: Arc<AtomicBool>,
    /// The SVG it is drawing is drawn in VOMD's own process, though SVGs are drawn apart (a child
    /// couldn't be started): it can't be killed ([`Images::let_go`]).
    #[cfg_attr(not(feature = "svg"), allow(dead_code))]
    inline: Arc<AtomicBool>,
}

impl Worker {
    fn spawn(tx: Sender<Done>, wake: Wake) -> Worker {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let theirs = shared.clone();
        let state = WorkerState::default();
        let cancel = state.cancel.clone();
        let inline = state.inline.clone();
        // An SVG's tree is walked by recursion, a call or two a level, 256 levels at most
        // (`svg::MAX_DEPTH`): the stack is said, not left to the platform. (Not the allocator's.)
        std::thread::Builder::new()
            .stack_size(WORKER_STACK)
            .spawn(move || Worker::work(&theirs, &tx, &wake, state))
            .expect("the decode worker");
        Worker {
            shared,
            cancel,
            inline,
        }
    }

    /// The worker's thread: the jobs of `theirs`, one at a time.
    fn work(theirs: &Shared, tx: &Sender<Done>, wake: &Wake, mut state: WorkerState) {
        let (lock, cv) = &**theirs;
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
                q.since = Some(std::time::Instant::now());
                #[cfg(feature = "svg")]
                state.fonts.set(q.fonts.clone());
                job
            };
            let mut made = decode_job(&job, &mut state);
            made.cancelled = made.bitmap.is_none()
                && made.need.is_none()
                && made.refused.is_none()
                && cancelled(&state.cancel);
            // Done: no deadline for it while its result waits to be taken.
            lock.lock().unwrap_or_else(|e| e.into_inner()).since = None;
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
                bitmap: made.bitmap,
                need: made.need,
                refused: made.refused,
                skipped: made.skipped,
                #[cfg(feature = "images")]
                anim: made.anim,
                cancelled: made.cancelled,
                worker: theirs.clone(),
            };
            if tx.send(done).is_err() {
                return;
            }
            wake();
        }
    }

    /// Replace the queue (most wanted first), skipping the job being decoded. Returns what that
    /// job holds, as of the swap: a job the worker took from the old queue since the caller last
    /// looked ([`Worker::busy_cost`]) can't be taken back, and it is only here that it is known.
    fn set(&self, mut jobs: Vec<Job>) -> usize {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((key, w, h, _)) = &q.busy {
            jobs.retain(|j| (&j.key, j.w, j.h) != (key, *w, *h));
        }
        q.jobs = jobs;
        cv.notify_one();
        q.busy.as_ref().map_or(0, |b| b.3)
    }

    /// The main thread took the result of the job the worker `shared` ran (it is in the cache,
    /// or dropped): that worker may go on to the next. It goes by the result, not by the current
    /// worker, which may be another one ([`Images::set_wake`]).
    fn taken(shared: &Shared) {
        let (lock, cv) = &**shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.busy = None;
        q.since = None;
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
        let paths: usize = q.jobs.iter().map(|j| path_cost(&j.source)).sum();
        // The font database its SVGs' text is set in, made or not.
        #[cfg(feature = "svg")]
        let fonts = q.fonts.as_ref().map_or(0, |f| svg_fonts_cost(f));
        #[cfg(not(feature = "svg"))]
        let fonts = 0;
        WORKER_FIXED
            .saturating_add(vec_bytes(&q.jobs))
            .saturating_add(paths)
            .saturating_add(fonts)
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

    /// The faces SVG text is set in from the next job on.
    #[cfg(feature = "svg")]
    fn set_fonts(&self, fonts: Option<Arc<crate::fonts::SvgFonts>>) {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).fonts = fonts;
    }

    /// What the largest job of this worker holds, queued, being decoded, or decoded and not taken.
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
        // An SVG drawn apart is stopped (its process killed): nothing waits for its result.
        #[cfg(feature = "svg")]
        if svg::apart() {
            self.cancel.store(true, Ordering::Relaxed);
        }
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.quit = true;
        // What was queued will not run: its hold on bodies ends now, not when the thread is gone.
        q.jobs = Vec::new();
        cv.notify_one();
    }
}

/// What the worker keeps between jobs: the flag that stops the job it runs ([`Worker::cancel`]),
/// and the font database of SVG text.
#[derive(Default)]
struct WorkerState {
    cancel: Arc<AtomicBool>,
    /// [`Worker::inline`].
    inline: Arc<AtomicBool>,
    #[cfg(feature = "svg")]
    fonts: svg::Fonts,
}

/// What the faces SVG text is set in cost to keep: the list of them, and the font database the
/// worker makes of them.
#[cfg(feature = "svg")]
fn svg_fonts_cost(f: &crate::fonts::SvgFonts) -> usize {
    let paths: usize = (f.origins.iter())
        .map(|o| match o {
            crate::fonts::FontOrigin::File(p) => p.as_os_str().len(),
            crate::fonts::FontOrigin::Embedded(_) => 0,
        })
        .sum();
    vec_bytes(&f.sources)
        .saturating_add(vec_bytes(&f.origins))
        .saturating_add(paths)
        .saturating_add(f.sources.len().saturating_mul(svg::FONTDB_COST))
}

/// What a job made: its pixels, or none if it failed (for VOMD's limits, `refused` says why);
/// for an SVG charged less than it needs, no pixels and what it needs.
#[derive(Default)]
struct Made {
    bitmap: Option<Bitmap>,
    need: Option<SvgCost>,
    refused: Option<Refusal>,
    skipped: bool,
    #[cfg(feature = "images")]
    anim: Option<AnimInfo>,
    /// The job was told to stop ([`Images::expire`]) and made nothing: the image was given up
    /// already, and has its own reason, not this result's.
    cancelled: bool,
}

#[cfg(feature = "images")]
fn decode_job(job: &Job, state: &mut WorkerState) -> Made {
    match &job.source {
        Source::File(path) => {
            // What the job is charged for is what the file was when its header was read: a file
            // that is bigger now, or has another header, is refused (it shows its alt text)
            // instead of decoded with memory nothing counted.
            let held = job.cost.saturating_sub(job.w as usize * job.h as usize * 4);
            let Some(mut bytes) = read_file(path, held) else {
                return Made::default();
            };
            let Some(cost) = bytes_cost(&bytes, job.w, job.h) else {
                return Made::default();
            };
            if bytes.len().saturating_add(cost) > job.cost {
                return Made::default();
            }
            let room = job.cost - bytes.len();
            // The bytes are ours: a JPEG's metadata is cut out of them where they are.
            if !shed_jpeg_metadata(&mut bytes) {
                return Made::default();
            }
            decode_in(&bytes, job, room, state)
        }
        Source::Bytes(bytes) => decode_in(bytes, job, job.cost, state),
    }
}

/// [`decode`] for `job`, holding at most `room` (its cost, less a file's bytes).
#[cfg(feature = "images")]
fn decode_in(bytes: &[u8], job: &Job, room: usize, state: &mut WorkerState) -> Made {
    #[cfg(feature = "svg")]
    if let Some(header) = info_of(bytes).filter(|h| h.format == Format::Svg) {
        let rendered = svg::draw(
            bytes,
            &header.svg,
            (job.w, job.h),
            room,
            &Limits::normal(),
            &state.cancel,
            &state.inline,
            &mut state.fonts,
        );
        return match rendered {
            svg::Rendered::Pixels(b, skipped) => Made {
                bitmap: Some(b),
                skipped,
                ..Made::default()
            },
            svg::Rendered::Need(need) => Made {
                need: Some(need),
                ..Made::default()
            },
            svg::Rendered::Refused(why) => Made {
                refused: svg_refusal(why),
                ..Made::default()
            },
            svg::Rendered::Cancelled => Made::default(),
        };
    }
    let _ = room;
    let limits = Limits::normal();
    #[cfg(feature = "svg")]
    let fonts = &mut svg::Fonts::default();
    #[cfg(not(feature = "svg"))]
    let fonts = &mut ();
    match decode_why(
        bytes,
        job.w,
        job.h,
        Sampling::Area,
        &limits,
        &state.cancel,
        fonts,
    ) {
        // Whether the image is animated is read from the bytes held already: block and chunk
        // headers only, nothing allocated.
        Ok(b) => Made {
            bitmap: Some(b),
            anim: anim::detect(bytes),
            ..Made::default()
        },
        Err(refused) => Made {
            refused,
            ..Made::default()
        },
    }
}

#[cfg(not(feature = "images"))]
fn decode_job(_: &Job, _: &mut WorkerState) -> Made {
    Made::default()
}

// ---- A document's images ----------------------------------------------------------------

/// Where one of the document's images comes from. Both kinds hold the document's own string for
/// the image (`ImageRef::src`, shared, not copied): where a local one's file is, is worked out
/// from it when a job or a file's size needs it ([`local_path`]) and not kept, so what an image
/// costs here is the size of this enum and nothing that grows with its path.
#[derive(Clone, Debug)]
enum Src {
    /// Nothing loadable (not found, failed to decode, not an image, a `data:` URL, ...).
    None,
    /// A local file: its `src`, which [`local_path`] resolves against the document's directory.
    File(Arc<str>),
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
    /// Not kept: its image is in view, and decoding it can't fit beside what the document's
    /// images cost to keep. It shows its alt text.
    Refuse,
}

/// The current document's images: where each comes from, the cache, and the threads that
/// decode (and, with `remote-images`, fetch) them.
pub struct Images {
    /// The document's directory, which relative paths resolve against. Shared (the layout's probe
    /// holds it too), so no copy of it is ever made.
    base: Option<Arc<Path>>,
    /// Bumped for every document: results for an older one are dropped.
    doc_gen: u64,
    /// One entry per image of the document, charged before it is made ([`Images::attach`]). The
    /// strings in it are the document's own, shared: the page's memory, as the `Doc` is, and not
    /// counted here.
    srcs: Vec<Src>,
    /// What each local image is (its file's size and header, for what decoding it holds: see
    /// [`decode_cost`]), by id, once its header is read: `None` if it can't be. Only the images
    /// asked for by the last requests have an entry: it is cut to those once it has twice as
    /// many ([`Images::request`]).
    costs: std::collections::HashMap<u32, Option<FileInfo>>,
    cache: Cache,
    /// What the operation in progress allocates at most beside what is kept (a request's jobs,
    /// lists and the copies of paths), charged with the bookkeeping while it runs and made room for
    /// before it starts ([`Images::make_room`]); nothing between operations.
    extra: usize,
    /// The tables of the document ([`Images::attach`]) are not made yet: a decode of the old
    /// document that is still running holds what they need. Every image shows its alt text
    /// meanwhile, and the poll that finds the room makes them ([`Images::build_pending`]).
    unbuilt: bool,
    /// The frame the tables were last tried for while they waited: a request in a later one asks
    /// the window to try again (what the old document painted is evictable from then on).
    unbuilt_tried: u64,
    /// Results with pixels taken so far ([`Stats::decodes`]).
    decodes: usize,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    wake: Wake,
    /// How many times the threads (and `request`) have called the wake, whoever it was given to:
    /// a window that is woken polls once for them; the tests that stand in for its event loop
    /// poll when this has gone up ([`Images::wakes`]).
    wakes: Arc<AtomicUsize>,
    worker: Option<Worker>,
    /// Workers replaced by [`Images::set_wake`] that may still be decoding (or hold a result the
    /// main thread hasn't taken): what they hold is counted until they are idle.
    retired: Vec<Shared>,
    /// The remote images in view when images were last asked for, with their boxes: a body that
    /// arrives for one is kept while the cache has room for it (and is decoded next), any other
    /// may be evicted at once.
    #[cfg(feature = "remote-images")]
    in_view: Vec<(Arc<str>, u32, u32)>,
    /// The scratch set of the distinct remote URLs in view that a request builds
    /// ([`Images::view_of`]), kept empty between requests. It is made with the document's tables,
    /// at the room for the most it can hold (the document's distinct URLs, at most [`VIEW_MAX`]),
    /// which is made and charged there ([`Images::tables_bytes`], [`Images::bookkeeping_bytes`]):
    /// a request never allocates it, so none of its room has to be made, or taken from what the
    /// request keeps, in the middle of it. It never grows.
    #[cfg(feature = "remote-images")]
    view_urls: std::collections::HashSet<Arc<str>>,
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
    /// The first job the last request held back for the fetchers ([`Images::alone`]): a poll
    /// that finds nothing in its way any more says so ([`Images::held_ready`]), since the
    /// transfer that was in the way may have given up without a result to take.
    #[cfg(feature = "remote-images")]
    held: Option<Key>,
    /// A poll said so since the last request: not said again before it.
    #[cfg(feature = "remote-images")]
    held_due: bool,
    /// Images whose decode can't fit beside what the document's images cost to keep (see
    /// [`Images::request`]), or that took too long: they show their alt text, and why, from the
    /// next poll.
    refused: Vec<(Key, Refusal)>,
    /// SVGs drawn without some of their embedded images, for VOMD's limits: a click draws them
    /// with ([`Images::force`]).
    partial: Vec<Key>,
    /// The animated images found by their decodes ([`Done::Decoded::anim`]): the current
    /// document's local ones, and the remote ones of its URLs or cached (a remote image's pixels
    /// may be cached from another document, and not decoded again).
    #[cfg(feature = "images")]
    animated: Vec<(Key, AnimInfo)>,
    /// The animations, besides those in the cache ([`anim::Anims`]).
    #[cfg(feature = "images")]
    anims: anim::Anims,
    /// An image was chosen to load past the limits since the last poll: it is laid out anew.
    repaint: bool,
    /// The loads past the limits the reader asked for ([`forced`]).
    #[cfg(feature = "images")]
    forced: forced::Forced,
    /// The images of the document that were chosen to load past the limits, and where from.
    #[cfg(feature = "images")]
    forced_ids: Vec<(u32, forced::Origin)>,
    /// The faces SVG text is set in, for the workers ([`Images::set_svg_fonts`]).
    #[cfg(feature = "svg")]
    svg_fonts: Option<Arc<crate::fonts::SvgFonts>>,
    /// A worker let go because its SVG took too long ([`Images::expire`]), drawn in VOMD's own
    /// process: while it is still drawing, no other SVG is (raster images are). One drawn apart
    /// is killed, and nothing waits for it.
    #[cfg(feature = "svg")]
    stuck: Option<Shared>,
    /// [`SVG_DEADLINE`] (the tests' is shorter).
    #[cfg(feature = "svg")]
    svg_deadline: std::time::Duration,
    #[cfg(feature = "remote-images")]
    pub(crate) remote: crate::remote::Remote,
}

/// The URL of a remote key (the empty string for a local one).
#[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
fn key_url(key: &Key) -> &str {
    match key {
        Key::Remote(url) => url,
        Key::Local(_) => "",
    }
}

/// What the body a job reads costs in the cache (a local file's bytes are in the job's cost).
fn body_cost(key: &Key, source: &Source) -> usize {
    match source {
        Source::File(_) => 0,
        Source::Bytes(body) => Slot::bytes_cost(key_url(key), body),
    }
}

/// What decoding a remote image of `sw` × `sh` to `w` × `h` is taken to hold when its body has
/// gone (see [`decode_cost`]): what its decoder holds whatever the box (`source`, as recorded
/// when it was fetched: [`source_cost`]), the resampler's tables (for either orientation,
/// `sw` × `sh` being the size shown) and the result twice (an EXIF orientation may turn it into
/// a copy), and `per_px` more a pixel of the box and `per_side` a pixel of its sides (an SVG's
/// layers: [`box_costs`]): no less than
/// what it holds when the body is there, so that deciding whether an image is wanted doesn't
/// change with whether its body is cached.
#[cfg(feature = "remote-images")]
fn unfetched_cost(
    source: usize,
    (per_px, per_side): (u32, u32),
    (sw, sh): (u32, u32),
    w: u32,
    h: u32,
) -> usize {
    let pixels = w as usize * h as usize;
    source
        .saturating_add(resample_cost(sw, sh, w, h).max(resample_cost(sh, sw, h, w)))
        .saturating_add(pixels.saturating_mul(per_px as usize + 8))
        .saturating_add((w as usize + h as usize).saturating_mul(per_side as usize))
}

/// What the copy of a local file's path a job carries holds (see [`Worker::bookkeeping_of`]).
fn path_cost(source: &Source) -> usize {
    match source {
        Source::File(path) => path.as_os_str().len() + STR_OVERHEAD,
        Source::Bytes(_) => 0,
    }
}

/// What the list of slots may grow by whatever the answer of [`Images::make_room`] is: see
/// [`Images::store`].
const SLOTS_FREE: usize = 16 << 10;

/// How many URLs of removed slots [`Images::forget_evicted`] looks at at once.
#[cfg(feature = "remote-images")]
const FORGET_CHUNK: usize = 512;

/// What a table entry of [`Images::deferred`] is.
#[cfg(feature = "remote-images")]
const DEFERRED_ENTRY: usize = size_of::<(Arc<str>, (usize, usize))>();

/// What an entry of [`Images::deferred`] holds when its body is not one dropped for room: the
/// entry stays, so that no entry is ever removed.
#[cfg(feature = "remote-images")]
const NOT_DEFERRED: (usize, usize) = (usize::MAX, usize::MAX);

/// What looking at a local file's header allocates, at most ([`file_info`]): the reader's buffer
/// and the header's own lists (a JPEG's components), one at a time.
const FILE_LOOKUP: usize = 8 << 10;

/// What a request may allocate whatever the room: a few dozen boxes' worth (when what is held
/// back for the fetchers and decodes is more than the budget, as it is when a decode is let run
/// alone, a request for the boxes in view is still made). It is no more than this, whatever the
/// answer of [`Images::make_room`], and nothing else is let past the budget.
const REQUEST_FREE: usize = 16 << 10;

/// What a request allocates for each box it is given, at most: the jobs made (a `Vec` grows by
/// doubling, a new buffer beside the old) and the list of images refused.
const REQUEST_PER_BOX: usize = 3 * (size_of::<Job>() + size_of::<Key>());

/// What a request allocates for each remote box, besides that: the list of the ones in view, and
/// the lists of URLs to fetch and those dropped for room.
#[cfg(feature = "remote-images")]
const REQUEST_PER_REMOTE_BOX: usize =
    3 * size_of::<(Arc<str>, u32, u32)>() + 6 * size_of::<Arc<str>>();

/// What a request can be made for ([`Images::request_fit`]).
#[derive(Default)]
struct Fit {
    /// How many of the boxes, most wanted first.
    boxes: usize,
    /// What the request allocates at most for them.
    allowance: usize,
    /// How many of those are not decoded yet, and may be asked for: the work that was charged.
    work: usize,
    /// How many of those are local images (each has an entry in the table of the files' sizes).
    local: usize,
    /// How many of those may become jobs now (a local image, or a remote one whose body is in the
    /// cache): each takes a slot when its result comes, and the list is given the room for them.
    results: usize,
}

/// Whether the tables of a document can be made ([`Images::admit_tables`]).
enum Tables {
    /// They fit, and the room for them is made.
    Now,
    /// They would fit if nothing were decoding.
    Later,
    /// They don't fit the budget: every image shows its alt text.
    Never,
}

/// Debug builds only: what the tests give to be told of every call of [`Images::make_room`]: its
/// name, the bytes asked for, and, when the room is made, whether it fits. A test with a counting
/// allocator holds the live bytes to the budget at both, so an allocation made before its
/// room is, is found where it is made ([`set_room_probe`]).
#[cfg(debug_assertions)]
type RoomProbe = fn(&'static str, usize, Option<bool>);

#[cfg(debug_assertions)]
static ROOM_PROBE: std::sync::OnceLock<RoomProbe> = std::sync::OnceLock::new();

/// Call `probe` at every [`Images::make_room`] (debug builds, once per process: tests).
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn set_room_probe(probe: RoomProbe) {
    let _ = ROOM_PROBE.set(probe);
}

/// What a request builds per remote image of the document, besides what stays: the list of URLs
/// to fetch (it doubles as it grows), the list of those dropped for room, and the set that takes
/// the duplicates out (see `Remote::fetch`), and the old queue as the new one replaces it.
#[cfg(feature = "remote-images")]
const REQUEST_PER_REMOTE: usize = 128;

/// How many images in view the viewport is noted for ([`Images::view_of`]), at most: the list
/// of the remote ones (`Images::in_view`) and the places in the list of slots are 128 KB and 360 KB
/// at most. A window shows a few dozen; one with more has the rest wait for room as an image out
/// of view does.
const VIEW_MAX: usize = 4096;

/// The images in view in a request ([`Images::view_of`]): how many of its boxes are, and how many
/// distinct images they are (a place each in the list of slots). The distinct remote URLs among
/// them are in [`Images::view_urls`] until [`Images::note_viewport`] has made [`Images::in_view`]
/// of them.
struct View {
    len: usize,
    places: usize,
}

/// What a document of `images` images, `remote` of them remote, costs to keep in [`Images`]: a
/// [`Src`] each, and a place in the set of URLs for a remote one. It is the whole of what grows
/// with the document's images, and is known from their numbers: [`Images::attach`] charges it,
/// and makes room for it, before making any of it.
fn attach_bytes(images: usize, remote: usize) -> usize {
    #[cfg(feature = "remote-images")]
    let urls = table_bytes(remote, size_of::<Arc<str>>());
    #[cfg(not(feature = "remote-images"))]
    let urls = {
        let _ = remote;
        0
    };
    images.saturating_mul(size_of::<Src>()).saturating_add(urls)
}

impl Default for Images {
    fn default() -> Self {
        let (tx, rx) = channel();
        let wakes: Arc<AtomicUsize> = Arc::default();
        let counted = wakes.clone();
        Images {
            base: None,
            doc_gen: 0,
            srcs: Vec::new(),
            costs: std::collections::HashMap::new(),
            cache: Cache::new(CACHE_BYTES),
            extra: 0,
            unbuilt: false,
            unbuilt_tried: 0,
            decodes: 0,
            tx,
            rx,
            wake: Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
            wakes,
            worker: None,
            retired: Vec::new(),
            #[cfg(feature = "remote-images")]
            in_view: Vec::new(),
            #[cfg(feature = "remote-images")]
            view_urls: std::collections::HashSet::new(),
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
            held: None,
            #[cfg(feature = "remote-images")]
            held_due: false,
            refused: Vec::new(),
            partial: Vec::new(),
            #[cfg(feature = "images")]
            animated: Vec::new(),
            #[cfg(feature = "images")]
            anims: anim::Anims::default(),
            repaint: false,
            #[cfg(feature = "images")]
            forced: forced::Forced::default(),
            #[cfg(feature = "images")]
            forced_ids: Vec::new(),
            #[cfg(feature = "svg")]
            svg_fonts: None,
            #[cfg(feature = "svg")]
            stuck: None,
            #[cfg(feature = "svg")]
            svg_deadline: SVG_DEADLINE,
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
            // (The list holds the workers still decoding or with a result untaken, each charged
            // at least a decode's fixed costs, hundreds of KB: a hundred of them are more than the
            // budget, and what the list holds is a kilobyte. The idle ones are forgotten first.)
            self.retired_cost();
            self.retired.push(old.shared.clone());
        }
        // Every call is counted, then passed on.
        let wakes = self.wakes.clone();
        let wake: Wake = Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
            wake();
        });
        #[cfg(feature = "remote-images")]
        self.remote.set_wake(wake.clone());
        self.wake = wake;
    }

    /// When the SVG the worker is drawing has taken too long ([`SVG_DEADLINE`]): it's given up
    /// then ([`Images::expire`]). `None` if it isn't drawing one: no timer.
    pub fn deadline(&self) -> Option<std::time::Instant> {
        #[cfg(feature = "svg")]
        if let Some(w) = &self.worker {
            let (key, since) = {
                let q = w.shared.0.lock().unwrap_or_else(|e| e.into_inner());
                (q.busy.as_ref()?.0.clone(), q.since?)
            };
            if self.is_svg(&key) {
                return Some(since + self.svg_deadline);
            }
        }
        None
    }

    /// Give up the SVG the worker is drawing if it has taken too long (the estimate of its work
    /// missed): its drawing is told to stop (its process killed, if SVGs are drawn apart), its
    /// image shows its alt text from the next poll, and the worker is let go (what it holds stays
    /// counted till it is done, which the flag makes soon); a new one decodes the rest. Returns true if it did (the window polls).
    pub fn expire(&mut self, now: std::time::Instant) -> bool {
        #[cfg(feature = "svg")]
        if self.deadline().is_some_and(|d| now >= d) {
            return self.let_go(true);
        }
        let _ = now;
        false
    }

    /// Let the worker go: its job is told to stop (an SVG drawn apart, its process killed), and
    /// a new worker decodes the rest; what it holds stays counted till it is done. `slow`: the
    /// image it draws took too long, and is refused for that. An SVG drawn in VOMD's own process
    /// (SVGs are not drawn apart, or a child couldn't be started for it) may not stop at once (its
    /// parse can't be): the other SVGs wait for it (`stuck`).
    #[cfg(feature = "svg")]
    fn let_go(&mut self, slow: bool) -> bool {
        let Some(w) = self.worker.take() else {
            return false;
        };
        w.cancel.store(true, Ordering::Relaxed);
        let key = w
            .shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .busy
            .clone();
        if let Some((key, ..)) = key
            && slow
            && !self.refused.iter().any(|r| r.0 == key)
        {
            self.refused.push((key, Refusal::Slow));
        }
        self.retired_cost();
        self.retired.push(w.shared.clone());
        if !svg::apart() || w.inline.load(Ordering::SeqCst) {
            self.stuck = Some(w.shared.clone());
        }
        // (Dropping it stops what is queued, not what runs.)
        drop(w);
        true
    }

    /// The SVG the worker is drawing, and the size, if it is drawing one.
    #[cfg(feature = "svg")]
    fn drawing_svg(&self) -> Option<(Key, u32, u32)> {
        let (key, w, h) = {
            let q = self.worker.as_ref()?.shared.0.lock();
            let q = q.unwrap_or_else(|e| e.into_inner());
            q.since?;
            let (key, w, h, _) = q.busy.as_ref()?;
            (key.clone(), *w, *h)
        };
        self.is_svg(&key).then_some((key, w, h))
    }

    /// Give an SVG `deadline` to draw instead of [`SVG_DEADLINE`], for tests.
    #[cfg(feature = "svg")]
    #[doc(hidden)]
    pub fn set_svg_deadline(&mut self, deadline: std::time::Duration) {
        self.svg_deadline = deadline;
    }

    /// Whether the image of `key` is an SVG, as far as is known: a local one's header, a remote
    /// one's body.
    #[cfg(feature = "svg")]
    fn is_svg(&self, key: &Key) -> bool {
        match key {
            Key::Local(id) => self
                .costs
                .get(id)
                .is_some_and(|c| c.is_some_and(|c| c.header.format == Format::Svg)),
            #[cfg(feature = "remote-images")]
            Key::Remote(url) => self
                .cache
                .peek_bytes(url)
                .and_then(|b| info_of(b))
                .is_some_and(|h| h.format == Format::Svg),
            #[cfg(not(feature = "remote-images"))]
            Key::Remote(_) => false,
        }
    }

    /// A worker let go for an SVG ([`Images::expire`]) is still drawing it, in VOMD's own process.
    #[cfg(feature = "svg")]
    #[doc(hidden)]
    pub fn svg_stuck(&mut self) -> bool {
        if self.stuck.as_ref().is_some_and(Worker::idle_of) {
            self.stuck = None;
        }
        self.stuck.is_some()
    }

    /// The faces SVG text is set in (`Fonts::svg_sources`), from the next job on: given again
    /// whenever the fonts change. (Images drawn already keep their text as it was.)
    #[cfg(feature = "svg")]
    pub fn set_svg_fonts(&mut self, fonts: crate::fonts::SvgFonts) {
        let changed = self
            .svg_fonts
            .as_ref()
            .is_some_and(|old| !same_fonts(old, &fonts));
        let fonts = Some(Arc::new(fonts));
        if let Some(w) = &self.worker {
            w.set_fonts(fonts.clone());
        }
        self.forced.fonts = fonts.clone();
        self.svg_fonts = fonts;
        // The SVGs drawn are drawn again, their text in the new faces, when they are next asked
        // for.
        if changed {
            let mut svgs: Vec<Key> = Vec::new();
            for s in &self.cache.slots {
                if s.pixels().is_some() && self.is_svg(&s.key) && !svgs.contains(&s.key) {
                    svgs.push(s.key.clone());
                }
            }
            if !svgs.is_empty() {
                self.cache
                    .retain(|s| s.pixels().is_none() || !svgs.contains(&s.key));
            }
            self.forced.forget_pixels();
        }
    }

    /// How many times the wake has been called (by the threads, or by [`Images::request`]): a
    /// window polls once for each wake it gets, so a state that is only reached by a poll
    /// nothing asked for is a lost wake. For the tests that stand in for the event loop.
    #[doc(hidden)]
    pub fn wakes(&self) -> usize {
        self.wakes.load(Ordering::SeqCst)
    }

    /// Fetch remote images from `body` instead of `curl` (see [`Remote::set_test_body`]).
    #[cfg(feature = "remote-images")]
    #[doc(hidden)]
    pub fn set_test_body(&mut self, body: crate::remote::TestBody) {
        self.remote.set_test_body(body);
    }

    pub fn set_base(&mut self, base: Option<PathBuf>) {
        self.base = base.map(Arc::from);
    }

    /// Start on a document (opened or reloaded) and its new layout: local images are decoded
    /// again (the files may have changed); remote ones stay cached by URL.
    ///
    /// What the document's images cost to keep is worked out from the numbers of its images
    /// ([`Images::tables_bytes`]): one [`Src`] each and, for a remote one, its place in the set of
    /// URLs and in the tables of what is known per URL, which are given the room they can ever
    /// need now, so that they never grow later. It is charged, and made room for (what is cached
    /// is evicted to it: nothing of the old document is being painted now), before any of it is
    /// made ([`Images::make_room`]), beside everything that can't be evicted: what the old
    /// document's decodes still hold (a result nobody has taken is taken now, the old document's is
    /// dropped), the fetchers and the bookkeeping. A document whose images cost more than the
    /// budget has none loaded: every image shows its alt text. One that fits when nothing is
    /// decoding, but not beside a decode still running, waits for it: its tables are made by the
    /// poll that takes the decode's result ([`Images::build_pending`]).
    pub fn attach(&mut self, doc: &Doc, layout: &mut Layout) {
        // Results for the old document are dropped from here on, and what is queued for it is not
        // run: the worker only finishes what it has begun.
        self.doc_gen += 1;
        // An SVG drawn apart for the old document is stopped (its process killed).
        #[cfg(feature = "svg")]
        if svg::apart() && self.drawing_svg().is_some() {
            self.let_go(false);
        }
        if let Some(w) = &self.worker {
            w.set(Vec::new());
        }
        // (A running decode's result is for the old document, and takes a place only if it is a
        // remote image's.)
        self.cache.expect = self.results_running();
        // A result nobody has taken holds its memory until it is: it is taken before anything is
        // made beside it (a decoded bitmap of the old document is dropped, a remote one is kept).
        self.drain_results();
        self.refused = Vec::new();
        self.partial = Vec::new();
        self.unbuilt = false;
        // A load past the limits for the old document stops; the images chosen are loaded again
        // when they are in view.
        #[cfg(feature = "images")]
        {
            self.detach_anims();
            self.forced.abandon();
            self.forced.stale();
            self.forced_ids = Vec::new();
            if !self.forced.is_unused() {
                let base = self.base.clone();
                for (i, img) in doc.images.iter().enumerate() {
                    if let Some(origin) = forced_origin(img, base.as_deref())
                        && self.forced.is_chosen(&origin)
                    {
                        self.forced_ids.push((i as u32 + 1, origin));
                    }
                }
            }
        }
        // What the old document had painted is not painted any more: room is made from it first.
        self.cache.frame += 1;
        self.cache.floor = 0;
        self.cache.retain(|s| matches!(s.key, Key::Remote(_)));
        // The last document's tables go before the new one's are made: they are never both held.
        self.srcs = Vec::new();
        self.costs = std::collections::HashMap::new();
        #[cfg(feature = "remote-images")]
        {
            self.urls = std::collections::HashSet::new();
            self.view_urls = std::collections::HashSet::new();
            self.remote_images = 0;
            self.in_view = Vec::new();
        }
        match self.admit_tables(doc) {
            Tables::Now => self.build_tables(doc, layout, true),
            Tables::Later => {
                self.unbuilt = true;
                self.unbuilt_tried = self.cache.frame;
                self.build_tables(doc, layout, false);
            }
            Tables::Never => self.build_tables(doc, layout, false),
        }
    }

    /// Take the results the threads have sent, for a document that is going: nothing of them is
    /// shown (no layout is changed), the decoded images of the document that went are dropped and
    /// every other result is taken as [`Images::poll`] takes it.
    fn drain_results(&mut self) {
        while let Ok(done) = self.rx.try_recv() {
            self.take_done(done, None);
        }
    }

    /// What the tables of a document of `images` images, `remote` of them remote, cost to keep
    /// (everything that grows with its images, and nothing else does: see the audit table): a
    /// [`Src`] each, and for the remote ones a place in the set of URLs ([`attach_bytes`]), in the
    /// table of what fetching found per URL and in the one of the bodies dropped for room (these
    /// two are given the room for every URL of the document at once and are never grown: only the
    /// document's URLs are recorded in them), and what a request builds for them while they are
    /// fetched ([`REQUEST_PER_REMOTE`], charged only once remote images are on, but made room for
    /// now, as it has to be there when they are).
    fn tables_bytes(&self, images: usize, remote: usize) -> usize {
        let own = attach_bytes(images, remote);
        #[cfg(feature = "remote-images")]
        let own = if remote == 0 {
            own
        } else {
            own.saturating_add(self.remote.sizes_bytes_for(remote))
                .saturating_add(table_bytes(remote.min(VIEW_MAX), size_of::<Arc<str>>()))
                .saturating_add(table_bytes(remote, DEFERRED_ENTRY))
                .saturating_add(remote.saturating_mul(REQUEST_PER_REMOTE))
        };
        own
    }

    /// Whether the tables of `doc` can be made: now, once a decode that is still running has
    /// ended, or never (their cost alone is over the budget, or over what is left of it when
    /// nothing is decoding or fetched). `Tables::Now` has made the room for them.
    fn admit_tables(&mut self, doc: &Doc) -> Tables {
        let images = doc.images.len();
        let remote = doc.images.iter().filter(|img| img.remote).count();
        let own = self.tables_bytes(images, remote);
        // What is held beside the cache is what the workers and the fetchers hold now.
        self.cache.reserve =
            self.retired_cost() + self.worker.as_ref().map_or(0, Worker::busy_cost);
        #[cfg(feature = "remote-images")]
        {
            self.cache.fetching = self.fetch_reserve(false);
        }
        if own > self.cache.cap {
            return Tables::Never;
        }
        // Making the tables takes a path for a moment: the longest one's worth.
        let scratch = doc
            .images
            .iter()
            .filter(|img| !img.remote)
            .map(|img| self.path_scratch(&img.src))
            .max()
            .unwrap_or(0);
        let need = own.saturating_add(scratch);
        if self.make_room("attach: tables", need) {
            Tables::Now
        } else if self.bookkeeping_bytes().saturating_add(need) <= self.cache.cap {
            Tables::Later
        } else {
            Tables::Never
        }
    }

    /// Make the tables of the document, or, with `loaded` false, none (every image shows its alt
    /// text), and set up what depends on them: the layout's probe, the set of URLs and the rest of
    /// what is kept per remote image. The room is made already ([`Images::admit_tables`]).
    fn build_tables(&mut self, doc: &Doc, layout: &mut Layout, loaded: bool) {
        if loaded {
            self.srcs = doc
                .images
                .iter()
                .map(|img| {
                    if img.remote {
                        // The document's own string: one copy of the URL, and it is the page's.
                        Src::Remote(img.src.clone())
                    } else if cfg!(feature = "images")
                        && local_path(&img.src, self.base.as_deref()).is_some()
                    {
                        // (Its path is worked out again when it is wanted, and not kept.)
                        Src::File(img.src.clone())
                    } else {
                        Src::None
                    }
                })
                .collect();
        }
        if cfg!(feature = "images") {
            let base = self.base.clone();
            // What loads past the limits, or failed to, is sized by that.
            #[cfg(feature = "images")]
            let forced = self.forced_sizes(doc);
            #[cfg(not(feature = "images"))]
            let forced: Vec<(Arc<str>, ImageSize)> = Vec::new();
            layout.set_image_probe(
                doc,
                Some(Box::new(move |img: &ImageRef| {
                    if !loaded {
                        ImageSize::Alt
                    } else if let Some((_, size)) = forced.iter().find(|f| f.0 == img.src) {
                        *size
                    } else {
                        probe(img, base.as_deref())
                    }
                })),
            );
        }
        #[cfg(feature = "remote-images")]
        {
            // Nothing is in view of the new document yet: the bodies that waited are let go.
            self.in_view.clear();
            // Made at the size it was charged for: one table, no growing.
            let remote = if loaded { doc.remote_image_count() } else { 0 };
            let mut urls = std::collections::HashSet::with_capacity(remote);
            urls.extend(self.srcs.iter().filter_map(|s| match s {
                Src::Remote(url) => Some(url.clone()),
                _ => None,
            }));
            self.urls = urls;
            self.remote_images = self.urls.len();
            // (Charged with the tables: [`Images::tables_bytes`].)
            self.view_urls =
                std::collections::HashSet::with_capacity(self.urls.len().min(VIEW_MAX));
            // Nothing is wanted of the new document yet: the first request says.
            self.heavy_wanted = false;
            self.held = None;
            self.admit_waiting();
            // (The tables of per-URL records are made room for only with the document's tables:
            // when it has none, nothing is made.)
            self.prune_urls(loaded.then_some(remote));
            self.sync_remote(doc, layout);
            if self.settle_exclusive() {
                (self.wake)();
            }
            self.sync_pause(false);
        }
        self.charge_bookkeeping();
    }

    /// The tables of a document that waited for a decode ([`Images::unbuilt`]) are made as soon as
    /// they fit beside what is still held. Returns true if they were (the layout has changed).
    fn build_pending(&mut self, doc: &Doc, layout: &mut Layout) -> bool {
        match self.admit_tables(doc) {
            Tables::Now => {
                self.unbuilt = false;
                self.build_tables(doc, layout, true);
                true
            }
            Tables::Later => {
                self.unbuilt_tried = self.cache.frame;
                false
            }
            Tables::Never => {
                // Whatever was in the way is gone and it still doesn't fit: it is not coming.
                self.unbuilt = false;
                false
            }
        }
    }

    /// Where the budget is: what the cache holds and what is set aside beside it (the memory
    /// tests print this when they fail).
    #[doc(hidden)]
    pub fn memory_report(&self) -> String {
        let c = &self.cache;
        #[cfg(feature = "remote-images")]
        let remote = format!(
            "paused {}, exclusive {:?}, held {}, waiting {}, in flight {:?}; {}",
            self.remote.paused(),
            self.exclusive,
            self.remote.held_bytes(),
            self.waiting.len(),
            self.remote.transfers(),
            self.remote.pool_state(),
        );
        #[cfg(not(feature = "remote-images"))]
        let remote = String::new();
        // (The biggest few, and a count of the rest: a cache of thousands of slots is no report.)
        let mut by_cost: Vec<&Slot> = c.slots.iter().collect();
        by_cost.sort_by_key(|s| std::cmp::Reverse(s.cost()));
        let mut slots: Vec<String> = by_cost
            .iter()
            .take(8)
            .map(|s| {
                format!(
                    "{}{}{} {}",
                    match &s.data {
                        Data::Pixels(b) => format!("pixels {}x{}", b.width, b.height),
                        Data::Bytes(_) => "bytes".to_string(),
                        #[cfg(feature = "images")]
                        Data::Anim(p) => format!("animation {}x{}", p.size.0, p.size.1),
                    },
                    if s.unevictable(c.frame) { " kept" } else { "" },
                    if s.pinned() { " pinned" } else { "" },
                    s.cost()
                )
            })
            .collect();
        if by_cost.len() > 8 {
            let kept = c.slots.iter().filter(|s| s.unevictable(c.frame)).count();
            slots.push(format!(
                "and {} more, {kept} kept in all",
                by_cost.len() - 8
            ));
        }
        let worker = self.worker.as_ref().map_or_else(String::new, |w| {
            let (lock, _) = &*w.shared;
            let q = lock.lock().unwrap_or_else(|e| e.into_inner());
            format!(
                "worker: {} queued, busy {:?}",
                q.jobs.len(),
                q.busy.as_ref().map(|b| (&b.0, b.1, b.2, b.3))
            )
        });
        format!(
            "cache {} in {} slots ({}), decode {}, fetchers {}, bookkeeping {}, retired {}; {worker}; {remote}",
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

    /// Whether image `id` is still to be shown: it is a file, or a remote image that remote images
    /// being on can fetch and that has not failed (so it is decoded, or will be, and when nothing
    /// is left to do it is decoded: the tests that look for work that never happens).
    #[doc(hidden)]
    pub fn is_waiting(&self, id: u32) -> bool {
        match self.src(id) {
            Src::None => false,
            Src::File(_) => true,
            #[cfg(feature = "remote-images")]
            Src::Remote(url) => self.remote.on && self.remote.size(url) != Some(None),
            #[cfg(not(feature = "remote-images"))]
            Src::Remote(_) => false,
        }
    }

    /// Whether the tables of the document are waiting for a decode ([`Images::attach`]).
    #[doc(hidden)]
    pub fn tables_waiting(&self) -> bool {
        self.unbuilt
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
            // The path is worked out for the lookup of the file's header, and dropped.
            Src::File(src) => {
                let base = self.base.as_deref();
                self.costs
                    .entry(id)
                    .or_insert_with(|| local_path(&src, base).and_then(|p| file_info(&p)))
                    .map_or(result, |info| info.cost(w, h))
            }
            #[cfg(feature = "remote-images")]
            Src::Remote(url) if self.remote.on => {
                // Fetched before, and the body is gone: what its decode may hold is not what it
                // holds when the body is back (the whole frame, 12 MB for a JPEG of 2 MP), and
                // an image near the viewport was wanted without it, fetched, found too big, and
                // let go (its body is nobody's, and evicted), and wanted again without it:
                // fetched, found too big...
                let unfetched = || match self.remote.size(&url) {
                    Some(Some((sw, sh))) => {
                        let source = self.remote.source_cost(&url).unwrap_or(0);
                        let box_costs = self.remote.box_costs(&url);
                        Some(unfetched_cost(source, box_costs, (sw, sh), w, h))
                    }
                    _ => None,
                };
                match self.cache.peek_bytes(&url).and_then(|b| info_of(b)) {
                    // What a job of an SVG measured is in its record, not in its bytes.
                    #[cfg(feature = "svg")]
                    Some(header) if header.format == Format::Svg => {
                        decode_cost(&header, w, h).max(unfetched().unwrap_or(0))
                    }
                    Some(header) => decode_cost(&header, w, h),
                    None => unfetched().unwrap_or(result),
                }
            }
            _ => result,
        }
    }

    /// A new frame is being painted: what it paints is the most recently used.
    pub fn begin_frame(&mut self) {
        self.cache.frame += 1;
        #[cfg(feature = "images")]
        self.anims.placed.clear();
    }

    /// Ask for the boxes near the viewport, most wanted first: (image id, width, height) in
    /// physical pixels, the first `on_screen` of them in view. Images not decoded at that size
    /// are queued for the worker, replacing what it hadn't started. With remote images on, URLs
    /// not fetched yet are queued for the fetchers: these boxes first, then the rest of the
    /// document's in order.
    ///
    /// The ones in view are always wanted; of the rest only as many as fit the cache beside
    /// them are, and those already decoded are kept while they are (the last to be evicted, and
    /// protected for the length of this request: only a frame that painted pixels keeps them
    /// from being evicted by anything else, [`Slot::unevictable`]). Otherwise a screen of images
    /// bigger than the cache would evict each other, decoded again and again.
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
        // The images chosen to load past the limits go to their own thread, outside the budget:
        // what the rest are asked for is as if they weren't there.
        #[cfg(feature = "images")]
        if !self.forced_ids.is_empty() {
            let mut wants = Vec::new();
            let (mut normal, mut shown) = (Vec::with_capacity(boxes.len()), 0);
            for (n, &(id, w, h)) in boxes.iter().enumerate() {
                let Some((_, origin)) = self.forced_ids.iter().find(|f| f.0 == id) else {
                    normal.push((id, w, h));
                    shown += usize::from(n < on_screen);
                    continue;
                };
                // A remote image whose size isn't known yet is fetched for it first.
                let size = match origin {
                    forced::Origin::Url(u) if self.forced.natural(u).is_none() => None,
                    _ => Some(clamp_box(w, h)),
                };
                if !wants.iter().any(|(o, _)| o == origin) {
                    wants.push((origin.clone(), size));
                }
            }
            #[cfg(feature = "remote-images")]
            {
                self.forced.fetch = Some(self.remote.forced_fetch.clone());
            }
            self.forced
                .request(wants, self.doc_gen, &self.tx, &self.wake);
            return self.request_normal(&normal, shown);
        }
        self.request_normal(boxes, on_screen);
    }

    /// [`Images::request`] for the images that aren't chosen to load past the limits.
    fn request_normal(&mut self, boxes: &[(u32, u32, u32)], on_screen: usize) {
        if self.unbuilt {
            // A frame is a change of state that can make the tables buildable (what the old
            // document painted may be evicted from it on, and nothing else wakes the window for
            // that): once for each, the poll tries again, and a repaint follows if it does.
            if self.unbuilt_tried != self.cache.frame {
                self.unbuilt_tried = self.cache.frame;
                (self.wake)();
            }
            return;
        }
        // The bookkeeping is paid for out of the same budget, and what this request builds beside
        // it too: the jobs (a `Vec` doubles as it grows), the paths they copy, the table of the
        // files' sizes it grows, the lists of URLs. All of it is worked out from the boxes and
        // made room for before any of it is made: as many of the boxes, most wanted first, as the
        // budget has room for beside what can't be evicted (the rest are asked for by a later
        // request, when a decode's result is taken or a frame has passed).
        // An SVG drawn apart at a size its box no longer has is stopped (its process killed): it
        // is asked for at the new one.
        #[cfg(feature = "svg")]
        if svg::apart()
            && let Some((key, w, h)) = self.drawing_svg()
        {
            let sizes = || {
                (boxes.iter())
                    .filter(|b| self.key(b.0).as_ref() == Some(&key))
                    .map(|b| clamp_box(b.1, b.2))
            };
            if sizes().next().is_some() && sizes().all(|s| s != (w, h)) {
                self.let_go(false);
            }
        }
        self.refresh_reserve();
        #[cfg(feature = "remote-images")]
        {
            self.forget_evicted();
            self.trim_records();
        }
        // What the request will keep is protected before it is worked out how many boxes it has
        // room for, so that what it can't evict is not counted as room: asked in the other order
        // the protection took the room away and the request, refused, returned with nothing asked
        // for, and an image admitted to be decoded alone sat in the cache with no job.
        // (The images in view are decided once, here, and by this alone: every place below
        // that asks whether an image is in view, this request's and the admission of a body's,
        // goes by the same set.)
        let view = self.view_of(boxes, on_screen);
        let on_screen = view.len;
        self.protect_wanted(boxes, on_screen);
        // (The viewport is the whole of what the window shows, whichever of the boxes this request
        // is made for; what the request keeps is kept while the room for its lists is made.)
        self.note_viewport(boxes, view);
        let running = self.results_running();
        let mut most = usize::MAX;
        let fit = loop {
            self.extra = 0;
            let fit = self.request_fit(boxes, most);
            self.extra = fit.allowance;
            // (The room for the results is kept while room is made: a trim would give it back.)
            self.cache.expect = running + fit.results;
            if fit.allowance == 0 || self.make_room("request", 0) {
                break fit;
            }
            // Not so by what `request_fit` worked out, which is all that can ever be made: a
            // request for fewer boxes is tried, down to what `REQUEST_FREE` lets in whatever the
            // room is, so that there is always a request.
            if fit.allowance <= REQUEST_FREE + SLOTS_FREE {
                break fit;
            }
            most = fit.allowance / 2;
        };
        self.cache.unprotect();
        let (boxes, on_screen) = (&boxes[..fit.boxes], on_screen.min(fit.boxes));
        let local = fit.local;
        // (The work the allowance is for: an image evicted since was cached when it was worked
        // out, and is asked for by the next request.)
        let mut work = fit.work;
        // The results go in places made ready for them, not in the room a result finds (and
        // drops the result for want of).
        self.cache.reserve_slots(if fit.results > 0 {
            fit.results + running
        } else {
            0
        });
        // The table of the files' sizes grows (a new one is made beside the old) for the local
        // images asked for, and is cut to them once it has twice as many: it holds the images of
        // the last requests, not the document's.
        if local > 0 {
            if self.costs.len() > 2 * local + 16 {
                let mut kept = std::collections::HashMap::with_capacity(local);
                for &(id, w, h) in boxes {
                    // (Only what is asked for needs its entry: a cached image's is not looked at.)
                    if kept.len() < local
                        && let Some(key) = self.key(id)
                        && !self.cache.has(&key, clamp_box(w, h).0, clamp_box(w, h).1)
                        && let Some(info) = self.costs.remove(&id)
                    {
                        kept.insert(id, info);
                    }
                }
                self.costs = kept;
            }
            self.costs.reserve(local);
        }
        let mut jobs = Vec::new();
        #[cfg(feature = "remote-images")]
        let mut fetch: Vec<Arc<str>> = Vec::new();
        #[cfg(feature = "remote-images")]
        let mut deferred: Vec<Arc<str>> = Vec::new();
        #[cfg(feature = "remote-images")]
        let mut anim_bodies: Vec<Key> = Vec::new();
        let mut wanted = 0usize;
        // The bodies that decodes read are held in the cache beside them (a job's source is an
        // `Arc` of the slot's bytes: [`Slot::pinned`]), so what is kept for the images near the
        // viewport is kept beside them too.
        let mut bodies = self.cache.pinned_cost();
        // Decodes of replaced workers still running hold memory beside the current worker's.
        let retired = self.retired_cost();
        let mut reserve = retired + self.worker.as_ref().map_or(0, Worker::busy_cost);
        // What is set aside for the fetchers, if any are (or are about to be) running.
        #[cfg(feature = "remote-images")]
        let mut starting = self.unfetched() || self.needs_fetch(boxes, on_screen);
        #[cfg(feature = "remote-images")]
        let fetching;
        #[cfg(not(feature = "remote-images"))]
        let fetching = 0;
        // What can't be evicted, and what the fetchers would be given if they weren't held off:
        // a job that doesn't fit beside them can only run with them held off.
        #[cfg(feature = "remote-images")]
        let (kept, fetch_static) = (
            self.cache.kept(),
            self.fetch_static(self.remote.busy(), starting),
        );
        #[cfg(feature = "remote-images")]
        let (mut heavy_wanted, mut heavy_chosen) = (false, false);
        #[cfg(feature = "remote-images")]
        let mut held: Option<Key> = None;
        for (n, &(id, w, h)) in boxes.iter().enumerate() {
            let Some(key) = self.key(id) else {
                continue;
            };
            let (w, h) = clamp_box(w, h);
            let cached = self.cache.has(&key, w, h);
            // The result is held beside the cache too, until it goes in.
            let cost = if cached { 0 } else { self.cost(id, w, h) };
            // What the fetchers are given whether they are held off now or not (a decision that
            // followed the pause would flip with it: an image near that is decoded while they
            // are held off, and evicted when they go on, and fetched and decoded again).
            // (An image that has to be fetched first is judged with them running: it starts them.)
            #[cfg(feature = "remote-images")]
            let room_fetch = {
                let fetches = !cached
                    && matches!(self.src(id), Src::Remote(url)
                        if self.remote.on
                            && self.remote.size(url) != Some(None)
                            && self.cache.peek_bytes(url).is_none());
                self.fetch_static(self.remote.busy() || fetches, starting || fetches)
            };
            #[cfg(not(feature = "remote-images"))]
            let room_fetch = 0;
            // (Working out a local image's path takes memory for a moment, taken off first.)
            let scratch = match self.src(id) {
                Src::File(src) => self.path_scratch(src),
                _ => 0,
            };
            // (What the request needs while it runs is taken off as well: it is held with what
            // is kept, so what is kept is what fits beside it, and the same images are wanted by
            // the next request, which needs the same: none is evicted to make room for it and
            // decoded again.)
            let room = (self.cache.cap.saturating_sub(room_fetch))
                .saturating_sub(self.cache.overhead)
                .saturating_sub(scratch)
                .saturating_sub(bodies)
                .saturating_sub(reserve.max(retired.saturating_add(cost)));
            // Kept only if it fits with the ones before it (one that crosses the line is the one
            // that doesn't: the budget is not gone over by a box).
            let bytes = w as usize * h as usize * 4;
            let over = n >= on_screen && wanted.saturating_add(bytes) > room;
            wanted = wanted.saturating_add(bytes);
            if over {
                continue;
            }
            if cached {
                self.cache.touch(&key, w, h);
                #[cfg(feature = "remote-images")]
                if n < on_screen {
                    anim_bodies.push(key);
                }
                continue;
            }
            // The allowance is for as much work as was worked out.
            if work == 0 {
                continue;
            }
            work -= 1;
            // (Only a remote image can be asked for twice, by two ids with one URL.)
            if matches!(key, Key::Remote(_))
                && jobs
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
            // An SVG waits while one given up is still being drawn.
            #[cfg(feature = "svg")]
            if self.svg_stuck() && self.is_svg(&key) {
                continue;
            }
            #[cfg(feature = "remote-images")]
            let mut fresh_body = false;
            #[cfg(not(feature = "remote-images"))]
            let fresh_body = false;
            let source = match self.src(id).clone() {
                // (Resolved when the job is made, and the copy belongs to the job: it is charged
                // when the job is queued, below. The one being made is in the request's scratch.)
                Src::File(src) => match local_path(&src, self.base.as_deref()) {
                    Some(path) => Source::File(path),
                    None => continue,
                },
                // One that failed (fetched, or decoded to nothing) is not tried again while the
                // document is open, whatever body is still held: two jobs for it, another size
                // each, pin its body for each other, and would fail one after the other for good.
                #[cfg(feature = "remote-images")]
                Src::Remote(url) if self.remote.on && self.remote.size(&url) == Some(None) => {
                    continue;
                }
                #[cfg(feature = "remote-images")]
                Src::Remote(url) if self.remote.on => match self.cache.peek_bytes(&url).cloned() {
                    Some(bytes) => {
                        // (A body no job holds yet is held by this one from now on.)
                        fresh_body = Arc::strong_count(&bytes) == 2;
                        Source::Bytes(bytes)
                    }
                    None => {
                        // Not fetched yet, or its bytes were evicted: fetch it (again). One
                        // dropped for lack of room, whose image isn't in view, waits to see
                        // if there is room once what is wanted this frame is known.
                        if self.remote.size(&url) != Some(None) {
                            if n >= on_screen && self.deferred_need(&url).is_some() {
                                deferred.push(url);
                            } else {
                                fetch.push(url);
                                starting = true;
                            }
                        }
                        continue;
                    }
                },
                _ => continue,
            };
            // A decode that fits the cache by itself but not beside what the document's images
            // cost to keep (their sources, URLs and records) can't run in the budget at all, and
            // is not tried: the image shows its alt text. (One bigger than the cache by itself is
            // another matter: it runs alone, over the budget by its own size.)
            let own = cost
                .saturating_add(body_cost(&key, &source))
                .saturating_add(path_cost(&source));
            if own <= self.cache.cap && !self.fits_bookkeeping(own) {
                if !self.refused.iter().any(|r| r.0 == key) {
                    self.refused.push((key, Refusal::Memory(own as u64)));
                }
                continue;
            }
            // An SVG whose drawing needs more than the budget by itself is not drawn (a raster
            // image's is bounded by its pixels, and runs alone, below): a click draws it anyway.
            #[cfg(feature = "svg")]
            if cost > self.cache.cap && self.is_svg(&key) {
                if !self.refused.iter().any(|r| r.0 == key) {
                    self.refused.push((key, Refusal::Memory(own as u64)));
                }
                continue;
            }
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
                    held.get_or_insert(key);
                    continue;
                }
                self.exclusive = Some(key.clone());
                heavy_chosen = true;
            }
            // A body no job holds yet is pinned by this one (it can't be evicted until the decode
            // is done), and a decode that is running or queued holds its own memory beside the
            // cache: a body that doesn't fit beside them is left to the request that follows the
            // decode's result, whatever its image's place (an in-view image is wanted, not at any
            // price: queued behind a 21 MB decode, four bodies of 3.5 MB each that eviction could
            // have made room from went 5 MB over the budget).
            #[cfg(feature = "remote-images")]
            if fresh_body && (reserve > 0 || !jobs.is_empty()) && body_cost(&key, &source) > room {
                continue;
            }
            // What the job holds is held for it from now on: the body is in use.
            #[cfg(feature = "remote-images")]
            if let Key::Remote(url) = &key {
                self.cache.bytes_of(url);
            }
            reserve = reserve.max(retired.saturating_add(cost));
            if fresh_body {
                bodies = bodies.saturating_add(body_cost(&key, &source));
            }
            // (The path the job carries is held by the queue from now on, beside the old queue's:
            // it is in the allowance of the request, [`Images::request_fit`].)
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
        for key in anim_bodies {
            if let Some(url) = self.body_for_anim(&key, reserve) {
                fetch.push(url);
                starting = true;
            }
        }
        #[cfg(feature = "remote-images")]
        for url in deferred {
            let fits = self.deferred_need(&url).is_some_and(|(cost, decode)| {
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
            self.heavy_wanted = heavy_wanted;
            self.held = held;
            self.held_due = false;
            fetching = self.fetch_reserve(starting);
        }
        self.cache.reserve = reserve;
        // The results to come are the jobs queued and the one running: what was set for room is
        // what was asked for in the end.
        self.cache.expect = running + jobs.len();
        // The cache is made room for them before they are let start: they can't take more. (Each
        // decode was judged to fit when it was chosen; this makes the room for the whole.)
        self.cache.fetching = fetching;
        self.make_room("request: decodes and fetchers", 0);
        if !jobs.is_empty() && self.worker.is_none() {
            let worker = Worker::spawn(self.tx.clone(), self.wake.clone());
            #[cfg(feature = "svg")]
            worker.set_fonts(self.svg_fonts.clone());
            self.worker = Some(worker);
        }
        if let Some(w) = &self.worker {
            // The worker may have started a job of the queue this replaces after the reserve was
            // worked out from what it held: that decode runs whatever the new queue is, and is
            // kept room for like any other.
            let started = w.set(jobs);
            self.cache.reserve = self.cache.reserve.max(retired.saturating_add(started));
        }
        #[cfg(feature = "remote-images")]
        {
            // The job a body let in was waiting for has been asked for, or isn't wanted now.
            self.awaiting = false;
            if self.settle_exclusive() {
                (self.wake)();
            }
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
        // What can't run in the budget shows its alt text: the next poll does it.
        if !self.refused.is_empty() {
            (self.wake)();
        }
        self.extra = 0;
        self.charge_bookkeeping();
    }

    /// Mark what the request for `boxes` will keep ([`Cache::protect`]): the cached images in
    /// view, and those near that fit beside what is in front of them, by the same count as the
    /// request's own, which has more to take off (the decode of the one asked for, the paths): so
    /// this keeps no less, and no more than the budget has room for.
    fn protect_wanted(&mut self, boxes: &[(u32, u32, u32)], on_screen: usize) {
        self.charge_bookkeeping();
        let retired = self.retired_cost();
        let reserve = retired + self.worker.as_ref().map_or(0, Worker::busy_cost);
        #[cfg(feature = "remote-images")]
        let fetch = self.fetch_static(
            self.remote.busy(),
            self.unfetched() || self.needs_fetch(boxes, on_screen),
        );
        #[cfg(not(feature = "remote-images"))]
        let fetch = 0;
        let room = self
            .cache
            .cap
            .saturating_sub(fetch)
            .saturating_sub(self.cache.overhead)
            .saturating_sub(self.cache.pinned_cost())
            .saturating_sub(reserve);
        let mut wanted = 0usize;
        for (n, &(id, w, h)) in boxes.iter().enumerate() {
            let Some(key) = self.key(id) else {
                continue;
            };
            let (w, h) = clamp_box(w, h);
            let bytes = w as usize * h as usize * 4;
            let over = n >= on_screen && wanted.saturating_add(bytes) > room;
            wanted = wanted.saturating_add(bytes);
            if !over {
                self.cache.protect(&key, w, h);
            }
        }
    }

    /// How many of `boxes` (most wanted first) a request can be made for, and what asking for
    /// them allocates at most, beside what no eviction frees: the jobs and lists a request builds
    /// ([`REQUEST_PER_BOX`], [`REQUEST_PER_REMOTE_BOX`]), the table of the files' sizes grown for
    /// the local ones, the path each of them copies for its job and the one being worked out
    /// ([`Images::path_scratch`]). It is worked out before anything is allocated, and the room for
    /// it made ([`Images::make_room`]); boxes beyond what the budget could ever have room for are
    /// not asked for (a window's few dozen always are: the number is for a request of thousands of
    /// tiny images beside a bitmap that fills the budget).
    ///
    /// Only the work a request does is charged: a box already decoded at its size costs nothing
    /// (it is touched, and for a remote one in view an entry of the list of those), so a prefix of
    /// cached images never uses up what the later ones need, and each request goes on where the
    /// last had to stop: the images it asked for are cached by the next, and cost nothing then.
    fn request_fit(&mut self, boxes: &[(u32, u32, u32)], most: usize) -> Fit {
        let limit = self.room_limit().min(most);
        let entry = size_of::<(u32, Option<FileInfo>)>();
        let base = self.base.as_ref().map_or(0, |b| b.as_os_str().len());
        let (mut work, mut local, mut paths, mut scratch) = (0usize, 0usize, 0usize, 0usize);
        let (mut remote, mut results) = (0usize, 0usize);
        // The results still to come take slots too (the running job's, and a replaced worker's).
        let running = self.results_running();
        let mut fit = Fit::default();
        for (k, &(id, w, h)) in boxes.iter().enumerate() {
            if let Some(key) = self.key(id) {
                let (w, h) = clamp_box(w, h);
                let cached = self.cache.has(&key, w, h);
                match self.src(id) {
                    Src::Remote(_) if cached => {}
                    Src::Remote(url) => {
                        work += 1;
                        remote += 1;
                        #[cfg(feature = "remote-images")]
                        {
                            results += usize::from(self.cache.peek_bytes(url).is_some());
                        }
                        #[cfg(not(feature = "remote-images"))]
                        let _ = url;
                    }
                    Src::File(src) if !cached => {
                        work += 1;
                        local += 1;
                        results += 1;
                        paths += src.len().min(MAX_SRC) + base + STR_OVERHEAD;
                        scratch = scratch.max(self.path_scratch(src));
                    }
                    _ => {}
                }
            }
            // (The list of those in view is no part of it: it is the viewport's, made whole whichever
            // boxes the request is for, [`Images::note_viewport`].)
            #[cfg(feature = "remote-images")]
            let remote_bytes = remote.saturating_mul(REQUEST_PER_REMOTE_BOX);
            #[cfg(not(feature = "remote-images"))]
            let remote_bytes = {
                let _ = remote;
                0
            };
            // (A table that has room for them as it is allocates nothing; one that has not is
            // made again, with at least what it holds and more.)
            let need = self.costs.len().saturating_add(local);
            let costs = if local == 0 || need <= self.costs.capacity() {
                0
            } else {
                table_bytes(need.max(self.costs.capacity().saturating_add(1)), entry)
            };
            // The slots the results will go in: the list has the room for them or is made larger
            // (a result that has no place to go can only be dropped).
            let slots = self
                .cache
                .reserve_bytes(if results > 0 { results + running } else { 0 });
            let bytes = work
                .saturating_mul(REQUEST_PER_BOX)
                .saturating_add(remote_bytes)
                .saturating_add(costs)
                .saturating_add(paths)
                .saturating_add(scratch)
                .saturating_add(slots)
                .saturating_add(if local > 0 { FILE_LOOKUP } else { 0 });
            if bytes > limit.max(REQUEST_FREE + SLOTS_FREE) {
                break;
            }
            fit = Fit {
                boxes: k + 1,
                allowance: bytes,
                work,
                local,
                results,
            };
        }
        fit
    }

    /// The images in view: the leading boxes of `boxes` (at most the first `on_screen`) that hold
    /// at most [`VIEW_MAX`] images *still to be loaded*, a remote image counted once however many
    /// times the document refers to it (4,096 references to one URL are one image). An image
    /// that is resolved ([`Images::resolved`]: decoded at its box, or shown as its alt text for
    /// good) has nothing left to ask for and takes no place of the cap, so the view advances past
    /// what has loaded: 4,500 distinct images that all fit a screen are admitted 4,096 at a time,
    /// the next ones as those decode (an image that was beyond the cap was never admitted, and a
    /// heavy one after 4,500 small ones stayed unloaded for good). It only advances as images
    /// resolve, each of which is an event (a decode or a failure taken), so a request that
    /// changes nothing leaves the view as it was. The first box that would be unresolved image
    /// number `VIEW_MAX + 1` ends the view: that box and the ones after are not in view, for
    /// every purpose, so a body that comes for one of them is held or dropped by the rule for
    /// what is near and not in view (dropped for room, it is fetched again when there is room,
    /// not at every request), never by that for what is in view in one place and not in view in
    /// another (a viewport of references to one URL followed by a heavy image fetched it at
    /// every request, 784 times in 8 s).
    ///
    /// The set of URLs is [`Images::view_urls`], made with the document's tables (room made and
    /// charged then, for the most it can hold: the document's distinct URLs, at most `VIEW_MAX`):
    /// a request allocates nothing here, and so has no room to make before it has protected what
    /// it keeps (a set sized by the references in view, charged and then allocated in the middle
    /// of a request, took 139,280 bytes the budget had not made room for).
    fn view_of(&mut self, boxes: &[(u32, u32, u32)], on_screen: usize) -> View {
        let shown = on_screen.min(boxes.len());
        #[cfg(feature = "remote-images")]
        let mut urls = std::mem::take(&mut self.view_urls);
        #[cfg(feature = "remote-images")]
        urls.clear();
        let mut view = View {
            len: shown,
            places: 0,
        };
        for (n, b) in boxes[..shown].iter().enumerate() {
            let src = self.src(b.0);
            match src {
                Src::None => continue,
                #[cfg(feature = "remote-images")]
                Src::Remote(url) if urls.contains(url) => continue,
                _ => {}
            }
            if self.resolved(b.0, b.1, b.2) {
                continue;
            }
            if view.places == VIEW_MAX {
                view.len = n;
                break;
            }
            view.places += 1;
            #[cfg(feature = "remote-images")]
            if let Src::Remote(url) = self.src(b.0) {
                urls.insert(url.clone());
            }
        }
        #[cfg(feature = "remote-images")]
        {
            self.view_urls = urls;
        }
        view
    }

    /// Whether image `id` at the box `w` × `h` has nothing left to ask for: it is decoded at that
    /// size, or it can't be shown (its alt text is, for good: a remote image whose fetch or decode
    /// failed while the document is open, one with remote images off, or one with no source).
    /// Only these leave the set of the images in view ([`Images::view_of`]); an image evicted is
    /// to be loaded again, and is in it again.
    fn resolved(&self, id: u32, w: u32, h: u32) -> bool {
        let Some(key) = self.key(id) else {
            return true;
        };
        if let Key::Remote(url) = &key {
            #[cfg(feature = "remote-images")]
            if self.remote.on && self.remote.size(url) != Some(None) {
                let (w, h) = clamp_box(w, h);
                return self.cache.has(&key, w, h);
            }
            let _ = url;
            return true;
        }
        let (w, h) = clamp_box(w, h);
        self.cache.has(&key, w, h)
    }

    /// Note the viewport, whole: the images in view (`view`, [`Images::view_of`]: at most
    /// [`VIEW_MAX`]) are wanted whether or not this request has the room to ask for all of them (a
    /// request is made for as many boxes as fit, [`Images::request_fit`], and the rest are asked
    /// for by the next), so the list of the remote ones is made of all of them ([`Images::in_view`]:
    /// a body that arrives for one waits for room, and is not dropped, and it was, and fetched again,
    /// every time, when the request had room for 60 of the 600 in view and listed those), and the
    /// list of slots is given a place for each of them beside the slots it holds, so that a result
    /// or a body is never turned away for want of one (a list that doubles beside the old one costs
    /// more than a request may take when the budget is full of what a window paints). These two are the
    /// window's own, the size of what it shows: the room for them is made first and they are made
    /// whatever the answer, as the pixels they are for are painted whatever the budget.
    #[cfg_attr(not(feature = "remote-images"), allow(unused_variables))]
    fn note_viewport(&mut self, boxes: &[(u32, u32, u32)], view: View) {
        let bytes = self.cache.reserve_bytes(view.places);
        if bytes > 0 {
            self.make_room("request: the viewport", bytes);
        }
        self.cache.set_floor(view.places);
        #[cfg(feature = "remote-images")]
        {
            self.in_view.clear();
            if self.view_urls.len() > self.in_view.capacity() {
                self.make_room(
                    "request: the viewport",
                    self.view_urls
                        .len()
                        .saturating_mul(size_of::<(Arc<str>, u32, u32)>()),
                );
                self.in_view.reserve_exact(self.view_urls.len());
            }
            // (Out of `self` for the loop only, which allocates nothing but its pushes, made room
            // for above.)
            let mut urls = std::mem::take(&mut self.view_urls);
            // (Each URL is listed once, with the box of the first reference to it.)
            for &(id, w, h) in &boxes[..view.len] {
                // (A reference that is resolved is not the one the image is wanted at.)
                if let Src::Remote(url) = self.src(id)
                    && urls.contains(url)
                    && !self.resolved(id, w, h)
                    && urls.remove(url)
                {
                    let (w, h) = clamp_box(w, h);
                    let url = url.clone();
                    self.in_view.push((url, w, h));
                }
            }
            shrink_vec(&mut self.in_view);
            // (What is left in it is the resolved ones: it is kept empty, and its room.)
            urls.clear();
            self.view_urls = urls;
        }
    }

    /// The results that are still to come back from the workers, each of which takes a slot: the
    /// job running (or whose result is not taken) and one for each replaced worker.
    fn results_running(&self) -> usize {
        self.worker.as_ref().map_or(0, |w| usize::from(!w.idle())) + self.retired.len()
    }

    /// What working out the path of a local image whose `src` is `len` bytes takes at most beside
    /// what is kept: the percent-decoded copy and the joined one (and the document's directory),
    /// for as long as it takes. It is taken off the room before the path is made.
    fn path_scratch(&self, src: &str) -> usize {
        let base = self.base.as_ref().map_or(0, |b| b.as_os_str().len());
        src.len()
            .min(MAX_SRC)
            .saturating_mul(2)
            .saturating_add(base)
            .saturating_add(2 * STR_OVERHEAD)
    }

    /// Set what the cache keeps free for decoding to what the workers have: the largest job
    /// queued, running or with its result untaken, and the replaced workers' decodes. (It was set
    /// by the last request, and stays whatever the jobs have done since: it only says too much,
    /// and not for long, but an operation that evicts before a request has run should not be
    /// held to a decode that is over.)
    fn refresh_reserve(&mut self) {
        self.cache.reserve = self
            .retired_cost()
            .saturating_add(self.worker.as_ref().map_or(0, Worker::pending_cost));
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
        self.refresh_reserve();
        self.charge_bookkeeping();
        let mut any = self.show_refused(doc, layout) | std::mem::take(&mut self.repaint);
        #[cfg(feature = "remote-images")]
        let paused = self.remote.paused();
        while let Ok(done) = self.rx.try_recv() {
            // A frame made ahead shows when it is due ([`Images::anim_deadline`]): only an
            // animation's first frame needs a repaint now.
            #[cfg(feature = "images")]
            if let Done::Anim { serial, output } = done {
                any |= self.anim_done(serial, output);
                continue;
            }
            any = true;
            self.take_done(done, Some((doc, &mut *layout)));
        }
        // What decoding holds is what the decodes still to come hold: the results just taken are
        // in the cache, and the room that is made from here on (the fetchers let go on) is not made
        // beside a decode that is over, out of the results that are not painted yet.
        self.refresh_reserve();
        // The decode the tables of the document were waiting for may be over.
        if self.unbuilt && self.build_pending(doc, layout) {
            any = true;
        }
        #[cfg(feature = "remote-images")]
        {
            // What the decoder made room for (its results are taken, a frame has passed) goes to
            // the bodies that waited.
            any |= self.admit_waiting();
            // What was decoded, and the bodies that went by, may have been the last of a decode
            // that ran with the fetchers held off.
            any |= self.settle_exclusive();
            self.sync_pause(false);
            // A body that can only go in alone goes in once the fetchers are held off (which
            // was just asked for) and have nothing in the way.
            if self.admit_alone() {
                any = true;
                self.sync_pause(false);
            }
            // Whatever made work runnable without a result to take (a transfer that gave up for
            // the pause, the pause let go) needs a frame for its request to find it so: this is
            // the window's only way to know.
            if paused && !self.remote.paused() {
                any = true;
            }
            if self.held_ready() && !std::mem::replace(&mut self.held_due, true) {
                any = true;
            }
            self.forget_evicted();
            self.trim_records();
            if any {
                self.sync_remote(doc, layout);
            }
        }
        // The animations painted are made their next frames, and those asked for are read.
        #[cfg(feature = "images")]
        self.pump_anims();
        // (What the steps and the results above did to the states painted last.)
        #[cfg(feature = "images")]
        {
            any |= self.state_changed();
        }
        self.cache.unprotect();
        any
    }

    /// Take one result: a decoded image goes in the cache (or is refused, its image showing its
    /// alt text); one that failed to decode shows its alt text, and a fetched body is let in or not
    /// ([`Images::arrived`]). `page` is the document and layout to tell what changes (a poll's):
    /// without it (the document is going, [`Images::attach`]) nothing is shown or laid out.
    fn take_done(&mut self, done: Done, mut page: Option<(&Doc, &mut Layout)>) {
        match done {
            Done::Decoded {
                doc_gen,
                key,
                bitmap,
                need,
                refused,
                skipped,
                #[cfg(feature = "images")]
                anim,
                cancelled,
                worker,
            } => {
                // The result is in the cache (or dropped) before the worker is let go on,
                // so what it holds is never uncounted: see `Queue::busy`.
                self.cache.expect = self.cache.expect.saturating_sub(1);
                // Given up for taking too long: it is refused for that, which is not undone by
                // the result of the drawing that stopped (it would show only alt text, and not
                // that a click loads it).
                if cancelled {
                    Worker::taken(&worker);
                    return;
                }
                if matches!(key, Key::Local(_)) && doc_gen != self.doc_gen {
                    Worker::taken(&worker);
                    return;
                }
                if let Some(need) = need {
                    self.remeasure(&key, need);
                    Worker::taken(&worker);
                    return;
                }
                match bitmap {
                    Some(b) => {
                        // (Kept, as any body, for an animation that is to play: it is played
                        // from it.)
                        #[cfg(feature = "remote-images")]
                        if let Key::Remote(url) = &key
                            && !(anim.is_some() && self.anims.wanted(&key))
                        {
                            self.cache.release_bytes(url);
                        }
                        // (At most one entry an image of the document, cleared with it.)
                        if skipped && !self.partial.contains(&key) {
                            self.partial.push(key.clone());
                        }
                        // (At most one entry an image too: one decoded again finds it there.)
                        #[cfg(feature = "images")]
                        if let Some(info) = anim
                            && !self.animated.iter().any(|(k, _)| *k == key)
                        {
                            self.animated.push((key.clone(), info));
                        }
                        self.decodes += 1;
                        self.charge_bookkeeping();
                        // A result with no place to go is dropped, and its image is not failed:
                        // there is no room now, which a later request finds otherwise (the
                        // image is asked for again, and the list of slots made the room first,
                        // so this is only for results that came beside bodies that took it).
                        let (w, h) = (b.width, b.height);
                        let current = doc_gen == self.doc_gen;
                        let key_kept = key.clone();
                        if self.store(key, Data::Pixels(b), current).is_ok() {
                            if page.is_some() {
                                // (Kept to the end of the poll that took it: the window paints
                                // it next, and what the rest of the poll does, the bodies it
                                // lets in and the room it makes, doesn't take it out first.
                                // After the poll nothing keeps it but a frame that painted it.)
                                self.cache.protect(&key_kept, w, h);
                            }
                        } else {
                            // (No place: nothing holds a late result's URL, nor what it found.)
                            #[cfg(feature = "images")]
                            if !self.anim_owned(&key_kept) {
                                self.animated.retain(|(k, _)| *k != key_kept);
                            }
                        }
                    }
                    // It could not be decoded: the data is bad, or over VOMD's limits. The image
                    // shows its alt text.
                    None => self.failed(
                        key,
                        refused,
                        page.as_mut().map(|(doc, layout)| (*doc, &mut **layout)),
                    ),
                }
                Worker::taken(&worker);
            }
            #[cfg(feature = "images")]
            Done::Forced {
                doc_gen,
                origin,
                result,
            } => self.take_forced(doc_gen, origin, result, page),
            #[cfg(feature = "images")]
            Done::Anim { serial, output } => {
                self.anim_done(serial, output);
            }
            #[cfg(feature = "remote-images")]
            Done::Fetched { url, result } => self.arrived(url, result, page.is_some()),
        }
    }

    /// The image for `key` can't be shown: it shows its alt text from now on (a local one, from
    /// the layout's next look; a remote one while the document is open: its body is of no use),
    /// and why, if that is VOMD's limits (`why`).
    fn failed(&mut self, key: Key, why: Option<Refusal>, page: Option<(&Doc, &mut Layout)>) {
        match key {
            Key::Local(id) => {
                if let Some(s) = id
                    .checked_sub(1)
                    .and_then(|i| self.srcs.get_mut(i as usize))
                {
                    *s = Src::None;
                }
                if let Some((doc, layout)) = page {
                    let size = why.map_or(ImageSize::Alt, ImageSize::Refused);
                    layout.set_image(doc, id, size);
                }
            }
            #[cfg(feature = "remote-images")]
            Key::Remote(url) => {
                let own = self.urls.contains(&url);
                self.remote.failed(&url, own, why);
                // The body that didn't decode is of no use: with it gone, the image is fetched
                // again when a document with it is opened again (the failure is kept while this
                // one is open).
                self.cache.discard_bytes(&url);
                if let Some((doc, layout)) = page {
                    self.sync_remote(doc, layout);
                }
            }
            #[cfg(not(feature = "remote-images"))]
            Key::Remote(_) => {
                let _ = page;
            }
        }
    }

    /// A job of the SVG for `key` was charged less than it needs (`need`): its record says so
    /// from now on (in place: nothing grows), and the next request asks for it with that charge,
    /// or refuses it if that can't fit. The charge only grows, so this happens a few times at
    /// most for an image.
    fn remeasure(&mut self, key: &Key, need: SvgCost) {
        match key {
            Key::Local(id) => {
                if let Some(Some(info)) = self.costs.get_mut(id) {
                    info.header.svg = need;
                }
            }
            #[cfg(all(feature = "remote-images", feature = "svg"))]
            Key::Remote(url) => {
                self.remote
                    .remeasure(url, svg::cost(&need, 0, 0), svg::box_costs(&need));
            }
            #[cfg(not(all(feature = "remote-images", feature = "svg")))]
            Key::Remote(_) => {}
        }
    }

    /// Images whose decode can't run in the budget ([`Images::refused`]) show their alt text.
    /// Returns true if there were any (the window needs a repaint).
    fn show_refused(&mut self, doc: &Doc, layout: &mut Layout) -> bool {
        if self.refused.is_empty() {
            return false;
        }
        for (key, why) in std::mem::take(&mut self.refused) {
            // (A remote one is laid out by the one `sync_remote` that follows, for all.)
            let page = matches!(key, Key::Local(_)).then_some((doc, &mut *layout));
            self.failed(key, Some(why), page);
        }
        #[cfg(feature = "remote-images")]
        self.sync_remote(doc, layout);
        true
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
    fn arrived(&mut self, url: Arc<str>, result: crate::remote::Fetched, polled: bool) {
        self.charge_bookkeeping();
        // (The bodies this poll let in already are not the room for this one: there is no request
        // between them to keep what it wants, and a body not in view, which nothing has used yet,
        // was the first the next one's room was made from, and was fetched again, and again.)
        self.cache.guard_admitted(true);
        let admit = self.judge(&url, &result);
        match admit {
            Admit::Wait => self.waiting.push((url, result)),
            Admit::Yes => {
                self.take_fetched(url.clone(), result, true);
                if polled {
                    self.cache.admitted(&url);
                }
            }
            Admit::Drop => self.drop_fetched(url, result),
            Admit::Refuse => self.refuse_fetched(url, result),
        }
        self.cache.guard_admitted(false);
    }

    /// What keeping the body of `url`, dropped for room, would have cost and what decoding it needs,
    /// if it was dropped and not fetched again since.
    #[cfg(feature = "remote-images")]
    fn deferred_need(&self, url: &Arc<str>) -> Option<(usize, usize)> {
        self.deferred
            .get(url)
            .copied()
            .filter(|n| *n != NOT_DEFERRED)
    }

    /// Note that the body of `url` was dropped for room. The table has the room for every URL of
    /// the document ([`Images::attach`]) and a URL's entry is never removed while the document is
    /// open ([`Images::undefer`] says it is none): a table that has had entries removed, and
    /// so tombstones, may be grown by an insert that finds no room, even when it is not full.
    #[cfg(feature = "remote-images")]
    fn defer(&mut self, url: &Arc<str>, need: (usize, usize)) {
        debug_assert!(
            self.deferred.len() < self.deferred.capacity() || self.deferred.contains_key(url),
            "the table of dropped bodies has room for the document's URLs"
        );
        self.deferred.insert(url.clone(), need);
    }

    /// The body of `url` is cached, or its image is wanted: it is not one dropped for room.
    #[cfg(feature = "remote-images")]
    fn undefer(&mut self, url: &Arc<str>) {
        if let Some(n) = self.deferred.get_mut(url) {
            *n = NOT_DEFERRED;
        }
    }

    /// What becomes of a fetched `result` for `url`: see [`Images::admission`]. A body for an image
    /// the document doesn't have (it arrives late, for another document) is let go: nothing is
    /// kept for a URL that isn't the document's, so nothing grows for one.
    #[cfg(feature = "remote-images")]
    fn judge(&mut self, url: &Arc<str>, result: &crate::remote::Fetched) -> Admit {
        match result {
            Ok((bytes, ..)) if self.urls.contains(url) => self.admission(url, bytes),
            _ => Admit::Drop,
        }
    }

    /// Take a body whose image can't be decoded in the budget: it is not kept, and the image
    /// shows its alt text (as it does when it fails to decode).
    #[cfg(feature = "remote-images")]
    fn refuse_fetched(&mut self, url: Arc<str>, result: crate::remote::Fetched) {
        let need = match &result {
            Ok((bytes, ..)) => self
                .decode_estimate(&url, bytes)
                .saturating_add(bytes.len()),
            Err(_) => 0,
        };
        self.take_fetched(url.clone(), result, false);
        let own = self.urls.contains(&url);
        self.remote
            .failed(&url, own, Some(Refusal::Memory(need as u64)));
    }

    /// Forget what is kept per URL that the current document doesn't have and nothing holds:
    /// the records of bodies dropped for room ([`Images::deferred`]), and what fetching found
    /// ([`Remote::size`](crate::remote::Remote::size)) unless a body or pixels for the URL are
    /// cached (the slot is charged for the entry). Without this, a session through many documents
    /// would collect an entry for every URL ever seen. A failed URL of a document left is tried
    /// again when it is opened again: its record goes whatever is cached, and so does a body of
    /// it that didn't decode (a cached body is taken for good, and would fail again).
    ///
    /// With `fit` (a document is being attached, with that many remote images), the two tables
    /// are given the room for them: what is put in them after is the document's URLs only, so they
    /// never grow again (the room for the one table of the old size and the other of the new was
    /// made, [`Images::tables_bytes`]). Without it (the document is being left) they keep their
    /// room: a smaller table would be made beside the old one, and the next document makes it.
    /// Looking at which URLs are cached takes a set of them, made room for first; if there is
    /// none the records of other documents wait for the next time.
    #[cfg(feature = "remote-images")]
    fn prune_urls(&mut self, fit: Option<usize>) {
        {
            let urls = &self.urls;
            self.deferred.retain(|u, _| urls.contains(u));
            // (No list of them is made: they are read from the table as their bodies go.)
            let (remote, cache) = (&self.remote, &mut self.cache);
            for url in remote.failed_urls().filter(|u| !urls.contains(*u)) {
                cache.discard_bytes(url);
            }
        }
        let slots = self
            .cache
            .slots
            .iter()
            .filter(|s| matches!(s.key, Key::Remote(_)))
            .count();
        if self.make_room(
            "prune: the cached URLs",
            table_bytes(slots, size_of::<&Arc<str>>()),
        ) {
            let urls = &self.urls;
            let mut cached: std::collections::HashSet<&Arc<str>> =
                std::collections::HashSet::with_capacity(slots);
            cached.extend(self.cache.slots.iter().filter_map(|s| match &s.key {
                Key::Remote(url) => Some(url),
                Key::Local(_) => None,
            }));
            self.remote
                .retain_sizes(|u, size| urls.contains(u) || (size.is_some() && cached.contains(u)));
            // What is known of a remote animation lives as long as its still or animation is
            // cached, or the document has it: back at it, nothing is decoded again to find it
            // out. Past that it is forgotten ([`Images::forget_evicted`] does so for a slot
            // evicted later).
            #[cfg(feature = "images")]
            {
                let owned =
                    |k: &Key| matches!(k, Key::Remote(u) if urls.contains(u) || cached.contains(u));
                keep_anims(&mut self.animated, &mut self.anims, &owned);
            }
        }
        if let Some(extra) = fit {
            fit_map(&mut self.deferred, extra);
            self.remote.fit_sizes(extra);
        }
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
            self.defer(&url, need);
        }
        self.take_fetched(url, result, false);
    }

    /// What decoding the body fetched for `url` will hold (see [`decode_cost`]), to see if there
    /// is room for it: to the box of its image if that is in view, else to the size it has
    /// (the box is known when it is asked for, and the reserve is made then).
    #[cfg(feature = "remote-images")]
    fn decode_estimate(&self, url: &Arc<str>, bytes: &Bytes) -> usize {
        // A body fetched again for its animation isn't decoded (its still is cached): what reading
        // it holds is charged by `start_anim`. (A key left in `fetched` whose still was evicted
        // since is fetched for the image.)
        #[cfg(feature = "images")]
        if self.cache.has_url(url) && self.anims.is_fetched(url) {
            return 0;
        }
        let Some(header) = info_of(bytes) else {
            return 0;
        };
        let (w, h) = match self.in_view.iter().find(|(u, ..)| u == url) {
            Some(&(_, w, h)) => (w, h),
            None => {
                let (w, h) = header.shown();
                clamp_box(w, h)
            }
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
        let in_view = self.in_view.iter().any(|(u, ..)| u == url);
        // The decode of an image in view that fits the cache by itself but not beside the
        // bookkeeping has no room to wait for (one bigger than the cache by itself is let in
        // alone, `admit_alone`).
        let own = cost.saturating_add(decode);
        if in_view && own <= self.cache.cap && !self.fits_bookkeeping(own) {
            return Admit::Refuse;
        }
        // Whatever the fetchers hold now (they may be held off, and hold nothing): a body let in
        // is protected, and stays when they are let go on, so it has to fit beside all they may take.
        // (And the list of slots it goes in, if that is full.)
        if self.cache.room_for(
            cost.saturating_add(self.cache.growth()),
            decode,
            self.fetch_static(true, true),
        ) {
            Admit::Yes
        } else if !in_view {
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
        // They are let go only if that makes the decode fit: with what is kept and what it needs
        // over the budget by themselves, dropping them gains nothing, and they were fetched
        // again, and again, by every body let in (700 one-pixel images beside a bitmap that took
        // the budget: thousands of fetches of the same URLs).
        if others(self) > 0
            && !self
                .cache
                .fits(kept, 0, need, 0, self.remote.reserve_idle())
            && self.cache.fits(kept, 0, need, 0, 0)
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
        // (It goes in a place in the list of slots: with none, it waits.)
        if !self.can_store() {
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
    fn settle_exclusive(&mut self) -> bool {
        let Some(key) = self.exclusive.clone() else {
            return false;
        };
        self.retired_cost();
        if !self.awaiting
            && self.retired.is_empty()
            && !self.worker.as_ref().is_some_and(|w| w.has(&key))
        {
            self.exclusive = None;
            // A decode held back for it (see `request`) can be asked for now: true, and the
            // caller says so to the window (a poll that took the result says it by returning true,
            // and no wake is sent for what it is already telling: a wake for every decode that ran
            // alone doubled the polls of a window full of them).
            return self.heavy_wanted;
        }
        false
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
                self.make_room("pause released", 0);
            }
            self.remote.pause(want);
        }
        self.cache.fetching = self.fetch_reserve(starting);
    }

    /// The body (or failure) of a finished fetch is taken from the fetchers (their credit is
    /// free) and, if `keep`, goes to the cache, before the fetchers can use what they are given.
    #[cfg(feature = "remote-images")]
    fn take_fetched(&mut self, url: Arc<str>, result: crate::remote::Fetched, keep: bool) {
        self.charge_bookkeeping();
        // Only the document's URLs are recorded: the tables have room for those and no more.
        let own = self.urls.contains(&url);
        debug_assert!(
            !own || self.remote.has_room_for(&url),
            "the table of sizes has room for the document's URLs"
        );
        // An animation's body that couldn't be fetched is the animation's failure, not the image's:
        // its still is shown, so the size and the still it has stay; a click asks again.
        #[cfg(feature = "images")]
        let lost = matches!(&result, Err(e) if !matches!(e, crate::remote::Error::NoCurl | crate::remote::Error::Yielded))
            && self.body_lost(&url);
        #[cfg(not(feature = "images"))]
        let lost = false;
        let taken = if lost {
            None
        } else {
            self.remote.take_fetched(&url, result, own)
        };
        #[cfg(feature = "images")]
        let arrived = taken.is_some();
        if keep {
            self.undefer(&url);
        }
        match (keep, taken) {
            (true, Some(bytes)) => {
                match self.store(Key::Remote(url.clone()), Data::Bytes(bytes), true) {
                    Ok(()) => {
                        if self.in_view.iter().any(|(u, ..)| *u == url) {
                            self.cache.bytes_of(&url);
                        }
                    }
                    // No room even for its place in the list, now: not a failure of the image. One
                    // in view waits for room, holding the fetchers' credit; any other is dropped
                    // for room, and fetched again when there is (see `Images::admission`).
                    Err(Data::Bytes(bytes)) => {
                        if let Some(Some((w, h))) = self.remote.size(&url) {
                            let result = Ok((bytes, w, h));
                            if self.in_view.iter().any(|(u, ..)| *u == url) {
                                self.waiting.push((url, result));
                            } else {
                                self.drop_fetched(url, result);
                            }
                            return;
                        }
                    }
                    Err(_) => {}
                }
            }
            (_, taken) => drop(taken),
        }
        // An animation's body is asked for again until it comes, kept or not (dropped for room, it
        // is asked for again once it would fit: `body_for_anim` looks at the record of the drop).
        #[cfg(feature = "images")]
        self.anims
            .fetched
            .retain(|k| !matches!(k, Key::Remote(u) if *u == url));
        // (A body that came, whatever it was fetched for, ends the failure of its animation.)
        #[cfg(feature = "images")]
        if arrived {
            self.anims
                .failed
                .retain(|k| !matches!(k, Key::Remote(u) if *u == url));
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
        self.charge_bookkeeping();
        let mut any = false;
        for (url, result) in std::mem::take(&mut self.waiting) {
            let admit = self.judge(&url, &result);
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
                Admit::Refuse => {
                    self.refuse_fetched(url, result);
                    any = true;
                }
            }
        }
        any
    }

    /// Whether a body that waited can go on now: its image is no longer in view, or there is room.
    #[cfg(feature = "remote-images")]
    fn waiting_ready(&mut self) -> bool {
        self.charge_bookkeeping();
        let waiting = std::mem::take(&mut self.waiting);
        let ready = waiting
            .iter()
            .any(|(url, result)| !matches!(self.judge(url, result), Admit::Wait));
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
        let growth = self.cache.growth();
        self.retired.is_empty()
            && (growth <= SLOTS_FREE || self.room_limit() >= growth)
            && self
                .waiting
                .iter()
                .any(|(u, r)| matches!(r, Ok((b, ..)) if self.is_heavy(kept, u, b)))
    }

    /// Whether the decode the last request held back ([`Images::held`]) has nothing in its way
    /// now, which a request (a frame) would find: no replaced worker is decoding, no other decode
    /// runs alone, no body is kept for a decode, and the fetchers are held off and hold nothing
    /// ([`Images::fetchers_idle`]). Nothing is changed. The ways each of these comes true all
    /// send the window a result to take, except the last, which a transfer can bring about by
    /// giving up: [`Images::poll`] says so when it does.
    #[cfg(feature = "remote-images")]
    fn held_ready(&mut self) -> bool {
        let Some(key) = self.held.clone() else {
            return false;
        };
        self.retired_cost();
        let own = match &key {
            Key::Remote(url) => Some(url),
            Key::Local(_) => None,
        };
        self.retired.is_empty()
            && self.exclusive.as_ref().is_none_or(|k| *k == key)
            && !self.cache.holds_used_bytes_but(own)
            && self.fetchers_idle()
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
    /// ran, unless the document has them: their size and its string. (What a URL had is not kept
    /// for it past its slot: nothing the cache evicts leaves a record behind.) The URLs are looked
    /// at [`FORGET_CHUNK`] at a time, so what is made to do it is a table of that size, whatever
    /// the number of slots evicted: a hundred thousand at once is a hundred and ninety-five rounds
    /// over the slots, and not a table of them made when what they freed may be spent.
    #[cfg(feature = "remote-images")]
    fn forget_evicted(&mut self) {
        while !self.cache.dropped.is_empty() {
            let at = self.cache.dropped.len().saturating_sub(FORGET_CHUNK);
            let mut gone: std::collections::HashSet<Arc<str>> =
                self.cache.dropped.drain(at..).collect();
            for s in &self.cache.slots {
                if let Key::Remote(url) = &s.key {
                    gone.remove(url);
                }
            }
            gone.retain(|u| !self.urls.contains(u));
            #[cfg(feature = "images")]
            self.forget_anim_urls(&gone);
            self.remote.forget_sizes(&gone);
        }
        // (The list's buffer is given back, not kept for the next removals.)
        self.cache.dropped = Vec::new();
    }

    /// Forget what is known of the remote animations at `gone`, the URLs that nothing holds any
    /// more (no document, no slot): that they are animated, and what was refused for them or
    /// forced. Their strings are charged by whoever holds them, and these lists would hold the
    /// last of them.
    #[cfg(all(feature = "remote-images", feature = "images"))]
    fn forget_anim_urls(&mut self, gone: &std::collections::HashSet<Arc<str>>) {
        let gone = |k: &Key| matches!(k, Key::Remote(u) if gone.contains(u));
        keep_anims(&mut self.animated, &mut self.anims, &|k| !gone(k));
    }

    /// Give back the room the table of sizes has left when most of it was forgotten (a cache of
    /// twenty thousand one-pixel images was evicted for a big one: its table was 1 MB of room for
    /// records that are gone, charged by capacity beside the decode). The smaller table is made
    /// beside the old one, so the room for it is made first ([`Images::make_room`]); with none it
    /// stays as it is, charged. It keeps the room for the document's URLs, which are recorded in
    /// it and never grow it.
    #[cfg(feature = "remote-images")]
    fn trim_records(&mut self) {
        let want = self.urls.len();
        if !self.remote.sizes_oversized(want) {
            return;
        }
        let bytes = self.remote.sizes_bytes_for(want);
        if self.make_room("sizes table", bytes) {
            self.remote.fit_sizes(want);
        }
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
                    self.view_urls.capacity(),
                    size_of::<Arc<str>>(),
                ))
                .saturating_add(table_bytes(
                    self.deferred.capacity(),
                    size_of::<(Arc<str>, (usize, usize))>(),
                ))
                .saturating_add(vec_bytes(&self.in_view))
                .saturating_add(vec_bytes(&self.waiting))
                .saturating_add(self.remote.bookkeeping_bytes());
            // What a request builds for the fetchers is built only while they are wanted.
            if self.remote.on {
                n = n.saturating_add(self.remote_images.saturating_mul(REQUEST_PER_REMOTE));
            }
        }
        n = n
            .saturating_add(vec_bytes(&self.refused))
            .saturating_add(vec_bytes(&self.partial));
        #[cfg(feature = "images")]
        {
            n = n
                .saturating_add(vec_bytes(&self.animated))
                .saturating_add(self.anims.bookkeeping())
                .saturating_add(self.forced.bookkeeping())
                .saturating_add(vec_bytes(&self.forced_ids));
        }
        n
    }

    /// Whether a decode of `own` bytes (with the body it reads) fits the budget by itself beside
    /// what the document's images cost to keep, once everything that can be evicted is. The
    /// bookkeeping includes the records of the slots (a slot list, the sizes of cached URLs), which
    /// go with them, so the answer is the one after they have: a cache full of small images
    /// doesn't make a big one's decode impossible, only a document that costs the room itself.
    /// (Whatever was painted in this frame stays, and is not counted: it is the exception to the
    /// budget, [`Images::alone`].) `false` means the decode can't run in the budget at all, and
    /// the image is refused: shown as its alt text. Nothing is allocated: only what is evicted.
    fn fits_bookkeeping(&mut self, own: usize) -> bool {
        // What the request in progress builds is not the document's: it is gone when the decode
        // runs, and an image refused for it would be refused for good, for a moment's cost.
        let extra = std::mem::take(&mut self.extra);
        let fits = self.fits_bookkeeping_now(own);
        self.extra = extra;
        self.charge_bookkeeping();
        fits
    }

    fn fits_bookkeeping_now(&mut self, own: usize) -> bool {
        self.charge_bookkeeping();
        if self.cache.fits(0, 0, own, 0, 0) {
            return true;
        }
        self.cache.evict_for(self.cache.cap);
        #[cfg(feature = "remote-images")]
        {
            self.forget_evicted();
            self.trim_records();
        }
        self.charge_bookkeeping();
        self.cache.fits(0, 0, own, 0, 0)
    }

    /// Set what the cache holds back for the bookkeeping to what it is now, and what the
    /// operation in progress allocates ([`Images::extra`]).
    fn charge_bookkeeping(&mut self) {
        self.cache.overhead = self.bookkeeping_bytes().saturating_add(self.extra);
    }

    /// The one place room is made for an allocation of the image code (a table, a list, a buffer,
    /// a path, a decode): `bytes` is what is about to be allocated, at most, and the answer is
    /// whether it fits the budget beside everything that can't be evicted: the bookkeeping (every
    /// container by its capacity, and what the operation in progress builds, [`Images::extra`]),
    /// what decodes hold ([`Cache::reserve`], a result nobody has taken included, and the replaced
    /// workers'), what is set aside for the fetchers ([`Cache::fetching`]), the bodies decodes read
    /// and the pixels painted in this frame. What can be evicted is, down to what is needed, and
    /// the records of what is evicted are forgotten ([`Images::forget_evicted`]). Every allocation
    /// of the image code that isn't covered by a reservation calls this before it is made, with
    /// its size, and has a defined answer to `false` (defer, refuse or skip: the audit table says
    /// which, site by site). `site` names the call for the tests' probe ([`set_room_probe`]).
    fn make_room(&mut self, site: &'static str, bytes: usize) -> bool {
        #[cfg(debug_assertions)]
        if let Some(probe) = ROOM_PROBE.get() {
            probe(site, bytes, None);
        }
        let _ = site;
        self.charge_bookkeeping();
        self.cache.evict_for(bytes);
        #[cfg(feature = "remote-images")]
        self.forget_evicted();
        self.charge_bookkeeping();
        let fits = self
            .cache
            .bytes
            .saturating_add(self.cache.held())
            .saturating_add(bytes)
            <= self.cache.cap;
        #[cfg(debug_assertions)]
        if let Some(probe) = ROOM_PROBE.get() {
            probe(site, bytes, Some(fits));
        }
        fits
    }

    /// The most [`Images::make_room`] could ever make: the budget less what can't be evicted.
    fn room_limit(&mut self) -> usize {
        self.charge_bookkeeping();
        self.cache
            .cap
            .saturating_sub(self.cache.held())
            .saturating_sub(self.cache.stuck())
    }

    /// Put a result in the cache, making room for the list it goes in first ([`Images::make_room`]:
    /// the slot's own room is made by the cache, the result being allocated already). `Err`: the
    /// list is full and its growth doesn't fit (everything evictable is gone, so what is left is
    /// painted or held by a decode), and the data is given back: there is no room *now*, which is
    /// no fault of the image, and what it becomes is the caller's to say (a decoded result is
    /// dropped and asked for again, a body waits). A request makes the room for the results it asks
    /// for first ([`Cache::reserve_slots`]), so this is for what came beside them. A growth of at
    /// most [`SLOTS_FREE`] is let in whatever the answer: the pixels painted in a frame are the one
    /// exception to the budget, and the list of them is the size of their number.
    fn store(&mut self, key: Key, data: Data, current: bool) -> Result<(), Data> {
        let growth = self.cache.growth();
        // (Nothing is evicted for a list that can't be made room for: what there is to evict is
        // wanted by whoever stored it, and the result that has no place is asked for again.)
        if growth > SLOTS_FREE && !self.can_make_room(growth) {
            return Err(data);
        }
        if growth > 0 {
            self.make_room("slot list", growth);
        }
        self.cache.insert_as(key, data, current);
        Ok(())
    }

    /// Whether [`Images::store`] would have a place for one more slot now (nothing is allocated;
    /// what is evictable is evicted, as `store` does).
    #[cfg(feature = "remote-images")]
    fn can_store(&mut self) -> bool {
        let growth = self.cache.growth();
        growth <= SLOTS_FREE || (self.can_make_room(growth) && self.make_room("slot list", growth))
    }

    /// Whether [`Images::make_room`] could make the room for `bytes`: whether they fit beside
    /// what can't be evicted, with nothing evicted to find out.
    fn can_make_room(&mut self, bytes: usize) -> bool {
        self.charge_bookkeeping();
        self.cache.can_fit(bytes)
    }

    /// Whether an image of `boxes` (the first `on_screen` in view) is a remote one that will be
    /// fetched: it has no body and no pixels at its box, and the fetchers are going to be asked for
    /// it (not one that failed, nor one dropped for room that is not in view: that waits). The
    /// fetchers are set aside their reserve while there is one, whatever they are doing at the
    /// moment, for the room a request keeps for the images near the viewport: a decision that
    /// followed whether they were idle or not flipped with it, so that an image near was decoded
    /// while they were idle, and evicted when they started (to fetch the body of another image
    /// near that had been evicted), and decoded again, and again (seed 19 of the random mix, with
    /// only what a frame painted kept from eviction).
    #[cfg(feature = "remote-images")]
    fn needs_fetch(&self, boxes: &[(u32, u32, u32)], on_screen: usize) -> bool {
        self.remote.on
            && !self.remote.no_curl
            && boxes.iter().enumerate().any(|(n, &(id, w, h))| {
                let Src::Remote(url) = self.src(id) else {
                    return false;
                };
                self.remote.size(url) != Some(None)
                    && self.cache.peek_bytes(url).is_none()
                    && !(n >= on_screen && self.deferred_need(url).is_some())
                    && {
                        let (w, h) = clamp_box(w, h);
                        !self.cache.has(&Key::Remote(url.clone()), w, h)
                    }
            })
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
                let id = i as u32 + 1;
                let size = match self.forced_ids.iter().find(|f| f.0 == id) {
                    Some((_, origin)) if self.remote.on => self.forced_size(origin),
                    _ => match self.forced.failure(&forced::Origin::Url(url.clone())) {
                        Some(why) if self.remote.on => ImageSize::Refused(why),
                        _ => self.remote.layout_size(url),
                    },
                };
                layout.set_image(doc, id, size);
            }
        }
    }

    /// Whether image `id` is an SVG drawn without some of its embedded images for VOMD's limits:
    /// a click draws it with them ([`Images::force`]).
    pub fn partial(&self, id: u32) -> bool {
        #[cfg(feature = "images")]
        if self.forced_ids.iter().any(|f| f.0 == id) {
            return false;
        }
        self.key(id).is_some_and(|k| self.partial.contains(&k))
    }

    /// Image `id`'s animation, if its decode found it is one: its canvas and loop count.
    pub fn animated(&self, id: u32) -> Option<AnimInfo> {
        #[cfg(feature = "images")]
        {
            let key = self.key(id)?;
            self.animated
                .iter()
                .find_map(|(k, info)| (*k == key).then_some(*info))
        }
        #[cfg(not(feature = "images"))]
        {
            let _ = id;
            None
        }
    }

    /// Whether animations play (the window's `A`): every animated image plays from its next
    /// paint, but those the reader chose otherwise for.
    pub fn animations(&self) -> bool {
        #[cfg(feature = "images")]
        return self.anims.on;
        #[cfg(not(feature = "images"))]
        false
    }

    /// Animations play, or not, all of them: the choices made for single images are forgotten.
    /// One stopped keeps the frame it shows.
    pub fn set_animations(&mut self, on: bool) {
        #[cfg(feature = "images")]
        {
            self.anims.on = on;
            self.anims.choices = Vec::new();
            if on {
                // (Pressing play asks again for the animations whose body couldn't be fetched.)
                self.anims.failed = Vec::new();
                self.restart_ended(None);
            }
            (self.wake)();
        }
        #[cfg(not(feature = "images"))]
        let _ = on;
    }

    /// The reader asked image `id`'s animation to play (whatever the window's choice), for the
    /// document's life. One over VOMD's limits plays past them: the reader asked for it.
    pub fn play(&mut self, id: u32) {
        self.choose_anim(id, true);
    }

    /// The reader asked image `id`'s animation to stop where it is, for the document's life.
    pub fn pause(&mut self, id: u32) {
        self.choose_anim(id, false);
    }

    fn choose_anim(&mut self, id: u32, play: bool) {
        #[cfg(feature = "images")]
        {
            let Some(key) = self.key(id) else {
                return;
            };
            if play {
                // (A click tries again an animation whose body couldn't be fetched.)
                anim::retain_keys(&mut self.anims.failed, &|k| *k != key);
                self.restart_ended(Some(&key));
            }
            self.anims.choices.retain(|c| c.0 != key);
            self.anims.choices.push((key.clone(), play));
            // One over the limits, or that wouldn't fit beside the others now it is to play, plays
            // past them: the reader asked for it.
            if play && !self.anims.forced.contains(&key) {
                let over = self.anims.refusal(&key) == Some(true) || {
                    let shown = self.anims_shown();
                    self.reallocate(&shown);
                    self.unstall(id, &key);
                    self.anims.waits(&key)
                };
                if over {
                    self.anims.forced.push(key);
                }
            }
            // (Its badge comes or goes.)
            self.repaint = true;
            (self.wake)();
        }
        #[cfg(not(feature = "images"))]
        let _ = (id, play);
    }

    /// Whether image `id` is animated and plays (its frames once they are made, its still till
    /// then).
    pub fn is_playing(&self, id: u32) -> bool {
        #[cfg(feature = "images")]
        {
            self.animated(id).is_some() && self.key(id).is_some_and(|k| self.anims.wanted(&k))
        }
        #[cfg(not(feature = "images"))]
        {
            let _ = id;
            false
        }
    }

    /// Whether image `id`'s animation doesn't play for VOMD's limits (one animation's, or all
    /// of them together's): [`Images::play`] plays it past them.
    pub fn anim_over(&self, id: u32) -> bool {
        #[cfg(feature = "images")]
        {
            self.key(id).is_some_and(|k| {
                (self.anims.refusal(&k) == Some(true) || self.anims.waits(&k))
                    && !self.anims.forced.contains(&k)
            })
        }
        #[cfg(not(feature = "images"))]
        {
            let _ = id;
            false
        }
    }

    /// What image `id`'s animation does: `None` when it isn't one, or is one VOMD can't play
    /// (broken, or over the forced-load ceiling).
    pub fn anim_state(&self, id: u32) -> Option<AnimState> {
        #[cfg(feature = "images")]
        {
            if self.animated.is_empty() {
                return None;
            }
            let key = self.key(id)?;
            if !self.animated.iter().any(|(k, _)| *k == key) {
                return None;
            }
            let forced = self.anims.forced.contains(&key);
            Some(match self.anims.refusal(&key) {
                Some(false) => return None,
                _ if !self.anims.wanted(&key) || self.anim_ended(&key) => AnimState::Paused,
                Some(true) if !forced => AnimState::Over,
                _ if self.anims.failed.contains(&key) => AnimState::Failed,
                _ if !forced && self.anims.waits(&key) => AnimState::Waiting,
                _ => AnimState::Playing,
            })
        }
        #[cfg(not(feature = "images"))]
        {
            let _ = id;
            None
        }
    }

    /// Whether the document has an animated image (one decoded so far): the toolbar's
    /// animations button shows.
    pub fn has_animated(&self) -> bool {
        // (The remote ones known from other documents, still cached, don't count.)
        #[cfg(feature = "images")]
        return self.animated.iter().any(|(k, _)| match k {
            Key::Local(_) => true,
            #[cfg(feature = "remote-images")]
            Key::Remote(url) => self.urls.contains(url),
            #[cfg(not(feature = "remote-images"))]
            Key::Remote(_) => false,
        });
        #[cfg(not(feature = "images"))]
        false
    }

    /// When the next frame of an animation painted in the last frame is due: its frame made, and
    /// the one it shows shown its delay. `None` when none is (nothing plays, or what plays waits
    /// for a paint or for the anim thread, which wakes the window): no timer.
    pub fn anim_deadline(&self) -> Option<std::time::Instant> {
        #[cfg(feature = "images")]
        {
            if self.anims.idle() {
                return None;
            }
            let frame = self.cache.frame;
            let slots = self.cache.slots.iter().filter_map(|s| match &s.data {
                Data::Anim(p) => Some((&s.key, &**p)),
                _ => None,
            });
            let outside = self.anims.outside.iter().map(|(k, p)| (k, &**p));
            let due = slots
                .chain(outside)
                .filter(|(k, p)| p.painted == frame.saturating_add(1) && self.anims.wanted(k))
                .filter_map(|(_, p)| p.due())
                .min();
            anim::next_tick(due, self.anims.last)
        }
        #[cfg(not(feature = "images"))]
        None
    }

    /// Show the next frame of every animation painted in the last frame whose next frame is due
    /// by `now` (or [`anim::COALESCE`] after: one repaint for them), unless frames were shown
    /// less than [`anim::CAP`] before, and run the steps of those whose frame shown was painted.
    /// Returns the images whose frame changed (to paint again).
    pub fn advance(&mut self, now: std::time::Instant) -> Vec<u32> {
        #[cfg(feature = "images")]
        {
            if self.anims.idle() {
                return Vec::new();
            }
            if self.anims.last.is_some_and(|t| now < t + anim::CAP) {
                self.pump_anims();
                return Vec::new();
            }
            let frame = self.cache.frame;
            let mut keys = Vec::new();
            for at in self.anims_at() {
                let key = self.key_at(at).clone();
                let wanted = self.anims.wanted(&key);
                let p = self.playing(at);
                if wanted
                    && p.painted == frame.saturating_add(1)
                    && p.due().is_some_and(|d| anim::shows_at(d, now))
                {
                    p.swap();
                    self.note_ended(at);
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
            if !keys.is_empty() {
                self.anims.last = Some(now);
            }
            self.pump_anims();
            self.ids_of(&keys)
        }
        #[cfg(not(feature = "images"))]
        {
            let _ = now;
            Vec::new()
        }
    }

    /// A frame was painted: the animations it showed have their next frames made (each is made
    /// once the one before was painted, never ahead).
    pub fn painted(&mut self) {
        #[cfg(feature = "images")]
        if !self.anims.idle() {
            self.pump_anims();
        }
    }

    /// The box animated image `id` was painted in in the last frame (frame pixels), if it was.
    #[cfg(feature = "images")]
    fn anim_placed(&self, id: u32) -> Option<(i32, i32, i32, i32)> {
        let placed = self.anims.placed.iter();
        placed.filter(|p| p.0 == id).find_map(|p| p.1)
    }

    /// The boxes the images `ids` (what [`Images::advance`] showed) were painted in in the last
    /// frame, `None` if an animation was painted in none (what changed is not known). One that
    /// is the image of more than one has a box where each is in view: one that isn't has none.
    #[cfg(feature = "images")]
    pub fn anim_boxes(&self, ids: &[u32]) -> Option<Vec<(i32, i32, i32, i32)>> {
        let mut boxes = Vec::new();
        for &id in ids {
            match self.anim_placed(id) {
                Some(b) => boxes.push(b),
                None => {
                    let key = self.key(id);
                    let other = |&o: &u32| self.key(o) == key && self.anim_placed(o).is_some();
                    if !ids.iter().any(other) {
                        return None;
                    }
                }
            }
        }
        Some(boxes)
    }

    /// Whether an animation painted in the last frame plays (it has frames to come).
    pub fn animating(&self) -> bool {
        #[cfg(feature = "images")]
        {
            if self.anims.idle() {
                return false;
            }
            let frame = self.cache.frame;
            let slots = self.cache.slots.iter().filter_map(|s| match &s.data {
                Data::Anim(p) => Some((&s.key, &**p)),
                _ => None,
            });
            let outside = self.anims.outside.iter().map(|(k, p)| (k, &**p));
            slots.chain(outside).any(|(k, p)| {
                p.painted == frame.saturating_add(1) && !p.ended() && self.anims.wanted(k)
            })
        }
        #[cfg(not(feature = "images"))]
        false
    }

    /// Paint animations as if at `clock` (`None`: now), for tests.
    #[cfg(feature = "images")]
    #[doc(hidden)]
    pub fn set_anim_clock(&mut self, clock: Option<std::time::Instant>) {
        self.anims.clock = clock;
    }

    /// The reader clicked image `id`'s placeholder (or an SVG drawn without its embedded images):
    /// load it past VOMD's limits, on a thread of its own ([`forced`]), from its next request on,
    /// and lay it out at its natural size (a box while it loads). Returns true if it will be.
    pub fn force(&mut self, id: u32, doc: &Doc, layout: &mut Layout) -> bool {
        #[cfg(feature = "images")]
        {
            let Some(img) = id.checked_sub(1).and_then(|i| doc.images.get(i as usize)) else {
                return false;
            };
            if img.remote && !self.remote_on() {
                return false;
            }
            let Some(origin) = forced_origin(img, self.base.as_deref()) else {
                return false;
            };
            // Every image of the document from that file or URL.
            let ids: Vec<u32> = (doc.images.iter().enumerate())
                .filter(|(_, other)| other.src == img.src)
                .map(|(i, _)| i as u32 + 1)
                .collect();
            self.force_ids(&origin, &ids);
            let size = self.forced_size(&origin);
            for id in ids {
                layout.set_image(doc, id, size);
            }
            true
        }
        #[cfg(not(feature = "images"))]
        {
            let _ = (id, doc, layout);
            false
        }
    }

    /// Load the images `ids`, of `origin`, past VOMD's limits from their next request on.
    #[cfg(feature = "images")]
    fn force_ids(&mut self, origin: &forced::Origin, ids: &[u32]) {
        self.forced.choose(origin.clone());
        for &id in ids {
            if !self.forced_ids.iter().any(|f| f.0 == id) {
                self.forced_ids.push((id, origin.clone()));
            }
        }
        self.repaint = true;
        (self.wake)();
    }

    /// The size layout gives an image chosen to load past the limits: its natural size (a 4:3
    /// box for a remote one until it is fetched), or why its load failed.
    #[cfg(feature = "images")]
    fn forced_size(&self, origin: &forced::Origin) -> ImageSize {
        if let Some(why) = self.forced.failure(origin) {
            return ImageSize::Refused(why);
        }
        match origin {
            forced::Origin::File(path) => open_regular(path)
                .and_then(|f| header_info(&mut std::io::BufReader::with_capacity(4096, f)))
                .map_or(ImageSize::Refused(Refusal::Failed), |h| {
                    let (w, h) = h.shown();
                    ImageSize::Pixels(w, h)
                }),
            forced::Origin::Url(url) => self
                .forced
                .natural(url)
                .map_or(ImageSize::Loading, |(w, h)| ImageSize::Pixels(w, h)),
        }
    }

    /// The sizes of `doc`'s local images that are chosen to load past the limits, or whose load
    /// failed, by `src`: what the layout's probe gives them.
    #[cfg(feature = "images")]
    fn forced_sizes(&self, doc: &Doc) -> Vec<(Arc<str>, ImageSize)> {
        let mut sizes = Vec::new();
        if self.forced.is_unused() {
            return sizes;
        }
        for img in doc.images.iter().filter(|img| !img.remote) {
            if sizes.iter().any(|(s, _)| *s == img.src) {
                continue;
            }
            let Some(origin) = forced_origin(img, self.base.as_deref()) else {
                continue;
            };
            if self.forced.is_chosen(&origin) || self.forced.failure(&origin).is_some() {
                sizes.push((img.src.clone(), self.forced_size(&origin)));
            }
        }
        sizes
    }

    /// A load past the limits is done: its pixels are kept (and its image laid out at its
    /// natural size), or the image shows why it failed, and isn't chosen any more.
    #[cfg(feature = "images")]
    fn take_forced(
        &mut self,
        doc_gen: u64,
        origin: forced::Origin,
        result: Result<forced::Loaded, Refusal>,
        page: Option<(&Doc, &mut Layout)>,
    ) {
        #[cfg(feature = "remote-images")]
        let mut page = page;
        let size = match result {
            Ok(loaded) => {
                let (w, h) = loaded.natural;
                // (Its key is the image's of the document it was loaded for.)
                if let Some(info) = loaded.anim
                    && doc_gen == self.doc_gen
                {
                    let ids = self
                        .forced_ids
                        .iter()
                        .filter(|f| f.1 == origin)
                        .map(|f| f.0);
                    for key in ids.filter_map(|id| self.key(id)).collect::<Vec<_>>() {
                        if !self.animated.iter().any(|(k, _)| *k == key) {
                            self.animated.push((key, info));
                        }
                    }
                }
                self.forced.loaded(origin.clone(), loaded, self.cache.frame);
                ImageSize::Pixels(w, h)
            }
            // (Only the body of an animation whose still is the cache's was fetched past the
            // limits: [`Images::body_lost`].)
            #[cfg(feature = "remote-images")]
            Err(why) if matches!(&origin, forced::Origin::Url(url) if self.body_lost(url)) => {
                self.forced.unchoose(&origin);
                // (Its layout stays.)
                page = None;
                ImageSize::Refused(why)
            }
            Err(why) => {
                self.forced.failed(origin.clone(), why);
                ImageSize::Refused(why)
            }
        };
        if doc_gen != self.doc_gen {
            return;
        }
        if let Some((doc, layout)) = page {
            for (id, _) in self.forced_ids.iter().filter(|f| f.1 == origin) {
                layout.set_image(doc, *id, size);
            }
        }
        if matches!(size, ImageSize::Refused(_)) {
            self.forced_ids.retain(|f| f.1 != origin);
        }
    }

    /// The body of the remote animation at `url`, fetched again to play it (or played past the
    /// limits), didn't come: the animation failed, not the image, whose still is the cache's and
    /// stays; a click asks again. Whether it was that.
    #[cfg(feature = "remote-images")]
    fn body_lost(&mut self, url: &Arc<str>) -> bool {
        let key = Key::Remote(url.clone());
        let lost = self.cache.has_url(url)
            && (self.anims.is_fetched(url) || self.anims.forced.contains(&key));
        if lost {
            self.anims.failed.push(key.clone());
            self.anim_changed(&key);
        }
        lost
    }

    /// Whether a load past the limits runs or waits (the tests).
    #[cfg(feature = "images")]
    #[doc(hidden)]
    pub fn forcing(&self) -> bool {
        self.forced.busy()
    }

    /// The most a load past the limits may need (`None`: what [`forced::ceiling`] says), and the
    /// bytes their pixels and bodies may hold beyond those painted (the tests).
    #[cfg(feature = "images")]
    #[doc(hidden)]
    pub fn set_forced_limits(&mut self, ceiling: Option<usize>, kept: usize) {
        self.forced.ceiling = ceiling;
        self.forced.cap = kept;
    }

    /// How many loads past the limits were started (the tests).
    #[cfg(feature = "images")]
    #[doc(hidden)]
    pub fn forced_loads(&self) -> usize {
        self.forced.loads
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
            self.view_urls = std::collections::HashSet::new();
            self.prune_urls(None);
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

/// Where `img` comes from, for a load past the limits: its file, or its URL.
#[cfg(feature = "images")]
fn forced_origin(img: &ImageRef, base: Option<&Path>) -> Option<forced::Origin> {
    if img.remote {
        Some(forced::Origin::Url(img.src.clone()))
    } else {
        local_path(&img.src, base).map(forced::Origin::File)
    }
}

/// Whether two sets of faces for SVG text are the same faces.
#[cfg(feature = "svg")]
fn same_fonts(a: &crate::fonts::SvgFonts, b: &crate::fonts::SvgFonts) -> bool {
    let faces = |f: &crate::fonts::SvgFonts| {
        f.sources
            .iter()
            .map(|(d, i)| (Arc::as_ptr(d).cast::<u8>(), *i))
            .collect::<Vec<_>>()
    };
    (a.sans, a.mono) == (b.sans, b.mono) && faces(a) == faces(b)
}

/// Where an animation is: a slot of the cache, or played past the limits.
#[cfg(feature = "images")]
#[derive(Clone, Copy)]
enum At {
    Slot(usize),
    Outside(usize),
}

// ---- Animations -----------------------------------------------------------------------------

// Which animations play is decided as a whole, not one by one: the allocation
// ([`anim::allocate`]) is a function of what was painted in the last frame (the animated images
// in view, on screen first from the top down, one box a key), whether each is to play (the
// window's `A`, the reader's choice), what each holds at its frame size (measured once a key and
// size, [`anim::Anims::costs`]), the refusals and those played past the limits (which take no
// part), and the room the cache has for them. It walks them in their order and lets each play
// that fits beside those before it, within [`anim::ANIM_ALL_MAX`] and that room; the others wait.
// What is done follows from it ([`Images::pump_anims`]): one it plays that isn't made is read
// (room made for it from those it doesn't play), one held that waits is let go once its step is
// back, and steps are run only for those it plays. The same inputs make the same allocation, and
// once it is followed nothing is left to do, so nothing repeats while nothing changes.
//
// An animation is read and indexed on the anim thread, and plays from that once it is known to
// fit: it holds a slot ([`Data::Anim`]), charged what it holds at most. One over the limits plays
// only past them, outside the cache, once the reader asks ([`Images::play`]). Each frame is
// pulled: a step is run on the anim thread only once the frame shown was painted, and its result
// is shown when the one before has been shown its delay ([`Images::advance`]), so an animation
// that isn't painted (off screen, a window that doesn't paint) costs nothing, and is evicted like
// any slot. One that comes back is read again, from its first frame.
#[cfg(feature = "images")]
impl Images {
    /// Where the animation of `key` is, if it is made.
    fn anim_at(&self, key: &Key) -> Option<At> {
        if let Some(i) = self
            .cache
            .slots
            .iter()
            .position(|s| matches!(s.data, Data::Anim(_)) && s.key == *key)
        {
            return Some(At::Slot(i));
        }
        let i = self.anims.outside.iter().position(|o| o.0 == *key)?;
        Some(At::Outside(i))
    }

    /// Where the animation `serial` is, if it is made.
    fn anim_of(&mut self, serial: u64) -> Option<At> {
        self.anims_at()
            .into_iter()
            .find(|&at| self.playing(at).serial == serial)
    }

    fn playing(&mut self, at: At) -> &mut anim::Playing {
        match at {
            At::Slot(i) => match &mut self.cache.slots[i].data {
                Data::Anim(p) => p,
                _ => unreachable!("an animation's slot"),
            },
            At::Outside(i) => &mut self.anims.outside[i].1,
        }
    }

    fn key_at(&self, at: At) -> &Key {
        match at {
            At::Slot(i) => &self.cache.slots[i].key,
            At::Outside(i) => &self.anims.outside[i].0,
        }
    }

    /// Every animation made, where it is.
    fn anims_at(&self) -> Vec<At> {
        let slots = self.cache.slots.iter().enumerate();
        slots
            .filter(|(_, s)| matches!(s.data, Data::Anim(_)))
            .map(|(i, _)| At::Slot(i))
            .chain((0..self.anims.outside.len()).map(At::Outside))
            .collect()
    }

    /// What the animations hold in the cache, and the reads for them.
    #[cfg(test)]
    fn anims_held(&self) -> usize {
        self.anim_slots_cost().saturating_add(self.cache.anims)
    }

    /// What the animations' slots hold.
    fn anim_slots_cost(&self) -> usize {
        let slots = self.cache.slots.iter();
        slots
            .filter(|s| matches!(s.data, Data::Anim(_)))
            .map(Slot::cost)
            .sum()
    }

    /// The state of the animation of `key` changed other than by a frame (it was let go, or
    /// refused): if it is in view, its still and its badge are painted.
    fn anim_changed(&mut self, key: &Key) {
        if self.placed_top(key).is_some() {
            self.repaint = true;
            (self.wake)();
        }
    }

    /// The top of the highest box image `key` was painted in in the last frame (`i32::MAX` for
    /// one whose pixels only were asked for), if it was.
    fn placed_top(&self, key: &Key) -> Option<i32> {
        let placed = self.anims.placed.iter();
        placed
            .filter(|p| self.key(p.0).as_ref() == Some(key))
            .map(|p| p.1.map_or(i32::MAX, |r| r.1))
            .min()
    }

    /// The animations painted in the last frame, one a key, in the order they are weighed in:
    /// on screen from the top down (one whose pixels only were asked for last). Each is its first
    /// image, and the frame size that image is painted at: the one size its frames are made at,
    /// whatever the size of its other images.
    fn anims_shown(&self) -> Vec<(u32, Key, (u32, u32))> {
        let mut placed: Vec<_> = (self.anims.placed.iter())
            .filter_map(|p| Some((p.1.map_or(i32::MAX, |r| r.1), p.0, p.3?)))
            .collect();
        placed.sort_unstable_by_key(|p| (p.0, p.1));
        // (Only a remote image can be the image of two ids.)
        let mut seen: std::collections::HashSet<&Arc<str>> = std::collections::HashSet::new();
        let mut shown = Vec::with_capacity(placed.len());
        for (_, id, size) in placed {
            if let Src::Remote(url) = self.src(id)
                && !seen.insert(url)
            {
                continue;
            }
            if let Some(key) = self.key(id) {
                shown.push((id, key, size));
            }
        }
        shown
    }

    /// Whether image `id` (of `key`) is played past the limits: the reader chose so, or chose
    /// its still to be loaded past them.
    fn anim_forced(&self, id: u32, key: &Key) -> bool {
        self.anims.forced.contains(key) || self.forced_ids.iter().any(|f| f.0 == id)
    }

    /// What reading the animation of image `id` holds before it plays (its file, its path, the
    /// index; a remote one's body is weighed as a body, [`Images::anim_body`]), found once (and
    /// again when [`Images::start_anim`] finds the file changed): `None` if its file can't be
    /// read any more.
    fn read_cost(&mut self, id: u32, key: &Key) -> Option<usize> {
        if let Some(r) = self.anims.reads.iter().find(|r| r.0 == *key) {
            return Some(r.1);
        }
        let Src::File(src) = self.src(id) else {
            return Some(anim::MEASURE_FIXED);
        };
        let path = local_path(src, self.base.as_deref())?;
        let len = std::fs::metadata(&path).ok()?.len().min(MAX_FILE) as usize;
        let cost = len
            .saturating_add(path.as_os_str().len() + STR_OVERHEAD)
            .saturating_add(anim::MEASURE_FIXED);
        if self.anims.reads.len() < VIEW_MAX {
            self.anims.reads.push((key.clone(), cost));
        }
        Some(cost)
    }

    /// What the allocation weighs of the animations `shown` ([`anim::Bid`]), in their order: those
    /// to play (wanted, and not refused, failed, ended or played past the limits) at what they
    /// hold, or will. One held at its frame size holds what it holds; one that isn't, what it was
    /// measured to hold there, and at least what reading it holds (it is read again to play), or
    /// only that until it is measured. One whose file went, or whose reading alone is over the
    /// limits, is refused here.
    fn anim_bids(&mut self, shown: &[(u32, Key, (u32, u32))]) -> Vec<anim::Bid> {
        let mut bids = Vec::with_capacity(shown.len());
        for (id, key, size) in shown {
            let slot = match self.anim_at(key) {
                Some(At::Slot(i)) => Some(&self.cache.slots[i]),
                _ => None,
            };
            let p = slot.and_then(|s| match &s.data {
                Data::Anim(p) => Some(&**p),
                _ => None,
            });
            if !self.anims.wanted(key)
                || self.anim_forced(*id, key)
                || self.anims.refusal(key).is_some()
                || self.anims.failed.contains(key)
                || self.anim_ended(key)
            {
                continue;
            }
            // (What a slot holds beside its animation: its overhead, its URL.)
            let held = slot.zip(p).and_then(|(s, p)| {
                let at = p.cost_at(*size)?;
                Some(at.saturating_add(s.cost() - p.cost))
            });
            let cost = match held {
                Some(cost) => cost,
                None => {
                    let Some(read) = self.read_cost(*id, key) else {
                        self.anims.refuse(key.clone(), false);
                        self.anim_changed(key);
                        continue;
                    };
                    if read > anim::ANIM_ALL_MAX {
                        self.anims.refuse(key.clone(), true);
                        self.anim_changed(key);
                        continue;
                    }
                    self.anims.cost(key, *size).map_or(read, |c| c.max(read))
                }
            };
            bids.push(anim::Bid {
                key: key.clone(),
                size: *size,
                cost,
                body: self.anim_body(key),
            });
        }
        bids
    }

    /// What the body of the remote animation `key` holds beside its slot once it plays, cached
    /// (held for it, or not), or to be fetched again for it: then with what fetching it and its
    /// decode need beside it, the test [`Images::body_for_anim`] fetches it at (0 for a local
    /// one). Letting the animation go frees it ([`Images::anim_room`]).
    fn anim_body(&self, key: &Key) -> usize {
        #[cfg(feature = "remote-images")]
        if let Key::Remote(url) = key {
            let cached = self.cache.peek_bytes(url).map(|b| Slot::bytes_cost(url, b));
            let fetched = self
                .deferred_need(url)
                .map(|(cost, decode)| (cost + decode).saturating_add(crate::remote::FETCH_RESERVE));
            return cached.or(fetched).unwrap_or(0);
        }
        let _ = key;
        0
    }

    /// The room the cache has for the animations painted: its budget less what stays whatever
    /// they do (the bookkeeping, the pixels painted in this frame, the steps of animations gone).
    /// The bodies remote animations play from are their bids' ([`Images::anim_body`]): a paused
    /// one's goes with it. One played past the limits plays from the cached body it was read
    /// from, if it was: that body stays while it does (it bids for nothing, and isn't let go for
    /// room), so it is fixed too. What comes and goes as images load (the decodes' reserve, the
    /// fetchers', a request's wishes) is left out, so that the allocation doesn't come and go
    /// with it: a read waits for it instead ([`Images::anim_room`]).
    fn anims_room_now(&self) -> usize {
        let frame = self.cache.frame;
        let fixed: usize = (self.cache.slots.iter())
            .filter(|s| match &s.data {
                Data::Pixels(_) => s.painted_in(frame),
                Data::Bytes(_) => s.pinned() && self.anims.outside.iter().any(|o| o.0 == s.key),
                _ => false,
            })
            .map(Slot::cost)
            .sum();
        let gone: usize = (self.anims.out.iter())
            .filter(|o| o.size == (0, 0))
            .map(|o| o.cost)
            .sum();
        self.cache
            .cap
            .saturating_sub(fixed)
            .saturating_sub(self.cache.overhead)
            .saturating_sub(gone)
    }

    /// Make the allocation again ([`anim::allocate`]) if what it weighs changed.
    fn reallocate(&mut self, shown: &[(u32, Key, (u32, u32))]) {
        anim::retain_keys(&mut self.anims.stalled, &|k| {
            shown.iter().any(|s| s.1 == *k)
        });
        let bids = self.anim_bids(shown);
        let room = self.anims_room_now();
        let alloc = &self.anims.alloc;
        if bids != alloc.bids || room != alloc.room {
            self.anims.alloc = anim::allocate(bids, room);
        }
    }

    /// Make room for an animation the allocation lets play: `slot` beside the animations' slots
    /// within [`anim::ANIM_ALL_MAX`], and `max(slot, read)` in the cache (it is read, then holds
    /// its slot). The animations let go for it are those the allocation doesn't play (paused,
    /// ended, off screen): those not painted first, least recently used first, then those
    /// painted, from the bottom up. One running a step is not (it is asked for no more, and let go
    /// once it is back). Nothing goes if not enough can. Whether it fits.
    fn anim_room(&mut self, slot: usize, read: usize) -> bool {
        let frame = self.cache.frame;
        let need = slot.max(read);
        let held = self.anim_slots_cost();
        let mut can = Vec::new();
        for s in &self.cache.slots {
            let Data::Anim(p) = &s.data else {
                continue;
            };
            if p.running || self.anims.alloc.play.contains(&s.key) {
                continue;
            }
            let rank = match self.placed_top(&s.key) {
                Some(top) if s.painted_in(frame) => (true, -i64::from(top)),
                _ => (false, s.used as i64),
            };
            let stuck = if s.unevictable(frame) { s.cost() } else { 0 };
            can.push((
                rank,
                s.cost(),
                stuck + self.body_freed(&s.key),
                s.key.clone(),
            ));
        }
        can.sort_unstable_by_key(|c| c.0);
        self.charge_bookkeeping();
        let (mut freed, mut stuck, mut keys) = (0usize, 0usize, Vec::new());
        let mut can = can.into_iter();
        while held.saturating_sub(freed).saturating_add(slot) > anim::ANIM_ALL_MAX
            || !self.cache.can_fit_without(need, stuck)
        {
            let Some((_, cost, unstuck, key)) = can.next() else {
                return false;
            };
            freed = freed.saturating_add(cost);
            stuck = stuck.saturating_add(unstuck);
            keys.push(key);
        }
        for key in keys {
            if let Some(at @ At::Slot(_)) = self.anim_at(&key) {
                self.drop_anim(at);
            }
        }
        true
    }

    /// What letting the held animation of `key` go frees beside its slot: the body of a remote
    /// one, if nothing else holds it (a step, a decode).
    fn body_freed(&self, key: &Key) -> usize {
        #[cfg(feature = "remote-images")]
        if let Key::Remote(url) = key {
            let body = self
                .cache
                .peek_bytes(url)
                .filter(|b| Arc::strong_count(b) == 2);
            return body.map_or(0, |b| Slot::bytes_cost(url, b));
        }
        let _ = key;
        0
    }

    /// Image `id` is painted at `w` × `h`: whether its animation has a frame to show. Its frames
    /// are made at [`anim::frame_size`] (which the paint enlarges to the box): that size is noted
    /// for the allocation ([`Images::pump_anims`]), which plays it or not.
    fn anim_shows(&mut self, id: u32, w: u32, h: u32) -> bool {
        let Some(key) = self.key(id) else {
            return false;
        };
        let Some((_, info)) = self.animated.iter().find(|(k, _)| *k == key) else {
            return false;
        };
        let size = anim::frame_size(info.size, (w, h));
        let placed = &mut self.anims.placed;
        if let Some(p) = placed.iter_mut().find(|p| p.0 == id) {
            p.3 = Some(size);
        } else if placed.len() < VIEW_MAX {
            placed.push((id, None, None, Some(size)));
        }
        let frame = self.cache.frame;
        // Its still is painted as well, whatever is shown: it is what shows when the animation
        // goes, and one asked for and not painted would be evicted by the next still's decode, to
        // be asked for again, and so on.
        let (cw, ch) = clamp_box(w, h);
        self.cache.pixels(&key, cw, ch);
        let Some(at) = self.anim_at(&key) else {
            return false;
        };
        // (Painted with its still while its first frame is made: it is on screen all the same.)
        let clock = self.anims.clock;
        let p = self.playing(at);
        p.painted = frame.saturating_add(1);
        let shows = p.front().is_some();
        if shows && p.shown.is_none() {
            p.shown = Some(clock.unwrap_or_else(std::time::Instant::now));
        }
        if let At::Slot(i) = at {
            let slot = &mut self.cache.slots[i];
            slot.used = frame;
            slot.painted = frame.saturating_add(1);
        }
        shows
    }

    /// The frame shown of image `id`'s animation ([`Images::anim_shows`] said it has one).
    fn anim_front(&mut self, id: u32) -> Option<&Bitmap> {
        let at = self.anim_at(&self.key(id)?)?;
        self.playing(at).front()
    }

    /// Whether an animation painted in the last frame is in another state than it was painted
    /// in (it ended, was refused, waits, plays): its badge, or its lack of one, is stale, and
    /// the window needs a repaint. Every change is asked for once, here, whatever made it.
    #[cfg(feature = "images")]
    fn state_changed(&mut self) -> bool {
        let mut changed = false;
        for i in 0..self.anims.placed.len() {
            let state = self.anim_state(self.anims.placed[i].0);
            let painted = &mut self.anims.placed[i].2;
            if *painted != state {
                *painted = state;
                changed = true;
            }
        }
        changed
    }

    /// Make what is made of the animations what the allocation says: those painted that wait
    /// (or are to play past the limits) and are held go (once a step running is back), their
    /// stills shown; those it plays that aren't made are read, room made for them first; those
    /// played past the limits are read as they are painted. How many things it set going: none,
    /// once it is so. (Those held that don't play, paused or ended, stay until their room is
    /// wanted: [`Images::anim_room`].)
    fn settle_anims(&mut self, shown: &[(u32, Key, (u32, u32))]) -> usize {
        let mut n = 0;
        for (id, key, _) in shown {
            let forced = self.anim_forced(*id, key);
            if !(forced || self.anims.waits(key)) {
                continue;
            }
            if let Some(at @ At::Slot(_)) = self.anim_at(key)
                && !self.playing(at).running
            {
                self.drop_anim(at);
                n += 1;
            }
        }
        for (id, key, size) in shown {
            let cost = if self.anim_forced(*id, key) {
                if !self.anims.wanted(key) || self.anim_ended(key) {
                    continue;
                }
                None
            } else {
                let alloc = &self.anims.alloc;
                if !alloc.play.contains(key) {
                    continue;
                }
                let bid = alloc.bids.iter().find(|b| b.key == *key);
                Some(bid.map_or(0, |b| b.cost))
            };
            n += usize::from(self.start_anim(*id, key.clone(), *size, cost));
        }
        n
    }

    /// Weigh the animations painted and do what that says ([`Images::settle_anims`], until it
    /// says nothing more: letting one go changes what the others are weighed with), then run
    /// the steps of those whose frame shown was painted: those it plays, and those played past
    /// the limits.
    fn pump_anims(&mut self) {
        if self.anims.idle() {
            return;
        }
        let shown = self.anims_shown();
        let ceiling = self.forced.ceiling.unwrap_or_else(forced::ceiling);
        // (Each is made at the frame size of its first image painted, whatever its others'. One
        // played past the limits is made at a new one only within the forced ceiling, as it was
        // at its first: else it plays on at its own, enlarged to the box.)
        for (_, key, size) in &shown {
            if let Some(at) = self.anim_at(key) {
                let p = self.playing(at);
                let outside = matches!(at, At::Outside(_));
                if !outside || *size == p.size || p.force(ceiling, *size) {
                    p.want = *size;
                } else {
                    p.want = p.size;
                }
            }
        }
        // (Each round lets go, or reads, what it does once: it ends with one that does
        // nothing.)
        let mut rounds = 0;
        loop {
            self.reallocate(&shown);
            let n = self.settle_anims(&shown);
            #[cfg(test)]
            {
                self.anims.effects += n;
            }
            if n == 0 {
                break;
            }
            rounds += 1;
            if rounds > 3 * shown.len() + 2 {
                debug_assert!(false, "the allocation settles");
                break;
            }
        }
        // (Room made for one may evict another, which moves the slots: each is found anew.)
        let frame = self.cache.frame;
        let serials: Vec<u64> = self
            .anims_at()
            .into_iter()
            .map(|at| self.playing(at).serial)
            .collect();
        for serial in serials {
            let Some(at) = self.anim_of(serial) else {
                continue;
            };
            let p = self.playing(at);
            // One played past the limits goes once it isn't painted (it isn't the cache's to
            // evict): it is read again when it is.
            if let At::Outside(_) = at
                && p.painted < frame
                && !p.running
            {
                self.drop_anim(at);
                continue;
            }
            let resized = p.want != p.size && (p.engine.is_some() || p.is_kept());
            let wants = p.wants_step(frame) || (resized && !p.running);
            let key = self.key_at(at);
            let plays = matches!(at, At::Outside(_)) || self.anims.alloc.play.contains(key);
            if wants && plays && self.anims.wanted(key) {
                self.step_anim(at);
            }
        }
        self.anims.live = !self.anims_at().is_empty();
    }

    /// The URL of the fetched image `key` to fetch again, to play its animation: one that is
    /// to play (the allocation plays it, or the reader chose to past the limits), whose body is
    /// no longer cached (it is played from it), listed until it comes. Remote images are on: it
    /// is fetched as any image is ([`Images::request`]), and only then.
    #[cfg(feature = "remote-images")]
    fn body_for_anim(&mut self, key: &Key, reserve: usize) -> Option<Arc<str>> {
        let Key::Remote(url) = key else {
            return None;
        };
        let wanted = self.remote.on
            && self.remote.size(url) != Some(None)
            && self.animated.iter().any(|(k, _)| k == key)
            && self.anims.wanted(key)
            && !self.anims.failed.contains(key)
            && match self.anims.refusal(key) {
                None => true,
                Some(over) => over && self.anims.forced.contains(key),
            }
            && (self.anims.fetched.contains(key) || self.anims.fetched.len() < VIEW_MAX)
            && self.anim_at(key).is_none()
            && !self.anims.out.iter().any(|o| o.key == *key)
            && self.cache.peek_bytes(url).is_none();
        if !wanted {
            return None;
        }
        // One asked for is listed again until it comes (each request replaces the fetchers'
        // queue; one in flight isn't fetched twice).
        if self.anims.fetched.contains(key) {
            return Some(url.clone());
        }
        // One dropped for room is fetched again once it would fit; one the reader chose to play
        // past the limits is fetched past them, as a click on its still does ([`Images::force`]),
        // and plays from that body.
        if let Some((cost, decode)) = self.deferred_need(url)
            && !self
                .cache
                .would_fit(cost, decode, reserve, self.fetch_reserve(true))
        {
            let origin = forced::Origin::Url(url.clone());
            if self.anims.forced.contains(key) && !self.forced_ids.iter().any(|f| f.1 == origin) {
                let ids = self.ids_of(std::slice::from_ref(key));
                self.force_ids(&origin, &ids);
            }
            return None;
        }
        // (One that waits is fetched again once the allocation plays it, or a click does.)
        if !self.anims.forced.contains(key) && !self.anims.alloc.play.contains(key) {
            return None;
        }
        self.anims.fetched.push(key.clone());
        Some(url.clone())
    }

    /// Have the anim thread read and index image `id`'s animation for the frame size `size`:
    /// within the limits, where the allocation gave it `cost` (room made for it first), or past
    /// them (`None`). Nothing is done if it is made or being read, or if the room isn't free yet
    /// (a step of one let go for it is running, a decode holds it): a later pump does it. One
    /// whose file can't be read any more is refused (no badge, nothing read again). Whether it
    /// was set going.
    fn start_anim(&mut self, id: u32, key: Key, size: (u32, u32), cost: Option<usize>) -> bool {
        let doc_gen = self.doc_gen;
        if self.anim_at(&key).is_some()
            || self
                .anims
                .out
                .iter()
                .any(|o| o.key == key && o.doc_gen == doc_gen)
            || self.anims.refusal(&key) == Some(false)
        {
            return false;
        }
        let forced = cost.is_none();
        let forced_id = self
            .forced_ids
            .iter()
            .find(|f| f.0 == id)
            .map(|f| f.1.clone());
        let source = match (&forced_id, self.src(id).clone()) {
            (Some(forced::Origin::File(path)), _) => Source::File(path.clone()),
            (Some(forced::Origin::Url(url)), _) => match self.forced.body(url) {
                Some(b) => Source::Bytes(b),
                None => return false,
            },
            (None, Src::File(src)) => match local_path(&src, self.base.as_deref()) {
                Some(path) => Source::File(path),
                None => return false,
            },
            // (Played from its body, kept for it or fetched again for it as images are
            // fetched: [`Images::body_for_anim`].)
            (None, Src::Remote(url)) => match self.cache.bytes_of(&url) {
                Some(b) => Source::Bytes(b),
                // (Room is made for it to be fetched again in: those it lets go don't come back.)
                None => {
                    if let Some(cost) = cost
                        && !self.anim_room(cost, self.anim_body(&key))
                    {
                        self.stall(&key);
                    }
                    return false;
                }
            },
            (None, Src::None) => return false,
        };
        let (own, path) = match &source {
            Source::File(p) => match std::fs::metadata(p) {
                Ok(m) => (
                    m.len().min(MAX_FILE) as usize,
                    p.as_os_str().len() + STR_OVERHEAD,
                ),
                Err(_) => {
                    self.anims.refuse(key.clone(), false);
                    self.anim_changed(&key);
                    return true;
                }
            },
            Source::Bytes(_) => (0, 0),
        };
        let read = if forced {
            0
        } else {
            own.saturating_add(path).saturating_add(anim::MEASURE_FIXED)
        };
        // A file that changed since it was weighed is weighed (and measured) anew: the allocation
        // decides again whether it plays at what it holds now.
        if let Some(r) = self.anims.reads.iter_mut().find(|r| r.0 == key)
            && !forced
            && r.1 != read
        {
            r.1 = read;
            self.anims.retain_costs(&|c| c.0 != key);
            return true;
        }
        if let Some(cost) = cost
            && !(self.anim_room(cost, read) && self.make_room("anim index", read))
        {
            self.stall(&key);
            return false;
        }
        anim::retain_keys(&mut self.anims.stalled, &|k| *k != key);
        #[cfg(test)]
        if !forced && self.anims.cost(&key, size).is_none() {
            self.anims.measures.push((key.clone(), size));
        }
        let serial = self.anims.next_serial();
        let cancel = Arc::new(AtomicBool::new(false));
        self.anims.out.push(anim::Out {
            serial,
            key,
            doc_gen,
            size,
            cost: read,
            forced,
            cancel: cancel.clone(),
        });
        self.cache.anims = self.cache.anims.saturating_add(read);
        self.anims.live = true;
        let task = anim::Task::Measure {
            source,
            limit: if forced { usize::MAX } else { own },
        };
        let job = anim::Job {
            serial,
            task,
            cancel,
            wake: self.wake.clone(),
        };
        self.anims.player.send(job, &self.tx);
        true
    }

    /// The animation of `key`, which the allocation plays, can't be given its room: if nothing
    /// under way gives any back (a step of one it doesn't play, a read, a decode, a fetch), the
    /// room weighed isn't there, and it waits ([`anim::Anims::waits`]: its badge says so, and a
    /// click plays it past the limits) rather than seem to play without a frame.
    fn stall(&mut self, key: &Key) {
        let play = &self.anims.alloc.play;
        let stepping = (self.cache.slots.iter())
            .any(|s| matches!(&s.data, Data::Anim(p) if p.running && !play.contains(&s.key)));
        let decoding = !(self.worker.as_ref().is_none_or(Worker::idle)
            && self.retired.iter().all(Worker::idle_of));
        #[cfg(feature = "remote-images")]
        let decoding = decoding || self.remote.busy();
        let reading = !self.anims.out.is_empty();
        if !(stepping || decoding || reading || self.anims.stalled.contains(key)) {
            self.anims.stalled.push(key.clone());
        }
    }

    /// The stalled animation of image `id` ([`Images::stall`]), which the allocation plays, is
    /// given its room now if it is there: it came back while nothing started it (it was paused),
    /// and then it plays within the limits, not past them.
    fn unstall(&mut self, id: u32, key: &Key) {
        if !self.anims.stalled.contains(key) || !self.anims.alloc.play.contains(key) {
            return;
        }
        let Some(cost) = (self.anims.alloc.bids.iter())
            .find(|b| b.key == *key)
            .map(|b| b.cost)
        else {
            return;
        };
        // (As [`Images::start_anim`] weighs it: a remote one whose body went is fetched again.)
        #[cfg(feature = "remote-images")]
        let fetch = matches!(key, Key::Remote(url) if self.cache.peek_bytes(url).is_none());
        #[cfg(not(feature = "remote-images"))]
        let fetch = false;
        let read = if fetch {
            Some(self.anim_body(key))
        } else {
            self.read_cost(id, key)
        };
        if read.is_some_and(|read| self.anim_room(cost, read)) {
            anim::retain_keys(&mut self.anims.stalled, &|k| k != key);
        }
    }

    /// Run the next step of the animation at `at` on the anim thread: made at the box it was
    /// last painted in, which is made room for first if that holds more. One whose frames are
    /// kept is read again for a new box; one that doesn't fit at it stops.
    fn step_anim(&mut self, at: At) {
        let p = self.playing(at);
        if p.want != p.size {
            if !p.resize() {
                return self.drop_anim(at);
            }
            if let At::Slot(_) = at {
                let (old, new) = (p.cost, p.charge());
                let grow = new.saturating_sub(old);
                let key = self.key_at(at).clone();
                if new > anim::ANIM_ONE_MAX
                    || !(self.anim_room(grow, 0) && self.make_room("anim box", grow))
                {
                    // (Room was made: the slot may have moved.)
                    if let Some(at) = self.anim_at(&key) {
                        self.drop_anim(at);
                    }
                    return;
                }
                // (`make_room` evicts: find it again.)
                let Some(at) = self.anim_at(&key) else {
                    return;
                };
                self.recharge(at);
                return self.send_step(at);
            }
        }
        self.send_step(at);
    }

    fn send_step(&mut self, at: At) {
        let wake = self.wake.clone();
        let p = self.playing(at);
        let Ok((engine, size, spare)) = p.task() else {
            return;
        };
        let job = anim::Job {
            serial: p.serial,
            task: anim::Task::Step {
                engine,
                size,
                spare,
            },
            cancel: p.cancel.clone(),
            wake,
        };
        self.anims.player.send(job, &self.tx);
    }

    /// Set the charge of the animation at `at` to what it holds now ([`anim::Playing::charge`]).
    fn recharge(&mut self, at: At) {
        let p = self.playing(at);
        let (old, new) = (p.cost, p.charge());
        p.cost = new;
        if let At::Slot(_) = at {
            self.cache.bytes = self.cache.bytes - old + new;
        }
    }

    /// Whether the animation of `key` has played its loops: its last frame stays while it is
    /// held, its still once it is let go, and it isn't played again until the reader asks.
    fn anim_ended(&self, key: &Key) -> bool {
        self.anims.ended.contains(key)
    }

    /// Note it if the animation at `at` has played its loops (it stays so, held or not).
    fn note_ended(&mut self, at: At) {
        if self.playing(at).ended() && !self.anim_ended(self.key_at(at)) {
            let key = self.key_at(at).clone();
            self.anims.ended.push(key);
        }
    }

    /// The animations that have played their loops (of `key`, or all) play again from their
    /// first frame: those whose frames are kept at once, the others read again (their still
    /// shows meanwhile).
    fn restart_ended(&mut self, key: Option<&Key>) {
        match key {
            Some(key) => anim::retain_keys(&mut self.anims.ended, &|k| k != key),
            None => self.anims.ended = Vec::new(),
        }
        for at in self.anims_at().into_iter().rev() {
            if key.is_some_and(|k| k != self.key_at(at)) {
                continue;
            }
            let p = self.playing(at);
            if p.ended() && !p.rewind() {
                self.drop_anim(at);
            }
        }
    }

    /// Drop the animation at `at` (not running): its image shows its still until it is read
    /// again.
    fn drop_anim(&mut self, at: At) {
        let key = self.key_at(at).clone();
        self.anim_changed(&key);
        match at {
            At::Slot(i) => {
                self.cache.remove(i);
                self.cache.trim();
            }
            At::Outside(i) => {
                self.anims.outside.swap_remove(i);
            }
        }
    }

    /// Whether the document or the cache holds the image of `key` (a remote one's URL is held by
    /// the document's list or by a slot): what is known of an animation is kept only then.
    fn anim_owned(&self, key: &Key) -> bool {
        match key {
            Key::Local(_) => true,
            #[cfg(feature = "remote-images")]
            Key::Remote(url) => self.urls.contains(url) || self.cache.has_url(url),
            #[cfg(not(feature = "remote-images"))]
            Key::Remote(_) => false,
        }
    }

    /// The anim thread read and indexed the animation of job `serial`: it plays, if it is still
    /// wanted and fits, from its first frame.
    fn anim_measured(&mut self, serial: u64, engine: Option<Box<anim::Engine>>) {
        let Some(i) = self.anims.out.iter().position(|o| o.serial == serial) else {
            return;
        };
        let out = self.anims.out.swap_remove(i);
        self.cache.anims -= out.cost;
        // (Its body, if it was fetched again for it, was read: it may be again.)
        anim::retain_keys(&mut self.anims.fetched, &|k| *k != out.key);
        let stale = matches!(out.key, Key::Local(_)) && out.doc_gen != self.doc_gen;
        // (A remote one nothing holds any more leaves nothing behind, a refusal included.)
        if stale
            || !self.anim_owned(&out.key)
            || !self.anims.wanted(&out.key)
            || self.anim_at(&out.key).is_some()
        {
            return;
        }
        let Some(mut engine) = engine else {
            return self.anims.refuse(out.key, false);
        };
        if out.forced {
            let ceiling = self.forced.ceiling.unwrap_or_else(forced::ceiling);
            if !engine.force(ceiling, out.size) {
                return self.anims.refuse(out.key, false);
            }
            let mut p = anim::Playing::new(serial, engine, out.size, false);
            // (It was painted when it was asked for.)
            p.painted = self.cache.frame.saturating_add(1);
            self.anims.outside.push((out.key, Box::new(p)));
            self.anims.live = true;
            return self.send_step(At::Outside(self.anims.outside.len() - 1));
        }
        let keep = engine.keeps(out.size);
        if !keep {
            engine.lean(out.size, anim::ANIM_ONE_MAX);
        }
        let p = anim::Playing::new(serial, engine, out.size, keep);
        let slot = p
            .cost
            .saturating_add(SLOT_OVERHEAD)
            .saturating_add(match &out.key {
                Key::Local(_) => 0,
                Key::Remote(url) => str_bytes(url),
            });
        if p.cost > anim::ANIM_ONE_MAX {
            return self.anims.refuse(out.key, true);
        }
        // What it holds is known now: it plays if the allocation, weighing that, plays it at the
        // size it was read for, and there is room (else it is read again when there is).
        let shown = self.anims_shown();
        self.anims
            .note_cost(out.key.clone(), out.size, slot, &shown);
        self.reallocate(&shown);
        let size = shown.iter().find(|s| s.1 == out.key).map(|s| s.2);
        if size != Some(out.size)
            || !self.anims.alloc.play.contains(&out.key)
            || !self.anim_room(slot, 0)
        {
            return;
        }
        let key = out.key.clone();
        if self.store(out.key, Data::Anim(Box::new(p)), true).is_err() {
            return;
        }
        self.anims.live = true;
        if let Some(at) = self.anim_at(&key) {
            self.send_step(at);
        }
    }

    /// The anim thread did job `serial`: whether an animation has a frame to show that it
    /// didn't have (its first).
    fn anim_done(&mut self, serial: u64, output: anim::Output) -> bool {
        match output {
            anim::Output::Measured(engine) => {
                self.anim_measured(serial, engine);
                false
            }
            anim::Output::Stepped(engine, made) => {
                let shown = self
                    .anim_of(serial)
                    .map(|at| self.playing(at).front().is_some());
                self.anim_stepped(serial, engine, made);
                let now = self
                    .anim_of(serial)
                    .map(|at| self.playing(at).front().is_some());
                shown == Some(false) && now == Some(true)
            }
        }
    }

    /// The anim thread made a step of the animation of `serial`: it is back with its result.
    /// One that went meanwhile (its document did) is let go, and its charge with it.
    fn anim_stepped(&mut self, serial: u64, engine: Box<anim::Engine>, made: anim::Made) {
        let at = self
            .anims_at()
            .into_iter()
            .find(|&at| self.playing(at).serial == serial);
        let Some(at) = at else {
            if let Some(i) = self.anims.out.iter().position(|o| o.serial == serial) {
                self.cache.anims -= self.anims.out.swap_remove(i).cost;
            }
            return;
        };
        self.playing(at).took(engine, made);
        self.recharge(at);
        self.note_ended(at);
    }

    /// A document is attached: the old one's local animations go (a step running is let go
    /// when it is back, charged until then), and so do the choices made for its images.
    fn detach_anims(&mut self) {
        let doc_gen = self.doc_gen;
        for o in &self.anims.out {
            if matches!(o.key, Key::Local(_)) {
                o.cancel.store(true, Ordering::Relaxed);
            }
        }
        for at in self.anims_at().into_iter().rev() {
            if !matches!(self.key_at(at), Key::Local(_)) {
                continue;
            }
            let p = self.playing(at);
            if p.running {
                p.cancel.store(true, Ordering::Relaxed);
                let (serial, cost) = match at {
                    At::Slot(i) => (p.serial, self.cache.slots[i].cost()),
                    At::Outside(_) => (p.serial, 0),
                };
                let key = self.key_at(at).clone();
                self.anims.out.push(anim::Out {
                    serial,
                    key,
                    doc_gen,
                    size: (0, 0),
                    cost,
                    forced: false,
                    cancel: Arc::default(),
                });
                self.cache.anims = self.cache.anims.saturating_add(cost);
            }
            match at {
                At::Slot(i) => self.cache.remove(i),
                At::Outside(i) => {
                    self.anims.outside.swap_remove(i);
                }
            }
        }
        self.anims.choices = Vec::new();
        // (The remote ones are pruned to the new document's URLs with the other records.)
        keep_anims(&mut self.animated, &mut self.anims, &|k| {
            matches!(k, Key::Remote(_))
        });
        self.anims.reads = Vec::new();
        self.anims.alloc = anim::Alloc::default();
        self.anims.fetched = Vec::new();
        self.anims.failed = Vec::new();
        self.anims.placed = Vec::new();
    }

    /// The ids of the document's images of `keys`.
    fn ids_of(&self, keys: &[Key]) -> Vec<u32> {
        let mut ids = Vec::new();
        for (i, src) in self.srcs.iter().enumerate() {
            let id = i as u32 + 1;
            let key = match src {
                Src::File(_) => Key::Local(id),
                Src::Remote(url) => Key::Remote(url.clone()),
                Src::None => continue,
            };
            if keys.contains(&key) {
                ids.push(id);
            }
        }
        ids
    }
}

/// Forget what is known of the animations whose keys `keep` doesn't keep: that they are
/// `animated`, and the rest ([`anim::Anims::keep`]); the room their lists held is given back.
#[cfg(feature = "images")]
fn keep_anims(
    animated: &mut Vec<(Key, AnimInfo)>,
    anims: &mut anim::Anims,
    keep: &dyn Fn(&Key) -> bool,
) {
    animated.retain(|(k, _)| keep(k));
    anims.keep(keep);
    shrink_vec(animated);
    shrink_vec(&mut anims.costs);
    shrink_vec(&mut anims.refused);
    shrink_vec(&mut anims.forced);
}

impl crate::render::Pixels for Images {
    #[cfg(feature = "images")]
    fn placed(&mut self, id: u32, rect: (i32, i32, i32, i32)) {
        if self.anims.idle() || self.animated(id).is_none() {
            return;
        }
        // (Painted in the state it is in now, whichever paint it is.)
        let state = self.anim_state(id);
        let placed = &mut self.anims.placed;
        if let Some(p) = placed.iter_mut().find(|p| p.0 == id) {
            (p.1, p.2) = (Some(rect), state);
        } else if placed.len() < VIEW_MAX {
            placed.push((id, Some(rect), state, None));
        }
    }

    #[cfg(feature = "images")]
    fn paused(&self, id: u32) -> bool {
        self.anim_state(id).is_some_and(|s| s != AnimState::Playing)
    }

    fn pixels(&mut self, id: u32, w: u32, h: u32) -> Option<&Bitmap> {
        // Loaded past the limits: those pixels, at whatever size they were last (a zoom shows
        // them until the new ones come), else what the cache has (an SVG drawn without its
        // embedded images, until it is drawn with them).
        // An animation shows its frame once it has one (playing or paused), else its still.
        #[cfg(feature = "images")]
        if !self.anims.idle() && self.anim_shows(id, w, h) {
            return self.anim_front(id);
        }
        #[cfg(feature = "images")]
        if let Some((_, origin)) = self.forced_ids.iter().find(|f| f.0 == id) {
            let frame = self.cache.frame;
            if self.forced.pixels(origin, frame).is_some() {
                return self.forced.pixels(origin, frame);
            }
        }
        let key = self.key(id)?;
        let (w, h) = clamp_box(w, h);
        self.cache.pixels(&key, w, h)
    }
}

#[cfg(all(test, feature = "images"))]
mod tests;
