//! Remote images: fetched only after the reader turns them on for the
//! document (`I`, the toolbar, or the `always` policy after the first frame), with the system
//! `curl`, so the binary carries no HTTP or TLS code.
//!
//! Each fetch is one `curl` process, its arguments an argv array (never a shell), the URL after
//! `--` so it can't be read as an option, `~/.curlrc` ignored (`-q`), no URL globbing
//! (`--globoff`: `x[1-1000000].png` is one URL), only http(s) allowed, also for redirects, with
//! caps on redirects, time and size. The size cap is enforced again while reading the pipe, and
//! the time cap by a watchdog that kills a `curl` still running well after it. The bytes are
//! sniffed: only PNG and JPEG are kept.
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

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::image::{self, Bytes, Done, Wake};
use crate::layout::ImageSize;
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
    /// Not a PNG or JPEG (or too many pixels to decode).
    NotAnImage,
}

/// Whether `url` may be handed to `curl`: `http://` or `https://` (any case), and no spaces or
/// control characters.
pub fn fetchable(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
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
    std::thread::spawn(move || {
        if finished.recv_timeout(wall) == Err(RecvTimeoutError::Timeout) {
            let _ = dog.lock().unwrap_or_else(|e| e.into_inner()).kill();
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
    let ok = loop {
        match child.lock().unwrap_or_else(|e| e.into_inner()).try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break false,
        }
    };
    drop(done);
    if !ok {
        return Err(Error::Failed);
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

/// A fetched image: its bytes and natural size.
pub(crate) type Fetched = Result<(Bytes, u32, u32), Error>;

/// What a fetcher thread runs for a URL (tests put their own in place of [`fetch_image`]).
pub(crate) type Fetcher = Arc<dyn Fn(&str, Grant) -> Fetched + Send + Sync>;

/// Fetch a remote image: its bytes and natural size, if it's a PNG or JPEG.
fn fetch_image(url: &str, grant: Grant) -> Fetched {
    let mut bytes = fetch_with("curl", url, LIMITS, grant)?;
    // A JPEG's metadata is cut out of the body where it is, so the cache doesn't keep it and the
    // decoder never sees it. The buffer is cut to size after (its credit goes with it, as the
    // body is what is left: a 300 KB JPEG that is 178 bytes without its metadata must not stay a
    // mapping of its own, a page at least, for every URL).
    if !image::shed_jpeg_metadata(&mut bytes) {
        return Err(Error::NotAnImage);
    }
    let bytes = cut_to_size(bytes);
    match image::header(&mut &bytes[..]) {
        Some((_, w, h)) if u64::from(w) * u64::from(h) <= image::MAX_PIXELS => {
            Ok((Arc::new(bytes), w, h))
        }
        _ => Err(Error::NotAnImage),
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
}

struct Queue {
    urls: Vec<Arc<str>>,
    /// Not queued again while here, however often the queue is replaced or fetching stopped
    /// ([`Pool::done`] removes a URL).
    active: Vec<Active>,
    next_seq: u64,
    wake: Wake,
    /// Fetching was stopped (and not asked for since): a fetch that gave up isn't queued again.
    stopped: bool,
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
    fn spawn(tx: Sender<Done>, wake: Wake, fetch: Fetcher) -> Pool {
        let queue = Queue {
            urls: Vec::new(),
            active: Vec::new(),
            next_seq: 0,
            wake,
            stopped: false,
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
                // `PENDING_MAX` results wait to be taken.
                while (q.urls.is_empty()
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
                let url = q.urls.remove(0);
                let seq = q.next_seq;
                q.next_seq += 1;
                q.active.push(Active {
                    url: url.clone(),
                    seq,
                    held: 0,
                    reading: true,
                    blocked: false,
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
                if !q.stopped {
                    q.urls.insert(0, url);
                }
                cv.notify_all();
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
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .retain(|a| a.url != *url);
        cv.notify_all();
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
        !q.urls.is_empty() || !q.active.is_empty()
    }

    /// Replace the queue with `urls` (not the ones being fetched).
    fn set(&self, mut urls: Vec<Arc<str>>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        urls.retain(|u| !q.active.iter().any(|a| a.url == *u));
        q.stopped = false;
        q.urls = urls;
        cv.notify_all();
    }

    /// Drop the queue, and don't queue again what gives up.
    fn stop(&self) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.stopped = true;
        q.urls.clear();
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
        q.urls.clear();
        cv.notify_all();
    }
}

/// Remote images for the window: the policy, whether they're on for this document, and what
/// fetching found this session, by URL.
pub struct Remote {
    pub policy: RemoteImages,
    /// Turned on for the current document.
    pub on: bool,
    /// `curl` couldn't be run: nothing is fetched, the toolbar says why.
    pub no_curl: bool,
    /// Each URL fetched: its natural size, or `None` if it failed or isn't PNG or JPEG. Kept for
    /// the current document's URLs and those with a body or pixels cached (`Images::attach`).
    sizes: HashMap<Arc<str>, Option<(u32, u32)>>,
    pool: Option<Pool>,
    fetcher: Fetcher,
    /// A fetcher was ever started.
    started: bool,
}

impl Default for Remote {
    fn default() -> Self {
        Remote {
            policy: RemoteImages::Ask,
            on: false,
            no_curl: false,
            sizes: HashMap::new(),
            pool: None,
            fetcher: Arc::new(fetch_image),
            started: false,
        }
    }
}

impl Remote {
    /// What fetching found for `url`: `None` not fetched, `Some(None)` failed.
    pub fn size(&self, url: &str) -> Option<Option<(u32, u32)>> {
        self.sizes.get(url).copied()
    }

    /// How many URLs were fetched, and how many failed or aren't PNG or JPEG.
    pub fn counts(&self) -> (usize, usize) {
        let fetched = self.sizes.values().filter(|s| s.is_some()).count();
        (fetched, self.sizes.len() - fetched)
    }

    /// The size layout gives a remote image now.
    pub fn layout_size(&self, url: &str) -> ImageSize {
        match (self.on, self.size(url)) {
            (false, _) | (true, Some(None)) => ImageSize::Alt,
            (true, Some(Some((w, h)))) => ImageSize::Pixels(w, h),
            (true, None) if self.no_curl => ImageSize::Alt,
            (true, None) => ImageSize::Loading,
        }
    }

    /// A fetch finished and its result is taken: the bytes to keep, if it brought an image. The
    /// fetchers' credit for it is free ([`Remote::release`]).
    #[cfg(test)]
    pub(crate) fn fetched(&mut self, url: &Arc<str>, result: Fetched) -> Option<Bytes> {
        let bytes = self.take_fetched(url, result);
        self.release(url);
        bytes
    }

    /// [`Remote::fetched`] without letting the credit go: the caller puts the bytes where they
    /// are charged (the cache) or frees them, and then calls [`Remote::release`], so the body is
    /// never allocated and uncounted at once.
    pub(crate) fn take_fetched(&mut self, url: &Arc<str>, result: Fetched) -> Option<Bytes> {
        match result {
            Ok((bytes, w, h)) => {
                self.sizes.insert(url.clone(), Some((w, h)));
                Some(bytes)
            }
            Err(Error::NoCurl) => {
                self.no_curl = true;
                self.stop();
                None
            }
            // Nothing was found out: asked for again, as an image not fetched yet.
            Err(Error::Yielded) => None,
            Err(_) => {
                self.sizes.insert(url.clone(), None);
                None
            }
        }
    }

    /// The result for `url` is out of the fetchers' hands (kept in the cache or freed): its
    /// credit is free.
    pub(crate) fn release(&mut self, url: &Arc<str>) {
        if let Some(pool) = &self.pool {
            pool.done(url);
        }
    }

    /// Forget what fetching found for the URLs `keep` says no to.
    pub(crate) fn retain_sizes(&mut self, mut keep: impl FnMut(&Arc<str>) -> bool) {
        self.sizes.retain(|u, _| keep(u));
    }

    /// The image at `url` didn't decode: it shows its alt text from now on.
    pub(crate) fn failed(&mut self, url: &Arc<str>) {
        self.sizes.insert(url.clone(), None);
    }

    /// Fetch `urls`, most wanted first (duplicates are fetched once), replacing what the
    /// fetchers hadn't started. The fetchers start with the first URL.
    pub(crate) fn fetch(&mut self, mut urls: Vec<Arc<str>>, tx: Sender<Done>, wake: Wake) {
        if self.no_curl || !self.on {
            return;
        }
        let mut seen = std::collections::HashSet::new();
        urls.retain(|u| seen.insert(u.clone()));
        if urls.is_empty() && self.pool.is_none() {
            return;
        }
        self.started = true;
        let fetcher = &self.fetcher;
        self.pool
            .get_or_insert_with(|| Pool::spawn(tx, wake, fetcher.clone()))
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

    /// Whether any fetcher was ever started (tests: nothing touches the network unasked).
    pub fn started(&self) -> bool {
        self.started
    }
}

#[cfg(test)]
mod tests;
