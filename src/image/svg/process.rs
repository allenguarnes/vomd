//! An SVG drawn in a process of its own: VOMD's own binary, started with [`ARG`], which reads the
//! job on its stdin, draws it as [`render_in`] does, writes what it made on its stdout, and exits.
//!
//! usvg's parse and parts of resvg can't be stopped once started, and what they do grows faster
//! than the counts before the parse can always say: in a process, a drawing that takes too long
//! (the deadline), isn't wanted any more (cancelled: its document or box changed, or it was given
//! up) or outlives VOMD is killed, and nothing of it is left in VOMD. On Linux the child dies with
//! the thread that started it (`PR_SET_PDEATHSIG`), and has hard limits of its own as a backstop
//! (`RLIMIT_DATA` about its charge, `RLIMIT_CPU`): one stopped by them is refused for memory or
//! time, never a crash of VOMD's. It ends by its own exit code then, not by a signal that would
//! dump core (the system's crash reports would name VOMD): the signals the limits end a process
//! with are caught, and no core is dumped for any other. On Windows a job object of its own does
//! the same: the child is killed when VOMD closes the job (as it does when it ends, however it
//! ends), its committed memory and its CPU time are capped, and it can start no process; a
//! failed allocation in it exits with the memory code instead of aborting (which would be a crash
//! for Windows Error Reporting).
//!
//! The child opens no display, reads no settings and fetches nothing: it reads the job, and the
//! font files the job names (the faces VOMD's own text is set in), and nothing else, as SVGs drawn
//! in VOMD did. The protocol needs no version: the child is the same file as the parent
//! (`/proc/self/exe`; on Windows the running executable, which can't be replaced while it runs).
//!
//! The job, little-endian: the box (`u32` × 2), the charge (`u64`), forced (`u8`) and the forced
//! load's memory (`u64`), a test mode (`u8`, [`set_test`]), the SVG's record (`SvgCost`), the
//! faces (none, or the regular and mono faces' places and each face's index and origin: an
//! embedded face's place, or a file's path), and the SVG's bytes. The result: pixels (whether
//! embedded images were left out, the box, and `0xAARRGGBB` per pixel), what it needs (an
//! `SvgCost`), or why it is refused.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::{Caps, Fonts, Refusal, Rendered, render_in};
use crate::fonts::{FontOrigin, SvgFonts};
use crate::image::{Bitmap, Limits, SvgCost, cancelled};

/// The argument that makes `vomd` the drawing process: before anything else in `main`.
pub const ARG: &str = "--draw-svg";

/// What the child may hold beyond its charge, its input and its stack: the binary's own data,
/// the allocator's slack, the font database's.
const HEADROOM: usize = 64 << 20;
/// The child's drawing thread's stack: the decode worker's, or the forced thread's (an SVG may be
/// 1,024 elements deep then).
const STACK: usize = 8 << 20;
const FORCED_STACK: usize = 64 << 20;
/// Seconds of CPU the child may use: well past the deadline (it is killed at that), and for a
/// forced load (which has none) a backstop.
const CPU_SECONDS: u64 = 5;
const FORCED_CPU_SECONDS: u64 = 120;
/// How often the parent looks at the job's cancel flag while it waits.
const POLL_MS: i32 = 10;

/// What the child exits with when it made no result: its arguments or input are bad (it was not
/// started by VOMD), its parent is gone, or it panicked; or a limit stopped it: its CPU time ran
/// out, or its memory did (an allocation failed, which aborts, as does an overflowed stack).
const EXIT_INPUT: i32 = 3;
const EXIT_PANIC: i32 = 4;
const EXIT_SLOW: i32 = 5;
const EXIT_MEMORY: i32 = 6;

/// The binary SVGs are drawn by (unset: they are drawn in VOMD's own process).
static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
/// What every child is told to do instead of drawing, for the tests: 0 draw, [`SPIN`], [`EAT`].
static TEST: AtomicU8 = AtomicU8::new(0);
/// Spin forever; allocate until its limit stops it.
pub const SPIN: u8 = 1;
pub const EAT: u8 = 2;
/// The seconds of CPU every child gets instead of [`CPU_SECONDS`] and [`FORCED_CPU_SECONDS`], for
/// the tests (0: those).
static CPU_TEST: AtomicU64 = AtomicU64::new(0);
/// The children running now.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Draw SVGs with `program` (VOMD's own binary) from now on. Once: the first one stays.
pub fn set_program(program: PathBuf) {
    let _ = PROGRAM.set(program);
}

