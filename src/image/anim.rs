//! Animated GIF and WebP: the index of an animation's frames, made once from its bytes, and the
//! canvas they are drawn on in turn, at the source size, premultiplied. The drawing is VOMD's own
//! for both formats: a GIF's as Chrome and Firefox draw it, a WebP's by its container's rules
//! (`image-webp`'s compositor ignores dispose-to-background with VOMD's transparent background).
//! Each frame is decoded with the decoders the first frame is: `gif.rs` gives one frame's blocks
//! to the `gif` crate, `webp.rs` gives one frame's chunks to `read_image` as a still.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{
    AnimInfo, AnimState, Bitmap, Bytes, Done, Format, Key, Loops, Source, Spans, Wake, cancelled,
    clamp_box, resample_cost, vec_bytes,
};

/// The frames an animation plays at most: those past it are left out of the index (the
/// animation ends there), which holds 192 KB at most.
pub(super) const MAX_FRAMES: usize = 8192;

/// What an animation's frames may take, all at the box's size, to be made once and kept (see
/// [`keeps`]).
pub(super) const KEEP_MAX: usize = 2 << 20;

/// What one animation may hold in the cache, three eighths of it: one over it plays only past
/// the limits, when the reader asks. Room for an 800 × 600 GIF at 2× in the line's width (a box
/// of 1440 × 1080: one bitmap of it, [`Engine::late`], and the canvas take 8.1 MB) and a few MB
/// of file.
pub(super) const ANIM_ONE_MAX: usize = 12 << 20;

/// What all animations may hold in the cache together, half of it: the stills keep the other.
pub(super) const ANIM_ALL_MAX: usize = 16 << 20;

/// What an animation holds besides its buffers: the boxes of [`Playing`] and [`Engine`] and
/// the allocator's rounding of their buffers, at most.
pub(super) const PLAYING_FIXED: usize = 1024;

/// What indexing an animation holds besides the bytes it reads: the index at its largest.
pub(super) const MEASURE_FIXED: usize = MAX_FRAMES * size_of::<Entry>() + PLAYING_FIXED;

/// How long the anim thread waits for a job before it ends.
const IDLE: Duration = Duration::from_secs(2);

/// A delay this short or shorter plays as [`SHORT_DELAY_AS`], as in Chrome and Firefox: content
/// made for browsers plays at the speed its author saw.
const SHORT_DELAY_MS: u32 = 10;
const SHORT_DELAY_AS: u32 = 100;

/// How long a frame written with a delay of `ms` is shown.
pub(super) fn delay(ms: u32) -> Duration {
    Duration::from_millis(u64::from(if ms <= SHORT_DELAY_MS {
        SHORT_DELAY_AS
    } else {
        ms
    }))
}

/// Frames due this soon after the earliest are shown with it: one repaint for them all.
pub(super) const COALESCE: Duration = Duration::from_millis(10);

/// The least time between two repaints for animations: 50 a second at most, however many play.
pub(super) const CAP: Duration = Duration::from_millis(20);

/// When the window is to show the next frames, the earliest due being `due`, the last frames
/// having been shown at `last`: no sooner than [`CAP`] after them.
pub(super) fn next_tick(due: Option<Instant>, last: Option<Instant>) -> Option<Instant> {
    let due = due?;
    Some(last.map_or(due, |t| due.max(t + CAP)))
}

/// Whether a frame due at `due` is shown by a tick at `now` (see [`COALESCE`]).
pub(super) fn shows_at(due: Instant, now: Instant) -> bool {
    due <= now + COALESCE
}

/// The size an animation of canvas `src` makes its frames at for the box `w` × `h`: the canvas's
/// own when the box is it enlarged a whole number of times each way (as at 1×, and at 2× or 3× on
/// a HiDPI screen), which the window's nearest-pixel drawing then enlarges exactly as the
/// resampler would (each box pixel is one canvas pixel whole), for a fraction of the memory; else
/// the box's ([`clamp_box`]).
pub(super) fn frame_size(src: (u32, u32), (w, h): (u32, u32)) -> (u32, u32) {
    let whole = |s: u32, b: u32| s > 0 && b >= s && b.is_multiple_of(s);
    if whole(src.0, w) && whole(src.1, h) {
        src
    } else {
        clamp_box(w, h)
    }
}

/// A rectangle of the canvas: left, top, width, height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Rect {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
}

impl Rect {
    fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// The part of it inside a canvas of `w` × `h`.
    fn clip(self, w: u32, h: u32) -> Rect {
        let (x, y) = (self.x.min(w), self.y.min(h));
        Rect {
            x,
            y,
            w: self.w.min(w - x),
            h: self.h.min(h - y),
        }
    }

    /// The smallest rectangle holding both.
    fn union(self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        let right = (self.x + self.w).max(o.x + o.w);
        let bottom = (self.y + self.h).max(o.y + o.h);
        Rect {
            x,
            y,
            w: right - x,
            h: bottom - y,
        }
    }
}

/// What is done with a frame's rectangle before the next frame is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dispose {
    /// Left as it is (a GIF's 0, "unspecified", and 1; its 4 to 7, which mean nothing, too).
    Keep,
    /// Cleared to transparent (a GIF's 2, a WebP's dispose bit): the background color is ignored,
    /// as browsers do.
    Background,
    /// A GIF's 3: put back as it was before the frame was drawn.
    Previous,
}

/// One frame of the index: 24 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    /// Where its bytes start in the file: a GIF's image descriptor, a WebP's frame data (the
    /// chunks after the `ANMF` chunk's frame header).
    pub(super) at: u32,
    /// How many: a GIF's through its last data sub-block's terminator.
    pub(super) len: u32,
    /// Left, top, width and height on the canvas. A GIF's may reach past it (it is clipped), a
    /// WebP's never does (the walk ends at one that would).
    pub(super) rect: [u16; 4],
    /// The delay after it as written, in milliseconds (see [`delay`] for how long it plays).
    pub(super) delay: u32,
    pub(super) dispose: Dispose,
    /// WebP: drawn over the canvas by its alpha; else its pixels replace the rectangle's. A GIF
    /// frame's transparent pixels leave the canvas as it is, and the rest replace it.
    pub(super) blend: bool,
    /// GIF: the transparent color index of its graphic control.
    pub(super) transparent: Option<u8>,
}

impl Entry {
    pub(super) fn size(&self) -> (usize, usize) {
        (usize::from(self.rect[2]), usize::from(self.rect[3]))
    }

    fn rect(&self) -> Rect {
        let [x, y, w, h] = self.rect.map(u32::from);
        Rect { x, y, w, h }
    }
}

/// What a walk of an animation's blocks gives each frame it finds: its entry, the bytes of its
/// decoded pixels (palette indices, or RGBA or RGB) and what its decoder holds beside them.
pub(super) struct Found {
    pub(super) entry: Entry,
    pub(super) out: usize,
    pub(super) working: usize,
}

/// What a walk finds besides the frames.
pub(super) struct Head {
    /// The canvas: a GIF's screen as the first frame is shown on it, a WebP's `VP8X` canvas.
    pub(super) size: (u32, u32),
    pub(super) loops: Loops,
    /// GIF: the bytes of the global color table, after the screen descriptor.
    pub(super) table: u32,
}

/// The frames of the animated GIF or WebP `bytes`, each given to `each` in order until it says
/// stop (see `gif::frames`, `webp::frames`). `None` if it is neither, or has no first frame.
fn walk(bytes: &[u8], each: impl FnMut(Found) -> bool) -> Option<(Format, Head)> {
    match bytes.get(..4)? {
        b"GIF8" => Some((Format::Gif, super::gif::frames(bytes, each)?)),
        b"RIFF" => Some((Format::Webp, super::webp::frames(bytes, each)?)),
        _ => None,
    }
}

