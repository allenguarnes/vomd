//! `cargo run --release --example image_rss -- --generate DIR`, then
//! `cargo run --release --example image_rss -- DIR`
//! Peak RSS with many images ("RSS with 50 images stays ≤ budget + cap"): a document of
//! every PNG/JPEG in DIR (one per paragraph) in a 1920 × 1080 view, scrolled from top to bottom
//! a page at a time, waiting at each page for the images near it to decode. Prints the peak RSS.
//! `--generate` first writes the 50 PNGs of 2000 × 1500 the figure is for (noisy gradients, ~4 MB
//! each) to DIR, in a process of its own so making them doesn't count in the peak.
use std::time::{Duration, Instant};

use vomd::doc::Doc;
use vomd::fonts::Fonts;
use vomd::platform;
use vomd::view::{Action, View};

/// Write 50 PNGs of 2000 × 1500 to `dir`: a gradient per image under noise (so they don't
/// compress to nothing, as photos don't).
fn generate(dir: &std::path::Path) {
    const W: usize = 2000;
    const H: usize = 1500;
    std::fs::create_dir_all(dir).unwrap();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for n in 0..50 {
        let mut rgb = Vec::with_capacity(W * H * 3);
        for y in 0..H {
            for x in 0..W {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let noise = (seed >> 59) as usize;
                let g = [x * 255 / W, y * 255 / H, (x + y + n * 40) * 255 / (W + H)];
                rgb.extend(g.map(|v| (v + noise).min(255) as u8));
            }
        }
        let file = std::fs::File::create(dir.join(format!("{n:02}.png"))).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W as u32, H as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    }
    println!("50 images of {W}×{H} in {}", dir.display());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().expect("DIR, or --generate DIR");
    if first == "--generate" {
        generate(std::path::Path::new(&args.next().expect("DIR")));
        return;
    }
    let dir = std::path::PathBuf::from(first);
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
    let fonts = Fonts::new().unwrap();
    let mut v = View::with_base(fonts, Doc::parse(&src), None, (w, h), 1.0, Some(dir));
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
