//! Images: where an image's file is, its size, decoding PNG and JPEG,
//! and the cache of decoded images with the worker thread that fills it.
//!
//! Layout only needs an image's size, which comes from its header (a few bytes, read when its
//! block is first measured), so the page doesn't jump when the pixels arrive. Pixels are decoded
//! lazily, for boxes within a screen of the viewport, on a worker thread, straight to the size
//! the box is shown at; the full-size pixels are dropped. Decoded images (and the bytes of
//! fetched remote ones, see `remote.rs`) share a ~32 MB budget; the least recently painted go
//! first, never one painted in the current frame.
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
/// Bytes of a JPEG looked through for its size, at most (segments before the frame header are
/// metadata: EXIF, color profiles, thumbnails).
const MAX_HEADER_SCAN: u64 = 4 << 20;

/// Calls the event loop back when a result is waiting ([`Images::poll`]).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

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
/// data or a zero size.
pub fn header(r: &mut impl Read) -> Option<(Format, u32, u32)> {
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
            // PNG sizes are at most 2³¹ − 1.
            let ok = (1..=i32::MAX as u32).contains(&w) && (1..=i32::MAX as u32).contains(&h);
            ok.then_some((Format::Png, w, h))
        }
        [0xFF, 0xD8] => jpeg_size(r).map(|(w, h)| (Format::Jpeg, w, h)),
        _ => None,
    }
}

/// The size in a JPEG's frame header, the segments after the start-of-image marker walked
/// (and skipped) until it. Every marker is taken to have a length, as the decoder does.
fn jpeg_size(r: &mut impl Read) -> Option<(u32, u32)> {
    let mut byte = || {
        let mut b = [0u8; 1];
        r.read_exact(&mut b).ok().map(|_| b[0])
    };
    let mut scanned = 0u64;
    loop {
        if byte()? != 0xFF {
            return None;
        }
        // Any number of fill bytes may come before the marker.
        let mut marker = 0xFF;
        while marker == 0xFF {
            marker = byte()?;
            scanned += 1;
            if scanned > MAX_HEADER_SCAN {
                return None;
            }
        }
        // The image ended, or its data began, before any frame header. (TEM and RSTn have no
        // segment in the standard, but the decoder skips a length for them like for any marker
        // it doesn't know: the two must agree on where the frame header is, see `decode`.)
        if matches!(marker, 0xD9 | 0xDA) {
            return None;
        }
        let len = u16::from_be_bytes([byte()?, byte()?]);
        if len < 2 {
            return None;
        }
        // SOF0..SOF15, but not DHT (C4), JPG (C8) or DAC (CC).
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let mut sof = [0u8; 5];
            for b in &mut sof {
                *b = byte()?;
            }
            let h = u32::from(u16::from_be_bytes([sof[1], sof[2]]));
            let w = u32::from(u16::from_be_bytes([sof[3], sof[4]]));
            // A height of 0 is defined later in the data (DNL): not worth supporting.
            return (w > 0 && h > 0).then_some((w, h));
        }
        let skip = u64::from(len - 2);
        scanned += skip + 2;
        if scanned > MAX_HEADER_SCAN {
            return None;
        }
        for _ in 0..skip {
            byte()?;
        }
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
/// PNG or JPEG, or too large to decode).
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

fn read_file(path: &Path) -> Option<Vec<u8>> {
    let file = open_regular(path)?;
    // `take` has no size hint: without the length the buffer doubles its way up, leaving up to
    // twice the file in capacity.
    let len = file.metadata().ok()?.len().min(MAX_FILE) as usize;
    let mut bytes = Vec::with_capacity(len + 1);
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_FILE).then_some(bytes)
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
            // The decoder may find a frame header other than the one `header` did (they read
            // odd markers differently): it gets the size we checked as its limit, and what it
            // reads must be that size, before it allocates anything.
            let options = DecoderOptions::default()
                .jpeg_set_out_colorspace(ColorSpace::RGB)
                .set_max_width(sw as usize)
                .set_max_height(sh as usize);
            let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
            d.decode_headers().ok()?;
            let info = d.info()?;
            if (u32::from(info.width), u32::from(info.height)) != (sw, sh) {
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

/// A PNG's pixels resampled to `w` × `h`. A PNG that isn't interlaced is read a row at a time
/// (`rows`; `false` reads the whole frame, as an interlaced one must be): its full-size pixels
/// can be hundreds of MB, and the memory the allocator then keeps would sit beside a full cache.
#[cfg(feature = "images")]
fn decode_png(bytes: &[u8], w: u32, h: u32, rows: bool) -> Option<Bitmap> {
    let limits = png::Limits {
        bytes: (MAX_PIXELS * 4) as usize + (1 << 20),
    };
    let mut d = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), limits);
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
        let mut frame = PngRows {
            reader,
            row: Vec::new(),
            next: 0,
        };
        return resample_rows(&mut frame, (width, height), channels, w, h);
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let frame = Frame {
        data: &buf,
        width: info.width,
        height: info.height,
        stride: info.line_size,
        channels,
    };
    resample(frame, w, h)
}

