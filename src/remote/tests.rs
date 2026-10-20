use super::*;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn have_curl() -> bool {
    Command::new("curl")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A 2 × 1 PNG.
fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 2, 1);
    enc.set_color(png::ColorType::Rgba);
    enc.write_header()
        .unwrap()
        .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
        .unwrap();
    out
}

/// `fixtures/images/wide.jpg` and the same with 6 `APP1` segments of 65,000 bytes after its
/// frame header: more metadata than a decode keeps.
fn wide_jpeg() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/images/wide.jpg"
    ))
    .unwrap()
}

fn jpeg_with_exif() -> Vec<u8> {
    let src = wide_jpeg();
    let sos = src.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let mut out = src[..sos].to_vec();
    for _ in 0..6 {
        out.extend([0xFF, 0xE1]);
        out.extend(65_002u16.to_be_bytes());
        out.extend(b"Exif\0\0");
        out.resize(out.len() + 64_994, 0x55);
    }
    out.extend(&src[sos..]);
    out
}

/// `fixtures/images/gray.jpg` (178 bytes) with 5 `APP1` segments of 65,000 bytes before its scan:
/// 325 KB of which all but 178 bytes go.
fn tiny_jpeg_with_exif() -> Vec<u8> {
    let src = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/images/gray.jpg"
    ))
    .unwrap();
    let sos = src.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let mut out = src[..sos].to_vec();
    for _ in 0..5 {
        out.extend([0xFF, 0xE1]);
        out.extend(65_002u16.to_be_bytes());
        out.extend(b"Exif\0\0");
        out.resize(out.len() + 64_994, 0x55);
    }
    out.extend(&src[sos..]);
    out
}

fn respond(mut s: TcpStream, status: &str, headers: &str, body: &[u8]) {
    let _ = write!(s, "HTTP/1.1 {status}\r\n{headers}Connection: close\r\n\r\n");
    let _ = s.write_all(body);
}

/// What `/slow` waits for.
static SLOW_GATE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Requests for the glob test's paths.
static GLOB_HITS: AtomicUsize = AtomicUsize::new(0);

/// Serve one request on `s`.
fn serve(s: TcpStream, port: u16) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    // Skip the headers.
    let mut h = String::new();
    while r.read_line(&mut h).is_ok_and(|n| n > 2) {
        h.clear();
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let png = png();
    let len = |n: usize| format!("Content-Length: {n}\r\n");
    match path.as_str() {
        "/ok.png" | "/-o" => respond(s, "200 OK", &len(png.len()), &png),
        p if p.starts_with("/r/") => {
            // `/r/N` redirects N times before the image.
            let n: u32 = p[3..].parse().unwrap_or(0);
            if n == 0 {
                respond(s, "200 OK", &len(png.len()), &png);
            } else {
                let to = format!("Location: http://127.0.0.1:{port}/r/{}\r\n", n - 1);
                respond(s, "302 Found", &(to + &len(0)), b"");
            }
        }
        "/to-file" => respond(
            s,
            "302 Found",
            "Location: file:///etc/hostname\r\nContent-Length: 0\r\n",
            b"",
        ),
        "/to-ftp" => respond(
            s,
            "302 Found",
            &format!("Location: ftp://127.0.0.1:{port}/x.png\r\nContent-Length: 0\r\n"),
            b"",
        ),
        // Says how big it is: curl refuses it before reading.
        "/big-declared" => respond(s, "200 OK", &len(4096), &[0u8; 4096]),
        // Doesn't say (the connection's end ends it): the pipe is cut while reading.
        "/big-stream" => respond(s, "200 OK", "", &[0u8; 4096]),
        "/slow" => {
            // Held until the test says (or half a minute, so the thread ends): a fetch that
            // gets the image from it was not cut by its time cap.
            let (open, cv) = &SLOW_GATE;
            let held = open.lock().unwrap();
            let _ = cv.wait_timeout_while(held, Duration::from_secs(30), |o| !*o);
            respond(s, "200 OK", &len(png.len()), &png);
        }
        p if p.starts_with("/glob") => {
            GLOB_HITS.fetch_add(1, Ordering::SeqCst);
            respond(s, "404 Not Found", &len(0), b"");
        }
        // A PNG with 4 MiB after it (the image's header is all that is looked at).
        p if p.starts_with("/big/") => {
            let mut body = png.clone();
            body.resize(png.len() + (4 << 20), 0);
            respond(s, "200 OK", &len(body.len()), &body);
        }
        // `wide.jpg` with 390 KB of EXIF between its frame header and its scan.
        "/meta.jpg" => {
            let body = jpeg_with_exif();
            respond(s, "200 OK", &len(body.len()), &body);
        }
        "/tiny-meta.jpg" => {
            let body = tiny_jpeg_with_exif();
            respond(s, "200 OK", &len(body.len()), &body);
        }
        "/page.html" => respond(s, "200 OK", &len(6), b"<html>"),
        _ => respond(s, "404 Not Found", &len(0), b""),
    }
}

