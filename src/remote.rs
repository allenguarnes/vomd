//! Remote images: fetched only after the reader turns them on for the
//! document (`I`, the toolbar, or the `always` policy after the first frame), with the system
//! `curl`, so the binary carries no HTTP or TLS code.
//!
//! Each fetch is one `curl` process, its arguments an argv array (never a shell), the URL after
//! `--` so it can't be read as an option, `~/.curlrc` ignored (`-q`), no URL globbing
//! (`--globoff`: `x[1-1000000].png` is one URL), only http(s) allowed, also for redirects, with
//! caps on redirects, time and size. The size cap is enforced again while reading the pipe, and
//! the time cap by a watchdog that kills a `curl` still running well after it. The bytes are
//! sniffed: only images of a format this build decodes are kept.
//!
//! Bodies are read into memory, so what is held is bounded: a fetcher takes credit for every
//! 256 KB it reads (before its buffer grows by that much), and for as long as the body is its
//! own, or waits in a result the main thread hasn't taken, it counts against [`FETCH_BYTES`],
//! as allocated (the buffer is cut to size when the body is whole). A fetcher that would go over
//! it waits (`curl` blocked on its pipe) until the main thread takes a result, and no fetch
//! starts while it is reached. When all the fetchers wait and no result is waiting to be taken,
//! the youngest gives up its body and its credit (it is fetched again later, as if it had
//! never started), so the oldest can finish: a body is never bigger than [`FETCH_BYTES`], so it
//! always can once it is the only one. So the pool alone never holds more than [`FETCH_BYTES`],
//! with nothing to look at or sample: the image cache sets that much (and the fetchers' read
//! buffers, [`FETCH_RESERVE`]) aside as long as any fetch is queued or running
//! (`Cache::fetching`), and the bodies, the bytes kept and the pixels together stay within the
//! one budget at every moment, not only after the main thread has polled. A body that is taken
//! goes to the cache before its credit is let go (`Remote::release`: the body is never allocated
//! and uncounted at once), and only if the cache has room for it
//! (`Images::admission`): a body for an image in view that has none stays in the pool, its credit
//! held, until the decoder has made room, so the fetchers stop, instead of the bodies of every
//! image in view piling up, protected from eviction, while the decoder works through them. A
//! JPEG's metadata is cut out of its body where it is, before it is kept
//! (`image::shed_jpeg_metadata`), and the buffer is cut to size after that.
//!
//! A decode that doesn't fit beside that reserve (a 4000 × 2000 JPEG holds 24 MB while it runs)
//! has the fetchers held off ([`Remote::pause`]): nothing starts, nothing takes more credit (a
//! transfer that needs more gives up as it does when all wait, and is queued again at the front,
//! and is not given up for the pause again: a URL is run at most twice for the pause's sake),
//! and what has the credit it needs finishes, so that once the results are taken they hold
//! nothing, and the decode has the budget beside it; they go on when its result is taken.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::image::{self, Bytes, Done, Wake};
use crate::layout::{ImageSize, Refusal};
use crate::settings::RemoteImages;

/// Fetchers running at once, at most.
pub const FETCHERS: usize = 4;

/// Bytes of fetched bodies held outside the cache at once, at most, as allocated: the ones
/// being read and the ones fetched and not yet taken by the main thread. The largest body
/// ([`LIMITS`]) is as big, so a fetch alone always fits.
pub const FETCH_BYTES: usize = 12 << 20;

/// What a fetcher reads the pipe into: a buffer of its own, not part of the body.
const READ_BUF: usize = 64 << 10;

/// Results (bodies and failures) fetched and not yet taken by the main thread, at most: no fetch
/// starts while there are this many, so what the pool keeps per result besides the body is bounded.
const PENDING_MAX: usize = 256;

/// What a result costs besides its body, at most: its entry in the pool, the message in the
/// channel, the `Arc` of the body and the URL (a failure costs these only).
const PENDING_COST: usize = 512;

/// What a fetcher holds beside the credit it has taken: its read buffer, and the copy of a body
/// under [`STEP`] made while the buffer it was read into (credited whole) is still allocated
/// ([`cut_to_size`]).
const FETCHER_SLACK: usize = READ_BUF + STEP;

/// What the image cache sets aside while anything is fetched: the bodies ([`FETCH_BYTES`]), what
/// the fetchers hold beside them ([`FETCHER_SLACK`]) and the bookkeeping of the results waiting
/// ([`PENDING_MAX`]), so nothing the fetchers allocate is outside the budget.
pub const FETCH_RESERVE: usize =
    FETCH_BYTES + FETCHERS * FETCHER_SLACK + PENDING_MAX * PENDING_COST;

/// Credit is taken and the body's buffer grown by this much at a time.
pub(crate) const STEP: usize = 256 << 10;
const _: () = assert!(FETCH_BYTES.is_multiple_of(STEP));

/// Takes credit for that many more bytes of the body being read, waiting until it is given
/// (see [`FETCH_BYTES`]): `false` if the fetch is to be given up instead (see [`Error::Yielded`]).
pub(crate) type Grant<'a> = &'a dyn Fn(usize) -> bool;

/// What one fetch may cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub bytes: u64,
    pub secs: u32,
    pub redirects: u32,
}

/// How long a `curl` may run past its own `--max-time` before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// `--max-redirs 5 --max-time 15 --max-filesize 12M` (what the fetchers may hold).
pub const LIMITS: Limits = Limits {
    bytes: FETCH_BYTES as u64,
    secs: 15,
    redirects: 5,
};