/// Whether `bytes` is an animation, which is two frames or more (one is a still, and is shown
/// as one), and its canvas and loop count as far as its second frame says: read by the decode
/// worker after it decoded the first frame. It walks block and chunk headers only, allocating
/// nothing. A WebP turned by its EXIF orientation is shown as its still, turned: its frames
/// would not be.
pub(super) fn detect(bytes: &[u8]) -> Option<AnimInfo> {
    let mut frames = 0;
    let (format, head) = walk(bytes, |_| {
        frames += 1;
        frames < 2
    })?;
    if frames < 2
        || format == Format::Webp && super::info_of(bytes).is_none_or(|h| h.orientation != 1)
    {
        return None;
    }
    Some(AnimInfo {
        size: head.size,
        loops: head.loops,
    })
}

/// The frames of an animation, made once from its bytes when it first plays.
pub(super) struct Index {
    pub(super) format: Format,
    /// The canvas's size.
    pub(super) size: (u32, u32),
    pub(super) loops: Loops,
    /// At most [`MAX_FRAMES`], allocated at their number. A frame that doesn't decode ends the
    /// animation: the index is cut there ([`Canvas::step`]).
    pub(super) frames: Vec<Entry>,
    /// GIF: the bytes of the global color table.
    table: u32,
    /// The most bytes a frame's decoded pixels take ([`Found::out`]): the canvas's scratch is
    /// made that big, and never grows.
    largest: usize,
    /// The most a frame's decoder holds beside them ([`Found::working`]).
    working: usize,
    /// GIF: the most pixels of the canvas a frame disposed to the previous covers, which are
    /// saved before it is drawn.
    previous: usize,
}

impl Index {
    /// The index of the animation `bytes`: `None` if it isn't one of two frames or more. Its
    /// headers are walked twice, to count the frames and then to keep them, so the index is
    /// allocated once, at its size.
    pub(super) fn new(bytes: &[u8]) -> Option<Index> {
        let mut n = 0;
        walk(bytes, |_| {
            n += 1;
            n < MAX_FRAMES
        })?;
        if n < 2 {
            return None;
        }
        let mut frames = Vec::with_capacity(n);
        let (mut largest, mut working, mut previous) = (0, 0, 0);
        let (format, head) = walk(bytes, |f| {
            largest = largest.max(f.out);
            working = working.max(f.working);
            frames.push(f.entry);
            frames.len() < n
        })?;
        let size = head.size;
        for e in &frames {
            if e.dispose == Dispose::Previous {
                let r = e.rect().clip(size.0, size.1);
                previous = previous.max(r.w as usize * r.h as usize);
            }
        }
        (frames.len() == n).then_some(Index {
            format,
            size,
            loops: head.loops,
            frames,
            table: head.table,
            largest,
            working,
            previous,
        })
    }

    /// What the index holds.
    pub(super) fn cost(&self) -> usize {
        vec_bytes(&self.frames)
    }

    /// What playing it holds besides its bytes and what is made of the canvas: the index, the
    /// canvas, the pixels saved for a frame disposed to the previous, and a frame's decode (its
    /// pixels and its decoder) at the most any frame holds.
    pub(super) fn held(&self) -> usize {
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        self.cost()
            .saturating_add(w.saturating_mul(h).saturating_mul(4))
            .saturating_add(self.previous.saturating_mul(4))
            .saturating_add(self.largest)
            .saturating_add(self.working)
    }

    /// What playing it holds with its frames made one at a time and resampled to `bw` × `bh`:
    /// [`Index::held`], the encoded `bytes` it holds itself (0 for a fetched body, whose slot is
    /// the cache's already), the resampler's tables and rows, and `bitmaps` bitmaps of the box
    /// (two: the one shown and the one being made; one: the one shown, made again from the canvas
    /// as each frame is shown, [`Engine::late`]).
    pub(super) fn streamed(&self, bytes: usize, (bw, bh): (u32, u32), bitmaps: usize) -> usize {
        let (w, h) = self.size;
        self.held()
            .saturating_add(bytes)
            .saturating_add(resample_cost(w, h, bw, bh))
            .saturating_add((bw as usize * bh as usize).saturating_mul(4 * bitmaps))
    }

    /// What keeping every frame resampled to `bw` × `bh` holds once they are made: their pixels
    /// and the index.
    pub(super) fn kept(&self, (bw, bh): (u32, u32)) -> usize {
        (self.frames.len() * bw as usize * bh as usize)
            .saturating_mul(4)
            .saturating_add(self.cost())
    }

    /// Whether its frames are made once and kept at the box `bw` × `bh`, not made anew each time
    /// they are shown: if they take no more than [`KEEP_MAX`]. (Even when that is more than
    /// streaming holds: then each loop after the first costs nothing but the paint.)
    pub(super) fn keeps(&self, size: (u32, u32)) -> bool {
        self.kept(size) <= KEEP_MAX
    }
}

/// What drawing the next frame did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Advance {
    /// Frame `frame` is on the canvas; what of it changed, and how long it is shown.
    Drawn {
        frame: usize,
        dirty: Rect,
        delay: Duration,
    },
    /// The animation has played its loops (or no frame of it decodes): the last frame drawn
    /// stays.
    Over,
    /// Told to stop: the canvas is as it was.
    Cancelled,
}

/// An animation's canvas, premultiplied `0xAARRGGBB` at the source size, and where it is in the
/// animation.
pub(super) struct Canvas {
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) px: Vec<u32>,
    /// What a frame disposed to the previous covered before it was drawn, packed.
    saved: Vec<u32>,
    /// A frame's decoded pixels, allocated once at [`Index::largest`].
    scratch: Vec<u8>,
    /// The frame drawn next.
    next: usize,
    /// The frame drawn last (clipped) and what is done with it before the next.
    last: Option<(Rect, Dispose)>,
    /// The plays done.
    plays: u32,
    /// Nothing of it has been shown since it was cleared: the next frame dirties all of it.
    fresh: bool,
    /// Played past the limits in this room: a WebP frame's Huffman codes may take it (see
    /// `webp::frame`).
    pub(super) forced: Option<usize>,
}

impl Canvas {
    /// A clear canvas for `index`, before its first frame. Allocates [`Index::held`] less the
    /// index and the decoder's working memory.
    pub(super) fn new(index: &Index) -> Canvas {
        let (w, h) = index.size;
        Canvas {
            w,
            h,
            px: vec![0; w as usize * h as usize],
            saved: Vec::with_capacity(index.previous),
            scratch: Vec::with_capacity(index.largest),
            next: 0,
            last: None,
            plays: 0,
            fresh: true,
            forced: None,
        }
    }

    /// Draws the next frame of `index`, decoded from `bytes`; after the last, the first again
    /// on a clear canvas while the loop count lets it. A frame that doesn't decode ends the
    /// animation at the one before it: the index is cut there, and what is left loops. Its
    /// pixels are decoded before anything is drawn, so a broken or cancelled frame leaves the
    /// canvas as it was.
    pub(super) fn step(&mut self, index: &mut Index, bytes: &[u8], cancel: &AtomicBool) -> Advance {
        loop {
            if self.next >= index.frames.len() {
                self.plays = self.plays.saturating_add(1);
                let again = match index.loops {
                    Loops::Forever => true,
                    Loops::Plays(n) => self.plays < n,
                };
                if !again || index.frames.is_empty() {
                    return Advance::Over;
                }
                self.next = 0;
                self.last = None;
                self.px.fill(0);
                self.fresh = true;
            }
            let e = index.frames[self.next];
            let mut colors = [0u32; 256];
            let channels = match index.format {
                Format::Gif => super::gif::frame(
                    bytes,
                    index.table,
                    &e,
                    &mut colors,
                    &mut self.scratch,
                    cancel,
                )
                .map(|()| 1),
                _ => super::webp::frame(bytes, &e, &mut self.scratch, self.forced),
            };
            match channels {
                Some(channels) => {
                    let dirty = self.draw(&e, channels, &colors);
                    let frame = self.next;
                    self.next += 1;
                    return Advance::Drawn {
                        frame,
                        dirty,
                        delay: delay(e.delay),
                    };
                }
                None if cancelled(cancel) => return Advance::Cancelled,
                None => index.frames.truncate(self.next),
            }
        }
    }

