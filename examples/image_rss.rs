//! `cargo run --release --example image_rss -- --generate DIR`, then
//! `cargo run --release --example image_rss -- DIR`
//! Peak RSS with many images ("RSS with 50 images stays ≤ budget + cap"): a document of
//! every PNG/JPEG in DIR (one per paragraph) in a 1920 × 1080 view, scrolled from top to bottom
//! a page at a time, waiting at each page for the images near it to decode. Prints the peak RSS,
//! and fails (exit status 1) unless every image was decoded, none failed, and each page showed
//! some: a run that loaded nothing is no measurement.
//! `--generate` first writes the 50 PNGs of 2000 × 1500 the figure is for (noisy gradients, ~4 MB
//! each) to DIR, in a process of its own so making them doesn't count in the peak;
//! `--generate-svg` writes 50 SVG diagrams of about 200 KB instead (paths, a blurred group, text
//! and an embedded PNG each), which the run reads the same way.
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

/// Write 50 SVG diagrams of about 200 KB to `dir`: a grid of stroked paths, a blurred group,
/// labels, and a 64 × 64 PNG embedded in each.
fn generate_svg(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let mut png = Vec::new();
    let mut enc = png::Encoder::new(&mut png, 64, 64);
    enc.set_color(png::ColorType::Rgb);
    let rgb: Vec<u8> = (0..64 * 64)
        .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 4) as u8, 128])
        .collect();
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    let png = base64(&png);
    for n in 0..50 {
        let mut svg = String::from(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1600\" height=\"1200\">\
             <defs><filter id=\"b\"><feGaussianBlur stdDeviation=\"6\"/></filter></defs>",
        );
        let mut i = 0;
        while svg.len() < 200_000 {
            let (x, y) = (i * 37 % 1600, (i * 53 + n * 7) % 1200);
            svg += &format!(
                "<path d=\"M{x} {y}l40 10l-10 40l30 -20\" stroke=\"#{:06x}\" fill=\"none\" stroke-width=\"2\"/>",
                (i * 2_654_435) & 0xFF_FFFF
            );
            i += 1;
        }
        svg += "<g filter=\"url(#b)\" opacity=\"0.6\"><circle cx=\"400\" cy=\"300\" r=\"200\" fill=\"#36c\"/>\
                <rect x=\"900\" y=\"600\" width=\"500\" height=\"300\" fill=\"#c63\"/></g>";
        for k in 0..20 {
            svg += &format!(
                "<text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"24\">Node {n}.{k}</text>",
                40 + k * 70,
                60 + k * 50
            );
        }
        svg += &format!(
            "<image x=\"1200\" y=\"100\" width=\"256\" height=\"256\" href=\"data:image/png;base64,{png}\"/></svg>"
        );
        std::fs::write(dir.join(format!("{n:02}.svg")), svg).unwrap();
    }
    println!("50 SVGs in {}", dir.display());
}

/// `data` in base64.
fn base64(data: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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

fn main() {
    platform::tune_allocator();
    let mut args = std::env::args().skip(1);
    let first = args
        .next()
        .expect("DIR, or --generate DIR, or --generate-svg DIR");
    if first == "--generate" {
        generate(std::path::Path::new(&args.next().expect("DIR")));
        return;
    }
    if first == "--generate-svg" {
        generate_svg(std::path::Path::new(&args.next().expect("DIR")));
        return;
    }
    let dir = std::path::PathBuf::from(first);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".png") || n.ends_with(".jpg") || n.ends_with(".svg"))
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
        let stats = v.image_stats();
        if stats.failed > 0 || stats.decoded == 0 {
            eprintln!("FAILED: page {pages}: {stats:?}");
            std::process::exit(1);
        }
        let before = v.scroll();
        v.apply(Action::Pages(1), Instant::now());
        if v.scroll() <= before {
            break;
        }
    }
    let stats = v.image_stats();
    if stats.decodes < names.len() {
        eprintln!("FAILED: {} images, {stats:?}", names.len());
        std::process::exit(1);
    }
    println!(
        "{} images, {pages} pages in {:.1} s: peak RSS {} KB",
        names.len(),
        start.elapsed().as_secs_f64(),
        platform::peak_rss_kb()
    );
    // For `cargo xtask budgets`.
    println!("image-rss peak_rss_kb={}", platform::peak_rss_kb());
}
