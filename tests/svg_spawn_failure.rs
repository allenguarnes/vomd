//! SVGs are drawn apart (`image::svg_child`), but the process can't be started (no `/proc`, no
//! processes left; here, no such program): each is drawn in VOMD's own process then, which can't
//! be killed, so the SVGs after one that took too long wait for it, as they do when SVGs are not
//! drawn apart. (Alone in its binary: the program is set once for all the tests in it.)
#![cfg(all(feature = "svg", target_os = "linux"))]

use std::path::Path;
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::image::{Images, set_svg_program, svg_children};
use vomd::layout::{ImageSize, Layout, Params, Refusal};
use vomd::render::Pixels;

/// A scratch directory, removed when this goes (a failed run too).
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/images")
            .join(name),
    )
    .unwrap()
}

/// A drawing that takes too long is given up at its deadline, and the SVG after it waits until
/// it is done: no second drawing runs beside the one that can't be stopped.
#[test]
fn an_svg_drawn_here_after_a_failed_start_is_waited_for() {
    set_svg_program("/nonexistent/vomd".into());
    let dir = std::env::temp_dir().join(format!("vomd-svg-spawn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _scratch = Scratch(dir.clone());
    // Random lines filled: about a tenth of a second, and well within the estimate's cap.
    let mut seed = 1u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (seed >> 8) % 4000
    };
    let mut d = String::from("M0 0");
    for _ in 0..8_000 {
        d += &format!(" L{} {}", rnd(), rnd());
    }
    let slow = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"512\" height=\"512\" viewBox=\"0 0 4000 4000\"><path d=\"{d}\"/></svg>"
    );
    std::fs::write(dir.join("slow.svg"), slow).unwrap();
    std::fs::write(dir.join("logo.svg"), fixture("logo-viewbox.svg")).unwrap();
    let doc = Doc::parse("![slow](slow.svg) ![logo](logo.svg)");
    let mut layout = Layout::new(&doc, Params::default());
    let mut images = Images::default();
    images.set_svg_deadline(Duration::from_millis(1));
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    let boxes = [(1, 512, 512), (2, 48, 32)];
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < Duration::from_secs(10), "never given up");
        images.request(&boxes[..1], 1);
        std::thread::sleep(Duration::from_millis(2));
        if images.expire(Instant::now()) {
            break;
        }
        images.poll(&doc, &mut layout);
    }
    // It runs on in VOMD's own process: the other SVG waits for it.
    assert!(images.svg_stuck());
    images.poll(&doc, &mut layout);
    assert_eq!(
        layout.image_size(1),
        Some(ImageSize::Refused(Refusal::Slow))
    );
    while images.pixels(2, 48, 32).is_none() || images.svg_stuck() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        images.request(&boxes[1..], 1);
        if images.svg_stuck() {
            assert!(images.pixels(2, 48, 32).is_none());
        }
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert_eq!(svg_children(), 0);
}