    /// Disposes of the last frame and draws `e`, its pixels in the scratch: palette indices
    /// (`channels` 1) of `colors`, or RGBA or RGB. Returns what changed.
    fn draw(&mut self, e: &Entry, channels: usize, colors: &[u32; 256]) -> Rect {
        let mut dirty = Rect::default();
        if let Some((r, dispose)) = self.last.take() {
            match dispose {
                Dispose::Keep => {}
                Dispose::Background => {
                    for row in self.rows(r) {
                        self.px[row].fill(0);
                    }
                    dirty = r;
                }
                Dispose::Previous => {
                    let w = r.w as usize;
                    for (i, row) in self.rows(r).enumerate() {
                        self.px[row].copy_from_slice(&self.saved[i * w..(i + 1) * w]);
                    }
                    dirty = r;
                }
            }
        }
        let r = e.rect().clip(self.w, self.h);
        if e.dispose == Dispose::Previous {
            self.saved.clear();
            for row in self.rows(r) {
                self.saved.extend_from_slice(&self.px[row]);
            }
        }
        let (cw, (fw, _)) = (self.w as usize, e.size());
        let (left, top) = (r.x as usize, r.y as usize);
        for (fy, src) in self.scratch.chunks_exact(fw * channels).enumerate() {
            let y = top + fy;
            if fy >= r.h as usize {
                break;
            }
            let line = &mut self.px[y * cw + left..y * cw + left + r.w as usize];
            if channels == 1 {
                for (d, &i) in line.iter_mut().zip(src) {
                    let c = colors[usize::from(i)];
                    if c != 0 {
                        *d = c;
                    }
                }
                continue;
            }
            for (d, p) in line.iter_mut().zip(src.chunks_exact(channels)) {
                let a = if channels == 4 { p[3] } else { 255 };
                let s = premultiplied(p[0], p[1], p[2], a);
                *d = if !e.blend || a == 255 {
                    s
                } else if a == 0 {
                    *d
                } else {
                    over(s, *d, a)
                };
            }
        }
        self.last = Some((r, e.dispose));
        if std::mem::take(&mut self.fresh) {
            return Rect {
                x: 0,
                y: 0,
                w: self.w,
                h: self.h,
            };
        }
        dirty.union(r)
    }

    /// The ranges of `px` that are the rows of `r` (inside the canvas).
    fn rows(&self, r: Rect) -> impl Iterator<Item = std::ops::Range<usize>> + use<> {
        let (cw, x, w) = (self.w as usize, r.x as usize, r.w as usize);
        (r.y as usize..(r.y + r.h) as usize).map(move |y| y * cw + x..y * cw + x + w)
    }
}

/// `r`, `g`, `b` at alpha `a` as premultiplied `0xAARRGGBB`, each color rounded as the resampler
/// rounds it (the first frame drawn matches the still).
fn premultiplied(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let a32 = u32::from(a);
    let m = |c: u8| (u32::from(c) * a32 + 127) / 255;
    a32 << 24 | m(r) << 16 | m(g) << 8 | m(b)
}

/// `s` (of alpha `a`) over `d`, premultiplied: the WebP container's blend, which it writes
/// unpremultiplied.
fn over(s: u32, d: u32, a: u8) -> u32 {
    let keep = 255 - u32::from(a);
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        let (sc, dc) = ((s >> shift) & 0xFF, (d >> shift) & 0xFF);
        out |= (sc + (dc * keep + 127) / 255) << shift;
    }
    out
}

/// Resamples the canvas to a box (by area, premultiplied, as stills are: [`super::resample_rows`]),
/// all of it or only what a dirty rectangle of the canvas feeds. Holds [`resample_cost`].
struct Scaler {
    src: (u32, u32),
    dst: (u32, u32),
    xs: Spans,
    ys: Spans,
    /// The last source row reduced to the target width, four floats a pixel.
    reduced: Vec<f32>,
    acc: Vec<f32>,
    /// Every target pixel is one source pixel whole (the same size, or 2×, as on a HiDPI
    /// screen): the sums would give back its value, so it is copied.
    copy: bool,
    /// Enlarging each way by a fraction (as at 1.5×, or a box the line's width cuts): each
    /// target pixel blends the one or two source pixels under it each way, with whole-number
    /// weights ([`blend`]), in place of the sums; the weights of the target columns, and the two
    /// source rows the target row is between, blended along the width ([`Blend`]).
    blend: Option<Blend>,
}

/// What a [`Scaler`] that enlarges blends with: for each target column its first source column
/// and that one's weight in 65536ths (the next has the rest), and two source rows blended to the
/// target width, which they are (`usize::MAX`: none yet). Holds less than the float rows it is
/// in place of.
struct Blend {
    cols: Vec<(u32, u32)>,
    rows: [Vec<u32>; 2],
    at: [usize; 2],
}

/// A weight of 1 in [`Blend`]'s 65536ths.
const ONE: u32 = 1 << 16;

/// A target pixel's weights (one source pixel or two, adding up to 1) as the first one's in
/// 65536ths.
fn first_weight(weights: &[f32]) -> u32 {
    match weights {
        [_] => ONE,
        [w, ..] => ((w * ONE as f32 + 0.5) as u32).min(ONE),
        [] => ONE,
    }
}

/// The premultiplied `a` and `b` blended, `a` weighing `wa` 65536ths, each channel rounded. Two
/// channels at once, 32 bits apart (the size-optimized build makes a loop of four a loop).
#[inline(always)]
fn blend(a: u32, b: u32, wa: u32) -> u32 {
    if wa == ONE {
        return a;
    }
    const LANES: u64 = 0xFF | 0xFF << 32;
    const HALF: u64 = (ONE as u64 / 2) * (1 | 1 << 32);
    // (Blue and red; green and alpha.)
    let split = |p: u32| {
        let p = u64::from(p);
        (
            p & 0xFF | (p & 0xFF_0000) << 16,
            p >> 8 & 0xFF | (p & 0xFF00_0000) << 8,
        )
    };
    let ((a0, a1), (b0, b1)) = (split(a), split(b));
    let (wa, wb) = (u64::from(wa), u64::from(ONE - wa));
    let lo = (a0 * wa + b0 * wb + HALF) >> 16 & LANES;
    let hi = (a1 * wa + b1 * wb + HALF) >> 16 & LANES;
    (lo | lo >> 16 | (hi | hi >> 16) << 8) as u32
}

impl Scaler {
    fn new(src: (u32, u32), dst: (u32, u32)) -> Scaler {
        Scaler::with(src, dst, true)
    }

    /// [`Scaler::new`], copying and blending where it can if `fast` (else always by the sums).
    fn with(src: (u32, u32), dst: (u32, u32), fast: bool) -> Scaler {
        let (xs, ys) = (Spans::new(src.0, dst.0), Spans::new(src.1, dst.1));
        let whole = |s: &Spans| (0..s.len()).all(|t| s.get(t).1 == [1.0]);
        let copy = fast && whole(&xs) && whole(&ys);
        let width = dst.0 as usize;
        let blend = (fast && !copy && dst.0 >= src.0 && dst.1 >= src.1).then(|| Blend {
            cols: (0..xs.len())
                .map(|t| {
                    let (x, w) = xs.get(t);
                    (x as u32, first_weight(w))
                })
                .collect(),
            rows: [vec![0; width], vec![0; width]],
            at: [usize::MAX; 2],
        });
        let floats = if copy || blend.is_some() {
            0
        } else {
            width * 4
        };
        Scaler {
            src,
            dst,
            copy,
            blend,
            xs,
            ys,
            reduced: vec![0.0; floats],
            acc: vec![0.0; floats],
        }
    }

