use super::*;

/// A job read back is the job written, its faces' origins with it.
#[test]
fn a_job_is_read_as_it_was_written() {
    let svg = SvgCost {
        xml: 123,
        gz: true,
        parse: 4 << 20,
        draw: 5,
        layers: 33,
        pixmaps: 2,
        raster: 7 << 20,
    };
    let fonts = crate::fonts::Fonts::new().unwrap().svg_sources();
    let mut job = Vec::new();
    put_job(
        &mut job,
        &svg,
        (40, 30),
        9 << 20,
        &Limits::forced(1 << 30),
        Some(&fonts),
    );
    job.extend_from_slice(&3u64.to_le_bytes());
    job.extend_from_slice(b"<x>");
    let read = get_job(&mut Input(&job[..])).unwrap();
    assert_eq!(read.size, (40, 30));
    assert_eq!(read.charge, 9 << 20);
    assert_eq!(read.limits, Limits::forced(1 << 30));
    assert_eq!(read.svg, svg);
    assert_eq!(read.bytes, b"<x>");
    let read = read.fonts.unwrap();
    assert_eq!(read.origins, fonts.origins);
    assert_eq!((read.sans, read.mono), (fonts.sans, fonts.mono));
    for ((a, i), (b, j)) in read.sources.iter().zip(&fonts.sources) {
        assert_eq!(((**a).as_ref(), i), ((**b).as_ref(), j));
    }
    // Cut short, it is no job.
    assert!(get_job(&mut Input(&job[..job.len() - 1])).is_none());
}

/// A child's end says why it made nothing: by the exit code it chose when a limit stopped it,
/// or by the signal of one that never got to.
#[cfg(unix)]
#[test]
fn a_childs_end_says_why() {
    use std::os::unix::process::ExitStatusExt;
    let signal = |s: i32| ExitStatus::from_raw(s);
    let code = |c: i32| ExitStatus::from_raw(c << 8);
    assert_eq!(refusal_of(code(EXIT_SLOW)), Refusal::Slow);
    assert_eq!(refusal_of(code(EXIT_MEMORY)), Refusal::Memory);
    assert_eq!(refusal_of(signal(24)), Refusal::Slow);
    assert_eq!(refusal_of(signal(6)), Refusal::Memory);
    assert_eq!(refusal_of(signal(9)), Refusal::Memory);
    assert_eq!(refusal_of(signal(11)), Refusal::Broken);
    assert_eq!(refusal_of(code(EXIT_PANIC)), Refusal::Broken);
    assert_eq!(refusal_of(code(EXIT_INPUT)), Refusal::Broken);
    assert_eq!(refusal_of(code(0)), Refusal::Broken);
}

/// The badge fixture drawn by `program` to 120 × 40: what came of it, and whether the drawing
/// was left to VOMD's own process.
fn drawn_by(program: &str) -> (Rendered, bool) {
    let bytes =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/badge.svg"))
            .unwrap();
    let mut header = crate::image::info_of(&bytes).unwrap();
    let inline = AtomicBool::new(false);
    loop {
        let charge = crate::image::decode_cost(&header, 120, 40);
        let rendered = draw(
            Path::new(program),
            &bytes,
            &header.svg,
            (120, 40),
            charge,
            &Limits::normal(),
            &AtomicBool::new(false),
            &inline,
            &mut Fonts::default(),
        );
        match rendered {
            Rendered::Need(need) => header.svg = need,
            rendered => return (rendered, inline.load(Ordering::SeqCst)),
        }
    }
}

/// A child that can't be started leaves the SVG to be drawn here, and says so: the worker's
/// SVG can't be killed then, and the SVGs after it wait for it ([`Images::let_go`]). A child that
/// starts clears it again, for the worker's next job.
#[test]
fn a_child_that_cannot_be_started_is_said_to_be_missing() {
    let (rendered, inline) = drawn_by("/nonexistent/vomd");
    assert!(matches!(rendered, Rendered::Pixels(..)), "drawn here");
    assert!(inline);
    // (A program that starts and says nothing: its SVG is broken, and not drawn here. On
    // Windows, this test binary, which refuses the arguments on its stderr.)
    #[cfg(unix)]
    let quiet = "/bin/true".to_owned();
    #[cfg(windows)]
    let quiet = std::env::current_exe().unwrap().display().to_string();
    let (rendered, inline) = drawn_by(&quiet);
    assert!(matches!(rendered, Rendered::Refused(Refusal::Broken)));
    assert!(!inline);
}

