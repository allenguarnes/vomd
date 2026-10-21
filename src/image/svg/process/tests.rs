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
    // (A program that starts and says nothing: its SVG is broken, and not drawn here.)
    let (rendered, inline) = drawn_by("/bin/true");
    assert!(matches!(rendered, Rendered::Refused(Refusal::Broken)));
    assert!(!inline);
}
