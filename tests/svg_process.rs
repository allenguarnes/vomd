//! SVGs drawn in processes of their own (`image::svg_child`), by the real binary: what they draw
//! is what VOMD's own process draws, and one that takes too long, isn't wanted any more, runs out
//! of memory or outlives its parent is killed, with nothing of it left. The tests take [`serial`]
//! first: what the children are told to do ([`set_svg_test`]) is for all of them.
#![cfg(all(feature = "svg", any(target_os = "linux", windows)))]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::image::{
    Images, SVG_TEST_EAT, SVG_TEST_SPIN, SvgJob, decode, set_svg_cpu_limit, set_svg_program,
    set_svg_test, svg_children,
};
use vomd::layout::{ImageSize, Layout, Params, Refusal};
use vomd::render::Pixels;

/// How long a child may take to end once it is killed, or the badge after it to be drawn: far
/// under what the child would run if it weren't (it never ends), far over what a loaded machine
/// makes of it.
const SANE: Duration = Duration::from_secs(10);

fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    set_svg_program(env!("CARGO_BIN_EXE_vomd").into());
    set_svg_test(0);
    set_svg_cpu_limit(0);
    guard
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/images")
            .join(name),
    )
    .unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vomd-svg-process-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Wait for no child to be running, at most `limit`.
