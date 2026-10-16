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

fn respond(mut s: TcpStream, status: &str, headers: &str, body: &[u8]) {
    let _ = write!(s, "HTTP/1.1 {status}\r\n{headers}Connection: close\r\n\r\n");
    let _ = s.write_all(body);
}

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
            std::thread::sleep(Duration::from_secs(4));
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
            fetch_with("/nonexistent/curl", url, SMALL, &|_| {}),
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
    let t = Instant::now();
    let got = fetch_with(fake.to_str().unwrap(), "https://x.y/a.png", SMALL, &|_| {});
    assert_eq!(got, Err(Error::TooLarge));
    assert!(t.elapsed() < Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_curl_is_told_apart() {
    assert_eq!(
        fetch_with(
            "/nonexistent/vomd-curl",
            "https://x.y/a.png",
            SMALL,
            &|_| {}
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
    assert_eq!(fetch(&url("/ok.png"), SMALL), Ok(png()));
    // A path that looks like an option stays a URL (it comes after `--`).
    assert_eq!(fetch(&url("/-o"), SMALL), Ok(png()));
    // At most 5 redirects.
    assert_eq!(fetch(&url("/r/5"), SMALL), Ok(png()));
    assert_eq!(fetch(&url("/r/6"), SMALL), Err(Error::Failed));
    // Redirects only to http(s).
    assert_eq!(fetch(&url("/to-file"), SMALL), Err(Error::Failed));
    assert_eq!(fetch(&url("/to-ftp"), SMALL), Err(Error::Failed));
    // The size cap, declared or not.
    assert_eq!(fetch(&url("/big-declared"), SMALL), Err(Error::Failed));
    // (Recent curl cuts an undeclared size off itself; the pipe is cut too, see below.)
    let streamed = fetch(&url("/big-stream"), SMALL);
    assert!(
        matches!(streamed, Err(Error::Failed | Error::TooLarge)),
        "{streamed:?}"
    );
    // HTTP errors fail.
    assert_eq!(fetch(&url("/missing"), SMALL), Err(Error::Failed));
    // The time cap.
    let t = Instant::now();
    assert_eq!(fetch(&url("/slow"), SMALL), Err(Error::Failed));
    assert!(
        t.elapsed() < Duration::from_millis(3500),
        "{:?}",
        t.elapsed()
    );
    // Only images are kept.
    assert!(fetch(&url("/page.html"), SMALL).is_ok());
    assert_eq!(
        fetch_image(&url("/page.html"), &|_| {}),
        Err(Error::NotAnImage)
    );
    let (bytes, w, h) = fetch_image(&url("/ok.png"), &|_| {}).unwrap();
    assert_eq!((&bytes[..], w, h), (&png()[..], 2, 1));
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
    r.failed(&url);
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
        let _ = fetch(&format!("http://127.0.0.1:{port}{path}"), SMALL);
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
    let t = Instant::now();
    let got = fetch_with(fake.to_str().unwrap(), "https://x.y/a.png", limits, &|_| {});
    assert_eq!(got, Err(Error::Failed));
    let took = t.elapsed();
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(8),
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
        assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn urls(n: usize) -> Vec<Arc<str>> {
    (0..n)
        .map(|i| format!("https://x.y/{i}.png").into())
        .collect()
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
        let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(10)) else {
            panic!("a fetch result");
        };
        assert!(result.is_ok());
        // Finished but not taken: asking for it again doesn't fetch it again.
        r.fetch(vec![url.clone()], tx.clone(), wake.clone());
        got.push(url);
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(probe.started(), FETCHERS, "nothing fetched twice");
    assert_eq!(woken.load(Ordering::SeqCst), FETCHERS);
    // Taken, they can be fetched again (an evicted image); the rest of the queue goes on.
    for url in &got {
        assert!(r.fetched(url, Ok((Arc::new(png()), 2, 1))).is_some());
    }
    r.fetch(all[FETCHERS..].to_vec(), tx.clone(), wake.clone());
    let mut seen = got.len();
    while seen < 10 {
        let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(10)) else {
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
/// (a fetch that would go past it waits, none starts), whatever the number of URLs; taking
/// results lets the rest through, and every URL is fetched once. (It was 16 bodies of 4 MiB
/// held at once in a channel, 99 MB, before anything was decoded.)
#[test]
fn bodies_waiting_for_the_main_thread_are_bounded() {
    const BODY: usize = 4 << 20;
    let started = Arc::new(AtomicUsize::new(0));
    let fetcher: Fetcher = {
        let started = started.clone();
        Arc::new(move |_: &str, grant: Grant| {
            started.fetch_add(1, Ordering::SeqCst);
            for _ in 0..BODY / STEP {
                grant(STEP);
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
    std::thread::sleep(Duration::from_millis(400));
    // Nothing is taken: the 12 MiB are spent, by finished bodies and by what the others had
    // read (or the oldest fetcher, let go on past the budget, took a step or two more).
    let (held, mut waiting) = (r.inflight(), rx.try_iter().collect::<Vec<_>>());
    assert!(
        (FETCH_BYTES..=FETCH_BYTES + BODY).contains(&held),
        "{held} held"
    );
    assert!(
        (1..=4).contains(&waiting.len()),
        "{} bodies waiting",
        waiting.len()
    );
    let n = started.load(Ordering::SeqCst);
    assert!((3..=4).contains(&n), "{n} fetches started");
    // Taking results frees credit, a fetch at a time.
    let mut taken = 0;
    let mut most = 0;
    while taken < 16 {
        let done = match waiting.pop() {
            Some(done) => done,
            None => rx.recv_timeout(Duration::from_secs(10)).expect("a result"),
        };
        let Done::Fetched { url, result } = done else {
            panic!("a fetch result");
        };
        most = most.max(r.inflight());
        assert!(r.fetched(&url, result).is_some());
        taken += 1;
        std::thread::sleep(Duration::from_millis(5));
        most = most.max(r.inflight());
    }
    assert!(most <= FETCH_BYTES + BODY, "{most} held at most");
    assert_eq!(started.load(Ordering::SeqCst), 16, "each URL once");
    assert_eq!(r.inflight(), 0);
}

/// Fetchers that all wait for credit while none holds a finished body can't deadlock: the
/// oldest goes on past the budget (by at most one body: 20 MiB), the rest follow.
#[test]
fn fetchers_waiting_on_each_other_are_let_through() {
    // Five fetchers' worth of bodies of 7 MiB: four read at once, none done at 12 MiB.
    const BODY: usize = 7 << 20;
    let fetcher: Fetcher = Arc::new(|_: &str, grant: Grant| {
        for _ in 0..BODY / STEP {
            grant(STEP);
        }
        Ok((Arc::new(vec![0u8; BODY]), 2, 1))
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let mut r = Remote {
        on: true,
        fetcher,
        ..Remote::default()
    };
    let all = urls(6);
    r.fetch(all, tx.clone(), Arc::new(|| {}));
    let mut got = 0;
    while got < 6 {
        let Ok(Done::Fetched { url, result }) = rx.recv_timeout(Duration::from_secs(10)) else {
            panic!("deadlocked after {got} of 6");
        };
        assert!(r.inflight() <= FETCH_BYTES + BODY, "{} held", r.inflight());
        assert!(r.fetched(&url, result).is_some());
        got += 1;
    }
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
    let mut r = Remote {
        on: true,
        ..Remote::default()
    };
    let all: Vec<Arc<str>> = (0..16)
        .map(|i| format!("http://127.0.0.1:{port}/big/{i}").into())
        .collect();
    r.fetch(all, tx.clone(), Arc::new(|| {}));
    std::thread::sleep(Duration::from_millis(1500));
    // (The oldest fetch still reading is let go on past it when nothing else can free credit,
    // by at most one body.)
    assert!(r.inflight() <= FETCH_BYTES + BODY, "{} held", r.inflight());
    let mut waiting: Vec<_> = rx.try_iter().collect();
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
            None => rx.recv_timeout(Duration::from_secs(20)).expect("a result"),
        };
        let Done::Fetched { url, result } = done else {
            panic!("a fetch result");
        };
        assert!(r.inflight() <= FETCH_BYTES + BODY, "{} held", r.inflight());
        assert!(result.is_ok(), "{url}: {result:?}");
        assert!(r.fetched(&url, result).is_some());
        taken += 1;
    }
    assert_eq!(r.inflight(), 0);
}
