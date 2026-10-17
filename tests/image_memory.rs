//! What decoding images allocates, counted (review rounds 3 to 8): in a test binary of
//! its own, with a counting allocator, so nothing else runs while it counts: each test takes
//! [`serial`] first, and the others wait without allocating.
#![cfg(feature = "images")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::image::{CACHE_BYTES, Images, decode};
use vomd::layout::{Layout as PageLayout, Params};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// What is live that is an image's (see [`Tag`]), and its peak.
static IMAGE_LIVE: AtomicUsize = AtomicUsize::new(0);
static IMAGE_PEAK: AtomicUsize = AtomicUsize::new(0);
/// What a random run holds the image code's live bytes to (0: no run is watched): the bytes live
/// when it began and the budget with its allowance for what is no image's, and, apart, what is
/// painted, which is the one exception ([`mix::Painted`]). Both are read by the allocator, to
/// say where the first allocation past them was made ([`trip`]), and by the library's probe of
/// every call that makes room ([`room_probe`]), which holds the live bytes to them where each is
/// made.
static BASE: AtomicUsize = AtomicUsize::new(0);
static ALLOW: AtomicUsize = AtomicUsize::new(0);
/// The first allocation of a run that took the image code's live bytes past [`BASE`] and
/// [`ALLOW`], and where: for the report of a run that fails.
static TRIPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TRIP: Mutex<String> = Mutex::new(String::new());

/// Whose an allocation is, by the thread that makes it and what it is doing: the image code's
/// (every thread but a test's own, and a test's while it isn't told otherwise: so nothing is left
/// out of the image's account by what a test forgets to say), or the page's (a `Doc` and its
/// `Layout`) and the test driver's (the boxes it builds, its log), which are no image's memory
/// and are counted apart ([`tagged`]). The tag is kept in a header before the allocation, so
/// that what is freed is taken off the account it was put on, by whichever thread frees it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum Tag {
    Image = 0,
    Page = 1,
    Driver = 2,
}

thread_local! {
    /// What this thread has allocated less what it has freed (what it frees of another's
    /// counts against it): [`built`] measures what a call makes by it, whatever the other
    /// threads allocate meanwhile.
    static MINE: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    static TAG: std::cell::Cell<Tag> = const { std::cell::Cell::new(Tag::Image) };
}

/// Run `f` with what this thread allocates counted as `tag`'s.
#[cfg(feature = "remote-images")]
fn tagged<T>(tag: Tag, f: impl FnOnce() -> T) -> T {
    let before = TAG.with(|t| t.replace(tag));
    let out = f();
    TAG.with(|t| t.set(before));
    out
}

fn mine(n: isize) {
    // (Not available while the thread's storage is being torn down: nothing is counted then.)
    let _ = MINE.try_with(|m| m.set(m.get() + n));
}

fn add(n: usize, tag: Tag) {
    mine(n as isize);
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
    if tag == Tag::Image {
        let live = IMAGE_LIVE.fetch_add(n, Relaxed) + n;
        IMAGE_PEAK.fetch_max(live, Relaxed);
        let base = BASE.load(Relaxed);
        if base != 0 && live > base + ALLOW.load(Relaxed) && !TRIPPED.swap(true, Relaxed) {
            trip(live, base + ALLOW.load(Relaxed));
        }
    }
}

/// Note where the allocation that took the image code's live bytes past what is allowed was made
/// (the one that did it first, from whichever thread). The backtrace allocates itself: it is
/// let, since it is only made once and the flag is set.
#[cold]
fn trip(live: usize, limit: usize) {
    let trace = std::backtrace::Backtrace::force_capture();
    let text =
        format!("{live} bytes live, {limit} allowed, the allocation that took it there:\n{trace}");
    *TRIP.lock().unwrap_or_else(|e| e.into_inner()) = text;
}

/// Where the first allocation past what is allowed was made, if one was, for a failure's message.
#[cfg_attr(not(feature = "remote-images"), allow(dead_code))]
fn trip_report() -> String {
    if TRIPPED.load(Relaxed) {
        TRIP.lock().unwrap_or_else(|e| e.into_inner()).clone()
    } else {
        "no single allocation took it past what is allowed (the sum of them did)".into()
    }
}

fn sub(n: usize, tag: Tag) {
    mine(-(n as isize));
    LIVE.fetch_sub(n, Relaxed);
    if tag == Tag::Image {
        IMAGE_LIVE.fetch_sub(n, Relaxed);
    }
}

/// Room before what a caller gets, for the tag (the alignment keeps it a multiple of itself).
fn header(l: Layout) -> usize {
    l.align().max(16)
}

/// The layout of the block that holds the caller's and its header, if there is one: a size that
/// is too big to add the header to (the caller's was within what a `Layout` may be, and its
/// header takes it out) is not an allocation that can be made, and the allocator says so by
/// returning null (a `GlobalAlloc` may not unwind).
fn outer(l: Layout) -> Option<Layout> {
    Layout::from_size_align(l.size().checked_add(header(l))?, header(l)).ok()
}

/// The block at `base` is the caller's `size` bytes after its header: tag it, count it.
unsafe fn seal(base: *mut u8, l: Layout) -> *mut u8 {
    if base.is_null() {
        return base;
    }
    let tag = TAG.try_with(|t| t.get()).unwrap_or(Tag::Image);
    let p = unsafe { base.add(header(l)) };
    unsafe { (p.sub(size_of::<usize>()) as *mut usize).write(tag as usize) };
    add(l.size(), tag);
    p
}

unsafe fn tag_of(p: *mut u8) -> Tag {
    match unsafe { (p.sub(size_of::<usize>()) as *const usize).read() } {
        1 => Tag::Page,
        2 => Tag::Driver,
        _ => Tag::Image,
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        match outer(l) {
            Some(o) => unsafe { seal(System.alloc(o), l) },
            None => std::ptr::null_mut(),
        }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        match outer(l) {
            Some(o) => unsafe { seal(System.alloc_zeroed(o), l) },
            None => std::ptr::null_mut(),
        }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // (What was allocated had a layout of its own, so this is there.)
        let Some(o) = outer(l) else { return };
        sub(l.size(), unsafe { tag_of(p) });
        unsafe { System.dealloc(p.sub(header(l)), o) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // A size that can't be allocated fails with null and leaves the block as it is.
        let (Some(old), Some(grown)) = (
            outer(l),
            Layout::from_size_align(new, l.align()).ok().and_then(outer),
        ) else {
            return std::ptr::null_mut();
        };
        let tag = unsafe { tag_of(p) };
        let base = unsafe { System.realloc(p.sub(header(l)), old, grown.size()) };
        if base.is_null() {
            return base;
        }
        if new >= l.size() {
            add(new - l.size(), tag);
        } else {
            sub(l.size() - new, tag);
        }
        unsafe { base.add(header(l)) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

static SERIAL: Mutex<()> = Mutex::new(());

/// One test counts at a time.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// What `f` leaves allocated by this thread (what a page's `Doc` and `Layout` are, for one).
#[cfg(feature = "remote-images")]
fn built<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = MINE.with(|m| m.get());
    let out = tagged(Tag::Page, f);
    (out, (MINE.with(|m| m.get()) - before).max(0) as usize)
}

/// Live bytes now; the peak is counted from here.
fn mark() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    IMAGE_PEAK.store(IMAGE_LIVE.load(Relaxed), Relaxed);
    live
}

/// The image code's live bytes now ([`Tag::Image`]); their peak is counted from the last [`mark`].
#[cfg(feature = "remote-images")]
fn image_live() -> usize {
    IMAGE_LIVE.load(Relaxed)
}

/// How far the image code's live bytes (from [`image_live`]) got over `start` since the last
/// look, which is where the next one's peak is counted from: no peak falls between two.
#[cfg(feature = "remote-images")]
fn image_step_over(start: usize) -> usize {
    let now = IMAGE_LIVE.load(Relaxed);
    IMAGE_PEAK.swap(now, Relaxed).saturating_sub(start)
}

/// How far the live bytes got over `start`.
fn peak_over(start: usize) -> usize {
    PEAK.load(Relaxed).saturating_sub(start)
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/images")
            .join(name),
    )
    .unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vomd-memory-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `wide.jpg` (600 × 200, baseline) with the segments `next` gives (until it gives none) between
/// its frame header and its scan: the header probe walks over them as the decoder does.
fn jpeg_with(mut next: impl FnMut() -> Option<Vec<u8>>) -> Vec<u8> {
    let src = fixture("wide.jpg");
    let sos = src.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let mut out = src[..sos].to_vec();
    while let Some(seg) = next() {
        out.extend(seg);
    }
    out.extend(&src[sos..]);
    out
}

/// A segment with `marker` holding `data`.
fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, marker];
    out.extend(((data.len() + 2) as u16).to_be_bytes());
    out.extend(data);
    out
}

/// `total` bytes of extended XMP in `APP1` segments (a decoder that copies metadata copies
/// these, then puts them together).
fn xmp(total: usize) -> impl FnMut() -> Option<Vec<u8>> {
    let mut off = 0;
    move || {
        let n = 65_000.min(total - off);
        if n == 0 {
            return None;
        }
        let mut data = b"http://ns.adobe.com/xmp/extension/\0".to_vec();
        data.extend([b'A'; 32]);
        data.extend((total as u32).to_be_bytes());
        data.extend((off as u32).to_be_bytes());
        data.extend(std::iter::repeat_n(b'x', n));
        off += n;
        Some(segment(0xE1, &data))
    }
}

/// `count` `APP2` ICC profile segments with a byte of data each: the smallest a decoder keeps,
/// with an entry and an allocation of its own, which is more than the bytes of the segment.
fn icc(count: usize) -> impl FnMut() -> Option<Vec<u8>> {
    let mut made = 0;
    move || {
        made += 1;
        (made <= count).then(|| {
            let mut data = b"ICC_PROFILE\0".to_vec();
            data.extend([1, 1, 0]);
            segment(0xE2, &data)
        })
    }
}

fn images_for(dir: &Path, src: &str) -> (Doc, PageLayout, Images) {
    let doc = Doc::parse(src);
    let mut layout = PageLayout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.to_path_buf()));
    images.attach(&doc, &mut layout);
    (doc, layout, images)
}

/// Poll until `n` results have been made (counted by the wake) and taken.
fn pump(images: &mut Images, doc: &Doc, layout: &mut PageLayout, woken: &AtomicUsize, n: usize) {
    let t = Instant::now();
    while woken.load(Relaxed) < n {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        images.poll(doc, layout);
        std::thread::sleep(Duration::from_millis(2));
    }
    images.poll(doc, layout);
}

/// A JPEG's metadata isn't kept by its decoder: 11 MiB of extended XMP (a file under the
/// remote limit) took 24 MB over the file's own bytes; 450,000 tiny ICC segments (8 MiB) more,
/// for an entry each.
#[test]
fn jpeg_metadata_is_not_kept() {
    let _serial = serial();
    use vomd::render::Pixels;
    let plain = decode(&fixture("wide.jpg"), 300, 100).unwrap();
    let cases: [(&str, Vec<u8>); 2] = [
        ("11 MiB of extended XMP", jpeg_with(xmp(11 << 20))),
        ("450,000 tiny ICC segments", jpeg_with(icc(450_000))),
    ];
    for (what, bytes) in cases {
        let len = bytes.len();
        // As a local file (the worker reads it, then cuts the metadata out where it is).
        let dir = scratch("meta");
        std::fs::write(dir.join("a.jpg"), &bytes).unwrap();
        drop(bytes);
        let (doc, mut layout, mut images) = images_for(&dir, "![a](a.jpg)");
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        images.set_wake(Arc::new(move || {
            w.fetch_add(1, Relaxed);
        }));
        let start = mark();
        images.request(&[(1, 300, 100)], 1);
        pump(&mut images, &doc, &mut layout, &woken, 1);
        let got = images.pixels(1, 300, 100).expect(what).clone();
        assert_eq!(got, plain, "{what}: the same pixels");
        let over = peak_over(start);
        println!("{what}: file {len} bytes, {over} bytes live over the start at most");
        // The file, its frame (360,000 bytes) and the result: the metadata is no part of it.
        assert!(
            over <= len + (2 << 20),
            "{what}: {over} bytes over, file {len}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
    // Decoding bytes that can't be changed copies them (once, less the metadata).
    let bytes = jpeg_with(xmp(11 << 20));
    let len = bytes.len();
    let start = mark();
    assert_eq!(decode(&bytes, 300, 100), Some(plain));
    let over = peak_over(start);
    println!("decode of borrowed bytes: {over} bytes live over the file at most");
    assert!(
        over <= len + (2 << 20),
        "decode: {over} bytes over, file {len}"
    );
}

/// A result nobody has taken yet is still charged: the worker starts no other decode until it
/// is, so what pending results hold can't pile up beside a full cache (3 more results made
/// while polling was delayed: 34.6 MB live over the start, in a 33.5 MB budget).
#[test]
fn pending_results_stay_in_the_budget() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("pending");
    std::fs::write(dir.join("wide.jpg"), fixture("wide.jpg")).unwrap();
    let src: String = (0..32).map(|_| "![x](wide.jpg)\n\n").collect();
    let (doc, mut layout, mut images) = images_for(&dir, &src);
    let woken = Arc::new(AtomicUsize::new(0));
    let w = woken.clone();
    images.set_wake(Arc::new(move || {
        w.fetch_add(1, Relaxed);
    }));
    let start = mark();
    // 29 images off screen, as many as fit (one that would cross the line is not wanted).
    images.request(&(1..=29).map(|id| (id, 900, 290)).collect::<Vec<_>>(), 0);
    pump(&mut images, &doc, &mut layout, &woken, 29);
    assert!(images.pixels(29, 900, 290).is_some());
    // Another frame: all of them can be evicted. Three more are wanted, and nobody polls.
    images.begin_frame();
    images.request(&[(30, 900, 290), (31, 900, 290), (32, 900, 290)], 0);
    let t = Instant::now();
    while woken.load(Relaxed) < 30 {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        woken.load(Relaxed),
        30,
        "no other decode started while a result is waiting"
    );
    let over = peak_over(start);
    println!("result waiting: {over} bytes live over the start at most (of {CACHE_BYTES})");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    // Taking it lets the worker go on; all three arrive, one at a time.
    pump(&mut images, &doc, &mut layout, &woken, 32);
    let over = peak_over(start);
    assert!(over <= CACHE_BYTES, "{over} bytes over at the end");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A grayscale PNG of `side` × `side` zeros (a few hundred KB, whatever its size in pixels), and
/// then `pad` bytes after its `IEND`: what the decoder never reads, to make the file as big as
/// the fetchers allow.
#[cfg(feature = "remote-images")]
fn png_padded(side: u32, total: usize) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, side, side);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.write_header()
            .unwrap()
            .write_image_data(&vec![0u8; side as usize * side as usize])
            .unwrap();
    }
    assert!(out.len() < total);
    out.resize(total, 0);
    out
}

