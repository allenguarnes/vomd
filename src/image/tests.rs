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
    let (xs, ys) = (spans(frame.width, w), spans(frame.height, h));
    let (sw, ch) = (frame.width as usize, frame.channels);
    let mut acc = vec![0f32; sw * 4];
    let mut data = Vec::new();
    for (y0, wy) in &ys {
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
        for (x0, wx) in &xs {
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
    let one = 100 + SLOT_OVERHEAD + 17;
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
    assert_eq!(c.bytes, 3 * one + SLOT_OVERHEAD + 17);
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
        (256 << 10) + SLOT_OVERHEAD + url.len()
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
        c.bytes >= 12_288 * (69 + SLOT_OVERHEAD),
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
    images.request(&[(1, 30, 20), (2, 10, 10), (3, 5, 5), (4, 5, 5)], 4);
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

/// How many results arrive over `rounds` polls, 20 ms apart.
fn arrivals(images: &mut Images, doc: &Doc, layout: &mut Layout, rounds: u32) -> u32 {
    let mut n = 0;
    for _ in 0..rounds {
        std::thread::sleep(Duration::from_millis(20));
        n += u32::from(images.poll(doc, layout));
    }
    n
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
    // Asked for again, as every frame does: nothing is queued, nothing arrives.
    images.begin_frame();
    images.request(&boxes, 1);
    assert_eq!(arrivals(&mut images, &doc, &mut layout, 5), 0);
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
    // Room for three of them (40,000 bytes each) and the decode of another (its result and a
    // few rows).
    images.cache.cap = 140_000;
    // Image 1 in view, the others a screen away.
    let boxes = [(1, 100, 100), (2, 100, 100), (3, 100, 100), (4, 100, 100)];
    // A frame per result: results straddle frames, as the decodes of big images do.
    let mut settled = 0;
    for frame in 0..60 {
        images.begin_frame();
        let _ = images.pixels(1, 100, 100);
        images.request(&boxes, 1);
        let t = Instant::now();
        let mut arrived = false;
        while !arrived && t.elapsed() < Duration::from_millis(40) {
            std::thread::sleep(Duration::from_millis(1));
            arrived = images.poll(&doc, &mut layout);
        }
        settled = if arrived { 0 } else { settled + 1 };
        if settled >= 5 {
            break;
        }
        assert!(frame < 59, "still decoding after {frame} frames");
    }
    let has = |i, images: &Images| images.cache.has(&Key::Local(i), 100, 100);
    assert!(has(1, &images) && has(2, &images) && has(3, &images));
    assert!(!has(4, &images), "the fourth doesn't fit");
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
    // The JPEG is 600 × 200: a frame of 360,000 bytes of RGB, and its file.
    let file = std::fs::metadata(dir.join("a.jpg")).unwrap().len() as usize;
    assert_eq!(images.cost(1), 360_000 + file + JPEG_META_COST);
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
    images.srcs[1] = Src::File(dir.join("b.png"));
    images.srcs[2] = Src::File(dir.join("c.png"));
    images.cache.insert(Key::Local(2), slot(25_000));
    images.cache.insert(Key::Local(3), slot(25_000));
    images.begin_frame();
    let _ = images.pixels(3, 25_000, 1);
    images.request(&[(1, 600, 200)], 1);
    // The frame, the metadata allowance, the file, and the result (600 × 200 pixels).
    assert_eq!(
        images.cache.reserve,
        360_000 + JPEG_META_COST + file + 480_000
    );
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
        segments.extend(gone[2..].iter().cloned());
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

/// A file that has the pairs the decoder takes for metadata where no segment of them is
/// (a comment is skipped by us and the decoder alike, whatever is in it) can't be cut: it is
/// refused, not decoded with what it makes the decoder keep.
#[test]
fn metadata_that_cannot_be_cut_out_is_refused() {
    let plain = fixture("wide.jpg");
    // 16,000 pairs of `FF E1` and a length of 65,535, each of which the decoder may read.
    let comment = segment(0xFE, &[0xFF, 0xE1, 0xFF, 0xFF].repeat(16_000));
    let bad = with_segments(&plain, 0, &[comment]);
    assert!(jpeg_metadata(&bad) > MAX_JPEG_META);
    let mut copy = bad.clone();
    assert!(!shed_jpeg_metadata(&mut copy));
    assert_eq!(decode(&bad, 8, 8), None);
    // The same comment without them decodes.
    let fine = with_segments(&plain, 0, &[segment(0xFE, &[b'a'; 64_000])]);
    assert!(decode(&fine, 8, 8).is_some());
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
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
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

/// A progressive JPEG's decoder holds every coefficient of the image as well as its RGB frame:
/// both count in what a decode costs (a baseline JPEG's only the frame).
#[test]
fn progressive_jpegs_count_their_coefficients() {
    let info = |b: &[u8]| header_info(&mut &b[..]).unwrap();
    assert_eq!(info(&fixture("gradient.jpg")).coefficients, 0, "baseline");
    // 48 × 24, every component sampled 1 × 1: three planes of 48 × 24 i16.
    let p = info(&fixture("progressive.jpg"));
    assert_eq!(p.coefficients, 3 * 48 * 24 * 2);
    assert_eq!(
        decode_cost(&p, false),
        48 * 24 * 3 + 3 * 48 * 24 * 2 + JPEG_META_COST
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
    // The same as baseline (SOF0): nothing but the frame.
    j[3] = 0xC0;
    assert_eq!(info(&j).coefficients, 0);
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
