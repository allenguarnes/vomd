//! WebP: lossy, lossless, with alpha, and the first frame of an animation. The header is read
//! from the RIFF chunks (a `VP8X` file's are walked for its EXIF orientation and the kind of its
//! image); `image-webp` decodes the whole frame, which is resampled like a JPEG's. An
//! animation's frames are found by [`frames`], and each is decoded by [`frame`] as a still made
//! of its chunks (`anim.rs` draws them).

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::AtomicBool;

use super::anim::{Dispose, Entry, Found, Head};
use super::{Bitmap, Format, Frame, Header, Loops, MAX_HEADER_SCAN, Sampling, exif, resample};
use crate::layout::Refusal;

/// What decoding a WebP holds for each pixel besides its output, by kind of image, in bytes
/// (see [`working`]).
const LOSSY_PER_PIXEL: usize = 2;
const ALPHA_PER_PIXEL: usize = 7;
const LOSSLESS_PER_PIXEL: usize = 2;
/// A lossless image without alpha is decoded as RGBA and copied to RGB.
const LOSSLESS_RGB_PER_PIXEL: usize = 4;
/// An animation's canvas, the first frame's own pixels, and what decoding it holds.
const ANIMATED_PER_PIXEL: usize = 16;
/// The copies a lossy frame's decoder makes of its data: its last partition is read to the end
/// into a `Vec` (which may have twice the room it needs), and copied; the others, and the first,
/// are read as their bytes arrive, doubling (a partition's length is the file's claim, not
/// something to allocate from), so one may be held twice over while it grows.
const DATA_COPIES: usize = 3;
/// What a lossless image's Huffman codes may take besides [`HUFFMAN_PER_PIXEL`] a pixel (see
/// [`huffman_limit`]).
const HUFFMAN_FIXED: usize = 1 << 20;
const HUFFMAN_PER_PIXEL: usize = 1;

/// What a WebP's image is, for [`working`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Lossy,
    Lossless,
    Animated,
}

/// The [`Header`] of a WebP, `r` just past `RI`: its size (the canvas, for a `VP8X` file), the
/// bytes a pixel of its output has (4 with alpha, else 3), what decoding it holds
/// ([`Header::held`]) and its EXIF orientation. `None` for anything else, or a side over
/// [`MAX_SOURCE_SIDE`](super::MAX_SOURCE_SIDE).
pub(super) fn header(r: &mut (impl Read + Seek)) -> Option<Header> {
    let mut riff = [0u8; 18];
    r.read_exact(&mut riff).ok()?;
    if &riff[..2] != b"FF" || &riff[6..10] != b"WEBP" {
        return None;
    }
    let le32 = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let le24 = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], 0]);
    let len = le32(&riff, 14);
    let (w, h, alpha, kind, data, orientation) = match &riff[10..14] {
        b"VP8 " => {
            // A key frame's tag, its start code, and 14 bits of width and of height.
            let mut b = [0u8; 10];
            r.read_exact(&mut b).ok()?;
            if b[0] & 1 != 0 || b[3..6] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let side = |i: usize| u32::from(u16::from_le_bytes([b[i], b[i + 1]]) & 0x3FFF);
            (side(6), side(8), false, Kind::Lossy, len, 1)
        }
        b"VP8L" => {
            // The signature, then 14 bits of width − 1 and of height − 1, alpha, the version.
            let mut b = [0u8; 5];
            r.read_exact(&mut b).ok()?;
            let bits = le32(&b, 1);
            if b[0] != 0x2F || bits >> 29 != 0 {
                return None;
            }
            let (w, h) = ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1);
            (w, h, bits >> 28 & 1 != 0, Kind::Lossless, len, 1)
        }
        b"VP8X" => {
            // Flags (alpha 0x10, EXIF 0x08, animation 0x02), 3 reserved bytes, 24 bits of the
            // canvas's width − 1 and of its height − 1.
            let mut b = [0u8; 10];
            r.read_exact(&mut b).ok()?;
            let (w, h) = (le24(&b, 4) + 1, le24(&b, 7) + 1);
            let flags = b[0];
            let walk = walk(r, len, flags & 0x08 != 0)?;
            // An animation is charged for its first frame's data: it must have been found
            // (another chunk first, an image chunk instead of a frame, or more chunks than are
            // looked through, is not one).
            let kind = match walk.kind? {
                Kind::Animated if flags & 0x02 == 0 => return None,
                kind if flags & 0x02 != 0 && kind != Kind::Animated => return None,
                kind => kind,
            };
            (w, h, flags & 0x10 != 0, kind, walk.data, walk.orientation)
        }
        _ => return None,
    };
    if !(1..=super::MAX_SOURCE_SIDE).contains(&w) || !(1..=super::MAX_SOURCE_SIDE).contains(&h) {
        return None;
    }
    Some(Header {
        format: Format::Webp,
        w,
        h,
        interlaced: false,
        row: 0,
        out_channels: if alpha { 4 } else { 3 },
        coefficients: 0,
        mcu_rows: 0,
        held: working(kind, alpha, w, h, data as usize),
        orientation,
        svg: super::SvgCost::default(),
    })
}