    /// Makes again the pixels of `out` (the box) that the canvas `px`'s rectangle `dirty` feeds;
    /// each exactly as a resample of the whole canvas makes it (the same sums in the same order).
    fn run(&mut self, px: &[u32], dirty: Rect, out: &mut [u32]) {
        let cols = covered(&self.xs, dirty.x, dirty.x + dirty.w);
        let rows = covered(&self.ys, dirty.y, dirty.y + dirty.h);
        if cols.is_empty() || rows.is_empty() {
            return;
        }
        let (sw, dw) = (self.src.0 as usize, self.dst.0 as usize);
        if self.copy {
            for t in rows {
                let row = &px[self.ys.get(t).0 * sw..][..sw];
                let line = &mut out[t * dw + cols.start..t * dw + cols.end];
                if sw == dw {
                    line.copy_from_slice(&row[cols.clone()]);
                    continue;
                }
                for (o, c) in line.iter_mut().zip(cols.clone()) {
                    *o = row[self.xs.get(c).0];
                }
            }
            return;
        }
        if let Some(b) = &mut self.blend {
            let (sh, last) = (self.src.1 as usize, sw - 1);
            let widen = |y: usize, row: &mut [u32]| {
                let line = &px[y * sw..][..sw];
                for (o, &(x, w)) in row[cols.clone()].iter_mut().zip(&b.cols[cols.clone()]) {
                    let x = x as usize;
                    *o = blend(line[x], line[(x + 1).min(last)], w);
                }
            };
            b.at = [usize::MAX; 2];
            for t in rows {
                let (y0, wy) = self.ys.get(t);
                let (y1, w) = ((y0 + 1).min(sh - 1), first_weight(wy));
                if b.at[0] != y0 {
                    if b.at[1] == y0 {
                        b.rows.swap(0, 1);
                        b.at.swap(0, 1);
                    } else {
                        widen(y0, &mut b.rows[0]);
                        b.at[0] = y0;
                    }
                }
                if w < ONE && b.at[1] != y1 {
                    widen(y1, &mut b.rows[1]);
                    b.at[1] = y1;
                }
                let line = &mut out[t * dw + cols.start..t * dw + cols.end];
                let (r0, r1) = (&b.rows[0][cols.clone()], &b.rows[1][cols.clone()]);
                if w == ONE {
                    line.copy_from_slice(r0);
                    continue;
                }
                for ((o, &p0), &p1) in line.iter_mut().zip(r0).zip(r1) {
                    *o = blend(p0, p1, w);
                }
            }
            return;
        }
        let (c0, c1) = (cols.start * 4, cols.end * 4);
        let mut last = usize::MAX;
        for t in rows {
            let (y0, wy) = self.ys.get(t);
            self.acc[c0..c1].fill(0.0);
            for (k, &weight) in wy.iter().enumerate() {
                let y = y0 + k;
                if last != y {
                    reduce(
                        &px[y * sw..(y + 1) * sw],
                        &self.xs,
                        cols.clone(),
                        &mut self.reduced,
                    );
                    last = y;
                }
                for (a, r) in self.acc[c0..c1].iter_mut().zip(&self.reduced[c0..c1]) {
                    *a += r * weight;
                }
            }
            let line = &mut out[t * dw + cols.start..t * dw + cols.end];
            for (o, p) in line.iter_mut().zip(self.acc[c0..c1].as_chunks::<4>().0) {
                *o = pack_sums(p);
            }
        }
    }
}

/// The target pixels along one axis whose source pixels reach into `from..to`.
fn covered(spans: &Spans, from: u32, to: u32) -> std::ops::Range<usize> {
    let (from, to) = (from as usize, to as usize);
    let mut range: Option<std::ops::Range<usize>> = None;
    for t in 0..spans.len() {
        let (first, weights) = spans.get(t);
        if first < to && first + weights.len() > from {
            range.get_or_insert(t..t).end = t + 1;
        } else if first >= to {
            break;
        }
    }
    range.unwrap_or(0..0)
}

/// The canvas row `row` (premultiplied) averaged along its width into `out`'s target pixels
/// `cols`, four sums a pixel (premultiplied r, g, b and alpha, 0 to 255).
fn reduce(row: &[u32], xs: &Spans, cols: std::ops::Range<usize>, out: &mut [f32]) {
    for t in cols {
        let (x0, wx) = xs.get(t);
        let mut sum = [0.0; 4];
        for (&p, &weight) in row[x0..].iter().zip(wx) {
            sum[0] += f32::from((p >> 16) as u8) * weight;
            sum[1] += f32::from((p >> 8) as u8) * weight;
            sum[2] += f32::from(p as u8) * weight;
            sum[3] += f32::from((p >> 24) as u8) * weight;
        }
        out[t * 4..t * 4 + 4].copy_from_slice(&sum);
    }
}

/// A target pixel's sums ([`reduce`]) as premultiplied `0xAARRGGBB`: [`super::pack`] for sums
/// without its factor 255, and rounded without a call (the size-optimized build makes `round`
/// one).
fn pack_sums(px: &[f32; 4]) -> u32 {
    // (`as` saturates: a sum below 0 is 0.)
    let ch = |v: f32| ((v + 0.5) as u32).min(255);
    let a = ch(px[3]);
    a << 24 | ch(px[0]).min(a) << 16 | ch(px[1]).min(a) << 8 | ch(px[2]).min(a)
}

/// What makes an animation's frames: its bytes, its index, and once it plays its canvas and the
/// resampler of the box. Moved to the anim thread for each frame it makes.
pub(crate) struct Engine {
    bytes: Bytes,
    /// The bytes are its own (a file's, read for it), not a cached body's.
    own: bool,
    pub(super) index: Index,
    canvas: Option<Canvas>,
    scaler: Option<Scaler>,
    /// Played past the limits in this room ([`Canvas::forced`]).
    forced: Option<usize>,
    /// The frames made, and what of the canvas the last one changed: a box given back that was
    /// made the frame before it is made again only where those two changed.
    made: u64,
    last: Rect,
    /// Its frames are resampled to the box only as each is shown, on the window's thread, into
    /// the one bitmap shown ([`Engine::show`]): the canvas is the frame made ahead. For a box
    /// that enlarges the canvas by a fraction, whose two bitmaps would be over the limits.
    pub(super) late: bool,
    /// What of the canvas changed since the bitmap shown was made from it ([`Engine::late`]).
    pending: Rect,
}

/// What a step made.
pub(crate) enum Made {
    /// A frame resampled to the box, the number of frames made with it (see
    /// [`Engine::step`]), how long it is shown, and whether it starts a play of the animation
    /// again.
    Frame {
        bitmap: Bitmap,
        made: u64,
        delay: Duration,
        again: bool,
    },
    /// A frame drawn on the canvas only, resampled as it is shown ([`Engine::late`]).
    Drawn {
        delay: Duration,
    },
    Over,
    Cancelled,
}

impl Engine {
    pub(super) fn new(bytes: Bytes, own: bool, index: Index) -> Engine {
        Engine {
            bytes,
            own,
            index,
            canvas: None,
            scaler: None,
            forced: None,
            made: 0,
            last: Rect::default(),
            late: false,
            pending: Rect::default(),
        }
    }

    /// What making its frames at `size` holds: [`Index::streamed`] with its bytes if they are its
    /// own, and [`PLAYING_FIXED`].
    pub(super) fn cost(&self, size: (u32, u32)) -> usize {
        let own = if self.own { self.bytes.capacity() } else { 0 };
        let bitmaps = if self.late { 1 } else { 2 };
        self.index
            .streamed(own, size, bitmaps)
            .saturating_add(PLAYING_FIXED)
    }