/// The binary SVGs are drawn by, if they are drawn apart.
pub(crate) fn program() -> Option<&'static Path> {
    PROGRAM.get().map(PathBuf::as_path)
}

/// Make the children do `mode` ([`SPIN`], [`EAT`]) instead of drawing, or draw (0): for tests.
pub fn set_test(mode: u8) {
    TEST.store(mode, Ordering::Relaxed);
}

/// Give every child `seconds` of CPU instead of the usual (0: the usual), for tests.
pub fn set_cpu_limit(seconds: u64) {
    CPU_TEST.store(seconds, Ordering::Relaxed);
}

/// How many children are running, for tests.
pub fn running() -> usize {
    RUNNING.load(Ordering::SeqCst)
}

// ---- The parent -----------------------------------------------------------------------------

/// [`render_in`] in a child run by `program`, killed once `cancel` is set. A child that can't
/// be started (no `/proc`, no processes left) leaves the SVG to be drawn here, and `inline` says
/// so (it is set while the drawing is VOMD's own, and cleared when a child draws it): the drawing
/// can't be killed then, and the SVGs after it wait for it as they do when SVGs are not drawn
/// apart. (It is set before the start is tried: a drawing given up during the start is taken for
/// VOMD's own, and if a child is started after all it sees the cancel and ends at once, which
/// frees the worker.)
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    program: &Path,
    bytes: &[u8],
    svg: &SvgCost,
    (w, h): (u32, u32),
    charge: usize,
    limits: &Limits,
    cancel: &AtomicBool,
    inline: &AtomicBool,
    fonts: &mut Fonts,
) -> Rendered {
    let (stack, cpu) = if limits.forced {
        (FORCED_STACK, FORCED_CPU_SECONDS)
    } else {
        (STACK, CPU_SECONDS)
    };
    let cpu = match CPU_TEST.load(Ordering::Relaxed) {
        0 => cpu,
        seconds => seconds,
    };
    inline.store(true, Ordering::SeqCst);
    let data = (bytes.len())
        .saturating_add(charge)
        .saturating_add(stack)
        .saturating_add(HEADROOM);
    let mut command = Command::new(program);
    command
        .arg(ARG)
        .arg(std::process::id().to_string())
        .arg(data.to_string())
        .arg(cpu.to_string())
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // (VOMD has no console, but a test's binary may: the child gets none either way.)
        command.creation_flags(windows::CREATE_NO_WINDOW);
    }
    let Ok(child) = command.spawn() else {
        let caps = Caps::of(limits);
        return render_in(bytes, svg, (w, h), charge, &caps, cancel, fonts);
    };
    inline.store(false, Ordering::SeqCst);
    // The child reads its job before it does anything: it is in its job object before then, and
    // if it couldn't be put in one it is still killed at the deadline or when cancelled.
    #[cfg(windows)]
    let job = windows::Job::confine(&child, data, cpu);
    let mut child = Running::new(child);
    #[cfg(windows)]
    {
        child.1 = job;
    }
    let mut job = Vec::new();
    put_job(
        &mut job,
        svg,
        (w, h),
        charge,
        limits,
        fonts.sources.as_deref(),
    );
    job.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    // (A child gone already fails the writes: its status says why.)
    if let Some(mut stdin) = child.0.stdin.take() {
        let _ = stdin.write_all(&job).and_then(|()| stdin.write_all(bytes));
    }
    drop(job);
    let Some(mut out) = child.0.stdout.take() else {
        return Rendered::Refused(Refusal::Broken);
    };
    match read_result(&mut out, (w, h), cancel) {
        Ok(rendered) => rendered,
        Err(Stop::Cancelled) => Rendered::Cancelled,
        Err(Stop::Ended) => match child.status(cancel) {
            None => Rendered::Cancelled,
            Some(status) => Rendered::Refused(refusal_of(status)),
        },
    }
}

