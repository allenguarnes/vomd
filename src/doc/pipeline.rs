//! Ordered parallel work for segmented parsing.
//!
//! Parsing a segment with pulldown-cmark is by far the most expensive part of reading a large
//! document, and segments are independent, while building the document from their events has to
//! go in order. So a few threads (the caller among them) each take the next segment, parse it,
//! and then wait for their turn to feed the events to the builder, which one thread does at a
//! time, strictly in segment order. Nothing but the builder is shared: a segment's events never
//! leave the thread that parsed them, so they may borrow that thread's own data.
//!
//! Threads only take segments `done .. done + window`, where `done` is how many have had their
//! turn, so however slow the builder is, at most `window` segments' events exist at once (being
//! made, or waiting for their turn).
//!
//! There is no deadlock: the segment whose turn it is, `done`, is always in the window, so a
//! thread has it (or takes it when it is free: threads wait for the window only if every
//! segment in it is taken), and a thread waits for its turn only while an earlier segment is on
//! its way. A thread that panics wakes every other up, and the panic is resumed in the caller.

use std::any::Any;
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

/// Most threads spawned on top of the caller, which works too.
const MAX_WORKERS: usize = 3;

/// How to run the pipeline.
pub(super) struct Config {
    /// Threads to spawn besides the caller; 0 means no pipeline (the caller parses and builds
    /// as it goes).
    pub workers: usize,
    /// Most segments in flight at once (at least 1).
    pub window: usize,
    /// The most segments that were in flight at once, over all runs with this config.
    #[cfg(test)]
    peak: AtomicUsize,
}

impl Config {
    pub fn new(workers: usize, window: usize) -> Config {
        Config {
            workers,
            window: window.max(1),
            #[cfg(test)]
            peak: AtomicUsize::new(0),
        }
    }

    /// One thread per spare core, up to [`MAX_WORKERS`], and a segment more in flight than
    /// threads, so that a slow segment doesn't leave the others with nothing to do.
    pub fn detect() -> Config {
        static CORES: OnceLock<usize> = OnceLock::new();
        let cores =
            *CORES.get_or_init(|| std::thread::available_parallelism().map_or(1, usize::from));
        let workers = (cores - 1).min(MAX_WORKERS);
        Config::new(workers, workers + 2)
    }

    #[cfg(test)]
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }
}

struct Gate {
    /// The next segment to hand out.
    next: usize,
    /// Segments that have had their turn (a turn that broke counts).
    done: usize,
    /// Whether some turn broke: the run failed, even if every segment has had its turn.
    broke: bool,
    stop: bool,
    /// The most segments in flight.
    peak: usize,
    /// The first panic of a thread.
    panic: Option<Box<dyn Any + Send>>,
}

struct Shared<S> {
    gate: Mutex<Gate>,
    wake: Condvar,
    total: usize,
    window: usize,
    /// What the turns work on.
    state: Mutex<S>,
}