    /// Its frames are made [`Engine::late`] if that is what lets it play within `max` at `size`:
    /// a box that enlarges the canvas, made as two bitmaps over `max`, as one within it.
    pub(super) fn lean(&mut self, size: (u32, u32), max: usize) {
        let (w, h) = self.index.size;
        self.late = false;
        let two = self.cost(size);
        self.late = true;
        let one = self.cost(size);
        self.late = two > max && one <= max && size.0 >= w && size.1 >= h;
    }

    /// It plays past the limits, under `ceiling`: whether it fits at `size` (if not, nothing
    /// changes). A WebP frame's Huffman codes may have what the rest leaves (see `webp::frame`).
    pub(super) fn force(&mut self, ceiling: usize, size: (u32, u32)) -> bool {
        let cost = self.cost(size);
        if cost > ceiling {
            return false;
        }
        self.forced = Some((ceiling - cost).saturating_add(self.index.working));
        if let Some(c) = &mut self.canvas {
            c.forced = self.forced;
        }
        true
    }

    /// Whether its frames are kept at `size` (see [`Index::keeps`]).
    pub(super) fn keeps(&self, size: (u32, u32)) -> bool {
        self.index.keeps(size)
    }

    /// Draws the next frame and resamples it to `size`, into `spare` if it is that size (a frame
    /// it made before, numbered as [`Made::Frame::made`] says: only what changed since is made
    /// again when it is the one before the last), else into a new bitmap.
    pub(super) fn step(
        &mut self,
        size: (u32, u32),
        spare: Option<Numbered>,
        cancel: &AtomicBool,
    ) -> Made {
        let canvas = self.canvas.get_or_insert_with(|| {
            let mut c = Canvas::new(&self.index);
            c.forced = self.forced;
            c
        });
        let (frame, dirty, delay) = match canvas.step(&mut self.index, &self.bytes, cancel) {
            Advance::Drawn {
                frame,
                dirty,
                delay,
            } => (frame, dirty, delay),
            Advance::Over => return Made::Over,
            Advance::Cancelled => return Made::Cancelled,
        };
        self.made += 1;
        if self.late {
            self.pending = self.pending.union(dirty);
            return Made::Drawn { delay };
        }
        let src = (canvas.w, canvas.h);
        if self.scaler.as_ref().is_none_or(|s| s.dst != size) {
            // (The old one goes before the new one is made.)
            self.scaler = None;
            self.scaler = Some(Scaler::new(src, size));
        }
        let whole = Rect {
            x: 0,
            y: 0,
            w: src.0,
            h: src.1,
        };
        let (mut bitmap, region) = match spare {
            Some((b, n)) if (b.width, b.height) == size => {
                let region = if n + 2 == self.made {
                    dirty.union(self.last)
                } else {
                    whole
                };
                (b, region)
            }
            _ => (
                Bitmap {
                    width: size.0,
                    height: size.1,
                    data: vec![0; size.0 as usize * size.1 as usize],
                },
                whole,
            ),
        };
        if let Some(s) = &mut self.scaler {
            s.run(&canvas.px, region, &mut bitmap.data);
        }
        self.last = dirty;
        Made::Frame {
            bitmap,
            made: self.made,
            delay,
            again: frame == 0 && self.made > 1,
        }
    }
}

impl Engine {
    /// The frame on the canvas resampled to `size` ([`Engine::late`]), into `shown` (the frame
    /// before, made from the canvas as it was) where only what changed since is made again, else
    /// into a new bitmap, the old one gone first. `None` before the first frame is drawn.
    pub(super) fn show(&mut self, shown: Option<Bitmap>, size: (u32, u32)) -> Option<Bitmap> {
        let canvas = self.canvas.as_ref()?;
        let whole = Rect {
            x: 0,
            y: 0,
            w: canvas.w,
            h: canvas.h,
        };
        let (mut bitmap, region) = match shown {
            Some(b) if (b.width, b.height) == size => (b, self.pending),
            old => {
                drop(old);
                let data = vec![0; size.0 as usize * size.1 as usize];
                let (width, height) = size;
                let b = Bitmap {
                    width,
                    height,
                    data,
                };
                (b, whole)
            }
        };
        if self.scaler.as_ref().is_none_or(|s| s.dst != size) {
            self.scaler = None;
            self.scaler = Some(Scaler::new((canvas.w, canvas.h), size));
        }
        if let Some(s) = &mut self.scaler {
            s.run(&canvas.px, region, &mut bitmap.data);
        }
        self.pending = Rect::default();
        Some(bitmap)
    }
}

/// A frame made, and its number (see [`Engine::step`]).
type Numbered = (Bitmap, u64);

/// A step to run on the anim thread: the engine, the box, and the frame to make the next in.
type Step = (Box<Engine>, (u32, u32), Option<Numbered>);

/// The frames of a small animation, made once and kept ([`Index::keeps`]).
struct Kept {
    frames: Vec<(Bitmap, Duration)>,
    /// The frame shown.
    at: usize,
    /// Its frames are still being made (its first play).
    building: bool,
    loops: Loops,
    plays: u32,
}

/// An animation playing (or paused) in a box: the frame shown, the one made next, and what
/// makes them. A step of it runs on the anim thread with its [`Engine`], while the frame shown
/// stays here to be painted.
pub(super) struct Playing {
    /// Which it is, for the anim thread's results.
    pub(super) serial: u64,
    /// The box its frames are made at.
    pub(super) size: (u32, u32),
    /// The box it was last painted in: its next frame is made at it.
    pub(super) want: (u32, u32),
    pub(super) engine: Option<Box<Engine>>,
    /// A step runs on the anim thread (its engine is there).
    pub(super) running: bool,
    /// Streamed: the frame shown, and its number (see [`Engine::step`]).
    front: Option<Numbered>,
    /// Streamed: the frame made next, ready to be shown.
    next: Option<(Bitmap, u64, Duration)>,
    /// Streamed: the frame shown before, which the next step makes its frame in.
    spare: Option<Numbered>,
    /// Streamed [`Engine::late`]: the frame made next is on the canvas, shown this long.
    drawn: Option<Duration>,
    /// How long the frame shown is shown.
    delay: Duration,
    kept: Option<Kept>,
    /// When the frame shown was first painted: the next is due its delay later.
    pub(super) shown: Option<Instant>,
    /// One more than the frame it was last painted in (0: never).
    pub(super) painted: u64,
    /// It has played its loops: the frame shown stays.
    pub(super) over: bool,
    /// What it is charged (outside the cache: what it holds, for its limit).
    pub(super) cost: usize,
    /// Its steps' flag: set, the one running stops (its document went).
    pub(super) cancel: Arc<AtomicBool>,
}

impl Playing {
    /// An animation of `engine` about to make its first frame at `size`, its frames kept if
    /// `keep`.
    pub(super) fn new(serial: u64, engine: Box<Engine>, size: (u32, u32), keep: bool) -> Playing {
        let kept = keep.then(|| Kept {
            frames: Vec::with_capacity(engine.index.frames.len()),
            at: 0,
            building: true,
            loops: engine.index.loops,
            plays: 0,
        });
        let mut p = Playing {
            serial,
            size,
            want: size,
            engine: Some(engine),
            running: false,
            front: None,
            next: None,
            spare: None,
            drawn: None,
            delay: Duration::ZERO,
            kept,
            shown: None,
            painted: 0,
            over: false,
            cost: 0,
            cancel: Arc::default(),
        };
        p.cost = p.charge();
        p
    }