/// A child, killed and waited for when this goes, whatever it is doing: none is left behind. On
/// Windows, with its job object (closed after it).
struct Running(Child, #[cfg(windows)] Option<windows::Job>);

impl Running {
    fn new(child: Child) -> Running {
        RUNNING.fetch_add(1, Ordering::SeqCst);
        Running(
            child,
            #[cfg(windows)]
            None,
        )
    }

    /// How the child ended (it closed its stdout), or `None` if `cancel` was set first.
    fn status(&mut self, cancel: &AtomicBool) -> Option<ExitStatus> {
        loop {
            if let Ok(Some(status)) = self.0.try_wait() {
                return Some(status);
            }
            if cancelled(cancel) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(POLL_MS as u64));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Why a child that ended without a result made nothing: its time ran out (`RLIMIT_CPU`), its
/// memory did (an allocation failed, and the process aborted) or it was killed (the system's
/// out-of-memory killer), or anything else, broken. The child says which of the first two by its
/// exit code; the signals are for one that died before it could (or was killed past its hard
/// limit).
#[cfg(unix)]
fn refusal_of(status: ExitStatus) -> Refusal {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(EXIT_SLOW), _) | (_, Some(SIGXCPU)) => Refusal::Slow,
        (Some(EXIT_MEMORY), _) | (_, Some(SIGABRT | SIGKILL)) => Refusal::Memory,
        _ => Refusal::Broken,
    }
}

/// Why the parent stopped reading.
enum Stop {
    /// The child closed its stdout before its result was whole.
    Ended,
    Cancelled,
}

/// The result the child writes on `out`, for a box of `size`.
fn read_result(
    out: &mut ChildStdout,
    size: (u32, u32),
    cancel: &AtomicBool,
) -> Result<Rendered, Stop> {
    let mut kind = [0u8; 1];
    fill(out, &mut kind, cancel)?;
    match kind[0] {
        PIXELS => {
            let mut head = [0u8; 9];
            fill(out, &mut head, cancel)?;
            let skipped = head[0] != 0;
            let width = u32::from_le_bytes(head[1..5].try_into().unwrap());
            let height = u32::from_le_bytes(head[5..9].try_into().unwrap());
            if (width, height) != size {
                return Ok(Rendered::Refused(Refusal::Broken));
            }
            let mut left = width as usize * height as usize;
            let mut data = Vec::with_capacity(left);
            let mut chunk = [0u8; 16 << 10];
            while left > 0 {
                let n = left.min(chunk.len() / 4);
                fill(out, &mut chunk[..n * 4], cancel)?;
                let (words, _) = chunk[..n * 4].as_chunks::<4>();
                data.extend(words.iter().map(|&b| u32::from_le_bytes(b)));
                left -= n;
            }
            let bitmap = Bitmap {
                width,
                height,
                data,
            };
            Ok(Rendered::Pixels(bitmap, skipped))
        }
        NEED => {
            let mut b = [0u8; COST_BYTES];
            fill(out, &mut b, cancel)?;
            Ok(Rendered::Need(get_cost(&b)))
        }
        REFUSED => {
            let mut why = [0u8; 1];
            fill(out, &mut why, cancel)?;
            Ok(Rendered::Refused(match why[0] {
                1 => Refusal::Complex,
                2 => Refusal::Slow,
                3 => Refusal::Memory,
                _ => Refusal::Broken,
            }))
        }
        _ => Ok(Rendered::Refused(Refusal::Broken)),
    }
}

/// Read `buf` whole from `out`, looking at `cancel` every [`POLL_MS`] while nothing comes.
fn fill(out: &mut ChildStdout, buf: &mut [u8], cancel: &AtomicBool) -> Result<(), Stop> {
    let mut at = 0;
    while at < buf.len() {
        if cancelled(cancel) {
            return Err(Stop::Cancelled);
        }
        if !readable(out, POLL_MS) {
            continue;
        }
        match out.read(&mut buf[at..]) {
            Ok(0) => return Err(Stop::Ended),
            Ok(n) => at += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(Stop::Ended),
        }
    }
    Ok(())
}

// ---- The protocol ---------------------------------------------------------------------------

/// The result's kinds.
const PIXELS: u8 = 0;
const NEED: u8 = 1;
const REFUSED: u8 = 2;

/// Bytes of an `SvgCost` written.
const COST_BYTES: usize = 4 + 1 + 8 + 8 + 4 + 4 + 8;

/// The kinds of a face's origin.
const EMBEDDED: u8 = 0;
const FILE: u8 = 1;

fn put_cost(out: &mut Vec<u8>, c: &SvgCost) {
    out.extend_from_slice(&c.xml.to_le_bytes());
    out.push(u8::from(c.gz));
    out.extend_from_slice(&(c.parse as u64).to_le_bytes());
    out.extend_from_slice(&(c.draw as u64).to_le_bytes());
    out.extend_from_slice(&c.layers.to_le_bytes());
    out.extend_from_slice(&c.pixmaps.to_le_bytes());
    out.extend_from_slice(&(c.raster as u64).to_le_bytes());
}

fn get_cost(b: &[u8; COST_BYTES]) -> SvgCost {
    let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let usize_at = |i: usize| {
        let n = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        usize::try_from(n).unwrap_or(usize::MAX)
    };
    SvgCost {
        xml: u32_at(0),
        gz: b[4] != 0,
        parse: usize_at(5),
        draw: usize_at(13),
        layers: u32_at(21),
        pixmaps: u32_at(25),
        raster: usize_at(29),
    }
}

/// The job, but the SVG's bytes and their length.
fn put_job(
    out: &mut Vec<u8>,
    svg: &SvgCost,
    (w, h): (u32, u32),
    charge: usize,
    limits: &Limits,
    fonts: Option<&SvgFonts>,
) {
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&(charge as u64).to_le_bytes());
    out.push(u8::from(limits.forced));
    out.extend_from_slice(&(limits.memory as u64).to_le_bytes());
    out.push(TEST.load(Ordering::Relaxed));
    put_cost(out, svg);
    let Some(fonts) = fonts else {
        out.push(0);
        return;
    };
    out.push(1);
    out.extend_from_slice(&(fonts.sans as u32).to_le_bytes());
    out.extend_from_slice(&(fonts.mono as u32).to_le_bytes());
    out.extend_from_slice(&(fonts.sources.len() as u32).to_le_bytes());
    for (i, (_, index)) in fonts.sources.iter().enumerate() {
        out.extend_from_slice(&index.to_le_bytes());
        match fonts.origins.get(i) {
            Some(FontOrigin::File(path)) => {
                let path = path_bytes(path);
                out.push(FILE);
                out.extend_from_slice(&(path.len() as u32).to_le_bytes());
                out.extend_from_slice(&path);
            }
            Some(FontOrigin::Embedded(at)) => {
                out.push(EMBEDDED);
                out.extend_from_slice(&(*at as u64).to_le_bytes());
            }
            None => {
                out.push(EMBEDDED);
                out.extend_from_slice(&u64::MAX.to_le_bytes());
            }
        }
    }
}