fn relock<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    // Nothing that could panic runs under the gate's lock, and a panic under the state's lock
    // stops everything anyway.
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<S> Shared<S> {
    /// The next segment to work on, once it is in the window; `None` when there are no more or
    /// everything has stopped.
    fn claim(&self) -> Option<usize> {
        let mut gate = relock(&self.gate);
        loop {
            if gate.stop || gate.next >= self.total {
                return None;
            }
            if gate.next < gate.done + self.window {
                let n = gate.next;
                gate.next += 1;
                gate.peak = gate.peak.max(gate.next - gate.done);
                return Some(n);
            }
            gate = self.wake.wait(gate).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn stop(&self, panic: Option<Box<dyn Any + Send>>) {
        let mut gate = relock(&self.gate);
        gate.stop = true;
        if gate.panic.is_none() {
            gate.panic = panic;
        }
        drop(gate);
        self.wake.notify_all();
    }

    fn run(&self, work: &(impl Fn(usize, &mut Turn<'_, S>) + Sync)) {
        while let Some(n) = self.claim() {
            let mut turn = Turn {
                shared: self,
                n,
                taken: false,
            };
            let result = catch_unwind(AssertUnwindSafe(|| {
                work(n, &mut turn);
                assert!(turn.taken, "a segment's work must take its turn");
            }));
            if let Err(panic) = result {
                self.stop(Some(panic));
                return;
            }
        }
    }
}

/// A segment's place in the order.
pub(super) struct Turn<'a, S> {
    shared: &'a Shared<S>,
    n: usize,
    taken: bool,
}

impl<S> Turn<'_, S> {
    /// Waits until every earlier segment has had its turn, then runs `f` on the state, alone.
    /// Must be called once, when the segment's work is done. If `f` breaks, everything stops
    /// (and so `f` doesn't run if something else stopped before).
    pub fn take(&mut self, f: impl FnOnce(&mut S) -> ControlFlow<()>) {
        assert!(!self.taken, "a segment takes its turn once");
        self.taken = true;
        let shared = self.shared;
        let mut gate = relock(&shared.gate);
        while !gate.stop && gate.done != self.n {
            gate = shared
                .wake
                .wait(gate)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if gate.stop {
            return;
        }
        drop(gate);
        let flow = f(&mut relock(&shared.state));
        let mut gate = relock(&shared.gate);
        gate.done += 1;
        gate.broke |= flow.is_break();
        gate.stop |= gate.broke;
        drop(gate);
        shared.wake.notify_all();
    }
}

/// Calls `work(n, turn)` for `n` in `0..total` on this thread and up to `config.workers` more,
/// each of which calls `turn.take(f)` once; the `f`s run one at a time on `state`, in the order
/// of `n`. Returns the state and whether every segment had its turn (not if an `f` broke). A
/// panic in `work` or an `f` is resumed here. At most `config.window` segments are in flight
/// at any time, from the start of `work` to the end of `f`.
pub(super) fn ordered<S: Send>(
    total: usize,
    config: &Config,
    state: S,
    work: impl Fn(usize, &mut Turn<'_, S>) + Sync,
) -> (S, bool) {
    let shared = Shared {
        gate: Mutex::new(Gate {
            next: 0,
            done: 0,
            broke: false,
            stop: false,
            peak: 0,
            panic: None,
        }),
        wake: Condvar::new(),
        total,
        window: config.window,
        state: Mutex::new(state),
    };
    std::thread::scope(|scope| {
        for _ in 0..config.workers.min(total.saturating_sub(1)) {
            scope.spawn(|| shared.run(&work));
        }
        shared.run(&work);
    });
    let mut gate = relock(&shared.gate);
    if let Some(panic) = gate.panic.take() {
        resume_unwind(panic);
    }
    #[cfg(test)]
    config.peak.fetch_max(gate.peak, Ordering::Relaxed);
    // A break in the last segment still counts as its turn, so `done == total` isn't enough.
    let completed = gate.done == total && !gate.broke;
    drop(gate);
    let state = shared
        .state
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    (state, completed)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    /// Runs `f` on a thread and fails the test if it hasn't finished in time (a deadlock).
    fn within<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(catch_unwind(AssertUnwindSafe(f)));
        });
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(result)) => result,
            Ok(Err(panic)) => resume_unwind(panic),
            Err(_) => panic!("the pipeline hangs"),
        }
    }

    fn pause(micros: u64) {
        std::thread::sleep(Duration::from_micros(micros));
    }

    #[test]
    fn segments_have_their_turns_in_order() {
        for (workers, window) in [(1, 1), (1, 4), (3, 1), (3, 2), (3, 5), (7, 8), (2, 100)] {
            let order = within(move || {
                let config = Config::new(workers, window);
                // Work that takes uneven times, so segments finish out of order.
                let (order, done) = ordered(40, &config, Vec::new(), |n, turn| {
                    pause((n as u64 * 7919 % 13) * 200);
                    turn.take(|order| {
                        order.push(n);
                        ControlFlow::Continue(())
                    });
                });
                assert!(done);
                assert!(config.peak() <= window, "{} in flight", config.peak());
                order
            });
            assert_eq!(order, (0..40).collect::<Vec<_>>(), "{workers} × {window}");
        }
    }

    #[test]
    fn segments_in_flight_stay_within_the_window() {
        /// Counts how many exist.
        struct Live<'a>(&'a AtomicUsize);
        impl<'a> Live<'a> {
            fn new(live: &'a AtomicUsize, high: &AtomicUsize) -> Self {
                high.fetch_max(live.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                Live(live)
            }
        }
        impl Drop for Live<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        for (workers, window) in [(3, 1), (3, 2), (3, 3), (3, 4), (2, 6)] {
            let (live, high) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let config = Config::new(workers, window);
            let live = &live;
            let (_, done) = ordered(30, &config, (), |n, turn| {
                let segment = Live::new(live, &high);
                // A slow builder: the first turn lasts until the other threads have run ahead as
                // far as they can (or give up waiting for that).
                turn.take(move |()| {
                    let end = std::time::Instant::now() + Duration::from_secs(10);
                    while n == 0
                        && live.load(Ordering::SeqCst) < window.min(workers + 1)
                        && std::time::Instant::now() < end
                    {
                        pause(100);
                    }
                    drop(segment);
                    ControlFlow::Continue(())
                });
            });
            assert!(done);
            assert_eq!(live.load(Ordering::SeqCst), 0);
            let high = high.load(Ordering::SeqCst);
            assert!(high <= window, "{high} in flight, window {window}");
            // The window is used up when there are threads to fill it.
            assert_eq!(high, window.min(workers + 1), "{workers} × {window}");
            assert_eq!(config.peak(), high);
        }
    }

    #[test]
    fn a_break_stops_the_rest() {
        for (workers, window) in [(1, 1), (3, 2), (3, 5)] {
            let taken = within(move || {
                let started = AtomicUsize::new(0);
                let config = Config::new(workers, window);
                let (taken, done) = ordered(200, &config, Vec::new(), |n, turn| {
                    started.fetch_add(1, Ordering::SeqCst);
                    pause(100);
                    turn.take(|taken| {
                        taken.push(n);
                        if n == 3 {
                            ControlFlow::Break(())
                        } else {
                            ControlFlow::Continue(())
                        }
                    });
                });
                assert!(!done);
                // Nothing ran far past the break: only what the window let through.
                assert!(started.load(Ordering::SeqCst) <= 4 + window);
                taken
            });
            assert_eq!(taken, [0, 1, 2, 3], "{workers} × {window}");
        }
    }

    #[test]
    fn a_break_in_any_turn_fails_the_run_even_the_last() {
        const TOTAL: usize = 12;
        for workers in [0, 1, 3] {
            for window in [1, 2, 5] {
                for bad in [0, 6, TOTAL - 1] {
                    let (taken, done) = within(move || {
                        let config = Config::new(workers, window);
                        ordered(TOTAL, &config, Vec::new(), |n, turn| {
                            pause((n as u64 * 31 % 7) * 100);
                            turn.take(|taken| {
                                taken.push(n);
                                if n == bad {
                                    ControlFlow::Break(())
                                } else {
                                    ControlFlow::Continue(())
                                }
                            });
                        })
                    });
                    let name = format!("{workers} × {window}, break at {bad}");
                    assert!(!done, "{name}");
                    assert_eq!(taken, (0..=bad).collect::<Vec<_>>(), "{name}");
                }
            }
        }
    }

    #[test]
    fn a_panic_in_any_thread_is_resumed_and_nothing_hangs() {
        for (workers, window) in [(1, 1), (3, 1), (3, 2), (3, 6)] {
            for (bad, in_turn) in [(0, false), (5, false), (5, true), (29, false), (29, true)] {
                let result = within(move || {
                    catch_unwind(AssertUnwindSafe(|| {
                        let config = Config::new(workers, window);
                        ordered(30, &config, (), |n, turn| {
                            pause((n as u64 * 31 % 7) * 100);
                            if n == bad && !in_turn {
                                panic!("boom {n}");
                            }
                            turn.take(|()| {
                                if n == bad {
                                    panic!("boom {n}");
                                }
                                ControlFlow::Continue(())
                            });
                        })
                    }))
                    .map_err(|p| p.downcast_ref::<String>().cloned())
                });
                let message = result.expect_err("the panic is resumed");
                assert_eq!(message.as_deref(), Some(&*format!("boom {bad}")));
            }
        }
    }

    #[test]
    fn work_that_skips_its_turn_is_a_bug_that_stops_everything() {
        let result = within(|| {
            catch_unwind(AssertUnwindSafe(|| {
                ordered(10, &Config::new(2, 3), (), |n, turn| {
                    if n != 4 {
                        turn.take(|()| ControlFlow::Continue(()));
                    }
                })
            }))
            .is_err()
        });
        assert!(result);
    }

    #[test]
    fn few_segments_and_no_segments_work() {
        for total in [0, 1, 2] {
            let (n, done) = ordered(total, &Config::new(3, 5), 0, |_, turn| {
                turn.take(|n| {
                    *n += 1;
                    ControlFlow::Continue(())
                })
            });
            assert!(done);
            assert_eq!(n, total);
        }
    }
}