/// The body being decoded is charged until the decode lets go of it, so navigating away doesn't
/// make room for bodies on top of it: an 11 MiB body (a 6000 × 6000 PNG) was being decoded to
/// 720 × 720 when another document with 30 inline 40 × 30 images of 4 MiB bodies was opened,
/// and 43.7 MB were live over the start in a 33.5 MB budget, until the decode was done. The
/// fetches are the system `curl`'s, a script in front of it on `PATH` that serves files of a
/// scratch directory (nothing is fetched from anywhere).
#[cfg(feature = "remote-images")]
#[test]
fn navigating_during_a_remote_decode_stays_in_the_budget() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("navigate");
    std::fs::write(dir.join("giant"), png_padded(6000, 11 << 20)).unwrap();
    let mut small = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut small, 40, 30);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&[7u8; 40 * 30])
            .unwrap();
    }
    small.resize(small.len() + (4 << 20), 0);
    std::fs::write(dir.join("body"), small).unwrap();
    fake_curl(&dir, &dir);

    let first = Doc::parse("![x](http://127.0.0.1/giant)");
    let mut first_layout = PageLayout::new(&first, Params::default());
    let second_src: String = (0..30)
        .map(|i| format!("![x](http://127.0.0.1/body?{i}) "))
        .collect();
    let second = Doc::parse(&second_src);
    let mut second_layout = PageLayout::new(&second, Params::default());
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    let start = mark();
    images.attach(&first, &mut first_layout);
    images.set_remote(true, &first, &mut first_layout);
    let t = Instant::now();
    while images.stats().fetched == 0 {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        images.begin_frame();
        images.request(&[(1, 720, 720)], 1);
        std::thread::sleep(Duration::from_millis(5));
        images.poll(&first, &mut first_layout);
    }
    // Its body is in the cache: the next frame starts the decode, which takes a while.
    images.begin_frame();
    images.request(&[(1, 720, 720)], 1);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        images.stats().decodes,
        0,
        "the decode is to be running when the document is left"
    );
    // The window moves on to another document.
    images.forget_remote();
    images.attach(&second, &mut second_layout);
    images.set_remote(true, &second, &mut second_layout);
    let boxes: Vec<_> = (1..=30).map(|id| (id, 40, 30)).collect();
    let t = Instant::now();
    loop {
        images.begin_frame();
        images.request(&boxes, boxes.len());
        std::thread::sleep(Duration::from_millis(16));
        images.poll(&second, &mut second_layout);
        if (1..=30).all(|id| images.pixels(id, 40, 30).is_some()) {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(120),
            "stuck: {:?}",
            images.stats()
        );
    }
    let over = peak_over(start);
    println!("navigating during a remote decode: {over} bytes live over the start at most");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    // The first decode finished too (its result is one more image).
    let t = Instant::now();
    while images.stats().decodes < 31 {
        assert!(
            t.elapsed() < Duration::from_secs(120),
            "{:?}",
            images.stats()
        );
        images.begin_frame();
        images.request(&boxes, boxes.len());
        std::thread::sleep(Duration::from_millis(16));
        images.poll(&second, &mut second_layout);
    }
    assert!(peak_over(start) <= CACHE_BYTES);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- Images made for the tests ---------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn png_chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let mut body = ty.to_vec();
    body.extend(data);
    out.extend(&body);
    out.extend(crc32(&body).to_be_bytes());
}

/// Bits put LSB first, as deflate has them.
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    /// `n` bits of `v`, low first; `code`: a Huffman code, high bit first.
    fn put(&mut self, v: u32, n: u32, code: bool) {
        let v = if code {
            (0..n).fold(0, |r, i| r | ((v >> i) & 1) << (n - 1 - i))
        } else {
            v
        };
        self.acc |= u64::from(v) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
}

/// A zlib stream of `n` zero bytes: one fixed-Huffman block of matches of 258 bytes a step, so a
/// PNG of any size is a few KB (the decoder doesn't check the Adler-32, but it is right).
fn zlib_zeros(n: usize) -> Vec<u8> {
    let mut b = Bits {
        out: vec![0x78, 0x01],
        acc: 0,
        n: 0,
    };
    b.put(1, 1, false); // the last block,
    b.put(1, 2, false); // with the fixed codes
    let mut left = n;
    if left > 0 {
        b.put(0x30, 8, true); // a literal 0,
        left -= 1;
    }
    while left >= 258 {
        b.put(0xC5, 8, true); // then 258 more of it (length code 285),
        b.put(0, 5, false); // a byte back
        left -= 258;
    }
    for _ in 0..left {
        b.put(0x30, 8, true);
    }
    b.put(0, 7, true); // the end of the block
    if b.n > 0 {
        b.put(0, 8 - b.n, false);
    }
    // The Adler-32 of zeros: 1 in the low half, the count in the high.
    let mut out = b.out;
    out.extend(((((n as u64 % 65_521) as u32) << 16) | 1).to_be_bytes());
    out
}

/// A PNG of `w` × `h` zero bytes of `color` (PNG color type: 0 gray, 2 RGB, 4 gray + alpha, 6
/// RGBA) and `depth`, Adam7 interlaced or not, its data in `IDAT` chunks of at most `idat` bytes.
fn zero_png(w: u32, h: u32, color: u8, depth: u8, interlaced: bool, idat: usize) -> Vec<u8> {
    let channels = match color {
        2 => 3,
        4 => 2,
        6 => 4,
        _ => 1,
    };
    let row = |px: usize| 1 + (px * channels * usize::from(depth)).div_ceil(8);
    let raw = if interlaced {
        // Pass origins and steps; a pass with no pixels has no rows.
        let passes = [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ];
        passes
            .iter()
            .map(|&(x0, y0, dx, dy): &(usize, usize, usize, usize)| {
                let (pw, ph) = (
                    (w as usize).saturating_sub(x0).div_ceil(dx),
                    (h as usize).saturating_sub(y0).div_ceil(dy),
                );
                if pw > 0 && ph > 0 { ph * row(pw) } else { 0 }
            })
            .sum()
    } else {
        h as usize * row(w as usize)
    };
    let mut out = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([depth, color, 0, 0, u8::from(interlaced)]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    for c in zlib_zeros(raw).chunks(idat.max(1)) {
        png_chunk(&mut out, b"IDAT", c);
    }
    png_chunk(&mut out, b"IEND", &[]);
    out
}

/// `base` (a fixture) with its frame header replaced by one of `marker` (`0xC0` baseline, `0xC2`
/// progressive), `w` × `h`, components sampled `comps` (H << 4 | V each), and its scan header by
/// one of `ns` components; the scan data is the fixture's (a decoder reads what it can of it
/// and goes on: what it allocates is allocated before it).
fn jpeg_variant(base: &[u8], marker: u8, w: u16, h: u16, comps: &[u8], ns: usize) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    while i < base.len() {
        let m = base[i + 1];
        let l = usize::from(u16::from_be_bytes([base[i + 2], base[i + 3]]));
        if matches!(m, 0xC0 | 0xC2) {
            out.extend([0xFF, marker]);
            out.extend(((8 + 3 * comps.len()) as u16).to_be_bytes());
            out.push(8);
            out.extend(h.to_be_bytes());
            out.extend(w.to_be_bytes());
            out.push(comps.len() as u8);
            for (k, &hv) in comps.iter().enumerate() {
                out.extend([k as u8 + 1, hv, 0]);
            }
        } else if m == 0xDA {
            out.extend([0xFF, 0xDA]);
            out.extend(((6 + 2 * ns) as u16).to_be_bytes());
            out.push(ns as u8);
            for k in 0..ns {
                out.extend([k as u8 + 1, 0]);
            }
            out.extend([0, 63, 0]);
            out.extend(&base[i + 2 + l..]);
            return out;
        } else {
            out.extend(&base[i..i + 2 + l]);
        }
        i += 2 + l;
    }
    out
}

/// A `curl` first on `PATH` that serves the files of `serve` (`http://127.0.0.1/NAME?...` is
/// `serve/NAME`; a name with no file is a failure; a name that starts with `slow` is served and
/// then held open for 5 seconds, as a transfer that doesn't end) and logs the URLs it is asked for in
/// `dir/curl.log`: nothing is fetched from anywhere. The tests run one at a time, so setting
/// `PATH` is safe.
#[cfg(feature = "remote-images")]
fn fake_curl(dir: &Path, serve: &Path) {
    fake_curl_hold(dir, serve, "5");
}

/// [`fake_curl`] with `hold` as the seconds a `slow` name is held open for after all its bytes,
/// and a `trickle` name between its halves (a transfer that still needs credit when it is held off).
#[cfg(feature = "remote-images")]
fn fake_curl_hold(dir: &Path, serve: &Path, hold: &str) {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let curl = bin.join("curl");
    std::fs::write(
        &curl,
        format!(
            "#!/bin/sh\nfor u; do :; done\necho \"$u\" >> '{log}'\nf=${{u##*/}}\nf=${{f%%\\?*}}\ncase $f in gated*) head -c 262144 /dev/zero; while [ ! -e '{gate}' ]; do sleep 0.02; done; printf x; exit 0;; esac\ncase $f in slow*) cat '{serve}'/\"$f\"; exec sleep {hold};; esac\ncase $f in trickle*) n=$(wc -c < '{serve}'/\"$f\"); h=$((n / 2)); head -c $h '{serve}'/\"$f\"; sleep {hold}; tail -c +$((h + 1)) '{serve}'/\"$f\"; exit 0;; esac\nexec cat '{serve}'/\"$f\"\n",
            log = dir.join("curl.log").display(),
            gate = dir.join("gate").display(),
            serve = serve.display(),
            hold = hold,
        ),
    )
    .unwrap();
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    unsafe { std::env::set_var("PATH", path) };
}

/// How many times `fake_curl` was run so far.
#[cfg(feature = "remote-images")]
fn curl_runs(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("curl.log")).map_or(0, |log| log.lines().count())
}

/// What the window does with images, and no more (`src/main.rs`): a frame (`begin_frame`, the
/// pixels in view painted, `request` of `boxes`, the first `on_screen` in view) for the viewport,
/// then a poll for each wake the threads send, `every` after it came (a window that is slow to
/// get to its events), and a frame again only if the poll says a repaint is needed. Nothing else
/// asks for a frame, so a wake that was never sent leaves a state stuck, which fails after
/// [`QUIET`] with nothing happening, and `done` (looked at after every wait, wake or not) never
/// holds.
fn frames(
    images: &mut Images,
    doc: &Doc,
    layout: &mut PageLayout,
    boxes: &[(u32, u32, u32)],
    on_screen: usize,
    every: Duration,
    done: impl FnMut(&Images) -> bool,
) {
    drive(images, doc, layout, boxes, on_screen, every, true, done);
}

/// [`frames`] without the first frame: the viewport hasn't changed, only what the threads do
/// makes anything happen.
#[cfg(feature = "remote-images")]
fn events(
    images: &mut Images,
    doc: &Doc,
    layout: &mut PageLayout,
    boxes: &[(u32, u32, u32)],
    on_screen: usize,
    every: Duration,
    done: impl FnMut(&Images) -> bool,
) {
    drive(images, doc, layout, boxes, on_screen, every, false, done);
}

/// How long the event loop that [`frames`] stands in for may have nothing to do (no wake, no
/// frame) while what it waits for isn't there: a transition nothing told the window of stays
/// stuck for good, and this is how a test finds out. (The fake `curl`s hold transfers open for
/// seconds, with nothing to wake for.)
const QUIET: Duration = Duration::from_secs(20);

/// One frame: what is in view painted, then the images asked for. Returns the bytes of the
/// pixels painted (the cache may not evict them: they are what the budget may be gone over by).
fn frame(images: &mut Images, boxes: &[(u32, u32, u32)], on_screen: usize) -> usize {
    use vomd::render::Pixels;
    images.begin_frame();
    let mut painted = 0;
    for &(id, w, h) in boxes.iter().take(on_screen) {
        painted += images
            .pixels(id, w, h)
            .map_or(0, |b| b.data.capacity() * size_of::<u32>());
    }
    // (What is painted can't be evicted by what the request makes room: the allowance for it is
    // there before the request is.)
    ALLOW.fetch_max(painted, Relaxed);
    images.request(boxes, on_screen);
    painted
}

#[allow(clippy::too_many_arguments)]
fn drive(
    images: &mut Images,
    doc: &Doc,
    layout: &mut PageLayout,
    boxes: &[(u32, u32, u32)],
    on_screen: usize,
    every: Duration,
    first: bool,
    mut done: impl FnMut(&Images) -> bool,
) {
    let t = Instant::now();
    let mut seen = images.wakes();
    // What came while nothing was driving it was handled by the event loop meanwhile.
    let polled = images.poll(doc, layout);
    let (mut repaint, mut last) = (first || polled, Instant::now());
    while !done(images) {
        assert!(
            t.elapsed() < Duration::from_secs(120) && last.elapsed() < QUIET,
            "stuck (a wake lost?): {:?}\n{}",
            images.stats(),
            images.memory_report()
        );
        if repaint {
            repaint = false;
            last = Instant::now();
            frame(images, boxes, on_screen);
        }
        std::thread::sleep(every);
        if images.wakes() != seen {
            seen = images.wakes();
            last = Instant::now();
            repaint = images.poll(doc, layout);
        }
    }
}