/// What [`walk`] finds in the chunks of a `VP8X` file.
struct Walk {
    /// What its (still) image is, if it has one.
    kind: Option<Kind>,
    /// The bytes of the image's data chunk (or the first frame's), no more than the file has.
    data: u32,
    orientation: u8,
}

/// The chunks after a `VP8X` header, `r` just past its data, `first` the length of the first
/// chunk (the header's: 10), walked by their headers: the first image chunk or frame, and the
/// `EXIF` chunk's orientation if the header says there is one (before the image, or usually
/// after it). The chunks that aren't the image are looked through within [`MAX_HEADER_SCAN`]
/// bytes; the image's own are not, they are skipped.
fn walk(r: &mut (impl Read + Seek), first: u32, mut exif: bool) -> Option<Walk> {
    let mut found = Walk {
        kind: None,
        data: 0,
        orientation: 1,
    };
    // Past the header's data, which is 10 bytes long if it's read as it should be.
    let mut skip = i64::from(first) + i64::from(first & 1) - 10;
    let mut scanned = 0u64;
    loop {
        let mut head = [0u8; 8];
        if r.seek_relative(skip).is_err() || r.read_exact(&mut head).is_err() {
            break;
        }
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        let kind = match &head[..4] {
            b"VP8 " => Some(Kind::Lossy),
            b"VP8L" => Some(Kind::Lossless),
            b"ANMF" => Some(Kind::Animated),
            _ => None,
        };
        scanned += 8;
        if kind.is_none() || found.kind.is_some() {
            scanned += u64::from(len);
        }
        if scanned > MAX_HEADER_SCAN {
            break;
        }
        skip = i64::from(len) + i64::from(len & 1);
        if kind.is_some() && found.kind.is_none() {
            found.kind = kind;
            // What the file has of it is all a decode can read.
            let at = r.stream_position().ok()?;
            let end = r.seek(SeekFrom::End(0)).ok()?;
            r.seek(SeekFrom::Start(at)).ok()?;
            found.data = u32::try_from(end - at).map_or(len, |left| len.min(left));
            if kind == Some(Kind::Animated) {
                skip -= first_frame(r)?;
            }
        }
        if &head[..4] == b"EXIF" && exif {
            let mut tiff = [0u8; exif::HEAD];
            let tiff = &mut tiff[..(len as usize).min(exif::HEAD)];
            if r.read_exact(tiff).is_err() {
                break;
            }
            skip -= tiff.len() as i64;
            found.orientation = exif::orientation(tiff);
            exif = false;
        }
        if found.kind.is_some() && !exif {
            break;
        }
    }
    Some(found)
}