/// A server on 127.0.0.1 answering every request in its own thread; its port.
fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || serve(s, port));
        }
    });
    port
}

const SMALL: Limits = Limits {
    bytes: 1024,
    secs: 2,
    redirects: 5,
};

/// [`SMALL`] with a time cap a loaded machine won't reach: for a local server that answers.
const ROOMY: Limits = Limits { secs: 120, ..SMALL };

/// [`LIMITS`] with the time cap of [`ROOMY`].
const ROOMY_BIG: Limits = Limits {
    secs: 120,
    ..LIMITS
};

#[test]
fn only_http_urls_are_handed_to_curl() {
    assert!(fetchable("https://x.y/a.png"));
    assert!(fetchable("HTTP://x.y/a.png"));
    for url in [
        "-o/tmp/x",
        "--config=/etc/passwd",
        "-K/etc/passwd",
        "file:///etc/passwd",
        "ftp://x.y/a.png",
        "x.png",
        "https://x.y/a b.png",
        "https://x.y/\na.png",
        "",
    ] {
        assert!(!fetchable(url), "{url:?}");
        assert_eq!(
            fetch_with("/nonexistent/curl", url, SMALL, &|_| true),
            Err(Error::Refused)
        );
    }
}

/// The size cap holds while reading the pipe, whatever the program does: a `curl` that
/// writes forever is stopped.
#[cfg(unix)]
#[test]
fn reading_stops_at_the_size_cap() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("vomd-remote-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("curl");
    std::fs::write(&fake, "#!/bin/sh\nexec yes\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // (A script just written may be busy to exec while another test's thread forks, `ETXTBSY`:
    // that spawn fails at once, and is tried again. A `yes` that is never stopped would not end.)
    let got = (0..50)
        .map(|_| {
            fetch_with(fake.to_str().unwrap(), "https://x.y/a.png", SMALL, &|_| {
                true
            })
        })
        .find(|got| *got != Err(Error::Failed));
    assert_eq!(got, Some(Err(Error::TooLarge)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_curl_is_told_apart() {
    assert_eq!(
        fetch_with(
            "/nonexistent/vomd-curl",
            "https://x.y/a.png",
            SMALL,
            &|_| true
        ),
        Err(Error::NoCurl)
    );
}

#[test]
fn fetching_follows_the_rules() {
    if !have_curl() {
        eprintln!("curl not installed: skipped");
        return;
    }
    let port = server();
    let url = |p: &str| format!("http://127.0.0.1:{port}{p}");
    assert_eq!(fetch(&url("/ok.png"), ROOMY), Ok(png()));
    // A path that looks like an option stays a URL (it comes after `--`).
    assert_eq!(fetch(&url("/-o"), ROOMY), Ok(png()));
    // At most 5 redirects.
    assert_eq!(fetch(&url("/r/5"), ROOMY), Ok(png()));
    assert_eq!(fetch(&url("/r/6"), ROOMY), Err(Error::Failed));
    // Redirects only to http(s).
    assert_eq!(fetch(&url("/to-file"), ROOMY), Err(Error::Failed));
    assert_eq!(fetch(&url("/to-ftp"), ROOMY), Err(Error::Failed));
    // The size cap, declared or not.
    assert_eq!(fetch(&url("/big-declared"), ROOMY), Err(Error::Failed));
    // (Recent curl cuts an undeclared size off itself; the pipe is cut too, see below.)
    let streamed = fetch(&url("/big-stream"), ROOMY);
    assert!(
        matches!(streamed, Err(Error::Failed | Error::TooLarge)),
        "{streamed:?}"
    );
    // HTTP errors fail.
    assert_eq!(fetch(&url("/missing"), ROOMY), Err(Error::Failed));
    // The time cap.
    // (A server that doesn't answer until it is told to: the fetch ends by its time cap, and
    // never has the image.)
    assert_eq!(fetch(&url("/slow"), SMALL), Err(Error::Failed));
    *SLOW_GATE.0.lock().unwrap() = true;
    SLOW_GATE.1.notify_all();
    // Only images are kept.
    assert!(fetch(&url("/page.html"), ROOMY).is_ok());
    assert_eq!(
        fetch_image(&url("/page.html"), &|_| true),
        Err(Error::NotAnImage)
    );
    let (bytes, w, h) = fetch_image(&url("/ok.png"), &|_| true).unwrap();
    assert_eq!((&bytes[..], w, h), (&png()[..], 2, 1));
}

/// What a body costs is what it is allocated, and a small one is not a 256 KB buffer: the
/// credit taken is the buffer's size at each step, and the body is cut to size when it is whole
/// (a 1,333-byte image kept a 262,144-byte buffer, uncounted, before: 512 of them were 134 MB).
#[test]
fn a_small_body_keeps_only_what_it_needs() {
    if !have_curl() {
        eprintln!("curl not installed: skipped");
        return;
    }
    let port = server();
    let taken = AtomicUsize::new(0);
    let bytes = fetch_with(
        "curl",
        &format!("http://127.0.0.1:{port}/ok.png"),
        ROOMY_BIG,
        &|n| {
            taken.fetch_add(n, Ordering::SeqCst);
            true
        },
    )
    .map(cut_to_size)
    .unwrap();
    assert_eq!(bytes, png());
    assert_eq!(
        taken.load(Ordering::SeqCst),
        STEP,
        "credit for the one step it read"
    );
    assert!(bytes.capacity() < 1024, "{} allocated", bytes.capacity());
    // A bigger body's credit is its whole buffer, as allocated, in steps.
    let taken = AtomicUsize::new(0);
    let big = fetch_with(
        "curl",
        &format!("http://127.0.0.1:{port}/big/0"),
        ROOMY_BIG,
        &|n| {
            taken.fetch_add(n, Ordering::SeqCst);
            true
        },
    )
    .unwrap();
    let want = big.len().div_ceil(STEP) * STEP;
    assert_eq!(taken.load(Ordering::SeqCst), want);
    assert!(big.capacity() <= want && big.capacity() >= big.len());
    // Not given credit, a fetch gives up: nothing is learned of the URL.
    let got = fetch_with(
        "curl",
        &format!("http://127.0.0.1:{port}/big/0"),
        ROOMY_BIG,
        &|_| false,
    );
    assert_eq!(got, Err(Error::Yielded));
}

/// A body is cut to the size it holds: a small one is copied (the buffer it was read into is a
/// mapping of its own, which the allocator would only shrink to a page), a big one cut in place.
#[test]
fn bodies_are_cut_to_size() {
    let mut small = Vec::with_capacity(STEP);
    small.extend([1u8; 69]);
    let small = cut_to_size(small);
    assert_eq!((small.len(), small.capacity()), (69, 69));
    let mut big = Vec::with_capacity(4 << 20);
    big.extend(vec![2u8; (3 << 20) + 5]);
    let big = cut_to_size(big);
    assert_eq!((big.len(), big.capacity()), ((3 << 20) + 5, (3 << 20) + 5));
    assert!(cut_to_size(Vec::new()).is_empty());
}

/// A fetched JPEG's metadata is cut out of its body where it is, so the cache doesn't keep it
/// and the decoder never sees it (the buffer is cut to size, its credit with it).
#[test]
fn a_fetched_jpegs_metadata_is_cut_out() {
    if !have_curl() {
        eprintln!("curl not installed: skipped");
        return;
    }
    let port = server();
    let plain = wide_jpeg();
    let (bytes, w, h) = fetch_image(&format!("http://127.0.0.1:{port}/meta.jpg"), &|_| true)
        .expect("a JPEG with metadata is an image");
    assert_eq!((w, h), (600, 200));
    assert_eq!(bytes.len(), plain.len(), "all of the 390 KB went");
    assert!(
        bytes.capacity() < plain.len() + 4096,
        "{} allocated for {} bytes",
        bytes.capacity(),
        bytes.len()
    );
    assert_eq!(
        image::decode(&bytes, 60, 20),
        image::decode(&plain, 60, 20),
        "decodes as it does without the metadata"
    );
}

/// A JPEG over 256 KB that is tiny without its metadata is copied to a buffer of its own size
/// *after* the metadata is cut out, not before: cut to size while it was 325 KB (a mapping of its
/// own) and shrunk in place after, it stays a mapping of a page at least, for every URL (16,384
/// of them were 70 MB resident, by 4 KB each, that nothing counted).
#[test]
fn a_jpeg_that_is_tiny_without_its_metadata_is_cut_to_size_after() {
    if !have_curl() {
        eprintln!("curl not installed: skipped");
        return;
    }
    let port = server();
    let url = format!("http://127.0.0.1:{port}/tiny-meta.jpg");
    // What is read is as it comes, with its credit's buffer: nothing is cut yet.
    let raw = fetch_with("curl", &url, ROOMY_BIG, &|_| true).unwrap();
    assert_eq!(raw.len(), tiny_jpeg_with_exif().len());
    assert!(raw.capacity() >= raw.len());
    let (bytes, w, h) = fetch_image(&url, &|_| true).expect("an image");
    assert!(w > 0 && h > 0);
    assert!(bytes.len() < 1024, "{} bytes kept", bytes.len());
    assert_eq!(
        bytes.capacity(),
        bytes.len(),
        "a buffer of its size, not what it was"
    );
    assert!(image::decode(&bytes, 8, 8).is_some());
}

#[test]
fn what_fetching_found_decides_the_layout() {
    let mut r = Remote::default();
    let url: Arc<str> = "https://x.y/a.png".into();
    assert_eq!(r.layout_size(&url), ImageSize::Alt, "off");
    r.on = true;
    assert_eq!(r.layout_size(&url), ImageSize::Loading);
    let bytes: Bytes = Arc::new(png());
    assert!(r.fetched(&url, Ok((bytes, 2, 1))).is_some());
    assert_eq!(r.layout_size(&url), ImageSize::Pixels(2, 1));
    r.failed(&url, true);
    assert_eq!(r.layout_size(&url), ImageSize::Alt);
    let other: Arc<str> = "https://x.y/b.png".into();
    assert!(r.fetched(&other, Err(Error::NoCurl)).is_none());
    assert!(r.no_curl);
    assert_eq!(r.layout_size(&other), ImageSize::Alt, "nothing can load");
    // Off, nothing is fetched (and no fetcher started).
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut r = Remote::default();
    r.fetch(vec![url.clone()], tx, Arc::new(|| {}));
    assert!(!r.started());
}

/// A URL with `[1-N]` or `{a,b}` is one URL (no `--globoff`, and one image would make `curl`
/// request a million).
#[test]
fn glob_urls_are_fetched_literally() {
    if !have_curl() {
        eprintln!("curl not installed: skipped");
        return;
    }
    let port = server();
    let before = GLOB_HITS.load(Ordering::SeqCst);
    for path in ["/glob[1-5].png", "/glob{a,b,c}.png"] {
        let _ = fetch(&format!("http://127.0.0.1:{port}{path}"), ROOMY);
    }
    // (The server answers them all 404: what counts is how many it saw.)
    let hits = GLOB_HITS.load(Ordering::SeqCst) - before;
    assert_eq!(hits, 2, "requests for 2 URLs");
}

/// A `curl` still running well past its `--max-time` is killed, so a fetcher is never stuck.
#[cfg(unix)]
#[test]
fn a_curl_that_overstays_is_killed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("vomd-remote-stuck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("curl");
    // Ignores its arguments, writes nothing and doesn't stop.
    std::fs::write(&fake, "#!/bin/sh\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let limits = Limits { secs: 1, ..SMALL };
    // A script just written may be busy to exec while another test's thread forks (`ETXTBSY`):
    // such a spawn fails at once, and is tried again.
    let (got, took) = (0..50)
        .find_map(|_| {
            let t = Instant::now();
            let got = fetch_with(fake.to_str().unwrap(), "https://x.y/a.png", limits, &|_| {
                true
            });
            (t.elapsed() > Duration::from_millis(500)).then(|| (got, t.elapsed()))
        })
        .expect("a curl that was run");
    assert_eq!(got, Err(Error::Failed));
    // Killed after its time and the grace (a 60 s sleep would show), however slow the machine.
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(45),
        "{took:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fetcher standing in for `curl`: records how many run at once and which URLs it was asked
/// for, and holds each until the gate is opened.
#[derive(Default)]
struct Probe {
    open: Mutex<bool>,
    cv: Condvar,
    now: AtomicUsize,
    peak: AtomicUsize,
    log: Mutex<Vec<String>>,
}

impl Probe {
    fn fetcher(self: &Arc<Self>) -> Fetcher {
        let p = self.clone();
        Arc::new(move |url: &str, _grant: Grant| {
            p.log.lock().unwrap().push(url.to_owned());
            let now = p.now.fetch_add(1, Ordering::SeqCst) + 1;
            p.peak.fetch_max(now, Ordering::SeqCst);
            let mut open = p.open.lock().unwrap();
            while !*open {
                open = p.cv.wait(open).unwrap();
            }
            p.now.fetch_sub(1, Ordering::SeqCst);
            Ok((Arc::new(png()), 2, 1))
        })
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }

    fn started(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn urls(n: usize) -> Vec<Arc<str>> {
    (0..n)
        .map(|i| format!("https://x.y/{i}.png").into())
        .collect()
}

/// Held off, the fetchers start nothing (a pool started while held off included), a fetch that
/// needs more credit gives up and is queued again at the front, and one that has what it needs
/// is waited for; let go, everything is fetched, once each but the one that gave up.
#[test]
fn held_off_fetchers_start_nothing_and_give_up_what_wants_more() {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake: Wake = Arc::new(|| {});
    let starts = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let fetcher: Fetcher = {
        let (starts, gate) = (starts.clone(), gate.clone());
        Arc::new(move |url, grant| {
            starts.fetch_add(1, Ordering::SeqCst);
            // The first URL takes credit, waits at the gate, and asks for more.
            if url.ends_with("/0.png") {
                assert!(grant(STEP) || starts.load(Ordering::SeqCst) > 1);
                let (open, cv) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                if !grant(STEP) {
                    return Err(Error::Yielded);
                }
            }
            Ok((Arc::new(png()), 2, 1))
        })
    };
    let mut r = Remote {
        on: true,
        fetcher,
        ..Remote::default()
    };
    // The pool is started while held off: nothing starts.
    r.pause(true);
    r.fetch(urls(5), tx.clone(), wake.clone());
    assert!(r.paused() && r.busy());
    assert_eq!(r.transfers(), (0, 0));
    // Let go: the first URL starts and holds credit at the gate; held off again, it asks for more
    // when the gate opens, and gives up: that URL is queued again, nothing else starts.
    r.pause(false);
    until("the first fetch holds credit", || r.inflight() >= STEP);
    r.pause(true);
    let before = starts.load(Ordering::SeqCst);
    {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }
    until("the fetch gave up", || r.transfers().0 == 0);
    // Whatever the other threads did before they were held off is over: none starts now.
    assert!(r.busy(), "its URL is queued again");
    let started = starts.load(Ordering::SeqCst);
    assert!(started >= before);
    // Let go: all five are fetched.
    r.pause(false);
    let mut got = 0;
    while got < 5 {
        let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(60)) else {
            panic!("a result");
        };
        assert!(r.fetched(&url, result).is_some());
        got += 1;
    }
    until("idle", || !r.busy());
}

/// A fetch that gave up for the pause is run again and is not given up for it again (round 14):
/// each pause of the fetchers made the transfers that had not taken their first credit give up,
/// and a request that replaced the queue forgot them, so a URL could be fetched as many times as
/// the fetchers were held off (a viewport of 700 one-pixel images beside a bitmap that takes the
/// budget: 346 runs more than there were URLs). The second run finishes under a pause, and the
/// request that replaces the queue meanwhile keeps its place.
#[test]
fn a_fetch_given_up_for_the_pause_finishes_the_second_time() {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake: Wake = Arc::new(|| {});
    let starts = Arc::new(AtomicUsize::new(0));
    // The attempt that may go past the gate (each takes its first credit, waits there, and asks
    // for more).
    let gate = Arc::new((Mutex::new(0usize), Condvar::new()));
    let fetcher: Fetcher = {
        let (starts, gate) = (starts.clone(), gate.clone());
        Arc::new(move |_, grant| {
            let n = starts.fetch_add(1, Ordering::SeqCst) + 1;
            assert!(grant(STEP));
            let (open, cv) = &*gate;
            let mut open = open.lock().unwrap();
            while *open < n {
                open = cv.wait(open).unwrap();
            }
            drop(open);
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
            Ok((Arc::new(png()), 2, 1))
        })
    };
    let open = |n: usize| {
        *gate.0.lock().unwrap() = n;
        gate.1.notify_all();
    };
    let mut r = Remote {
        on: true,
        fetcher,
        ..Remote::default()
    };
    let url = urls(1);
    r.fetch(url.clone(), tx.clone(), wake.clone());
    until("the first run holds credit", || r.inflight() >= STEP);
    // Held off, it asks for more and gives up.
    r.pause(true);
    open(1);
    until("the first run gave up", || r.transfers().0 == 0);
    assert!(r.busy(), "it is next in line");
    // A request replaces the queue (the URL is still wanted): it is not forgotten, nor queued twice.
    r.fetch(url.clone(), tx.clone(), wake.clone());
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "nothing starts while held off"
    );
    r.pause(false);
    until("the second run holds credit", || {
        starts.load(Ordering::SeqCst) == 2 && r.inflight() >= STEP
    });
    // Held off again: it goes on, and finishes.
    r.pause(true);
    open(2);
    let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(60)) else {
        panic!("a result");
    };
    assert!(r.fetched(&url, result).is_some(), "the second run finished");
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    until("idle", || !r.busy());
}

/// At most [`FETCHERS`] fetches run at once and none runs twice, however the queue is replaced
/// and however often fetching is stopped and started again meanwhile; what finished is told
/// apart from what is still being fetched until the main thread takes it.
#[test]
fn the_fetchers_never_exceed_their_number_or_repeat_a_url() {
    let probe = Arc::new(Probe::default());
    let woken = Arc::new(AtomicUsize::new(0));
    let wake: Wake = {
        let woken = woken.clone();
        Arc::new(move || {
            woken.fetch_add(1, Ordering::SeqCst);
        })
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut r = Remote {
        on: true,
        fetcher: probe.fetcher(),
        ..Remote::default()
    };
    let all = urls(10);
    r.fetch(all.clone(), tx.clone(), wake.clone());
    until("4 fetches started", || probe.started() == FETCHERS);
    // The queue is replaced: what was waiting is gone, what is running is not asked for again.
    let later = urls(14)[10..].to_vec();
    r.fetch([&all[..2], &later[..]].concat(), tx.clone(), wake.clone());
    // Off and on again, three times, with the same URLs.
    for _ in 0..3 {
        r.stop();
        r.fetch(all.clone(), tx.clone(), wake.clone());
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(probe.started(), FETCHERS, "{:?}", probe.log.lock().unwrap());
    assert_eq!(probe.peak.load(Ordering::SeqCst), FETCHERS);
    // Stopped for good: what is queued is dropped, the running four finish and are kept.
    r.stop();
    probe.release();
    let mut got: Vec<Arc<str>> = Vec::new();
    while got.len() < FETCHERS {
        let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(60)) else {
            panic!("a fetch result");
        };
        assert!(result.is_ok());
        // Finished but not taken: asking for it again doesn't fetch it again.
        r.fetch(vec![url.clone()], tx.clone(), wake.clone());
        got.push(url);
    }
    // (The wake follows each result: a result is seen before its wake is called.)
    until("every result woke the window", || {
        woken.load(Ordering::SeqCst) == FETCHERS
    });
    assert_eq!(probe.started(), FETCHERS, "nothing fetched twice");
    // Taken, they can be fetched again (an evicted image); the rest of the queue goes on.
    for url in &got {
        assert!(r.fetched(url, Ok((Arc::new(png()), 2, 1))).is_some());
    }
    r.fetch(all[FETCHERS..].to_vec(), tx.clone(), wake.clone());
    let mut seen = got.len();
    while seen < 10 {
        let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(60)) else {
            panic!("a fetch result");
        };
        r.fetched(&url, result);
        seen += 1;
    }
    let mut log = probe.log.lock().unwrap().clone();
    log.sort();
    log.dedup();
    assert_eq!(log.len(), probe.started(), "no URL twice");
    assert_eq!(probe.started(), 10);
    assert_eq!(probe.peak.load(Ordering::SeqCst), FETCHERS);
}

/// Bodies in memory are bounded: fetchers whose results nobody takes stop at [`FETCH_BYTES`]
/// (a fetch that would go past it waits, none starts), whatever the number of URLs, and never
/// go past it; taking results lets the rest through, and every URL is fetched. (It was 16
/// bodies of 4 MiB held at once in a channel, 99 MB, before anything was decoded.)
#[test]
fn bodies_waiting_for_the_main_thread_are_bounded() {
    const BODY: usize = 4 << 20;
    let started = Arc::new(Mutex::new(Vec::<String>::new()));
    let fetcher: Fetcher = {
        let started = started.clone();
        Arc::new(move |url: &str, grant: Grant| {
            started.lock().unwrap().push(url.to_owned());
            for _ in 0..BODY / STEP {
                if !grant(STEP) {
                    return Err(Error::Yielded);
                }
            }
            Ok((Arc::new(vec![0u8; BODY]), 2, 1))
        })
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut r = Remote {
        on: true,
        fetcher,
        ..Remote::default()
    };
    let all = urls(16);
    r.fetch(all.clone(), tx.clone(), Arc::new(|| {}));
    // Nothing is taken: the 12 MiB are spent by finished bodies (3 at most), and whatever else
    // is reading waits for credit or has given it up (to be fetched again once there is some).
    // (How many are finished by the time this looks depends on how the four interleaved.)
    until("a body is finished", || r.transfers().1 >= 1);
    let mut waiting = vec![rx.recv_timeout(Duration::from_secs(60)).expect("a result")];
    waiting.extend(rx.try_iter());
    let held = r.inflight();
    assert!(held <= FETCH_BYTES, "{held} held");
    assert!(
        (1..=3).contains(&waiting.len()),
        "{} bodies waiting",
        waiting.len()
    );
    assert!(r.busy());
    // Taking results frees credit, a fetch at a time.
    let mut taken = Vec::new();
    while taken.len() < 16 {
        let done = match waiting.pop() {
            Some(done) => done,
            None => rx.recv_timeout(Duration::from_secs(60)).expect("a result"),
        };
        let Done::Fetched { url, result } = done else {
            panic!("a fetch result");
        };
        assert!(r.inflight() <= FETCH_BYTES, "{} held", r.inflight());
        assert!(r.fetched(&url, result).is_some());
        taken.push(url);
        std::thread::sleep(Duration::from_millis(5));
        assert!(r.inflight() <= FETCH_BYTES, "{} held", r.inflight());
    }
    taken.sort();
    taken.dedup();
    assert_eq!(taken.len(), 16, "every URL, once");
    assert_eq!(r.inflight(), 0);
    assert!(!r.busy());
}

/// Fetchers that all wait for credit while none holds a finished body can't deadlock: the
/// youngest gives up (and is fetched again) until the oldest can finish, the rest follow, and
/// the credit is never exceeded, not even by the one that goes on. (It was let go on past the
/// budget, by up to 20 MiB, before.)
#[test]
fn fetchers_waiting_on_each_other_are_let_through() {
    // Bodies of 7 MiB: four read at once, none done at 12 MiB.
    const BODY: usize = 7 << 20;
    let gave_up = Arc::new(AtomicUsize::new(0));
    let fetcher: Fetcher = {
        let gave_up = gave_up.clone();
        Arc::new(move |_: &str, grant: Grant| {
            for _ in 0..BODY / STEP {
                if !grant(STEP) {
                    gave_up.fetch_add(1, Ordering::SeqCst);
                    return Err(Error::Yielded);
                }
                // Reading takes time: the others get to ask too.
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok((Arc::new(vec![0u8; BODY]), 2, 1))
        })
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut r = Remote {
        on: true,
        fetcher,
        ..Remote::default()
    };
    r.fetch(urls(6), tx.clone(), Arc::new(|| {}));
    let mut got = std::collections::HashSet::new();
    while got.len() < 6 {
        let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(60)) else {
            panic!("deadlocked after {} of 6", got.len());
        };
        assert!(r.inflight() <= FETCH_BYTES, "{} held", r.inflight());
        assert!(result.is_ok(), "{result:?}");
        assert!(r.fetched(&url, result).is_some());
        assert!(got.insert(url), "once");
    }
    assert!(
        gave_up.load(Ordering::SeqCst) > 0,
        "the fetchers did wait on each other"
    );
}

/// A fetch that gave up when fetching was stopped meanwhile isn't started again.
#[test]
fn a_fetch_that_gives_up_after_stopping_is_not_queued_again() {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let fetcher: Fetcher = {
        let (log, gate) = (log.clone(), gate.clone());
        Arc::new(move |url: &str, _: Grant| {
            log.lock().unwrap().push(url.to_owned());
            let mut open = gate.0.lock().unwrap();
            while !*open {
                open = gate.1.wait(open).unwrap();
            }
            Err(Error::Yielded)
        })
    };
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut r = Remote {
        on: true,
        fetcher,
        ..Remote::default()
    };
    r.fetch(urls(1), tx.clone(), Arc::new(|| {}));
    until("started", || log.lock().unwrap().len() == 1);
    r.stop();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    // It gave up, and is not queued again: the pool has nothing (it would have started it again).
    until("nothing is queued or running", || !r.busy());
    assert_eq!(log.lock().unwrap().len(), 1);
}

/// The same with a real `curl` and bodies of 4 MiB from a local server: 16 URLs, results not
/// taken, hold no more than the budget.
#[test]
fn real_bodies_stop_at_the_budget() {
    const BODY: usize = 4 << 20;
    if !have_curl() {
        eprintln!("curl not installed: skipped");
        return;
    }
    let port = server();
    let (tx, rx) = std::sync::mpsc::channel();
    // (With the time cap of [`ROOMY_BIG`]: 15 s is too short when the CPUs are busy.)
    let mut r = Remote {
        on: true,
        fetcher: Arc::new(|url, grant| image_of(fetch_with("curl", url, ROOMY_BIG, grant)?)),
        ..Remote::default()
    };
    let all: Vec<Arc<str>> = (0..16)
        .map(|i| format!("http://127.0.0.1:{port}/big/{i}").into())
        .collect();
    r.fetch(all, tx.clone(), Arc::new(|| {}));
    // Bodies finish as credit allows (4.25 MiB each, at most two of the 12 MiB, and no more while
    // they wait): the first to be done waits for the main thread, with whatever else is done by
    // then, never more than three.
    until("a body is finished", || r.transfers().1 >= 1);
    let mut waiting = vec![rx.recv_timeout(Duration::from_secs(60)).expect("a result")];
    waiting.extend(rx.try_iter());
    assert!(r.inflight() <= FETCH_BYTES, "{} held", r.inflight());
    assert!(
        (1..=3).contains(&waiting.len()),
        "{} waiting",
        waiting.len()
    );
    // Taking them lets the rest through, all 16 once, within the budget throughout.
    let mut taken = 0;
    while taken < 16 {
        let done = match waiting.pop() {
            Some(done) => done,
            None => rx.recv_timeout(Duration::from_secs(60)).expect("a result"),
        };
        let Done::Fetched { url, result } = done else {
            panic!("a fetch result");
        };
        assert!(r.inflight() <= FETCH_BYTES, "{} held", r.inflight());
        assert!(result.is_ok(), "{url}: {result:?}");
        let bytes = r.fetched(&url, result).unwrap();
        assert!(bytes.len() > BODY && bytes.capacity() <= BODY + STEP);
        taken += 1;
    }
    assert_eq!(r.inflight(), 0);
}

/// Results nobody has taken are bounded in number too, not only in bytes: tiny bodies and
/// failures hold no credit to speak of, so without a limit of their own the pool would run
/// through every URL, keeping a record and a message for each. Taking them lets the rest
/// through.
#[test]
fn results_waiting_for_the_main_thread_are_bounded_in_number() {
    let fetcher: Fetcher = Arc::new(|url: &str, grant: Grant| {
        if url.ends_with('7') {
            return Err(Error::Failed);
        }
        assert!(grant(STEP));
        Ok((Arc::new(vec![0u8; 8]), 2, 1))
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let mut r = Remote {
        on: true,
        fetcher,
        ..Remote::default()
    };
    let all = urls(2000);
    r.fetch(all.clone(), tx.clone(), Arc::new(|| {}));
    // No fetch starts while `PENDING_MAX` results wait: the ones being read finish, and that is
    // all (each result is in the channel when `transfers` counts it, within a moment).
    until("the results wait", || {
        let (reading, untaken) = r.transfers();
        reading == 0 && untaken >= PENDING_MAX
    });
    let n = r.transfers().1;
    let mut waiting: Vec<_> = (0..n)
        .map(|_| rx.recv_timeout(Duration::from_secs(60)).expect("a result"))
        .collect();
    assert!(
        (PENDING_MAX..PENDING_MAX + FETCHERS).contains(&waiting.len()),
        "{} results waiting",
        waiting.len()
    );
    let mut taken = std::collections::HashSet::new();
    while taken.len() < all.len() {
        let done = match waiting.pop() {
            Some(done) => done,
            None => rx.recv_timeout(Duration::from_secs(60)).expect("a result"),
        };
        let Done::Fetched { url, result } = done else {
            panic!("a fetch result");
        };
        r.fetched(&url, result);
        assert!(taken.insert(url), "once");
    }
    assert!(!r.busy());
}
