//! The Windows halves of the SVG process: the child's job object (its limits, and its end with
//! VOMD's), what a child's failed allocation does, reading the child's pipe without blocking, and
//! what its exit code says. Plain Win32 calls, declared here by hand.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::process::{Child, ChildStdout, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{EXIT_MEMORY, EXIT_SLOW};
use crate::image::svg::Refusal;

type Handle = *mut c_void;

pub(super) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    per_process_user_time: i64,
    per_job_user_time: i64,
    flags: u32,
    min_working_set: usize,
    max_working_set: usize,
    active_processes: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn PeekNamedPipe(
        pipe: Handle,
        buf: *mut c_void,
        len: u32,
        read: *mut u32,
        available: *mut u32,
        left: *mut u32,
    ) -> i32;
    fn SetErrorMode(mode: u32) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn TerminateProcess(process: Handle, code: u32) -> i32;
}

const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const JOB_OBJECT_LIMIT_PROCESS_TIME: u32 = 0x2;
const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x8;
const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x100;
const JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: u32 = 0x400;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const SEM_FAILCRITICALERRORS: u32 = 0x1;
const SEM_NOGPFAULTERRORBOX: u32 = 0x2;
const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;

/// What a process ended by its job's CPU limit exits with (`STATUS_QUOTA_EXCEEDED`).
const ENDED_BY_TIME_LIMIT: u32 = 0xC000_0044;
/// What an abort exits with (a failed allocation outside the child's hook, an overflowed stack's
/// message), and an overflowed stack that got no message.
const STATUS_STACK_BUFFER_OVERRUN: u32 = 0xC000_0409;
const STATUS_STACK_OVERFLOW: u32 = 0xC000_00FD;
const STATUS_NO_MEMORY: u32 = 0xC000_0017;

/// A child's job object: the child is killed when it is closed (dropped), as it is when VOMD ends
/// however it ends.
pub(super) struct Job(Handle);

// SAFETY: a kernel handle, usable from any thread.
unsafe impl Send for Job {}

impl Job {
    /// Put `child` in a job of its own: at most `memory` bytes committed and `cpu` seconds of
    /// CPU, no process of its own started, no error dialog, and killed with the job. `None` if
    /// that couldn't be done.
    pub(super) fn confine(child: &Child, memory: usize, cpu: u64) -> Option<Job> {
        // SAFETY: plain calls with a valid, sized limit structure; the handle is closed by the
        // `Job` from the moment it exists.
        unsafe {
            let job = Job(CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()));
            if job.0.is_null() {
                return None;
            }
            let limits = ExtendedLimits {
                basic: BasicLimits {
                    // In units of 100 ns.
                    per_process_user_time: i64::try_from(cpu.saturating_mul(10_000_000))
                        .unwrap_or(i64::MAX),
                    flags: JOB_OBJECT_LIMIT_PROCESS_TIME
                        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                        | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
                        | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    active_processes: 1,
                    ..Default::default()
                },
                process_memory: memory,
                ..Default::default()
            };
            let set = SetInformationJobObject(
                job.0,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&raw const limits).cast(),
                size_of::<ExtendedLimits>() as u32,
            );
            let assigned = set != 0 && AssignProcessToJobObject(job.0, child.as_raw_handle()) != 0;
            assigned.then_some(job)
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the job's own handle, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Whether the child's pipe has bytes to read, or has ended (a read then says so at once).
pub(super) fn available(out: &ChildStdout) -> bool {
    let mut n = 0u32;
    // SAFETY: a pipe handle we own, and only the count of bytes waiting asked for.
    let ok = unsafe {
        PeekNamedPipe(
            out.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut n,
            std::ptr::null_mut(),
        )
    };
    ok == 0 || n > 0
}

/// Why a child that ended without a result made nothing: its time ran out (its job ends it with
/// `STATUS_QUOTA_EXCEEDED`, or it said so), its memory did (its allocation hook's exit code, or
/// the status of an abort or an overflowed stack), or anything else, broken.
pub(super) fn refusal_of(status: ExitStatus) -> Refusal {
    match status.code().map(|c| c as u32) {
        Some(c) if c == EXIT_SLOW as u32 || c == ENDED_BY_TIME_LIMIT => Refusal::Slow,
        Some(c)
            if c == EXIT_MEMORY as u32
                || matches!(
                    c,
                    STATUS_STACK_BUFFER_OVERRUN | STATUS_STACK_OVERFLOW | STATUS_NO_MEMORY
                ) =>
        {
            Refusal::Memory
        }
        _ => Refusal::Broken,
    }
}

/// Set in the child: a failed allocation ends it with [`EXIT_MEMORY`].
static CHILD: AtomicBool = AtomicBool::new(false);

/// The child's own setup: no error dialogs, and a failed allocation (the job's memory limit) ends
/// it with [`EXIT_MEMORY`] instead of an abort, which Windows reports as a crash.
pub(super) fn child_setup() {
    // SAFETY: a plain call.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)
    };
    CHILD.store(true, Ordering::Relaxed);
}

/// The system's allocator, but in the child a null (a failed allocation) ends the process with
/// [`EXIT_MEMORY`] there and then: `TerminateProcess` allocates nothing. VOMD's own process is
/// unchanged (one load of a flag after an allocation fails). The binary's global allocator
/// (`main.rs`): the child is always the binary, and a test binary may count with its own.
pub struct Allocator;

impl Allocator {
    fn check(p: *mut u8) -> *mut u8 {
        if p.is_null() && CHILD.load(Ordering::Relaxed) {
            // Not `ExitProcess`, which runs the libraries' detach code after stopping the other
            // threads, and could wait for a lock one of them held (the heap's) forever.
            // SAFETY: ends this process at once; nothing after it runs.
            unsafe {
                TerminateProcess(GetCurrentProcess(), EXIT_MEMORY as u32);
            }
            std::process::abort();
        }
        p
    }
}

// SAFETY: every call is the system allocator's, with its own arguments and results.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::check(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::check(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::check(unsafe { System.realloc(ptr, layout, new_size) })
    }
}