/// What the child reads its job from.
struct Input<R: Read>(R);

impl<R: Read> Input<R> {
    fn bytes<const N: usize>(&mut self) -> Option<[u8; N]> {
        let mut b = [0u8; N];
        self.0.read_exact(&mut b).ok()?;
        Some(b)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes::<1>()?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes()?))
    }

    fn usize(&mut self) -> Option<usize> {
        usize::try_from(u64::from_le_bytes(self.bytes()?)).ok()
    }

    fn vec(&mut self, len: usize) -> Option<Vec<u8>> {
        let mut v = Vec::new();
        v.try_reserve_exact(len).ok()?;
        (&mut self.0).take(len as u64).read_to_end(&mut v).ok()?;
        (v.len() == len).then_some(v)
    }
}

/// A job as the child reads it.
struct Job {
    size: (u32, u32),
    charge: usize,
    limits: Limits,
    test: u8,
    svg: SvgCost,
    fonts: Option<SvgFonts>,
    bytes: Vec<u8>,
}

fn get_job(r: &mut Input<impl Read>) -> Option<Job> {
    let size = (r.u32()?, r.u32()?);
    let charge = r.usize()?;
    let forced = r.u8()? != 0;
    let memory = r.usize()?;
    let limits = if forced {
        Limits::forced(memory)
    } else {
        Limits::normal()
    };
    let test = r.u8()?;
    let svg = get_cost(&r.bytes()?);
    let fonts = match r.u8()? {
        0 => None,
        _ => {
            let (sans, mono, n) = (r.u32()? as usize, r.u32()? as usize, r.u32()?);
            let mut fonts = SvgFonts {
                sources: Vec::new(),
                origins: Vec::new(),
                sans: 0,
                mono: 0,
            };
            for i in 0..n as usize {
                let index = r.u32()?;
                let origin = match r.u8()? {
                    FILE => {
                        let len = r.u32()? as usize;
                        FontOrigin::File(path_of(r.vec(len)?)?)
                    }
                    _ => FontOrigin::Embedded(r.usize().unwrap_or(usize::MAX)),
                };
                // A face that can't be had is left out (the places of the others move up).
                let Some(data) = crate::fonts::font_data(&origin) else {
                    continue;
                };
                if i == sans {
                    fonts.sans = fonts.sources.len();
                }
                if i == mono {
                    fonts.mono = fonts.sources.len();
                }
                fonts.sources.push((data, index));
                fonts.origins.push(origin);
            }
            Some(fonts)
        }
    };
    let len = r.usize()?;
    let bytes = r.vec(len)?;
    Some(Job {
        size,
        charge,
        limits,
        test,
        svg,
        fonts,
        bytes,
    })
}