fn no_children_within(limit: Duration) {
    let t = Instant::now();
    while svg_children() > 0 {
        assert!(
            t.elapsed() < limit,
            "{} children still running",
            svg_children()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// An SVG `n` badges and one more, `slow.svg`, which the test makes its children spin on.
fn page(dir: &Path, n: usize) -> (Doc, Layout, Images) {
    std::fs::write(dir.join("slow.svg"), fixture("logo-viewbox.svg")).unwrap();
    std::fs::write(dir.join("badge.svg"), fixture("badge.svg")).unwrap();
    let mut src = String::from("![slow](slow.svg)\n\n");
    for _ in 0..n {
        src += "![badge](badge.svg) ";
    }
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.to_path_buf()));
    images.attach(&doc, &mut layout);
    (doc, layout, images)
}

/// What the children draw is what VOMD's own process draws: badges with text, a viewBox, an
/// svgz, an embedded raster image.
#[test]
fn svgs_drawn_apart_are_drawn_as_here() {
    let _s = serial();
    for name in [
        "badge.svg",
        "badge-arial.svg",
        "badge.svgz",
        "logo-viewbox.svg",
        "nested-logo.svg",
        "raster-inside.svg",
        "units.svg",
    ] {
        let bytes = fixture(name);
        // (Without fonts: `decode` draws no text, and neither does the child.)
        let apart = decode(&bytes, 120, 40);
        let mut plain = SvgJob::of(&bytes).unwrap();
        let plain = loop {
            if let Some(b) = plain.run(&bytes, 120, 40) {
                break b.unwrap();
            }
        };
        assert_eq!(apart.as_ref(), Some(&plain), "{name}");
    }
    no_children_within(SANE);
}

/// Text is set in the faces VOMD's own text is: the child is given them, and draws what VOMD's
/// own process does.
#[test]
fn text_drawn_apart_is_set_in_vomds_faces() {
    let _s = serial();
    let dir = scratch("text");
    std::fs::write(dir.join("badge.svg"), fixture("badge.svg")).unwrap();
    let doc = Doc::parse("![badge](badge.svg)");
    let mut layout = Layout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    let fonts = vomd::fonts::Fonts::new().unwrap();
    images.set_svg_fonts(fonts.svg_sources());
    images.attach(&doc, &mut layout);
    let t = Instant::now();
    while images.pixels(1, 120, 40).is_none() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        images.request(&[(1, 120, 40)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    let bytes = fixture("badge.svg");
    let mut job = SvgJob::of(&bytes).unwrap().with_fonts(&fonts);
    let here = loop {
        if let Some(b) = job.run(&bytes, 120, 40) {
            break b.unwrap();
        }
    };
    assert_eq!(images.pixels(1, 120, 40), Some(&here));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child that never ends is killed at the deadline: its image is refused as slow (a click
/// loads it), nothing of it runs on, and the SVGs after it are drawn at once, with no wait for
/// it.
#[test]
fn a_child_past_its_deadline_is_killed_and_the_rest_are_drawn() {
    let _s = serial();
    let dir = scratch("deadline");
    let (doc, mut layout, mut images) = page(&dir, 1);
    images.set_svg_deadline(Duration::from_millis(300));
    set_svg_test(SVG_TEST_SPIN);
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < Duration::from_secs(10), "never given up");
        images.request(&[(1, 64, 64)], 1);
        std::thread::sleep(Duration::from_millis(2));
        if images.expire(Instant::now()) {
            break;
        }
        images.poll(&doc, &mut layout);
    }
    assert!(t.elapsed() >= Duration::from_millis(300));
    set_svg_test(0);
    // Killed within a poll of the flag.
    no_children_within(SANE);
    images.poll(&doc, &mut layout);
    assert_eq!(
        layout.image_size(1),
        Some(ImageSize::Refused(Refusal::Slow))
    );
    let t = Instant::now();
    while images.pixels(2, 120, 40).is_none() {
        assert!(t.elapsed() < SANE, "the badge waited");
        images.request(&[(2, 120, 40)], 1);
        std::thread::sleep(Duration::from_millis(1));
        images.poll(&doc, &mut layout);
    }
    images.poll(&doc, &mut layout);
    assert_eq!(
        layout.image_size(1),
        Some(ImageSize::Refused(Refusal::Slow))
    );
    assert_eq!(images.deadline(), None);
    no_children_within(SANE);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child whose drawing isn't wanted any more is killed: its document is replaced, or its box
/// changes size (it is drawn again at the new one).
#[test]
fn a_child_no_longer_wanted_is_killed() {
    let _s = serial();
    let dir = scratch("cancel");
    let (doc, mut layout, mut images) = page(&dir, 1);
    images.set_svg_deadline(Duration::from_secs(600));
    let started = |images: &mut Images, layout: &mut Layout, size: u32| {
        let t = Instant::now();
        while svg_children() == 0 {
            assert!(t.elapsed() < Duration::from_secs(10), "never started");
            images.request(&[(1, size, size)], 1);
            std::thread::sleep(Duration::from_millis(1));
            images.poll(&doc, layout);
        }
    };
    set_svg_test(SVG_TEST_SPIN);
    started(&mut images, &mut layout, 64);
    // Its box changes size: drawn at the new one, which it would wait for if the old one ran on.
    set_svg_test(0);
    let t = Instant::now();
    while images.pixels(1, 65, 65).is_none() {
        assert!(t.elapsed() < SANE, "the old size's child ran on");
        images.request(&[(1, 65, 65)], 1);
        std::thread::sleep(Duration::from_millis(1));
        images.poll(&doc, &mut layout);
    }
    no_children_within(SANE);
    set_svg_test(SVG_TEST_SPIN);
    started(&mut images, &mut layout, 66);
    // Its document is replaced.
    let other = Doc::parse("text");
    let mut other_layout = Layout::new(&other, Params::default());
    images.attach(&other, &mut other_layout);
    no_children_within(SANE);
    set_svg_test(0);
    // Nothing runs while nothing is asked for.
    images.poll(&other, &mut other_layout);
    assert_eq!(svg_children(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A load past the limits (a click on the placeholder) is drawn apart too: one that never ends
/// is killed when its document goes.
#[test]
fn a_forced_child_is_killed_when_its_document_goes() {
    let _s = serial();
    let dir = scratch("forced");
    let (doc, mut layout, mut images) = page(&dir, 1);
    set_svg_test(SVG_TEST_SPIN);
    assert!(images.force(1, &doc, &mut layout));
    let t = Instant::now();
    while svg_children() == 0 {
        assert!(t.elapsed() < Duration::from_secs(10), "never started");
        images.request(&[(1, 64, 64)], 1);
        std::thread::sleep(Duration::from_millis(1));
        images.poll(&doc, &mut layout);
    }
    // No deadline: it would run on.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(svg_children(), 1);
    let other = Doc::parse("text");
    let mut other_layout = Layout::new(&other, Params::default());
    images.attach(&other, &mut other_layout);
    no_children_within(SANE);
    set_svg_test(0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child past its memory limit is refused for memory (a click loads it anyway), and the
/// images after it are drawn.
#[test]
fn a_child_out_of_memory_is_refused_for_memory() {
    let _s = serial();
    let dir = scratch("memory");
    let (doc, mut layout, mut images) = page(&dir, 1);
    set_svg_test(SVG_TEST_EAT);
    let t = Instant::now();
    while layout.image_size(1) != Some(ImageSize::Refused(Refusal::Memory(0))) {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "{:?}",
            layout.image_size(1)
        );
        images.request(&[(1, 64, 64)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    set_svg_test(0);
    no_children_within(SANE);
    let t = Instant::now();
    while images.pixels(2, 120, 40).is_none() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        images.request(&[(2, 120, 40)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The job a child that does `mode` ([`SVG_TEST_SPIN`], [`SVG_TEST_EAT`]) reads: what the parent
/// writes (see `svg::process`), the SVG empty.
#[cfg(target_os = "linux")]
fn job_of(mode: u8) -> Vec<u8> {
    let mut job = Vec::new();
    job.extend_from_slice(&64u32.to_le_bytes());
    job.extend_from_slice(&64u32.to_le_bytes());
    job.extend_from_slice(&(1u64 << 20).to_le_bytes());
    job.push(0);
    job.extend_from_slice(&0u64.to_le_bytes());
    job.push(mode);
    job.extend_from_slice(&[0; 37]);
    job.push(0);
    job.extend_from_slice(&0u64.to_le_bytes());
    job
}

/// The drawing process of `job`, started by this process as VOMD does (with `data` bytes of data
/// and `cpu` seconds of CPU), with nothing on its output.
#[cfg(target_os = "linux")]
fn child_of(dir: &Path, job: &[u8], data: u64, cpu: u64) -> std::process::Child {
    std::fs::write(dir.join("job"), job).unwrap();
    std::process::Command::new(env!("CARGO_BIN_EXE_vomd"))
        .args(["--draw-svg", &std::process::id().to_string()])
        .args([data.to_string(), cpu.to_string()])
        .stdin(std::fs::File::open(dir.join("job")).unwrap())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// A child doesn't outlive the process that started it.
#[cfg(target_os = "linux")]
#[test]
fn a_child_dies_with_its_parent() {
    let _s = serial();
    let dir = scratch("orphan");
    std::fs::write(dir.join("job"), job_of(SVG_TEST_SPIN)).unwrap();
    // A shell starts it (with the shell's pid as its parent's), says its pid and that it runs,
    // and exits.
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#""$1" --draw-svg $$ 1000000000 60 < "$2" & echo $!; sleep 0.3; kill -0 $! && echo spinning"#)
        .arg("sh")
        .arg(env!("CARGO_BIN_EXE_vomd"))
        .arg(dir.join("job"))
        .output()
        .unwrap();
    let out = String::from_utf8(out.stdout).unwrap();
    let mut lines = out.lines();
    let pid = lines.next().unwrap().to_owned();
    assert_eq!(
        lines.next(),
        Some("spinning"),
        "it ran while its parent did"
    );
    let stat = PathBuf::from(format!("/proc/{pid}/stat"));
    let t = Instant::now();
    // Gone, or a zombie its new parent hasn't reaped yet.
    while std::fs::read_to_string(&stat).is_ok_and(|s| {
        s.rsplit(')')
            .next()
            .is_some_and(|r| !r.trim_start().starts_with('Z'))
    }) {
        assert!(t.elapsed() < SANE, "the child outlived its parent");
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Started by anything but VOMD (its parent's pid isn't the one it is given), the drawing mode
/// does nothing.
#[test]
fn the_drawing_mode_wants_its_parent() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_vomd"))
        .args(["--draw-svg", "1", "1000000", "5"])
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(3));
}

/// What a child costs: 50 badges drawn apart, one after the other, take little longer than
/// drawn here.
#[test]
fn badges_drawn_apart_are_quick() {
    let _s = serial();
    let bytes = fixture("badge.svg");
    let t = Instant::now();
    for _ in 0..50 {
        assert!(decode(&bytes, 120, 40).is_some());
    }
    let apart = t.elapsed();
    let mut job = SvgJob::of(&bytes).unwrap();
    let t = Instant::now();
    for _ in 0..50 {
        while job.run(&bytes, 120, 40).is_none() {}
    }
    let here = t.elapsed();
    eprintln!(
        "50 badges: apart {apart:?} ({:?} each), here {here:?}",
        apart / 50
    );
    assert!(
        apart < here * 20 + Duration::from_secs(1),
        "{apart:?} / {here:?}"
    );
}

/// A child that runs out of memory or CPU time ends by an exit code, not by the signal the limit
/// ends a process with: that dumps core, and the system's crash reports named VOMD for it.
#[cfg(target_os = "linux")]
#[test]
fn a_child_at_its_limits_exits_by_a_code() {
    use std::os::unix::process::ExitStatusExt;
    let dir = scratch("limits");
    // Memory: a failed allocation aborts (6 after the handler), well inside the limit's reach.
    let t = Instant::now();
    let status = child_of(&dir, &job_of(SVG_TEST_EAT), 200 << 20, 60)
        .wait()
        .unwrap();
    assert_eq!(status.signal(), None, "{status:?}");
    assert_eq!(status.code(), Some(6), "{status:?}");
    assert!(t.elapsed() < Duration::from_secs(10));
    // CPU time: the signal of its soft limit (5).
    let t = Instant::now();
    let status = child_of(&dir, &job_of(SVG_TEST_SPIN), 1 << 30, 1)
        .wait()
        .unwrap();
    assert_eq!(status.signal(), None, "{status:?}");
    assert_eq!(status.code(), Some(5), "{status:?}");
    assert!(t.elapsed() < Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child is named for what it does (the system's process lists and reports would say `exe`,
/// the name of the file it is run through), and dumps no core.
#[cfg(target_os = "linux")]
#[test]
fn a_child_is_named_and_dumps_no_core() {
    let dir = scratch("named");
    let mut child = child_of(&dir, &job_of(SVG_TEST_SPIN), 1 << 30, 60);
    let comm = PathBuf::from(format!("/proc/{}/comm", child.id()));
    let limits = PathBuf::from(format!("/proc/{}/limits", child.id()));
    let t = Instant::now();
    // (It names itself when it starts.)
    while std::fs::read_to_string(&comm).is_ok_and(|c| c.trim() != "vomd-svg") {
        assert!(t.elapsed() < SANE, "never named");
        std::thread::sleep(Duration::from_millis(5));
    }
    while !std::fs::read_to_string(&limits).is_ok_and(|l| {
        (l.lines())
            .find(|l| l.starts_with("Max core file size"))
            .is_some_and(|l| l.split_whitespace().rev().nth(2) == Some("0"))
    }) {
        assert!(t.elapsed() < SANE, "core not limited");
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child stopped by its CPU limit (the deadline is far off) is refused as slow: a click loads
/// it, and the CPU limit of that load stops it too (it couldn't load).
#[test]
fn a_child_out_of_cpu_time_is_refused_as_slow() {
    let _s = serial();
    let dir = scratch("cpu");
    let (doc, mut layout, mut images) = page(&dir, 1);
    images.set_svg_deadline(Duration::from_secs(600));
    set_svg_test(SVG_TEST_SPIN);
    set_svg_cpu_limit(1);
    let t = Instant::now();
    while layout.image_size(1) != Some(ImageSize::Refused(Refusal::Slow)) {
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "{:?}",
            layout.image_size(1)
        );
        images.request(&[(1, 64, 64)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert!(t.elapsed() >= Duration::from_secs(1), "stopped by its CPU");
    no_children_within(SANE);
    // The click: drawn again, stopped by the same limit.
    assert!(images.force(1, &doc, &mut layout));
    let t = Instant::now();
    while layout.image_size(1) != Some(ImageSize::Refused(Refusal::Failed)) {
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "{:?}",
            layout.image_size(1)
        );
        images.request(&[(1, 64, 64)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    set_svg_test(0);
    set_svg_cpu_limit(0);
    no_children_within(SANE);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child is killed when its window goes (the images are dropped), and when the window's wake
/// is replaced (the decode it was drawing for is let go, and asked for again).
#[test]
fn a_child_is_killed_when_its_window_goes() {
    let _s = serial();
    let dir = scratch("window");
    let (doc, mut layout, mut images) = page(&dir, 1);
    images.set_svg_deadline(Duration::from_secs(600));
    let started = |images: &mut Images, layout: &mut Layout, doc: &Doc| {
        let t = Instant::now();
        while svg_children() == 0 {
            assert!(t.elapsed() < Duration::from_secs(10), "never started");
            images.request(&[(1, 64, 64)], 1);
            std::thread::sleep(Duration::from_millis(1));
            images.poll(doc, layout);
        }
    };
    set_svg_test(SVG_TEST_SPIN);
    started(&mut images, &mut layout, &doc);
    // A new window's wake: the old decode stops, and the image is drawn by the new one.
    images.set_wake(std::sync::Arc::new(|| {}));
    no_children_within(SANE);
    set_svg_test(0);
    let t = Instant::now();
    while images.pixels(1, 64, 64).is_none() {
        assert!(t.elapsed() < SANE, "not drawn again");
        images.request(&[(1, 64, 64)], 1);
        std::thread::sleep(Duration::from_millis(1));
        images.poll(&doc, &mut layout);
    }
    // The window closes while one is drawing.
    let (doc, mut layout, mut images) = page(&dir, 1);
    images.set_svg_deadline(Duration::from_secs(600));
    set_svg_test(SVG_TEST_SPIN);
    started(&mut images, &mut layout, &doc);
    drop(images);
    no_children_within(SANE);
    set_svg_test(0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `data` in base64, for `data:` URLs.
fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= c.len() {
                ABC[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// A raster image that is refused when the child draws it (a valid WebP over the limit of its
/// Huffman codes: it passes the checks of its header) leaves the SVG drawn without it, and
/// noted as such, as an image refused for its size is: a click draws it.
#[test]
fn a_raster_image_refused_in_a_child_leaves_the_svg_clickable() {
    let _s = serial();
    let dir = scratch("refused-raster");
    let webp = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fuzz/seeds/image/webp-lossless-many-huffman-groups.bin"),
    )
    .unwrap();
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><image width=\"4\" height=\"4\" href=\"data:image/webp;base64,{}\"/></svg>",
        base64(&webp)
    );
    std::fs::write(dir.join("groups.svg"), &svg).unwrap();
    // Loaded past the limits, it is drawn: an opaque black pixel.
    let forced = vomd::image::decode_forced(svg.as_bytes(), 4, 4, 32 << 20).unwrap();
    assert_eq!(forced.data[0], 0xFF00_0000);
    let doc = Doc::parse("![groups](groups.svg)");
    let mut layout = Layout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    let t = Instant::now();
    let corner = loop {
        assert!(t.elapsed() < SANE, "never drawn");
        images.request(&[(1, 4, 4)], 1);
        images.poll(&doc, &mut layout);
        if let Some(b) = images.pixels(1, 4, 4) {
            break b.data[0];
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(corner, 0, "drawn without the image");
    assert!(images.partial(1), "and a click draws it");
    drop(images);
    no_children_within(SANE);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An opaque gray PNG of `w` × `h` with the EXIF orientation `o`.
fn turned_png(w: u32, h: u32, o: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    // A TIFF of one entry (the orientation, a short), big-endian.
    let mut tiff = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
    tiff.extend([0, o, 0, 0, 0, 0, 0, 0]);
    writer.write_chunk(png::chunk::eXIf, &tiff).unwrap();
    writer
        .write_image_data(&vec![255; (w * h) as usize])
        .unwrap();
    writer.finish().unwrap();
    out
}

/// A raster image turned by an EXIF orientation that swaps its sides is as big and as centered
/// in a child as it is shown (see `a_turned_raster_image_is_as_big_and_as_centered_as_it_is_shown`
/// in `image::svg`, which works the bounds out): by area and by the three nearest-neighbor modes.
#[test]
fn a_turned_raster_image_is_as_big_and_as_centered_in_a_child() {
    let _s = serial();
    // The image, its box, and the columns it covers: from the 4th to the 9th of 12 for the
    // 5⅓ columns a turned 6 × 4 has at 12 × 8 (3⅓ to 8⅔), the 4th and 5th of 8 for the 2 a
    // turned 2 × 1 has at 8 × 4 (3 to 5); every row of both.
    for ((w, h), (bw, bh), columns) in [((6, 4), (12, 8), 3..9), ((2, 1), (8, 4), 3..5)] {
        for o in 5..=8 {
            for mode in ["auto", "optimizeSpeed", "crisp-edges", "pixelated"] {
                let svg = format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\"><image width=\"{w}\" height=\"{h}\" style=\"image-rendering:{mode}\" href=\"data:image/png;base64,{}\"/></svg>",
                    base64(&turned_png(w, h, o))
                );
                // (`decode` draws it in a child: `SvgJob::run` would draw it here.)
                let drawn = decode(svg.as_bytes(), bw, bh).unwrap();
                for y in 0..bh as usize {
                    let row: Vec<usize> = (0..bw as usize)
                        .filter(|&x| drawn.data[y * bw as usize + x] >> 24 > 127)
                        .collect();
                    assert_eq!(
                        row,
                        columns.clone().collect::<Vec<_>>(),
                        "{w}×{h} {o} {mode} row {y}"
                    );
                }
            }
        }
    }
    no_children_within(SANE);
}