/// The source rows of an image, as [`resample_rows`] takes them: it only ever asks for the row
/// it last had or the next ones.
trait Rows {
    /// Row `y` (`channels` bytes a pixel, at least a row's worth; more is ignored), or `None` if
    /// it can't be had: the data ended, or `y` is before the row the source is at.
    fn row(&mut self, y: usize) -> Option<&[u8]>;
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

/// A PNG decoded a row at a time: `row` holds row `next - 1`.
#[cfg(feature = "images")]
struct PngRows<'a> {
    reader: png::Reader<std::io::Cursor<&'a [u8]>>,
    row: Vec<u8>,
    next: usize,
}

#[cfg(feature = "images")]
impl Rows for PngRows<'_> {
    fn row(&mut self, y: usize) -> Option<&[u8]> {
        while self.next <= y {
            let row = self.reader.next_row().ok()??;
            self.row.clear();
            self.row.extend_from_slice(row.data());
            self.next += 1;
        }
        (self.next == y + 1).then_some(&self.row)
    }
}

/// [`resample_rows`] for pixels in memory.
fn resample(mut frame: Frame, w: u32, h: u32) -> Option<Bitmap> {
    let size = (frame.width, frame.height);
    let channels = frame.channels;
    resample_rows(&mut frame, size, channels, w, h)
}

/// For each target pixel along one axis, the source pixels under it: (first, weights).
fn spans(src: u32, dst: u32) -> Vec<(usize, Vec<f32>)> {
    let f = f64::from(src) / f64::from(dst);
    (0..dst)
        .map(|t| {
            let (a, b) = (f64::from(t) * f, f64::from(t + 1) * f);
            let first = a.floor() as usize;
            let last = (b.ceil() as usize).min(src as usize).max(first + 1);
            let weights = (first..last)
                .map(|s| ((b.min(s as f64 + 1.0) - a.max(s as f64)) / f).max(0.0) as f32)
                .collect();
            (first, weights)
        })
        .collect()
}