    /// What it holds at most until its state changes: making frames, its engine's
    /// [`Engine::cost`] at its box (and the frame shown at another box, while it is made at a
    /// new one), and its frames kept, all of them, once they are made or while they are; done,
    /// what it shows.
    pub(super) fn charge(&self) -> usize {
        let px = |b: &Bitmap| b.data.capacity() * 4;
        let frames = |k: &Kept| k.frames.iter().map(|f| px(&f.0)).sum::<usize>();
        match (&self.engine, &self.kept) {
            (Some(e), Some(k)) if k.building => {
                let all = e.index.kept(self.size).max(frames(k));
                e.cost(self.size).saturating_add(all)
            }
            (Some(e), None) => {
                let old = self
                    .front
                    .as_ref()
                    .filter(|f| (f.0.width, f.0.height) != self.size);
                e.cost(self.size)
                    .saturating_add(old.map_or(0, |f| px(&f.0)))
            }
            (_, Some(k)) => frames(k)
                .saturating_add(vec_bytes(&k.frames))
                .saturating_add(PLAYING_FIXED),
            (None, None) => {
                let shown = self.front.as_ref().map_or(0, |f| px(&f.0));
                let next = self.next.as_ref().map_or(0, |f| px(&f.0));
                shown.saturating_add(next).saturating_add(PLAYING_FIXED)
            }
        }
    }

    /// What it will hold made at `size` (its next frame made there, [`Playing::resize`]), if
    /// that is known here: streamed, its engine home.
    pub(super) fn cost_at(&self, size: (u32, u32)) -> Option<usize> {
        if size == self.size {
            return Some(self.cost);
        }
        let e = self.engine.as_ref().filter(|_| self.kept.is_none())?;
        let old = self.front.as_ref().map_or(0, |f| f.0.data.capacity() * 4);
        Some(e.cost(size).saturating_add(old))
    }

    /// It plays past the limits under `ceiling` ([`Engine::force`]): whether it fits made at
    /// `size`, beside the frame it shows until one is made there. Its engine is home.
    pub(super) fn force(&mut self, ceiling: usize, size: (u32, u32)) -> bool {
        let old = self.front.as_ref().map_or(0, |f| f.0.data.capacity() * 4);
        let e = self.engine.as_mut().filter(|_| self.kept.is_none());
        e.is_some_and(|e| e.force(ceiling.saturating_sub(old), size))
    }

    /// The frame shown, once one is made.
    pub(super) fn front(&self) -> Option<&Bitmap> {
        match &self.kept {
            Some(k) => k.frames.get(k.at).map(|f| &f.0),
            None => self.front.as_ref().map(|f| &f.0),
        }
    }

    /// Its frames are kept (once made, no step is run for them).
    pub(super) fn is_kept(&self) -> bool {
        self.kept.as_ref().is_some_and(|k| !k.building)
    }

    /// Whether its frames kept, all made, have a frame after the one shown.
    fn kept_more(k: &Kept) -> bool {
        k.at + 1 < k.frames.len()
            || match k.loops {
                Loops::Forever => k.frames.len() > 1,
                Loops::Plays(n) => k.plays.saturating_add(1) < n,
            }
    }

    /// When the next frame is due, if it is made and the one shown was painted.
    pub(super) fn due(&self) -> Option<Instant> {
        let ready = match &self.kept {
            Some(k) if k.building => k.at + 1 < k.frames.len(),
            Some(k) => Playing::kept_more(k),
            None => self.next.is_some() || self.drawn.is_some(),
        };
        (ready && !self.over).then_some(self.shown? + self.delay)
    }

    /// It has played its loops: the frame shown is its last, for good.
    pub(super) fn ended(&self) -> bool {
        self.over
            || self
                .kept
                .as_ref()
                .is_some_and(|k| !k.building && !Playing::kept_more(k))
    }

    /// Play it again from its first frame, if its frames are kept (`false`: it is read again).
    pub(super) fn rewind(&mut self) -> bool {
        let Some(k) = self.kept.as_mut().filter(|k| !k.building) else {
            return false;
        };
        k.at = 0;
        k.plays = 0;
        self.delay = k.frames.first().map_or(Duration::ZERO, |f| f.1);
        self.shown = None;
        true
    }

    /// Show the next frame (see [`Playing::due`]).
    pub(super) fn swap(&mut self) {
        self.shown = None;
        if let Some(k) = &mut self.kept {
            k.at += 1;
            if k.at >= k.frames.len() {
                k.at = 0;
                k.plays = k.plays.saturating_add(1);
            }
            self.delay = k.frames.get(k.at).map_or(Duration::ZERO, |f| f.1);
            return;
        }
        if let Some((b, n, delay)) = self.next.take() {
            self.spare = self.front.replace((b, n));
            self.delay = delay;
        }
        if let (Some(delay), Some(e)) = (self.drawn, &mut self.engine) {
            let shown = self.front.take().map(|f| f.0);
            self.front = e.show(shown, self.size).map(|b| (b, 0));
            self.delay = delay;
            self.drawn = None;
        }
    }

    /// Whether a step is to run now: it was painted (in `frame`), and there is no frame made
    /// ahead of the one shown (frames are pulled as they are shown, never made ahead).
    pub(super) fn wants_step(&self, frame: u64) -> bool {
        let ahead = match &self.kept {
            Some(k) => !k.building || k.at + 1 < k.frames.len(),
            None => self.next.is_some() || self.drawn.is_some(),
        };
        let shown = self.front().is_none() || (self.shown.is_some() && self.painted == frame + 1);
        self.engine.is_some() && !self.running && !self.over && !ahead && shown
    }

    /// The step to run now: its engine goes to the anim thread with the frame to make the next
    /// in (none while frames are kept: each is new). `Err` if it is to be made at a new box and
    /// its frames are kept (they are made again from the first).
    pub(super) fn task(&mut self) -> Result<Step, ()> {
        if self.want != self.size && !self.resize() {
            return Err(());
        }
        let engine = self.engine.take().ok_or(())?;
        let spare = self
            .spare
            .take()
            .filter(|s| (s.0.width, s.0.height) == self.size);
        self.running = true;
        Ok((engine, self.size, spare))
    }

    /// Its next frame is made at the box it was last painted in: the frame made ahead and the
    /// one before are of the old box, and go. `false` if its frames are kept (they are made
    /// again from the first, by a new one).
    pub(super) fn resize(&mut self) -> bool {
        if self.kept.is_some() {
            return false;
        }
        self.size = self.want;
        self.next = None;
        self.spare = None;
        self.drawn = None;
        true
    }

    /// The anim thread is done with a step: `engine` is back, and made `made`.
    pub(super) fn took(&mut self, engine: Box<Engine>, made: Made) {
        self.running = false;
        self.engine = Some(engine);
        match made {
            Made::Frame {
                bitmap,
                made,
                delay,
                again,
            } => {
                if let Some(k) = &mut self.kept {
                    if again {
                        // All of them are made: the engine (its bytes, its canvas) goes. (The
                        // play is counted as the frame shown wraps, [`Playing::swap`].)
                        k.building = false;
                        self.engine = None;
                        return;
                    }
                    if k.frames.is_empty() {
                        self.delay = delay;
                    }
                    k.frames.push((bitmap, delay));
                    return;
                }
                if self.front.is_none() {
                    self.front = Some((bitmap, made));
                    self.delay = delay;
                } else {
                    self.next = Some((bitmap, made, delay));
                }
            }
            // (The first is shown as it comes, as a frame made is.)
            Made::Drawn { delay } => match &mut self.engine {
                Some(e) if self.front.is_none() => {
                    self.front = e.show(None, self.size).map(|b| (b, 0));
                    self.delay = delay;
                }
                _ => self.drawn = Some(delay),
            },
            Made::Over => {
                self.engine = None;
                self.spare = None;
                self.drawn = None;
                match &mut self.kept {
                    // Kept as they are, played to the end of this play.
                    Some(k) => {
                        k.building = false;
                        k.loops = Loops::Plays(k.plays.saturating_add(1));
                    }
                    None => self.over = true,
                }
            }
            Made::Cancelled => {}
        }
    }
}