/// What resampling allocates is in what a decode is charged: a valid 1 × 65,535 PNG at the box
/// layout may ask for (1 × 16,384) has 16,384 spans of weights of its own, which were not, and
/// took a cache of evictable 300 × 100 bitmaps (a few hundred bytes short of full) 0.9 MB over
/// the budget. The same for the other way round, an interlaced image, and JPEGs, a progressive
/// one and one with its scans a component each.
#[test]
fn resampling_is_in_the_decode_reservation() {
    use vomd::render::Pixels;
    let _serial = serial();
    let wide = fixture("wide.jpg");
    let prog = fixture("progressive.jpg");
    /// What it is, its file's name, the file, and the box asked for.
    type Case = (&'static str, &'static str, Vec<u8>, (u32, u32));
    let cases: Vec<Case> = vec![
        (
            "tall PNG",
            "t.png",
            zero_png(1, 65_535, 0, 8, false, 1 << 20),
            (1, 16_384),
        ),
        (
            "wide PNG",
            "t.png",
            zero_png(65_535, 1, 0, 8, false, 1 << 20),
            (16_384, 1),
        ),
        (
            "tall RGBA PNG",
            "t.png",
            zero_png(3, 65_535, 6, 8, false, 1 << 20),
            (3, 16_384),
        ),
        (
            "tall interlaced PNG",
            "t.png",
            zero_png(2, 40_000, 6, 8, true, 1 << 20),
            (2, 16_384),
        ),
        (
            "wide interlaced PNG",
            "t.png",
            zero_png(40_000, 2, 6, 8, true, 1 << 20),
            (16_384, 2),
        ),
        (
            "tall JPEG",
            "t.jpg",
            jpeg_variant(&wide, 0xC0, 8, 65_535, &[0x11, 0x11, 0x11], 3),
            (8, 16_384),
        ),
        (
            "wide JPEG",
            "t.jpg",
            jpeg_variant(&wide, 0xC0, 20_000, 8, &[0x22, 0x11, 0x11], 3),
            (16_384, 8),
        ),
        (
            "tall progressive JPEG",
            "t.jpg",
            jpeg_variant(&prog, 0xC2, 8, 40_000, &[0x11, 0x11, 0x11], 3),
            (8, 16_384),
        ),
        (
            "tall JPEG, a scan a component",
            "t.jpg",
            jpeg_variant(&wide, 0xC0, 8, 40_000, &[0x11, 0x11, 0x11], 1),
            (8, 16_384),
        ),
    ];
    for (what, name, bytes, (bw, bh)) in cases {
        let dir = scratch("resample");
        std::fs::write(dir.join(name), &bytes).unwrap();
        std::fs::write(dir.join("s.png"), zero_png(30, 10, 0, 8, false, 1 << 20)).unwrap();
        // 278 small ones: 278 bitmaps of 300 × 100 fill the cache to within 90 KB.
        let mut src: String = (0..278).map(|_| "![s](s.png) ").collect();
        src += &format!("![t]({name})");
        let (doc, mut layout, mut images) = images_for(&dir, &src);
        let start = mark();
        let small: Vec<_> = (1..=278).map(|id| (id, 300, 100)).collect();
        frames(
            &mut images,
            &doc,
            &mut layout,
            &small,
            278,
            Duration::ZERO,
            |i| i.stats().decoded == 278,
        );
        // Not painted any more: the tall one may evict them.
        images.begin_frame();
        let tall = [(279, bw, bh)];
        frames(
            &mut images,
            &doc,
            &mut layout,
            &tall,
            1,
            Duration::from_millis(1),
            |i| i.stats().decoded > 0 && i.stats().failed == 0 && i.stats().decodes >= 279,
        );
        assert!(images.pixels(279, bw, bh).is_some(), "{what}");
        let over = peak_over(start);
        println!("{what}: {over} bytes live over the start at most (of {CACHE_BYTES})");
        assert!(
            over <= CACHE_BYTES,
            "{what}: {over} bytes live over the start, {CACHE_BYTES} allowed"
        );
        drop(images);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Live bytes now.
fn live() -> usize {
    LIVE.load(Relaxed)
}

/// Removing slots gives back the room of the list they were in, not only their charge: 5,000
/// one-pixel local images (each charged, a list with room for 8,192) and then another document
/// with one bitmap that fills what the budget leaves: 100,000 of them left a list of 7.3 MB
/// that nothing counted, and 37.3 MB of live allocations beside a budget of 33.5 MB. (The first
/// document's own tables are gone too, so what is live after the switch is measured against what
/// was live before anything, and has to be next to nothing.) `VOMD_TINY_SLOTS` makes it more.
#[test]
fn removed_slots_leave_no_room_behind() {
    use vomd::render::Pixels;
    let _serial = serial();
    let n: usize = std::env::var("VOMD_TINY_SLOTS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(5_000);
    let dir = scratch("slots");
    std::fs::write(dir.join("p.png"), zero_png(1, 1, 0, 8, false, 1 << 20)).unwrap();
    let src: String = (0..n).map(|_| "![p](p.png) ").collect();
    let before = mark();
    let (doc, mut layout, mut images) = images_for(&dir, &src);
    let boxes: Vec<_> = (1..=n as u32).map(|id| (id, 1, 1)).collect();
    // A window shows tens of images, not thousands: a request looks through the cache for every
    // box, so they are asked for 25 at a time (those before them stay cached, not asked for).
    for (k, chunk) in boxes.chunks(25).enumerate() {
        let end = (k * 25 + chunk.len()).min(n);
        frames(
            &mut images,
            &doc,
            &mut layout,
            chunk,
            chunk.len(),
            Duration::ZERO,
            |i| i.stats().decoded >= end,
        );
    }
    drop(boxes);
    let tiny = peak_over(before);
    println!("{n} one-pixel images: {tiny} bytes live over the start at most");
    // Another document: they go.
    let doc2 = Doc::parse("![b](p.png)");
    let mut layout2 = PageLayout::new(&doc2, Params::default());
    drop((doc, layout));
    images.attach(&doc2, &mut layout2);
    let residue = live().saturating_sub(before);
    println!("{n} slots gone: {residue} bytes live over the start");
    assert!(residue < 64 << 10, "{residue} bytes left behind");
    // One bitmap of what is left of the budget (a box of 31 MB).
    let side = 10_000u32;
    let wide = (CACHE_BYTES - (CACHE_BYTES >> 5) - (1 << 20)) as u32 / 4 / side;
    frames(
        &mut images,
        &doc2,
        &mut layout2,
        &[(1, wide, side)],
        1,
        Duration::from_millis(1),
        |i| i.stats().decoded == 1,
    );
    assert!(images.pixels(1, wide, side).is_some());
    let over = peak_over(before);
    println!(
        "then one bitmap of {wide} × {side}: {over} bytes live over the start at most (of {CACHE_BYTES})"
    );
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start, {CACHE_BYTES} allowed"
    );
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A remote image whose body has sound dimensions and cut-off data fails to decode: it shows its
/// alt text while the document is open (a repaint, a reload: nothing is fetched again), and is
/// fetched again when the document is opened again, which it wasn't: the body stayed cached and
/// the failure with it, whatever documents came between, and the `curl` log had one line.
#[cfg(feature = "remote-images")]
#[test]
fn a_failed_remote_decode_is_fetched_again_when_reopened() {
    let _serial = serial();
    let dir = scratch("reopen");
    let serve = dir.join("srv");
    std::fs::create_dir_all(&serve).unwrap();
    let png = zero_png(300, 200, 2, 8, false, 1 << 20);
    std::fs::write(serve.join("cut.png"), &png[..png.len() - 40]).unwrap();
    std::fs::write(serve.join("ok.png"), &png).unwrap();
    fake_curl(&dir, &serve);
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    let open = |images: &mut Images, src: &str| {
        let doc = Doc::parse(src);
        let mut layout = PageLayout::new(&doc, Params::default());
        // What `View::open` does.
        images.forget_remote();
        images.attach(&doc, &mut layout);
        images.set_remote(true, &doc, &mut layout);
        (doc, layout)
    };
    let (doc, mut layout) = open(&mut images, "![x](http://127.0.0.1/cut.png)");
    let boxes = [(1, 300, 200)];
    frames(
        &mut images,
        &doc,
        &mut layout,
        &boxes,
        1,
        Duration::from_millis(2),
        |i| i.stats().failed == 1,
    );
    assert_eq!(curl_runs(&dir), 1);
    // Repainting and a reload of this document: it still shows its alt text, and nothing is
    // fetched.
    let still = |images: &mut Images, doc: &Doc, layout: &mut PageLayout, n: u32| {
        for _ in 0..n {
            images.begin_frame();
            images.request(&boxes, 1);
            std::thread::sleep(Duration::from_millis(3));
            images.poll(doc, layout);
        }
    };
    still(&mut images, &doc, &mut layout, 30);
    images.attach(&doc, &mut layout);
    still(&mut images, &doc, &mut layout, 30);
    assert_eq!((images.stats().failed, curl_runs(&dir)), (1, 1));
    // Another document (without it, and with another image), then this one again.
    let (doc2, mut layout2) = open(&mut images, "nothing here ![y](http://127.0.0.1/ok.png)");
    frames(
        &mut images,
        &doc2,
        &mut layout2,
        &boxes,
        1,
        Duration::from_millis(2),
        |i| i.stats().decoded == 1,
    );
    assert_eq!(curl_runs(&dir), 2);
    let (doc, mut layout) = open(&mut images, "![x](http://127.0.0.1/cut.png)");
    frames(
        &mut images,
        &doc,
        &mut layout,
        &boxes,
        1,
        Duration::from_millis(2),
        |i| i.stats().failed == 1,
    );
    assert_eq!(curl_runs(&dir), 3, "fetched again");
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What decoding allocates, besides the file's bytes and the pixels it makes, is within
/// `decode_reservation` for every shape, kind and box of a grid (a bigger one, which takes a
/// minute, with `VOMD_FULL_GRID=1`): the formulas of `decode_cost`, checked against the
/// decoders. Prints the least margin seen, which says how much room the constants have.
#[test]
fn the_reservation_covers_what_decoding_allocates() {
    let _serial = serial();
    let full = std::env::var("VOMD_FULL_GRID").is_ok();
    let mut least = (usize::MAX, String::new());
    let mut count = 0;
    let mut check = |what: String, bytes: &[u8], (tw, th): (u32, u32)| {
        let reserved = vomd::image::decode_reservation(bytes, tw, th).expect(&what);
        let start = mark();
        let bitmap = vomd::image::decode(bytes, tw, th);
        let over = peak_over(start);
        drop(bitmap);
        let (tw, th) = vomd::image::clamp_box(tw, th);
        let used = over.saturating_sub(4 * tw as usize * th as usize);
        assert!(
            used <= reserved,
            "{what}: {used} bytes allocated, {reserved} reserved"
        );
        if reserved - used < least.0 {
            least = (reserved - used, format!("{what}: {used} of {reserved}"));
        }
        count += 1;
    };
    let (colors, widths, heights): (&[(u8, u8)], &[u32], &[u32]) = if full {
        (
            &[(0, 8), (6, 8), (6, 16), (2, 8), (4, 16), (0, 16), (2, 16)],
            &[1, 2, 100, 1000, 4000, 8192, 16_383, 32_767, 65_535],
            &[1, 3, 20, 300, 3000],
        )
    } else {
        (
            &[(0, 8), (6, 16), (2, 8)],
            &[1, 100, 12_000],
            &[1, 20, 1_500],
        )
    };
    for &(color, depth) in colors {
        for &w in widths {
            for &h in heights {
                if u64::from(w) * u64::from(h) > 8_000_000 {
                    continue;
                }
                for (interlaced, idat) in [(false, 32_768), (false, 1), (true, 1 << 20)] {
                    let bytes = zero_png(w, h, color, depth, interlaced, idat);
                    for target in [
                        (1, 1),
                        (300, 100),
                        (w.min(16_384), h.min(16_384)),
                        (1, h.min(16_384)),
                        (w.min(16_384), 1),
                    ] {
                        let what = format!(
                            "PNG {color}/{depth} {w}×{h} interlaced {interlaced} idat {idat} to {target:?}"
                        );
                        check(what, &bytes, target);
                    }
                }
            }
        }
    }
    let (wide, prog) = (fixture("wide.jpg"), fixture("progressive.jpg"));
    let samplings: &[&[u8]] = if full {
        &[
            &[0x11],
            &[0x11, 0x11, 0x11],
            &[0x22, 0x11, 0x11],
            &[0x21, 0x11, 0x11],
            &[0x12, 0x11, 0x11],
            &[0x41, 0x11, 0x11],
            &[0x22, 0x22, 0x22],
            &[0x44, 0x44, 0x44],
            &[0x11, 0x11, 0x11, 0x11],
            &[0x22, 0x11, 0x11, 0x22],
            &[0x24, 0x11, 0x11],
        ]
    } else {
        &[&[0x11], &[0x22, 0x11, 0x11], &[0x44, 0x44, 0x44]]
    };
    let dims: &[(u16, u16)] = if full {
        &[
            (1, 1),
            (100, 50),
            (4000, 8),
            (65_535, 8),
            (65_535, 16),
            (300, 3000),
            (8, 65_535),
            (2000, 3000),
        ]
    } else {
        &[(1, 1), (100, 50), (12_000, 8), (8, 12_000)]
    };

    for (base, marker) in [(&wide, 0xC0), (&prog, 0xC2)] {
        for comps in samplings {
            for &(w, h) in dims {
                for ns in [comps.len(), 1] {
                    let bytes = jpeg_variant(base, marker, w, h, comps, ns);
                    for target in [
                        (1, 1),
                        (300, 100),
                        (u32::from(w).min(16_384), u32::from(h).min(16_384)),
                    ] {
                        let what = format!(
                            "JPEG {marker:#x} {comps:x?} {w}×{h} scan of {ns} to {target:?}"
                        );
                        check(what, &bytes, target);
                    }
                }
            }
        }
    }
    println!("{count} decodes within their reservation; the least room: {least:?}");
}

/// A valid 795 × 10,000 gray PNG: shown at its natural size its bitmap is 31.8 MB, which a
/// 33.5 MB budget has room for only if nothing else is held beside it.
fn tall_png() -> Vec<u8> {
    zero_png(795, 10_000, 0, 8, false, 1 << 20)
}

/// A decode that fits the budget but not beside the fetchers' reserve is held back until the
/// downloads have drained: a valid baseline 4000 × 2000 JPEG (a reservation of 27.5 MB) for a
/// 300 × 150 box, fetched while three other transfers each hold 3,932,160 bytes and their pipes
/// open for 5 seconds. It was let in alone, beside what they held: 36,838,378 bytes live over the
/// start in a 33,554,432 budget (24,842,622 now; a debug build). The fetchers are a `curl` in
/// front of the real one on `PATH`.
#[cfg(feature = "remote-images")]
#[test]
fn a_decode_that_does_not_fit_waits_for_the_downloads() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("alone");
    let serve = dir.join("srv");
    std::fs::create_dir_all(&serve).unwrap();
    let big = jpeg_variant(
        &fixture("wide.jpg"),
        0xC0,
        4000,
        2000,
        &[0x22, 0x11, 0x11],
        3,
    );
    let reserved = vomd::image::decode_reservation(&big, 300, 150).unwrap();
    assert!(reserved > 20 << 20 && reserved < CACHE_BYTES, "{reserved}");
    std::fs::write(serve.join("big.jpg"), &big).unwrap();
    for i in 0..3 {
        std::fs::write(
            serve.join(format!("slow{i}.png")),
            png_padded(100, 3_932_160),
        )
        .unwrap();
    }
    fake_curl(&dir, &serve);
    let src = "![a](http://127.0.0.1/big.jpg) ![b](http://127.0.0.1/slow0.png) \
               ![c](http://127.0.0.1/slow1.png) ![d](http://127.0.0.1/slow2.png)";
    let doc = Doc::parse(src);
    let mut layout = PageLayout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    let start = mark();
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let boxes = [(1, 300, 150)];
    images.begin_frame();
    images.request(&boxes, 1);
    // The transfers have read their 3.75 MiB each (they are held open, not slow to start).
    let t = Instant::now();
    while images.fetch_held() < 3 * 3_932_160 {
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "the transfers did not start"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    frames(
        &mut images,
        &doc,
        &mut layout,
        &boxes,
        1,
        Duration::from_millis(2),
        |i| i.stats().decoded == 1,
    );
    assert!(images.pixels(1, 300, 150).is_some());
    let over = peak_over(start);
    println!("a decode beside three downloads: {over} bytes live over the start at most");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Records kept for cached URLs go when their last slot is evicted: 20,000 distinct one-pixel
/// remote images cached, and then a document with a valid 795 × 10,000 PNG at its natural size,
/// which evicts them all. `sizes` kept its 20,000 URLs, their strings and its table: 35,064,224
/// bytes live over the start at most in a 33,554,432 budget, 34,231,019 after the decode (debug
/// build; 32,321,766 and 31,805,759 now).
#[cfg(feature = "remote-images")]
#[test]
fn evicting_the_last_slot_of_a_url_forgets_its_record() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("evict-sizes");
    std::fs::write(dir.join("tall.png"), tall_png()).unwrap();
    let pixel = zero_png(1, 1, 0, 8, false, 1 << 20);
    let before = mark();
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    images.set_test_body(Arc::new(move |_| Some(pixel.clone())));
    images.set_base(Some(dir.clone()));
    {
        let src: String = (0..20_000)
            .map(|i| format!("![p](http://127.0.0.1/{i}) "))
            .collect();
        let doc = Doc::parse(&src);
        let mut layout = PageLayout::new(&doc, Params::default());
        images.attach(&doc, &mut layout);
        images.set_remote(true, &doc, &mut layout);
        frames(
            &mut images,
            &doc,
            &mut layout,
            &[],
            0,
            Duration::from_millis(1),
            |i| i.stats().fetched == 20_000,
        );
    }
    // Their fetches are over: nothing is queued. Then another document.
    let doc = Doc::parse("![t](tall.png)");
    let mut layout = PageLayout::new(&doc, Params::default());
    images.forget_remote();
    images.attach(&doc, &mut layout);
    mark();
    frames(
        &mut images,
        &doc,
        &mut layout,
        &[(1, 795, 10_000)],
        1,
        Duration::from_millis(1),
        |i| i.stats().decoded == 1,
    );
    assert!(images.pixels(1, 795, 10_000).is_some());
    let over = peak_over(before);
    let after = live().saturating_sub(before);
    println!(
        "20,000 cached URLs, then a bitmap of 31.8 MB: {over} bytes live over the start at most, {after} after"
    );
    assert_eq!(images.stats().fetched, 0, "no record of them is left");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert!(after <= CACHE_BYTES, "{after} bytes live after");
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Stopping the fetch queue gives back its room: 100,000 distinct remote URLs queued behind
/// four fetches that don't return, fetching stopped, the fetches failed, and then a document
/// with the same local PNG: the queue's 1.6 MB stayed allocated with nothing counting it,
/// 34,418,492 bytes live over the start at most in a 33,554,432 budget (33,902,487 after; a debug
/// build, 32,321,568 and 31,805,563 now).
#[cfg(feature = "remote-images")]
#[test]
fn stopping_the_fetch_queue_gives_back_its_room() {
    let _serial = serial();
    use std::sync::atomic::AtomicBool;
    use vomd::render::Pixels;
    let dir = scratch("stop-queue");
    std::fs::write(dir.join("tall.png"), tall_png()).unwrap();
    let before = mark();
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    let release = Arc::new(AtomicBool::new(false));
    let gate = release.clone();
    images.set_test_body(Arc::new(move |_| {
        while !gate.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    }));
    images.set_base(Some(dir.clone()));
    {
        let src: String = (0..100_000)
            .map(|i| format!("![p](http://127.0.0.1/{i}) "))
            .collect();
        let doc = Doc::parse(&src);
        let mut layout = PageLayout::new(&doc, Params::default());
        images.attach(&doc, &mut layout);
        images.set_remote(true, &doc, &mut layout);
        images.begin_frame();
        images.request(&[], 0);
        // The four fetchers have their URLs; the rest waits in the queue.
        images.set_remote(false, &doc, &mut layout);
        release.store(true, Relaxed);
        frames(
            &mut images,
            &doc,
            &mut layout,
            &[],
            0,
            Duration::from_millis(1),
            |i| i.stats().fetched + i.stats().failed >= 4,
        );
    }
    let doc = Doc::parse("![t](tall.png)");
    let mut layout = PageLayout::new(&doc, Params::default());
    images.forget_remote();
    images.attach(&doc, &mut layout);
    mark();
    frames(
        &mut images,
        &doc,
        &mut layout,
        &[(1, 795, 10_000)],
        1,
        Duration::from_millis(1),
        |i| i.stats().decoded == 1,
    );
    assert!(images.pixels(1, 795, 10_000).is_some());
    let over = peak_over(before);
    let after = live().saturating_sub(before);
    println!(
        "100,000 URLs queued and stopped, then a bitmap of 31.8 MB: {over} bytes live over the start at most, {after} after"
    );
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert!(after <= CACHE_BYTES, "{after} bytes live after");
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- Review round 9 ------------------------------------------------------------------------

/// A transfer that gives up because the fetchers were held off sends no result, only a wake, and a
/// local decode held back for the fetchers is started when the last of them has: a `curl` that
/// writes 262,144 bytes and waits at a gate for one more, a local 4000 × 2000 JPEG for a 300 × 150
/// box asked for meanwhile (it is held back, and the fetchers held off), and then the gate opens:
/// the transfer asks for credit it is refused, and the window gets one wake. Nothing else asks for
/// a frame (`events`), so the decode starts only if that poll says a repaint is needed. It
/// was never started: no transfer, no decode, a poll that said no a hundred times in a row, and
/// the fetchers held off until something else asked for a frame.
#[cfg(feature = "remote-images")]
#[test]
fn a_transfer_that_gives_up_for_the_pause_starts_the_decode_held_back_for_it() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("held-yield");
    let serve = dir.join("srv");
    std::fs::create_dir_all(&serve).unwrap();
    let big = jpeg_variant(
        &fixture("wide.jpg"),
        0xC0,
        4000,
        2000,
        &[0x22, 0x11, 0x11],
        3,
    );
    std::fs::write(dir.join("big.jpg"), &big).unwrap();
    fake_curl(&dir, &serve);
    let doc = Doc::parse("![j](big.jpg) ![g](http://127.0.0.1/gated.png)");
    let mut layout = PageLayout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    // The transfer has its 256 KiB of credit and waits for more of the body.
    frames(
        &mut images,
        &doc,
        &mut layout,
        &[],
        0,
        Duration::from_millis(2),
        |i| i.fetch_held() >= 262_144,
    );
    // The image comes into view: the decode is held back, the fetchers are held off.
    let boxes = [(1, 300, 150)];
    frames(
        &mut images,
        &doc,
        &mut layout,
        &boxes,
        1,
        Duration::from_millis(2),
        |i| i.memory_report().contains("paused true"),
    );
    assert_eq!(images.stats().decodes, 0);
    // The body goes on: the transfer is refused the credit and gives up.
    std::fs::write(dir.join("gate"), b"").unwrap();
    events(
        &mut images,
        &doc,
        &mut layout,
        &boxes,
        1,
        Duration::from_millis(2),
        |i| i.stats().decodes > 0,
    );
    assert!(images.pixels(1, 300, 150).is_some());
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A document's remote URLs are its own strings, not image memory: 2,000 distinct URLs of 1,000
/// characters, none of them fetched (remote images are off), and a valid 795 × 10,000 PNG (a
/// reservation of 32,594,135 bytes, under the budget) asked for at its natural size. The code
/// kept a copy of every URL, which the budget counted, and the decode was let run beside it:
/// 34,471,848 bytes live over the start at most in a 33,554,432 budget (34,000,975 after; a debug
/// build).
#[test]
fn a_documents_urls_are_the_documents_not_image_memory() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("urls");
    std::fs::write(dir.join("tall.png"), tall_png()).unwrap();
    let mut src = String::from("![t](tall.png)");
    for i in 0..2000 {
        src += &format!(" ![u](http://127.0.0.1/{i}/{})", "a".repeat(1000));
    }
    let doc = Doc::parse(&src);
    let mut layout = PageLayout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    let start = mark();
    images.attach(&doc, &mut layout);
    frames(
        &mut images,
        &doc,
        &mut layout,
        &[(1, 795, 10_000)],
        1,
        Duration::from_millis(1),
        |i| i.stats().decoded == 1,
    );
    assert!(images.pixels(1, 795, 10_000).is_some());
    let over = peak_over(start);
    let after = live().saturating_sub(start);
    println!(
        "2,000 long URLs and a bitmap of 31.8 MB: {over} bytes live over the start at most, {after} after"
    );
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert!(after <= CACHE_BYTES, "{after} bytes live after");
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A local image's path is worked out when a job or a file's size needs it, and not kept: the
/// same PNG with 3,000 local images of 1,000-character paths (3.2 MB of resolved paths kept, as
/// many again copied by a request, when `Images` kept them) is decoded in the budget beside them,
/// where it was refused (the paths and the sources they were kept in left it 35,504,718 bytes
/// live over the start at most: refused, 3,192,439 over with it shown as its alt text; a debug
/// build). One bigger than the budget by itself is another matter (it runs alone, and goes over
/// by its own size), and a decode the bookkeeping leaves no room for is refused (the unit test
/// `a_decode_the_bookkeeping_leaves_no_room_for_is_refused`).
#[test]
fn local_paths_cost_nothing_beside_a_big_decode() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("paths-beside");
    std::fs::write(dir.join("tall.png"), tall_png()).unwrap();
    let mut src = String::from("![t](tall.png)");
    for i in 0..3000 {
        src += &format!(" ![p]({}{i}.png)", "d".repeat(1000));
    }
    let doc = Doc::parse(&src);
    let mut layout = PageLayout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    let start = mark();
    images.attach(&doc, &mut layout);
    frames(
        &mut images,
        &doc,
        &mut layout,
        &[(1, 795, 10_000)],
        1,
        Duration::from_millis(1),
        |i| i.stats().decoded == 1,
    );
    let over = peak_over(start);
    println!("a decode beside 3,000 long local paths: {over} bytes live over the start at most");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert!(images.pixels(1, 795, 10_000).is_some());
    assert_eq!(images.stats().decodes, 1);
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What a document's images cost to keep is one entry each, charged and made room for before it
/// is made (review round 10): 34,000 local images with paths of 1,000 characters took
/// 34,940,890 bytes of resolved paths and sources at `attach`, with nothing cached, decoding or
/// downloading (the budget is 33,554,432), and a second document opened over it (or the same
/// reloaded) was made while the first's was alive. Now an entry is the document's own string,
/// shared, and the path is worked out when a job wants it.
#[test]
fn attaching_a_document_of_long_paths_keeps_no_copy_of_them() {
    let _serial = serial();
    let path = |i: usize| format!("/tmp/{}{i}.png", "abcdefghij/".repeat(90));
    let page = |tag: &str| -> String {
        (0..34_000)
            .map(|i| format!("![{tag}]({})\n\n", path(i)))
            .collect()
    };
    let (first, second) = (Doc::parse(&page("a")), Doc::parse(&page("b")));
    let mut layout = PageLayout::new(&first, Params::default());
    let mut other = PageLayout::new(&second, Params::default());
    let mut images = Images::default();
    // The 24 bytes of a `Src` (the document's string is shared) and what a count of them rounds
    // to: nothing here grows with a path.
    let bound = 34_000 * 32 + (64 << 10);
    let start = mark();
    images.attach(&first, &mut layout);
    let over = peak_over(start);
    let after = live().saturating_sub(start);
    println!(
        "attach of 34,000 long paths: {over} bytes live over the start at most, {after} after"
    );
    assert!(
        over <= bound,
        "{over} bytes live over the start, {bound} allowed"
    );
    // Another document opened over it, and the first again (a reload): the old one's tables are
    // gone before the new one's are made.
    for n in 0..3 {
        let start = mark();
        if n == 1 {
            images.attach(&first, &mut layout);
        } else {
            images.attach(&second, &mut other);
        }
        let over = peak_over(start);
        println!("attach over a document: {over} bytes live over what was there at most");
        assert!(
            over <= bound,
            "{over} bytes over what was there, {bound} allowed"
        );
    }
    let held = live().saturating_sub(start);
    assert!(held <= bound, "{held} bytes live after the documents");
    // The paths are made when wanted: three of them, which are not files, show their alt text.
    let boxes: Vec<_> = (1..=3).map(|id| (id, 40, 30)).collect();
    let start = mark();
    frames(
        &mut images,
        &second,
        &mut other,
        &boxes,
        3,
        Duration::from_millis(1),
        |i| i.stats().failed >= 3,
    );
    let over = peak_over(start);
    assert!(
        over <= 64 << 10,
        "{over} bytes for a request of three images"
    );
}

/// Attaching makes room first: a 795 × 10,000 bitmap (31.8 MB) of a remote image was cached
/// (not painted any more: the document is left) when 3,000 local images with 1,000-character
/// paths were attached, and took 34,883,674 bytes live over the start, beside it, until the next
/// request evicted it. Now their entries are small enough for it to stay, and a document with
/// as many images as the cache has no room for beside it has it evicted before its tables are made.
/// The fetch is a `curl` in front of the real one on `PATH` (nothing is fetched from anywhere).
#[cfg(feature = "remote-images")]
#[test]
fn attaching_makes_room_before_it_makes_the_tables() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("attach-room");
    std::fs::write(dir.join("tall"), tall_png()).unwrap();
    fake_curl(&dir, &dir);
    let first = Doc::parse("![t](http://127.0.0.1/tall)");
    let mut first_layout = PageLayout::new(&first, Params::default());
    let mut paths = String::new();
    for i in 0..3000 {
        paths += &format!("![p]({}{i}.png) ", "d".repeat(1000));
    }
    let long = Doc::parse(&paths);
    let mut long_layout = PageLayout::new(&long, Params::default());
    // 80,000 images: their entries (24 bytes each and a place in the set) are over what the
    // bitmap leaves of the budget.
    let many: String = (0..80_000).map(|i| format!("![p](p{i}.png) ")).collect();
    let many = Doc::parse(&many);
    let mut many_layout = PageLayout::new(&many, Params::default());
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    images.set_base(Some(dir.clone()));
    let start = mark();
    images.attach(&first, &mut first_layout);
    images.set_remote(true, &first, &mut first_layout);
    frames(
        &mut images,
        &first,
        &mut first_layout,
        &[(1, 795, 10_000)],
        1,
        Duration::from_millis(1),
        |i| i.stats().decoded == 1,
    );
    assert!(images.pixels(1, 795, 10_000).is_some());
    let bitmap = live().saturating_sub(start);
    // Left for the next document: remote images are off again.
    images.forget_remote();
    mark();
    images.attach(&long, &mut long_layout);
    let over = peak_over(start);
    println!(
        "3,000 long paths attached beside the bitmap ({bitmap} bytes): {over} bytes live over the start at most"
    );
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert_eq!(images.stats().decoded, 1, "there is room: it stays cached");
    images.forget_remote();
    mark();
    images.attach(&many, &mut many_layout);
    let over = peak_over(start);
    println!("80,000 images attached beside the bitmap: {over} bytes live over the start at most");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert_eq!(
        images.stats().decoded,
        0,
        "no room: evicted before the tables"
    );
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The counting allocator fails an allocation it can't make with null, as a `GlobalAlloc` must
/// (it may not unwind): a layout that is within what `Layout` takes but not with the tag's header
/// added, asked of `alloc`, `alloc_zeroed` and `realloc`, which leaves the block it was given.
#[test]
fn the_counting_allocator_fails_instead_of_unwinding() {
    let _serial = serial();
    let huge = Layout::from_size_align(isize::MAX as usize, 1).unwrap();
    let small = Layout::from_size_align(64, 8).unwrap();
    // (What this thread has live: the others, still ending what earlier tests started, count
    // their own.)
    let mine = || MINE.with(|m| m.get());
    let before = mine();
    unsafe {
        assert!(ALLOC.alloc(huge).is_null());
        assert!(ALLOC.alloc_zeroed(huge).is_null());
        let p = ALLOC.alloc(small);
        assert!(!p.is_null());
        p.write_bytes(7, small.size());
        // Too big to grow to: null, and the block is what it was.
        assert!(ALLOC.realloc(p, small, isize::MAX as usize).is_null());
        assert!(ALLOC.realloc(p, small, usize::MAX).is_null());
        assert!((0..64).all(|i| *p.add(i) == 7));
        assert_eq!(mine(), before + 64);
        // And it can still be grown and freed.
        let q = ALLOC.realloc(p, small, 4096);
        assert!(!q.is_null());
        assert!((0..64).all(|i| *q.add(i) == 7));
        ALLOC.dealloc(q, Layout::from_size_align(4096, 8).unwrap());
    }
    assert_eq!(mine(), before);
}

/// Attaching counts the decode of the old document that nobody has taken the result of: a local
/// 795 × 10,000 PNG (31.8 MB) was decoded, its wake was given and nobody polled, when a document
/// with 80,000 local images was attached, and its tables (1.9 MB) were made beside the bitmap
/// still in the channel: 33,722,626 bytes live over the start in a 33,554,432 budget. The result
/// is of the old document's: it is dropped by the attach (the tables are then made in a cache with
/// room for them). And with the decode still running, the tables wait for it (every image shows
/// its alt text meanwhile, and is looked at again when the decode is taken), never made beside it.
#[test]
fn attaching_counts_a_decode_nobody_has_taken() {
    let _serial = serial();
    let dir = scratch("attach-untaken");
    std::fs::write(dir.join("tall.png"), tall_png()).unwrap();
    let first = Doc::parse("![t](tall.png)");
    let mut first_layout = PageLayout::new(&first, Params::default());
    let many: String = (0..80_000).map(|i| format!("![p](p{i}.png) ")).collect();
    let many = Doc::parse(&many);
    let mut many_layout = PageLayout::new(&many, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&first, &mut first_layout);
    let start = mark();
    // Asked for, decoded, its wake given: the result waits in the channel.
    let seen = images.wakes();
    images.begin_frame();
    images.request(&[(1, 795, 10_000)], 1);
    let t = Instant::now();
    while images.wakes() == seen {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    let waiting = peak_over(start);
    mark();
    images.attach(&many, &mut many_layout);
    let over = peak_over(start);
    println!(
        "80,000 images attached with a decode untaken ({waiting} bytes live over the start): {over} bytes live over the start at most"
    );
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A request makes room before it allocates anything: a 795 × 10,000 bitmap (31.8 MB) was cached,
/// not painted in the frame that began, when a request for 10,000 valid 1 × 1 local images made
/// the table of their sizes (and their jobs) beside it, and the bitmap was evicted only after:
/// 34,940,695 bytes live over the start in a 33,554,432 budget. Now the bitmap goes first.
#[test]
fn a_request_makes_room_before_it_allocates() {
    use vomd::render::Pixels;
    let _serial = serial();
    let dir = scratch("request-room");
    std::fs::write(dir.join("tall.png"), tall_png()).unwrap();
    std::fs::write(dir.join("p.png"), zero_png(1, 1, 0, 8, false, 1 << 20)).unwrap();
    let mut src = String::from("![t](tall.png) ");
    src.extend((0..10_000).map(|_| "![p](p.png) "));
    let (doc, mut layout, mut images) = images_for(&dir, &src);
    let boxes: Vec<_> = (2..=10_001u32).map(|id| (id, 1, 1)).collect();
    let start = mark();
    frames(
        &mut images,
        &doc,
        &mut layout,
        &[(1, 795, 10_000)],
        1,
        Duration::from_millis(1),
        |i| i.stats().decoded == 1,
    );
    assert!(images.pixels(1, 795, 10_000).is_some());
    // The next frame is begun and paints nothing: the bitmap can go.
    images.begin_frame();
    mark();
    let t = Instant::now();
    images.request(&boxes, 25);
    println!("the request took {:?}", t.elapsed());
    let over = peak_over(start);
    println!(
        "10,000 tiny images asked for beside the bitmap: {over} bytes live over the start at most"
    );
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    // And the way a window gets them: a screen at a time, frame after frame.
    for chunk in boxes.chunks(25).take(8) {
        let done = images.stats().decoded + chunk.len();
        frames(
            &mut images,
            &doc,
            &mut layout,
            chunk,
            chunk.len(),
            Duration::ZERO,
            |i| i.stats().decoded >= done.min(200),
        );
    }
    let over = peak_over(start);
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A request for local images carries a path for each job, charged as the job is queued (the
/// queue is replaced by the next request, beside it): 800 images of 900-character paths (hard
/// links to one small PNG) decode in the budget, with the table of their sizes cut to what the
/// requests ask for.
#[test]
fn a_request_for_many_local_images_stays_in_the_budget() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("many-local");
    let deep = dir
        .join("a".repeat(200))
        .join("b".repeat(200))
        .join("c".repeat(200));
    std::fs::create_dir_all(&deep).unwrap();
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 8, 8);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&[9u8; 64])
            .unwrap();
    }
    std::fs::write(deep.join("0.png"), &png).unwrap();
    let rel = format!(
        "{}/{}/{}",
        "a".repeat(200),
        "b".repeat(200),
        "c".repeat(200)
    );
    let mut src = String::new();
    for i in 0..800 {
        if i > 0 {
            std::fs::hard_link(deep.join("0.png"), deep.join(format!("{i}.png"))).unwrap();
        }
        src += &format!("![p]({rel}/{i}.png) ");
    }
    let doc = Doc::parse(&src);
    let mut layout = PageLayout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    let start = mark();
    images.attach(&doc, &mut layout);
    let boxes: Vec<_> = (1..=800).map(|id| (id, 8, 8)).collect();
    frames(
        &mut images,
        &doc,
        &mut layout,
        &boxes,
        800,
        Duration::from_millis(1),
        |i| i.stats().decoded == 800,
    );
    let over = peak_over(start);
    println!("800 local images of 900-character paths: {over} bytes live over the start at most");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert!((1..=800).all(|id| images.pixels(id, 8, 8).is_some()));
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A URL is judged by its length and its first bytes before anything is copied of it: one that
/// begins `http://é` (a character that straddles the eighth byte, which a slice of the first 8
/// bytes can't be made at) was lowercased whole to look at its scheme, and then found too long:
/// a URL of 40 MiB took 8 MiB more than the budget (41,943,040 bytes of the image code's live
/// over the start with it in a document, in a budget of 33,554,432), and so did a copy of any
/// other of that size. It is refused with nothing allocated.
#[cfg(feature = "remote-images")]
#[test]
fn an_oversized_url_is_refused_before_it_is_copied() {
    let _serial = serial();
    let dir = scratch("huge-url");
    let mut url = String::from("http://\u{e9}");
    url.push_str(&"a".repeat(40 << 20));
    let doc_src = format!("![x](<{url}>) ![y](http://127.0.0.1/y)");
    fake_curl(&dir, &dir);
    // The page's own (a `Doc` is the page's, and not what is being measured).
    let ((doc, mut layout), _) = built(|| {
        let doc = Doc::parse(&doc_src);
        let layout = PageLayout::new(&doc, Params::default());
        (doc, layout)
    });
    drop(doc_src);
    // The URL by itself, and in a document that is shown.
    let start = mark();
    let img_start = image_live();
    assert!(!vomd::remote::fetchable(&url));
    let direct = image_step_over(img_start);
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    frames(
        &mut images,
        &doc,
        &mut layout,
        &[(1, 40, 30), (2, 40, 30)],
        2,
        Duration::from_millis(1),
        |i| i.stats().failed >= 1,
    );
    let over = image_step_over(img_start);
    println!(
        "a URL of 40 MiB: {direct} bytes of the image code's live over the start when it is judged, {over} in a document"
    );
    assert!(direct <= 1 << 20, "{direct} bytes allocated to refuse it");
    assert!(
        over <= CACHE_BYTES,
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    assert_eq!(curl_runs(&dir), 1, "only the short URL is fetched");
    // Schemes are in any case, and judged by their bytes.
    for ok in ["http://a/x", "HTTPS://a/x", "hTtP://a/x"] {
        assert!(vomd::remote::fetchable(ok), "{ok}");
    }
    for no in [
        "",
        "http:/",
        "httpx://a",
        "\u{e9}http://a",
        "ftp://a/x",
        "http://a b",
    ] {
        assert!(!vomd::remote::fetchable(no), "{no}");
    }
    let _ = start;
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A remote decode that is still running when another document is opened leaves its result for
/// the new one, which is not in the way of its tables: they are made when it is taken, and if
/// anything is still in the way, by a request for the next frame (which asks the window to try
/// again). The result was kept as painted in the frame it arrived in, so the poll that took it
/// could not evict it for the tables of the document with 80,000 images, and nothing asked for
/// another poll: they stayed unbuilt for good, every image of the document showing its alt text,
/// the decode's bitmap (the old document's) in a slot of the cache, workers and fetchers idle.
#[cfg(feature = "remote-images")]
#[test]
fn the_tables_of_a_document_waiting_for_a_decode_are_made_without_a_poll_asked_for() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("late-decode");
    std::fs::write(dir.join("tall"), tall_png()).unwrap();
    std::fs::write(dir.join("tiny.png"), zero_png(1, 1, 0, 8, false, 1 << 20)).unwrap();
    fake_curl(&dir, &dir);
    let first = Doc::parse("![t](http://127.0.0.1/tall)");
    let mut first_layout = PageLayout::new(&first, Params::default());
    let mut src: String = (0..20).map(|_| "![t](tiny.png) ").collect();
    src.extend((0..80_000).map(|i| format!("![p](p{i}.png) ")));
    let many = Doc::parse(&src);
    let mut many_layout = PageLayout::new(&many, Params::default());
    let mut images = Images::default();
    images.set_policy(vomd::settings::RemoteImages::Always);
    images.set_base(Some(dir.clone()));
    let start = mark();
    images.attach(&first, &mut first_layout);
    images.set_remote(true, &first, &mut first_layout);
    let t = Instant::now();
    while images.stats().fetched == 0 {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        images.begin_frame();
        images.request(&[(1, 795, 10_000)], 1);
        std::thread::sleep(Duration::from_millis(5));
        images.poll(&first, &mut first_layout);
    }
    // Its body is in the cache: the next frame starts the decode, which takes a while.
    images.begin_frame();
    images.request(&[(1, 795, 10_000)], 1);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(images.stats().decodes, 0, "the decode is running");
    images.forget_remote();
    mark();
    images.attach(&many, &mut many_layout);
    // The window shows the new document, and does what it is woken for, and nothing else.
    let boxes: Vec<_> = (1..=20).map(|id| (id, 1, 1)).collect();
    frames(
        &mut images,
        &many,
        &mut many_layout,
        &boxes,
        20,
        Duration::from_millis(1),
        |i| i.stats().decodes >= 21,
    );
    assert!((1..=20).all(|id| images.pixels(id, 1, 1).is_some()));
    let over = peak_over(start);
    println!(
        "a decode finishing after the next document is opened: {over} bytes live over the start at most"
    );
    assert!(
        over <= CACHE_BYTES + (2 << 20),
        "{over} bytes live over the start at most, {CACHE_BYTES} allowed"
    );
    drop(images);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A request asks for the boxes it has room for, most wanted first, and the rest later: the
/// ones already decoded cost it nothing, so what comes after them is asked for by the next one.
/// They were charged as if they were to be decoded, a prefix of boxes that were cached ate the
/// allowance again with each request, and the images after them were never asked for: a valid
/// 835 × 10,000 PNG decoded (a bitmap that takes the budget) and painted with 300 valid 1 × 1
/// ones in view settled with 145 of the 301 cached, the worker idle and nothing to wake the
/// window for, however many frames were painted (143 of 301 here, the others neither decoded nor
/// failed). Beside a bitmap that leaves room for the rest all 301 are decoded; beside one that
/// leaves none (the slot list can't grow, and a result that can't go in is its image's alt text,
/// see `Images::store`) every one is either: nothing is left waiting.
#[test]
fn a_request_cut_short_goes_on_where_it_left_off() {
    let _serial = serial();
    use vomd::render::Pixels;
    let dir = scratch("cut-short");
    std::fs::write(dir.join("tiny.png"), zero_png(1, 1, 0, 8, false, 1 << 20)).unwrap();
    let mut src = String::from("![b](big.png)");
    src.extend((0..300).map(|_| " ![t](tiny.png)"));
    for (side, room) in [(700, true), (835, false)] {
        std::fs::write(
            dir.join("big.png"),
            zero_png(side, 10_000, 0, 8, false, 1 << 20),
        )
        .unwrap();
        let (doc, mut layout, mut images) = images_for(&dir, &src);
        let mut boxes = vec![(1u32, side, 10_000u32)];
        boxes.extend((2..=301).map(|id| (id, 1, 1)));
        frames(
            &mut images,
            &doc,
            &mut layout,
            &boxes[..1],
            1,
            Duration::from_millis(1),
            |i| i.stats().decoded == 1,
        );
        frames(
            &mut images,
            &doc,
            &mut layout,
            &boxes,
            boxes.len(),
            Duration::from_millis(1),
            |i| i.stats().decoded + i.stats().failed >= 301,
        );
        let stats = images.stats();
        println!(
            "{side} x 10,000 and 300 one-pixel images in view: {} of 301 decoded, {} failed",
            stats.decoded, stats.failed
        );
        if room {
            assert_eq!((stats.decoded, stats.failed), (301, 0));
            assert!(
                boxes
                    .iter()
                    .all(|&(id, w, h)| images.pixels(id, w, h).is_some())
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The sites that make room ([`vomd::image::set_room_probe`]), in the order the counts are kept.
#[cfg(feature = "remote-images")]
const ROOM_SITES: [&str; 7] = [
    "attach: tables",
    "slot list",
    "request",
    "request: decodes and fetchers",
    "pause released",
    "prune: the cached URLs",
    "sizes table",
];

/// How many times each of [`ROOM_SITES`] made room in a run that was watched, and, last, any other.
#[cfg(feature = "remote-images")]
static ROOM_CALLS: [AtomicUsize; ROOM_SITES.len() + 1] =
    [const { AtomicUsize::new(0) }; ROOM_SITES.len() + 1];

/// Held to at every call that makes room ([`vomd::image::set_room_probe`], debug builds): the
/// image code's live bytes are within the budget and what is painted (see [`BASE`], [`ALLOW`])
/// when the call is made, so that what was allocated before the room for it was made is found
/// where it is, not at the end of the operation; and when the room is said to be made, they are
/// within it with the allocation that is asked for (a `make_room` whose answer leaves the
/// allocation over the budget is wrong: whatever it left out of its arithmetic is the failure).
#[cfg(all(feature = "remote-images", debug_assertions))]
fn room_probe(site: &'static str, bytes: usize, granted: Option<bool>) {
    let base = BASE.load(Relaxed);
    if base == 0 {
        return;
    }
    let limit = base + ALLOW.load(Relaxed);
    let live = IMAGE_LIVE.load(Relaxed);
    if granted.is_none() {
        let i = ROOM_SITES.iter().position(|s| *s == site);
        ROOM_CALLS[i.unwrap_or(ROOM_SITES.len())].fetch_add(1, Relaxed);
        assert!(
            live <= limit,
            "make_room({site:?}, {bytes}) was called with {live} bytes of the image code live, {limit} allowed: something was allocated before its room was made\n{}",
            trip_report()
        );
    } else if granted == Some(true) {
        assert!(
            live + bytes <= limit,
            "make_room({site:?}, {bytes}) said the room is made with {live} bytes of the image code live, {limit} allowed: its arithmetic leaves something out\n{}",
            trip_report()
        );
    }
}

#[cfg(feature = "remote-images")]
mod mix {
    use super::*;

    // ---- A random mix of operations -----------------------------------------------------------

    /// What the image code's live bytes may go over `CACHE_BYTES` by, besides the painted pixels
    /// of the operation (see [`Painted`]), which are the only exception to the budget: the
    /// allocations nothing charges and no one's memory but the allocator's and the threads' (the
    /// thread structures, the channels, what `Command` builds to start a `curl`). The page's
    /// `Doc` and `Layout` and the driver's own are counted apart ([`Tag`]) and have no part in
    /// it. It is measured, and never used: 200 seeds had the image code's live bytes 0.96 MB or
    /// more under the budget (3.1 MB painted in the frame is the most the exception was).
    const SLACK: usize = 64 << 10;

    /// What a heavy image's decode holds, its reservation and the file it reads from, at most:
    /// with the 3 MiB painted in a frame and the bookkeeping it is within the budget, so that
    /// one that can't be run beside the fetchers is held to it too, as one bigger than the cache
    /// is not (see [`FIT`]).
    const HEAVY: usize = 29 << 20;

    /// The mix never has more painted in a frame than this (a window full of images may go over the
    /// budget), nor an image whose decode holds more than `FIT` besides its result: a
    /// decode that holds the whole budget is let run alone and may go over it too.
    const PAINTED: usize = 3 << 20;

    /// What may be live after a run is over: what the threads and the standard library keep.
    const LEFT: usize = 64 << 10;
    const FIT: usize = 3 << 20;

    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }

        /// xorshift64*.
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn range(&mut self, lo: u32, hi: u32) -> u32 {
            lo + self.below(u64::from(hi - lo) + 1) as u32
        }

        fn percent(&mut self, p: u64) -> bool {
            self.below(100) < p
        }
    }

    /// An image of the mix: its file's name and natural size (0 × 0: not an image).
    #[derive(Clone, Default)]
    struct Asset {
        name: String,
        w: u32,
        h: u32,
        /// Whether it is fetched by `curl`: shown only while remote images are on.
        remote: bool,
    }

    /// Whether decoding `bytes` holds no more than [`FIT`] besides its result.
    fn fits(bytes: &[u8]) -> bool {
        vomd::image::decode_reservation(bytes, 700, 700).is_some_and(|c| c + bytes.len() <= FIT)
    }

    /// What decoding `bytes` to a box a mix makes holds, with the bytes themselves (a fetched
    /// body is the cache's, a local file is read: either way they are held while it runs).
    fn held(bytes: &[u8]) -> usize {
        vomd::image::decode_reservation(bytes, 600, 400).unwrap_or(0) + bytes.len()
    }

    /// A valid image that decodes with 3 MiB to [`HEAVY`] (29 MiB) held, the sizes cameras make: baseline
    /// and progressive JPEGs, an interlaced 16-bit PNG. (Decoded to a box of a few hundred pixels,
    /// the result is small: what is held is the source's frame.) Its natural size is returned.
    fn heavy_image(rng: &mut Rng) -> (Vec<u8>, u32, u32) {
        let (wide, prog) = (fixture("wide.jpg"), fixture("progressive.jpg"));
        loop {
            let (mut w, mut h) = (rng.range(900, 4_200), rng.range(700, 3_000));
            let bytes = match rng.below(4) {
                0 | 1 => jpeg_variant(&wide, 0xC0, w as u16, h as u16, &[0x22, 0x11, 0x11], 3),
                2 => jpeg_variant(&prog, 0xC2, w as u16, h as u16, &[0x22, 0x11, 0x11], 3),
                _ => {
                    (w, h) = (w.min(1_800), h.min(1_200));
                    zero_png(w, h, 6, 16, true, 1 << 20)
                }
            };
            if (3 << 20..=HEAVY).contains(&held(&bytes)) {
                return (bytes, w, h);
            }
        }
    }

    /// The bytes of an image of a random kind and shape, smaller until it [`fits`] (or isn't an
    /// image at all), and its natural size. Extreme sides, interlaced and 16-bit PNGs, baseline,
    /// progressive and multi-scan JPEGs of every sampling, a JPEG with metadata, a PNG cut short.
    fn random_image(rng: &mut Rng, profile: u64) -> (Vec<u8>, u32, u32) {
        let (mut w, mut h) = match (profile, rng.below(8)) {
            // Big pictures, so a page of them fills the cache.
            (1 | 4 | 5, 0..=5) => (rng.range(400, 1_500), rng.range(300, 1_500)),
            // Tiny ones, thousands of slots' worth of bookkeeping in a few.
            (3, 0..=6) => (rng.range(1, 64), rng.range(1, 64)),
            (_, 0 | 1) => (rng.range(16, 900), rng.range(16, 700)),
            (_, 2) => (rng.range(1, 6), rng.range(2_000, 65_535)),
            (_, 3) => (rng.range(2_000, 65_535), rng.range(1, 6)),
            (_, 4) => (rng.range(500, 1_500), rng.range(500, 1_500)),
            (_, 5) => (1, 65_535),
            (_, 6) => (65_535, 1),
            _ => (rng.range(1, 64), rng.range(1, 64)),
        };
        let kind = rng.below(12);
        let color_depth = [(0, 8), (2, 8), (4, 8), (6, 8), (6, 16), (0, 16), (2, 16)];
        let (color, depth) = color_depth[rng.below(7) as usize];
        let interlaced = kind == 5;
        let sampling: [&[u8]; 8] = [
            &[0x11, 0x11, 0x11],
            &[0x22, 0x11, 0x11],
            &[0x21, 0x11, 0x11],
            &[0x12, 0x11, 0x11],
            &[0x41, 0x11, 0x11],
            &[0x11],
            &[0x11, 0x11, 0x11, 0x11],
            &[0x22, 0x22, 0x22],
        ];
        let comps = sampling[rng.below(8) as usize];
        let (wide, prog) = (fixture("wide.jpg"), fixture("progressive.jpg"));
        loop {
            let bytes = match kind {
                0..=5 => zero_png(w, h, color, depth, interlaced, 1 << 20),
                6 => jpeg_variant(
                    &wide,
                    0xC0,
                    w.min(65_535) as u16,
                    h.min(65_535) as u16,
                    comps,
                    comps.len(),
                ),
                7 => jpeg_variant(&prog, 0xC2, w as u16, h as u16, comps, comps.len()),
                8 => jpeg_variant(&wide, 0xC0, w as u16, h as u16, comps, 1),
                9 => {
                    // The size is sound and the data is cut short.
                    let png = zero_png(w, h, color, depth, false, 1 << 20);
                    png[..png.len() - 20].to_vec()
                }
                10 => {
                    // 600 × 200 with up to 3 MB of metadata, which is cut out.
                    let total = rng.range(100_000, 3_000_000) as usize;
                    return (jpeg_with(xmp(total)), 600, 200);
                }
                _ => {
                    // Not an image.
                    let n = rng.range(10, 5_000) as usize;
                    return ((0..n).map(|_| rng.next() as u8).collect(), 0, 0);
                }
            };
            if fits(&bytes) || (w <= 1 && h <= 1) {
                return (bytes, w, h);
            }
            // Smaller on its longer side.
            if w >= h {
                w = (w / 2).max(1);
            } else {
                h = (h / 2).max(1);
            }
        }
    }

    /// The files of one run: `dir/local` for the local images, `dir/srv` for what the fake `curl`
    /// serves (which may be padded with up to 2 MiB after the image, 4 MiB in the heavy profiles, a name
    /// without a file failing),
    /// 12 of each.
    struct Pool {
        local: Vec<Asset>,
        remote: Vec<Asset>,
        /// A bitmap that fills the budget by itself when it is cached (600 × 13,250 gray, shown
        /// as it is: 31.8 MB), and a valid 1 × 1 one (profile 10: thousands of the second asked
        /// for beside the first).
        tall: Asset,
        tiny: Asset,
    }

    fn make_pool(rng: &mut Rng, dir: &Path, profile: u64) -> Pool {
        // 6 and 7: some of the images are heavy, and some transfers are held open.
        let heavy = (6..10).contains(&profile);
        let (local_dir, srv) = (dir.join("local"), dir.join("srv"));
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::create_dir_all(&srv).unwrap();
        std::fs::write(
            local_dir.join("tall.png"),
            zero_png(600, 13_250, 0, 8, false, 1 << 20),
        )
        .unwrap();
        std::fs::write(
            local_dir.join("tiny.png"),
            zero_png(1, 1, 0, 8, false, 1 << 20),
        )
        .unwrap();
        let mut pool = Pool {
            local: Vec::new(),
            remote: Vec::new(),
            tall: Asset {
                name: "tall.png".into(),
                w: 600,
                h: 13_250,
                remote: false,
            },
            tiny: Asset {
                name: "tiny.png".into(),
                w: 1,
                h: 1,
                remote: false,
            },
        };
        for i in 0..12 {
            let (bytes, w, h) = if heavy && rng.percent(40) {
                heavy_image(rng)
            } else {
                random_image(rng, profile)
            };
            let ext = if bytes.starts_with(&[0xFF, 0xD8]) {
                "jpg"
            } else {
                "png"
            };
            let name = format!("l{i}.{ext}");
            std::fs::write(local_dir.join(&name), &bytes).unwrap();
            pool.local.push(Asset {
                name,
                w,
                h,
                remote: false,
            });
        }
        for i in 0..12 {
            let (mut bytes, w, h) = if heavy && rng.percent(40) {
                heavy_image(rng)
            } else {
                random_image(rng, profile)
            };
            let ext = if bytes.starts_with(&[0xFF, 0xD8]) {
                "jpg"
            } else {
                "png"
            };
            // A transfer held open for a while after its bytes are all there (`fake_curl`).
            let slow = if heavy && rng.percent(30) {
                "slow"
            } else if heavy && rng.percent(35) {
                "trickle"
            } else {
                ""
            };
            let name = format!("{slow}r{i}.{ext}");
            if rng.percent(15) {
                // Not there: the fetch fails.
                pool.remote.push(Asset {
                    name,
                    remote: true,
                    ..Asset::default()
                });
                continue;
            }
            let pad = if slow == "trickle" {
                // More than the credit its first half takes, so the second half asks for more.
                (2 << 20) + rng.below(2 << 20) as usize
            } else if heavy && !slow.is_empty() {
                // What a transfer held open has read: the fetchers hold it for as long.
                (3 << 20) + rng.below(1 << 20) as usize
            } else if rng.percent(60) {
                rng.below(if heavy { 4 << 20 } else { 2 << 20 }) as usize
            } else {
                0
            };
            // What its decode holds is no more than [`HEAVY`] with the padding either: the body
            // is the file the decode reads from, so the padding is cut where it would take the
            // image past it, and checked once it is added.
            let room = HEAVY.saturating_sub(held(&bytes));
            bytes.resize(bytes.len() + pad.min(room), 0);
            assert!(
                held(&bytes) <= HEAVY,
                "{name}: a decode holds {} with its padding",
                held(&bytes)
            );
            std::fs::write(srv.join(&name), &bytes).unwrap();
            pool.remote.push(Asset {
                name,
                w,
                h,
                remote: true,
            });
        }
        pool
    }

    /// A document of the mix: its source, and the asset of each image (by id - 1).
    struct Page {
        src: String,
        assets: Vec<Asset>,
    }

    fn random_page(rng: &mut Rng, pool: &Pool, profile: u64) -> Page {
        let mut page = Page {
            src: String::new(),
            assets: Vec::new(),
        };
        // 10: a tiny image, the bitmap, and thousands of tiny ones after it: asked for beside the
        // bitmap once it is cached and has not been painted.
        if profile == 10 {
            page.src += "![t](tiny.png) ![big](tall.png)";
            page.assets.push(pool.tiny.clone());
            page.assets.push(pool.tall.clone());
            for i in 0..rng.range(5_000, 10_000) {
                page.src += &format!(" ![p{i}](tiny.png)");
                page.assets.push(pool.tiny.clone());
            }
            return page;
        }
        let alone = rng.percent(50);
        let count = if profile == 3 {
            rng.range(20, 60)
        } else if (6..10).contains(&profile) {
            rng.range(2, 14)
        } else {
            rng.range(2, 30)
        };
        let remote = [50, 50, 90, 50, 0, 100, 50, 90, 70, 20, 0, 40][profile as usize];
        for i in 0..count {
            let asset = if !rng.percent(remote) {
                let a = pool.local[rng.below(12) as usize].clone();
                page.src += &format!("![i{i}]({})", a.name);
                a
            } else {
                let a = pool.remote[rng.below(12) as usize].clone();
                page.src += &format!("![i{i}](http://127.0.0.1/{})", a.name);
                a
            };
            page.src += if alone { "\n\n" } else { " " };
            page.assets.push(asset);
        }
        // 8 and 9: a document with hundreds of other images after them, with long URLs that can't
        // be fetched (a space in one: refused at once, without a `curl`) or long local paths
        // of files that aren't there: what a document costs to keep per image, in the budget
        // beside the decodes of the heavy images above.
        if (8..10).contains(&profile) {
            for i in 0..rng.range(100, 700) {
                let pad = "u".repeat(rng.range(30, 300) as usize);
                page.src += &if profile == 8 {
                    format!(" ![x](<http://127.0.0.1/ {i} {pad}>)")
                } else {
                    format!(" ![x]({pad}{i}.png)")
                };
                page.assets.push(Asset::default());
            }
        }
        page
    }

    /// A page of profile 11 with tens of thousands of images more: local ones of files that are not
    /// there, and remote ones that are refused at once (a space in the URL, no `curl` is run),
    /// as many as a document that costs most of the budget to keep, or more than it.
    fn huge_page(rng: &mut Rng, pool: &Pool) -> Page {
        let mut page = random_page(rng, pool, 11);
        let extra = rng.range(20_000, 110_000);
        let remote = rng.below(60);
        for i in 0..extra {
            page.src += &if rng.percent(remote) {
                format!(" ![x](<http://127.0.0.1/ {i}>)")
            } else {
                format!(" ![x](m{i}.png)")
            };
            page.assets.push(Asset::default());
        }
        page
    }

    /// The box of image `asset` shown at `zoom` in a column 600 pixels wide, in at most `budget` bytes
    /// of pixels (taken off it): what layout makes of its natural size (a stand-in), as small as
    /// 1 × 1.
    fn box_of(asset: &Asset, zoom: f32, budget: &mut usize) -> (u32, u32) {
        let (w, h) = if asset.w == 0 {
            (100.0, 100.0)
        } else {
            (asset.w as f32, asset.h as f32)
        };
        let scale = zoom * (600.0 / w).min(1.0);
        let (mut bw, mut bh) = ((w * scale).round().max(1.0), (h * scale).round().max(1.0));
        let bytes = bw * bh * 4.0;
        if bytes > *budget as f32 {
            let shrink = (*budget as f32 / bytes).sqrt();
            bw = (bw * shrink).floor().max(1.0);
            bh = (bh * shrink).floor().max(1.0);
        }
        *budget = budget.saturating_sub((bw * bh) as usize * 4);
        (bw as u32, bh as u32)
    }

    /// The pixels painted in the frame, which the cache may not evict: the one exception to the
    /// budget, and so the only thing the image code's live bytes may be over it by
    /// (besides [`SLACK`]), and only by their amount, which the test saw painted: the bytes of the
    /// bitmaps `pixels` gave the frame. During an operation the pixels that can't go are the last
    /// frame's (until the next one begins) or a frame of the operation's, so the most of either is
    /// the allowance for the operation.
    #[derive(Default)]
    struct Painted {
        /// What the last frame painted.
        now: usize,
        /// The most that was protected since the operation began.
        step: usize,
    }

    impl Painted {
        fn begin_step(&mut self) {
            self.step = self.now;
            ALLOW.store(self.step, Relaxed);
        }

        /// A frame of `boxes`.
        fn frame(&mut self, images: &mut Images, boxes: &[(u32, u32, u32)], on_screen: usize) {
            self.now = super::frame(images, boxes, on_screen);
            self.step = self.step.max(self.now);
            ALLOW.fetch_max(self.step, Relaxed);
        }

        /// Note `over`, the image code's live bytes over the start in this operation: how far it
        /// is past the budget (negative: within it), the exception taken off, when that is the
        /// most so far, and with how much painted.
        fn note(&self, over: usize, worst: isize, shown: usize) -> (isize, usize) {
            let past = over as isize - (CACHE_BYTES + self.step) as isize;
            if past >= worst {
                (past, self.step)
            } else {
                (worst, shown)
            }
        }
    }

    /// Wait (a little) for a wake the window hasn't polled for since `seen`: a result that is
    /// there for the next operation to find untaken.
    fn await_wake(images: &Images, seen: usize) {
        let t = Instant::now();
        while images.wakes() == seen && t.elapsed() < Duration::from_millis(150) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// One run of the mix for `seed`: documents opened and left, scrolling, zooming, frames polled
    /// at every pace, remote images turned off and on, the wake replaced, reloads: and after each
    /// operation the image code's live bytes (counted apart from the page's and the driver's: see
    /// [`Tag`]) may not have gone over `CACHE_BYTES + SLACK`, but by the pixels painted in the
    /// frame (see [`Painted`]); the image that holds most is held to [`HEAVY`] with the padding of
    /// a fetched body, checked after it is added. Returns the most the image code's live bytes
    /// were past the budget with the painted pixels taken off (negative: within it).
    fn random_run(seed: u64) -> isize {
        let mut rng = Rng::new(seed);
        let before = live();
        let dir = scratch(&format!("mix{seed}"));
        // 0: a mix, 1: big pictures, 2: mostly remote, 3: many tiny ones, 4: big and all local
        // (the whole budget is theirs), 5: big and all remote, 6: heavy images (decodes of 3 to
        // 29 MiB) and transfers held open, local and remote, 7: the same, mostly remote, 8 and 9: the
        // same with hundreds more images after them, 8 with long URLs, 9 with long local paths,
        // 10: thousands of tiny images asked for beside a bitmap of 18 MB that is cached and not
        // painted, 11: documents of tens of thousands of images opened at any moment, and results
        // nobody has taken when they are (10 and 11 leave polls out, and open and reload with
        // results waiting).
        let profile = seed % 12;
        #[cfg(debug_assertions)]
        vomd::image::set_room_probe(room_probe);
        let pool = tagged(Tag::Driver, || make_pool(&mut rng, &dir, profile));
        fake_curl_hold(&dir, &dir.join("srv"), "0.3");
        let mut images = Images::default();
        images.set_policy(vomd::settings::RemoteImages::Always);
        images.set_base(Some(dir.join("local")));
        // The window's wake, which the window answers with a poll (and a frame if the poll says
        // so): a state nothing wakes it for stays unseen.
        let window = || -> vomd::image::Wake { Arc::new(|| {}) };
        images.set_wake(window());
        // What the page, its layout and the boxes are, and where the window is.
        let new_page = |rng: &mut Rng| {
            if profile == 11 && rng.percent(35) {
                huge_page(rng, &pool)
            } else {
                random_page(rng, &pool, profile)
            }
        };
        let page0 = tagged(Tag::Driver, || random_page(&mut rng, &pool, profile));
        let ((doc, layout), _) = built(|| {
            let doc = Doc::parse(&page0.src);
            let layout = PageLayout::new(&doc, Params::default());
            (doc, layout)
        });
        let mut state = (page0, doc, layout);
        // What the pages' own memory (a `Doc` and its `Layout`) and the driver's (the boxes it
        // makes, its log) are is counted apart from the image code's ([`Tag`]): the budget is
        // held to what the image code has live, with no allowance for what is no image's, and
        // the exception to the budget, the pixels painted in the frame, is the amount the test
        // saw painted (see `Painted`).
        let start = mark();
        let img_start = image_live();
        // From here the allocator and the library's probe hold the live bytes to the budget, until
        // this run is over (whatever way it ends).
        struct Armed;
        impl Drop for Armed {
            fn drop(&mut self) {
                BASE.store(0, Relaxed);
            }
        }
        TRIPPED.store(false, Relaxed);
        ALLOW.store(0, Relaxed);
        BASE.store(img_start + CACHE_BYTES + SLACK, Relaxed);
        let armed = Armed;
        let mut painted = Painted::default();
        let mut log: Vec<String> = Vec::new();
        let (mut top, mut zoom, mut remote) = (0usize, 1.0f32, false);
        let mut floods = 0;
        // The most the image code's live bytes were past the budget, the painted pixels the
        // operation had to keep (the exception, and nothing else, may take it past the budget)
        // taken off, and what was painted when that was.
        let (mut past, mut shown) = (isize::MIN, 0usize);
        // The boxes of the last frame, and how many were in view.
        let mut last: (Vec<(u32, u32, u32)>, usize) = (Vec::new(), 0);
        let open = |images: &mut Images, state: &mut (Page, Doc, PageLayout), page: Page| {
            let ((doc, mut layout), _) = built(|| {
                let doc = Doc::parse(&page.src);
                let layout = PageLayout::new(&doc, Params::default());
                (doc, layout)
            });
            // What `View::open` does.
            images.forget_remote();
            images.attach(&doc, &mut layout);
            *state = (page, doc, layout);
        };
        open(
            &mut images,
            &mut state,
            tagged(Tag::Driver, || new_page(&mut rng)),
        );
        // The wakes the window has taken a poll for.
        let mut seen = images.wakes();
        let big = matches!(profile, 1 | 4 | 5);
        for step in 0..if big { 260 } else { 160 } {
            // The peak is looked at for each operation, with what is painted for it.
            painted.begin_step();
            let n = state.0.assets.len();
            let mut op = rng.below(100);
            // A page of big pictures is kept long enough for its images to fill the cache.
            if big && matches!(op, 60..=67) && rng.percent(85) {
                op = 0;
            }
            let what = match op {
                0..=39 => "frame",
                40..=54 => "scroll",
                55..=59 => "zoom",
                60..=64 => "open",
                65..=67 => "reload",
                68..=72 => "remote off and on",
                73..=80 => "slow poll",
                81..=83 => "set_wake",
                84..=93 => "poll",
                _ => "settle",
            };
            tagged(Tag::Driver, || {
                log.push(format!(
                    "{step}: {what} (top {top}, zoom {zoom}, remote {remote})"
                ));
            });
            match what {
                "scroll" => {
                    top =
                        (top as i64 + rng.range(0, 10) as i64 - 4).clamp(0, n as i64 - 1) as usize;
                }
                "zoom" => zoom = [0.5, 1.0, 1.0, 2.0][rng.below(4) as usize],
                "open" => {
                    let page = tagged(Tag::Driver, || new_page(&mut rng));
                    // Results nobody has taken (a wake the window has not polled for yet) when it
                    // is attached.
                    if profile >= 10 && rng.percent(60) {
                        await_wake(&images, seen);
                    }
                    open(&mut images, &mut state, page);
                    (top, remote) = (0, false);
                }
                "reload" => {
                    if profile >= 10 && rng.percent(60) {
                        await_wake(&images, seen);
                    }
                    let (page, doc, layout) = &mut state;
                    ((*doc, *layout), _) = built(|| {
                        let doc = Doc::parse(&page.src);
                        let layout = PageLayout::new(&doc, Params::default());
                        (doc, layout)
                    });
                    images.forget_remote();
                    images.attach(doc, layout);
                    remote = false;
                }
                "remote off and on" => {
                    let (_, doc, layout) = &mut state;
                    if remote {
                        images.set_remote(false, doc, layout);
                        remote = false;
                    } else {
                        images.set_remote(true, doc, layout);
                        remote = true;
                    }
                }
                "set_wake" => images.set_wake(window()),
                "slow poll" => std::thread::sleep(Duration::from_millis(rng.range(30, 90).into())),
                _ => {}
            }
            // The window turns remote images on after a document's first frame, usually.
            if !remote && matches!(what, "open" | "reload" | "frame") && rng.percent(60) {
                let (_, doc, layout) = &mut state;
                images.set_remote(true, doc, layout);
                remote = true;
            }
            let (page, doc, layout) = &mut state;
            let n = page.assets.len();
            let mut wide = false;
            // Frame: paint what is in view, ask for the images near.
            if matches!(
                what,
                "frame" | "scroll" | "zoom" | "open" | "reload" | "slow poll" | "settle"
            ) {
                // Profile 10: now and then hundreds of tiny images in view beside the cached
                // bitmap, painted again and again (a request has room for some of them: the next
                // asks for the rest), and the reader waits for all of them.
                wide = profile == 10 && floods < 3 && what == "frame" && rng.percent(15);
                let in_view = if wide {
                    let (lo, hi) = wide_view();
                    rng.range(lo, hi)
                } else {
                    rng.range(1, if profile == 3 { 40 } else { 6 })
                } as usize;
                // Profile 10: now and then a request for thousands of tiny images (more than a
                // window holds), which is not repeated by every result that comes: the frames
                // that follow ask for what a window does.
                // (At most three in a run: each decodes the bitmap again, which takes seconds.)
                let flood =
                    profile == 10 && !wide && floods < 3 && what == "frame" && rng.percent(15);
                floods += usize::from(flood || wide);
                let near = match profile {
                    1 | 4 | 5 => rng.range(0, 14),
                    10 if flood => rng.range(3_000, 9_000),
                    10 if wide => 0,
                    10 => rng.range(0, 12),
                    _ => rng.range(0, 8),
                } as usize;
                let build = |top: usize, in_view: usize, near: usize| {
                    let mut budget = PAINTED;
                    let mut boxes = Vec::new();
                    for (k, id) in (top..n).enumerate().take(in_view + near) {
                        // (Profile 10's bitmap is asked for near, at its size: it is not painted.)
                        let near_cap = if profile == 10 { 20 << 20 } else { 4 << 20 };
                        let mut near_budget = if k < in_view { budget } else { near_cap };
                        let b = box_of(&page.assets[id], zoom, &mut near_budget);
                        if k < in_view {
                            budget = near_budget;
                        }
                        boxes.push((id as u32 + 1, b.0, b.1));
                    }
                    boxes
                };
                if flood || wide {
                    // The bitmap is cached first (in view: it is decoded at its size), and then
                    // the window has scrolled past it, painting what is not it: what the flood
                    // finds is a cache that is full of it, nothing in it wanted or painted.
                    let warm = vec![(1, 1, 1), (2, 600, 13_250)];
                    painted.frame(&mut images, &warm, 2);
                    last = (warm, 2);
                    let t = Instant::now();
                    // (The slot of the bitmap costs its pixels and 64 bytes: the report says so.)
                    let bitmap = format!("{}", 600 * 13_250 * 4 + 64);
                    while !images.memory_report().contains(&bitmap)
                        && t.elapsed() < Duration::from_secs(30)
                    {
                        std::thread::sleep(Duration::from_millis(2));
                        if images.wakes() != seen {
                            seen = images.wakes();
                            if images.poll(doc, layout) {
                                painted.frame(&mut images, &last.0, last.1);
                            }
                        }
                    }
                    let away = vec![(1, 1, 1)];
                    painted.frame(&mut images, &away, 1);
                    last = (away, 1);
                    // (What the warm-up did is looked at with what it painted, before the flood is
                    // looked at with what the flood has.)
                    let over = image_step_over(img_start);
                    (past, shown) = painted.note(over, past, shown);
                    assert!(
                        over <= CACHE_BYTES + SLACK + painted.step,
                        "seed {seed}: {over} bytes of the image code's live over the start while the bitmap was cached, {} painted ({}); {:?}\n{}",
                        painted.step,
                        images.memory_report(),
                        images.stats(),
                        trip_report()
                    );
                    painted.begin_step();
                    top = 2.min(n.saturating_sub(1));
                }
                let mut boxes = tagged(Tag::Driver, || build(top, in_view, near));
                // The bitmap is in view too, painted at its size with all of them: the budget is
                // its, and what is left is what the images beside it have.
                let in_view = if wide {
                    boxes.insert(0, (2, 600, 13_250));
                    in_view + 1
                } else {
                    in_view
                };
                if flood {
                    // Asked for without a paint and not remembered: the next frame is the
                    // window's, and replaces what was queued.
                    let t = Instant::now();
                    images.begin_frame();
                    images.request(&boxes, in_view.min(boxes.len()));
                    log.push(format!(
                        "   a flood of {} boxes took {:?}",
                        boxes.len(),
                        t.elapsed()
                    ));
                } else {
                    painted.frame(&mut images, &boxes, in_view.min(boxes.len()));
                    last = (boxes, in_view);
                }
            } else if what == "remote off and on" {
                // Turning them on or off asks for the images again without a repaint (what was
                // painted is still the frame's).
                images.request(&last.0, last.1);
            }
            if what != "slow poll" {
                let pace = [0u64, 0, 1, 2, 5, 16][rng.below(6) as usize];
                std::thread::sleep(Duration::from_millis(pace));
            }
            // The window handles what the threads woke it for: a poll for them, and a frame if the
            // poll says so. (Not always, in profiles 10 and 11: the next operation may find
            // results waiting.)
            let leave = profile >= 10 && what != "settle" && rng.percent(30);
            if images.wakes() != seen && !leave {
                seen = images.wakes();
                if images.poll(doc, layout) {
                    painted.frame(&mut images, &last.0, last.1);
                }
            }
            // The reader stops and waits: what the last frame asked for is done by the threads
            // and the window's polls alone, and the budget holds meanwhile.
            if what == "settle" || wide {
                let t = Instant::now();
                painted.frame(&mut images, &last.0, last.1);
                while !images.is_idle() || images.wakes() != seen {
                    assert!(
                        t.elapsed() < Duration::from_secs(60),
                        "seed {seed}: stuck after step {step} with nothing more asked for: {:?} ({})\n{}",
                        images.stats(),
                        images.memory_report(),
                        log.iter()
                            .rev()
                            .take(8)
                            .rev()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    std::thread::sleep(Duration::from_millis(2));
                    if images.wakes() != seen {
                        seen = images.wakes();
                        if images.poll(doc, layout) {
                            painted.frame(&mut images, &last.0, last.1);
                        }
                    }
                    let over = image_step_over(img_start);
                    (past, shown) = painted.note(over, past, shown);
                    assert!(
                        over <= CACHE_BYTES + SLACK + painted.step,
                        "seed {seed}: {over} bytes of the image code's live over the start, {} painted ({}); {:?}\n{}",
                        painted.step,
                        images.memory_report(),
                        images.stats(),
                        trip_report()
                    );
                }
                liveness(
                    seed,
                    &format!("after step {step}"),
                    &mut images,
                    page,
                    remote,
                    &last,
                    &log,
                );
            }
            let allowed = CACHE_BYTES + SLACK + painted.step;
            let over = image_step_over(img_start);
            (past, shown) = painted.note(over, past, shown);
            if over > allowed {
                let recent: Vec<_> = log.iter().rev().take(14).rev().cloned().collect();
                panic!(
                    "seed {seed}: {over} bytes of the image code's live over the start after step {step}, {} allowed with {} painted ({:?}; {})\nrecent steps:\n{}\n{}",
                    allowed,
                    painted.step,
                    images.stats(),
                    images.memory_report(),
                    recent.join("\n"),
                    trip_report()
                );
            }
        }
        // What the last frame asked for gets done, in the budget, whatever was held back on the
        // way (a transfer held off for a decode, a body that waits): nothing is stuck for good.
        let t = Instant::now();
        let (page, doc, layout) = &mut state;
        // One last frame, as the window paints after the last thing it was asked; then only
        // what the threads wake it for (a state that is left only by a request nothing woke the
        // window for is stuck for good, and a failure here).
        painted.begin_step();
        painted.frame(&mut images, &last.0, last.1);
        while !images.is_idle() || images.wakes() != seen {
            assert!(
                t.elapsed() < Duration::from_secs(60),
                "seed {seed}: stuck with nothing more asked for: {:?} ({})\n{}",
                images.stats(),
                images.memory_report(),
                log.iter()
                    .rev()
                    .take(8)
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            std::thread::sleep(Duration::from_millis(2));
            if images.wakes() != seen {
                seen = images.wakes();
                if images.poll(doc, layout) {
                    painted.frame(&mut images, &last.0, last.1);
                }
            }
            let over = image_step_over(img_start);
            (past, shown) = painted.note(over, past, shown);
            assert!(
                over <= CACHE_BYTES + SLACK + painted.step,
                "seed {seed}: {over} bytes of the image code's live over the start, {} painted ({}); {:?}\n{}",
                painted.step,
                images.memory_report(),
                images.stats(),
                trip_report()
            );
        }
        liveness(seed, "at the end", &mut images, page, remote, &last, &log);
        let _ = start;
        drop(armed);
        // Everything is let go, and nothing is left: the threads end and take what they hold
        // with them.
        let stats = images.stats();
        drop((images, state, pool, log));
        let _ = std::fs::remove_dir_all(&dir);
        let t = Instant::now();
        while live() > before + LEFT && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = live().saturating_sub(before);
        assert!(
            left <= LEFT,
            "seed {seed}: {left} bytes live after everything is dropped"
        );
        println!(
            "seed {seed}: the image code's live bytes were at most {past} past the budget ({shown} painted), {left} left after, {stats:?}"
        );
        past
    }

    /// Nothing is left to do (no thread working, no wake the window has not taken a poll for, no
    /// frame asked for): every image in view of the last frame that is still to be shown (a file,
    /// or a remote image while remote images are on that has not failed: [`Images::is_waiting`])
    /// is decoded at the size of its box, and the tables of the document are made. If not, work
    /// that could be done is not and nothing will do it: an idle state with work left in it is
    /// stuck for good. (An image the budget has no room for is not waiting: its alt text is its
    /// answer.)
    fn liveness(
        seed: u64,
        when: &str,
        images: &mut Images,
        page: &Page,
        remote: bool,
        last: &(Vec<(u32, u32, u32)>, usize),
        log: &[String],
    ) {
        use vomd::render::Pixels;
        let report = |images: &Images| {
            format!(
                "{:?}\n{}\nrecent steps:\n{}",
                images.stats(),
                images.memory_report(),
                log.iter()
                    .rev()
                    .take(10)
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        assert!(
            !images.tables_waiting(),
            "seed {seed}: the tables of the document are not made and nothing is left to do {when}\n{}",
            report(images)
        );
        for &(id, w, h) in last.0.iter().take(last.1) {
            let Some(asset) = page.assets.get(id as usize - 1) else {
                continue;
            };
            if (asset.remote && !remote) || !images.is_waiting(id) {
                continue;
            }
            let (w, h) = vomd::image::clamp_box(w, h);
            let shown = images
                .pixels(id, w, h)
                .is_some_and(|b| (b.width, b.height) == (w, h));
            assert!(
                shown,
                "seed {seed}: image {id} ({}, {} x {}) is in view at {w} x {h}, is still to be shown and nothing is left to do {when}: not decoded\n{}",
                asset.name,
                asset.w,
                asset.h,
                report(images)
            );
        }
    }

    /// How many tiny images a wide frame of profile 10 has in view: `VOMD_IMAGE_WIDE=A..B`, 150 to
    /// 300 by default (a request is made for hundreds of them beside the bitmap that fills the
    /// budget, and takes a while in a debug build: with a few thousand a request has room for only
    /// some of the boxes, and the next asks for the rest).
    fn wide_view() -> (u32, u32) {
        let var = std::env::var("VOMD_IMAGE_WIDE").unwrap_or_default();
        match var.split_once("..") {
            Some((a, b)) => (a.parse().unwrap(), b.parse().unwrap()),
            None => (150, 300),
        }
    }

    /// Which seeds to run: `VOMD_IMAGE_SEEDS=A..B` (or `N`: 0 to N), 3 by default.
    fn seeds() -> std::ops::Range<u64> {
        let var = std::env::var("VOMD_IMAGE_SEEDS").unwrap_or_default();
        match var.split_once("..") {
            Some((a, b)) => a.parse().unwrap()..b.parse().unwrap(),
            None => 0..var.parse().unwrap_or(3),
        }
    }

    /// A random mix of what the window does with images, local and remote (through
    /// a fake `curl`), of every shape the decoders take, with the counting allocator watching the
    /// live bytes: they stay within the budget and what documented here isn't image memory.
    #[test]
    fn a_random_mix_of_operations_stays_in_the_budget() {
        let _serial = serial();
        let mut worst = isize::MIN;
        for seed in seeds() {
            worst = worst.max(random_run(seed));
        }
        println!(
            "worst: the image code's live bytes were {worst} past the budget, painted pixels taken off (of {SLACK} allowed)"
        );
        println!(
            "make_room calls, each held to the budget when made: {}",
            ROOM_SITES
                .iter()
                .zip(&ROOM_CALLS)
                .map(|(site, n)| format!("{site} {}", n.load(Relaxed)))
                .chain([format!(
                    "other {}",
                    ROOM_CALLS[ROOM_SITES.len()].load(Relaxed)
                )])
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