/// Checks the first frame of an animation, `r` at the start of its `ANMF` chunk's data:
/// `None` if its lossy image (after an `ALPH` chunk) isn't the size of the frame, which
/// `image-webp` 0.2 doesn't check there (it panics; the patched one refuses it before decoding,
/// as it does a frame without alpha, which this lets by). The bytes read, if it is.
fn first_frame(r: &mut (impl Read + Seek)) -> Option<i64> {
    // The frame's place, width − 1 and height − 1 (24 bits each), duration and flags; then the
    // header of its first chunk.
    let mut b = [0u8; 24];
    r.read_exact(&mut b).ok()?;
    if &b[16..20] != b"ALPH" {
        return Some(24);
    }
    let le24 = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], 0]);
    let (w, h) = (le24(6) + 1, le24(9) + 1);
    let alph = u32::from_le_bytes([b[20], b[21], b[22], b[23]]);
    let alph = i64::from(alph) + i64::from(alph & 1);
    r.seek_relative(alph).ok()?;
    // The `VP8 ` chunk's header, the key frame's tag and start code, its width and height.
    let mut v = [0u8; 18];
    r.read_exact(&mut v).ok()?;
    let side = |i: usize| u32::from(u16::from_le_bytes([v[i], v[i + 1]]) & 0x3FFF);
    (&v[..4] == b"VP8 " && (side(14), side(16)) == (w, h)).then_some(24 + alph + 18)
}

/// What decoding a WebP of `w` × `h` holds besides its output and
/// [`WEBP_FIXED`](super::WEBP_FIXED), from the code of `image-webp` 0.2: a lossy frame's Y, U
/// and V planes, padded to 16 × 16 macroblocks, and copies of its compressed data (`data`); an
/// `ALPH` plane, which may be a lossless image of 4 bytes a pixel and its green channel copied
/// out; a lossless image's transforms and entropy image, and its 4 bytes a pixel when it has no
/// alpha (it is decoded as RGBA and copied to RGB); an animation's canvas and first frame beside
/// what decoding the frame holds; and, for each but a lossy image without alpha, the Huffman codes
/// of a lossless image ([`huffman_limit`]). Checked by counting in `tests/image_memory.rs`, for
/// files `libwebp` makes too.
fn working(kind: Kind, alpha: bool, w: u32, h: u32, data: usize) -> usize {
    let pixels = w as usize * h as usize;
    let padded = (w as usize).div_ceil(16) * 16 * ((h as usize).div_ceil(16) * 16);
    let planes = LOSSY_PER_PIXEL.saturating_mul(padded);
    let data = DATA_COPIES.saturating_mul(data);
    let huffman = huffman_limit(w, h);
    match kind {
        Kind::Lossy if alpha => planes
            .saturating_add(ALPHA_PER_PIXEL.saturating_mul(pixels))
            .saturating_add(data)
            .saturating_add(huffman),
        Kind::Lossy => planes.saturating_add(data),
        // Read as it is decoded: its data isn't copied.
        Kind::Lossless if alpha => LOSSLESS_PER_PIXEL
            .saturating_mul(pixels)
            .saturating_add(huffman),
        Kind::Lossless => (LOSSLESS_PER_PIXEL + LOSSLESS_RGB_PER_PIXEL)
            .saturating_mul(pixels)
            .saturating_add(huffman),
        Kind::Animated => ANIMATED_PER_PIXEL
            .saturating_mul(pixels)
            .saturating_add(planes)
            .saturating_add(data)
            .saturating_add(huffman),
    }
}

/// The bytes the Huffman codes of a lossless image of `w` × `h` (a `VP8L` image, an `ALPH`
/// plane, a frame of an animation that size or smaller) may take: `vendor/image-webp`'s patch
/// fails the decode past it (one code more, about 80 KB at most, may be built before that is found:
/// that is in [`WEBP_FIXED`](super::WEBP_FIXED)). The format lets a file of 1 MB name 65,536 groups of
/// five codes, hundreds of MB of tables; what `libwebp` makes needs up to 1.5 bytes a pixel of a
/// small image and 0.3 of a large one (its slowest, largest setting), and far less mostly. A file
/// over it is over VOMD's limits, not broken.
pub(super) fn huffman_limit(w: u32, h: u32) -> usize {
    (w as usize * h as usize)
        .saturating_mul(HUFFMAN_PER_PIXEL)
        .saturating_add(HUFFMAN_FIXED)
}