// ---- The child ------------------------------------------------------------------------------

/// If this process was started to draw an SVG ([`ARG`]), draw it: the code to exit with.
/// `None` for any other process, which goes on as VOMD.
pub fn child() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != ARG {
        return None;
    }
    let mut num = || args.next()?.to_str()?.parse::<u64>().ok();
    let (Some(parent), Some(data), Some(cpu)) = (num(), num(), num()) else {
        return Some(EXIT_INPUT);
    };
    Some(child_run(parent, data, cpu))
}

#[cfg(target_os = "linux")]
fn child_run(parent: u64, data: u64, cpu: u64) -> i32 {
    // The limits end a process with a signal that dumps core (a crash in the system's reports,
    // and a core of an SVG's memory on disk): they end it by an exit code instead (`_exit`: it
    // allocates nothing, which is what a failed allocation leaves). Named for what it does, for
    // `ps` and the reports.
    // SAFETY: plain integers, a NUL-terminated name, and handlers that only exit.
    unsafe {
        signal(SIGABRT, stopped as extern "C" fn(i32) as usize);
        signal(SIGXCPU, stopped as extern "C" fn(i32) as usize);
        prctl(PR_SET_NAME, c"vomd-svg".as_ptr());
    }
    set_limit(RLIMIT_CORE, 0, 0);
    // Killed when the thread that started it ends; and if that was before this, gone now.
    // SAFETY: plain integers; `getppid` can't fail.
    unsafe {
        prctl(PR_SET_PDEATHSIG, SIGKILL as u64);
        if u64::try_from(getppid()) != Ok(parent) {
            return EXIT_INPUT;
        }
    }
    crate::platform::tune_allocator();
    set_limit(RLIMIT_CPU, cpu, cpu.saturating_add(1));
    set_limit(RLIMIT_DATA, data, data);
    draw_job()
}

/// The child on Windows: its job object (made by the parent) holds its limits and ends it with
/// VOMD, so only what its allocations do when they fail is set here.
#[cfg(windows)]
fn child_run(_parent: u64, _data: u64, _cpu: u64) -> i32 {
    windows::child_setup();
    draw_job()
}

/// Read the job, draw it, write the result: the exit code.
fn draw_job() -> i32 {
    // A panic is a broken SVG's (no message: nobody reads it).
    std::panic::set_hook(Box::new(|_| std::process::exit(EXIT_PANIC)));
    let Some(job) = get_job(&mut Input(std::io::stdin().lock())) else {
        return EXIT_INPUT;
    };
    let stack = if job.limits.forced {
        FORCED_STACK
    } else {
        STACK
    };
    let drawn = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || run(job))
        .and_then(|t| t.join().map_err(|_| std::io::ErrorKind::Other.into()));
    let Ok(result) = drawn else {
        return EXIT_PANIC;
    };
    let mut out = std::io::stdout().lock();
    match write_result(&mut out, &result).and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(_) => EXIT_INPUT,
    }
}

/// Draw `job`, as VOMD's own process would.
fn run(job: Job) -> Rendered {
    match job.test {
        SPIN => loop {
            std::hint::black_box(());
        },
        EAT => {
            let mut held: Vec<Vec<u8>> = Vec::new();
            loop {
                held.push(vec![1; 1 << 20]);
                std::hint::black_box(&held);
            }
        }
        _ => {}
    }
    let mut fonts = Fonts::default();
    fonts.set(job.fonts.map(Arc::new));
    let caps = Caps::of(&job.limits);
    let never = AtomicBool::new(false);
    render_in(
        &job.bytes, &job.svg, job.size, job.charge, &caps, &never, &mut fonts,
    )
}

