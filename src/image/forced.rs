//! Images loaded past VOMD's limits because the reader clicked their placeholder ("click to
//! load"): a forced load.
//!
//! The reader has consented for that one image, so the caps that keep the others within the
//! image budget don't apply to it (its pixels, an SVG's size, elements, depth, `<use>`, work and
//! deadline), and what it holds is not counted in the budget. Two things stay: data that is
//! broken or of a format VOMD doesn't read is never loaded, and nothing is let crash the
//! process: a load needs no more memory than [`ceiling`] (half the memory available, 1 GiB at
//! most), worked out with the same charges as the budget's before anything is decoded, and an
//! SVG keeps hard caps ([`super::svg::Caps`]). The privacy rules don't change either: an SVG
//! reads no file and fetches nothing, and a remote image is only fetched once remote images
//! are on.
//!
//! Loads run on a thread of their own, one at a time and in the order they were asked for, so
//! the other images keep loading on the decode worker meanwhile; the thread is started for a
//! load and ends when there are none left, so nothing runs while nothing is loading. A load has
//! no deadline: it ends when it is done, or when it is cancelled (a flag it checks once a row,
//! a node of an SVG, ...; an SVG drawn in a process of its own is killed), which happens when
//! its document goes, or its box changes size (it is loaded again at the new one).
//!
//! What a load made is kept here ([`Forced`]), beside the cache and not in it: the last size of
//! each image, the least recently painted going first past [`KEPT_BYTES`] (never one painted in
//! the current frame), and the bodies of remote ones, so a zoom doesn't fetch them again. The
//! choice itself is kept for the window's life, by file or URL: a reload or another document
//! with the same image loads it again, without asking. One that fails is not tried again until
//! its file changes.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};

use super::{Bitmap, Bytes, Done, Wake};
use crate::layout::Refusal;

/// What a load past the limits may need at most when `/proc/meminfo` can't say what is free.
const FALLBACK_CEILING: usize = 512 << 20;
/// What a load past the limits may need at most, however much memory is free.
const MAX_CEILING: usize = 1 << 30;
/// Bytes of the forced loads' pixels and bodies kept beyond those painted in the current
/// frame ([`Forced::kept`]).
pub(super) const KEPT_BYTES: usize = 128 << 20;
/// Images chosen (and choices that failed) remembered, at most: the oldest are forgotten.
const MAX_CHOSEN: usize = 4096;
/// The forced thread's stack: an SVG's tree may be 1,024 elements deep.
const STACK: usize = 64 << 20;

/// What a load past the limits may need at most: half the memory available now, and no more
/// than 1 GiB.
pub(crate) fn ceiling() -> usize {
    let available = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| {
        let line = m.lines().find(|l| l.starts_with("MemAvailable:"))?;
        let kib: usize = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib.saturating_mul(1024))
    });
    available
        .map_or(FALLBACK_CEILING, |a| a / 2)
        .min(MAX_CEILING)
}

/// Where a chosen image comes from: its file, or its URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Origin {
    File(PathBuf),
    Url(Arc<str>),
}