/// What a fetch for a load past VOMD's limits (a click on the placeholder) may cost: a body of
/// up to 256 MiB, and the time to bring it.
pub const FORCED_LIMITS: Limits = Limits {
    bytes: 256 << 20,
    secs: 120,
    redirects: 5,
};

/// What fetches a remote image for a load past VOMD's limits: its body, or why not
/// (`Refusal::Ceiling` past [`FORCED_LIMITS`], `Refusal::Failed` for anything else). The fetch
/// is ended (its `curl` killed) once the flag is set.
pub(crate) type ForcedFetch =
    Arc<dyn Fn(&str, &Arc<AtomicBool>) -> Result<Vec<u8>, Refusal> + Send + Sync>;

/// [`ForcedFetch`] with `curl`.
fn forced_fetch(url: &str, cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, Refusal> {
    fetch_cancellable("curl", url, FORCED_LIMITS, &|_| true, Some(cancel)).map_err(|e| match e {
        Error::TooLarge => Refusal::Ceiling(FORCED_LIMITS.bytes),
        _ => Refusal::Failed,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// There's no `curl` to run.
    NoCurl,
    /// Not an http(s) URL; nothing was run.
    Refused,
    /// `curl` failed: network, HTTP error, too many redirects, a scheme refused, too slow.
    Failed,
    /// More than [`Limits::bytes`].
    TooLarge,
    /// Given up to let an older fetch have the credit it needs: nothing was found out about the
    /// URL, it is fetched again.
    Yielded,
    /// Not a format this build decodes.
    NotAnImage,
    /// An image over VOMD's limits (too many pixels, an SVG too big).
    OverLimits(Refusal),
}

impl Error {
    /// Why the image is over VOMD's limits, if that is why it failed: its placeholder says so,
    /// and a click loads it anyway.
    fn refusal(self) -> Option<Refusal> {
        match self {
            Error::TooLarge => Some(Refusal::Bytes(0)),
            Error::OverLimits(why) => Some(why),
            _ => None,
        }
    }
}

/// Whether `url` may be handed to `curl`: `http://` or `https://` (any case), no longer than
/// [`image::MAX_SRC`] (the copies `Command` makes of it, one for each fetcher, are the size of
/// what it is given: a URL of megabytes would cost them each its length three times over, which
/// nothing counts, and no server takes one), and no spaces or control characters.
pub fn fetchable(url: &str) -> bool {
    // The length first, and no copy of the scheme: a URL of megabytes is never copied or read
    // past what it takes to say no.
    url.len() <= image::MAX_SRC
        && crate::doc::has_http_scheme(url)
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Fetch `url` with the system `curl` within `limits`.
pub fn fetch(url: &str, limits: Limits) -> Result<Vec<u8>, Error> {
    fetch_with("curl", url, limits, &|_| true).map(cut_to_size)
}

/// [`fetch`] with `program` standing in for `curl`, taking credit as the body is read. The body
/// is as it was read: its buffer is as big as the credit taken, for the caller to [`cut_to_size`]
/// once it has done what changes the size of it.
fn fetch_with(program: &str, url: &str, limits: Limits, grant: Grant) -> Result<Vec<u8>, Error> {
    fetch_cancellable(program, url, limits, grant, None)
}

/// How often the watchdog of a fetch that can be cancelled looks at its flag.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// [`fetch_with`] that ends (the `curl` killed, the fetch failed) soon after `cancel` is set.
fn fetch_cancellable(
    program: &str,
    url: &str,
    limits: Limits,
    grant: Grant,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Vec<u8>, Error> {
    if !fetchable(url) {
        return Err(Error::Refused);
    }
    let mut child = Command::new(program)
        // `-q` only works as the first argument.
        .args([
            "-q",
            "--globoff",
            "--silent",
            "--show-error",
            "--fail",
            "--location",
        ])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .arg("--max-redirs")
        .arg(limits.redirects.to_string())
        .arg("--max-time")
        .arg(limits.secs.to_string())
        .arg("--max-filesize")
        .arg(limits.bytes.to_string())
        .arg("--user-agent")
        .arg(concat!("vomd/", env!("CARGO_PKG_VERSION")))
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NoCurl,
            _ => Error::Failed,
        })?;
    let stdout = child.stdout.take();
    let child = Arc::new(Mutex::new(child));
    // `--max-time` is per transfer: whatever keeps this one running past it is killed, which
    // ends the read below. Dropping `done` (when this function returns) retires the watchdog.
    let (done, finished) = channel::<()>();
    let dog = child.clone();
    let wall = Duration::from_secs(u64::from(limits.secs)) + GRACE;
    let cancel = cancel.cloned();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        // (Without a flag, one wait for the whole time: nothing wakes while it runs.)
        let step = if cancel.is_some() { CANCEL_POLL } else { wall };
        loop {
            if finished.recv_timeout(step) != Err(RecvTimeoutError::Timeout) {
                return;
            }
            let stopped = cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
            if stopped || start.elapsed() >= wall {
                let _ = dog.lock().unwrap_or_else(|e| e.into_inner()).kill();
                return;
            }
        }
    });
    // The buffer is as big as the credit taken, no more: it grows by `STEP` at a time instead
    // of doubling (which would leave up to half of it unaccounted).
    let mut bytes: Vec<u8> = Vec::new();
    let mut granted = 0usize;
    let mut over = false;
    let mut yielded = false;
    let read = match stdout {
        Some(mut out) => {
            let mut buf = vec![0u8; READ_BUF];
            loop {
                match out.read(&mut buf) {
                    Ok(0) => break true,
                    Ok(n) => {
                        if (bytes.len() + n) as u64 > limits.bytes {
                            over = true;
                            break true;
                        }
                        if bytes.len() + n > granted {
                            if !grant(STEP) {
                                yielded = true;
                                break true;
                            }
                            granted += STEP;
                            bytes.reserve_exact(granted - bytes.len());
                        }
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break false,
                }
            }
        }
        None => false,
    };
    let kill = |child: &Mutex<std::process::Child>| {
        let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = c.kill();
        let _ = c.wait();
    };
    if over || bytes.len() as u64 > limits.bytes {
        kill(&child);
        drop(done);
        return Err(Error::TooLarge);
    }
    if yielded {
        kill(&child);
        drop(done);
        return Err(Error::Yielded);
    }
    if !read {
        kill(&child);
        drop(done);
        return Err(Error::Failed);
    }
    // The pipe is closed: it is on its way out (waiting without the lock held, so the watchdog
    // can still kill it).
    let status = loop {
        match child.lock().unwrap_or_else(|e| e.into_inner()).try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    drop(done);
    match status {
        Some(s) if s.success() => {}
        // `CURLE_FILESIZE_EXCEEDED`: the size the server declared is over the cap.
        Some(s) if s.code() == Some(63) => return Err(Error::TooLarge),
        _ => return Err(Error::Failed),
    }
    Ok(bytes)
}

/// `bytes` with no more allocated than it holds (a small body, as most are, doesn't keep a 256 KB
/// buffer). A body under [`STEP`] is copied to a buffer of its size: the one it was read into is
/// a mapping of its own (see `platform::tune_allocator`), and the allocator shrinks a mapping
/// only to a whole page, which for a 69-byte image is 4 KB and a mapping per URL: 12,288 of them
/// were 70 MB resident. A bigger one is cut where it is (the mapping gives the rest back). What
/// is left of the body, once its metadata is cut out, is what is cut to size, not what was read.
pub(crate) fn cut_to_size(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.len() < STEP {
        bytes = bytes.as_slice().to_vec();
    } else {
        bytes.shrink_to_fit();
    }
    bytes
}

/// What gives a URL's bytes in tests (`None`: it fails): see [`Remote::set_test_body`].
pub type TestBody = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

/// A fetched image: its bytes and natural size.
pub(crate) type Fetched = Result<(Bytes, u32, u32), Error>;

/// What a fetcher thread runs for a URL (tests put their own in place of [`fetch_image`]).
pub(crate) type Fetcher = Arc<dyn Fn(&str, Grant) -> Fetched + Send + Sync>;

/// Fetch a remote image: its bytes and natural size, if it's of a format this build decodes.
fn fetch_image(url: &str, grant: Grant) -> Fetched {
    let bytes = fetch_with("curl", url, LIMITS, grant)?;
    image_of(bytes)
}

/// A body as fetched, taken as a remote image: see [`fetch_image`].
fn image_of(mut bytes: Vec<u8>) -> Fetched {
    // A JPEG's metadata is cut out of the body where it is, so the cache doesn't keep it and the
    // decoder never sees it. The buffer is cut to size after (its credit goes with it, as the
    // body is what is left: a 300 KB JPEG that is 178 bytes without its metadata must not stay a
    // mapping of its own, a page at least, for every URL).
    if !image::shed_jpeg_metadata(&mut bytes) {
        return Err(Error::NotAnImage);
    }
    let bytes = cut_to_size(bytes);
    if let Some(why) = image::refusal_of(&bytes) {
        return Err(Error::OverLimits(why));
    }
    match image::header_of(&bytes) {
        Some((_, w, h)) => Ok((Arc::new(bytes), w, h)),
        None => Err(Error::NotAnImage),
    }
}

/// A URL being fetched, or fetched and not yet taken by the main thread.
struct Active {
    url: Arc<str>,
    /// Fetches are numbered as they start: the oldest still `reading` is let go on.
    seq: u64,
    /// Credit taken for its body (see [`FETCH_BYTES`]).
    held: usize,
    /// Still being fetched (not a result waiting for the main thread).
    reading: bool,
    /// Waiting for credit.
    blocked: bool,
    /// Started again after it gave up ([`Queue::requeue`]): the pause doesn't make it give up
    /// again ([`Pool::grant`]), so a URL is never run more than twice for the pause's sake, however
    /// often the fetchers are held off.
    second: bool,
}

struct Queue {
    urls: Vec<Arc<str>>,
    /// The URLs of fetches that gave up (see [`Error::Yielded`]), next in line again: at most one
    /// for each fetcher, so the room for them is made with the queue and never grows. (Put at the
    /// front of `urls` they would grow its buffer, which is as big as the document's list, by
    /// doubling it.) A replaced queue ([`Pool::set`]) keeps the ones it still wants: a fetch that
    /// gave up is not forgotten, with the second go that is its right, by a request.
    requeue: Vec<Arc<str>>,
    /// What the strings of `urls` and `requeue` hold ([`image::str_bytes`] of each), kept as it
    /// changes: the URLs are shared with the document's, until the document goes.
    urls_str: usize,
    /// Not queued again while here, however often the queue is replaced or fetching stopped
    /// ([`Pool::done`] removes a URL).
    active: Vec<Active>,
    next_seq: u64,
    wake: Wake,
    /// Fetching was stopped (and not asked for since): a fetch that gave up isn't queued again.
    stopped: bool,
    /// Nothing starts and nothing takes more credit while this is on ([`Remote::pause`]): what is
    /// being read goes on (and is waited for) or gives up, so the fetchers can be made to hold
    /// nothing.
    paused: bool,
    quit: bool,
}

impl Queue {
    /// Credit held, in all.
    fn held(&self) -> usize {
        self.active.iter().map(|a| a.held).sum()
    }
}

/// [`FETCHERS`] threads taking the most wanted URL first; the queue is replaced as the
/// viewport moves. The threads live as long as the window: stopping only empties the queue, so
/// no more than [`FETCHERS`] fetches ever run, and none twice.
struct Pool {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl Pool {
    fn spawn(tx: Sender<Done>, wake: Wake, fetch: Fetcher, paused: bool) -> Pool {
        let queue = Queue {
            urls: Vec::new(),
            requeue: Vec::with_capacity(FETCHERS),
            urls_str: 0,
            active: Vec::new(),
            next_seq: 0,
            wake,
            stopped: false,
            paused,
            quit: false,
        };
        let shared = Arc::new((Mutex::new(queue), Condvar::new()));
        for _ in 0..FETCHERS {
            let (shared, tx, fetch) = (shared.clone(), tx.clone(), fetch.clone());
            std::thread::spawn(move || Pool::run(&shared, &tx, &*fetch));
        }
        Pool { shared }
    }

    /// One fetcher thread: `fetch` what the queue says until it is told to quit.
    fn run(
        shared: &(Mutex<Queue>, Condvar),
        tx: &Sender<Done>,
        fetch: &dyn Fn(&str, Grant) -> Fetched,
    ) {
        let (lock, cv) = shared;
        loop {
            let (url, seq) = {
                let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                // Nothing starts while the credit is spent, or while a fetch waits for it: a
                // `curl` left waiting for credit would time out, and one that starts only to
                // give up again (see `grant`) would be fetched over and over. Nor while
                // `PENDING_MAX` results wait to be taken, or while paused.
                while ((q.urls.is_empty() && q.requeue.is_empty())
                    || q.paused
                    || q.held() >= FETCH_BYTES
                    || q.active.iter().any(|a| a.blocked)
                    || q.active.iter().filter(|a| !a.reading).count() >= PENDING_MAX)
                    && !q.quit
                {
                    q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
                if q.quit {
                    return;
                }
                let second = !q.requeue.is_empty();
                let url = if second {
                    q.requeue.remove(0)
                } else {
                    q.urls.remove(0)
                };
                q.urls_str -= image::str_bytes(&url);
                let seq = q.next_seq;
                q.next_seq += 1;
                q.active.push(Active {
                    url: url.clone(),
                    seq,
                    held: 0,
                    reading: true,
                    blocked: false,
                    second,
                });
                (url, seq)
            };
            let result = fetch(&url, &|n| Pool::grant(shared, seq, n));
            Pool::finish(shared, seq, &result);
            if matches!(result, Err(Error::Yielded)) {
                // It gave up for an older fetch: it is next in line again, as if it hadn't
                // started, and the main thread has nothing to take.
                let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                q.active.retain(|a| a.seq != seq);
                image::shrink_vec(&mut q.active);
                if !q.stopped {
                    // (At most one for each fetcher is ever in it: the room is made.)
                    debug_assert!(q.requeue.len() < q.requeue.capacity());
                    q.urls_str += image::str_bytes(&url);
                    q.requeue.push(url);
                }
                cv.notify_all();
                // Paused to have the fetchers hold nothing: whoever waits for that is told.
                let wake = q.paused.then(|| q.wake.clone());
                drop(q);
                if let Some(wake) = wake {
                    wake();
                }
                continue;
            }
            if tx.send(Done::Fetched { url, result }).is_err() {
                return;
            }
            let wake = lock.lock().unwrap_or_else(|e| e.into_inner()).wake.clone();
            wake();
        }
    }

    /// Fetch `seq` takes credit for `n` more bytes: it waits while that would go over
    /// [`FETCH_BYTES`] and credit may yet be freed (by a result being taken, or by a fetch
    /// giving up). `false` if it is to give up itself (or the pool is gone): when every fetch
    /// still reading waits and no result is waiting to be taken, the youngest does.
    fn grant(shared: &(Mutex<Queue>, Condvar), seq: u64, n: usize) -> bool {
        let (lock, cv) = shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut told = false;
        loop {
            if q.quit {
                return false;
            }
            // Paused: nothing takes more credit, whatever it holds is given up. (Except a fetch
            // that has been started again: it gave up once already, and a URL that is given up
            // for every pause is fetched for ever. It finishes, if the credit it needs is there to
            // be had at once: it never waits for credit while held off, which the decode that is
            // waiting for the fetchers to hold nothing would wait for in turn.)
            if q.paused {
                let second = q.active.iter().any(|a| a.seq == seq && a.second);
                let older = q
                    .active
                    .iter()
                    .any(|a| a.reading && a.blocked && a.seq < seq);
                if second && q.held().saturating_add(n) <= FETCH_BYTES && !older {
                    if let Some(a) = q.active.iter_mut().find(|a| a.seq == seq) {
                        a.held += n;
                        if std::mem::take(&mut a.blocked) {
                            // (It may have been waiting for credit when the pause came: the
                            // others that wait for it go on.)
                            cv.notify_all();
                        }
                    }
                    return true;
                }
                if let Some(a) = q.active.iter_mut().find(|a| a.seq == seq) {
                    a.blocked = false;
                }
                cv.notify_all();
                return false;
            }
            // Credit goes to the oldest that waits for it first, so the youngest can't keep
            // taking what the oldest needs.
            let older = q
                .active
                .iter()
                .any(|a| a.reading && a.blocked && a.seq < seq);
            if q.held().saturating_add(n) <= FETCH_BYTES && !older {
                if let Some(a) = q.active.iter_mut().find(|a| a.seq == seq) {
                    a.held += n;
                    if std::mem::take(&mut a.blocked) {
                        // Fetches may start again, and the next in line may go on.
                        cv.notify_all();
                    }
                }
                return true;
            }
            if let Some(a) = q.active.iter_mut().find(|a| a.seq == seq) {
                a.blocked = true;
            }
            let reading = || q.active.iter().filter(|a| a.reading);
            let stuck =
                reading().all(|a| a.blocked) && !q.active.iter().any(|a| !a.reading && a.held > 0);
            if stuck && reading().map(|a| a.seq).max() == Some(seq) {
                if let Some(a) = q.active.iter_mut().find(|a| a.seq == seq) {
                    a.blocked = false;
                }
                return false;
            }
            // The one that made them all wait may not be the youngest, which has to know.
            if !told {
                told = true;
                cv.notify_all();
            }
            q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Fetch `seq` ended with `result`: what it holds is the body, if there is one.
    fn finish(shared: &(Mutex<Queue>, Condvar), seq: u64, result: &Fetched) {
        let (lock, cv) = shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = q.active.iter_mut().find(|a| a.seq == seq) {
            a.reading = false;
            // What it holds is what the body is allocated, the credit it took or not.
            a.held = match result {
                Ok((bytes, ..)) => bytes.capacity(),
                Err(_) => 0,
            };
        }
        // Whoever waits for the oldest to finish goes on.
        cv.notify_all();
    }

    /// The main thread took the result for `url`: its credit is free.
    fn done(&self, url: &Arc<str>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.active.retain(|a| a.url != *url);
        image::shrink_vec(&mut q.active);
        cv.notify_all();
    }

    /// Fetches being read, and results fetched and not yet released (see [`Pool::done`]).
    fn counts(&self) -> (usize, usize) {
        let (lock, _) = &*self.shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let reading = q.active.iter().filter(|a| a.reading).count();
        (reading, q.active.len() - reading)
    }

    /// Hold the fetchers off (or let them go on): see [`Queue::paused`].
    fn pause(&self, on: bool) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.paused = on;
        cv.notify_all();
    }

    /// What the queue holds: its buffer, as allocated, and its URLs' strings. (The entries of
    /// `active` and the results in the channel are in [`FETCH_RESERVE`]: at most `FETCHERS +
    /// PENDING_MAX` of them, and the list is cut to size when it empties.)
    fn bookkeeping(&self) -> usize {
        let (lock, _) = &*self.shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        image::vec_bytes(&q.urls)
            .saturating_add(image::vec_bytes(&q.requeue))
            .saturating_add(q.urls_str)
    }

    /// What the pool is doing, for the report of a test that failed.
    fn state(&self) -> String {
        let (lock, _) = &*self.shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let active: Vec<String> = q
            .active
            .iter()
            .map(|a| {
                format!(
                    "{}#{}{}{}{} {}",
                    a.url.rsplit('/').next().unwrap_or(""),
                    a.seq,
                    if a.reading { " reading" } else { " result" },
                    if a.blocked { " blocked" } else { "" },
                    if a.second { " second" } else { "" },
                    a.held
                )
            })
            .collect();
        format!(
            "pool: active [{}], {} queued, {} requeued, paused {}, stopped {}",
            active.join(", "),
            q.urls.len(),
            q.requeue.len(),
            q.paused,
            q.stopped
        )
    }

    /// Bytes of bodies held: being read, or waiting to be taken.
    fn held(&self) -> usize {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).held()
    }

    /// Bytes of bodies held: being read, or waiting to be taken.
    #[cfg(test)]
    fn inflight(&self) -> usize {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).held()
    }

    /// Whether anything is queued, being fetched, or fetched and not taken: the fetchers may
    /// hold memory, up to [`FETCH_BYTES`].
    fn busy(&self) -> bool {
        let (lock, _) = &*self.shared;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        !q.urls.is_empty() || !q.requeue.is_empty() || !q.active.is_empty()
    }

    /// Replace the queue with `urls` (not the ones being fetched). The fetches that gave up and
    /// are still wanted stay next in line (they are in `urls` too, and are run once, from there).
    fn set(&self, mut urls: Vec<Arc<str>>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.requeue.retain(|r| urls.iter().any(|u| u == r));
        let q = &mut *q;
        urls.retain(|u| !q.active.iter().any(|a| a.url == *u) && !q.requeue.contains(u));
        q.stopped = false;
        q.urls_str = urls
            .iter()
            .chain(q.requeue.iter())
            .map(|u| image::str_bytes(u))
            .sum();
        q.urls = urls;
        cv.notify_all();
    }

    /// Drop the queue, and don't queue again what gives up. The queue's buffer goes with it (a
    /// `clear` would keep it: 100,000 URLs are 1.6 MB, which nothing would count).
    fn stop(&self) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.stopped = true;
        q.urls = Vec::new();
        q.requeue.clear();
        q.urls_str = 0;
        cv.notify_all();
    }

    /// Call `wake` from now on when a fetch is done.
    fn set_wake(&self, wake: Wake) {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).wake = wake;
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.quit = true;
        q.urls = Vec::new();
        q.requeue.clear();
        q.urls_str = 0;
        cv.notify_all();
    }
}

/// What fetching found of an image: its natural size, and what decoding it holds: `kib`, rounded
/// up to a KiB, whatever the box (`image::source_cost`), and for each pixel of the box besides
/// its result twice, and of its sides (`image::box_costs`: an SVG's layers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub w: u32,
    pub h: u32,
    pub kib: u32,
    pub per_px: u32,
    pub per_side: u32,
}

/// What fetching found of a URL ([`Remote::sizes`]).
type Found = Result<Record, Option<Refusal>>;

/// `bytes` in KiB, rounded up.
fn kib(bytes: usize) -> u32 {
    u32::try_from(bytes.div_ceil(1024)).unwrap_or(u32::MAX)
}

/// Remote images for the window: the policy, whether they're on for this document, and what
/// fetching found this session, by URL.
pub struct Remote {
    pub policy: RemoteImages,
    /// Turned on for the current document.
    pub on: bool,
    /// `curl` couldn't be run: nothing is fetched, the toolbar says why.
    pub no_curl: bool,
    /// Each URL fetched: its natural size and what decoding it holds ([`Record`]), or `Err` if
    /// it failed or isn't an image this build decodes (with why, if that is VOMD's limits). Kept
    /// for the current document's URLs and those with a body or pixels cached (`Images::attach`).
    sizes: HashMap<Arc<str>, Found>,
    /// What the strings of `sizes` hold ([`image::str_bytes`] of each), kept as it changes.
    sizes_str: usize,
    pool: Option<Pool>,
    fetcher: Fetcher,
    /// What fetches for loads past the limits ([`ForcedFetch`]), outside the pool.
    pub(crate) forced_fetch: ForcedFetch,
    /// A fetcher was ever started.
    started: bool,
    /// The fetchers are held off ([`Remote::pause`]), the pool that is started later too.
    paused: bool,
}

impl Default for Remote {
    fn default() -> Self {
        Remote {
            policy: RemoteImages::Ask,
            on: false,
            no_curl: false,
            sizes: HashMap::new(),
            sizes_str: 0,
            pool: None,
            fetcher: Arc::new(fetch_image),
            forced_fetch: Arc::new(forced_fetch),
            started: false,
            paused: false,
        }
    }
}

impl Remote {
    /// What fetching found for `url`: `None` not fetched, `Some(None)` failed.
    pub fn size(&self, url: &str) -> Option<Option<(u32, u32)>> {
        self.sizes.get(url).map(|s| s.ok().map(|r| (r.w, r.h)))
    }

    /// Why the image at `url` is over VOMD's limits, if it was fetched and is.
    pub(crate) fn refusal(&self, url: &str) -> Option<Refusal> {
        *self.sizes.get(url)?.as_ref().err()?
    }

    /// What decoding the image fetched for `url` holds whatever the box (see [`Remote::sizes`]),
    /// rounded up to a KiB, if it was fetched and is an image.
    pub(crate) fn source_cost(&self, url: &str) -> Option<usize> {
        let r = self.sizes.get(url)?.ok()?;
        Some((r.kib as usize).saturating_mul(1024))
    }

    /// What decoding the image fetched for `url` holds for each pixel of its box besides its
    /// result twice, and for each pixel of its sides (see [`Record`]).
    pub(crate) fn box_costs(&self, url: &str) -> (u32, u32) {
        self.sizes
            .get(url)
            .and_then(|r| r.ok())
            .map_or((0, 0), |r| (r.per_px, r.per_side))
    }

    /// A job of the SVG fetched for `url` found it holds more than its record said: what it
    /// holds whatever the box (`source`), and for each pixel of it and of its sides, which only
    /// grow.
    #[cfg(feature = "svg")]
    pub(crate) fn remeasure(&mut self, url: &str, source: usize, (per_px, per_side): (u32, u32)) {
        if let Some(Ok(r)) = self.sizes.get_mut(url) {
            r.kib = r.kib.max(kib(source));
            r.per_px = r.per_px.max(per_px);
            r.per_side = r.per_side.max(per_side);
        }
    }

    /// How many URLs were fetched, and how many failed or aren't images.
    pub fn counts(&self) -> (usize, usize) {
        let fetched = self.sizes.values().filter(|s| s.is_ok()).count();
        (fetched, self.sizes.len() - fetched)
    }

    /// The size layout gives a remote image now.
    pub fn layout_size(&self, url: &str) -> ImageSize {
        match (self.on, self.size(url)) {
            (true, Some(None)) => self.refusal(url).map_or(ImageSize::Alt, ImageSize::Refused),
            (false, _) => ImageSize::Alt,
            (true, Some(Some((w, h)))) => ImageSize::Pixels(w, h),
            (true, None) if self.no_curl => ImageSize::Alt,
            (true, None) => ImageSize::Loading,
        }
    }

    /// A fetch finished and its result is taken: the bytes to keep, if it brought an image. The
    /// fetchers' credit for it is free ([`Remote::release`]).
    #[cfg(test)]
    pub(crate) fn fetched(&mut self, url: &Arc<str>, result: Fetched) -> Option<Bytes> {
        let bytes = self.take_fetched(url, result, true);
        self.release(url);
        bytes
    }

    /// [`Remote::fetched`] without letting the credit go: the caller puts the bytes where they
    /// are charged (the cache) or frees them, and then calls [`Remote::release`], so the body is
    /// never allocated and uncounted at once.
    ///
    /// What is found is recorded for `url` only if `own` (it is the document's, and the table has
    /// room for it: [`Remote::reserve_sizes`]); for another's, only a record there already is
    /// brought up to date, and none is made.
    pub(crate) fn take_fetched(
        &mut self,
        url: &Arc<str>,
        result: Fetched,
        own: bool,
    ) -> Option<Bytes> {
        match result {
            Ok((bytes, w, h)) => {
                let (per_px, per_side) = image::box_costs(&bytes);
                let record = Record {
                    w,
                    h,
                    kib: kib(image::source_cost(&bytes)),
                    per_px,
                    per_side,
                };
                self.record(url, Ok(record), own);
                Some(bytes)
            }
            Err(Error::NoCurl) => {
                self.no_curl = true;
                self.stop();
                None
            }
            // Nothing was found out: asked for again, as an image not fetched yet.
            Err(Error::Yielded) => None,
            Err(e) => {
                self.record(url, Err(e.refusal()), own);
                None
            }
        }
    }

    /// What fetching found for `url`. The table never grows here: a record already there is
    /// replaced, and a new one (`insert`: the document's URL) goes in the room made for the
    /// document's URLs ([`Remote::fit_sizes`]), which is there by what the document's numbers say.
    fn record(&mut self, url: &Arc<str>, size: Found, insert: bool) {
        if let Some(found) = self.sizes.get_mut(url) {
            *found = size;
        } else if insert {
            self.sizes.insert(url.clone(), size);
            self.sizes_str += image::str_bytes(url);
        }
    }

    /// The result for `url` is out of the fetchers' hands (kept in the cache or freed): its
    /// credit is free.
    pub(crate) fn release(&mut self, url: &Arc<str>) {
        if let Some(pool) = &self.pool {
            pool.done(url);
        }
    }

    /// Forget what fetching found for the URLs `keep` says no to (it is given each URL and what
    /// was found: `None` if it failed), and the room the table has left from them.
    pub(crate) fn retain_sizes(
        &mut self,
        mut keep: impl FnMut(&Arc<str>, Option<(u32, u32)>) -> bool,
    ) {
        let mut freed = 0;
        self.sizes.retain(|u, size| {
            let k = keep(u, size.as_ref().ok().map(|r| (r.w, r.h)));
            if !k {
                freed += image::str_bytes(u);
            }
            k
        });
        self.sizes_str -= freed;
    }

    /// Forget what fetching found for `urls` (their last slot in the cache was evicted, and the
    /// document doesn't have them): the entries and their strings. (The table keeps its room,
    /// which is the document's: it is cut to size when the next one is attached,
    /// [`Remote::fit_sizes`], and a smaller one is never made beside it here.)
    pub(crate) fn forget_sizes<'a>(&mut self, urls: impl IntoIterator<Item = &'a Arc<str>>) {
        for u in urls {
            if self.sizes.remove(u).is_some() {
                self.sizes_str -= image::str_bytes(u);
            }
        }
    }

    /// Whether a record for `url` can be made without the table growing (it has one, or room:
    /// the table is given the room for a document's URLs, [`Remote::fit_sizes`]).
    pub(crate) fn has_room_for(&self, url: &Arc<str>) -> bool {
        self.sizes.len() < self.sizes.capacity() || self.sizes.contains_key(url)
    }

    /// Whether the table of sizes has more than twice the room it needs for what it holds and
    /// `extra` more URLs (and 16): see [`image::fit_map`].
    pub(crate) fn sizes_oversized(&self, extra: usize) -> bool {
        self.sizes.capacity() > 2 * self.sizes.len().saturating_add(extra) + 16
    }

    /// What a table of sizes with room for `extra` more URLs than it holds costs
    /// ([`image::table_bytes`]): what a document of that many remote images is charged for it.
    pub(crate) fn sizes_bytes_for(&self, extra: usize) -> usize {
        image::table_bytes(
            self.sizes.len().saturating_add(extra),
            size_of::<(Arc<str>, Found)>(),
        )
    }

    /// Give the table of sizes room for `extra` more URLs (a document is attached, with that many
    /// remote images: what is recorded after is only theirs), and cut it to what that needs if it
    /// has more than twice. The one place it is made, beside the old, for the room made for it.
    pub(crate) fn fit_sizes(&mut self, extra: usize) {
        image::fit_map(&mut self.sizes, extra);
    }

    /// The URLs that failed or aren't images (what [`Remote::size`] calls `Some(None)`).
    pub(crate) fn failed_urls(&self) -> impl Iterator<Item = &Arc<str>> {
        self.sizes
            .iter()
            .filter(|(_, s)| s.is_err())
            .map(|(u, _)| u)
    }

    /// The image at `url` didn't decode: it shows its alt text from now on, and why, if that is
    /// VOMD's limits (`why`). (`own`: the URL is the document's, see [`Remote::record`].)
    pub(crate) fn failed(&mut self, url: &Arc<str>, own: bool, why: Option<Refusal>) {
        self.record(url, Err(why), own);
    }

    /// Fetch `urls`, most wanted first (duplicates are fetched once), replacing what the
    /// fetchers hadn't started. The fetchers start with the first URL.
    pub(crate) fn fetch(&mut self, mut urls: Vec<Arc<str>>, tx: Sender<Done>, wake: Wake) {
        if self.no_curl || !self.on {
            return;
        }
        // (Made at the size it will be: one table, not one grown by doubling.)
        let mut seen = std::collections::HashSet::with_capacity(urls.len());
        urls.retain(|u| seen.insert(u.clone()));
        if urls.is_empty() && self.pool.is_none() {
            return;
        }
        self.started = true;
        let (fetcher, paused) = (&self.fetcher, self.paused);
        self.pool
            .get_or_insert_with(|| Pool::spawn(tx, wake, fetcher.clone(), paused))
            .set(urls);
    }

    /// Stop fetching: what's queued is dropped (fetches in flight finish and are kept).
    pub(crate) fn stop(&mut self) {
        if let Some(pool) = &self.pool {
            pool.stop();
        }
    }

    /// The fetchers call `wake` from now on when a fetch is done.
    pub(crate) fn set_wake(&mut self, wake: Wake) {
        if let Some(pool) = &self.pool {
            pool.set_wake(wake);
        }
    }

    /// Hold the fetchers off, or let them go on: while held off nothing starts and nothing takes
    /// more credit (a transfer that has what it needs finishes, one that needs more gives up and
    /// is queued again), so once every transfer is over and the results are taken, the fetchers
    /// hold nothing ([`Remote::reserve_idle`]).
    pub(crate) fn pause(&mut self, on: bool) {
        self.paused = on;
        if let Some(pool) = &self.pool {
            pool.pause(on);
        }
    }

    /// Fetches being read, and results fetched and not yet released ([`Remote::release`]): the
    /// ones in the channel, the ones the cache is deciding on and the ones that wait.
    pub(crate) fn transfers(&self) -> (usize, usize) {
        self.pool.as_ref().map_or((0, 0), Pool::counts)
    }

    /// Whether the fetchers are held off ([`Remote::pause`]).
    pub(crate) fn paused(&self) -> bool {
        self.paused
    }

    /// Bytes of bodies held: being read, or fetched and not released.
    pub(crate) fn held_bytes(&self) -> usize {
        self.pool.as_ref().map_or(0, Pool::held)
    }

    /// What the fetchers hold while none is being read: the bodies of the results not released,
    /// and the bookkeeping of each, which is all that is left of [`FETCH_RESERVE`].
    pub(crate) fn reserve_idle(&self) -> usize {
        let held = self.pool.as_ref().map_or(0, Pool::held);
        held.saturating_add(self.transfers().1 * PENDING_COST)
    }

    /// What the table of sizes and the queue hold, besides the bodies: the capacity of each
    /// buffer and its strings. The entries of the pool's list and the results in its channel
    /// are in [`FETCH_RESERVE`].
    pub(crate) fn bookkeeping_bytes(&self) -> usize {
        let sizes = image::table_bytes(self.sizes.capacity(), size_of::<(Arc<str>, Found)>());
        sizes
            .saturating_add(self.sizes_str)
            .saturating_add(self.pool.as_ref().map_or(0, Pool::bookkeeping))
    }

    /// Bytes of bodies held outside the cache: being read, or waiting to be taken.
    #[cfg(test)]
    pub(crate) fn inflight(&self) -> usize {
        self.pool.as_ref().map_or(0, Pool::inflight)
    }

    /// Whether fetches are queued, running, or done and not taken: [`FETCH_BYTES`] may be held.
    pub(crate) fn busy(&self) -> bool {
        self.pool.as_ref().is_some_and(Pool::busy)
    }

    /// Fetch with `fetcher` instead of `curl` (tests).
    #[cfg(test)]
    pub(crate) fn set_fetcher(&mut self, fetcher: Fetcher) {
        self.fetcher = fetcher;
    }

    /// Fetch with `body`, which gives the bytes of a URL (`None`: it fails), instead of `curl`:
    /// credit is taken for the body as for a real one, and what is brought is made of it as a
    /// fetched image is ([`image_of`]). For tests that count what fetching allocates, and need
    /// more URLs than `curl` can be run for.
    #[doc(hidden)]
    pub fn set_test_body(&mut self, body: TestBody) {
        let forced = body.clone();
        self.forced_fetch = Arc::new(move |url, _| forced(url).ok_or(Refusal::Failed));
        self.fetcher = Arc::new(move |url, grant| {
            let bytes = body(url).ok_or(Error::Failed)?;
            let mut granted = 0;
            while granted < bytes.len() {
                if !grant(STEP) {
                    return Err(Error::Yielded);
                }
                granted += STEP;
            }
            let mut held = Vec::with_capacity(granted);
            held.extend_from_slice(&bytes);
            drop(bytes);
            image_of(held)
        });
    }

    /// What the pool is doing (the report of a test that failed).
    #[doc(hidden)]
    pub fn pool_state(&self) -> String {
        self.pool.as_ref().map_or_else(String::new, Pool::state)
    }

    /// Whether any fetcher was ever started (tests: nothing touches the network unasked).
    pub fn started(&self) -> bool {
        self.started
    }
}

#[cfg(test)]
mod tests;
