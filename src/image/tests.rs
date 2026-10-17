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
    // Fill bytes before the marker, then SOF0: precision, height 300, width 400, one
    // component.
    j.extend([0xFF, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x01, 0x2C, 0x01, 0x90]);
    j.extend([0x01, 0x01, 0x11, 0x00]);
    assert_eq!(header(&mut &j[..]), Some((Format::Jpeg, 400, 300)));
    // DHT (C4) isn't a frame header; the scan starting first means there's none.
    let j = [0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x02, 0xFF, 0xDA, 0x00, 0x02];
    assert_eq!(header(&mut &j[..]), None);
}

/// A JPEG that says 16×16 to a reader taking `FF 01` for a marker without a segment, and
/// 60000×60000 to one that gives it a length (the decoder does): `FF D8 FF 01`, whose "length"
/// is the first two bytes of a decoy frame header, so the real one comes after the padding.
fn crafted_jpeg() -> Vec<u8> {
    let sof = |w: u16, h: u16| {
        let mut s = vec![0xFF, 0xC0, 0x00, 0x0B, 0x08];
        s.extend(h.to_be_bytes());
        s.extend(w.to_be_bytes());
        s.extend([0x01, 0x01, 0x11, 0x00]);
        s
    };
    let mut j = vec![0xFF, 0xD8, 0xFF, 0x01];
    j.extend(sof(16, 16));
    // 0xFFC0 bytes from the start of the decoy, the length included.
    j.resize(4 + 0xFFC0, 0);
    j.extend(sof(60_000, 60_000));
    j.extend([0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
    j
}

/// The size the JPEG decoder reads from the headers (without decoding anything).
fn zune_size(bytes: &[u8]) -> Option<(u32, u32)> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, options::DecoderOptions};
    let options = DecoderOptions::default()
        .set_max_width(usize::from(u16::MAX))
        .set_max_height(usize::from(u16::MAX));
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    d.decode_headers().ok()?;
    let info = d.info()?;
    Some((u32::from(info.width), u32::from(info.height)))
}

/// Our JPEG header and the decoder agree on the size (they once read `FF 01` differently, and
/// the decoder allocated what a 16×16 header had promised it wouldn't), and an image that
/// is too big isn't decoded either way.
#[test]
fn the_jpeg_header_agrees_with_the_decoder() {
    for name in ["gradient.jpg", "progressive.jpg", "gray.jpg", "wide.jpg"] {
        let j = fixture(name);
        let (_, w, h) = header(&mut &j[..]).unwrap();
        assert_eq!(zune_size(&j), Some((w, h)), "{name}");
    }
    let j = crafted_jpeg();
    assert_eq!(header(&mut &j[..]), Some((Format::Jpeg, 60_000, 60_000)));
    assert_eq!(zune_size(&j), Some((60_000, 60_000)));
    // 3.6 gigapixels: never decoded (this used to try to allocate 10.8 GB).
    assert_eq!(decode(&j, 10, 10), None);
    let dir = scratch("crafted");
    let path = dir.join("c.jpg");
    std::fs::write(&path, &j).unwrap();
    assert_eq!(probe_file(&path), None);
    let _ = std::fs::remove_dir_all(&dir);
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
    // Neither are paths that make Windows connect to a server, however written or encoded.
    for net in [
        r"\\attacker\share\x.png",
        "//attacker/share/x.png",
        r"/\attacker\share\x.png",
        r"\/attacker/share/x.png",
        "%2F%2Fattacker/share/x.png",
        "%5C%5Cattacker%5Cshare%5Cx.png",
        "%2f%5cattacker/share/x.png",
        r"\\?\UNC\attacker\share\x.png",
        r"\\.\pipe\x",
        "//?/C:/x.png",
        "///x.png",
        // The NT form is handed to the system as it is, with no parsing: `?` and `*` can't be in
        // a Windows filename, so no path with one is a file.
        r"%5C%3F%3F%5CUNC%5Chost%5Cshare%5Cx.png",
        "%2F%3F%3F%2FUNC%2Fhost%2Fshare%2Fx.png",
        r"\??\UNC\host\share\x.png",
        "/??/UNC/host/share/x.png",
        r"/??\UNC/host/x.png",
        "%2F%3F%3F%2Fx.png",
        "a%3Fb.png",
        "a%2Ab.png",
        "x/*.png",
    ] {
        assert_eq!(p(net), None, "{net:?}");
        assert_eq!(local_path(net, None), None, "{net:?}");
    }
    // The query is cut off before decoding: a raw `?` ends the path, it isn't in it.
    assert_eq!(p("a.png?x=?*"), Some(PathBuf::from("/docs/proj/a.png")));
    // A single root, and what only looks like one, are paths.
    assert_eq!(p("%2Fabs/a.png"), Some(PathBuf::from("/abs/a.png")));
    assert_eq!(p("a//b.png"), Some(PathBuf::from("/docs/proj/a//b.png")));
    // A `src` longer than any system's path (as written, so percent-encoded) isn't a file, and
    // one of the longest allowed is.
    assert_eq!(p(&"a".repeat(MAX_SRC + 1)), None);
    assert!(p(&"a".repeat(MAX_SRC)).is_some());
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
        assert_eq!(read_file(&fifo, usize::MAX), None);
        assert!(t.elapsed() < Duration::from_secs(30));
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

/// A PNG read a row at a time is resampled to the same pixels as its whole frame: sizes that
/// shrink, enlarge and don't divide, and every color type the decoder expands to 8 bits.
#[test]
fn a_png_read_by_rows_decodes_like_its_whole_frame() {
    let noise = |x: u32, y: u32| {
        let v = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503) ^ (x * y);
        [
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
            255 - (v >> 4) as u8 % 90,
        ]
    };
    for (sw, sh) in [(1, 1), (7, 5), (64, 3), (3, 64), (101, 97)] {
        let png = png_bytes(sw, sh, noise);
        for (w, h) in [(1, 1), (2, 3), (sw, sh), (sw / 2 + 1, sh * 2), (200, 150)] {
            let rows = decode_png(&png, w, h, true).unwrap();
            let frame = decode_png(&png, w, h, false).unwrap();
            assert_eq!(rows, frame, "{sw}×{sh} to {w}×{h}");
        }
    }
    let encode = |color, depth, palette: Option<&[u8]>, trns: Option<&[u8]>, data: &[u8]| {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 5, 4);
        enc.set_color(color);
        enc.set_depth(depth);
        if let Some(p) = palette {
            enc.set_palette(p.to_vec());
        }
        if let Some(t) = trns {
            enc.set_trns(t.to_vec());
        }
        enc.write_header().unwrap().write_image_data(data).unwrap();
        out
    };
    let ramp = |n: usize| (0..n).map(|i| (i * 37 % 251) as u8).collect::<Vec<u8>>();
    for (name, png) in [
        (
            "gray",
            encode(
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                None,
                None,
                &ramp(20),
            ),
        ),
        (
            "gray alpha",
            encode(
                png::ColorType::GrayscaleAlpha,
                png::BitDepth::Eight,
                None,
                None,
                &ramp(40),
            ),
        ),
        (
            "rgb 16",
            encode(
                png::ColorType::Rgb,
                png::BitDepth::Sixteen,
                None,
                None,
                &ramp(120),
            ),
        ),
        (
            "gray 1",
            encode(
                png::ColorType::Grayscale,
                png::BitDepth::One,
                None,
                None,
                &[0b1010_0000; 4],
            ),
        ),
        (
            "indexed",
            encode(
                png::ColorType::Indexed,
                png::BitDepth::Eight,
                Some(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9]),
                Some(&[255, 128]),
                &(0..20).map(|i| i as u8 % 4).collect::<Vec<u8>>(),
            ),
        ),
    ] {
        let rows = decode_png(&png, 3, 2, true).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(Some(rows), decode_png(&png, 3, 2, false), "{name}");
    }
    // An interlaced PNG is read whole (its rows arrive pass by pass) and comes out the same as
    // the same pixels not interlaced.
    let plain = fixture("alpha.png");
    let adam7 = fixture("interlaced.png");
    assert_eq!(header(&mut &adam7[..]), Some((Format::Png, 320, 160)));
    for (w, h) in [(160, 80), (320, 160), (500, 333), (7, 5)] {
        assert_eq!(decode(&plain, w, h), decode(&adam7, w, h), "{w}×{h}");
    }
    // Data that ends early doesn't decode, whichever way it's read.
    let png = png_bytes(40, 40, noise);
    let cut = &png[..png.len() - png.len() / 3];
    assert_eq!(decode_png(cut, 10, 10, true), None);
    assert_eq!(decode_png(cut, 10, 10, false), None);
}

/// The image resampled by averaging the source area under each target pixel a column at a time
/// first (the order before rows were reduced to the target width as they came), a whole frame.
fn resampled_columns_first(frame: &Frame, w: u32, h: u32) -> Vec<u32> {
    let (xs, ys) = (Spans::new(frame.width, w), Spans::new(frame.height, h));
    let (sw, ch) = (frame.width as usize, frame.channels);
    let mut acc = vec![0f32; sw * 4];
    let mut data = Vec::new();
    for t in 0..ys.len() {
        let (y0, wy) = ys.get(t);
        acc.fill(0.0);
        for (k, &weight) in wy.iter().enumerate() {
            let row = &frame.data[(y0 + k) * frame.stride..];
            for (a, p) in acc.chunks_exact_mut(4).zip(row.chunks_exact(ch)) {
                let al = f32::from(p[3]) * weight;
                a[0] += f32::from(p[0]) * al;
                a[1] += f32::from(p[1]) * al;
                a[2] += f32::from(p[2]) * al;
                a[3] += al;
            }
        }
        for t in 0..xs.len() {
            let (x0, wx) = xs.get(t);
            let mut px = [0f32; 4];
            for (k, &weight) in wx.iter().enumerate() {
                for c in 0..4 {
                    px[c] += acc[(x0 + k) * 4 + c] * weight;
                }
            }
            let q = |v: f32, scale: f32| (v / scale).round().clamp(0.0, 255.0) as u32;
            let a = q(px[3], 1.0);
            let (r, g, b) = (
                q(px[0], 255.0).min(a),
                q(px[1], 255.0).min(a),
                q(px[2], 255.0).min(a),
            );
            data.push(a << 24 | r << 16 | g << 8 | b);
        }
    }
    data
}

/// Reducing each row to the target width first gives the pixels the average of the area gives
/// (the float sums are in another order: a channel may differ by 1 in the last place).
#[test]
fn rows_reduced_first_resample_like_columns_first() {
    let noise = |x: u32, y: u32| {
        let v = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503) ^ (x * y);
        [
            v as u8,
            (v >> 8) as u8,
            (v >> 16) as u8,
            255 - (v >> 4) as u8 % 90,
        ]
    };
    for (sw, sh) in [(1, 1), (7, 5), (64, 3), (3, 64), (101, 97), (640, 480)] {
        let data: Vec<u8> = (0..sh)
            .flat_map(|y| (0..sw).flat_map(move |x| noise(x, y)))
            .collect();
        for (w, h) in [
            (1, 1),
            (2, 3),
            (sw, sh),
            (sw / 2 + 1, sh * 2),
            (200, 150),
            (97, 61),
        ] {
            let frame = Frame {
                data: &data,
                width: sw,
                height: sh,
                stride: sw as usize * 4,
                channels: 4,
            };
            let new = resample(
                Frame {
                    data: &data,
                    ..frame
                },
                w,
                h,
            )
            .unwrap();
            let old = resampled_columns_first(&frame, w, h);
            let mut exact = 0;
            for (a, b) in new.data.iter().zip(&old) {
                let d = |s: u32| ((a >> s) & 0xff).abs_diff((b >> s) & 0xff);
                assert!(
                    d(0).max(d(8)).max(d(16)).max(d(24)) <= 1,
                    "{sw}×{sh} to {w}×{h}"
                );
                exact += usize::from(a == b);
            }
            assert!(
                exact * 10 >= old.len() * 9,
                "{sw}×{sh} to {w}×{h}: {exact}/{}",
                old.len()
            );
        }
    }
}

