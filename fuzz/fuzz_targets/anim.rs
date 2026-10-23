//! Bytes → an animation's index and 64 of its frames drawn: never panics, never more allocated
//! than playing it is charged, and the canvas and what changes of it are inside its size. Counted by an allocator of its own.
#![no_main]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use libfuzzer_sys::fuzz_target;
use vomd::image::play_frames;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            PEAK.fetch_max(LIVE.fetch_add(l.size(), Relaxed) + l.size(), Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, size) };
        // What it grows or shrinks by, as `tests/image_memory.rs` counts it.
        if !q.is_null() && size >= l.size() {
            let grown = size - l.size();
            PEAK.fetch_max(LIVE.fetch_add(grown, Relaxed) + grown, Relaxed);
        } else if !q.is_null() {
            LIVE.fetch_sub(l.size() - size, Relaxed);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

fuzz_target!(|data: &[u8]| {
    let start = LIVE.load(Relaxed);
    PEAK.store(start, Relaxed);
    // The canvas's length, and how far right and down a frame's changes reach.
    let (mut len, mut reach) = (None, (0, 0));
    // What one animation may hold of the cache (8 MiB, a quarter): larger ones (`big-anim.webp`
    // holds 15 MB) only slow the runs down.
    let Some(((w, h), held)) = play_frames(data, 64, 8 << 20, |_, px, [x, y, dw, dh]| {
        len = Some(px.len());
        if dw > 0 && dh > 0 {
            reach = (reach.0.max(x + dw), reach.1.max(y + dh));
        }
    }) else {
        return;
    };
    let peak = PEAK.load(Relaxed) - start;
    assert!(peak <= held, "{peak} bytes allocated, {held} charged");
    if let Some(n) = len {
        assert_eq!(n, w as usize * h as usize);
    }
    assert!(
        reach.0 <= w && reach.1 <= h,
        "changes at {reach:?} of {w} × {h}"
    );
});