/// The WebP `bytes` (`header` its [`Header`]), resampled to `w` × `h` (`None`: cancelled); `Err`
/// as for `decode_why`.
pub(super) fn decode(
    bytes: &[u8],
    header: &Header,
    w: u32,
    h: u32,
    sampling: Sampling,
    limits: &super::Limits,
    cancel: &AtomicBool,
) -> Result<Option<Bitmap>, Option<Refusal>> {
    // (A lossy frame of another size than the canvas or the frame says is refused by the decoder
    // before it allocates anything: `VP8` data's own size isn't what was costed.)
    let mut d = image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(|_| None)?;
    let channels = if d.has_alpha() { 4 } else { 3 };
    // It must make what the header was costed as.
    if d.dimensions() != (header.w, header.h) || channels != header.out_channels {
        return Err(None);
    }
    let cost = super::decode_cost(header, w, h);
    d.set_memory_limit(cost);
    // Over it the decode fails: a refusal for VOMD's limits, not for broken data. A forced load
    // has its memory instead, less what the rest of the decode holds beside the codes.
    let huffman = if limits.forced {
        let rest = cost.saturating_sub(huffman_limit(header.w, header.h));
        limits.memory.saturating_sub(rest)
    } else {
        huffman_limit(header.w, header.h)
    };
    d.set_huffman_limit(huffman);
    let mut data = vec![0; d.output_buffer_size().ok_or(None)?];
    match d.read_image(&mut data) {
        Ok(()) => {}
        Err(image_webp::DecodingError::MemoryLimitExceeded) => {
            return Err(Some(Refusal::Memory(0)));
        }
        Err(_) => return Err(None),
    }
    let frame = Frame {
        data: &data,
        width: header.w,
        height: header.h,
        stride: header.w as usize * channels,
        channels,
    };
    Ok(resample(frame, w, h, sampling, cancel))
}

/// The frames of the animated WebP `bytes`, each given to `each` in order until it says stop:
/// every `ANMF` chunk, with the loop count of the `ANIM` chunk before it; to the end of the file
/// or the RIFF, or the first frame that is cut short, reaches past the canvas (`libwebp` refuses
/// those), or holds no image. `None` if it isn't a `VP8X` file with the animation flag, or a side
/// of its canvas is over [`MAX_SOURCE_SIDE`](super::MAX_SOURCE_SIDE). Reads chunk headers, the
/// frames' and their first chunk's, only.
pub(super) fn frames(bytes: &[u8], mut each: impl FnMut(Found) -> bool) -> Option<Head> {
    let le24 = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], 0]);
    let le32 = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let head = bytes.get(..30)?;
    if &head[..4] != b"RIFF" || &head[8..16] != b"WEBPVP8X" || head[20] & 0x02 == 0 {
        return None;
    }
    let (w, h) = (le24(head, 24) + 1, le24(head, 27) + 1);
    if w > super::MAX_SOURCE_SIDE || h > super::MAX_SOURCE_SIDE {
        return None;
    }
    let end = bytes.len().min((le32(head, 4) as usize).saturating_add(8));
    let first = le32(head, 16) as usize;
    let mut at = 20usize.saturating_add(first).saturating_add(first & 1);
    let mut loops = Loops::Plays(1);
    while let Some(chunk) = bytes
        .get(at..at.saturating_add(8))
        .filter(|_| at + 8 <= end)
    {
        let len = le32(chunk, 4) as usize;
        let data = at + 8;
        let Some(body) = bytes
            .get(data..data.saturating_add(len))
            .filter(|_| data + len <= end)
        else {
            break;
        };
        match &chunk[..4] {
            b"ANIM" if len >= 6 => {
                loops = match u16::from_le_bytes([body[4], body[5]]) {
                    0 => Loops::Forever,
                    n => Loops::Plays(u32::from(n)),
                };
            }
            b"ANMF" => {
                if len < 24 {
                    break;
                }
                let (x, y) = (2 * le24(body, 0), 2 * le24(body, 3));
                let (fw, fh) = (le24(body, 6) + 1, le24(body, 9) + 1);
                if x + fw > w || y + fh > h {
                    break;
                }
                let payload = &body[16..];
                let Some((out, working)) = frame_cost(payload, fw, fh) else {
                    break;
                };
                let found = Found {
                    entry: Entry {
                        at: u32::try_from(data + 16).ok()?,
                        len: u32::try_from(payload.len()).ok()?,
                        // (Inside a canvas of at most 65,535 a side.)
                        rect: [x, y, fw, fh].map(|v| v as u16),
                        delay: le24(body, 12),
                        dispose: if body[15] & 1 != 0 {
                            Dispose::Background
                        } else {
                            Dispose::Keep
                        },
                        blend: body[15] & 2 == 0,
                        transparent: None,
                    },
                    out,
                    working,
                };
                if !each(found) {
                    break;
                }
            }
            _ => {}
        }
        at = data + len + (len & 1);
    }
    Some(Head {
        size: (w, h),
        loops,
        table: 0,
    })
}

