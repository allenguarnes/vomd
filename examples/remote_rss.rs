//! `cargo run --release --example remote_rss -- BASE [N [MODE]]`
//! Memory with many remote images, the `always` policy, in a 1920 × 1080
//! view, against the server `scripts/remote-rss.sh` starts on 127.0.0.1. Reports the peak RSS and
//! the peak of live heap allocations (a counting allocator: what the image budget is about).
//! `MODE`:
//! - `big` (default): N documents' images `BASE/big/0.png` … of ~4 MiB each. The first 3 s
//!   nothing is polled (the window is busy): what the fetchers hold then is the first figure.
//!   Then the results are taken (and the images near the viewport decoded) until none came for
//!   2 s; the peak is the second.
//! - `tiny`: N images (default 12,288) `BASE/tiny/0.png` … of 69 bytes each, polled as `big`.
//! - `grow`: N images (default 8) of 4 MiB are fetched, taken and cached; then another document
//!   asks for 4 images that start and don't finish (`BASE/stall/0.png` …, 7 MiB declared, about
//!   6.5 MiB sent, polled every 5 ms for 4 s): the cache is full of bodies nothing shows.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::platform;
use vomd::settings::RemoteImages;
use vomd::view::View;

/// Counts the bytes live, and the most there were.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add(n: usize) {
    let live = LIVE.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(live, Ordering::Relaxed);
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
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new >= l.size() {
            add(new - l.size());
        } else {
            LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const SIZE: (u32, u32) = (1920, 1080);

fn doc(base: &str, dir: &str, n: usize) -> Doc {
    let src: String = (0..n)
        .map(|i| format!("Image {i}:\n\n![{i}]({base}/{dir}/{i}.png)\n\n"))
        .collect();
    Doc::parse(&src)
}

/// Live bytes before any image was asked for: the window, fonts and layout.
static START: AtomicUsize = AtomicUsize::new(0);

fn report(what: &str) {
    let peak = PEAK.load(Ordering::Relaxed);
    println!(
        "{what}: peak RSS {} KB, peak live allocations {peak} bytes ({} over the start's {})",
        platform::peak_rss_kb(),
        peak.saturating_sub(START.load(Ordering::Relaxed)),
        START.load(Ordering::Relaxed)
    );
}

/// Paint and take results every 5 ms until `stop` says so.
fn run(v: &mut View, buf: &mut [u32], mut stop: impl FnMut(bool) -> bool) {
    loop {
        let now = Instant::now();
        v.paint(buf, SIZE.0 as usize, now);
        v.tick(now, Some(now));
        let got = v.poll_images(Instant::now());
        if stop(got) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Poll until nothing came for 2 s.
fn drain(v: &mut View, buf: &mut [u32]) {
    let mut quiet = Instant::now();
    run(v, buf, |got| {
        if got {
            quiet = Instant::now();
        }
        quiet.elapsed() > Duration::from_secs(2)
    });
}

fn main() {
    let mut args = std::env::args().skip(1);
    let base = args.next().expect("BASE url");
    let n = args
        .next()
        .map(|n| n.parse::<usize>().expect("N"))
        .filter(|&n| n > 0);
    let mode = args.next().unwrap_or_else(|| "big".into());
    let (w, h) = SIZE;
    let fonts = Fonts::new().unwrap();
    let first = match mode.as_str() {
        "big" => doc(&base, "big", n.unwrap_or(16)),
        "tiny" => doc(&base, "tiny", n.unwrap_or(12_288)),
        "grow" => doc(&base, "big", n.unwrap_or(8)),
        _ => panic!("MODE: big, tiny or grow"),
    };
    let mut v = View::with_base(fonts, first, None, (w, h), 1.0, None);
    v.set_remote_policy(RemoteImages::Always);
    let mut buf = vec![0u32; (w * h) as usize];
    // The first frame turns remote images on and starts the fetchers; then the window is busy.
    v.paint(&mut buf, w as usize, Instant::now());
    v.tick(Instant::now(), Some(Instant::now()));
    START.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let start = Instant::now();
    if mode != "grow" {
        std::thread::sleep(Duration::from_secs(3));
        report("busy for 3 s");
    }
    drain(&mut v, &mut buf);
    report(&format!(
        "{mode}: results taken in {:.1} s",
        start.elapsed().as_secs_f64()
    ));
    if mode == "grow" {
        // The bodies are cached; the new document wants others, which don't finish.
        let (live, rss) = (LIVE.load(Ordering::Relaxed), platform::rss_kb());
        println!("cached: {live} bytes live, RSS {rss} KB");
        PEAK.store(live, Ordering::Relaxed);
        v.open(doc(&base, "stall", 4), None);
        let t = Instant::now();
        run(&mut v, &mut buf, |_| t.elapsed() > Duration::from_secs(4));
        println!(
            "unfinished fetches: {} bytes live",
            LIVE.load(Ordering::Relaxed)
        );
        report("grow");
    }
}
