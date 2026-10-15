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

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::image::{self, Done, Wake};
use crate::layout::ImageSize;
use crate::settings::RemoteImages;

/// Fetchers running at once, at most.
pub const FETCHERS: usize = 4;

/// What one fetch may cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub bytes: u64,
    pub secs: u32,
    pub redirects: u32,
}

/// How long a `curl` may run past its own `--max-time` before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// `--max-redirs 5 --max-time 15 --max-filesize 20M`.
pub const LIMITS: Limits = Limits {
    bytes: 20 << 20,
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
    fetch_with("curl", url, limits)
}

/// [`fetch`] with `program` standing in for `curl`.
fn fetch_with(program: &str, url: &str, limits: Limits) -> Result<Vec<u8>, Error> {
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
    let mut bytes = Vec::new();
    let read = match stdout {
        Some(out) => out.take(limits.bytes + 1).read_to_end(&mut bytes).is_ok(),
        None => false,
    };
    let kill = |child: &Mutex<std::process::Child>| {
        let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = c.kill();
        let _ = c.wait();
    };
    if bytes.len() as u64 > limits.bytes {
        kill(&child);
        drop(done);
        return Err(Error::TooLarge);
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
    if ok { Ok(bytes) } else { Err(Error::Failed) }
}

/// A fetched image: its bytes and natural size.
pub(crate) type Fetched = Result<(Arc<[u8]>, u32, u32), Error>;

/// What a fetcher thread runs for a URL (tests put their own in place of [`fetch_image`]).
type Fetcher = Arc<dyn Fn(&str) -> Fetched + Send + Sync>;

/// Fetch a remote image: its bytes and natural size, if it's a PNG or JPEG.
fn fetch_image(url: &str) -> Fetched {
    let bytes = fetch(url, LIMITS)?;
    match image::header(&mut &bytes[..]) {
        Some((_, w, h)) if u64::from(w) * u64::from(h) <= image::MAX_PIXELS => {
            Ok((bytes.into(), w, h))
        }
        _ => Err(Error::NotAnImage),
    }
}

struct Queue {
    urls: Vec<Arc<str>>,
    /// Being fetched, or fetched and not yet taken by the main thread ([`Pool::done`]): not
    /// queued again meanwhile, however often the queue is replaced or fetching stopped.
    active: Vec<Arc<str>>,
    wake: Wake,
    quit: bool,
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
            wake,
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
    fn run(shared: &(Mutex<Queue>, Condvar), tx: &Sender<Done>, fetch: &dyn Fn(&str) -> Fetched) {
        let (lock, cv) = shared;
        loop {
            let url = {
                let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                while q.urls.is_empty() && !q.quit {
                    q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
                if q.quit {
                    return;
                }
                let url = q.urls.remove(0);
                q.active.push(url.clone());
                url
            };
            let result = fetch(&url);
            if tx.send(Done::Fetched { url, result }).is_err() {
                return;
            }
            let wake = lock.lock().unwrap_or_else(|e| e.into_inner()).wake.clone();
            wake();
        }
    }

    /// The main thread took the result for `url`.
    fn done(&self, url: &Arc<str>) {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .retain(|u| u != url);
    }

    /// Replace the queue with `urls` (not the ones being fetched).
    fn set(&self, mut urls: Vec<Arc<str>>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        urls.retain(|u| !q.active.contains(u));
        q.urls = urls;
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
    /// Each URL fetched: its natural size, or `None` if it failed or isn't PNG or JPEG.
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

    /// The size layout gives a remote image now.
    pub fn layout_size(&self, url: &str) -> ImageSize {
        match (self.on, self.size(url)) {
            (false, _) | (true, Some(None)) => ImageSize::Alt,
            (true, Some(Some((w, h)))) => ImageSize::Pixels(w, h),
            (true, None) if self.no_curl => ImageSize::Alt,
            (true, None) => ImageSize::Loading,
        }
    }

    /// A fetch finished: the bytes to keep, if it brought an image.
    pub(crate) fn fetched(&mut self, url: &Arc<str>, result: Fetched) -> Option<Arc<[u8]>> {
        if let Some(pool) = &self.pool {
            pool.done(url);
        }
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
            Err(_) => {
                self.sizes.insert(url.clone(), None);
                None
            }
        }
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
            pool.set(Vec::new());
        }
    }

    /// The fetchers call `wake` from now on when a fetch is done.
    pub(crate) fn set_wake(&mut self, wake: Wake) {
        if let Some(pool) = &self.pool {
            pool.set_wake(wake);
        }
    }

    /// Whether any fetcher was ever started (tests: nothing touches the network unasked).
    pub fn started(&self) -> bool {
        self.started
    }
}

#[cfg(test)]
mod tests;