/// What a file was when its load failed: it is tried again only once it has changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Stamp {
    pub(crate) fn of(origin: &Origin) -> Option<Stamp> {
        let Origin::File(path) = origin else {
            return None;
        };
        let meta = std::fs::metadata(path).ok()?;
        Some(Stamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// One load: `origin` drawn at `w` × `h` (none for a remote image whose size isn't known yet:
/// it is only fetched), for the document of `doc_gen`.
pub(crate) struct Job {
    pub doc_gen: u64,
    pub origin: Origin,
    pub size: Option<(u32, u32)>,
    /// A remote image's body, fetched before.
    pub body: Option<Bytes>,
    pub cancel: Arc<AtomicBool>,
}

/// What a load made: the pixels (none for a fetch alone), the image's natural size, and a
/// remote image's body, to keep.
pub(crate) struct Loaded {
    pub bitmap: Option<Bitmap>,
    pub natural: (u32, u32),
    pub body: Option<Bytes>,
}

/// What a remote image is fetched with (`Remote::forced_fetch`).
#[cfg(feature = "remote-images")]
type Fetch = crate::remote::ForcedFetch;

/// The load running: what it is for (see [`Job`]), and its flag.
type Running = (Origin, Option<(u32, u32)>, Arc<AtomicBool>);

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    running: Option<Running>,
    /// The thread is there (it ends once the queue is empty).
    alive: bool,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;

/// A forced load's pixels, kept.
struct Kept {
    origin: Origin,
    bitmap: Bitmap,
    /// The frame they were last painted in.
    used: u64,
    /// Made for a document that has gone (see [`Forced::stale`]): shown until new ones come.
    stale: bool,
}

/// The forced loads: the choices, the queue and its thread, what they made.
pub(crate) struct Forced {
    shared: Shared,
    /// What the reader chose to load, oldest first.
    chosen: Vec<Origin>,
    /// Chosen images whose load failed, and why, oldest first.
    failed: Vec<(Origin, Refusal, Option<Stamp>)>,
    kept: Vec<Kept>,
    /// Remote images' bodies.
    bodies: Vec<(Arc<str>, Bytes)>,
    /// Remote images' natural sizes, as the loads found them, oldest first: kept when the body is
    /// let go (it is fetched again for another box), as a size that comes and goes is a box that
    /// flips between a size and loading, and a load that never ends.
    naturals: Vec<(Arc<str>, (u32, u32))>,
    /// What `kept` and `bodies` may hold beyond what is painted ([`KEPT_BYTES`]; the tests' is
    /// smaller).
    pub cap: usize,
    /// The ceiling of the next loads (`None`: [`ceiling`], read when one is asked for).
    pub ceiling: Option<usize>,
    /// The faces SVG text is set in.
    #[cfg(feature = "svg")]
    pub fonts: Option<Arc<crate::fonts::SvgFonts>>,
    /// How remote images are fetched.
    #[cfg(feature = "remote-images")]
    pub fetch: Option<Fetch>,
    /// Loads started (for the tests).
    pub loads: usize,
    /// Set, a load waits before it starts until it is cleared or the load cancelled (the tests).
    #[cfg(test)]
    pub hold: Arc<AtomicBool>,
}

impl Default for Forced {
    fn default() -> Self {
        Forced {
            shared: Arc::default(),
            chosen: Vec::new(),
            failed: Vec::new(),
            kept: Vec::new(),
            bodies: Vec::new(),
            naturals: Vec::new(),
            cap: KEPT_BYTES,
            ceiling: None,
            #[cfg(feature = "svg")]
            fonts: None,
            #[cfg(feature = "remote-images")]
            fetch: None,
            loads: 0,
            #[cfg(test)]
            hold: Arc::default(),
        }
    }
}

impl Drop for Forced {
    fn drop(&mut self) {
        // The thread isn't waited for: what runs stops at its next check, and nothing is queued.
        self.abandon();
    }
}

impl Forced {
    /// Nothing was chosen, or failed: every image goes the normal way.
    pub(crate) fn is_unused(&self) -> bool {
        self.chosen.is_empty() && self.failed.is_empty()
    }

    pub(crate) fn is_chosen(&self, origin: &Origin) -> bool {
        self.chosen.contains(origin)
    }

    /// The reader clicked the placeholder of `origin`: it is loaded past the limits from now on.
    pub(crate) fn choose(&mut self, origin: Origin) {
        self.failed.retain(|f| f.0 != origin);
        if !self.chosen.contains(&origin) {
            if self.chosen.len() >= MAX_CHOSEN {
                self.chosen.remove(0);
            }
            self.chosen.push(origin);
        }
    }

    /// Why the load of `origin` failed, if it did and (a file) it hasn't changed since.
    pub(crate) fn failure(&self, origin: &Origin) -> Option<Refusal> {
        let (_, why, stamp) = self.failed.iter().find(|f| f.0 == *origin)?;
        (*stamp == Stamp::of(origin)).then_some(*why)
    }

    /// The natural size of the remote image at `url`, if a load fetched it.
    pub(crate) fn natural(&self, url: &str) -> Option<(u32, u32)> {
        self.naturals.iter().find(|n| &*n.0 == url).map(|n| n.1)
    }

    /// The pixels kept for `origin`, of whatever size (a zoom shows the last ones until the new
    /// ones come): painted in `frame`.
    pub(crate) fn pixels(&mut self, origin: &Origin, frame: u64) -> Option<&Bitmap> {
        let k = self.kept.iter_mut().find(|k| k.origin == *origin)?;
        k.used = frame;
        Some(&k.bitmap)
    }

    /// Whether pixels of `origin` at `w` × `h` are kept.
    fn has(&self, origin: &Origin, size: Option<(u32, u32)>) -> bool {
        let Some((w, h)) = size else {
            // A fetch alone: done once the body is here.
            return matches!(origin, Origin::Url(u) if self.natural(u).is_some());
        };
        self.kept
            .iter()
            .any(|k| k.origin == *origin && !k.stale && (k.bitmap.width, k.bitmap.height) == (w, h))
    }

    /// A document is attached: the pixels kept are of what the files were before, which may have
    /// changed (a reload). They are shown until they are loaded again, when they are asked for.
    pub(crate) fn stale(&mut self) {
        for k in &mut self.kept {
            k.stale = true;
        }
    }

    /// Load the images `wants` asks for (most wanted first) that aren't loaded at that size: a
    /// load of one at another size is cancelled first.
    pub(crate) fn request(
        &mut self,
        wants: Vec<(Origin, Option<(u32, u32)>)>,
        doc_gen: u64,
        tx: &Sender<Done>,
        wake: &Wake,
    ) {
        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut started = false;
        for (origin, size) in wants {
            if self.has(&origin, size) {
                continue;
            }
            if let Some((o, s, cancel)) = &q.running
                && *o == origin
            {
                // (One cancelled, by a reload, ends with no result: the load is asked for again.)
                if *s == size && !cancel.load(Ordering::Relaxed) {
                    continue;
                }
                cancel.store(true, Ordering::Relaxed);
            }
            if let Some(j) = q.jobs.iter().find(|j| j.origin == origin) {
                if j.size == size {
                    continue;
                }
                q.jobs.retain(|j| j.origin != origin);
            }
            let body = match &origin {
                Origin::Url(u) => self.bodies.iter().find(|b| b.0 == *u).map(|b| b.1.clone()),
                Origin::File(_) => None,
            };
            q.jobs.push_back(Job {
                doc_gen,
                origin,
                size,
                body,
                cancel: Arc::default(),
            });
            self.loads += 1;
            started = true;
        }
        if started && !q.alive {
            q.alive = true;
            let shared = self.shared.clone();
            let tx = tx.clone();
            let wake = wake.clone();
            let ceiling = self.ceiling;
            #[cfg(test)]
            let hold = self.hold.clone();
            #[cfg(feature = "svg")]
            let fonts = self.fonts.clone();
            #[cfg(feature = "remote-images")]
            let fetch = self.fetch.clone();
            let spawned = std::thread::Builder::new()
                .stack_size(STACK)
                .spawn(move || {
                    let mut env = Env {
                        #[cfg(test)]
                        hold,
                        ceiling,
                        #[cfg(feature = "svg")]
                        fonts: Default::default(),
                        #[cfg(feature = "remote-images")]
                        fetch,
                    };
                    #[cfg(feature = "svg")]
                    env.fonts.set(fonts);
                    work(&shared, &tx, &wake, &mut env);
                });
            if spawned.is_err() {
                q.alive = false;
            }
        }
        cv.notify_one();
    }

    /// Stop every load: the one running at its next check, the queued ones now (the document
    /// went).
    pub(crate) fn abandon(&mut self) {
        let mut q = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        q.jobs.clear();
        if let Some((_, _, cancel)) = &q.running {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the thread has taken a load (the tests).
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .running
            .is_some()
    }

    /// Whether a load runs or waits (the tests).
    pub(crate) fn busy(&self) -> bool {
        let q = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        q.alive || !q.jobs.is_empty()
    }

    /// A load of `origin` made `loaded`: kept, the least recently painted going first past the
    /// cap, never those painted in `frame`.
    pub(crate) fn loaded(&mut self, origin: Origin, loaded: Loaded, frame: u64) {
        let mut stored = None;
        if let Origin::Url(url) = &origin {
            self.naturals.retain(|n| n.0 != *url);
            if self.naturals.len() >= MAX_CHOSEN {
                self.naturals.remove(0);
            }
            self.naturals.push((url.clone(), loaded.natural));
            if let Some(body) = loaded.body {
                self.bodies.retain(|b| b.0 != *url);
                self.bodies.push((url.clone(), body));
                stored = Some(url.clone());
            }
        }
        if let Some(bitmap) = loaded.bitmap {
            self.kept.retain(|k| k.origin != origin);
            self.kept.push(Kept {
                origin,
                bitmap,
                used: frame,
                stale: false,
            });
        }
        self.trim(frame, stored.as_deref());
    }

    /// The pixels and bodies kept within the cap, but those painted in `frame` and the body of
    /// `stored` (the one just made: its load's own pixels are drawn from it for the next sizes).
    pub(crate) fn trim(&mut self, frame: u64, stored: Option<&str>) {
        let pixels = |k: &Kept| k.bitmap.data.len() * 4;
        let mut held: usize = self.kept.iter().map(pixels).sum::<usize>()
            + self.bodies.iter().map(|b| b.1.len()).sum::<usize>();
        while held > self.cap {
            // Bodies first: a body is fetched again only for a box of another size.
            if let Some(i) = self.bodies.iter().position(|b| Some(&*b.0) != stored) {
                held -= self.bodies.remove(i).1.len();
                continue;
            }
            let Some(i) = (0..self.kept.len())
                .filter(|&i| self.kept[i].used != frame)
                .min_by_key(|&i| self.kept[i].used)
            else {
                break;
            };
            held -= pixels(&self.kept.remove(i));
        }
    }

    /// The load of `origin` failed: it isn't chosen any more, and isn't tried again until its file
    /// changes.
    pub(crate) fn failed(&mut self, origin: Origin, why: Refusal) {
        self.chosen.retain(|o| *o != origin);
        self.kept.retain(|k| k.origin != origin);
        self.failed.retain(|f| f.0 != origin);
        if self.failed.len() >= MAX_CHOSEN {
            self.failed.remove(0);
        }
        let stamp = Stamp::of(&origin);
        self.failed.push((origin, why, stamp));
    }

    /// The pixels kept are drawn with the old fonts: SVGs are drawn again.
    #[cfg(feature = "svg")]
    pub(crate) fn forget_pixels(&mut self) {
        self.kept.clear();
    }

    /// What the choices cost to keep, for the bookkeeping of the budget (their pixels and bodies
    /// are outside it).
    pub(crate) fn bookkeeping(&self) -> usize {
        let origin = |o: &Origin| match o {
            Origin::File(p) => p.as_os_str().len(),
            Origin::Url(_) => 0,
        };
        super::vec_bytes(&self.chosen)
            .saturating_add(super::vec_bytes(&self.failed))
            .saturating_add(super::vec_bytes(&self.naturals))
            .saturating_add(self.chosen.iter().map(origin).sum())
            .saturating_add(self.failed.iter().map(|f| origin(&f.0)).sum())
    }
}

/// What the forced thread keeps between loads.
struct Env {
    #[cfg(test)]
    hold: Arc<AtomicBool>,
    ceiling: Option<usize>,
    #[cfg(feature = "svg")]
    fonts: super::svg::Fonts,
    #[cfg(feature = "remote-images")]
    fetch: Option<Fetch>,
}

/// The forced thread: the loads of the queue, one at a time, until there are none.
fn work(shared: &Shared, tx: &Sender<Done>, wake: &Wake, env: &mut Env) {
    let (lock, _) = &**shared;
    loop {
        let job = {
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            q.running = None;
            let Some(job) = q.jobs.pop_front() else {
                q.alive = false;
                return;
            };
            q.running = Some((job.origin.clone(), job.size, job.cancel.clone()));
            job
        };
        let result = load(&job, env);
        // A load cancelled made nothing anyone wants: its memory is free before the next starts.
        if job.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let done = Done::Forced {
            doc_gen: job.doc_gen,
            origin: job.origin,
            result,
        };
        if tx.send(done).is_err() {
            lock.lock().unwrap_or_else(|e| e.into_inner()).alive = false;
            return;
        }
        wake();
    }
}

/// Run `job`: read or fetch its bytes, and decode them within the ceiling.
fn load(job: &Job, env: &mut Env) -> Result<Loaded, Refusal> {
    #[cfg(test)]
    while env.hold.load(Ordering::Relaxed) && !job.cancel.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if job.cancel.load(Ordering::Relaxed) {
        return Err(Refusal::Failed);
    }
    let ceiling = env.ceiling.unwrap_or_else(ceiling);
    let (bytes, fetched) = match (&job.origin, &job.body) {
        (_, Some(body)) => (body.clone(), false),
        (Origin::File(path), None) => {
            let mut bytes = super::read_file(path, usize::MAX).ok_or(Refusal::Failed)?;
            if !super::shed_jpeg_metadata(&mut bytes) {
                return Err(Refusal::Failed);
            }
            (Arc::new(bytes), false)
        }
        #[cfg(feature = "remote-images")]
        (Origin::Url(url), None) => {
            let fetch = env.fetch.as_ref().ok_or(Refusal::Failed)?;
            let mut bytes = fetch(url, &job.cancel)?;
            if !super::shed_jpeg_metadata(&mut bytes) {
                return Err(Refusal::Failed);
            }
            (Arc::new(crate::remote::cut_to_size(bytes)), true)
        }
        #[cfg(not(feature = "remote-images"))]
        (Origin::Url(_), None) => return Err(Refusal::Failed),
    };
    let header = super::info_of(&bytes).ok_or(Refusal::Failed)?;
    let natural = header.shown();
    let body = matches!(job.origin, Origin::Url(_)).then(|| bytes.clone());
    let Some((w, h)) = job.size else {
        // Fetched for its size: drawn once its box is known.
        let _ = fetched;
        return Ok(Loaded {
            bitmap: None,
            natural,
            body,
        });
    };
    let (w, h) = super::clamp_box(w, h);
    let room = ceiling.saturating_sub(bytes.len());
    let need = super::decode_cost(&header, w, h).saturating_add(bytes.len());
    // An SVG's charge is what it is found to need: its job says (`Refusal::Memory`).
    if !super::is_svg(&header) && need > ceiling {
        return Err(Refusal::Ceiling(need as u64));
    }
    let limits = super::Limits::forced(room);
    #[cfg(feature = "svg")]
    let fonts = &mut env.fonts;
    #[cfg(not(feature = "svg"))]
    let fonts = &mut ();
    match super::decode_why(
        &bytes,
        w,
        h,
        super::Sampling::Area,
        &limits,
        &job.cancel,
        fonts,
    ) {
        Ok(bitmap) => Ok(Loaded {
            bitmap: Some(bitmap),
            natural,
            body,
        }),
        Err(Some(Refusal::Memory(n))) if n > 0 => Err(Refusal::Ceiling(n)),
        Err(_) => Err(Refusal::Failed),
    }
}