/// A job of the anim thread that is charged outside the slots ([`super::Cache::anims`]): an
/// animation being read and indexed, or a step whose animation went while it ran.
pub(super) struct Out {
    pub(super) serial: u64,
    pub(super) key: Key,
    pub(super) doc_gen: u64,
    /// The box it is read for.
    pub(super) size: (u32, u32),
    pub(super) cost: usize,
    /// Played past the limits.
    pub(super) forced: bool,
    pub(super) cancel: Arc<AtomicBool>,
}

/// An animated image painted: its id, its box (left, top, right, bottom; `None` if it was asked
/// for its pixels only), its state, and the frame size it was asked for at.
pub(super) type Placed = (
    u32,
    Option<(i32, i32, i32, i32)>,
    Option<AnimState>,
    Option<(u32, u32)>,
);

/// The (key, frame size) pairs whose costs are kept at most, beside those of the animations
/// painted: a zoom in steps makes a new size at each.
const COSTS_MAX: usize = 64;

/// An animation the allocation weighs ([`allocate`]): one to play that was painted in the last
/// frame, at its frame size, with what it holds playing there (what it holds now if it is held
/// at that size, else what it was measured to hold, and at least what reading it holds; only
/// that until it is measured), and what the body it plays from will hold in the cache beside it
/// (a remote one not held: the body isn't an animation's, so only the cache's room weighs it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Bid {
    pub(super) key: Key,
    pub(super) size: (u32, u32),
    pub(super) cost: usize,
    pub(super) body: usize,
}

/// Which animations may play, made from the bids and the room by [`allocate`], with what it was
/// made of: the same bids and room give the same allocation, so it is only made again when they
/// change.
#[derive(Default)]
pub(super) struct Alloc {
    pub(super) bids: Vec<Bid>,
    pub(super) room: usize,
    /// Those that play within the limits, in their order.
    pub(super) play: Vec<Key>,
    /// Those that don't fit beside those before them: they wait (a click plays one past the
    /// limits).
    pub(super) waiting: Vec<Key>,
}

/// The animations that play, given `bids` in their order (on screen from the top down) and the
/// `room` the cache has for them: each in turn that fits beside those before it, within
/// [`ANIM_ALL_MAX`] and (with its body) the room, plays; one that doesn't, waits. Nothing but its
/// arguments decides it.
pub(super) fn allocate(bids: Vec<Bid>, room: usize) -> Alloc {
    let mut alloc = Alloc::default();
    let (mut slots, mut given) = (0usize, 0usize);
    for bid in &bids {
        let all = given.saturating_add(bid.cost).saturating_add(bid.body);
        if slots.saturating_add(bid.cost) <= ANIM_ALL_MAX && all <= room {
            slots += bid.cost;
            given = all;
            alloc.play.push(bid.key.clone());
        } else {
            alloc.waiting.push(bid.key.clone());
        }
    }
    alloc.bids = bids;
    alloc.room = room;
    alloc
}

/// What an animation measured holds playing at a frame size ([`Anims::costs`]).
pub(super) type Cost = (Key, (u32, u32), usize);

/// The window's animations, besides those in the cache.
#[derive(Default)]
pub(super) struct Anims {
    /// Animations play (the window's `A`), unless the reader chose otherwise for one.
    pub(super) on: bool,
    /// What the reader chose for one image (play or not), for the document's life.
    pub(super) choices: Vec<(Key, bool)>,
    /// The images the reader chose to play past the limits (a click on one over them): they
    /// take no part in the allocation.
    pub(super) forced: Vec<Key>,
    /// The images that don't play: over the limits (`true`: a click plays them past them), or
    /// not an animation that can be played at all.
    pub(super) refused: Vec<(Key, bool)>,
    /// The remote animations whose body couldn't be fetched again to play them (their stills stay):
    /// not asked for again until the reader clicks.
    pub(super) failed: Vec<Key>,
    /// The animations that played their loops: ended, held or let go, until the reader plays
    /// them again ([`super::Images::restart_ended`]).
    pub(super) ended: Vec<Key>,
    /// The jobs charged outside the slots.
    pub(super) out: Vec<Out>,
    /// The animations played past the limits: not in the cache, and not charged to it.
    pub(super) outside: Vec<(Key, Box<Playing>)>,
    pub(super) player: Player,
    /// The last serial given.
    pub(super) serial: u64,
    /// An animation may be made (in the cache or outside).
    pub(super) live: bool,
    /// The time the window paints at (the tests'); `None`: now.
    pub(super) clock: Option<Instant>,
    /// When frames were last shown ([`next_tick`]).
    pub(super) last: Option<Instant>,
    /// The animated images painted in the last frame (see [`Placed`]): what the allocation weighs,
    /// on screen first from the top down, and the states they were painted in: one whose state
    /// changed since is painted again ([`super::Images::state_changed`]).
    pub(super) placed: Vec<Placed>,
    /// What each animation measured holds playing at a frame size (its slot), by key and size:
    /// measured once each, so another size is another entry and two boxes of one image don't
    /// undo each other's.
    pub(super) costs: Vec<Cost>,
    /// What reading each local animation holds (its file, its path, the index), found once.
    pub(super) reads: Vec<(Key, usize)>,
    /// The allocation made last ([`allocate`]).
    pub(super) alloc: Alloc,
    /// The fetched images whose bodies were fetched again to play them (once, until they do).
    pub(super) fetched: Vec<Key>,
    /// The animations the allocation plays that can't be read, for want of room nothing under
    /// way gives back ([`super::Images::stall`]): they wait, until one is read.
    pub(super) stalled: Vec<Key>,
    /// What the allocation set going (reads started, animations let go), and the (key, size)
    /// pairs read to be measured, for the tests.
    #[cfg(test)]
    pub(super) effects: usize,
    #[cfg(test)]
    pub(super) measures: Vec<(Key, (u32, u32))>,
}

impl Anims {
    /// What its lists hold, for the bookkeeping (what is outside the limits is not).
    pub(super) fn bookkeeping(&self) -> usize {
        vec_bytes(&self.choices)
            .saturating_add(vec_bytes(&self.forced))
            .saturating_add(vec_bytes(&self.refused))
            .saturating_add(vec_bytes(&self.out))
            .saturating_add(vec_bytes(&self.outside))
            .saturating_add(vec_bytes(&self.placed))
            .saturating_add(vec_bytes(&self.costs))
            .saturating_add(vec_bytes(&self.reads))
            .saturating_add(vec_bytes(&self.alloc.bids))
            .saturating_add(vec_bytes(&self.alloc.play))
            .saturating_add(vec_bytes(&self.alloc.waiting))
            .saturating_add(vec_bytes(&self.fetched))
            .saturating_add(vec_bytes(&self.stalled))
            .saturating_add(vec_bytes(&self.failed))
            .saturating_add(vec_bytes(&self.ended))
    }

    /// Whether nothing of animations is to be done: none plays, none is chosen, none is made
    /// (with `animations = off` the paint and the polls are what they were).
    pub(super) fn idle(&self) -> bool {
        !self.on && self.choices.is_empty() && !self.live && self.out.is_empty()
    }

    /// Whether the animation of `key` plays: the reader's choice for it, else the window's.
    pub(super) fn wanted(&self, key: &Key) -> bool {
        self.choices
            .iter()
            .find(|c| c.0 == *key)
            .map_or(self.on, |c| c.1)
    }

    /// Whether the body of the remote image at `url` was asked for again to play its animation.
    #[cfg(feature = "remote-images")]
    pub(super) fn is_fetched(&self, url: &Arc<str>) -> bool {
        self.fetched
            .iter()
            .any(|k| matches!(k, Key::Remote(u) if u == url))
    }

    /// Whether the last allocation has `key`'s animation wait, or it can't be read for room.
    pub(super) fn waits(&self, key: &Key) -> bool {
        self.alloc.waiting.contains(key) || self.stalled.contains(key)
    }