/// A PNG's side is at most 65,535, so no row of it (or sum over a row) is bigger than that:
/// 64,000,000 × 1 is within the pixel limit but made rows of 256 MB and sums of 1 GB.
#[test]
fn a_very_wide_png_is_refused_or_decoded_in_rows() {
    let ihdr = |w: u32, h: u32| {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    };
    for (w, h) in [
        (64_000_000, 1),
        (1, 64_000_000),
        (65_536, 1),
        (8_000_000, 8),
    ] {
        let png = ihdr(w, h);
        assert_eq!(header(&mut &png[..]), None, "{w}×{h}");
        assert_eq!(decode(&png, 100, 100), None, "{w}×{h}");
    }
    assert_eq!(
        header(&mut &ihdr(65_535, 976)[..]),
        Some((Format::Png, 65_535, 976))
    );
    // The widest, shrunk and enlarged, with a row a time held.
    let noise = |x: u32, y: u32| [x as u8, (x >> 8) as u8 ^ y as u8, (x * 7) as u8, 255];
    let png = png_bytes(MAX_SOURCE_SIDE, 4, noise);
    let b = decode(&png, 300, 2).unwrap();
    assert_eq!((b.width, b.height, b.data.len()), (300, 2, 600));
    let tall = png_bytes(1, MAX_SOURCE_SIDE, noise);
    let b = decode(&tall, 1, 300).unwrap();
    assert_eq!((b.width, b.height, b.data.len()), (1, 300, 300));
    let dir = scratch("wide");
    std::fs::write(dir.join("w.png"), &png).unwrap();
    assert_eq!(probe_file(&dir.join("w.png")), Some((MAX_SOURCE_SIDE, 4)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A PNG's last row is not trusted before the checksum of the data it came in, nor is the
/// image before its `IEND`: a bit wrong in the last `IDAT`'s checksum or `IEND`'s, or the image
/// cut anywhere in them, is refused whether the PNG is read by rows or whole.
#[test]
fn a_png_with_a_bad_final_checksum_is_refused() {
    let good = png_bytes(9, 7, |x, y| [x as u8 * 20, y as u8 * 30, 5, 255]);
    assert!(decode(&good, 9, 7).is_some());
    // The last `IDAT` ends 12 bytes (the whole `IEND`) before the end: its checksum is the 4
    // before that.
    let crc = good.len() - 12 - 4;
    assert_eq!(&good[good.len() - 8..good.len() - 4], b"IEND");
    let flip = |at: usize| {
        let mut bad = good.clone();
        bad[at] ^= 1;
        bad
    };
    let cases = [
        ("a bit of the checksum", flip(crc + 3)),
        ("a bit of the first byte of the checksum", flip(crc)),
        ("a bit of the pixel data", flip(crc - 3)),
        ("cut inside the checksum", good[..crc + 2].to_vec()),
        ("cut after the checksum, no IEND", good[..crc + 4].to_vec()),
        ("cut before the checksum", good[..crc].to_vec()),
        ("a bit of the IEND checksum", flip(good.len() - 1)),
    ];
    for (what, bad) in cases {
        for rows in [true, false] {
            assert_eq!(decode_png(&bad, 9, 7, rows), None, "{what} (rows: {rows})");
        }
        assert_eq!(decode(&bad, 4, 3), None, "{what}");
    }
    // What follows a good `IEND` isn't looked at.
    let mut trailing = good.clone();
    trailing.extend(b"trailing bytes");
    for rows in [true, false] {
        assert!(decode_png(&trailing, 9, 7, rows).is_some(), "rows: {rows}");
    }
}

/// An interlaced (Adam7) PNG is read whole, and held to its `IEND` as a PNG read by rows is: a
/// flipped bit in the checksum of `IEND`, or no `IEND`, shows the alt text, and bytes after it
/// don't matter.
#[test]
fn an_interlaced_png_is_checked_to_its_iend() {
    let good = fixture("interlaced.png");
    assert_eq!(good[28], 1, "Adam7");
    assert_eq!(&good[good.len() - 8..good.len() - 4], b"IEND");
    let (w, h, _) = header(&mut &good[..]).map(|(f, w, h)| (w, h, f)).unwrap();
    assert!(decode(&good, w, h).is_some());
    let flip = |at: usize| {
        let mut bad = good.clone();
        bad[at] ^= 1;
        bad
    };
    let cases = [
        ("a bit of the IEND checksum", flip(good.len() - 1)),
        ("no IEND", good[..good.len() - 12].to_vec()),
        ("a cut IEND", good[..good.len() - 6].to_vec()),
        (
            "a bit of the last IDAT's checksum",
            flip(good.len() - 12 - 1),
        ),
    ];
    for (what, bad) in cases {
        assert_eq!(decode(&bad, w, h), None, "{what}");
        assert_eq!(decode_png(&bad, w, h, true), None, "{what}");
    }
    let mut trailing = good.clone();
    trailing.extend(b"trailing bytes");
    assert!(decode(&trailing, w, h).is_some());
}

/// `png` with `chunks` (type, data) put after its `IHDR`, their checksums left zero: the
/// decoder only checks the ones it reads.
fn with_chunks(png: &[u8], chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    // Signature, then `IHDR`: length, type, 13 bytes of data, checksum.
    let at = 8 + 12 + 13;
    let mut out = png[..at].to_vec();
    for (kind, data) in chunks {
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(*kind);
        out.extend(*data);
        out.extend([0; 4]);
    }
    out.extend(&png[at..]);
    out
}

/// A PNG's metadata is never given to its decoder, which keeps `eXIf` as it comes and
/// decompresses `iCCP` and text up to its limit: a 100 KB file held a 100 MiB profile (peak RSS
/// 117 MB for a 1 × 1 image), none of it part of a decode's cost. The profile here is 64 MiB.
#[test]
fn png_metadata_is_not_given_to_the_decoder() {
    let plain = png_bytes(3, 2, |x, y| [x as u8 * 80, y as u8 * 120, 9, 255]);
    let mut info = png::Info::with_size(3, 2);
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(vec![0u8; 64 << 20].into());
    info.exif_metadata = Some(vec![7u8; 4 << 20].into());
    let mut with = Vec::new();
    let mut enc = png::Encoder::with_info(&mut with, info).unwrap();
    enc.add_text_chunk("Comment".into(), "x".repeat(1 << 20))
        .unwrap();
    enc.add_ztxt_chunk("Zipped".into(), "y".repeat(8 << 20))
        .unwrap();
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().unwrap();
    let mut data = Vec::new();
    for y in 0..2u32 {
        for x in 0..3u32 {
            data.extend([x as u8 * 80, y as u8 * 120, 9, 255]);
        }
    }
    w.write_image_data(&data).unwrap();
    w.finish().unwrap();
    // The metadata is really there (4 MiB of `eXIf` alone), and not read.
    assert!(with.len() > plain.len() + 1000);
    for kind in [&b"iCCP"[..], b"eXIf", b"tEXt", b"zTXt"] {
        assert!(with.windows(4).any(|w| w == kind), "{kind:?}");
    }
    let mut seen = Vec::new();
    std::io::Read::read_to_end(&mut PngChunks::new(&with), &mut seen).unwrap();
    assert!(seen.len() < 200, "{} bytes", seen.len());
    for kind in [&b"iCCP"[..], b"eXIf", b"tEXt", b"zTXt"] {
        assert!(!seen.windows(4).any(|w| w == kind), "{kind:?}");
    }
    // The decoder with the limit `decode` gives it refuses the file as it is.
    let limits = png::Limits { bytes: 1 << 20 };
    let raw = png::Decoder::new_with_limits(std::io::Cursor::new(&with[..]), limits);
    assert!(raw.read_info().is_err());
    assert_eq!(decode(&with, 3, 2), decode(&plain, 3, 2));
    assert!(decode(&with, 3, 2).is_some());
    for rows in [true, false] {
        assert_eq!(
            decode_png(&with, 2, 2, rows),
            decode_png(&plain, 2, 2, rows),
            "rows {rows}"
        );
    }
    // Metadata anywhere in the file, a chunk of a type nothing knows, and animation chunks
    // are skipped; a critical chunk nothing knows still makes it undecodable.
    let odd = with_chunks(
        &plain,
        &[(b"acTL", &[0; 8]), (b"abCD", &[1; 99]), (b"eXIf", &[0; 5])],
    );
    assert_eq!(decode(&odd, 3, 2), decode(&plain, 3, 2));
    let critical = with_chunks(&plain, &[(b"ABCD", &[1; 9])]);
    assert_eq!(decode(&critical, 3, 2), None);
    // Cut off inside a skipped chunk's length, or inside a kept one.
    for cut in [8 + 12 + 13 + 5, plain.len() - 20] {
        assert_eq!(decode(&odd[..cut], 3, 2), None, "cut at {cut}");
    }
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
    // A JPEG cut short after its header decodes as far as it goes, the rest gray, as in a
    // browser; one without a decodable scan (the tables cut off, the data garbage) doesn't.
    let wide = fixture("wide.jpg");
    let mut cut = wide[..400].to_vec();
    cut.extend([0xFF, 0xD9]);
    let b = decode(&cut, 10, 10).expect("a truncated scan decodes as far as it goes");
    assert_eq!((b.width, b.height), (10, 10));
    for (name, bad) in [
        ("tables cut off", {
            let mut v = wide[..60].to_vec();
            v.extend([0xFF, 0xD9]);
            v
        }),
        ("garbage after the SOI", {
            let mut v = vec![0xFF, 0xD8];
            v.extend((0..200u32).map(|i| (i * 37 + 11) as u8));
            v
        }),
        ("header only", {
            let sof = wide.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
            wide[..sof + 19].to_vec()
        }),
    ] {
        assert_eq!(decode(&bad, 10, 10), None, "{name}");
    }
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
    // Room for two bitmaps of 4 pixels (and their overhead), not three.
    let one = 16 + SLOT_OVERHEAD;
    let mut c = Cache::new(2 * one + 20);
    c.insert(Key::Local(1), bm(4));
    c.frame += 1;
    c.insert(Key::Local(2), bm(4));
    c.frame += 1;
    // Image 1 painted again: image 2 is now the least recent.
    assert!(c.pixels(&Key::Local(1), 4, 1).is_some());
    c.insert(Key::Local(3), bm(4));
    assert!(c.has(&Key::Local(1), 4, 1) && c.has(&Key::Local(3), 4, 1));
    assert!(!c.has(&Key::Local(2), 4, 1));
    assert_eq!(c.bytes, 2 * one);
    // Nothing painted this frame is evicted, even over the budget.
    c.frame += 1;
    assert!(c.pixels(&Key::Local(1), 4, 1).is_some());
    assert!(c.pixels(&Key::Local(3), 4, 1).is_some());
    c.insert(Key::Local(4), bm(4));
    assert_eq!(c.slots.len(), 3);
    assert_eq!(c.bytes, 3 * one);
    // Another size of an image stands in while the right one is decoded; the same size
    // replaces its slot.
    assert_eq!(c.pixels(&Key::Local(1), 8, 2).map(|b| b.width), Some(4));
    c.insert(Key::Local(1), bm(4));
    assert_eq!(c.slots.iter().filter(|s| s.key == Key::Local(1)).count(), 1);
    c.retain(|s| s.key != Key::Local(1));
    assert_eq!(c.bytes, 2 * one);
}

/// A fetched body that arrives is not "painted this frame": one for an image far from the
/// viewport is the first to go when room is needed, and what the fetchers hold is room too.
/// (Bodies were stored as used in the current frame, so a stream of them pushed out what was
/// on screen, or stayed, unpainted, over the budget.)
#[test]
fn fetched_bytes_are_evictable_and_what_fetchers_hold_counts() {
    let body = |n: usize| Data::Bytes(Arc::new(vec![0u8; n]));
    let url = |n: u32| Key::Remote(format!("https://x.y/{n}.png").into());
    // A slot of a body of 100 bytes costs this (the URLs are 17 bytes).
    let one = 100 + SLOT_OVERHEAD + str_bytes("https://x.y/1.png");
    let mut c = Cache::new(one * 5 / 2);
    c.frame = 5;
    c.insert(url(1), body(100));
    c.insert(url(2), body(100));
    // The third doesn't fit: the first, unpainted, goes (not the one arriving).
    c.insert(url(3), body(100));
    assert_eq!(c.bytes, 2 * one);
    assert!(c.peek_bytes(&"https://x.y/1.png".into()).is_none());
    assert!(c.peek_bytes(&"https://x.y/3.png".into()).is_some());
    // A body something was made of this frame stays.
    let two: Arc<str> = "https://x.y/2.png".into();
    assert!(c.bytes_of(&two).is_some());
    c.insert(url(4), body(100));
    assert!(c.peek_bytes(&two).is_some(), "painted this frame");
    assert!(c.peek_bytes(&"https://x.y/3.png".into()).is_none());
    // What is set aside for the fetchers comes out of the budget as well.
    c.frame += 1;
    c.fetching = one;
    c.evict();
    assert_eq!(c.bytes, one, "one of two bodies goes");
    c.fetching = 2 * one;
    c.evict();
    assert_eq!(c.bytes, 0);
    // A body bigger than the room isn't evicted by arriving: it is the only one.
    c.fetching = 0;
    c.insert(url(5), body(3 * one));
    assert_eq!(
        c.bytes,
        3 * one + SLOT_OVERHEAD + str_bytes("https://x.y/5.png")
    );
}

/// A buffer costs what it is allocated, not what it holds, and every slot costs a little more
/// than its buffer: a 1,333-byte image in a 256 KB buffer was charged 1,333, and 12,288 tiny
/// bodies raised the resident size by 53 MB while charging 0.8 MB.
#[test]
fn a_slot_costs_its_allocation_and_its_bookkeeping() {
    let mut loose = Vec::with_capacity(256 << 10);
    loose.extend_from_slice(&[0u8; 1333]);
    let url: Arc<str> = "https://x.y/a.png".into();
    let slot = |data| Slot {
        key: Key::Remote(url.clone()),
        data,
        used: 0,
    };
    assert_eq!(
        slot(Data::Bytes(Arc::new(loose))).cost(),
        (256 << 10) + SLOT_OVERHEAD + str_bytes(&url)
    );
    let mut pixels = Vec::with_capacity(100);
    pixels.push(0u32);
    let bitmap = Bitmap {
        width: 1,
        height: 1,
        data: pixels,
    };
    assert_eq!(
        Slot {
            key: Key::Local(1),
            data: Data::Pixels(bitmap),
            used: 0
        }
        .cost(),
        400 + SLOT_OVERHEAD
    );
    // Thousands of tiny bodies (69 bytes each) are charged their overhead too.
    let mut c = Cache::new(CACHE_BYTES);
    for n in 0..12_288u32 {
        c.insert(
            Key::Remote(format!("https://x.y/{n}.png").into()),
            Data::Bytes(Arc::new(vec![0u8; 69])),
        );
    }
    assert_eq!(c.slots.len(), 12_288);
    assert!(
        c.bytes >= 12_288 * (69 + SLOT_OVERHEAD + STR_OVERHEAD),
        "{} charged",
        c.bytes
    );
    assert_eq!(c.bytes, c.slots.iter().map(Slot::cost).sum::<usize>());
}

/// What the fetchers may hold is set aside before they start and as long as any run, not
/// sampled as they grow: the cache is made small when the fetches are asked for, and a body
/// taken is in the cache before its credit goes (it was a snapshot of what the fetchers held
/// at the last poll, so fetches growing after it went over the budget: eight cached 4 MiB
/// bodies and four unfinished 7 MiB ones held 46 MB with old ones evictable).
#[cfg(feature = "remote-images")]
#[test]
fn the_cache_is_made_room_for_the_fetchers_before_they_start() {
    use crate::remote::FETCH_RESERVE;
    let doc = Doc::parse("![a](https://x.y/a.png) ![b](https://x.y/b.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.attach(&doc, &mut layout);
    // The cache is full of pixels that aren't in view.
    for id in 10..18u32 {
        images.cache.insert(
            Key::Local(id),
            Data::Pixels(Bitmap {
                width: 1024,
                height: 768,
                data: vec![0; 1024 * 768],
            }),
        );
    }
    images.begin_frame();
    assert!(images.cache.bytes > CACHE_BYTES - FETCH_RESERVE);
    // Remote images are off: nothing is set aside, nothing evicted.
    images.request(&[], 0);
    assert_eq!(images.cache.fetching, 0);
    // Turned on, with the fetches not started yet when the cache is made small.
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let fetcher: crate::remote::Fetcher = {
        let gate = gate.clone();
        Arc::new(move |_, grant| {
            let mut open = gate.0.lock().unwrap();
            while !*open {
                open = gate.1.wait(open).unwrap();
            }
            assert!(grant(256 << 10));
            Ok((Arc::new(vec![0u8; 256 << 10]), 2, 1))
        })
    };
    images.remote.set_fetcher(fetcher);
    images.set_remote(true, &doc, &mut layout);
    images.request(&[], 0);
    assert_eq!(images.cache.fetching, FETCH_RESERVE);
    assert!(
        images.cache.bytes + FETCH_RESERVE <= CACHE_BYTES,
        "{} kept",
        images.cache.bytes
    );
    // Taken, they are in the cache, and what is set aside goes once nothing is running.
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    wait(&mut images, &doc, &mut layout, |i| {
        i.cache
            .slots
            .iter()
            .filter(|s| matches!(s.key, Key::Remote(_)))
            .count()
            == 2
    });
    assert_eq!(images.cache.fetching, 0);
    assert!(images.cache.bytes <= CACHE_BYTES);
}

/// Wait for `images` to hand over results until `done` holds (or fail after a while): a poll for
/// each wake, as the window does (the first at once, for what came before).
fn wait(images: &mut Images, doc: &Doc, layout: &mut Layout, done: impl Fn(&mut Images) -> bool) {
    let t = Instant::now();
    let mut seen = images.wakes();
    images.poll(doc, layout);
    while !done(images) {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        if images.wakes() == seen {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        seen = images.wakes();
        images.poll(doc, layout);
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
    images.request(&[(1, 30, 20), (2, 10, 10), (3, 5, 5), (4, 5, 5)], 4);
    wait(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 30, 20).is_some()
    });
    assert_eq!(images.pixels(1, 30, 20).unwrap().data[0], 0xFF00_00FF);
    let t = Instant::now();
    while layout.image_size(2) != Some(ImageSize::Alt) {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        images.poll(&doc, &mut layout);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(images.pixels(3, 5, 5).is_none() && images.pixels(4, 5, 5).is_none());
    // Another document drops the local images; its results are told apart from late ones.
    images.attach(&doc, &mut layout);
    assert!(images.pixels(1, 30, 20).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The boxes layout makes for the image at `src` of `doc`, as `View` asks for them:
/// (id, width, height).
fn boxes_of(layout: &Layout) -> Vec<(u32, u32, u32)> {
    layout
        .geoms()
        .iter()
        .filter_map(|g| g.content.as_deref())
        .flat_map(|c| c.boxes.iter())
        .map(|b| {
            let size = |v: f32| v.round().max(1.0) as u32;
            (b.b.id, size(b.b.width), size(b.b.ascent + b.b.descent))
        })
        .collect()
}

/// A tall, narrow image's box is decoded at exactly its size, so what was decoded is what is
/// asked for next time: nothing is decoded again and again (this used to keep the viewer
/// busy forever).
#[test]
fn a_tall_images_box_is_found_once_decoded() {
    let dir = scratch("tall");
    // 4 × 20000: its box can't be as tall as the image.
    std::fs::write(
        dir.join("t.png"),
        png_bytes(4, 20_000, |_, y| [y as u8, 0, 0, 255]),
    )
    .unwrap();
    let doc = Doc::parse("![t](t.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 600.0);
    let boxes = boxes_of(&layout);
    assert_eq!(boxes.len(), 1);
    let (id, w, h) = boxes[0];
    assert!(w >= 1 && h == MAX_SIDE, "{w}×{h}");
    images.request(&boxes, 1);
    wait(&mut images, &doc, &mut layout, |i| {
        i.cache.has(&Key::Local(1), w, h)
    });
    // Asked for again, as every frame does: nothing is queued (so nothing can arrive).
    images.begin_frame();
    images.request(&boxes, 1);
    assert!(images.is_idle(), "the box found is not decoded again");
    assert!(!images.poll(&doc, &mut layout));
    use crate::render::Pixels;
    assert!(images.pixels(id, w, h).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A window of images bigger than the cache keeps what it has: the ones in view are never
/// evicted, and of the rest only as many as fit beside them are decoded, so no image evicts
/// another to be decoded again by the next request.
#[test]
fn images_beyond_the_cache_are_not_decoded_over_and_over() {
    use crate::render::Pixels;
    let dir = scratch("thrash");
    for i in 1..=4 {
        std::fs::write(
            dir.join(format!("{i}.png")),
            png_bytes(100, 100, |_, _| [i as u8, 0, 0, 255]),
        )
        .unwrap();
    }
    let doc = Doc::parse("![1](1.png) ![2](2.png) ![3](3.png) ![4](4.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    // Room for three of them (40,000 bytes each, as boxes: a fourth would make 160,000 of
    // them) beside the decode of another (its file, its result and what decoding holds). One that
    // would cross the line is the one that isn't kept: the budget is not gone over by a box.
    images.cache.cap = images.cost(4, 100, 100) + 130_000;
    // Image 1 in view, the others a screen away.
    let boxes = [(1, 100, 100), (2, 100, 100), (3, 100, 100), (4, 100, 100)];
    // A frame per result: results straddle frames, as the decodes of big images do. It is
    // settled when a frame asks for nothing the cache doesn't have (the next would be the same).
    for frame in 0..60 {
        images.begin_frame();
        let _ = images.pixels(1, 100, 100);
        images.request(&boxes, 1);
        if images.is_idle() {
            break;
        }
        wait(&mut images, &doc, &mut layout, |i| i.is_idle());
        assert!(frame < 59, "still decoding after {frame} frames");
    }
    let has = |i, images: &Images| images.cache.has(&Key::Local(i), 100, 100);
    assert!(has(1, &images) && has(2, &images) && has(3, &images));
    assert!(!has(4, &images), "the fourth doesn't fit");
    let _ = std::fs::remove_dir_all(&dir);
}

/// What an image costs to keep is the document's own string, shared, and the size of a [`Src`]:
/// the path of a local one is worked out when a job or a file's size wants it, and is not kept,
/// so a document of long paths holds what one of short ones does (it kept a copy of each,
/// resolved: 34,000 images of 1,000-character paths were 34.9 MB beside a budget of 33.5).
#[test]
fn a_local_images_path_is_not_kept() {
    assert!(size_of::<Src>() <= 24, "{}", size_of::<Src>());
    let attached = |len: usize| {
        let src: String = (0..100)
            .map(|i| format!("![p]({}{i}.png) ", "d".repeat(len)))
            .collect();
        let doc = Doc::parse(&src);
        let mut layout = Layout::new(&doc, crate::layout::Params::default());
        let mut images = Images::default();
        images.set_base(Some(PathBuf::from("/docs")));
        images.attach(&doc, &mut layout);
        assert!(images.srcs.iter().all(|s| matches!(s, Src::File(_))));
        // The strings are the document's: not copies.
        for (src, img) in images.srcs.iter().zip(&doc.images) {
            match src {
                Src::File(s) => assert!(Arc::ptr_eq(s, &img.src)),
                _ => unreachable!(),
            }
        }
        images.bookkeeping_bytes()
    };
    assert_eq!(attached(5), attached(5_000));
    // And what is charged for a document is known from its numbers: before it is made.
    assert_eq!(attach_bytes(100, 0) % size_of::<Src>(), 0);
    assert!(attach_bytes(100, 0) >= 100 * size_of::<Src>());
}

/// A document whose images cost more to keep than the budget has none loaded, and says so:
/// every image shows its alt text, and nothing is made for it (the tables are charged, and made
/// room for, before they are made, so there is nothing to give back). A smaller one beside it is
/// loaded as always.
#[test]
fn a_document_whose_images_cost_more_than_the_budget_has_none() {
    let dir = scratch("none");
    std::fs::write(
        dir.join("a.png"),
        png_bytes(30, 20, |_, _| [0, 0, 255, 255]),
    )
    .unwrap();
    let src: String = (0..1_000).map(|_| "![p](a.png) ").collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.cache.cap = attach_bytes(1_000, 0) - 1;
    images.attach(&doc, &mut layout);
    assert!(images.srcs.is_empty(), "nothing made");
    assert!(images.key(1).is_none());
    assert_eq!(images.cache.overhead, images.bookkeeping_bytes());
    assert!(images.cache.overhead < attach_bytes(1_000, 0) / 4);
    // The layout is told the images are alt text, though the file is there.
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 600.0);
    assert_eq!(layout.image_size(1), Some(ImageSize::Alt));
    // Asked for, they are nothing.
    images.begin_frame();
    images.request(&[(1, 10, 10)], 1);
    assert!(images.is_idle());
    // The same budget has room for a document a tenth of that.
    let src: String = (0..100).map(|_| "![p](a.png) ").collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    images.attach(&doc, &mut layout);
    assert_eq!(images.srcs.len(), 100);
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 600.0);
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(30, 20)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Attaching makes room for the tables before it makes them: what is cached is evicted to it,
/// including what the old document had painted (it is not painted any more), and not more than
/// that.
#[test]
fn attach_evicts_for_its_tables_and_no_more() {
    let slot = |n: u32| {
        Data::Pixels(Bitmap {
            width: n,
            height: 1,
            data: vec![0; n as usize],
        })
    };
    let url = |i: u32| Key::Remote(Arc::from(format!("http://x/{i}")));
    let doc = Doc::parse("![a](a.png) ![b](b.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(PathBuf::from("/docs")));
    // Two bitmaps painted in the last frame of the old document, with room for both and the
    // tables of this one.
    images.cache.insert(url(1), slot(10_000));
    images.cache.insert(url(2), slot(10_000));
    images.begin_frame();
    images.cache.touch(&url(1), 10_000, 1);
    images.cache.touch(&url(2), 10_000, 1);
    images.cache.cap = images.cache.bytes + attach_bytes(2, 0) + 4_000;
    images.attach(&doc, &mut layout);
    assert_eq!(images.stats().decoded, 2, "room enough: nothing goes");
    // Room for one: the least recently painted is evicted, though they were painted in the last
    // frame of the old document.
    images.cache.cap = images.cache.bytes + attach_bytes(2, 0) - 1_000;
    images.attach(&doc, &mut layout);
    assert_eq!(images.stats().decoded, 1, "{}", images.memory_report());
}

/// The table of the sizes of local files holds what the last requests asked for, cut to them
/// once it has twice as many: a document of thousands of images read from the top to the bottom
/// doesn't keep an entry for each.
#[test]
fn the_table_of_file_sizes_follows_the_requests() {
    let src: String = (0..200).map(|i| format!("![p](p{i}.png) ")).collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(PathBuf::from("/nowhere")));
    images.attach(&doc, &mut layout);
    // Down the document, a screen of 20 at a time: none is ever cached (the files aren't
    // there), so each is asked about.
    let mut most = 0;
    for top in (1..=200u32).step_by(10) {
        let boxes: Vec<_> = (top..top + 20)
            .filter(|&id| id <= 200)
            .map(|id| (id, 10, 10))
            .collect();
        images.begin_frame();
        images.request(&boxes, 2);
        most = most.max(images.costs.len());
    }
    assert!(most <= 3 * 20 + 16, "{most} entries");
    assert!(images.costs.len() <= 3 * 20 + 16);
    // And the capacity is charged as it is.
    assert!(
        images.bookkeeping_bytes()
            >= table_bytes(
                images.costs.capacity(),
                size_of::<(u32, Option<FileInfo>)>()
            )
    );
}

/// A decode that fits the budget by itself but not beside what the document's images cost to
/// keep is refused, and its image shows its alt text, instead of being let in over the budget
/// (what the document's tables hold is charged by [`Images::bookkeeping_bytes`]): the same
/// decode, with the same room, is run beside a document with a tenth of the images.
#[test]
fn a_decode_the_bookkeeping_leaves_no_room_for_is_refused() {
    let dir = scratch("refuse-unit");
    std::fs::write(
        dir.join("a.png"),
        png_bytes(100, 100, |_, _| [1, 2, 3, 255]),
    )
    .unwrap();
    let run = |others: usize| {
        let mut src = String::from("![a](a.png)");
        for i in 0..others {
            src += &format!(" ![p](p{i}.png)");
        }
        let doc = Doc::parse(&src);
        let mut layout = Layout::new(&doc, crate::layout::Params::default());
        let mut images = Images::default();
        images.set_base(Some(dir.clone()));
        images.attach(&doc, &mut layout);
        // Room for the decode and 40,000 bytes of tables, which 2,000 images are over.
        images.cache.cap = images.cost(1, 100, 100) + 40_000;
        images.begin_frame();
        images.request(&[(1, 100, 100)], 1);
        wait(&mut images, &doc, &mut layout, |i| {
            i.stats().decoded == 1 || i.stats().failed == 1
        });
        images.stats()
    };
    assert_eq!(run(100).decoded, 1, "room beside 100 images");
    let refused = run(2_000);
    assert_eq!((refused.decoded, refused.failed), (0, 1), "{refused:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A decode's own memory is kept free of the cache: slots not painted this frame are evicted
/// to make room for the biggest job, and none painted this frame.
#[test]
fn a_decode_is_given_room_by_evicting_what_is_not_painted() {
    use crate::render::Pixels;
    let dir = scratch("room");
    std::fs::write(dir.join("a.jpg"), fixture("wide.jpg")).unwrap();
    let doc = Doc::parse("![a](a.jpg) ![b](b.png) ![c](c.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    // The JPEG is 600 × 200: a frame of 360,000 bytes of RGB, the metadata allowance, the rows of
    // its 3 components (4:2:0, so MCUs of 16 rows, 608 pixels wide), the resampler's tables and
    // rows, the result (600 × 200 pixels), and its file.
    let file = std::fs::metadata(dir.join("a.jpg")).unwrap().len() as usize;
    let decode = 360_000
        + JPEG_META_COST
        + 3 * 608 * 16 * JPEG_ROW_BYTES
        + resample_cost(600, 200, 600, 200)
        + 480_000;
    assert_eq!(images.cost(1, 600, 200), file + decode);
    // Two other slots of 100,000 bytes beside it, in a cache that has no room for them and the
    // decode: one painted this frame.
    let slot = |n| {
        Data::Pixels(Bitmap {
            width: n,
            height: 1,
            data: vec![0; n as usize],
        })
    };
    images.cache.cap = 500_000 + JPEG_META_COST;
    images.srcs[1] = Src::File(Arc::from("b.png"));
    images.srcs[2] = Src::File(Arc::from("c.png"));
    images.cache.insert(Key::Local(2), slot(25_000));
    images.cache.insert(Key::Local(3), slot(25_000));
    images.begin_frame();
    let _ = images.pixels(3, 25_000, 1);
    images.request(&[(1, 600, 200)], 1);
    assert_eq!(images.cache.reserve, file + decode);
    assert!(
        !images.cache.has(&Key::Local(2), 25_000, 1),
        "evicted for the decode"
    );
    assert!(images.cache.has(&Key::Local(3), 25_000, 1), "painted: kept");
    // Once nothing is queued nothing is kept free.
    images.request(&[], 0);
    assert_eq!(
        images.cache.reserve,
        images.worker.as_ref().map_or(0, Worker::busy_cost)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A segment with marker `FF code` and `data`.
fn segment(code: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, code];
    out.extend(((data.len() + 2) as u16).to_be_bytes());
    out.extend(data);
    out
}

/// `jpeg` with `segments` put before its `n`th start of scan (0 for the first).
fn with_segments(jpeg: &[u8], n: usize, segments: &[Vec<u8>]) -> Vec<u8> {
    let sos = jpeg
        .windows(2)
        .enumerate()
        .filter(|(_, w)| *w == [0xFF, 0xDA])
        .nth(n)
        .unwrap()
        .0;
    let mut out = jpeg[..sos].to_vec();
    out.extend(segments.concat());
    out.extend(&jpeg[sos..]);
    out
}

/// A JPEG's decoder keeps its EXIF, XMP, ICC and IPTC segments, none of which is used: more than
/// [`MAX_JPEG_META`] of them are cut out of the bytes in place, and nothing else is.
#[test]
fn jpeg_metadata_over_the_limit_is_cut_out_in_place() {
    for name in ["wide.jpg", "progressive.jpg", "gray.jpg"] {
        let plain = fixture(name);
        // What stays: JFIF, a comment, Adobe's color transform. What goes: the rest.
        let mut keep = vec![segment(
            0xFE,
            b"a comment with an FF E1 inside: \xFF\xE1\xFF\xFF is only text",
        )];
        if name != "gray.jpg" {
            keep.push(segment(0xEE, b"Adobe\0\x64\0\0\0\0\x01"));
        }
        let gone = [
            segment(0xE1, &[b'E'; 65_000]),
            segment(0xE2, &[b'I'; 40_000]),
            segment(0xED, &[b'P'; 30_000]),
            segment(0xE1, &[b'X'; 10]),
        ];
        let mut segments = Vec::new();
        for (a, b) in keep.iter().zip(&gone) {
            segments.push(a.clone());
            segments.push(b.clone());
        }
        segments.extend(gone[keep.len()..].iter().cloned());
        let with = with_segments(&plain, 0, &segments);
        // The same with everything that stays, and none that goes.
        let expect = with_segments(&plain, 0, &keep);
        let want = decode(&expect, 8, 8).unwrap();
        assert!(jpeg_metadata(&with) > MAX_JPEG_META);
        let mut cut = with.clone();
        assert!(shed_jpeg_metadata(&mut cut), "{name}");
        assert_eq!(cut, expect, "{name}: what stays is what was kept");
        assert!(cut.capacity() <= cut.len() + 64, "{name}: cut to size");
        // Decoding is the same either way (a copy is cut first if the bytes are borrowed).
        assert_eq!(decode(&with, 8, 8).as_ref(), Some(&want), "{name}");
        assert_eq!(decode(&cut, 8, 8).as_ref(), Some(&want), "{name}");
        // Under the limit nothing is touched.
        let little = with_segments(&plain, 0, &[segment(0xE1, &[b'E'; 60_000])]);
        let mut same = little.clone();
        assert!(shed_jpeg_metadata(&mut same));
        assert_eq!(same, little, "{name}");
        // Other formats are left alone, however many `FF E1` they hold.
        let mut not_jpeg = [0x89, b'P', 0xFF, 0xE1, 0xFF, 0xFF].repeat(100_000);
        let copy = not_jpeg.clone();
        assert!(shed_jpeg_metadata(&mut not_jpeg));
        assert_eq!(not_jpeg, copy);
    }
}

/// Segments between the scans of a progressive JPEG (where the decoder reads markers too) go
/// as well.
#[test]
fn metadata_between_scans_is_cut_out() {
    let plain = fixture("progressive.jpg");
    let scans = plain.windows(2).filter(|w| *w == [0xFF, 0xDA]).count();
    assert!(scans > 2, "{scans} scans");
    let want = decode(&plain, 8, 8);
    let with = with_segments(&plain, 2, &vec![segment(0xE1, &[b'E'; 65_000]); 4]);
    assert!(jpeg_metadata(&with) > MAX_JPEG_META);
    let mut cut = with.clone();
    assert!(shed_jpeg_metadata(&mut cut));
    assert_eq!(cut, plain);
    assert_eq!(decode(&cut, 8, 8), want);
    assert_eq!(decode(&with, 8, 8), want);
}

/// What is inside a segment that isn't metadata is payload, however much it looks like metadata:
/// a comment or an application segment (`APP12`) full of `FF E1 FF FF` is skipped by the decoder
/// and by [`JpegWalk`] alike, so it is not counted, not cut, and the image decodes (the
/// bytes were once counted as 16,000 segments of 65,535, and the image refused).
#[test]
fn marker_pairs_inside_a_segments_payload_are_not_metadata() {
    let plain = fixture("wide.jpg");
    let want = decode(&plain, 8, 8).unwrap();
    let pairs = [0xFF, 0xE1, 0xFF, 0xFF].repeat(16_000);
    for code in [0xFE, 0xEC, 0xE3, 0xE0] {
        let bytes = with_segments(&plain, 0, &[segment(code, &pairs)]);
        assert_eq!(jpeg_metadata(&bytes), 0, "FF {code:X}");
        assert_eq!(decode(&bytes, 8, 8).as_ref(), Some(&want), "FF {code:X}");
        let mut same = bytes.clone();
        assert!(shed_jpeg_metadata(&mut same));
        assert_eq!(same, bytes, "FF {code:X}: nothing to cut");
    }
    // Beside real metadata over the limit: that is cut, the payload isn't.
    let comment = segment(0xFE, &pairs);
    let mut bytes = with_segments(
        &plain,
        0,
        &[
            comment.clone(),
            segment(0xE1, &[b'E'; 65_000]),
            segment(0xE1, &[b'E'; 65_000]),
            segment(0xE1, &[b'E'; 10_000]),
        ],
    );
    assert!(jpeg_metadata(&bytes) > MAX_JPEG_META);
    assert!(shed_jpeg_metadata(&mut bytes));
    assert_eq!(bytes, with_segments(&plain, 0, &[comment]));
    assert_eq!(decode(&bytes, 8, 8).as_ref(), Some(&want));
}

/// What [`jpeg_metadata`] counts is what the decoder reads as metadata, wherever it finds it: for
/// every way of putting a segment in a file that the decoder takes (fill bytes before the marker, a
/// stuffed zero, between scans, after the last), it is counted, and what the decoder has kept
/// of it is within the count. (Checked against zune-jpeg 0.5: its marker reading is what this
/// follows.)
#[test]
fn metadata_is_counted_where_the_decoder_reads_it() {
    use zune_jpeg::zune_core::{
        bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
    };
    let exif = |n: usize| {
        let mut d = b"Exif\0\0".to_vec();
        d.extend(vec![b'E'; n]);
        d
    };
    let seg = |n: usize| segment(0xE1, &exif(n));
    let plain = fixture("wide.jpg");
    let eoi = plain.len() - 2;
    assert_eq!(&plain[eoi..], [0xFF, 0xD9]);
    let mut fill = vec![0xFF];
    fill.extend(seg(40_000));
    let mut stuffed = vec![0xFF, 0x00];
    stuffed.extend(seg(40_000));
    let mut after = plain[..eoi].to_vec();
    after.extend(seg(40_000));
    after.extend([0xFF, 0xD9]);
    let cases = [
        ("plain", with_segments(&plain, 0, &[seg(40_000)])),
        ("fill byte", with_segments(&plain, 0, &[fill])),
        ("stuffed zero", with_segments(&plain, 0, &[stuffed])),
        ("after the scan", after),
    ];
    for (name, bytes) in cases {
        let counted = jpeg_metadata(&bytes);
        assert!(counted >= 40_000, "{name}: {counted}");
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(&bytes[..]), options);
        d.decode().unwrap();
        let kept = d.exif().map_or(0, Vec::len);
        assert_eq!(kept, 40_000, "{name}: the decoder keeps it");
        assert!(kept <= counted, "{name}");
        // And it is cut out, whichever it was.
        let mut cut = with_segments(&bytes, 0, &[seg(60_000), seg(60_000)]);
        assert!(shed_jpeg_metadata(&mut cut), "{name}");
        assert_eq!(jpeg_metadata(&cut), 0, "{name}");
        assert_eq!(decode(&cut, 8, 8), decode(&plain, 8, 8), "{name}");
    }
}

/// What the decoder keeps of what is put into a JPEG anywhere, with markers, fill, stuffed zeros,
/// lengths and payloads made of the bytes that matter most, is never more than is counted: the
/// bound the cache's budget relies on (checked against zune-jpeg 0.5, whose marker reading
/// [`JpegWalk`] follows). A fixed seed, so a failure repeats.
#[test]
fn the_decoder_never_keeps_more_than_is_counted() {
    use zune_jpeg::zune_core::{
        bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
    };
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as usize % n
    };
    let mut kept_some = 0;
    for name in ["wide.jpg", "progressive.jpg", "gray.jpg"] {
        let plain = fixture(name);
        for round in 0..150 {
            let mut bytes = plain.clone();
            for _ in 0..1 + next(3) {
                // Where: at a marker (before a segment, a scan, the end), or anywhere (inside a
                // segment, inside the entropy-coded data).
                let starts: Vec<usize> = bytes
                    .windows(2)
                    .enumerate()
                    .filter(|(_, w)| {
                        w[0] == 0xFF
                            && matches!(w[1], 0xC0 | 0xC2 | 0xC4 | 0xDA | 0xDB | 0xE0 | 0xD9)
                    })
                    .map(|(i, _)| i)
                    .collect();
                let at = if next(8) == 0 {
                    2 + next(bytes.len() - 2)
                } else {
                    starts[next(starts.len())]
                };
                let mut put = Vec::new();
                for _ in 0..next(3) {
                    put.push(if next(2) == 0 { 0xFF } else { 0x00 });
                }
                let code = [0xE1, 0xE1, 0xE2, 0xED, 0xFE, 0xEC, 0xE0, 0xEE, 0xD0, 0xFF][next(10)];
                let body: Vec<u8> = (0..next(300))
                    .map(|_| [0xFF, 0xE1, b'E', b'E', b'E', b'E', b'E', 0xE2][next(8)])
                    .collect();
                let prefix: &[u8] = [
                    &b"Exif\0\0"[..],
                    b"ICC_PROFILE\0\x01\x01",
                    b"",
                    b"Photoshop 3.0\0",
                ][next(4)];
                let mut data = prefix.to_vec();
                data.extend(body);
                put.extend([0xFF, code]);
                // The length is right, or whatever it is.
                let l = if next(10) == 0 {
                    next(70_000) as u16
                } else {
                    (data.len() + 2) as u16
                };
                put.extend(l.to_be_bytes());
                put.extend(data);
                let tail = bytes.split_off(at);
                bytes.extend(put);
                bytes.extend(tail);
            }
            let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
            let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(&bytes[..]), options);
            let _ = d.decode();
            let kept = d.exif().map_or(0, Vec::len)
                + d.xmp().map_or(0, Vec::len)
                + d.iptc().map_or(0, Vec::len)
                + d.icc_profile().map_or(0, |v| v.len());
            kept_some += usize::from(kept > 0);
            let counted = jpeg_metadata(&bytes);
            assert!(
                kept <= counted,
                "{name} round {round}: kept {kept}, counted {counted}"
            );
        }
    }
    assert!(
        kept_some > 50,
        "the test puts metadata where it is read: {kept_some}"
    );
}

/// A decode's result is held (and its cost kept free of the cache) until the main thread takes
/// it: the worker starts no other job meanwhile, so results can't pile up unpolled outside the
/// budget.
#[test]
fn the_worker_waits_for_its_result_to_be_taken() {
    use crate::render::Pixels;
    let dir = scratch("ack");
    for i in 0..3 {
        std::fs::write(
            dir.join(format!("{i}.png")),
            png_bytes(40, 30, |x, y| [x as u8, y as u8, i, 255]),
        )
        .unwrap();
    }
    let doc = Doc::parse("![a](0.png) ![b](1.png) ![c](2.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woken.clone();
    images.set_wake(Arc::new(move || {
        w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    images.attach(&doc, &mut layout);
    let woken = || woken.load(std::sync::atomic::Ordering::SeqCst);
    images.request(&[(1, 40, 30), (2, 40, 30), (3, 40, 30)], 3);
    let t = Instant::now();
    while woken() == 0 {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    // Nobody polls: the first result waits, charged, and the others aren't started.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(woken(), 1, "one result, no other decode");
    let held = images.worker.as_ref().unwrap().busy_cost();
    assert!(held >= 40 * 30 * 4, "the result is still counted: {held}");
    // Taking it lets the next go: all three arrive, one result at a time.
    wait(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 40, 30).is_some()
            && i.pixels(2, 40, 30).is_some()
            && i.pixels(3, 40, 30).is_some()
    });
    wait(&mut images, &doc, &mut layout, |i| {
        i.worker.as_ref().unwrap().busy_cost() == 0
    });
    assert_eq!(woken(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Without `curl` a blocked image isn't offered to load (the note would say to press `I`).
#[cfg(feature = "remote-images")]
#[test]
fn blocked_images_are_not_offered_without_curl() {
    let doc = Doc::parse("![r](https://x.y/r.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.attach(&doc, &mut layout);
    assert!(images.blocked(1));
    images.remote.no_curl = true;
    assert!(!images.blocked(1));
}

/// A progressive JPEG's decoder holds every coefficient of the image as well as its RGB frame,
/// and so does a baseline one whose first scan has some of its components only (it waits for the
/// scans of the rest): both count in what a decode costs.
#[test]
fn progressive_jpegs_count_their_coefficients() {
    let info = |b: &[u8]| header_info(&mut &b[..]).unwrap();
    assert_eq!(info(&fixture("gradient.jpg")).coefficients, 0, "baseline");
    // 48 × 24, every component sampled 1 × 1: three planes of 48 × 24 i16.
    let p = info(&fixture("progressive.jpg"));
    assert_eq!(p.coefficients, 3 * 48 * 24 * 2);
    assert_eq!(
        decode_cost(&p, 48, 24),
        48 * 24 * 3
            + 3 * 48 * 24 * 2
            + JPEG_META_COST
            + 3 * 48 * 8 * JPEG_ROW_BYTES
            + resample_cost(48, 24, 48, 24)
            + 48 * 24 * 4
    );
    // A 4000 × 3000 phone photo saved progressive at 4:2:0: luma padded to 16-pixel MCUs
    // (4000 × 3008), chroma at half size each way.
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xC2, 0x00, 0x11, 0x08];
    j.extend(3000u16.to_be_bytes());
    j.extend(4000u16.to_be_bytes());
    j.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    let p = info(&j);
    assert_eq!((p.w, p.h), (4000, 3000));
    assert_eq!(p.coefficients, (4000 * 3008 + 2 * 2000 * 1504) * 2);
    // The rows of its three components, 4000 pixels padded to 4000 and 16 rows of MCU.
    assert_eq!(p.mcu_rows, 3 * 4000 * 16 * JPEG_ROW_BYTES);
    // The same as baseline (SOF0), its first scan (SOS) with all three components: nothing but
    // the frame.
    j[3] = 0xC0;
    let scan = |ns: u8| {
        let mut sos = vec![0xFF, 0xDA, 0, 6 + 2 * u16::from(ns) as u8, ns];
        sos.extend((1..=ns).flat_map(|c| [c, 0]));
        sos.extend([0, 63, 0]);
        sos
    };
    let all = [j.clone(), scan(3)].concat();
    assert_eq!(info(&all).coefficients, 0);
    // A first scan with one of the three components: the decoder holds them all till the
    // others have come.
    let one = [j.clone(), scan(1)].concat();
    assert_eq!(info(&one).coefficients, p.coefficients);
    // The walk of the headers doesn't get to a scan: taken to be the larger.
    assert_eq!(info(&j).coefficients, p.coefficients);
    // Whatever comes between the frame and the scan is walked over, to the size the same.
    let mut between = j.clone();
    between.extend(segment(0xFE, b"a comment"));
    between.extend(scan(3));
    let b = info(&between);
    assert_eq!((b.w, b.h, b.coefficients), (4000, 3000, 0));
}

/// Looking at a fetched image's bytes to cost its decode doesn't mark them painted, so an
/// image that isn't decoded now doesn't keep them from being evicted.
#[test]
fn costing_remote_bytes_does_not_pin_them() {
    let url: Arc<str> = "https://x.y/a.png".into();
    let mut c = Cache::new(10);
    c.insert(Key::Remote(url.clone()), Data::Bytes(vec![0u8; 8].into()));
    c.frame += 1;
    assert!(c.peek_bytes(&url).is_some());
    assert_eq!(c.slots[0].used, 0);
    // Over the budget, it goes.
    c.insert(Key::Local(1), Data::Bytes(vec![0u8; 8].into()));
    assert!(c.peek_bytes(&url).is_none());
}

/// A fetched body that nothing painted yet is protected only once the cache has admitted it,
/// and it is admitted only if it fits: 24 images in view with 3 MiB bodies (72 MiB, for 40 × 30
/// pictures of 4.8 KB) are fetched while the window polls only every 30 ms (the fetchers fill the
/// time with bodies, the decoder takes one result at a time), and what is held, outside the
/// cache and in it, never goes over the budget, nothing waits forever and every image is shown.
#[cfg(feature = "remote-images")]
#[test]
fn bodies_of_images_in_view_wait_for_room_instead_of_piling_up() {
    use crate::remote::{Error, STEP};
    use crate::render::Pixels;
    const N: u32 = 24;
    let body = {
        let mut b = png_bytes(40, 30, |x, y| [x as u8, y as u8, 7, 255]);
        let tail = vec![0u8; 3 << 20];
        b.extend(tail);
        b
    };
    let src: String = (0..N)
        .map(|i| format!("![{i}](https://x.y/{i}.png) "))
        .collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let fetcher: crate::remote::Fetcher = Arc::new(move |_, grant| {
        for _ in 0..body.len().div_ceil(STEP) {
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
        }
        // The body is its own buffer, as allocated.
        Ok((Arc::new(body.clone()), 40, 30))
    });
    images.remote.set_fetcher(fetcher);
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let boxes: Vec<_> = (1..=N).map(|id| (id, 40, 30)).collect();
    let (mut waited, mut peak) = (0, 0);
    let t = Instant::now();
    loop {
        images.begin_frame();
        images.request(&boxes, boxes.len());
        // The window is busy: the fetchers fetch, the decoder decodes, nothing is taken.
        std::thread::sleep(Duration::from_millis(30));
        let held = images.cache.bytes + images.remote.inflight();
        peak = peak.max(held);
        assert!(
            held <= CACHE_BYTES,
            "{held} bytes held in all: cache {} inflight {} fetching {} reserve {} overhead {} paused {} exclusive {:?} waiting {}",
            images.cache.bytes,
            images.remote.inflight(),
            images.cache.fetching,
            images.cache.reserve,
            images.cache.overhead,
            images.remote.paused(),
            images.exclusive,
            images.waiting.len()
        );
        let before = images.stats();
        images.poll(&doc, &mut layout);
        waited = waited.max(before.waiting);
        let s = images.stats();
        if s.decoded == N as usize && s.waiting == 0 {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "stuck: {s:?}, {} held",
            images.cache.bytes
        );
    }
    assert!(waited > 0, "bodies had to wait, or this tests nothing");
    assert!(peak > 3 << 20, "bodies were held: {peak}");
    for id in 1..=N {
        assert!(images.pixels(id, 40, 30).is_some(), "image {id}");
    }
    // Decoded, the bodies are no use: what is cached is not what they cost.
    assert!(images.cache.bytes <= CACHE_BYTES);
}

/// With the cache full of what is in view (decoded images are never evicted, so a window of
/// them may take the whole budget), a body for an image in view is still let in, one at a time,
/// when nothing else could make room (no decode is running and no body is waiting for one): the
/// images load, one after another, and the cache never holds two bodies of them.
#[cfg(feature = "remote-images")]
#[test]
fn a_body_is_let_in_alone_when_nothing_can_make_room() {
    use crate::remote::{Error, STEP};
    use crate::render::Pixels;
    const N: u32 = 4;
    let body = {
        let mut b = png_bytes(40, 30, |x, y| [x as u8, y as u8, 9, 255]);
        b.extend(vec![0u8; 300 << 10]);
        b
    };
    let src: String = (0..N)
        .map(|i| format!("![{i}](https://x.y/{i}.png) "))
        .collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let fetcher: crate::remote::Fetcher = Arc::new(move |_, grant| {
        for _ in 0..body.len().div_ceil(STEP) {
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
        }
        Ok((Arc::new(body.clone()), 40, 30))
    });
    images.remote.set_fetcher(fetcher);
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    // Less than the fetchers are set aside: no body fits, ever.
    images.cache.cap = 1 << 20;
    let boxes: Vec<_> = (1..=N).map(|id| (id, 40, 30)).collect();
    let t = Instant::now();
    loop {
        images.begin_frame();
        images.request(&boxes, boxes.len());
        std::thread::sleep(Duration::from_millis(10));
        images.poll(&doc, &mut layout);
        let bodies = images
            .cache
            .slots
            .iter()
            .filter(|s| matches!(s.data, Data::Bytes(_)))
            .count();
        assert!(bodies <= 1, "{bodies} bodies held");
        if images.stats().decoded == N as usize {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "stuck");
    }
    for id in 1..=N {
        assert!(images.pixels(id, 40, 30).is_some(), "image {id}");
    }
}

/// A decoded image in view (17.3 MB: decoded images are never evicted) leaves no room for the
/// 8 MiB body of the remote image just below it, which is within the prefetch range but not in
/// view: the body is dropped, and it is not fetched again while nothing changes, however often
/// the window paints and polls. It is fetched when its image comes into view, and when the
/// image above is gone and there is room.
#[cfg(all(feature = "remote-images", feature = "images"))]
#[test]
fn a_body_dropped_for_lack_of_room_is_not_fetched_in_a_loop() {
    use crate::remote::{Error, STEP};
    use crate::render::Pixels;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = scratch("refetch");
    std::fs::write(
        dir.join("tall.png"),
        png_bytes(720, 6000, |_, y| [y as u8, 3, 4, 255]),
    )
    .unwrap();
    let body = {
        let mut b = png_bytes(40, 30, |x, y| [x as u8, y as u8, 5, 255]);
        b.extend(vec![0u8; 8 << 20]);
        b
    };
    let doc = Doc::parse("![t](tall.png)\n\n![r](https://x.y/r.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.set_policy(crate::settings::RemoteImages::Always);
    let fetches = Arc::new(AtomicUsize::new(0));
    let count = fetches.clone();
    let fetcher: crate::remote::Fetcher = Arc::new(move |_, grant| {
        count.fetch_add(1, Ordering::SeqCst);
        for _ in 0..body.len().div_ceil(STEP) {
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
        }
        Ok((Arc::new(body.clone()), 40, 30))
    });
    images.remote.set_fetcher(fetcher);
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let both = [(1, 720, 6000), (2, 40, 30)];
    let url: Arc<str> = "https://x.y/r.png".into();
    // The tall image decodes, and the body is fetched and dropped for lack of room (its cost is
    // kept): the state the window is in until something changes.
    drive(&mut images, &doc, &mut layout, &both, 1, |i| {
        i.pixels(1, 720, 6000).is_some() && i.deferred.contains_key(&url) && i.is_idle()
    });
    let n0 = fetches.load(Ordering::SeqCst);
    assert!(n0 <= 2, "the body was fetched {n0} times");
    // However often the window paints and polls from here, nothing is fetched (the pool is
    // idle when the last frame is over: a fetch asked for in it would show).
    for _ in 0..300 {
        images.begin_frame();
        images.request(&both, 1);
        images.poll(&doc, &mut layout);
        assert!(images.pixels(1, 720, 6000).is_some());
    }
    drive(&mut images, &doc, &mut layout, &both, 1, |i| i.is_idle());
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        n0,
        "fetched again with nothing changed"
    );
    assert!(images.pixels(2, 40, 30).is_none());
    assert_eq!(images.stats().waiting, 0);
    // Scrolled into view, it loads.
    drive(&mut images, &doc, &mut layout, &both, 2, |i| {
        i.pixels(2, 40, 30).is_some()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same, then the image above goes (the window moves on): there is room, so the body is
/// fetched again, once, and decoded.
#[cfg(all(feature = "remote-images", feature = "images"))]
#[test]
fn a_dropped_body_is_fetched_again_when_room_is_made() {
    use crate::remote::{Error, STEP};
    use crate::render::Pixels;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = scratch("refetch-room");
    std::fs::write(
        dir.join("tall.png"),
        png_bytes(720, 6000, |_, y| [y as u8, 3, 4, 255]),
    )
    .unwrap();
    let body = {
        let mut b = png_bytes(40, 30, |x, y| [x as u8, y as u8, 5, 255]);
        b.extend(vec![0u8; 8 << 20]);
        b
    };
    let doc = Doc::parse("![t](tall.png)\n\n![r](https://x.y/r.png)\n\n![s](https://x.y/s.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.set_policy(crate::settings::RemoteImages::Always);
    let fetches = Arc::new(AtomicUsize::new(0));
    let count = fetches.clone();
    let fetcher: crate::remote::Fetcher = Arc::new(move |_, grant| {
        count.fetch_add(1, Ordering::SeqCst);
        for _ in 0..body.len().div_ceil(STEP) {
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
        }
        Ok((Arc::new(body.clone()), 40, 30))
    });
    images.remote.set_fetcher(fetcher);
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let urls: [Arc<str>; 2] = ["https://x.y/r.png".into(), "https://x.y/s.png".into()];
    let with_tall = [(1, 720, 6000), (2, 40, 30), (3, 40, 30)];
    // Both bodies are fetched and dropped for lack of room, and nothing more is fetched from then
    // on however often the window paints and polls.
    drive(&mut images, &doc, &mut layout, &with_tall, 1, |i| {
        i.pixels(1, 720, 6000).is_some()
            && urls.iter().all(|u| i.deferred.contains_key(u))
            && i.is_idle()
    });
    let n0 = fetches.load(Ordering::SeqCst);
    assert!(n0 <= 4, "{n0} fetches of two bodies that don't fit");
    for _ in 0..300 {
        images.begin_frame();
        images.request(&with_tall, 1);
        images.poll(&doc, &mut layout);
    }
    drive(&mut images, &doc, &mut layout, &with_tall, 1, |i| {
        i.is_idle()
    });
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        n0,
        "fetched again with nothing changed"
    );
    assert!(images.pixels(2, 40, 30).is_none() && images.pixels(3, 40, 30).is_none());
    // Scrolled past the tall image: it is out of the boxes, and what was dropped is wanted.
    let past = [(2, 40, 30), (3, 40, 30)];
    drive(&mut images, &doc, &mut layout, &past, 1, |i| {
        i.pixels(2, 40, 30).is_some() && i.pixels(3, 40, 30).is_some()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// A body a job of the worker holds (an `Arc` clone: the decode reads it in place) stays
/// charged, and in the cache, until the job lets go of it, whatever the cache wants: it can't be
/// evicted, replaced or counted as room, or the memory it is still using would be free on paper
/// and taken by other bodies (a body of 11 MiB being decoded, the window moved on and 30 more
/// bodies of 4 MiB let in beside it: 43.7 MB live over the start, in a 33.5 MB budget).
#[test]
fn a_body_a_job_holds_keeps_its_charge() {
    let a: Arc<str> = "https://x.y/a.png".into();
    let mut c = Cache::new(100_000);
    c.insert(
        Key::Remote(a.clone()),
        Data::Bytes(Arc::new(vec![0u8; 40_000])),
    );
    // What `Images::request` gives a job.
    let job = c.bytes_of(&a).unwrap();
    c.frame += 1;
    let charged = c.bytes;
    assert!(charged >= 40_000);
    c.evict_for(100_000);
    assert_eq!(c.bytes, charged, "not evicted");
    assert!(!c.room_for(80_000, 0, 0), "not counted as room");
    assert!(!c.would_fit(80_000, 0, 0, 0), "nor as room it would make");
    // A body for the same URL doesn't replace it.
    c.insert(Key::Remote(a.clone()), Data::Bytes(Arc::new(vec![1u8; 10])));
    assert!(Arc::ptr_eq(c.peek_bytes(&a).unwrap(), &job));
    assert_eq!(c.bytes, charged);
    // The job is done with it: now it is the first to go.
    drop(job);
    assert!(c.would_fit(80_000, 0, 0, 0));
    assert!(c.room_for(80_000, 0, 0));
    assert!(c.peek_bytes(&a).is_none());
    assert_eq!(c.bytes, 0);
}

/// A job's hold ends before its result is sent: when the main thread has the result, the body's
/// slot can go, and nothing is allocated that isn't charged.
#[test]
fn the_worker_lets_go_of_a_body_before_it_sends_the_result() {
    let body = Arc::new(png_bytes(40, 30, |x, y| [x as u8, y as u8, 2, 255]));
    let (tx, rx) = channel();
    let worker = Worker::spawn(tx, Arc::new(|| {}));
    worker.set(vec![Job {
        doc_gen: 0,
        key: Key::Remote("https://x.y/a.png".into()),
        source: Source::Bytes(body.clone()),
        w: 40,
        h: 30,
        cost: 40 * 30 * 4,
    }]);
    let Ok(Done::Decoded {
        bitmap, worker: w, ..
    }) = rx.recv_timeout(Duration::from_secs(60))
    else {
        panic!("a result");
    };
    assert!(bitmap.is_some());
    assert_eq!(Arc::strong_count(&body), 1, "the job let go of the body");
    Worker::taken(&w);
}

/// A file that is bigger now than when its header was read isn't decoded with memory nothing
/// counted: the job is charged for the file as it was.
#[test]
fn a_file_that_changed_since_it_was_sized_is_refused() {
    let dir = scratch("grew");
    let path = dir.join("a.png");
    std::fs::write(&path, png_bytes(40, 30, |x, y| [x as u8, y as u8, 3, 255])).unwrap();
    let job = |cost| Job {
        doc_gen: 0,
        key: Key::Local(1),
        source: Source::File(path.clone()),
        w: 40,
        h: 30,
        cost,
    };
    let cost = file_info(&path).unwrap().cost(40, 30);
    assert!(decode_job(&job(cost)).is_some());
    std::fs::write(
        &path,
        png_bytes(400, 300, |x, y| [(x * y) as u8, y as u8, 3, 255]),
    )
    .unwrap();
    assert!(decode_job(&job(cost)).is_none(), "bigger than it was");
    // Charged for what it is now, it decodes.
    let cost = file_info(&path).unwrap().cost(40, 30);
    assert!(decode_job(&job(cost)).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a replaced worker is decoding stays counted ([`Images::set_wake`] starts another).
#[test]
fn a_replaced_worker_still_counts_what_it_holds() {
    let mut images = Images::default();
    images.worker = Some(Worker::spawn(images.tx.clone(), images.wake.clone()));
    let shared = images.worker.as_ref().unwrap().shared.clone();
    shared.0.lock().unwrap().busy = Some((Key::Local(1), 10, 10, 5000));
    images.set_wake(Arc::new(|| {}));
    assert!(images.worker.is_none());
    assert_eq!(images.retired_cost(), 5000);
    images.request(&[], 0);
    assert_eq!(images.cache.reserve, 5000);
    // Its result is taken: it holds nothing, and is forgotten.
    Worker::taken(&shared);
    assert_eq!(images.retired_cost(), 0);
    assert!(images.retired.is_empty());
}

/// A fetcher that fails for URLs with `bad` in them and brings a small PNG for the rest.
#[cfg(feature = "remote-images")]
fn fake_remote(images: &mut Images) {
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 1, 255]);
    images.remote.set_fetcher(Arc::new(move |url, _| {
        if url.contains("bad") {
            Err(crate::remote::Error::Failed)
        } else {
            Ok((Arc::new(png.clone()), 40, 30))
        }
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
}

/// Open `src` as `View::open` does (remote images off, then on at once) and frame until every
/// remote image of it has been fetched or has failed.
#[cfg(feature = "remote-images")]
fn open_and_settle(images: &mut Images, src: &str) -> (Doc, Layout) {
    let doc = Doc::parse(src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    images.forget_remote();
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    settle(images, &doc, &mut layout);
    (doc, layout)
}

#[cfg(feature = "remote-images")]
fn settle(images: &mut Images, doc: &Doc, layout: &mut Layout) {
    let n = doc.remote_image_count() as u32;
    let boxes: Vec<_> = (1..=n).map(|id| (id, 40, 30)).collect();
    let n = boxes.len();
    drive(images, doc, layout, &boxes, n, |images| {
        let done = images.srcs.iter().all(|s| match s {
            Src::Remote(url) => images.remote.size(url).is_some(),
            _ => true,
        });
        done && images.waiting.is_empty() && images.worker.as_ref().is_none_or(Worker::idle)
    });
}

/// What is kept per URL goes with the document that had it, so a session through many documents
/// doesn't collect a record for every URL it has seen; a repaint or a reload doesn't clear
/// the records of the document it keeps (the refetch loop would be back), and a body that comes
/// late for a document that is gone leaves none.
#[cfg(feature = "remote-images")]
#[test]
fn per_url_records_do_not_outlive_their_document() {
    let src = |tag: &str, bad: usize, good: bool| -> String {
        let mut s: String = (0..bad)
            .map(|i| format!("![x](https://x.y/{tag}-bad{i}.png) "))
            .collect();
        if good {
            s += &format!("![g](https://x.y/{tag}-good.png)");
        }
        s
    };
    let url = |s: &str| -> Arc<str> { s.into() };
    let mut images = Images::default();
    fake_remote(&mut images);
    let (doc_a, mut layout_a) = open_and_settle(&mut images, &src("a", 5, true));
    assert_eq!(images.remote.counts(), (1, 5));
    // A reload of the same document keeps what is known of it, and its records.
    images
        .deferred
        .insert(url("https://x.y/a-bad0.png"), (1, 1));
    images
        .deferred
        .insert(url("https://x.y/elsewhere.png"), (1, 1));
    images.attach(&doc_a, &mut layout_a);
    assert_eq!(images.remote.counts(), (1, 5));
    assert_eq!(images.deferred.len(), 1, "{:?}", images.deferred);
    assert!(images.deferred.contains_key("https://x.y/a-bad0.png"));
    // Repainting changes nothing.
    settle(&mut images, &doc_a, &mut layout_a);
    assert_eq!(images.deferred.len(), 1);
    // Another document: the failures of the first are forgotten, and so is its record. What is
    // cached stays known (its slot is charged for it).
    let (doc_b, _) = open_and_settle(&mut images, &src("b", 5, true));
    assert_eq!(images.remote.counts(), (2, 5), "a's good, b's");
    assert!(images.deferred.is_empty());
    // A body that comes late for a URL the document hasn't leaves no record.
    let png = Arc::new(png_bytes(40, 30, |_, _| [0, 0, 0, 255]));
    images.drop_fetched(url("https://x.y/late.png"), Ok((png.clone(), 40, 30)));
    assert!(images.deferred.is_empty());
    images.drop_fetched(url("https://x.y/b-good.png"), Ok((png, 40, 30)));
    assert_eq!(images.deferred.len(), 1, "one the document has");
    drop(doc_b);
    // Many documents of failing URLs don't pile up.
    for k in 0..30 {
        open_and_settle(&mut images, &src(&format!("k{k}"), 20, false));
        let (fetched, failed) = images.remote.counts();
        assert!(
            fetched <= 2 && failed <= 21,
            "after {k}: {fetched} {failed}"
        );
    }
}

/// What [`Spans::new`] makes never has more weights than [`spans_weights`] says, so its buffer is
/// the one it was made with (the capacity [`spans_cost`] charges): every size to 40, and the
/// extremes a decode may meet.
#[test]
fn spans_stay_within_their_bound() {
    let check = |src: u32, dst: u32| {
        let s = Spans::new(src, dst);
        assert!(
            s.weights.len() <= spans_weights(src, dst),
            "{src} to {dst}: {} weights",
            s.weights.len()
        );
        assert_eq!(
            s.weights.capacity(),
            spans_weights(src, dst),
            "{src} to {dst}"
        );
        assert_eq!(s.spans.capacity(), dst as usize + 1, "{src} to {dst}");
        assert_eq!(s.len(), dst as usize);
        // Every target pixel has at least one source pixel under it, weights that add up to
        // one, and the first source pixel is in the image.
        for t in 0..s.len() {
            let (first, w) = s.get(t);
            assert!(
                !w.is_empty() && first < src as usize,
                "{src} to {dst} at {t}"
            );
            let sum: f32 = w.iter().sum();
            assert!((sum - 1.0).abs() < 1e-3, "{src} to {dst} at {t}: {sum}");
        }
    };
    for src in 1..=40 {
        for dst in 1..=40 {
            check(src, dst);
        }
    }
    for (src, dst) in [
        (MAX_SOURCE_SIDE, 1),
        (MAX_SOURCE_SIDE, MAX_SIDE),
        (1, MAX_SIDE),
        (3, MAX_SIDE),
        (MAX_SOURCE_SIDE, 7),
        (MAX_SOURCE_SIDE, 16_383),
        (16_383, MAX_SOURCE_SIDE.min(MAX_SIDE)),
        (999, 1000),
        (1001, 1000),
    ] {
        check(src, dst);
    }
}

/// The slot list gives back the room slots left when they go: a new document dropping thousands of
/// local images, or the cache evicting them, leaves a list the size of what is kept, not of the
/// most it ever held (100,000 one-pixel images left 7 MB, beside a budget that was full of a
/// bitmap).
#[test]
fn the_slot_list_gives_back_its_room() {
    let tiny = |n: u32| {
        Data::Pixels(Bitmap {
            width: 1,
            height: 1,
            data: vec![n],
        })
    };
    let mut c = Cache::new(usize::MAX);
    for n in 0..5000 {
        c.insert(Key::Local(n), tiny(n));
    }
    assert!(c.slots.capacity() >= 5000);
    // A new document: only a few stay.
    c.retain(|s| matches!(s.key, Key::Local(n) if n < 3));
    assert_eq!(c.slots.len(), 3);
    assert!(c.slots.capacity() <= 2 * 3 + 16, "{}", c.slots.capacity());
    assert_eq!(c.bytes, c.slots.iter().map(Slot::cost).sum::<usize>());
    // Eviction, one slot at a time, doesn't leave it either.
    let mut c = Cache::new(usize::MAX);
    for n in 0..5000 {
        c.insert(Key::Local(n), tiny(n));
    }
    c.frame = 1;
    c.cap = 100 * (4 + SLOT_OVERHEAD);
    c.evict();
    assert!(c.slots.len() <= 100, "{}", c.slots.len());
    assert!(
        c.slots.capacity() <= 2 * c.slots.len() + 16,
        "{} slots in room for {}",
        c.slots.len(),
        c.slots.capacity()
    );
    // And so does a map pruned: what the sizes table and the deferred table keep is theirs.
    #[cfg(feature = "remote-images")]
    {
        let mut m: std::collections::HashMap<u32, u32> = (0..5000).map(|n| (n, n)).collect();
        m.retain(|&n, _| n < 3);
        shrink_map(&mut m);
        assert!(m.capacity() <= 2 * 3 + 16, "{}", m.capacity());
    }
}

/// A body that didn't decode is dropped (a decode that still holds it makes it the first to go),
/// so the image is fetched again when a document with it is opened again.
#[test]
fn a_body_that_did_not_decode_is_dropped() {
    let url: Arc<str> = "https://x.y/a.png".into();
    let mut c = Cache::new(CACHE_BYTES);
    c.insert(Key::Remote(url.clone()), Data::Bytes(vec![0u8; 100].into()));
    c.insert(
        Key::Local(1),
        Data::Pixels(Bitmap {
            width: 1,
            height: 1,
            data: vec![0],
        }),
    );
    c.frame = 3;
    let held = c.bytes_of(&url).unwrap();
    c.discard_bytes(&url);
    assert!(c.peek_bytes(&url).is_some(), "a decode holds it");
    assert_eq!(
        c.slots
            .iter()
            .find(|s| s.key == Key::Remote(url.clone()))
            .unwrap()
            .used,
        0
    );
    drop(held);
    c.discard_bytes(&url);
    assert!(c.peek_bytes(&url).is_none());
    assert_eq!(c.slots.len(), 1, "the pixels of another image stay");
    assert_eq!(c.bytes, c.slots.iter().map(Slot::cost).sum::<usize>());
}

/// A remote image whose body decodes to nothing (its header is sound, its data is cut short)
/// shows its alt text while the document is open, a repaint and a reload included (no
/// fetching in a loop), and is fetched again when the document is opened again.
#[cfg(feature = "remote-images")]
#[test]
fn a_remote_decode_failure_is_retried_when_the_document_is_opened_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 1, 255]);
    let cut = png[..png.len() - png.len() / 3].to_vec();
    let fetches = Arc::new(AtomicUsize::new(0));
    let count = fetches.clone();
    let mut images = Images::default();
    images.remote.set_fetcher(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok((Arc::new(cut.clone()), 40, 30))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    let url: Arc<str> = "https://x.y/cut.png".into();
    let failed = |images: &Images| images.remote.size(&url) == Some(None);
    let frames =
        |images: &mut Images, doc: &Doc, layout: &mut Layout, until: &dyn Fn(&Images) -> bool| {
            drive(images, doc, layout, &[(1, 40, 30)], 1, |i| until(i));
        };
    let src = "![x](https://x.y/cut.png)";
    let doc = Doc::parse(src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    images.forget_remote();
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    frames(&mut images, &doc, &mut layout, &failed);
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    assert!(
        images.cache.peek_bytes(&url).is_none(),
        "the body is of no use"
    );
    // A repaint and a reload of the document: the failure is kept, nothing is fetched.
    for _ in 0..20 {
        images.begin_frame();
        images.request(&[(1, 40, 30)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    images.attach(&doc, &mut layout);
    for _ in 0..20 {
        images.begin_frame();
        images.request(&[(1, 40, 30)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert!(failed(&images));
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        1,
        "a repaint and a reload fetch nothing"
    );
    // Another document, and back to the first: the image is tried again.
    let other = Doc::parse("nothing here");
    let mut other_layout = Layout::new(&other, crate::layout::Params::default());
    images.forget_remote();
    images.attach(&other, &mut other_layout);
    assert_eq!(images.remote.size(&url), None, "the failure is forgotten");
    images.forget_remote();
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    frames(&mut images, &doc, &mut layout, &|i| {
        fetches.load(Ordering::SeqCst) >= 2 && i.remote.size(&url) == Some(None)
    });
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

// ---- Decodes that don't fit beside the fetchers (review round 8) ----------------

/// A valid baseline JPEG of `w` × `h` pixels as far as its headers go (`wide.jpg`'s frame header
/// with another size: the decoder makes what it can of the scan data, and pads).
#[cfg(feature = "remote-images")]
fn big_jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut jpeg = fixture("wide.jpg");
    let sof = jpeg.windows(2).position(|p| p == [0xFF, 0xC0]).unwrap();
    jpeg[sof + 5..sof + 7].copy_from_slice(&h.to_be_bytes());
    jpeg[sof + 7..sof + 9].copy_from_slice(&w.to_be_bytes());
    jpeg
}

/// How long the event loop that [`drive`] stands in for may sit with nothing to do while what it
/// waits for is not there: a wake that was never sent (a transition nothing told the window of)
/// stays stuck for good, and this is how a test finds out, long before the deadline.
#[cfg(feature = "remote-images")]
const QUIET: Duration = Duration::from_secs(5);

/// What the window does with images, and no more (`src/main.rs`): the viewport is
/// new (`boxes`, the first `on_screen` in view), so a frame is painted (`begin_frame`, the
/// pixels in view painted, `request`); after that the threads wake it, and it polls for a wake
/// (once for however many came), and paints a frame only if the poll says a repaint is needed.
/// Nothing else asks for a frame: a result that is only reached by a request nothing woke the
/// window for stays unreached, and `done` (looked at every millisecond, wake or not) never holds.
/// Fails if it doesn't within [`QUIET`] of the last frame or wake, or in a minute.
#[cfg(feature = "remote-images")]
fn drive(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32)],
    on_screen: usize,
    done: impl FnMut(&mut Images) -> bool,
) {
    drive_from(true, images, doc, layout, boxes, on_screen, done);
}

/// [`drive`] with no frame for the viewport to start with (`first` false): it hasn't changed,
/// and only what the threads do makes anything happen.
#[cfg(feature = "remote-images")]
fn drive_from(
    first: bool,
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32)],
    on_screen: usize,
    mut done: impl FnMut(&mut Images) -> bool,
) {
    use crate::render::Pixels;
    let t = Instant::now();
    let mut seen = images.wakes();
    // What came while nothing was driving it was handled by the event loop meanwhile.
    let polled = images.poll(doc, layout);
    let (mut frame, mut last) = (first || polled, Instant::now());
    while !done(images) {
        assert!(
            t.elapsed() < Duration::from_secs(60) && last.elapsed() < QUIET,
            "stuck (a wake lost?): {:?} {}",
            images.stats(),
            images.memory_report()
        );
        if frame {
            frame = false;
            last = Instant::now();
            images.begin_frame();
            for &(id, w, h) in boxes.iter().take(on_screen) {
                let _ = images.pixels(id, w, h);
            }
            images.request(boxes, on_screen);
        }
        if images.wakes() == seen {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        seen = images.wakes();
        last = Instant::now();
        frame = images.poll(doc, layout);
    }
}

/// Fetchers wait at a gate until it is opened.
#[cfg(feature = "remote-images")]
struct Gate(Mutex<bool>, Condvar);

#[cfg(feature = "remote-images")]
impl Gate {
    fn new() -> Arc<Gate> {
        Arc::new(Gate(Mutex::new(false), Condvar::new()))
    }

    fn wait(&self) {
        let mut open = self.0.lock().unwrap();
        while !*open {
            open = self.1.wait(open).unwrap();
        }
    }

    fn open(&self) {
        *self.0.lock().unwrap() = true;
        self.1.notify_all();
    }
}

/// A fetcher that counts the URLs it starts, takes credit for 3.75 MiB (so three fit in what
/// the pool may hold, and the fourth waits for credit), holds it at `gate`, and brings `body`.
#[cfg(feature = "remote-images")]
fn holding_fetcher(
    gate: &Arc<Gate>,
    starts: &Arc<std::sync::atomic::AtomicUsize>,
    body: Vec<u8>,
) -> crate::remote::Fetcher {
    holding_fetcher_counting(gate, starts, &Arc::default(), body)
}

/// [`holding_fetcher`] that also counts the transfers that hold their credit at the gate.
#[cfg(feature = "remote-images")]
fn holding_fetcher_counting(
    gate: &Arc<Gate>,
    starts: &Arc<std::sync::atomic::AtomicUsize>,
    holding: &Arc<std::sync::atomic::AtomicUsize>,
    body: Vec<u8>,
) -> crate::remote::Fetcher {
    use crate::remote::{Error, STEP};
    let (gate, starts, holding) = (gate.clone(), starts.clone(), holding.clone());
    // One transfer takes its credit at a time, so that it is the first three that hold 3.75 MiB
    // each, and the fourth that waits for credit, whichever thread runs first.
    let turn = Arc::new(Mutex::new(()));
    Arc::new(move |_, grant| {
        starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let taking = turn.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..15 {
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
        }
        drop(taking);
        holding.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        gate.wait();
        Ok((Arc::new(body.clone()), 40, 30))
    })
}

/// A local decode that doesn't fit beside what the fetchers may hold (a 4000 × 2000 JPEG is held
/// for 24.5 MB, and they are given 14 MB of the 33.5) is not started while they hold anything:
/// they are held off (nothing starts, the one waiting for credit gives up), it is handed to the
/// worker when they have nothing, and they go on when its result is taken, the ones that were
/// queued included.
#[cfg(feature = "remote-images")]
#[test]
fn a_local_decode_that_does_not_fit_waits_for_the_fetchers_and_holds_them_off() {
    use crate::render::Pixels;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = scratch("heavy-local");
    std::fs::write(dir.join("big.jpg"), big_jpeg(4000, 2000)).unwrap();
    let mut src = String::from("![j](big.jpg) ");
    src += &(0..6)
        .map(|i| format!("![r](https://x.y/{i}.png) "))
        .collect::<String>();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.set_policy(crate::settings::RemoteImages::Always);
    let (gate, starts) = (Gate::new(), Arc::new(AtomicUsize::new(0)));
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 2, 255]);
    images
        .remote
        .set_fetcher(holding_fetcher(&gate, &starts, png));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    // Three transfers hold 3.75 MiB each at the gate, the fourth waits for credit.
    drive(&mut images, &doc, &mut layout, &[], 0, |i| {
        starts.load(Ordering::SeqCst) == 4 && i.remote.inflight() >= 3 * 15 * (256 << 10)
    });
    // The image comes into view: the decode is held back, and the fetchers are held off.
    let boxes = [(1, 300, 150)];
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.remote.paused() && i.remote.transfers().0 == 3
    });
    assert!(images.worker.is_none(), "nothing was handed to the worker");
    assert_eq!(images.stats().decodes, 0);
    assert_eq!(
        starts.load(Ordering::SeqCst),
        4,
        "the one that gave up is not restarted"
    );
    // The transfers end: the decode goes, and nothing starts until its result is taken. (No frame
    // for the viewport: the window is woken for each result, and that is all that asks for one.)
    gate.open();
    drive_from(false, &mut images, &doc, &mut layout, &boxes, 1, |i| {
        // Only a poll takes the result, and lets the fetchers go on: until it has, none started.
        let decoded = i.stats().decodes > 0;
        assert!(
            decoded || starts.load(Ordering::SeqCst) == 4,
            "a fetch started during the decode"
        );
        decoded
    });
    assert!(images.pixels(1, 300, 150).is_some());
    // Then they go on: the one that gave up and the two that were queued.
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().fetched == 6 && i.is_idle()
    });
    assert!(
        starts.load(Ordering::SeqCst) >= 7,
        "{} fetches started",
        starts.load(Ordering::SeqCst)
    );
    assert!(!images.remote.paused() && images.exclusive.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A transfer that gives up because the fetchers were held off sends no result, only a wake: a
/// local decode held back for the fetchers is started when the last of them gives up, though
/// nothing asks for a frame (review round 9). One transfer holds its credit at a gate, the
/// image comes into view (the decode is held back, the fetchers held off), the gate opens and the
/// transfer asks for more credit, which it is refused. The window gets one wake and polls: with
/// nothing else asking for a frame the decode is started only if that poll says a repaint is
/// needed (it was never started: zero transfers, zero decodes, a poll that said no a hundred
/// times, and the fetchers held off for good).
#[cfg(feature = "remote-images")]
#[test]
fn a_transfer_that_gives_up_for_the_pause_starts_the_decode_held_back_for_it() {
    use crate::remote::{Error, STEP};
    use crate::render::Pixels;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = scratch("held-yield");
    std::fs::write(dir.join("big.jpg"), big_jpeg(4000, 2000)).unwrap();
    let doc = Doc::parse("![j](big.jpg) ![r](https://x.y/a.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.set_policy(crate::settings::RemoteImages::Always);
    let (gate, starts) = (Gate::new(), Arc::new(AtomicUsize::new(0)));
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 3, 255]);
    {
        let (gate, starts) = (gate.clone(), starts.clone());
        images.remote.set_fetcher(Arc::new(move |_, grant| {
            starts.fetch_add(1, Ordering::SeqCst);
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
            gate.wait();
            // One more byte than it was given credit for.
            if !grant(STEP) {
                return Err(Error::Yielded);
            }
            Ok((Arc::new(png.clone()), 40, 30))
        }));
    }
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    // The transfer has its credit and waits at the gate.
    drive(&mut images, &doc, &mut layout, &[], 0, |i| {
        starts.load(Ordering::SeqCst) == 1 && i.remote.inflight() == STEP
    });
    // The image comes into view: the decode is held back and the fetchers are held off.
    let boxes = [(1, 300, 150)];
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.remote.paused()
    });
    assert!(images.worker.is_none(), "nothing was handed to the worker");
    assert_eq!(images.remote.transfers(), (1, 0));
    // The gate opens: the transfer asks for credit and gives up. The window is woken (once) and
    // nothing else asks for a frame.
    gate.open();
    drive_from(false, &mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().decodes > 0
    });
    assert!(images.pixels(1, 300, 150).is_some());
    // The fetchers go on when its result is taken: the transfer that gave up is fetched again.
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().fetched == 1 && i.is_idle()
    });
    assert!(!images.remote.paused() && images.exclusive.is_none());
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The same for a fetched body: the 4000 × 2000 JPEG of a remote image in view arrives while
/// three transfers hold their credit. It waits (not in the cache, nothing decoding) while they
/// do, with them held off, is let in alone when they are over, and they go on after its decode.
#[cfg(feature = "remote-images")]
#[test]
fn a_body_that_does_not_fit_waits_for_the_fetchers_and_is_let_in_alone() {
    use crate::render::Pixels;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let jpeg = big_jpeg(4000, 2000);
    let mut src = String::from("![j](https://x.y/big.jpg) ");
    src += &(0..6)
        .map(|i| format!("![r](https://x.y/slow{i}.png) "))
        .collect::<String>();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let (gate, starts) = (Gate::new(), Arc::new(AtomicUsize::new(0)));
    let held = Arc::new(AtomicUsize::new(0));
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 3, 255]);
    let holding = holding_fetcher_counting(&gate, &starts, &held, png);
    {
        let (starts, held) = (starts.clone(), held.clone());
        images
            .remote
            .set_fetcher(Arc::new(move |url: &str, grant: crate::remote::Grant| {
                if url.ends_with("big.jpg") {
                    starts.fetch_add(1, Ordering::SeqCst);
                    assert!(grant(256 << 10));
                    // The body arrives when three transfers hold their credit.
                    while held.load(Ordering::SeqCst) < 3 {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok((Arc::new(jpeg.clone()), 4000, 2000))
                } else {
                    holding(url, grant)
                }
            }));
    }
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let boxes = [(1, 300, 150)];
    // The body arrives, and waits; the transfers still hold their credit.
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().waiting == 1 && i.remote.paused() && i.remote.transfers().0 == 3
    });
    for _ in 0..20 {
        images.begin_frame();
        images.request(&boxes, 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    let url: Arc<str> = "https://x.y/big.jpg".into();
    assert_eq!(images.stats().waiting, 1);
    assert!(
        images.cache.peek_bytes(&url).is_none(),
        "not let in beside them"
    );
    assert_eq!(images.stats().decodes, 0);
    // (The fetcher the JPEG left free may have started a fifth, which waited for credit, and gave
    // up: nothing starts from here.)
    let before = starts.load(Ordering::SeqCst);
    assert!((4..=5).contains(&before), "{before} fetches started");
    // They end; it is let in, decoded, and nothing starts until its result is taken. (No frame
    // for the viewport: the window is woken for each result, and that is all that asks for one.)
    gate.open();
    drive_from(false, &mut images, &doc, &mut layout, &boxes, 1, |i| {
        let decoded = i.stats().decodes > 0;
        assert!(
            decoded || starts.load(Ordering::SeqCst) == before,
            "a fetch started before the decode was over"
        );
        decoded
    });
    assert!(images.pixels(1, 300, 150).is_some());
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().fetched == 7 && i.is_idle()
    });
    // (The one that gave up started again, and so may others that wanted credit at once.)
    assert!(
        starts.load(Ordering::SeqCst) >= 8,
        "{} fetches started",
        starts.load(Ordering::SeqCst)
    );
    assert!(!images.remote.paused() && images.exclusive.is_none());
}

/// Two bodies that can only be decoded alone, both in view, both wait, with the fetchers held
/// off, and are let in one after the other: no deadlock, whatever else waits. With bodies of
/// 3 MiB the second doesn't fit beside the first's decode, so it is let go and fetched again.
#[cfg(feature = "remote-images")]
#[test]
fn two_bodies_that_can_only_be_decoded_alone_both_get_their_turn() {
    use crate::render::Pixels;
    use std::sync::atomic::{AtomicUsize, Ordering};
    for padding in [0usize, 3 << 20] {
        let mut jpeg = big_jpeg(4000, 2000);
        jpeg.resize(jpeg.len() + padding, 0);
        let doc = Doc::parse("![a](https://x.y/a.jpg) ![b](https://x.y/b.jpg)");
        let mut layout = Layout::new(&doc, crate::layout::Params::default());
        let mut images = Images::default();
        images.set_policy(crate::settings::RemoteImages::Always);
        let starts = Arc::new(AtomicUsize::new(0));
        let count = starts.clone();
        images
            .remote
            .set_fetcher(Arc::new(move |_: &str, grant: crate::remote::Grant| {
                count.fetch_add(1, Ordering::SeqCst);
                for _ in 0..jpeg.len().div_ceil(256 << 10) {
                    if !grant(256 << 10) {
                        return Err(crate::remote::Error::Yielded);
                    }
                }
                Ok((Arc::new(jpeg.clone()), 4000, 2000))
            }));
        images.attach(&doc, &mut layout);
        images.set_remote(true, &doc, &mut layout);
        let boxes = [(1, 300, 150), (2, 300, 150)];
        drive(&mut images, &doc, &mut layout, &boxes, 2, |i| {
            i.stats().decoded == 2
        });
        assert!(images.pixels(1, 300, 150).is_some() && images.pixels(2, 300, 150).is_some());
        let n = starts.load(Ordering::SeqCst);
        assert_eq!(n, if padding == 0 { 2 } else { 3 }, "padding {padding}");
        drive(&mut images, &doc, &mut layout, &boxes, 2, |i| i.is_idle());
        assert!(!images.remote.paused() && images.exclusive.is_none());
    }
}

/// What was held back is let go of when the image goes out of view: the fetchers go on.
#[cfg(feature = "remote-images")]
#[test]
fn the_fetchers_go_on_when_the_decode_held_back_is_no_longer_wanted() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = scratch("heavy-away");
    std::fs::write(dir.join("big.jpg"), big_jpeg(4000, 2000)).unwrap();
    let doc = Doc::parse("![j](big.jpg) ![a](https://x.y/a.png) ![b](https://x.y/b.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.set_policy(crate::settings::RemoteImages::Always);
    let (gate, starts) = (Gate::new(), Arc::new(AtomicUsize::new(0)));
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 4, 255]);
    images
        .remote
        .set_fetcher(holding_fetcher(&gate, &starts, png));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    drive(&mut images, &doc, &mut layout, &[], 0, |_| {
        starts.load(Ordering::SeqCst) == 2
    });
    let boxes = [(1, 300, 150)];
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.remote.paused()
    });
    // Scrolled away: nothing wants it.
    images.begin_frame();
    images.request(&[], 0);
    assert!(!images.remote.paused(), "the fetchers go on");
    gate.open();
    drive(&mut images, &doc, &mut layout, &[], 0, |i| {
        i.stats().fetched == 2 && i.is_idle()
    });
    assert_eq!(images.stats().decodes, 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The body case of the one above: the last transfer to end is one that gives up for the pause,
/// with a body in view waiting for the fetchers to hold nothing. The window is woken for it and
/// nothing else asks for a frame: the poll lets the body in ([`Images::admit_alone`]) and says
/// a repaint is needed, which asks for its decode.
#[cfg(feature = "remote-images")]
#[test]
fn a_body_that_waits_is_let_in_when_the_last_transfer_gives_up() {
    use crate::remote::{Error, STEP};
    use crate::render::Pixels;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let jpeg = big_jpeg(4000, 2000);
    let doc = Doc::parse("![j](https://x.y/big.jpg) ![r](https://x.y/slow.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let (gate, held) = (Gate::new(), Arc::new(AtomicUsize::new(0)));
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 4, 255]);
    {
        let (gate, held) = (gate.clone(), held.clone());
        images
            .remote
            .set_fetcher(Arc::new(move |url: &str, grant: crate::remote::Grant| {
                if !grant(STEP) {
                    return Err(Error::Yielded);
                }
                if url.ends_with("big.jpg") {
                    // The body arrives while the other transfer holds its credit.
                    while held.load(Ordering::SeqCst) < 1 {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    return Ok((Arc::new(jpeg.clone()), 4000, 2000));
                }
                held.fetch_add(1, Ordering::SeqCst);
                gate.wait();
                if !grant(STEP) {
                    return Err(Error::Yielded);
                }
                Ok((Arc::new(png.clone()), 40, 30))
            }));
    }
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let boxes = [(1, 300, 150)];
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().waiting == 1 && i.remote.paused() && i.remote.transfers() == (1, 1)
    });
    assert_eq!(images.stats().decodes, 0);
    gate.open();
    drive_from(false, &mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().decodes > 0
    });
    assert!(images.pixels(1, 300, 150).is_some());
    drive(&mut images, &doc, &mut layout, &boxes, 1, |i| {
        i.stats().fetched == 2 && i.is_idle()
    });
}

/// An image that failed is not tried again while the document is open, whatever body of it is
/// still held (found by the random mix, the first state it left stuck: a thread decoding for
/// ever, 100% of a core): the image is in the document twice, in two sizes, and fails to decode.
/// Its body is not let go while a job holds it, and each failing job found the other's queued,
/// so the body stayed, and the next request made a job of it again, one size and then the
/// other, for good.
#[cfg(feature = "remote-images")]
#[test]
fn an_image_that_failed_is_not_decoded_again_for_the_body_still_held() {
    let png = png_bytes(40, 30, |x, y| [x as u8, y as u8, 1, 255]);
    let doc = Doc::parse("![a](https://x.y/cut.png) ![b](https://x.y/cut.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let url: Arc<str> = "https://x.y/cut.png".into();
    // Its body is held (a job's, pinned) and it is known to have failed.
    images.cache.insert(
        Key::Remote(url.clone()),
        Data::Bytes(Arc::new(png[..png.len() / 2].to_vec())),
    );
    images.remote.failed(&url);
    let boxes = [(1, 40, 30), (2, 20, 15)];
    images.begin_frame();
    images.request(&boxes, 2);
    assert!(images.worker.is_none(), "a job was made of it");
    assert!(images.is_idle());
}

/// A body let in alone whose image is gone before its decode is asked for (another document
/// opens) doesn't keep the fetchers held off.
#[cfg(feature = "remote-images")]
#[test]
fn a_body_let_in_alone_does_not_hold_the_fetchers_if_nothing_asks_for_it() {
    let jpeg = big_jpeg(4000, 2000);
    let doc = Doc::parse("![j](https://x.y/big.jpg)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    images
        .remote
        .set_fetcher(Arc::new(move |_: &str, grant: crate::remote::Grant| {
            assert!(grant(256 << 10));
            Ok((Arc::new(jpeg.clone()), 4000, 2000))
        }));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let boxes = [(1, 300, 150)];
    // Fetched and waiting, nothing else asked for (no `request` after the first).
    images.begin_frame();
    images.request(&boxes, 1);
    let t = Instant::now();
    while images.exclusive.is_none() {
        assert!(t.elapsed() < Duration::from_secs(60), "stuck");
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert!(images.awaiting && images.remote.paused());
    // Another document: the image is not wanted.
    let other = Doc::parse("nothing here");
    let mut other_layout = Layout::new(&other, crate::layout::Params::default());
    images.forget_remote();
    images.attach(&other, &mut other_layout);
    images.begin_frame();
    images.request(&[], 0);
    assert!(images.exclusive.is_none() && !images.remote.paused());
}

/// A removed slot's URL has no record left unless the document has it: its size, its string
/// and the room its table has.
#[cfg(feature = "remote-images")]
#[test]
fn evicting_the_last_slot_of_a_url_forgets_what_is_kept_for_it() {
    let doc = Doc::parse("![k](https://x.y/kept.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.attach(&doc, &mut layout);
    let url = |n: &str| -> Arc<str> { format!("https://x.y/{n}.png").into() };
    for name in ["kept", "a", "b"] {
        let u = url(name);
        let body: Bytes = Arc::new(vec![0u8; 64]);
        images
            .remote
            .take_fetched(&u, Ok((body.clone(), 4, 4)))
            .unwrap();
        images
            .cache
            .insert(Key::Remote(u.clone()), Data::Bytes(body));
    }
    // Pixels at two sizes for `b`: one is evicted, the URL still has a slot.
    for w in [4, 8] {
        images.cache.insert(
            Key::Remote(url("b")),
            Data::Pixels(Bitmap {
                width: w,
                height: 1,
                data: vec![0; w as usize],
            }),
        );
    }
    assert_eq!(images.remote.counts(), (3, 0));
    images.cache.frame += 1;
    let a = images
        .cache
        .slots
        .iter()
        .position(|s| s.key == Key::Remote(url("a")))
        .unwrap();
    images.cache.remove(a);
    let pixels = images
        .cache
        .slots
        .iter()
        .position(|s| s.pixels().is_some_and(|b| b.width == 8))
        .unwrap();
    images.cache.remove(pixels);
    images.forget_evicted();
    assert_eq!(
        images.remote.size(&url("a")),
        None,
        "no slot left, not the document's"
    );
    assert_eq!(
        images.remote.size(&url("b")),
        Some(Some((4, 4))),
        "a slot is left"
    );
    assert_eq!(images.remote.size(&url("kept")), Some(Some((4, 4))));
    // The document's own survives its last slot.
    let kept = images
        .cache
        .slots
        .iter()
        .position(|s| s.key == Key::Remote(url("kept")))
        .unwrap();
    images.cache.remove(kept);
    images.forget_evicted();
    assert_eq!(
        images.remote.size(&url("kept")),
        Some(Some((4, 4))),
        "the document's"
    );
    assert!(images.cache.dropped.is_empty());
}

/// Everything long-lived of the image code is in what the budget holds back, by its capacity,
/// and goes out of it with the room: a document with thousands of images, a fetch queue of as
/// many URLs, and then another document.
#[cfg(feature = "remote-images")]
#[test]
fn the_bookkeeping_is_charged_by_capacity_and_given_back() {
    let n = 5_000;
    let src: String = (0..n)
        .map(|i| format!("![p](https://x.y/{i}.png) "))
        .collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let gate = Gate::new();
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let png = png_bytes(4, 4, |_, _| [0, 0, 0, 255]);
    images
        .remote
        .set_fetcher(holding_fetcher(&gate, &starts, png));
    let empty = images.bookkeeping_bytes();
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    images.request(&[], 0);
    // The list of sources, their strings, the set of URLs, what a request builds for each of
    // them, and the queue (4 are being fetched).
    let busy = images.bookkeeping_bytes();
    assert!(
        busy >= empty + n * (size_of::<Src>() + 16 + 16 + 24) + n * 16,
        "{busy} bytes for {n} images"
    );
    assert_eq!(images.cache.overhead, busy, "what the cache holds back");
    // Stopping gives the queue's buffer back, not just its elements.
    let queue = images.remote.bookkeeping_bytes();
    images.set_remote(false, &doc, &mut layout);
    assert!(
        images.remote.bookkeeping_bytes() + (n - 8) * 16 <= queue,
        "{} of {queue} left after the queue was stopped",
        images.remote.bookkeeping_bytes()
    );
    gate.open();
    // Another document: what the first had is gone, and what is charged is next to nothing.
    let other = Doc::parse("![x](a.png)");
    let mut other_layout = Layout::new(&other, crate::layout::Params::default());
    images.forget_remote();
    images.attach(&other, &mut other_layout);
    drive(&mut images, &other, &mut other_layout, &[], 0, |i| {
        i.is_idle()
    });
    images.request(&[], 0);
    let after = images.bookkeeping_bytes();
    assert!(after < busy / 10, "{after} bytes of {busy} left");
    assert_eq!(images.cache.overhead, after);
}

/// A decode that is running when the fetchers would start (remote images turned on, another
/// document opened) and doesn't fit beside what they may hold holds them off, and they start
/// when its result is taken: the decode was handed over when nothing was fetched, and four
/// transfers took their 12 MiB beside it (a 19.4 MB decode, 34.3 MB live in a 33.5 MB budget).
#[cfg(feature = "remote-images")]
#[test]
fn a_decode_running_when_the_fetchers_would_start_holds_them_off() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let doc = Doc::parse("![a](https://x.y/a.png) ![b](https://x.y/b.png) ![c](https://x.y/c.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let starts = Arc::new(AtomicUsize::new(0));
    let count = starts.clone();
    images
        .remote
        .set_fetcher(Arc::new(move |_: &str, _: crate::remote::Grant| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok((Arc::new(png_bytes(4, 4, |_, _| [0, 0, 0, 255])), 4, 4))
        }));
    // A decode of 20 MB is running (as a job handed over earlier would be).
    images.worker = Some(Worker::spawn(images.tx.clone(), images.wake.clone()));
    let shared = images.worker.as_ref().unwrap().shared.clone();
    shared.0.lock().unwrap().busy = Some((Key::Local(9), 10, 10, 20 << 20));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    images.begin_frame();
    images.request(&[], 0);
    assert!(images.remote.paused(), "held off while it runs");
    // (The pool was started held off: nothing is fetched while it is.)
    assert!(images.remote.busy());
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    // Its result is taken: they start.
    Worker::taken(&shared);
    drive(&mut images, &doc, &mut layout, &[], 0, |i| {
        i.stats().fetched == 3 && i.is_idle()
    });
    assert!(!images.remote.paused());
    assert_eq!(starts.load(Ordering::SeqCst), 3);
}