fn write_result(out: &mut impl Write, r: &Rendered) -> std::io::Result<()> {
    match r {
        Rendered::Pixels(b, skipped) => {
            out.write_all(&[PIXELS, u8::from(*skipped)])?;
            out.write_all(&b.width.to_le_bytes())?;
            out.write_all(&b.height.to_le_bytes())?;
            let mut chunk = Vec::with_capacity(16 << 10);
            for words in b.data.chunks(4 << 10) {
                chunk.clear();
                chunk.extend(words.iter().flat_map(|w| w.to_le_bytes()));
                out.write_all(&chunk)?;
            }
            Ok(())
        }
        Rendered::Need(c) => {
            let mut b = vec![NEED];
            put_cost(&mut b, c);
            out.write_all(&b)
        }
        Rendered::Refused(why) => {
            let code = match why {
                Refusal::Broken => 0,
                Refusal::Complex => 1,
                Refusal::Slow => 2,
                Refusal::Memory => 3,
            };
            out.write_all(&[REFUSED, code])
        }
        // (Never: the child's drawing isn't cancelled, it is killed.)
        Rendered::Cancelled => out.write_all(&[REFUSED, 0]),
    }
}

// ---- The system's -------------------------------------------------------------------------

/// A font file's path as the job carries it: its bytes on Unix, its UTF-16 units on Windows.
fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

/// [`path_bytes`] read back (`None`: not a path's).
fn path_of(bytes: Vec<u8>) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let (units, rest) = bytes.as_chunks::<2>();
        if !rest.is_empty() {
            return None;
        }
        let units: Vec<u16> = units.iter().map(|&b| u16::from_le_bytes(b)).collect();
        Some(PathBuf::from(std::ffi::OsString::from_wide(&units)))
    }
}

/// Whether `out` has something to read (or has ended), waiting `ms` at the most.
#[cfg(target_os = "linux")]
fn readable(out: &ChildStdout, ms: i32) -> bool {
    use std::os::fd::AsRawFd;
    let mut fd = PollFd {
        fd: out.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    };
    // SAFETY: one valid `pollfd`, as the count says.
    unsafe { poll(&mut fd, 1, ms) > 0 }
}

/// Whether `out` has something to read (or has ended), waiting `ms` at the most. An anonymous
/// pipe can't be waited on: it is looked at, and slept on while it is empty.
#[cfg(windows)]
fn readable(out: &ChildStdout, ms: i32) -> bool {
    if windows::available(out) {
        return true;
    }
    std::thread::sleep(Duration::from_millis(ms as u64));
    false
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct RLimit {
    cur: u64,
    max: u64,
}

#[cfg(target_os = "linux")]
const POLLIN: i16 = 1;
#[cfg(target_os = "linux")]
const PR_SET_PDEATHSIG: i32 = 1;
#[cfg(target_os = "linux")]
const PR_SET_NAME: i32 = 15;
#[cfg(target_os = "linux")]
const SIGABRT: i32 = 6;
#[cfg(target_os = "linux")]
const SIGKILL: i32 = 9;
#[cfg(target_os = "linux")]
const SIGXCPU: i32 = 24;
#[cfg(target_os = "linux")]
const RLIMIT_CPU: i32 = 0;
#[cfg(target_os = "linux")]
const RLIMIT_DATA: i32 = 2;
#[cfg(target_os = "linux")]
const RLIMIT_CORE: i32 = 4;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn poll(fds: *mut PollFd, n: u64, timeout_ms: i32) -> i32;
    fn prctl(option: i32, ...) -> i32;
    fn getppid() -> i32;
    fn signal(signal: i32, handler: usize) -> usize;
    fn _exit(code: i32) -> !;
    fn setrlimit(resource: i32, limit: *const RLimit) -> i32;
}

/// The handler of the signals the limits end the child with: exit with the code that says which.
#[cfg(target_os = "linux")]
extern "C" fn stopped(signal: i32) {
    let code = if signal == SIGXCPU {
        EXIT_SLOW
    } else {
        EXIT_MEMORY
    };
    // SAFETY: `_exit` may be called from a handler.
    unsafe { _exit(code) }
}

/// Set the soft and hard limits of `resource` (lowering them needs no privilege).
#[cfg(target_os = "linux")]
fn set_limit(resource: i32, cur: u64, max: u64) {
    let limit = RLimit { cur, max };
    // SAFETY: a valid `rlimit`.
    unsafe { setrlimit(resource, &limit) };
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Allocator as SvgChildAllocator;
#[cfg(windows)]
use windows::refusal_of;

#[cfg(test)]
mod tests;
