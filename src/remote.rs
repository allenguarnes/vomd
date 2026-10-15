//! Remote images: fetched only after the reader turns them on for the
//! document (`I`, the toolbar, or the `always` policy after the first frame), with the system
//! `curl`, so the binary carries no HTTP or TLS code.
//!
//! Each fetch is one `curl` process, its arguments an argv array (never a shell), the URL after
//! `--` so it can't be read as an option, `~/.curlrc` ignored (`-q`), only http(s) allowed, also
//! for redirects, with caps on redirects, time and size. The size cap is enforced again while
//! reading the pipe. The bytes are sniffed: only PNG and JPEG are kept.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

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
        .args(["-q", "--silent", "--show-error", "--fail", "--location"])
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
    let mut bytes = Vec::new();
    let read = match child.stdout.take() {
        Some(out) => out.take(limits.bytes + 1).read_to_end(&mut bytes).is_ok(),
        None => false,
    };
    if bytes.len() as u64 > limits.bytes {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::TooLarge);
    }
    let ok = child.wait().is_ok_and(|s| s.success());
    if read && ok {
        Ok(bytes)
    } else {
        Err(Error::Failed)
    }
}

/// Fetch a remote image: its bytes and natural size, if it's a PNG or JPEG.
fn fetch_image(url: &str) -> Result<(Arc<[u8]>, u32, u32), Error> {
    let bytes = fetch(url, LIMITS)?;
    match image::header(&mut &bytes[..]) {
        Some((_, w, h)) if u64::from(w) * u64::from(h) <= image::MAX_PIXELS => {
            Ok((bytes.into(), w, h))
        }
        _ => Err(Error::NotAnImage),
    }
}

#[derive(Default)]
struct Queue {
    urls: Vec<Arc<str>>,
    /// Being fetched, or fetched and not yet taken by the main thread ([`Pool::done`]): not
    /// queued again meanwhile.
    active: Vec<Arc<str>>,
    quit: bool,
}

/// [`FETCHERS`] threads taking the most wanted URL first; the queue is replaced as the
/// viewport moves.
struct Pool {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl Pool {
    fn spawn(tx: Sender<Done>, wake: Wake) -> Pool {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        for _ in 0..FETCHERS {
            let (shared, tx, wake) = (shared.clone(), tx.clone(), wake.clone());
            std::thread::spawn(move || {
                let (lock, cv) = &*shared;
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
                    let result = fetch_image(&url);
                    if tx.send(Done::Fetched { url, result }).is_err() {
                        return;
                    }
                    wake();
                }
            });
        }
        Pool { shared }
    }

    /// The main thread took the result for `url`.
    fn done(&self, url: &Arc<str>) {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .retain(|u| u != url);
    }

    fn set(&self, mut urls: Vec<Arc<str>>) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        urls.retain(|u| !q.active.contains(u));
        q.urls = urls;
        cv.notify_all();
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
    pub(crate) fn fetched(
        &mut self,
        url: &Arc<str>,
        result: Result<(Arc<[u8]>, u32, u32), Error>,
    ) -> Option<Arc<[u8]>> {
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
                self.pool = None;
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
        self.pool
            .get_or_insert_with(|| Pool::spawn(tx, wake))
            .set(urls);
    }

    /// Stop fetching: what's queued is dropped (fetches in flight finish and are kept).
    pub(crate) fn stop(&mut self) {
        self.pool = None;
    }

    /// Whether any fetcher was ever started (tests: nothing touches the network unasked).
    pub fn started(&self) -> bool {
        self.started
    }
}

#[cfg(test)]
mod tests;
