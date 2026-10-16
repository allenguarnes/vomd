//! `cargo run --release --example remote_rss -- BASE [N [MODE [POLL_MS]]]`
//! Memory with many remote images, the `always` policy, in a 1920 × 1080
//! view, against the server `scripts/remote-rss.sh` starts on 127.0.0.1. Reports the peak RSS and
//! the peak of live heap allocations (a counting allocator: what the image budget is about).
//! The window paints and takes results every `POLL_MS` ms (default 16, a frame: a real window
//! takes them when it is woken, no sooner, and the decode worker waits for each result to be
//! taken); the run fails (exit status 1) unless the images the mode is about were all fetched
//! and decoded, within 3 minutes. `MODE`:
//! - `big` (default): N documents' images `BASE/big/0.png` … of ~4 MiB each. The first 3 s
//!   nothing is polled (the window is busy): what the fetchers hold then is the first figure.
//!   Then the results are taken (and the images near the viewport decoded) until all N are
//!   fetched; the peak is the second.
//! - `tiny`: N images (default 12,288) `BASE/tiny/0.png` … of 69 bytes each, polled as `big`.
//! - `inline`: N images (default 50) `BASE/big/0.png` … of 4 MiB with a 40 × 30 picture each, all
//!   in one paragraph, so all are in view and all are decoded: the bodies of the images in view
//!   are held until the decoder is done with them, not (as they once were) however many arrive.
//! - `stripped`: N images (default 16,384) `BASE/stripped/0.jpg` … of a 178-byte JPEG with 325 KB
//!   of EXIF in front, which is cut out of what is kept (the bodies, a mapping each when they
//!   are read, must not stay one).
//! - `grow`: N images (default 8) of 4 MiB are fetched, taken and cached; then another document
//!   asks for 4 images that start and don't finish (`BASE/stall/0.png` …, 7 MiB declared, about
//!   6.5 MiB sent, polled for 4 s): the cache is full of bodies nothing shows.
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

fn doc(base: &str, dir: &str, ext: &str, n: usize, inline: bool) -> Doc {
    let src: String = if inline {
        (0..n)
            .map(|i| format!("![{i}]({base}/{dir}/{i}.{ext}) "))
            .collect()
    } else {
        (0..n)
            .map(|i| format!("Image {i}:\n\n![{i}]({base}/{dir}/{i}.{ext})\n\n"))
            .collect()
    };
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

/// Every poll: the window paints, and takes what is ready.
static POLL: AtomicUsize = AtomicUsize::new(16);

/// How long a measurement may take before it fails.
const DEADLINE: Duration = Duration::from_secs(180);

/// Paint and take results every `POLL_MS` until `stop` says so.
fn run(v: &mut View, buf: &mut [u32], mut stop: impl FnMut(&View, bool) -> bool) {
    let every = Duration::from_millis(POLL.load(Ordering::Relaxed) as u64);
    loop {
        let now = Instant::now();
        v.paint(buf, SIZE.0 as usize, now);
        v.tick(now, Some(now));
        let got = v.poll_images(Instant::now());
        if stop(v, got) {
            return;
        }
        std::thread::sleep(every);
    }
}

/// What a run has to have loaded: fetched all of `fetched`, decoded at least `decoded`, nothing
/// failed, nothing held waiting.
#[derive(Clone, Copy)]
struct Expect {
    fetched: usize,
    decoded: usize,
}

impl Expect {
    fn met(self, v: &View) -> bool {
        let s = v.image_stats();
        s.fetched == self.fetched && s.decoded >= self.decoded && s.failed == 0 && s.waiting == 0
    }
}

/// Poll until `expect` is met and nothing came for 0.5 s (and it still is), or fail.
fn drain(v: &mut View, buf: &mut [u32], expect: Expect) {
    let begun = Instant::now();
    let mut quiet = Instant::now();
    run(v, buf, |v, got| {
        if got {
            quiet = Instant::now();
        }
        // A failure is never what is wanted: no use waiting for the rest.
        if begun.elapsed() > DEADLINE || v.image_stats().failed > 0 {
            let s = v.image_stats();
            eprintln!(
                "FAILED: after {:.1} s {s:?}, wanted {} fetched and {} decoded, none failed",
                begun.elapsed().as_secs_f64(),
                expect.fetched,
                expect.decoded
            );
            std::process::exit(1);
        }
        quiet.elapsed() > Duration::from_millis(500) && expect.met(v)
    });
}

fn main() {
    platform::tune_allocator();
    let mut args = std::env::args().skip(1);
    let base = args.next().expect("BASE url");
    let n = args
        .next()
        .map(|n| n.parse::<usize>().expect("N"))
        .filter(|&n| n > 0);
    let mode = args.next().unwrap_or_else(|| "big".into());
    if let Some(ms) = args.next() {
        POLL.store(ms.parse().expect("POLL_MS"), Ordering::Relaxed);
    }
    let (w, h) = SIZE;
    let fonts = Fonts::new().unwrap();
    let (first, expect) = match mode.as_str() {
        "big" => {
            let n = n.unwrap_or(16);
            let expect = Expect {
                fetched: n,
                decoded: n.min(5),
            };
            (doc(&base, "big", "png", n, false), expect)
        }
        "tiny" => {
            let n = n.unwrap_or(12_288);
            let expect = Expect {
                fetched: n,
                decoded: n.min(5),
            };
            (doc(&base, "tiny", "png", n, false), expect)
        }
        "inline" => {
            let n = n.unwrap_or(50);
            let expect = Expect {
                fetched: n,
                decoded: n,
            };
            (doc(&base, "big", "png", n, true), expect)
        }
        "stripped" => {
            let n = n.unwrap_or(16_384);
            let expect = Expect {
                fetched: n,
                decoded: 0,
            };
            (doc(&base, "stripped", "jpg", n, false), expect)
        }
        "grow" => {
            let n = n.unwrap_or(8);
            let expect = Expect {
                fetched: n,
                decoded: n.min(5),
            };
            (doc(&base, "big", "png", n, false), expect)
        }
        _ => panic!("MODE: big, tiny, inline, stripped or grow"),
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
    drain(&mut v, &mut buf, expect);
    report(&format!(
        "{mode}: all {} fetched, {} decoded, in {:.1} s, polled every {} ms",
        v.image_stats().fetched,
        v.image_stats().decoded,
        start.elapsed().as_secs_f64(),
        POLL.load(Ordering::Relaxed)
    ));
    if mode == "grow" {
        // The bodies are cached; the new document wants others, which don't finish.
        let (live, rss) = (LIVE.load(Ordering::Relaxed), platform::rss_kb());
        println!("cached: {live} bytes live, RSS {rss} KB");
        PEAK.store(live, Ordering::Relaxed);
        v.open(doc(&base, "stall", "png", 4, false), None);
        let t = Instant::now();
        run(&mut v, &mut buf, |_, _| {
            t.elapsed() > Duration::from_secs(4)
        });
        println!(
            "unfinished fetches: {} bytes live",
            LIVE.load(Ordering::Relaxed)
        );
        report("grow");
    }
    // For `cargo xtask budgets`: only a run that loaded what it was to gets here.
    println!("image-rss peak_rss_kb={}", platform::peak_rss_kb());
}
