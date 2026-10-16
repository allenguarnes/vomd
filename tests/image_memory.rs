//! What decoding images allocates, counted (review round 3): in a test binary of its
//! own, with a counting allocator, so nothing else runs while it counts. It is one test, run in
//! steps one after another, for the same reason.
#![cfg(feature = "images")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::image::{CACHE_BYTES, Images, decode};
use vomd::layout::{Layout as PageLayout, Params};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        add(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        add(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new >= l.size() {
            add(new - l.size());
        } else {
            LIVE.fetch_sub(l.size() - new, Relaxed);
        }
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Live bytes now; the peak is counted from here.
fn mark() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
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
fn jpeg_metadata_is_not_kept() {
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
fn pending_results_stay_in_the_budget() {
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
    // 29 images off screen, as many as fit.
    images.request(&(1..=29).map(|id| (id, 900, 300)).collect::<Vec<_>>(), 0);
    pump(&mut images, &doc, &mut layout, &woken, 29);
    assert!(images.pixels(29, 900, 300).is_some());
    // Another frame: all of them can be evicted. Three more are wanted, and nobody polls.
    images.begin_frame();
    images.request(&[(30, 900, 300), (31, 900, 300), (32, 900, 300)], 0);
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
fn navigating_during_a_remote_decode_stays_in_the_budget() {
    use std::os::unix::fs::PermissionsExt;
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
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let curl = bin.join("curl");
    std::fs::write(
        &curl,
        format!(
            "#!/bin/sh\nfor u; do :; done\nf=${{u##*/}}\nf=${{f%%\\?*}}\nexec cat '{}'/\"$f\"\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // Nothing else is running in this test binary: one test, run in steps.
    unsafe { std::env::set_var("PATH", path) };

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

#[test]
fn image_memory_stays_within_the_budget() {
    jpeg_metadata_is_not_kept();
    pending_results_stay_in_the_budget();
    #[cfg(feature = "remote-images")]
    navigating_during_a_remote_decode_stays_in_the_budget();
}