/// The RIFF and `VP8X` headers of the still a frame is decoded as.
const STILL_HEAD: usize = 30;

/// What decoding a frame of `w` × `h` whose data is `data` holds: its pixels (RGBA, or RGB for
/// a lossy frame without an `ALPH` chunk), and what the decoder holds beside them
/// ([`working`] and [`WEBP_FIXED`](super::WEBP_FIXED)) and the still it reads; `None` if the data doesn't start with
/// an image chunk. A lossless frame is read with alpha, which a frame always has.
fn frame_cost(data: &[u8], w: u32, h: u32) -> Option<(usize, usize)> {
    let (kind, alpha) = match data.get(..4)? {
        b"ALPH" => (Kind::Lossy, true),
        b"VP8 " => (Kind::Lossy, false),
        b"VP8L" => (Kind::Lossless, true),
        _ => return None,
    };
    let out = (w as usize * h as usize).saturating_mul(if alpha { 4 } else { 3 });
    // (And the still made of it, a copy of its chunks: see `frame`.)
    let working = working(kind, alpha, w, h, data.len())
        .saturating_add(super::WEBP_FIXED)
        .saturating_add(STILL_HEAD + data.len());
    Some((out, working))
}

/// Frame `e` of the animated WebP `bytes` (see [`frames`]), decoded into `out` by `read_image`
/// as a still of its own chunks, copied after the headers of a canvas the frame's size (so the patched decoder's checks hold: a lossy image of another
/// size than the frame says is refused before it is decoded), with the memory and Huffman
/// limits of a still of its size and kind (`forced`: the Huffman codes may have that room, less
/// the rest of the decode, as a still loaded past the limits). Returns its channels: 4, or 3 for
/// a lossy frame without alpha. `None` if it doesn't decode.
pub(super) fn frame(
    bytes: &[u8],
    e: &Entry,
    out: &mut Vec<u8>,
    forced: Option<usize>,
) -> Option<usize> {
    let data = bytes.get(e.at as usize..)?.get(..e.len as usize)?;
    let (fw, fh) = (u32::from(e.rect[2]), u32::from(e.rect[3]));
    let (size, working) = frame_cost(data, fw, fh)?;
    let alpha = &data[..4] != b"VP8 ";
    let mut head = [0u8; STILL_HEAD];
    head[..4].copy_from_slice(b"RIFF");
    head[4..8].copy_from_slice(&u32::try_from(22 + data.len()).ok()?.to_le_bytes());
    head[8..16].copy_from_slice(b"WEBPVP8X");
    head[16] = 10;
    head[20] = if alpha { 0x10 } else { 0 };
    head[24..27].copy_from_slice(&(fw - 1).to_le_bytes()[..3]);
    head[27..30].copy_from_slice(&(fh - 1).to_le_bytes()[..3]);
    // A copy, read as the stills are (one reader's type: the decoder is built in once).
    let mut still = Vec::with_capacity(head.len() + data.len());
    still.extend_from_slice(&head);
    still.extend_from_slice(data);
    let mut d = image_webp::WebPDecoder::new(Cursor::new(&still[..])).ok()?;
    let channels = if alpha { 4 } else { 3 };
    if d.dimensions() != (fw, fh) || d.has_alpha() != alpha || d.output_buffer_size() != Some(size)
    {
        return None;
    }
    d.set_memory_limit(working);
    // An animation played past the limits has the room it was played in instead, less what the
    // rest of the frame's decode holds beside the codes (as `decode` gives a forced still).
    let huffman = huffman_limit(fw, fh);
    d.set_huffman_limit(forced.map_or(huffman, |room| {
        room.saturating_sub(working.saturating_sub(huffman))
    }));
    out.clear();
    out.resize(size, 0);
    d.read_image(out).ok()?;
    Some(channels)
}

#[cfg(test)]
mod tests;
