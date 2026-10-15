//! `cargo run --release --example image_rss -- DIR`
//! Peak RSS with many images ("RSS with 50 images stays ≤ budget + cap"): a document of
//! every PNG/JPEG in DIR (one per paragraph) in a 1920 × 1080 view, scrolled from top to bottom
//! a page at a time, waiting at each page for the images near it to decode. Prints the peak RSS.
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::platform;
use vomd::view::{Action, View};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("DIR"));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".png") || n.ends_with(".jpg"))
        .collect();
    names.sort();
    let src: String = names
        .iter()
        .map(|n| format!("Image {n}:\n\n![{n}]({n})\n\n"))
        .collect();
    let (w, h) = (1920u32, 1080u32);
    let mut v = View::new(Fonts::new().unwrap(), Doc::parse(&src), None, (w, h), 1.0);
    v.set_image_base(Some(dir));
    let mut buf = vec![0u32; (w * h) as usize];
    let start = Instant::now();
    let mut pages = 0;
    loop {
        // Paint, ask for the images near the viewport, and take results until none came for a
        // while.
        let mut quiet = Instant::now();
        while quiet.elapsed() < Duration::from_millis(300) {
            v.paint(&mut buf, w as usize, Instant::now());
            v.tick(Instant::now(), Some(Instant::now()));
            if v.poll_images(Instant::now()) {
                quiet = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        pages += 1;
        let before = v.scroll();
        v.apply(Action::Pages(1), Instant::now());
        if v.scroll() <= before {
            break;
        }
    }
    println!(
        "{} images, {pages} pages in {:.1} s: peak RSS {} KB",
        names.len(),
        start.elapsed().as_secs_f64(),
        platform::peak_rss_kb()
    );
}