    /// What `key`'s animation was measured to hold at `size`.
    pub(super) fn cost(&self, key: &Key, size: (u32, u32)) -> Option<usize> {
        self.costs
            .iter()
            .find(|c| c.0 == *key && c.1 == size)
            .map(|c| c.2)
    }

    /// `key`'s animation holds `cost` at `size`. The oldest of the others go past
    /// [`COSTS_MAX`], but not those of `shown` (painted now).
    pub(super) fn note_cost(
        &mut self,
        key: Key,
        size: (u32, u32),
        cost: usize,
        shown: &[(u32, Key, (u32, u32))],
    ) {
        self.retain_costs(&|c| !(c.0 == key && c.1 == size));
        while self.costs.len() >= COSTS_MAX {
            let painted =
                |c: &(Key, (u32, u32), usize)| shown.iter().any(|s| s.1 == c.0 && s.2 == c.1);
            match self.costs.iter().position(|c| !painted(c)) {
                Some(i) => {
                    self.costs.remove(i);
                }
                None => break,
            }
        }
        self.costs.push((key, size, cost));
    }

    /// Keep the costs `keep` keeps. (Each list is kept through one function, whatever the test,
    /// for the binary's size.)
    pub(super) fn retain_costs(&mut self, keep: &dyn Fn(&Cost) -> bool) {
        self.costs.retain(|c| keep(c));
    }

    /// Forget what is known of the animations whose keys `keep` doesn't keep: what was refused
    /// for them or forced, that they ended, their costs.
    pub(super) fn keep(&mut self, keep: &dyn Fn(&Key) -> bool) {
        self.refused.retain(|r| keep(&r.0));
        retain_keys(&mut self.forced, keep);
        retain_keys(&mut self.ended, keep);
        self.retain_costs(&|c| keep(&c.0));
    }

    pub(super) fn refusal(&self, key: &Key) -> Option<bool> {
        self.refused.iter().find(|r| r.0 == *key).map(|r| r.1)
    }

    pub(super) fn refuse(&mut self, key: Key, over: bool) {
        // (One over the limits that then can't be played at all, past them, is that.)
        match self.refused.iter_mut().find(|r| r.0 == key) {
            Some(r) => r.1 &= over,
            None => self.refused.push((key, over)),
        }
    }

    pub(super) fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }
}

/// Keep the keys `keep` keeps (see [`Anims::retain_costs`]).
pub(super) fn retain_keys(keys: &mut Vec<Key>, keep: &dyn Fn(&Key) -> bool) {
    keys.retain(|k| keep(k));
}

/// What the anim thread is asked to do.
pub(super) enum Task {
    /// Read the file (of `limit` bytes at most) or take the body, and index it.
    Measure { source: Source, limit: usize },
    /// Make the next frame at `size`.
    Step {
        engine: Box<Engine>,
        size: (u32, u32),
        spare: Option<Numbered>,
    },
}

pub(super) struct Job {
    pub(super) serial: u64,
    pub(super) task: Task,
    pub(super) cancel: Arc<AtomicBool>,
    /// The window's, when it was asked for (a new one may have come since the thread started).
    pub(super) wake: Wake,
}

/// What the anim thread sends back for a job, always (what it holds is charged until then).
pub(crate) enum Output {
    /// The engine of an animation, not playing yet; `None` if it isn't one, or can't be read.
    Measured(Option<Box<Engine>>),
    Stepped(Box<Engine>, Made),
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// The thread is there (it ends once it has waited [`IDLE`] for a job).
    alive: bool,
    quit: bool,
    /// How long each step takes besides its work (the tests': a slow machine).
    #[cfg(test)]
    slow: Duration,
}

/// The anim thread: started for a job, ending once none has come for a while, so nothing runs
/// while nothing plays. Its jobs run one at a time, in order.
#[derive(Default)]
pub(super) struct Player {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl Drop for Player {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.quit = true;
        for j in &q.jobs {
            j.cancel.store(true, Ordering::Relaxed);
        }
        cv.notify_one();
    }
}

impl Player {
    pub(super) fn send(&self, job: Job, tx: &Sender<Done>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.jobs.push_back(job);
        if !q.alive {
            q.alive = true;
            let (shared, tx) = (self.shared.clone(), tx.clone());
            let spawned = std::thread::Builder::new()
                .name("vomd-anim".into())
                .spawn(move || work(&shared, &tx));
            if spawned.is_err() {
                q.alive = false;
            }
        }
        cv.notify_one();
    }

    /// Make each step take `by` more (the tests).
    #[cfg(test)]
    pub(super) fn slow_steps(&self, by: Duration) {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).slow = by;
    }

    /// Whether the thread is there (the tests: it ends when idle).
    #[cfg(test)]
    pub(super) fn alive(&self) -> bool {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .alive
    }
}

fn work(shared: &Arc<(Mutex<Queue>, Condvar)>, tx: &Sender<Done>) {
    let (lock, cv) = &**shared;
    loop {
        let job = {
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.quit {
                    q.alive = false;
                    return;
                }
                if let Some(job) = q.jobs.pop_front() {
                    break job;
                }
                let (next, timeout) = cv.wait_timeout(q, IDLE).unwrap_or_else(|e| e.into_inner());
                q = next;
                if timeout.timed_out() && q.jobs.is_empty() {
                    q.alive = false;
                    return;
                }
            }
        };
        let output = match job.task {
            Task::Measure { source, limit } => Output::Measured(measure(source, limit)),
            Task::Step {
                mut engine,
                size,
                spare,
            } => {
                #[cfg(test)]
                std::thread::sleep(lock.lock().unwrap_or_else(|e| e.into_inner()).slow);
                let made = engine.step(size, spare, &job.cancel);
                Output::Stepped(engine, made)
            }
        };
        let done = Done::Anim {
            serial: job.serial,
            output,
        };
        if tx.send(done).is_err() {
            lock.lock().unwrap_or_else(|e| e.into_inner()).alive = false;
            return;
        }
        (job.wake)();
    }
}

/// The engine of the animation `source`: its file read (if it is no longer than `limit`), its
/// index made.
fn measure(source: Source, limit: usize) -> Option<Box<Engine>> {
    let (bytes, own) = match source {
        Source::File(path) => (Arc::new(super::read_file(&path, limit)?), true),
        Source::Bytes(b) => (b, false),
    };
    let index = Index::new(&bytes)?;
    Some(Box::new(Engine::new(bytes, own, index)))
}

/// Plays `bytes` resampled to `size` until `shows` frames were shown, as the window and the anim
/// thread play it (each frame pulled once the one before is shown, streamed or kept), giving
/// `each` the frames shown; `None` if it isn't an animation or what it is charged is over `room`
/// (checked once the index is made, before the canvas is). It is made [`Engine::late`] if that
/// is what fits it in `room`, as the window makes one within [`ANIM_ONE_MAX`]. Returns the
/// charge: the most it holds besides `bytes`.
pub(super) fn play_box(
    bytes: Bytes,
    size: (u32, u32),
    shows: usize,
    room: usize,
    mut each: impl FnMut(&Bitmap),
) -> Option<usize> {
    let index = Index::new(&bytes)?;
    let mut engine = Box::new(Engine::new(bytes, false, index));
    let keep = engine.keeps(size);
    if !keep {
        engine.lean(size, room);
    }
    let mut p = Playing::new(0, engine, size, keep);
    let charge = p.cost;
    if charge > room {
        return None;
    }
    let mut shown = 0;
    while shown < shows {
        if p.wants_step(0) {
            let (mut engine, size, spare) = p.task().ok()?;
            let made = engine.step(size, spare, &super::NO_CANCEL);
            p.took(engine, made);
        } else if p.shown.is_none() && p.front().is_some() {
            each(p.front()?);
            shown += 1;
            p.shown = Some(Instant::now());
            p.painted = 1;
        } else if p.due().is_some() {
            p.swap();
        } else {
            break;
        }
    }
    Some(charge)
}

#[cfg(test)]
mod tests;
