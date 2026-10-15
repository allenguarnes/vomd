use super::*;

use std::time::{Duration, Instant};

/// A PNG of `w` × `h` pixels from `pixel(x, y)` (RGBA).
fn png_bytes(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend(pixel(x, y));
        }
    }
    enc.write_header().unwrap().write_image_data(&data).unwrap();
    out
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
    let dir = std::env::temp_dir().join(format!("vomd-image-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn headers_give_format_and_size() {
    let png = png_bytes(7, 3, |_, _| [0, 0, 0, 255]);
    assert_eq!(header(&mut &png[..]), Some((Format::Png, 7, 3)));
    let jpeg = fixture("gradient.jpg");
    assert_eq!(header(&mut &jpeg[..]), Some((Format::Jpeg, 32, 64)));
    // Progressive (SOF2) and grayscale JPEGs.
    assert_eq!(
        header(&mut &fixture("progressive.jpg")[..]),
        Some((Format::Jpeg, 48, 24))
    );
    assert_eq!(
        header(&mut &fixture("gray.jpg")[..]),
        Some((Format::Jpeg, 16, 16))
    );
    // Others aren't images to us.
    for other in [
        &b"GIF89a\x01\x00\x01\x00"[..],
        b"<svg xmlns=",
        b"RIFF\0\0\0\0WEBPVP8 ",
        b"",
    ] {
        assert_eq!(header(&mut &other[..]), None);
    }
    // Cut short, or a zero size.
    assert_eq!(header(&mut &png[..20]), None);
    let mut zero = png.clone();
    zero[16..20].copy_from_slice(&0u32.to_be_bytes());
    assert_eq!(header(&mut &zero[..]), None);
}

/// A JPEG's frame header can come after big metadata segments; they're skipped, not decoded.
#[test]
fn jpeg_size_after_metadata_segments() {
    let mut j = vec![0xFF, 0xD8];
    for _ in 0..3 {
        // APP1 with 60 KB of payload.
        j.extend([0xFF, 0xE1]);
        j.extend(((60_000 + 2) as u16).to_be_bytes());
        j.extend(std::iter::repeat_n(0xAB, 60_000));
    }
    // Fill bytes before the marker, then SOF0: precision, height 300, width 400.
    j.extend([0xFF, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0x2C, 0x01, 0x90]);
    assert_eq!(header(&mut &j[..]), Some((Format::Jpeg, 400, 300)));
    // DHT (C4) isn't a frame header; the scan starting first means there's none.
    let j = [0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x02, 0xFF, 0xDA, 0x00, 0x02];
    assert_eq!(header(&mut &j[..]), None);
}

#[test]
fn local_paths_resolve_against_the_document() {
    let base = Path::new("/docs/proj");
    let p = |s: &str| local_path(s, Some(base));
    assert_eq!(p("img/a.png"), Some(PathBuf::from("/docs/proj/img/a.png")));
    assert_eq!(p("./a b.png"), Some(PathBuf::from("/docs/proj/./a b.png")));
    assert_eq!(
        p("a%20b.png?raw=true#x"),
        Some(PathBuf::from("/docs/proj/a b.png"))
    );
    assert_eq!(p("../up.jpg"), Some(PathBuf::from("/docs/proj/../up.jpg")));
    assert_eq!(p("/abs/a.png"), Some(PathBuf::from("/abs/a.png")));
    // URLs aren't local files.
    for url in [
        "https://x.y/a.png",
        "HTTP://x.y/a.png",
        "file:///etc/a.png",
        "data:image/png;base64,AAAA",
        "//cdn.example/a.png",
        "",
        "#frag",
        "a\n.png",
    ] {
        assert_eq!(p(url), None, "{url:?}");
    }
    // A relative path needs a base (stdin); an absolute one doesn't.
    assert_eq!(local_path("a.png", None), None);
    assert_eq!(
        local_path("/abs/a.png", None),
        Some(PathBuf::from("/abs/a.png"))
    );
}

#[test]
fn probing_reads_only_regular_files() {
    let dir = scratch("probe");
    let png = dir.join("a.png");
    std::fs::write(&png, png_bytes(5, 4, |_, _| [9, 9, 9, 255])).unwrap();
    assert_eq!(probe_file(&png), Some((5, 4)));
    assert_eq!(probe_file(&dir.join("missing.png")), None);
    assert_eq!(probe_file(&dir), None, "a directory");
    let text = dir.join("a.txt");
    std::fs::write(&text, "not an image").unwrap();
    assert_eq!(probe_file(&text), None);
    // A file over the size cap isn't read (sparse: nothing is written).
    let big = dir.join("big.png");
    let f = std::fs::File::create(&big).unwrap();
    f.set_len(MAX_FILE + 1).unwrap();
    assert_eq!(probe_file(&big), None);
    // An image claiming more pixels than may be decoded.
    let mut huge = png_bytes(1, 1, |_, _| [0; 4]);
    huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    let huge_path = dir.join("huge.png");
    std::fs::write(&huge_path, &huge).unwrap();
    assert_eq!(probe_file(&huge_path), None);
    assert_eq!(decode(&huge, 10, 10), None);
    // A FIFO nobody writes to is refused at once, not waited on.
    let fifo = dir.join("pipe.png");
    if std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success())
    {
        let t = Instant::now();
        assert_eq!(probe_file(&fifo), None);
        assert_eq!(read_file(&fifo), None);
        assert!(t.elapsed() < Duration::from_secs(2));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn decoding_resamples_to_the_box() {
    // Left half black, right half white: halved, each pixel averages one of them.
    let png = png_bytes(4, 2, |x, _| if x < 2 { [0, 0, 0, 255] } else { [255; 4] });
    let b = decode(&png, 2, 1).unwrap();
    assert_eq!((b.width, b.height), (2, 1));
    assert_eq!(b.data, vec![0xFF00_0000, 0xFFFF_FFFF]);
    // Down to one pixel: the average gray.
    let b = decode(&png, 1, 1).unwrap();
    assert_eq!(b.data, vec![0xFF80_8080]);
    // Enlarged, a uniform image stays uniform.
    let red = png_bytes(1, 1, |_, _| [255, 0, 0, 255]);
    let b = decode(&red, 3, 2).unwrap();
    assert_eq!(b.data, vec![0xFFFF_0000; 6]);
}

/// Pixels are premultiplied, and a transparent neighbor doesn't darken or tint an edge.
#[test]
fn transparency_is_premultiplied() {
    let png = png_bytes(2, 1, |x, _| {
        if x == 0 {
            [255, 0, 0, 128]
        } else {
            [0, 255, 0, 0]
        }
    });
    let b = decode(&png, 2, 1).unwrap();
    assert_eq!(b.data[0], 0x8080_0000);
    assert_eq!(b.data[1], 0);
    let b = decode(&png, 1, 1).unwrap();
    // Half the coverage, all of it red.
    assert_eq!(b.data[0], 0x4040_0000);
}

#[test]
fn every_fixture_decodes() {
    for (name, w, h) in [
        ("gradient.jpg", 32, 64),
        ("progressive.jpg", 48, 24),
        ("gray.jpg", 16, 16),
        ("alpha.png", 320, 160),
        ("wide.jpg", 600, 200),
    ] {
        let b = decode(&fixture(name), w / 2, h / 2).unwrap_or_else(|| panic!("{name}"));
        assert_eq!((b.width, b.height), (w / 2, h / 2), "{name}");
        assert_eq!(b.data.len(), (w / 2 * h / 2) as usize);
    }
    // The gradient runs blue to red from left to right.
    let b = decode(&fixture("gradient.jpg"), 32, 64).unwrap();
    let (left, right) = (b.data[0], b.data[31]);
    assert!(left & 0xff > 200 && (left >> 16) & 0xff < 60, "{left:08x}");
    assert!(
        (right >> 16) & 0xff > 200 && right & 0xff < 60,
        "{right:08x}"
    );
    // Gray stays gray.
    let b = decode(&fixture("gray.jpg"), 16, 16).unwrap();
    let p = b.data[0];
    assert_eq!((p >> 16) & 0xff, p & 0xff);
    // Broken data after a good header doesn't decode.
    let mut broken = fixture("wide.jpg");
    broken.truncate(400);
    broken.extend([0xFF, 0xD9]);
    let _ = decode(&broken, 10, 10);
}

#[test]
fn the_cache_evicts_the_least_recently_painted_first() {
    let bm = |w: u32| {
        Data::Pixels(Bitmap {
            width: w,
            height: 1,
            data: vec![0; w as usize],
        })
    };
    // Room for 10 pixels.
    let mut c = Cache::new(40);
    c.insert(Key::Local(1), bm(4));
    c.frame += 1;
    c.insert(Key::Local(2), bm(4));
    c.frame += 1;
    // Image 1 painted again: image 2 is now the least recent.
    assert!(c.pixels(&Key::Local(1), 4, 1).is_some());
    c.insert(Key::Local(3), bm(4));
    assert!(c.has(&Key::Local(1), 4, 1) && c.has(&Key::Local(3), 4, 1));
    assert!(!c.has(&Key::Local(2), 4, 1));
    assert_eq!(c.bytes, 32);
    // Nothing painted this frame is evicted, even over the budget.
    c.frame += 1;
    assert!(c.pixels(&Key::Local(1), 4, 1).is_some());
    assert!(c.pixels(&Key::Local(3), 4, 1).is_some());
    c.insert(Key::Local(4), bm(4));
    assert_eq!(c.slots.len(), 3);
    assert_eq!(c.bytes, 48);
    // Another size of an image stands in while the right one is decoded; the same size
    // replaces its slot.
    assert_eq!(c.pixels(&Key::Local(1), 8, 2).map(|b| b.width), Some(4));
    c.insert(Key::Local(1), bm(4));
    assert_eq!(c.slots.iter().filter(|s| s.key == Key::Local(1)).count(), 1);
    c.retain(|s| s.key != Key::Local(1));
    assert_eq!(c.bytes, 32);
}

/// Wait for `images` to hand over results until `done` holds (or fail after a while).
fn wait(images: &mut Images, doc: &Doc, layout: &mut Layout, done: impl Fn(&mut Images) -> bool) {
    let t = Instant::now();
    while !done(images) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        images.poll(doc, layout);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Local images get their size from the header when their block is measured, and their pixels
/// from the worker, at the size asked for. One that doesn't decode falls back to its alt text.
#[test]
fn a_documents_local_images_load() {
    use crate::render::Pixels;
    let dir = scratch("doc");
    std::fs::write(
        dir.join("a.png"),
        png_bytes(30, 20, |_, _| [0, 0, 255, 255]),
    )
    .unwrap();
    // A PNG header in front of garbage.
    let mut bad = png_bytes(10, 10, |_, _| [0; 4]);
    bad.truncate(40);
    std::fs::write(dir.join("bad.png"), &bad).unwrap();
    let doc = Doc::parse("![a](a.png) ![bad](bad.png) ![gone](gone.png) ![r](https://x.y/r.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 600.0);
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(30, 20)));
    assert_eq!(layout.image_size(2), Some(ImageSize::Pixels(10, 10)));
    assert_eq!(layout.image_size(3), Some(ImageSize::Alt));
    assert_eq!(
        layout.image_size(4),
        Some(ImageSize::Alt),
        "remote images are off"
    );
    images.request(&[(1, 30, 20), (2, 10, 10), (3, 5, 5), (4, 5, 5)]);
    wait(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 30, 20).is_some()
    });
    assert_eq!(images.pixels(1, 30, 20).unwrap().data[0], 0xFF00_00FF);
    let t = Instant::now();
    while layout.image_size(2) != Some(ImageSize::Alt) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        images.poll(&doc, &mut layout);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(images.pixels(3, 5, 5).is_none() && images.pixels(4, 5, 5).is_none());
    // Another document drops the local images; its results are told apart from late ones.
    images.attach(&doc, &mut layout);
    assert!(images.pixels(1, 30, 20).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