/// On Windows a child's end says why by its exit code: the one it chose (its allocation hook's
/// for memory), the one its job ends it with at its CPU limit, or an abort's or an overflowed
/// stack's status.
#[cfg(windows)]
#[test]
fn a_childs_end_says_why() {
    use std::os::windows::process::ExitStatusExt;
    let code = |c: u32| ExitStatus::from_raw(c);
    assert_eq!(refusal_of(code(EXIT_SLOW as u32)), Refusal::Slow);
    assert_eq!(refusal_of(code(0xC000_0044)), Refusal::Slow);
    assert_eq!(refusal_of(code(EXIT_MEMORY as u32)), Refusal::Memory);
    assert_eq!(refusal_of(code(0xC000_0409)), Refusal::Memory);
    assert_eq!(refusal_of(code(0xC000_00FD)), Refusal::Memory);
    assert_eq!(refusal_of(code(0xC000_0005)), Refusal::Broken);
    assert_eq!(refusal_of(code(EXIT_PANIC as u32)), Refusal::Broken);
    assert_eq!(refusal_of(code(EXIT_INPUT as u32)), Refusal::Broken);
    assert_eq!(refusal_of(code(0)), Refusal::Broken);
}

/// The drawing process (the real binary, which `cargo test` builds beside this one's directory)
/// of a job that does `mode` instead of drawing, put in its job object with `memory` and `cpu`.
/// The job is given once the child is in it, as VOMD does.
#[cfg(windows)]
fn confined(mode: u8, memory: usize, cpu: u64) -> (std::process::Child, Option<windows::Job>) {
    // target/<profile>/deps/<this test>.exe → target/<profile>/vomd.exe
    let exe = std::env::current_exe().unwrap();
    let program = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("vomd.exe");
    let mut child = Command::new(&program)
        .args([ARG, "1", "1", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("{}: {e} (run with `cargo test`)", program.display()));
    let job = windows::Job::confine(&child, memory, cpu);
    let mut input = Vec::new();
    put_job(
        &mut input,
        &SvgCost::default(),
        (8, 8),
        0,
        &Limits::normal(),
        None,
    );
    // The test mode's byte, after the box, the charge, forced and the forced memory (`TEST` is
    // every child's, and other tests draw beside this one).
    input[4 + 4 + 8 + 1 + 8] = mode;
    input.extend_from_slice(&0u64.to_le_bytes());
    child.stdin.take().unwrap().write_all(&input).unwrap();
    (child, job)
}

/// A child at its job's limits ends by the code that says which: memory (its allocation hook)
/// and CPU time (the job's own end of it), quickly, with nothing reported as a crash.
#[cfg(windows)]
#[test]
fn a_child_at_its_limits_exits_by_a_code() {
    let t = std::time::Instant::now();
    let (mut child, job) = confined(EAT, 200 << 20, 60);
    assert!(job.is_some());
    let status = child.wait().unwrap();
    assert_eq!(refusal_of(status), Refusal::Memory, "{status:?}");
    assert_eq!(status.code(), Some(EXIT_MEMORY), "{status:?}");
    assert!(t.elapsed() < Duration::from_secs(20));
    let t = std::time::Instant::now();
    let (mut child, job) = confined(SPIN, 1 << 30, 1);
    assert!(job.is_some());
    let status = child.wait().unwrap();
    assert_eq!(refusal_of(status), Refusal::Slow, "{status:?}");
    assert!(t.elapsed() < Duration::from_secs(20));
}

/// A child goes with its job object, which VOMD holds: closed (dropped, or VOMD ended), the
/// child is killed.
#[cfg(windows)]
#[test]
fn a_child_dies_with_its_job() {
    let (mut child, job) = confined(SPIN, 1 << 30, 60);
    std::thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "it spins");
    drop(job);
    let t = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the child outlived its job"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
