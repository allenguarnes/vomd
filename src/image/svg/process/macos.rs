//! The macOS halves of the SVG process. macOS enforces neither `RLIMIT_DATA` nor `RLIMIT_AS` on
//! what malloc maps, so the child counts its own memory: the binary's allocator ([`Allocator`])
//! adds up what is live once the child has set its charge, and ends it with [`EXIT_MEMORY`] past
//! it. There is no `PR_SET_PDEATHSIG` either: a thread of the child looks at its parent every
//! [`PARENT_POLL`] and ends it once the parent is gone (`RLIMIT_CPU`, which macOS enforces, ends
//! an orphan anyway). And a running binary there can be replaced (a new VOMD.app copied over the
//! old one): a child is only started from the file VOMD was started from ([`same_program`]).

use std::alloc::{GlobalAlloc, Layout, System};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;

use super::{EXIT_INPUT, EXIT_MEMORY};

/// How often the child looks for its parent.
const PARENT_POLL: Duration = Duration::from_millis(100);

/// Set in the child once its charge is known: allocations are counted from then on.
static CHILD: AtomicBool = AtomicBool::new(false);
/// The bytes the child may have live, and what it has (counted from when [`CHILD`] was set: a
/// block from before that, freed, takes its size off, which only leaves more room).
static CHARGE: AtomicIsize = AtomicIsize::new(isize::MAX);
static LIVE: AtomicIsize = AtomicIsize::new(0);

/// The child's memory is `data` bytes at the most from now on.
pub(super) fn limit_memory(data: u64) {
    CHARGE.store(
        isize::try_from(data).unwrap_or(isize::MAX),
        Ordering::Relaxed,
    );
    CHILD.store(true, Ordering::Relaxed);
}

/// End the child once `parent` is gone (it is then the child of another, launchd).
pub(super) fn watch_parent(parent: u64) {
    std::thread::spawn(move || {
        loop {
            // SAFETY: `getppid` can't fail.
            if u64::try_from(unsafe { super::getppid() }) != Ok(parent) {
                // SAFETY: ends this process at once.
                unsafe { super::_exit(EXIT_INPUT) };
            }
            std::thread::sleep(PARENT_POLL);
        }
    });
}

/// A file's identity (see [`identity`]).
type Identity = (u64, u64, u64, i64, i64);

/// The file's identity: its device, inode, length and time of modification (a file copied over
/// the old one in place keeps its inode, and changes the last two; the time of change isn't used:
/// an extended attribute, such as Gatekeeper's quarantine one, moves it).
fn identity(program: &Path) -> Option<Identity> {
    let m = std::fs::metadata(program).ok()?;
    Some((m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
}

/// The program's identity when SVGs were first drawn apart (VOMD's start).
static STARTED: OnceLock<Option<Identity>> = OnceLock::new();

/// Note what `program` is now (once: the first stays), for [`same_program`].
pub(super) fn note_program(program: &Path) {
    let _ = STARTED.set(identity(program));
}

/// Whether `program` is still the file VOMD was started from (the protocol has no version: a
/// child of another build could misread the job). One that couldn't be looked at then is tried.
pub(super) fn same_program(program: &Path) -> bool {
    match STARTED.get() {
        Some(Some(started)) => identity(program).as_ref() == Some(started),
        _ => true,
    }
}

/// The system's allocator, but in the child it counts the bytes live, and an allocation past the
/// charge (or one that fails) ends the process with [`EXIT_MEMORY`] there and then (`_exit`
/// allocates nothing). VOMD's own process pays one load of a flag per allocation. The binary's
/// global allocator (`main.rs`): the child is always the binary.
pub struct Allocator;

impl Allocator {
    /// Count `grow` bytes more live (fewer if negative); past the charge, end the child.
    fn count(grow: isize) {
        let live = LIVE.fetch_add(grow, Ordering::Relaxed) + grow;
        if grow > 0 && live > CHARGE.load(Ordering::Relaxed) {
            // SAFETY: ends this process at once; nothing after it runs.
            unsafe { super::_exit(EXIT_MEMORY) };
        }
    }

    fn check(p: *mut u8) -> *mut u8 {
        if p.is_null() && CHILD.load(Ordering::Relaxed) {
            // SAFETY: as above.
            unsafe { super::_exit(EXIT_MEMORY) };
        }
        p
    }
}

// SAFETY: every call is the system allocator's, with its own arguments and results; the counts
// only look on.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if CHILD.load(Ordering::Relaxed) {
            Self::count(layout.size() as isize);
        }
        Self::check(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if CHILD.load(Ordering::Relaxed) {
            Self::count(layout.size() as isize);
        }
        Self::check(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if CHILD.load(Ordering::Relaxed) {
            Self::count(-(layout.size() as isize));
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if CHILD.load(Ordering::Relaxed) {
            Self::count(new_size as isize - layout.size() as isize);
        }
        Self::check(unsafe { System.realloc(ptr, layout, new_size) })
    }
}