/// Resample an image of `sw` × `sh` pixels to `w` × `h` by averaging the source area under each
/// target pixel, premultiplied (transparent pixels don't bleed their color). Shrinking averages
/// many pixels; enlarging (a HiDPI window) blends the one or two under each edge. Works a target
/// row at a time and takes the source rows in order, each target row sharing at most its first
/// source row with the last of the one before, so the only extra memory is one row of sums.
/// `None` if the source ends early.
fn resample_rows(
    rows: &mut impl Rows,
    (sw, sh): (u32, u32),
    channels: usize,
    w: u32,
    h: u32,
) -> Option<Bitmap> {
    let sw_px = sw as usize;
    let (xs, ys) = (spans(sw, w), spans(sh, h));
    let mut acc = vec![0f32; sw_px * 4];
    let mut data = Vec::with_capacity(w as usize * h as usize);
    for (y0, wy) in &ys {
        acc.fill(0.0);
        for (k, &weight) in wy.iter().enumerate() {
            let row = rows.row(y0 + k)?.get(..sw_px * channels)?;
            for (a, p) in acc.chunks_exact_mut(4).zip(row.chunks_exact(channels)) {
                let (r, g, b, alpha) = match channels {
                    1 => (p[0], p[0], p[0], 255),
                    2 => (p[0], p[0], p[0], p[1]),
                    3 => (p[0], p[1], p[2], 255),
                    _ => (p[0], p[1], p[2], p[3]),
                };
                let al = f32::from(alpha) * weight;
                a[0] += f32::from(r) * al;
                a[1] += f32::from(g) * al;
                a[2] += f32::from(b) * al;
                a[3] += al;
            }
        }
        for (x0, wx) in &xs {
            let mut px = [0f32; 4];
            for (k, &weight) in wx.iter().enumerate() {
                let a = &acc[(x0 + k) * 4..];
                for c in 0..4 {
                    px[c] += a[c] * weight;
                }
            }
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
    Some(Bitmap {
        width: w,
        height: h,
        data,
    })
}

// ---- The cache ------------------------------------------------------------------------------

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
    Bytes(Arc<[u8]>),
}

struct Slot {
    key: Key,
    data: Data,
    /// The frame it was last painted (or stored) in.
    used: u64,
}

impl Slot {
    fn cost(&self) -> usize {
        match &self.data {
            Data::Pixels(b) => b.data.len() * 4,
            Data::Bytes(b) => b.len(),
        }
    }

    fn pixels(&self) -> Option<&Bitmap> {
        match &self.data {
            Data::Pixels(b) => Some(b),
            Data::Bytes(_) => None,
        }
    }
}

/// Decoded images and fetched bytes under one memory budget, least recently painted first out.
struct Cache {
    slots: Vec<Slot>,
    bytes: usize,
    cap: usize,
    frame: u64,
}

impl Cache {
    fn new(cap: usize) -> Cache {
        Cache {
            slots: Vec::new(),
            bytes: 0,
            cap,
            frame: 0,
        }
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

    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    fn bytes_of(&mut self, url: &Arc<str>) -> Option<Arc<[u8]>> {
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
        self.retain(|s| !same(s));
        let slot = Slot {
            key,
            data,
            used: self.frame,
        };
        self.bytes += slot.cost();
        self.slots.push(slot);
        self.evict();
    }

    /// Drop the least recently painted slots until the budget holds, never one painted in the
    /// current frame.
    fn evict(&mut self) {
        while self.bytes > self.cap {
            let Some((i, _)) = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.used < self.frame)
                .min_by_key(|(_, s)| s.used)
            else {
                break;
            };
            let s = self.slots.swap_remove(i);
            self.bytes -= s.cost();
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(&Slot) -> bool) {
        let bytes = &mut self.bytes;
        self.slots.retain(|s| {
            let k = keep(s);
            if !k {
                *bytes -= s.cost();
            }
            k
        });
    }
}

// ---- The decode worker ------------------------------------------------------------------

pub(crate) enum Source {
    File(PathBuf),
    #[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
    Bytes(Arc<[u8]>),
}

/// Decode `source` to `w` × `h` for `key` (of document generation `doc_gen`).
pub(crate) struct Job {
    doc_gen: u64,
    key: Key,
    source: Source,
    w: u32,
    h: u32,
}

/// What the worker threads send back.
pub(crate) enum Done {
    Decoded {
        doc_gen: u64,
        key: Key,
        bitmap: Option<Bitmap>,
    },
    /// A remote image was fetched: its bytes and natural size, or why not.
    #[cfg(feature = "remote-images")]
    Fetched {
        url: Arc<str>,
        result: crate::remote::Fetched,
    },
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    /// The job being decoded: (key, size) — not queued again while it runs.
    busy: Option<(Key, u32, u32)>,
    quit: bool,
}

/// One thread decoding the most wanted image first; the queue is replaced as the viewport
/// moves, so images scrolled past are never decoded.
struct Worker {
    shared: Arc<(Mutex<Queue>, Condvar)>,
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
                    while q.jobs.is_empty() && !q.quit {
                        q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                    if q.quit {
                        return;
                    }
                    let job = q.jobs.remove(0);
                    q.busy = Some((job.key.clone(), job.w, job.h));
                    job
                };
                let bitmap = decode_job(&job);
                lock.lock().unwrap_or_else(|e| e.into_inner()).busy = None;
                let done = Done::Decoded {
                    doc_gen: job.doc_gen,
                    key: job.key,
                    bitmap,
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
        if let Some((key, w, h)) = &q.busy {
            jobs.retain(|j| (&j.key, j.w, j.h) != (key, *w, *h));
        }
        q.jobs = jobs;
        cv.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        cv.notify_one();
    }
}

#[cfg(feature = "images")]
fn decode_job(job: &Job) -> Option<Bitmap> {
    match &job.source {
        Source::File(path) => decode(&read_file(path)?, job.w, job.h),
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

/// The current document's images: where each comes from, the cache, and the threads that
/// decode (and, with `remote-images`, fetch) them.
pub struct Images {
    /// The document's directory, which relative paths resolve against.
    base: Option<PathBuf>,
    /// Bumped for every document: results for an older one are dropped.
    doc_gen: u64,
    srcs: Vec<Src>,
    cache: Cache,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    wake: Wake,
    worker: Option<Worker>,
    #[cfg(feature = "remote-images")]
    pub(crate) remote: crate::remote::Remote,
}

impl Default for Images {
    fn default() -> Self {
        let (tx, rx) = channel();
        Images {
            base: None,
            doc_gen: 0,
            srcs: Vec::new(),
            cache: Cache::new(CACHE_BYTES),
            tx,
            rx,
            wake: Arc::new(|| {}),
            worker: None,
            #[cfg(feature = "remote-images")]
            remote: crate::remote::Remote::default(),
        }
    }
}

impl Images {
    pub fn set_wake(&mut self, wake: Wake) {
        // The decoder thread keeps the old one: it is started again with the new one.
        self.worker = None;
        #[cfg(feature = "remote-images")]
        self.remote.set_wake(wake.clone());
        self.wake = wake;
    }

    pub fn set_base(&mut self, base: Option<PathBuf>) {
        self.base = base;
    }

    /// Start on a document (opened or reloaded) and its new layout: local images are decoded
    /// again (the files may have changed); remote ones stay cached by URL.
    pub fn attach(&mut self, doc: &Doc, layout: &mut Layout) {
        self.doc_gen += 1;
        self.cache.retain(|s| matches!(s.key, Key::Remote(_)));
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
        self.sync_remote(doc, layout);
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
    pub fn request(&mut self, boxes: &[(u32, u32, u32)], on_screen: usize) {
        // Without decoders there's nothing to ask for (and the worker isn't built in).
        if !cfg!(feature = "images") {
            return;
        }
        let mut jobs = Vec::new();
        #[cfg(feature = "remote-images")]
        let mut fetch: Vec<Arc<str>> = Vec::new();
        let mut wanted = 0usize;
        for (n, &(id, w, h)) in boxes.iter().enumerate() {
            let Some(key) = self.key(id) else {
                continue;
            };
            let (w, h) = clamp_box(w, h);
            let over = n >= on_screen && wanted >= self.cache.cap;
            wanted = wanted.saturating_add(w as usize * h as usize * 4);
            if over {
                continue;
            }
            if self.cache.has(&key, w, h) {
                self.cache.touch(&key, w, h);
                continue;
            }
            if jobs
                .iter()
                .any(|j: &Job| (&j.key, j.w, j.h) == (&key, w, h))
            {
                continue;
            }
            let source = match self.src(id).clone() {
                Src::File(path) => Source::File(path),
                #[cfg(feature = "remote-images")]
                Src::Remote(url) if self.remote.on => match self.cache.bytes_of(&url) {
                    Some(bytes) => Source::Bytes(bytes),
                    None => {
                        // Not fetched yet, or its bytes were evicted: fetch it (again).
                        if self.remote.size(&url) != Some(None) {
                            fetch.push(url);
                        }
                        continue;
                    }
                },
                _ => continue,
            };
            jobs.push(Job {
                doc_gen: self.doc_gen,
                key,
                source,
                w,
                h,
            });
        }
        if !jobs.is_empty() && self.worker.is_none() {
            self.worker = Some(Worker::spawn(self.tx.clone(), self.wake.clone()));
        }
        if let Some(w) = &self.worker {
            w.set(jobs);
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
    }

    /// Take the results that arrived: decoded images go in the cache; an image that turned out
    /// not to decode shows its alt text, and a fetched one gets its size (its block is laid out
    /// again, the reading position kept by the caller's anchor). Returns true if anything
    /// arrived (the window needs a repaint).
    pub fn poll(&mut self, doc: &Doc, layout: &mut Layout) -> bool {
        if !cfg!(feature = "images") {
            return false;
        }
        let mut any = false;
        while let Ok(done) = self.rx.try_recv() {
            any = true;
            match done {
                Done::Decoded {
                    doc_gen,
                    key,
                    bitmap,
                } => {
                    if matches!(key, Key::Local(_)) && doc_gen != self.doc_gen {
                        continue;
                    }
                    match (bitmap, key) {
                        (Some(b), key) => self.cache.insert(key, Data::Pixels(b)),
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
                            self.sync_remote(doc, layout);
                        }
                        #[cfg(not(feature = "remote-images"))]
                        (None, Key::Remote(_)) => {}
                    }
                }
                #[cfg(feature = "remote-images")]
                Done::Fetched { url, result } => {
                    if let Some(bytes) = self.remote.fetched(&url, result) {
                        self.cache.insert(Key::Remote(url), Data::Bytes(bytes));
                    }
                    self.sync_remote(doc, layout);
                }
            }
        }
        any
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
