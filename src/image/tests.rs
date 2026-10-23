use super::*;

use crate::render::Pixels as _;
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
    assert_eq!(header_of(&png), Some((Format::Png, 7, 3)));
    let jpeg = fixture("gradient.jpg");
    assert_eq!(header_of(&jpeg), Some((Format::Jpeg, 32, 64)));
    // Progressive (SOF2) and grayscale JPEGs.
    assert_eq!(
        header_of(&fixture("progressive.jpg")[..]),
        Some((Format::Jpeg, 48, 24))
    );
    assert_eq!(
        header_of(&fixture("gray.jpg")[..]),
        Some((Format::Jpeg, 16, 16))
    );
    // Others aren't images to us.
    for other in [
        &b"GIF89a\x01\x00\x01\x00"[..],
        b"<svg xmlns=",
        b"RIFF\0\0\0\0WEBPVP8 ",
        b"",
    ] {
        assert_eq!(header_of(other), None);
    }
    // Cut short, or a zero size.
    assert_eq!(header_of(&png[..20]), None);
    let mut zero = png.clone();
    zero[16..20].copy_from_slice(&0u32.to_be_bytes());
    assert_eq!(header_of(&zero), None);
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
    assert_eq!(header_of(&j), Some((Format::Jpeg, 400, 300)));
    // DHT (C4) isn't a frame header; the scan starting first means there's none.
    let j = [0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x02, 0xFF, 0xDA, 0x00, 0x02];
    assert_eq!(header_of(&j), None);
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
        let (_, w, h) = header_of(&j).unwrap();
        assert_eq!(zune_size(&j), Some((w, h)), "{name}");
    }
    let j = crafted_jpeg();
    assert_eq!(header_of(&j), Some((Format::Jpeg, 60_000, 60_000)));
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
            let rows = decode_png(&png, w, h, true, Sampling::Area, &NO_CANCEL).unwrap();
            let frame = decode_png(&png, w, h, false, Sampling::Area, &NO_CANCEL).unwrap();
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
        let rows = decode_png(&png, 3, 2, true, Sampling::Area, &NO_CANCEL)
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(
            Some(rows),
            decode_png(&png, 3, 2, false, Sampling::Area, &NO_CANCEL),
            "{name}"
        );
    }
    // An interlaced PNG is read whole (its rows arrive pass by pass) and comes out the same as
    // the same pixels not interlaced.
    let plain = fixture("alpha.png");
    let adam7 = fixture("interlaced.png");
    assert_eq!(header_of(&adam7), Some((Format::Png, 320, 160)));
    for (w, h) in [(160, 80), (320, 160), (500, 333), (7, 5)] {
        assert_eq!(decode(&plain, w, h), decode(&adam7, w, h), "{w}×{h}");
    }
    // Data that ends early doesn't decode, whichever way it's read.
    let png = png_bytes(40, 40, noise);
    let cut = &png[..png.len() - png.len() / 3];
    assert_eq!(
        decode_png(cut, 10, 10, true, Sampling::Area, &NO_CANCEL),
        None
    );
    assert_eq!(
        decode_png(cut, 10, 10, false, Sampling::Area, &NO_CANCEL),
        None
    );
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
            for (a, p) in acc
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(row.chunks_exact(ch))
            {
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
                Sampling::Area,
                &NO_CANCEL,
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
        assert_eq!(header_of(&png), None, "{w}×{h}");
        assert_eq!(decode(&png, 100, 100), None, "{w}×{h}");
    }
    assert_eq!(
        header_of(&ihdr(65_535, 976)[..]),
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
            assert_eq!(
                decode_png(&bad, 9, 7, rows, Sampling::Area, &NO_CANCEL),
                None,
                "{what} (rows: {rows})"
            );
        }
        assert_eq!(decode(&bad, 4, 3), None, "{what}");
    }
    // What follows a good `IEND` isn't looked at.
    let mut trailing = good.clone();
    trailing.extend(b"trailing bytes");
    for rows in [true, false] {
        assert!(
            decode_png(&trailing, 9, 7, rows, Sampling::Area, &NO_CANCEL).is_some(),
            "rows: {rows}"
        );
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
    let (w, h, _) = header_of(&good).map(|(f, w, h)| (w, h, f)).unwrap();
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
        assert_eq!(
            decode_png(&bad, w, h, true, Sampling::Area, &NO_CANCEL),
            None,
            "{what}"
        );
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
            decode_png(&with, 2, 2, rows, Sampling::Area, &NO_CANCEL),
            decode_png(&plain, 2, 2, rows, Sampling::Area, &NO_CANCEL),
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

/// Every input of a past fuzz finding (fuzz/seeds/image/) goes through what the target does
/// without a panic: a JPEG segment whose length is under 2 overflowed as it was read.
#[test]
fn every_image_fuzz_seed_is_read_without_a_panic() {
    let dir = format!("{}/fuzz/seeds/image", env!("CARGO_MANIFEST_DIR"));
    let mut seeds = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let data = std::fs::read(entry.unwrap().path()).unwrap();
        assert!((1..=8).contains(&exif::orientation(&data)));
        if let Some((_, w, h)) = header_of(&data) {
            for (bw, bh) in [(1, 1), (7, 5), (w.min(64), h.min(64))] {
                if let Some(b) = decode(&data, bw, bh) {
                    assert_eq!((b.width, b.height), (bw, bh));
                }
            }
        }
        seeds += 1;
    }
    assert!(seeds >= 1);
}

/// The colors of `exif-N.jpg` and `exif-3.png` (3 × 2 cells of 8 pixels, red, green, blue over
/// white, black, yellow, as stored) shown upright, a row at a time: what a browser shows.
const UPRIGHT: [(u8, &str); 8] = [
    (1, "RGB WKY"),
    (2, "BGR YKW"),
    (3, "YKW BGR"),
    (4, "WKY RGB"),
    (5, "RW GK BY"),
    (6, "WR KG YB"),
    (7, "YB KG WR"),
    (8, "BY GK RW"),
];

/// The letters of `b`'s pixels (one a cell), in rows, by the color each is nearest.
fn cells(b: &Bitmap) -> String {
    let rows: Vec<String> = b
        .data
        .chunks(b.width as usize)
        .map(|row| {
            row.iter()
                .map(|&p| {
                    let c = |s: u32| (p >> s) & 0xff > 128;
                    match (c(16), c(8), c(0)) {
                        (true, false, false) => 'R',
                        (false, true, false) => 'G',
                        (false, false, true) => 'B',
                        (true, true, true) => 'W',
                        (false, false, false) => 'K',
                        (true, true, false) => 'Y',
                        _ => '?',
                    }
                })
                .collect()
        })
        .collect();
    rows.join(" ")
}

/// Every EXIF orientation is applied: the size a JPEG or PNG is laid out at is the upright
/// one, and so are its pixels, at the size of its cells and at its own.
#[test]
fn exif_orientation_turns_the_image_upright() {
    for (o, want) in UPRIGHT {
        let bytes = fixture(&format!("exif-{o}.jpg"));
        let (w, h) = if o >= 5 { (16, 24) } else { (24, 16) };
        assert_eq!(header_of(&bytes), Some((Format::Jpeg, w, h)), "{o}");
        assert_eq!(info_of(&bytes).unwrap().orientation, o, "{o}");
        let b = decode(&bytes, w / 8, h / 8).unwrap();
        assert_eq!((b.width, b.height), (w / 8, h / 8), "{o}");
        assert_eq!(cells(&b), want, "orientation {o}");
        // At full size, each cell's middle pixel.
        let full = decode(&bytes, w, h).unwrap();
        assert_eq!((full.width, full.height), (w, h), "{o}");
        let middles = Bitmap {
            width: w / 8,
            height: h / 8,
            data: (0..h / 8)
                .flat_map(|y| (0..w / 8).map(move |x| (x, y)))
                .map(|(x, y)| full.data[((y * 8 + 4) * w + x * 8 + 4) as usize])
                .collect(),
        };
        assert_eq!(cells(&middles), want, "orientation {o}, full size");
    }
    // A PNG's `eXIf` before its data is applied, exactly; one after it isn't.
    let png = fixture("exif-3.png");
    assert_eq!(header_of(&png), Some((Format::Png, 24, 16)));
    assert_eq!(cells(&decode(&png, 3, 2).unwrap()), "YKW BGR");
    let late = fixture("exif-late.png");
    assert_eq!(info_of(&late).unwrap().orientation, 1);
    assert_eq!(header_of(&late), Some((Format::Png, 24, 16)));
    assert_eq!(cells(&decode(&late, 3, 2).unwrap()), "RGB WKY");
    // A file's probe gives the upright size too.
    assert_eq!(
        probe_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images/exif-6.jpg")),
        Some((16, 24))
    );
}

/// An `eXIf` is found past other chunks, and not past the scan's limit or the image data.
#[test]
fn a_pngs_exif_is_looked_for_up_to_its_data() {
    let plain = png_bytes(3, 2, |_, _| [0, 0, 0, 255]);
    let exif = fixture("exif-3.png");
    let at = exif.windows(4).position(|w| w == b"eXIf").unwrap() - 4;
    let chunk = &exif[at..at + 12 + usize::from(exif[at + 3])];
    let ihdr_end = 33;
    let put = |before: &[u8], text: usize| {
        let mut b = plain[..ihdr_end].to_vec();
        if text > 0 {
            b.extend(png_chunk(b"tEXt", &vec![b'a'; text]));
        }
        b.extend(before);
        b.extend(&plain[ihdr_end..]);
        b
    };
    for text in [0, 1, 100_000] {
        let b = put(chunk, text);
        assert_eq!(
            info_of(&b).unwrap().orientation,
            3,
            "after {text} bytes of text"
        );
        assert_eq!(header_of(&b), Some((Format::Png, 3, 2)));
        assert!(decode(&b, 3, 2).is_some());
    }
    // Past the limit of the scan, it isn't read.
    let b = put(chunk, MAX_HEADER_SCAN as usize);
    assert_eq!(info_of(&b).unwrap().orientation, 1);
    // An `eXIf` cut short, or one claiming more than the file has, is as stored.
    let mut cut = put(chunk, 0);
    cut.truncate(ihdr_end + 20);
    assert_eq!(info_of(&cut).unwrap().orientation, 1);
    let mut long = chunk.to_vec();
    long[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(info_of(&put(&long, 0)).unwrap().orientation, 1);
}

/// A chunk of type `ty` holding `data`, its checksum not checked by the header.
fn png_chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(ty);
    out.extend(data);
    out.extend([0; 4]);
    out
}

/// Cutting a JPEG's metadata out keeps its orientation: an `APP1` of its own is put back, which
/// the next look at the header (the decode's, or a fetched body's) finds.
#[test]
fn shedding_metadata_keeps_the_orientation() {
    let plain = fixture("exif-6.jpg");
    let want = decode(&plain, 2, 3).unwrap();
    // A big EXIF block (a thumbnail, say) first, then others.
    let mut big_exif = b"Exif\0\0".to_vec();
    big_exif.extend(&plain[12..38]);
    big_exif.resize(65_000, 0xAB);
    for exif_first in [false, true] {
        let mut b = plain.clone();
        if exif_first {
            b.splice(2..38, segment(0xE1, &big_exif));
        }
        let with = with_segments(
            &b,
            0,
            &[
                segment(0xE1, &[b'X'; 65_000]),
                segment(0xE2, &[b'I'; 40_000]),
                segment(0xE2, &[b'I'; 40_000]),
            ],
        );
        assert!(jpeg_metadata(&with) > MAX_JPEG_META);
        assert_eq!(header_of(&with), Some((Format::Jpeg, 16, 24)));
        let mut cut = with.clone();
        assert!(shed_jpeg_metadata(&mut cut));
        assert!(cut.len() < plain.len() + 64, "{} bytes left", cut.len());
        assert_eq!(
            header_of(&cut),
            Some((Format::Jpeg, 16, 24)),
            "{exif_first}"
        );
        assert_eq!(decode(&cut, 2, 3).as_ref(), Some(&want), "{exif_first}");
        assert_eq!(decode(&with, 2, 3).as_ref(), Some(&want), "{exif_first}");
    }
    // As stored: nothing is put back.
    let upright = fixture("exif-1.jpg");
    let mut cut = with_segments(&upright, 0, &vec![segment(0xE1, &[b'X'; 65_000]); 3]);
    assert!(shed_jpeg_metadata(&mut cut));
    assert!(!cut.windows(4).any(|w| w == b"Exif"));
}

/// A turned image's decode is charged its copy: resampled to the box with its sides swapped,
/// and copied into the box.
#[test]
fn a_turned_image_is_charged_its_copy() {
    let upright = info_of(&fixture("exif-1.jpg")).unwrap();
    let turned = info_of(&fixture("exif-6.jpg")).unwrap();
    let flipped = info_of(&fixture("exif-3.jpg")).unwrap();
    assert_eq!(decode_cost(&flipped, 24, 16), decode_cost(&upright, 24, 16));
    let (w, h) = (200, 300);
    let stored = Header {
        orientation: 1,
        ..turned
    };
    assert_eq!(
        decode_cost(&turned, w, h),
        decode_cost(&stored, h, w) + 4 * (w * h) as usize
    );
    // The cost of a fetched image when its body has gone covers it, either way round.
    #[cfg(feature = "remote-images")]
    for (sw, sh) in [(16, 24), (24, 16), (1, 5000), (5000, 1)] {
        for (w, h) in [(1, 1), (16, 24), (300, 7), (7, 300)] {
            for header in [
                Header {
                    w: sh,
                    h: sw,
                    ..turned
                },
                Header {
                    w: sw,
                    h: sh,
                    ..upright
                },
            ] {
                let (sw, sh) = header.shown();
                let source = header_cost(&header);
                assert!(
                    unfetched_cost(source, (0, 0), (sw, sh), w, h) >= decode_cost(&header, w, h)
                );
            }
        }
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
    // (Image 4 was only stored: nothing painted it, and the insert took it out to make room.)
    assert!(!c.has(&Key::Local(4), 4, 1));
    c.retain(|s| s.key != Key::Local(1));
    assert_eq!(c.bytes, one);
}

/// Only a frame that was drawn keeps pixels (round 14): a result that was stored and not painted,
/// and pixels that were only touched (asked for, kept near the viewport), are evicted like any
/// other when room is wanted; what a frame painted is not, until the next frame begins. (A result
/// stored since the last paint was taken for pixels a window keeps: a request that followed, with
/// no repaint, was run over the budget beside them.)
#[test]
fn only_a_frame_that_painted_pixels_keeps_them() {
    let bm = |w: u32| {
        Data::Pixels(Bitmap {
            width: w,
            height: 1,
            data: vec![0; w as usize],
        })
    };
    let one = 16 + SLOT_OVERHEAD;
    let mut c = Cache::new(2 * one + 20);
    c.frame = 1;
    c.insert(Key::Local(1), bm(4));
    c.insert(Key::Local(2), bm(4));
    // Stored in this frame, asked for in it, and not painted: nothing is kept.
    c.touch(&Key::Local(1), 4, 1);
    assert_eq!(c.stuck(), 0);
    assert_eq!(c.kept(), 0);
    c.insert(Key::Local(3), bm(4));
    assert_eq!(c.slots.len(), 2, "one of them made the room");
    assert!(c.has(&Key::Local(3), 4, 1));
    // Painted: kept, in this frame, even over the budget.
    assert!(c.pixels(&Key::Local(3), 4, 1).is_some());
    let other = if c.has(&Key::Local(1), 4, 1) { 1 } else { 2 };
    assert!(c.pixels(&Key::Local(other), 4, 1).is_some());
    c.insert(Key::Local(4), bm(4));
    assert_eq!(c.kept(), 2 * one);
    assert!(!c.room_for(10 * one, 0, 0), "nothing more can be made");
    assert!(c.has(&Key::Local(3), 4, 1) && c.has(&Key::Local(other), 4, 1));
    // The next frame begins: what was painted is evictable again.
    c.frame += 1;
    assert_eq!(c.kept(), 0);
    c.evict_for(2 * one);
    assert!(c.slots.is_empty());
}

/// Making room is for the room: an admission that would not fit however much is evicted evicts
/// nothing (a body that waited for room took out the results nobody had painted yet, and its image
/// was decoded again, again).
#[test]
fn room_that_cannot_be_made_is_not_made_by_evicting() {
    let bm = |w: u32| {
        Data::Pixels(Bitmap {
            width: w,
            height: 1,
            data: vec![0; w as usize],
        })
    };
    let one = 16 + SLOT_OVERHEAD;
    let mut c = Cache::new(4 * one);
    c.frame = 1;
    c.insert(Key::Local(1), bm(4));
    c.insert(Key::Local(2), bm(4));
    c.reserve = 10 * one;
    assert!(
        !c.room_for(one, 0, 0),
        "what is held is more than the budget"
    );
    assert_eq!(c.slots.len(), 2, "nothing was evicted for it");
    c.reserve = 0;
    assert!(c.room_for(3 * one, 0, 0));
    assert_eq!(c.slots.len(), 1, "evicted for the room that is there now");
}

/// What a remote image's decode is taken to hold doesn't fall when its body is gone (round 14):
/// with the body cached it is what the decoder holds (30 MB for a 3200 × 2400 JPEG to a box of
/// 599 × 1748), without it the result alone (4.2 MB), and an image near the viewport was wanted
/// without the body (it fits), fetched, found too big (it doesn't), and let go, and wanted without
/// it again: the body of one URL was fetched 93 times in a window that did not change (seed 19 of
/// the random mix, with only what a frame painted kept from eviction).
#[cfg(feature = "remote-images")]
#[test]
fn a_fetched_images_cost_is_the_same_without_its_body() {
    use crate::remote::{Error, STEP};
    let jpeg = big_jpeg(3200, 2400);
    let doc = Doc::parse("![j](https://x.y/j.jpg)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let body = jpeg.clone();
    images
        .remote
        .set_fetcher(Arc::new(move |_: &str, grant: crate::remote::Grant| {
            for _ in 0..body.len().div_ceil(STEP) {
                if !grant(STEP) {
                    return Err(Error::Yielded);
                }
            }
            Ok((Arc::new(body.clone()), 3200, 2400))
        }));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    // Not fetched yet: nothing is known, and the cost is the result's.
    assert_eq!(images.cost(1, 599, 1748), 599 * 1748 * 4);
    drive(&mut images, &doc, &mut layout, &[(1, 599, 1748)], 1, |i| {
        i.cache.peek_bytes(&"https://x.y/j.jpg".into()).is_some()
    });
    let with = images.cost(1, 599, 1748);
    assert!(
        with > 3200 * 2400 * 3,
        "{with}: the frame of the JPEG is in it"
    );
    // The body goes (evicted, or never admitted): what is known of the URL stays.
    images.cache.retain(|s| !matches!(s.data, Data::Bytes(_)));
    assert!(
        images
            .cache
            .peek_bytes(&"https://x.y/j.jpg".into())
            .is_none()
    );
    let without = images.cost(1, 599, 1748);
    assert!(
        without >= with,
        "{without} without the body, {with} with it"
    );
}

/// The window's viewport is noted whole, whatever number of its boxes a request has the room for
/// (round 14): a body for the 600th image in view was taken for one that is not in view when the
/// request, cut to the 60 it had room to ask for, listed only those, and was dropped and fetched
/// again with every request.
#[cfg(feature = "remote-images")]
#[test]
fn the_viewport_is_noted_whole_whatever_a_request_asks_for() {
    const N: u32 = 600;
    let src: String = (0..N)
        .map(|i| format!("![t](https://x.y/{i}.png) "))
        .collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    // Nothing of the budget is left over for what a request builds: it asks for few of the boxes.
    images.cache.cap = 1 << 20;
    let boxes: Vec<_> = (1..=N).map(|id| (id, 1, 1)).collect();
    images.begin_frame();
    images.request(&boxes, boxes.len());
    assert_eq!(images.in_view.len(), N as usize);
    // Only the first 100 are in view the next time, and the list is what is in view.
    images.begin_frame();
    images.request(&boxes, 100);
    assert_eq!(images.in_view.len(), 100);
}

/// The list of slots has a place for each image in view beside what it holds (round 14): made
/// for the viewport when it is asked for, whatever the room (the budget is full of what a window
/// paints and a list that doubles beside the old one costs more than a request may take), and
/// kept when slots go. A document of thousands of images that has few in view holds no more than
/// those: nothing is charged for the document's.
#[test]
fn the_list_of_slots_has_a_place_for_each_image_in_view() {
    const N: u32 = 300;
    let src: String = (0..N).map(|_| "![a](/nowhere/a.png) ").collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.attach(&doc, &mut layout);
    assert!(
        images.cache.slots.capacity() < 16,
        "a document's images are no places"
    );
    // No room at all, and all of them in view.
    images.cache.cap = 1 << 10;
    let boxes: Vec<_> = (1..=N).map(|id| (id, 4, 4)).collect();
    images.begin_frame();
    images.request(&boxes, boxes.len());
    assert_eq!(images.cache.floor, N as usize);
    assert!(images.cache.slots.capacity() >= N as usize);
    // Fewer in view: that many, and the list is cut to them.
    images.begin_frame();
    images.request(&boxes, 10);
    assert_eq!(images.cache.floor, 10);
    images.cache.trim();
    assert!(images.cache.slots.capacity() >= 10);
    assert!(images.cache.slots.capacity() <= 2 * 10 + 16 + images.cache.expect);
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
        painted: 0,
        protect: false,
        admitted: false,
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
            used: 0,
            painted: 0,
            protect: false,
            admitted: false,
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
    // them) beside the decode of another (its file, its result and what decoding holds) and what
    // the request needs while it runs (a few KB). One that
    // would cross the line is the one that isn't kept: the budget is not gone over by a box.
    images.cache.cap = images.cost(4, 100, 100) + 140_000;
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

/// A document whose tables fit the budget when nothing is decoding, but not beside a decode that is
/// still running (the old document's, whose result nobody has taken), has them made by the poll
/// that takes the decode's result: until then nothing is allocated for it and every image shows
/// its alt text, and the decode is never held beside them.
#[test]
fn tables_that_do_not_fit_beside_a_decode_wait_for_it() {
    let dir = scratch("later");
    std::fs::write(
        dir.join("a.png"),
        png_bytes(30, 20, |_, _| [0, 0, 255, 255]),
    )
    .unwrap();
    let old = Doc::parse("![o](a.png)");
    let mut old_layout = Layout::new(&old, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&old, &mut old_layout);
    // A decode that is running: the worker has a job it has not delivered, which holds 1 MB.
    images.worker = Some(Worker::spawn(images.tx.clone(), images.wake.clone()));
    let shared = images.worker.as_ref().unwrap().shared.clone();
    shared.0.lock().unwrap().busy = Some((Key::Local(1), 30, 20, 1_000_000));
    let src: String = (0..100).map(|_| "![p](a.png) ").collect();
    let doc = Doc::parse(&src);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    // Room for the tables and what is kept, and not for the decode beside them.
    images.cache.cap = 1_000_000 + attach_bytes(100, 0) / 2;
    images.attach(&doc, &mut layout);
    assert!(images.unbuilt, "the tables wait");
    assert!(
        images.srcs.is_empty() && images.srcs.capacity() == 0,
        "nothing made"
    );
    images.begin_frame();
    images.request(&[(1, 30, 20)], 1);
    assert!(images.srcs.is_empty());
    assert_eq!(images.stats().decodes, 0);
    // The decode is over and its result taken: the tables are made, the layout told.
    Worker::taken(&shared);
    assert!(images.poll(&doc, &mut layout), "the poll makes them");
    assert!(!images.unbuilt);
    assert_eq!(images.srcs.len(), 100);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 600.0);
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(30, 20)));
    // One that wouldn't fit when nothing is decoding either is never made.
    images.cache.cap = attach_bytes(100, 0) - 1;
    images.attach(&doc, &mut layout);
    assert!(
        !images.unbuilt && images.srcs.is_empty(),
        "no room even idle: alt text"
    );
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
    let info = |b: &[u8]| info_of(b).unwrap();
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
        frame(&mut images, &boxes, boxes.len());
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
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "stuck: {:?} {}",
            images.stats(),
            images.memory_report()
        );
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
        i.pixels(1, 720, 6000).is_some() && i.deferred_need(&url).is_some() && i.is_idle()
    });
    let n0 = fetches.load(Ordering::SeqCst);
    assert!(n0 <= 2, "the body was fetched {n0} times");
    // However often the window paints and polls from here, nothing is fetched (the pool is
    // idle when the last frame is over: a fetch asked for in it would show).
    for _ in 0..300 {
        frame(&mut images, &both, 1);
        images.poll(&doc, &mut layout);
        assert!(
            images.pixels(1, 720, 6000).is_some(),
            "{}",
            images.memory_report()
        );
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
            && urls.iter().all(|u| i.deferred_need(u).is_some())
            && i.is_idle()
    });
    let n0 = fetches.load(Ordering::SeqCst);
    assert!(n0 <= 4, "{n0} fetches of two bodies that don't fit");
    for _ in 0..300 {
        frame(&mut images, &with_tall, 1);
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

/// Fetches that finish together are taken by one poll, with no request between them to keep what
/// it wants: a body let in is not the first thing the room for the next is made from. (Nothing
/// has used it yet, which was all the cache went by, so the oldest body and the one that had just
/// come were alike to it: a result for an image near the viewport was let in, evicted by the next
/// in the same poll and fetched again, and so was that, until the viewport had a third fetch of a
/// URL it never moved from.) What doesn't fit beside it is dropped for room, and not fetched
/// again until it does.
#[cfg(feature = "remote-images")]
#[test]
fn a_body_let_in_is_not_evicted_by_the_next_of_the_same_poll() {
    use crate::render::Pixels;
    let body = || {
        let mut b = png_bytes(40, 30, |x, y| [x as u8, y as u8, 9, 255]);
        b.extend(vec![0u8; 1 << 20]);
        b
    };
    let doc = Doc::parse("![a](https://x.y/a.png) ![b](https://x.y/b.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let gate = Gate::new();
    images.remote.set_fetcher(holding_fetcher(
        &gate,
        &Arc::default(),
        png_bytes(40, 30, |_, _| [0, 0, 0, 255]),
    ));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let (a, b): (Arc<str>, Arc<str>) = ("https://x.y/a.png".into(), "https://x.y/b.png".into());
    // Room for one body beside what is set aside for the fetchers, and not for two. No image is
    // in view: both are for the images near it, or nearer, and the same to the cache.
    let sample = Arc::new(body());
    let one = Slot::bytes_cost(&a, &sample);
    let decode = images.decode_estimate(&a, &sample);
    images.cache.cap =
        images.fetch_static(true, true) + images.cache.overhead + decode + one * 3 / 2;
    for url in [&a, &b] {
        let result = Ok((Arc::new(body()), 40, 30));
        images
            .tx
            .send(Done::Fetched {
                url: url.clone(),
                result,
            })
            .unwrap();
    }
    images.poll(&doc, &mut layout);
    assert!(
        images.cache.peek_bytes(&a).is_some(),
        "{}",
        images.memory_report()
    );
    assert!(
        images.cache.peek_bytes(&b).is_none(),
        "{}",
        images.memory_report()
    );
    assert!(images.deferred_need(&b).is_some(), "dropped for room");
    // The protection ends with the poll: what comes next evicts the body like any other.
    assert!(images.cache.slots.iter().all(|s| !s.protect));
    let result = Ok((Arc::new(body()), 40, 30));
    images
        .tx
        .send(Done::Fetched {
            url: b.clone(),
            result,
        })
        .unwrap();
    images.poll(&doc, &mut layout);
    assert!(
        images.cache.peek_bytes(&b).is_some(),
        "{}",
        images.memory_report()
    );
    assert!(
        images.cache.peek_bytes(&a).is_none(),
        "{}",
        images.memory_report()
    );
    gate.open();
    let _ = images.pixels(1, 40, 30);
}

/// A body let in earlier in the poll is kept from the next body's room and from no other: a
/// decoded result that comes after it is stored whatever there is, and evicts it, so that the
/// budget holds beside the job that starts next (the one the result's worker goes on to). Kept
/// from it too, a body for an image near the viewport made the cache over its budget by the
/// body's size for as long as that job ran: nothing evicts again until the next request.
#[cfg(feature = "remote-images")]
#[test]
fn a_decoded_result_evicts_a_body_let_in_earlier_in_the_poll() {
    use crate::render::Pixels;
    let side = 500;
    let pc = side as usize * side as usize * 4;
    let body = || {
        let mut b = png_bytes(40, 30, |x, y| [x as u8, y as u8, 9, 255]);
        b.extend(vec![0u8; 1 << 20]);
        b
    };
    let doc = Doc::parse("![a](https://x.y/a.png) ![p](https://x.y/p.png) ![q](https://x.y/q.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_policy(crate::settings::RemoteImages::Always);
    let gate = Gate::new();
    images.remote.set_fetcher(holding_fetcher(
        &gate,
        &Arc::default(),
        png_bytes(40, 30, |_, _| [0, 0, 0, 255]),
    ));
    images.attach(&doc, &mut layout);
    images.set_remote(true, &doc, &mut layout);
    let (a, p): (Arc<str>, Arc<str>) = ("https://x.y/a.png".into(), "https://x.y/p.png".into());
    // Their fetches are in flight, holding what they may.
    drive(&mut images, &doc, &mut layout, &[], 0, |i| {
        i.remote.inflight() >= 3 * 15 * (256 << 10)
    });
    // A worker with no thread: the job of `p` has its result sent, and `q` is queued behind it.
    let job = |key| Job {
        doc_gen: images.doc_gen,
        key,
        source: Source::Bytes(Arc::new(Vec::new())),
        w: side,
        h: side,
        cost: pc,
    };
    let shared: Shared = Arc::default();
    {
        let mut q = shared.0.lock().unwrap();
        q.busy = Some((Key::Remote(p.clone()), side, side, pc));
        q.jobs.push(job(Key::Remote("https://x.y/q.png".into())));
    }
    images.worker = Some(Worker {
        cancel: Arc::default(),
        inline: Arc::default(),
        shared: shared.clone(),
    });
    images.cache.expect = 1;
    // Room for one body beside the decode and what is set aside for the fetchers, and the
    // pixels of `p` in place of it, and no more.
    let sample = Arc::new(body());
    let one = Slot::bytes_cost(&a, &sample);
    images.cache.cap = images.fetch_static(true, true)
        + images.cache.overhead
        + pc
        + images.decode_estimate(&a, &sample).saturating_sub(pc)
        + one
        + (64 << 10);
    let bitmap = Bitmap {
        width: side,
        height: side,
        data: vec![0u32; side as usize * side as usize],
    };
    images
        .tx
        .send(Done::Fetched {
            url: a.clone(),
            result: Ok((Arc::new(body()), 40, 30)),
        })
        .unwrap();
    images
        .tx
        .send(Done::Decoded {
            doc_gen: images.doc_gen,
            key: Key::Remote(p.clone()),
            bitmap: Some(bitmap),
            need: None,
            refused: None,
            skipped: false,
            anim: None,
            cancelled: false,
            worker: shared.clone(),
        })
        .unwrap();
    images.cache.fetching = images.fetch_reserve(false);
    images.poll(&doc, &mut layout);
    assert!(
        images.cache.has(&Key::Remote(p.clone()), side, side),
        "the result is stored: {}",
        images.memory_report()
    );
    // The body was let in (it was not dropped for room), and is gone for the result's.
    assert!(
        images.deferred_need(&a).is_none(),
        "{}",
        images.memory_report()
    );
    assert!(images.cache.peek_bytes(&a).is_none());
    // The next job runs (the queue is the worker's own to move on).
    {
        let mut q = shared.0.lock().unwrap();
        assert!(q.busy.is_none(), "the result was taken");
        let next = q.jobs.remove(0);
        q.busy = Some((next.key, next.w, next.h, next.cost));
    }
    images.refresh_reserve();
    assert!(
        images.cache.bytes + images.cache.held() <= images.cache.cap,
        "{} + {} over {}\n{}",
        images.cache.bytes,
        images.cache.held(),
        images.cache.cap,
        images.memory_report()
    );
    assert!(images.cache.slots.iter().all(|s| !s.protect && !s.admitted));
    gate.open();
    let _ = images.pixels(1, 40, 30);
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

/// Replacing the queue says what the job the worker has begun holds, as of the swap: a job taken
/// from the old queue after a request worked out its reserve runs all the same, and the request
/// keeps room for it from this.
#[test]
fn replacing_the_queue_returns_what_the_running_job_holds() {
    let body = Arc::new(png_bytes(40, 30, |x, y| [x as u8, y as u8, 2, 255]));
    let (tx, rx) = channel();
    let worker = Worker::spawn(tx, Arc::new(|| {}));
    assert_eq!(worker.set(Vec::new()), 0, "nothing runs");
    let job = || Job {
        doc_gen: 0,
        key: Key::Remote("https://x.y/a.png".into()),
        source: Source::Bytes(body.clone()),
        w: 40,
        h: 30,
        cost: 12_345,
    };
    worker.set(vec![job()]);
    // The result is not taken: the job is held until it is.
    let Ok(Done::Decoded { worker: w, .. }) = rx.recv_timeout(Duration::from_secs(60)) else {
        panic!("a result");
    };
    assert_eq!(worker.set(Vec::new()), 12_345);
    assert_eq!(worker.busy_cost(), 12_345);
    Worker::taken(&w);
    assert_eq!(worker.set(Vec::new()), 0, "its result was taken");
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
    assert!(
        decode_job(&job(cost), &mut WorkerState::default())
            .bitmap
            .is_some()
    );
    std::fs::write(
        &path,
        png_bytes(400, 300, |x, y| [(x * y) as u8, y as u8, 3, 255]),
    )
    .unwrap();
    assert!(
        decode_job(&job(cost), &mut WorkerState::default())
            .bitmap
            .is_none(),
        "bigger than it was"
    );
    // Charged for what it is now, it decodes.
    let cost = file_info(&path).unwrap().cost(40, 30);
    assert!(
        decode_job(&job(cost), &mut WorkerState::default())
            .bitmap
            .is_some()
    );
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
        fit_map(&mut m, 0);
        assert!(m.capacity() <= 2 * 3 + 16, "{}", m.capacity());
        // With room for what a document will add, which is there whatever was pruned.
        fit_map(&mut m, 100);
        assert!(m.capacity() >= 103, "{}", m.capacity());
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

/// One frame of the window: begun, what is in view painted (a frame is what protects pixels from
/// being evicted: nothing else does), the images near asked for.
#[cfg(feature = "remote-images")]
fn frame(images: &mut Images, boxes: &[(u32, u32, u32)], on_screen: usize) {
    use crate::render::Pixels;
    images.begin_frame();
    for &(id, w, h) in boxes.iter().take(on_screen) {
        let _ = images.pixels(id, w, h);
    }
    images.request(boxes, on_screen);
}

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
    images.remote.failed(&url, true, None);
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
            .take_fetched(&u, Ok((body.clone(), 4, 4)), true)
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
    // (The fetchers take their URLs from the queue on their own threads: once all four have,
    // nothing more is taken from it until the gate opens, and what is charged holds still.)
    let t = Instant::now();
    while starts.load(std::sync::atomic::Ordering::SeqCst) < 4 {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
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

/// The images in view are decided by one function ([`Images::view_of`], round 15): at most
/// `VIEW_MAX` images, a URL counted once however often the document refers to it, and the first
/// box past the cap ends the view (it and the ones after are not in view for any purpose).
#[cfg(feature = "remote-images")]
#[test]
fn the_view_counts_each_remote_image_once_and_ends_at_the_cap() {
    let boxes = |n: usize| (1..=n as u32).map(|id| (id, 10, 10)).collect::<Vec<_>>();
    let view = |text: &str, n: usize, on_screen: usize| {
        let doc = Doc::parse(text);
        let mut layout = Layout::new(&doc, crate::layout::Params::default());
        let mut images = Images::default();
        images.attach(&doc, &mut layout);
        images.set_policy(crate::settings::RemoteImages::Always);
        images.set_remote(true, &doc, &mut layout);
        let view = images.view_of(&boxes(n), on_screen);
        (view.len, view.places, images.view_urls.len())
    };
    // 5,000 references to one URL and then another: two images, all in view.
    let mut text = "![t](http://127.0.0.1/a.png) ".repeat(5_000);
    text.push_str("![j](http://127.0.0.1/b.jpg)");
    assert_eq!(view(&text, 5_001, 5_001), (5_001, 2, 2));
    // (Only the first `on_screen` boxes are looked at.)
    assert_eq!(view(&text, 5_001, 100), (100, 1, 1));
    // VIEW_MAX distinct images are in view, and the one after them is not.
    let distinct = |n: usize| {
        (0..n)
            .map(|i| format!("![t](http://127.0.0.1/{i}.png) "))
            .collect::<String>()
    };
    assert_eq!(
        view(&distinct(VIEW_MAX), VIEW_MAX, VIEW_MAX),
        (VIEW_MAX, VIEW_MAX, VIEW_MAX)
    );
    assert_eq!(
        view(&distinct(VIEW_MAX + 1), VIEW_MAX + 1, VIEW_MAX + 1),
        (VIEW_MAX, VIEW_MAX, VIEW_MAX)
    );
    // A repeat of one in view beyond the cap's box is not what ends it, the next new one is.
    let mut text = distinct(VIEW_MAX - 1);
    text.push_str("![t](http://127.0.0.1/0.png) ![t](http://127.0.0.1/last.png) ![t](http://127.0.0.1/over.png)");
    assert_eq!(
        view(&text, VIEW_MAX + 2, VIEW_MAX + 2),
        (VIEW_MAX + 1, VIEW_MAX, VIEW_MAX)
    );
}

/// An image that is resolved (decoded at its box, or not to be loaded at all) takes no place of
/// the cap on the images in view, so the view advances past what has loaded (round 16): 4,500
/// distinct images on one screen are in view 4,096 at a time, and the heavy one after them is in
/// view once they have decoded.
#[cfg(feature = "remote-images")]
#[test]
fn the_view_advances_past_the_images_that_have_loaded() {
    let distinct = |n: usize| {
        (0..n)
            .map(|i| format!("![t](http://127.0.0.1/{i}.png)"))
            .collect::<String>()
    };
    let view = |n: usize, loaded: usize, remote: bool| {
        let doc = Doc::parse(&distinct(n));
        let mut layout = Layout::new(&doc, crate::layout::Params::default());
        let mut images = Images::default();
        images.attach(&doc, &mut layout);
        images.set_policy(crate::settings::RemoteImages::Always);
        images.set_remote(remote, &doc, &mut layout);
        for id in 1..=loaded as u32 {
            let Src::Remote(url) = images.src(id).clone() else {
                unreachable!()
            };
            let one = Data::Pixels(Bitmap {
                width: 1,
                height: 1,
                data: vec![0],
            });
            images.cache.insert(Key::Remote(url), one);
        }
        let boxes: Vec<_> = (1..=n as u32).map(|id| (id, 1, 1)).collect();
        let view = images.view_of(&boxes, n);
        (view.len, view.places, images.view_urls.len())
    };
    // None loaded: the cap ends the view; the first VIEW_MAX loaded: the rest are in view.
    assert_eq!(
        view(VIEW_MAX + 404, 0, true),
        (VIEW_MAX, VIEW_MAX, VIEW_MAX)
    );
    assert_eq!(
        view(VIEW_MAX + 404, VIEW_MAX, true),
        (VIEW_MAX + 404, 404, 404)
    );
    // The cap is of the ones still to load: 4,000 loaded and 4,200 to go end it 4,096 on.
    assert_eq!(
        view(8_200, 4_000, true),
        (4_000 + VIEW_MAX, VIEW_MAX, VIEW_MAX)
    );
    // With remote images off none of them is to be loaded: none is wanted.
    assert_eq!(view(VIEW_MAX + 404, 0, false), (VIEW_MAX + 404, 0, 0));
}

/// An SVG whose first job finds it needs more than it was charged is asked for again with what
/// it needs, and drawn; one that needs more than the cache is refused (its placeholder says so),
/// and drawn once it is clicked, outside the budget.
#[cfg(feature = "svg")]
#[test]
fn an_svg_charged_too_little_is_asked_for_again() {
    use crate::render::Pixels;
    let dir = scratch("svg-again");
    let svg = |groups: usize| {
        let body =
            "<g transform=\"translate(1 1)\"><rect width=\"1\" height=\"1\" fill=\"#e00\"/></g>"
                .repeat(groups);
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\">{body}</svg>")
    };
    std::fs::write(dir.join("a.svg"), svg(8_000)).unwrap();
    std::fs::write(dir.join("b.svg"), svg(40_000)).unwrap();
    let doc = Doc::parse("![a](a.svg) ![b](b.svg)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 600.0);
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(40, 30)));
    let t = Instant::now();
    let mut requests = 0;
    while images.pixels(1, 40, 30).is_none()
        || !matches!(layout.image_size(2), Some(ImageSize::Refused(_)))
    {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        images.request(&[(1, 40, 30), (2, 40, 30)], 2);
        requests += 1;
        std::thread::sleep(Duration::from_millis(5));
        images.poll(&doc, &mut layout);
    }
    assert_eq!(images.pixels(1, 40, 30).unwrap().data[40 + 1], 0xFFEE_0000);
    for id in [1, 2] {
        assert!(images.costs[&id].unwrap().header.svg.parse > 0, "measured");
    }
    let need = images.costs[&2].unwrap().cost(40, 30);
    assert!(need > CACHE_BYTES);
    let Some(ImageSize::Refused(Refusal::Memory(n))) = layout.image_size(2) else {
        panic!("{:?}", layout.image_size(2));
    };
    assert!(n as usize >= need, "{n} < {need}");
    assert_eq!(images.stats().decodes, 1);
    // Not asked for over and over: a job or two each, one more when measured.
    assert!(requests < 200, "{requests}");
    // Clicked, it is drawn on the forced thread, and nothing of it is in the cache.
    assert!(images.force(2, &doc, &mut layout));
    assert_eq!(layout.image_size(2), Some(ImageSize::Pixels(40, 30)));
    while images.pixels(2, 40, 30).is_none() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out");
        images.request(&[(1, 40, 30), (2, 40, 30)], 2);
        std::thread::sleep(Duration::from_millis(5));
        images.poll(&doc, &mut layout);
    }
    assert_eq!(images.pixels(2, 40, 30).unwrap().data[40 + 1], 0xFFEE_0000);
    assert!(
        !images.cache.has(&Key::Local(2), 40, 30),
        "not in the cache"
    );
    assert_eq!(images.forced_loads(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An SVG that draws a big raster image whole (nearest neighbor) is loaded at one box size and
/// at another, a zoom in or out: what the image holds doesn't grow with the box, so neither is
/// refused for it.
#[cfg(feature = "svg")]
#[test]
fn an_svg_with_a_whole_raster_loads_at_another_box_size() {
    use crate::render::Pixels;
    let dir = scratch("svg-zoom");
    let mut png = Vec::new();
    let mut enc = png::Encoder::new(&mut png, 1024, 1024);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .unwrap()
        .write_image_data(&vec![7; 1024 * 1024])
        .unwrap();
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"100\"><image width=\"100\" height=\"100\" style=\"image-rendering:pixelated\" href=\"data:image/png;base64,{}\"/></svg>",
        crate::image::svg::tests::base64(&png)
    );
    std::fs::write(dir.join("a.svg"), svg).unwrap();
    for (first, then) in [(100, 200), (200, 100)] {
        let (doc, mut layout, mut images) = forced_page(&dir, "![a](a.svg)");
        for side in [first, then] {
            let boxes = [(1, side, side)];
            pump(&doc, &mut layout, &mut images, &boxes, |i, l| {
                assert!(
                    !matches!(l.image_size(1), Some(ImageSize::Refused(_))),
                    "refused at {side}: {:?}",
                    l.image_size(1)
                );
                i.pixels(1, side, side).is_some_and(|b| b.width == side)
            });
            assert_eq!(images.pixels(1, side, side).unwrap().data[0], 0xFF07_0707);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An SVG that takes too long to draw is given up at its deadline: told to stop, shown as its
/// alt text with a click to load, and its worker let go; the raster images after it are decoded by a new one, and
/// another SVG waits until the one given up is done. With nothing drawing, there's no deadline.
#[cfg(feature = "svg")]
#[test]
fn an_svg_that_takes_too_long_is_given_up() {
    use crate::render::Pixels;
    let dir = scratch("svg-slow");
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
    std::fs::write(dir.join("slow.svg"), &slow).unwrap();
    std::fs::write(dir.join("logo.svg"), fixture("logo-viewbox.svg")).unwrap();
    std::fs::write(
        dir.join("a.png"),
        png_bytes(30, 20, |_, _| [0, 0, 255, 255]),
    )
    .unwrap();
    let doc = Doc::parse("![slow](slow.svg) ![a](a.png) ![logo](logo.svg)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images {
        svg_deadline: Duration::from_millis(1),
        ..Images::default()
    };
    images.set_base(Some(dir.clone()));
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 600.0);
    assert_eq!(images.deadline(), None);
    let boxes = [(1, 512, 512), (2, 30, 20), (3, 48, 32)];
    // The worker takes the SVG first (measured, and asked for again: then it's slow). Only it is
    // asked for, so the logo can't be drawn before the slow one is given up.
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
    assert!(images.worker.is_none() && images.svg_stuck());
    images.poll(&doc, &mut layout);
    assert_eq!(
        layout.image_size(1),
        Some(ImageSize::Refused(Refusal::Slow))
    );
    // The PNG is decoded by a new worker; the other SVG waits for the one given up.
    while images.pixels(2, 30, 20).is_none() || images.svg_stuck() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        images.request(&boxes[1..], 2);
        if images.svg_stuck() {
            assert!(images.pixels(3, 48, 32).is_none());
        }
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    // What the stuck drawing made when it stopped doesn't take its refusal away: the placeholder
    // still says a click loads it.
    images.poll(&doc, &mut layout);
    assert_eq!(
        layout.image_size(1),
        Some(ImageSize::Refused(Refusal::Slow))
    );
    images.svg_deadline = SVG_DEADLINE;
    while images.pixels(3, 48, 32).is_none() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        images.request(&boxes[1..], 2);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert_eq!(images.deadline(), None);
    assert!(!images.expire(Instant::now() + Duration::from_secs(60)));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Loads past the limits (a click on the placeholder) ---------------------------------------

/// A black 1-bit gray PNG of `w` × `h`: a few KB however many pixels.
fn big_png(w: u32, h: u32) -> Vec<u8> {
    filled_png(w, h, 0)
}

/// [`big_png`] filled with the bytes `fill` (0 black, 255 white).
fn filled_png(w: u32, h: u32, fill: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::One);
    let data = vec![fill; w.div_ceil(8) as usize * h as usize];
    enc.write_header().unwrap().write_image_data(&data).unwrap();
    out
}

/// A document of `md` in `dir`, attached and laid out.
fn forced_page(dir: &Path, md: &str) -> (Doc, Layout, Images) {
    let doc = Doc::parse(md);
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_base(Some(dir.to_path_buf()));
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 2000.0);
    (doc, layout, images)
}

/// Stand in for the event loop: ask for `boxes` and poll until `done`, at most 20 s.
fn pump(
    doc: &Doc,
    layout: &mut Layout,
    images: &mut Images,
    boxes: &[(u32, u32, u32)],
    mut done: impl FnMut(&mut Images, &Layout) -> bool,
) {
    let t = Instant::now();
    while !done(images, layout) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        images.begin_frame();
        images.request(boxes, boxes.len());
        std::thread::sleep(Duration::from_millis(2));
        images.poll(doc, layout);
    }
}

/// An image over VOMD's limits shows why, and a click loads it on the forced thread, outside
/// the cache: at each box size, the old pixels shown until the new ones come; the choice is kept
/// over a reload. What isn't over the limits (missing, broken) is plain alt text.
#[test]
fn a_clicked_image_loads_past_the_limits() {
    use crate::render::Pixels;
    let dir = scratch("forced");
    std::fs::write(dir.join("big.png"), big_png(9000, 8000)).unwrap();
    std::fs::write(dir.join("ok.png"), png_bytes(4, 3, |_, _| [9, 9, 9, 255])).unwrap();
    std::fs::write(dir.join("broken.png"), b"\x89PNG\r\n\x1a\nnot really").unwrap();
    let md = "![big](big.png) ![ok](ok.png) ![missing](none.png) ![broken](broken.png)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    let refused = ImageSize::Refused(Refusal::Pixels(9000, 8000));
    assert_eq!(layout.image_size(1), Some(refused));
    assert_eq!(layout.image_size(2), Some(ImageSize::Pixels(4, 3)));
    assert_eq!(layout.image_size(3), Some(ImageSize::Alt));
    assert_eq!(layout.image_size(4), Some(ImageSize::Alt));
    // Nothing is clicked: nothing past the limits.
    assert!(
        images.force(3, &doc, &mut layout),
        "the API doesn't judge: the view does"
    );
    images.request(&[(2, 4, 3)], 1);
    pump(&doc, &mut layout, &mut images, &[(2, 4, 3)], |i, _| {
        i.pixels(2, 4, 3).is_some() && !i.forcing()
    });
    assert_eq!(
        layout.image_size(3),
        Some(ImageSize::Refused(Refusal::Failed))
    );

    assert!(images.force(1, &doc, &mut layout));
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(9000, 8000)));
    let loads = images.forced_loads();
    pump(&doc, &mut layout, &mut images, &[(1, 90, 80)], |i, _| {
        i.pixels(1, 90, 80).is_some()
    });
    assert_eq!(images.forced_loads(), loads + 1);
    let b = images.pixels(1, 90, 80).unwrap();
    assert_eq!((b.width, b.height, b.data[0]), (90, 80, 0xFF00_0000));
    assert!(!images.cache.has(&Key::Local(1), 90, 80), "not the cache's");
    // Another size: the old pixels until the new ones come, and one load.
    images.request(&[(1, 180, 160)], 1);
    assert_eq!(images.pixels(1, 180, 160).unwrap().width, 90);
    pump(&doc, &mut layout, &mut images, &[(1, 180, 160)], |i, _| {
        i.pixels(1, 180, 160).unwrap().width == 180
    });
    assert_eq!(images.forced_loads(), loads + 2);
    // Reloaded: still chosen, laid out at its size, never refused in between.
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 2000.0);
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(9000, 8000)));
    pump(&doc, &mut layout, &mut images, &[(1, 180, 160)], |i, _| {
        !i.forcing() && i.pixels(1, 180, 160).is_some()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// A load past the limits runs beside the normal decodes (a PNG is decoded while it is held),
/// and ends when its document goes or its box changes size (and it is loaded again at the new
/// one); with nothing to load, the thread is gone.
#[test]
fn a_load_past_the_limits_is_abandoned() {
    use crate::render::Pixels;
    let dir = scratch("forced-abandon");
    std::fs::write(dir.join("big.png"), big_png(9000, 8000)).unwrap();
    std::fs::write(dir.join("ok.png"), png_bytes(4, 3, |_, _| [9, 9, 9, 255])).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![big](big.png) ![ok](ok.png)");
    images.forced.hold.store(true, Ordering::Relaxed);
    assert!(images.force(1, &doc, &mut layout));
    let boxes = [(1, 90, 80), (2, 4, 3)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        i.pixels(2, 4, 3).is_some()
    });
    assert!(
        images.forcing() && images.pixels(1, 90, 80).is_none(),
        "held"
    );
    // Another size: the load held is cancelled, and the new one waits.
    images.request(&[(1, 45, 40)], 1);
    images.forced.hold.store(false, Ordering::Relaxed);
    pump(&doc, &mut layout, &mut images, &[(1, 45, 40)], |i, _| {
        i.pixels(1, 45, 40).is_some() && !i.forcing()
    });
    assert_eq!(
        images.pixels(1, 45, 40).unwrap().width,
        45,
        "only the new size"
    );
    // Held again at another size, then another document: over within 200 ms.
    images.forced.hold.store(true, Ordering::Relaxed);
    images.request(&[(1, 90, 80)], 1);
    assert!(images.forcing());
    let other = Doc::parse("Nothing.");
    images.attach(&other, &mut layout);
    let t = Instant::now();
    while images.forcing() {
        assert!(t.elapsed() < Duration::from_secs(10), "still loading");
        std::thread::sleep(Duration::from_millis(1));
    }
    // Idle: nothing wakes the window.
    let wakes = images.wakes();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(images.wakes(), wakes);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A reload while a load past the limits runs (it is cancelled with the old document) loads the
/// image again, though nothing asks for it a second time; and one loaded already is loaded
/// again, as its file may have changed, the old pixels shown until the new ones come.
#[test]
fn a_reload_loads_a_clicked_image_again() {
    use crate::render::Pixels;
    let dir = scratch("forced-reload");
    std::fs::write(dir.join("big.png"), big_png(9000, 8000)).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![big](big.png)");
    images.forced.hold.store(true, Ordering::Relaxed);
    assert!(images.force(1, &doc, &mut layout));
    images.request(&[(1, 90, 80)], 1);
    while !images.forced.running() {
        std::thread::sleep(Duration::from_millis(1));
    }
    // Reloaded while it runs: asked for once, and then the window only polls (as it would, woken).
    images.attach(&doc, &mut layout);
    images.request(&[(1, 90, 80)], 1);
    images.forced.hold.store(false, Ordering::Relaxed);
    let t = Instant::now();
    while images.pixels(1, 90, 80).is_none() {
        assert!(t.elapsed() < Duration::from_secs(20), "never loaded");
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert_eq!(images.pixels(1, 90, 80).unwrap().data[0], 0xFF00_0000);
    // The file changed, and the page is reloaded: the old pixels until the new ones.
    std::fs::write(dir.join("big.png"), filled_png(9000, 8000, 255)).unwrap();
    images.attach(&doc, &mut layout);
    assert_eq!(images.pixels(1, 90, 80).unwrap().data[0], 0xFF00_0000);
    pump(&doc, &mut layout, &mut images, &[(1, 90, 80)], |i, _| {
        i.pixels(1, 90, 80).unwrap().data[0] == 0xFFFF_FFFF
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// A load whose charge is over the ceiling isn't started: the placeholder says so, isn't
/// clickable, and the image isn't tried again (a reload, a zoom) until its file changes.
#[test]
fn a_load_over_the_ceiling_says_so() {
    let dir = scratch("forced-ceiling");
    std::fs::write(dir.join("big.png"), big_png(9000, 8000)).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![big](big.png)");
    images.set_forced_limits(Some(1 << 20), forced::KEPT_BYTES);
    assert!(images.force(1, &doc, &mut layout));
    pump(&doc, &mut layout, &mut images, &[(1, 900, 800)], |_, l| {
        matches!(l.image_size(1), Some(ImageSize::Refused(_)))
    });
    let Some(ImageSize::Refused(why @ Refusal::Ceiling(n))) = layout.image_size(1) else {
        panic!("{:?}", layout.image_size(1));
    };
    assert!(n > 1 << 20 && !why.forceable());
    let loads = images.forced_loads();
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 2000.0);
    assert_eq!(layout.image_size(1), Some(ImageSize::Refused(why)));
    images.request(&[(1, 90, 80)], 1);
    assert_eq!(images.forced_loads(), loads, "not tried again");
    // Changed, it is over the normal limits again: clickable.
    std::fs::write(dir.join("big.png"), big_png(9000, 8001)).unwrap();
    images.attach(&doc, &mut layout);
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 2000.0);
    assert_eq!(
        layout.image_size(1),
        Some(ImageSize::Refused(Refusal::Pixels(9000, 8001)))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pixels of loads past the limits are kept within their own cap, the least recently
/// painted going first: one evicted is loaded again (once) when it is asked for, never refused.
#[test]
fn loads_past_the_limits_are_evicted_and_loaded_again() {
    use crate::render::Pixels;
    let dir = scratch("forced-evict");
    std::fs::write(dir.join("a.png"), big_png(9000, 8000)).unwrap();
    std::fs::write(dir.join("b.png"), big_png(8000, 9000)).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](a.png) ![b](b.png)");
    images.set_forced_limits(None, 1 << 10);
    assert!(images.force(1, &doc, &mut layout) && images.force(2, &doc, &mut layout));
    pump(&doc, &mut layout, &mut images, &[(1, 90, 80)], |i, _| {
        i.pixels(1, 90, 80).is_some()
    });
    pump(&doc, &mut layout, &mut images, &[(2, 80, 90)], |i, _| {
        i.pixels(2, 80, 90).is_some()
    });
    images.begin_frame();
    assert!(images.pixels(1, 90, 80).is_none(), "evicted for the other");
    let loads = images.forced_loads();
    pump(&doc, &mut layout, &mut images, &[(1, 90, 80)], |i, _| {
        i.pixels(1, 90, 80).is_some()
    });
    assert_eq!(images.forced_loads(), loads + 1);
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(9000, 8000)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A remote image over the limits shows why once remote images are on; a click fetches it again
/// (with the forced fetch) for its size, and then draws it at its box.
#[cfg(feature = "remote-images")]
#[test]
fn a_clicked_remote_image_is_fetched_and_drawn() {
    use crate::render::Pixels;
    let url = "https://x.y/big.png";
    let doc = Doc::parse(&format!("![big]({url})"));
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    let body = big_png(9000, 8000);
    images.set_test_body(Arc::new(move |_| Some(body.clone())));
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 2000.0);
    assert!(!images.force(1, &doc, &mut layout), "remote images are off");
    images.set_remote(true, &doc, &mut layout);
    pump(&doc, &mut layout, &mut images, &[(1, 400, 300)], |_, l| {
        matches!(l.image_size(1), Some(ImageSize::Refused(_)))
    });
    assert_eq!(
        layout.image_size(1),
        Some(ImageSize::Refused(Refusal::Pixels(9000, 8000)))
    );
    assert!(images.force(1, &doc, &mut layout));
    assert_eq!(layout.image_size(1), Some(ImageSize::Loading));
    pump(&doc, &mut layout, &mut images, &[(1, 400, 300)], |_, l| {
        l.image_size(1) == Some(ImageSize::Pixels(9000, 8000))
    });
    pump(&doc, &mut layout, &mut images, &[(1, 90, 80)], |i, _| {
        i.pixels(1, 90, 80).is_some()
    });
    assert_eq!(images.forced_loads(), 2, "fetched, then drawn");
}

/// A body over what forced loads keep is let go once its image is drawn, but its size is not
/// forgotten: the image stays at its size, with no fetch again and no flip back to loading.
#[cfg(feature = "remote-images")]
#[test]
fn a_clicked_remote_image_over_the_kept_cap_is_fetched_once() {
    use crate::render::Pixels;
    let url = "https://x.y/big.png";
    let doc = Doc::parse(&format!("![big]({url})"));
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    let mut images = Images::default();
    images.set_forced_limits(None, 1 << 10);
    let body = big_png(9000, 8000);
    images.set_test_body(Arc::new(move |_| Some(body.clone())));
    images.attach(&doc, &mut layout);
    let fonts = crate::fonts::Fonts::new().unwrap();
    layout.ensure(&doc, &fonts, crate::layout::Anchor::TOP, 2000.0);
    images.set_remote(true, &doc, &mut layout);
    pump(&doc, &mut layout, &mut images, &[(1, 400, 300)], |_, l| {
        matches!(l.image_size(1), Some(ImageSize::Refused(_)))
    });
    assert!(images.force(1, &doc, &mut layout));
    pump(&doc, &mut layout, &mut images, &[(1, 90, 80)], |i, _| {
        i.pixels(1, 90, 80).is_some()
    });
    // Frames on: it stays drawn, and nothing is fetched or decoded again.
    for _ in 0..100 {
        images.begin_frame();
        images.request(&[(1, 90, 80)], 1);
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
        assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(9000, 8000)));
        assert!(images.pixels(1, 90, 80).is_some());
    }
    assert_eq!(images.forced_loads(), 2, "fetched, then drawn");
}

/// An SVG drawn without some of its embedded images says so (its note and click), and is drawn
/// with them once clicked; SVGs drawn are drawn again when the faces of their text change.
#[cfg(feature = "svg")]
#[test]
fn a_partial_svg_is_drawn_whole_when_clicked() {
    use crate::render::Pixels;
    let dir = scratch("forced-partial");
    let png = png_bytes(1, 1, |_, _| [0, 0, 255, 255]);
    let mut b64 = String::new();
    {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for c in png.chunks(3) {
            let n = c
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
            for i in 0..4 {
                b64.push(if i <= c.len() {
                    ABC[(n >> (18 - 6 * i) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
    }
    let rasters: String = (0..65)
        .map(|i| {
            format!(
                "<image x=\"{i}\" width=\"1\" height=\"1\" href=\"data:image/png;base64,{b64}\"/>"
            )
        })
        .collect();
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"65\" height=\"1\">{rasters}</svg>"
    );
    std::fs::write(dir.join("a.svg"), svg).unwrap();
    std::fs::write(dir.join("logo.svg"), fixture("logo-viewbox.svg")).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](a.svg) ![logo](logo.svg)");
    let (lw, lh) = match layout.image_size(2) {
        Some(ImageSize::Pixels(w, h)) => (w, h),
        other => panic!("{other:?}"),
    };
    let boxes = [(1, 65, 1), (2, lw, lh)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        i.pixels(1, 65, 1).is_some() && i.pixels(2, lw, lh).is_some()
    });
    assert!(images.partial(1) && !images.partial(2));
    assert_eq!(
        images.pixels(1, 65, 1).unwrap().data[64],
        0,
        "the 65th left out"
    );
    // New faces: the SVGs drawn are gone from the cache, and drawn again.
    let fonts = crate::fonts::Fonts::new().unwrap();
    images.set_svg_fonts(fonts.svg_sources());
    let empty = crate::fonts::SvgFonts {
        sources: Vec::new(),
        origins: Vec::new(),
        sans: 0,
        mono: 0,
    };
    images.set_svg_fonts(empty);
    assert!(images.pixels(2, lw, lh).is_none(), "drawn again");
    assert!(images.force(1, &doc, &mut layout));
    assert!(!images.partial(1));
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        !i.forcing()
            && i.pixels(1, 65, 1).is_some_and(|b| b.data[64] != 0)
            && i.pixels(2, lw, lh).is_some()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reduced by nearest neighbor, a decode keeps the source pixel under the center of each target
/// pixel (as resvg's `Nearest` drawing does for `image-rendering: optimizeSpeed`, which can't
/// pick them after they were averaged), for every format and way of reading: half the size of an
/// image is its pixels at odd places, as stored; and halved by area it is their mean.
#[test]
fn nearest_sampling_keeps_the_pixels_under_the_centers() {
    let stripes = png_bytes(8, 6, |x, y| {
        let v = if x % 2 == 0 { 0 } else { 255 };
        [v, y as u8 * 40, 255 - v, 255]
    });
    let mut files = vec![("png", stripes)];
    for name in [
        "lossy.webp",
        "lossy-alpha.webp",
        "lossless-alpha.webp",
        "anim.gif",
        "exif-1.jpg",
    ] {
        files.push((name, fixture(name)));
    }
    let go = |bytes: &[u8], w, h, sampling| {
        #[cfg(feature = "svg")]
        let fonts = &mut svg::Fonts::default();
        #[cfg(not(feature = "svg"))]
        let fonts = &mut ();
        decode_why(bytes, w, h, sampling, &Limits::normal(), &NO_CANCEL, fonts).ok()
    };
    for (name, bytes) in &files {
        let h = info_of(bytes).unwrap();
        assert!(
            h.w.is_multiple_of(2) && h.h.is_multiple_of(2) && h.orientation == 1,
            "{name}"
        );
        let (w, h) = (h.w, h.h);
        let full = go(bytes, w, h, Sampling::Area).expect(name);
        let half = go(bytes, w / 2, h / 2, Sampling::Nearest).expect(name);
        assert_eq!((half.width, half.height), (w / 2, h / 2), "{name}");
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                let at = |x: u32, y: u32| full.data[(y * w + x) as usize];
                assert_eq!(
                    half.data[(y * (w / 2) + x) as usize],
                    at(2 * x + 1, 2 * y + 1),
                    "{name} {x},{y}"
                );
            }
        }
    }
    // The PNG the other ways: a whole frame (as an interlaced one is read), and by area.
    let png = &files[0].1;
    let whole = decode_png(png, 4, 3, false, Sampling::Nearest, &NO_CANCEL).unwrap();
    let rows = decode_png(png, 4, 3, true, Sampling::Nearest, &NO_CANCEL).unwrap();
    assert_eq!(whole, rows);
    let mean = decode_png(png, 4, 3, true, Sampling::Area, &NO_CANCEL).unwrap();
    assert_eq!(whole.data[0] >> 16 & 0xff, 255);
    assert_eq!(mean.data[0] >> 16 & 0xff, 128);
}

/// Nearest-neighbor spans take no more room than the area ones (the tables are charged for
/// that: [`spans_cost`]), pick the pixel under each target pixel's center, and copy pixels
/// when the image is enlarged.
#[test]
fn nearest_spans_stay_within_their_bound() {
    for src in (1..=40).chain([1000, 16_383]) {
        for dst in (1..=40).chain([1000, 16_383]) {
            let s = Spans::nearest(src, dst);
            assert!(s.weights.len() <= spans_weights(src, dst), "{src} to {dst}");
            assert_eq!(
                s.weights.capacity(),
                spans_weights(src, dst),
                "{src} to {dst}"
            );
            assert_eq!(s.spans.capacity(), dst as usize + 1, "{src} to {dst}");
            assert_eq!(s.len(), dst as usize);
            for t in 0..s.len() {
                let (first, w) = s.get(t);
                assert_eq!(w, [1.0], "{src} to {dst}, pixel {t}");
                let center = (2 * t + 1) * src as usize / (2 * dst as usize);
                assert_eq!(first, center, "{src} to {dst}, pixel {t}");
                assert!(first < src as usize);
            }
        }
    }
}

/// The decode of an image's first frame finds whether it is animated, and its record says so
/// (for the badge and the toolbar from phase 5 on); a still is not, nor anything that isn't
/// decoded. A document's local images' records go with it.
#[test]
fn decodes_find_animated_images() {
    use crate::render::Pixels;
    let dir = scratch("animated");
    for name in ["anim.gif", "anim.webp", "offset.gif", "lossy.webp"] {
        std::fs::write(dir.join(name), fixture(name)).unwrap();
    }
    let md = "![a](anim.gif) ![b](anim.webp) ![c](offset.gif) ![d](lossy.webp) ![e](none.gif)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    let boxes = [(1, 6, 4), (2, 6, 4), (3, 10, 8), (4, 6, 4)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    let anim = |size| {
        Some(AnimInfo {
            size,
            loops: Loops::Forever,
        })
    };
    assert_eq!(images.animated(1), anim((6, 4)));
    assert_eq!(images.animated(2), anim((6, 4)));
    for id in 3..=5 {
        assert_eq!(images.animated(id), None, "image {id}");
    }
    assert!(images.bookkeeping_bytes() >= vec_bytes(&images.animated));
    let doc = Doc::parse("![a](anim.gif)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    images.attach(&doc, &mut layout);
    assert!(images.animated.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Paint image `id` at `w` × `h` as a frame of the window does, and poll: what it showed.
fn paint_one(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    id: u32,
    w: u32,
    h: u32,
) -> Option<Bitmap> {
    use crate::render::Pixels;
    images.begin_frame();
    let shown = images.pixels(id, w, h).map(|b| as_painted(b, w, h));
    images.poll(doc, layout);
    shown
}

/// `b` as the window paints it in a box of `w` × `h`: enlarged by the nearest pixel if it is
/// smaller (an animation's frames at [`anim::frame_size`]), as `Frame::blit_bitmap` does.
fn as_painted(b: &Bitmap, w: u32, h: u32) -> Bitmap {
    if (b.width, b.height) == (w, h) {
        return b.clone();
    }
    let at = |d: u32, s: u32, n: u32| (u64::from(d) * u64::from(s) / u64::from(n)) as usize;
    let data = (0..h)
        .flat_map(|y| (0..w).map(move |x| (y, x)))
        .map(|(y, x)| b.data[at(y, b.height, h) * b.width as usize + at(x, b.width, w)])
        .collect();
    Bitmap {
        width: w,
        height: h,
        data,
    }
}

/// Paint until `done` says so (the anim thread's results come between).
fn paint_until(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    (id, w, h): (u32, u32, u32),
    mut done: impl FnMut(&mut Images) -> bool,
) {
    let t = Instant::now();
    while !done(images) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        paint_one(images, doc, layout, id, w, h);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn anim_slots(images: &Images) -> usize {
    images
        .cache
        .slots
        .iter()
        .filter(|s| matches!(s.data, Data::Anim(_)))
        .count()
}

/// What the cache is charged is what its slots cost, animations' included.
fn charged_right(images: &Images) {
    let sum: usize = images.cache.slots.iter().map(Slot::cost).sum();
    assert_eq!(images.cache.bytes, sum);
}

/// A page of `md` whose images are the fixtures `names`, decoded at their boxes.
fn anim_page(name: &str, md: &str, boxes: &[(u32, u32, u32)]) -> (PathBuf, Doc, Layout, Images) {
    let dir = scratch(name);
    for f in ["anim.gif", "anim.webp", "big-anim.webp", "alpha.png"] {
        std::fs::write(dir.join(f), fixture(f)).unwrap();
    }
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    pump(&doc, &mut layout, &mut images, boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    (dir, doc, layout, images)
}

#[test]
fn with_animations_off_nothing_plays() {
    let (dir, doc, mut layout, mut images) = anim_page(
        "anim-off",
        "![a](anim.gif) ![b](alpha.png)",
        &[(1, 12, 8), (2, 4, 4)],
    );
    assert!(images.animated(1).is_some());
    let still = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
    for _ in 0..5 {
        let shown = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
        assert_eq!(shown.data, still.data);
    }
    assert!(images.anims.idle());
    assert_eq!(images.anim_deadline(), None);
    assert!(
        images
            .advance(Instant::now() + Duration::from_secs(3600))
            .is_empty()
    );
    assert!(!images.anims.player.alive(), "no thread");
    assert_eq!((anim_slots(&images), images.cache.anims), (0, 0));
    assert!(!images.is_playing(1) && !images.is_playing(2));
    // Paused, with the badge; the still is no animation.
    assert!(images.has_animated());
    assert_eq!(images.anim_state(1), Some(AnimState::Paused));
    assert_eq!(images.anim_state(2), None);
    assert!(images.paused(1) && !images.paused(2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_animation_plays_frame_by_frame() {
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-play", "![a](anim.gif)", &[(1, 12, 8)]);
    let still = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
    let t0 = Instant::now();
    images.set_anim_clock(Some(t0));
    images.set_animations(true);
    assert!(images.is_playing(1));
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    assert!(!images.paused(1));
    // Its still shows until its first frame is made, then the next is made and due.
    paint_until(&mut images, &doc, &mut layout, (1, 12, 8), |i| {
        i.anim_deadline().is_some()
    });
    let due = images.anim_deadline().unwrap();
    assert!(due > t0);
    let first = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
    assert_eq!(first.data, still.data, "a 6 × 4 GIF made twice its size");
    assert!(
        images
            .advance(due - anim::COALESCE - Duration::from_millis(1))
            .is_empty()
    );
    assert_eq!(images.advance(due), vec![1]);
    // Not painted yet: nothing is due.
    assert_eq!(images.anim_deadline(), None);
    let second = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
    assert_ne!(second.data, first.data);
    assert_eq!((anim_slots(&images), images.anims.outside.len()), (1, 0));
    charged_right(&images);
    // Paused, it keeps its frame, and nothing is due.
    images.pause(1);
    assert!(!images.is_playing(1));
    assert_eq!(images.anim_state(1), Some(AnimState::Paused));
    paint_one(&mut images, &doc, &mut layout, 1, 12, 8);
    assert_eq!(images.anim_deadline(), None);
    let shown = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
    assert_eq!(shown.data, second.data);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_animation_over_the_limits_plays_when_asked() {
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-over", "![a](big-anim.webp)", &[(1, 1024, 768)]);
    images.set_animations(true);
    paint_until(&mut images, &doc, &mut layout, (1, 1024, 768), |i| {
        i.anim_over(1)
    });
    assert_eq!(images.anim_state(1), Some(AnimState::Over));
    assert!(images.paused(1));
    assert_eq!((anim_slots(&images), images.cache.anims), (0, 0));
    // A click plays it past the limits: outside the cache.
    images.play(1);
    assert!(!images.anim_over(1));
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    paint_until(&mut images, &doc, &mut layout, (1, 1024, 768), |i| {
        i.anims
            .outside
            .first()
            .is_some_and(|p| p.1.front().is_some())
    });
    paint_until(&mut images, &doc, &mut layout, (1, 1024, 768), |i| {
        i.anim_deadline().is_some()
    });
    assert_eq!(anim_slots(&images), 0);
    assert!(images.anims.outside[0].1.cost > anim::ANIM_ONE_MAX);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_step_running_when_its_document_goes_is_let_go() {
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-gone", "![a](anim.webp)", &[(1, 60, 40)]);
    images.set_animations(true);
    paint_until(&mut images, &doc, &mut layout, (1, 60, 40), |i| {
        i.anims_at()
            .first()
            .is_some_and(|&at| i.playing(at).front().is_some())
    });
    // Painted, then polled: its next step is sent, and the document goes at once.
    paint_one(&mut images, &doc, &mut layout, 1, 60, 40);
    let doc = Doc::parse("![a](alpha.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    images.attach(&doc, &mut layout);
    assert_eq!(anim_slots(&images), 0);
    let t = Instant::now();
    while !images.anims.out.is_empty() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert_eq!(images.cache.anims, 0);
    assert!(images.anims.choices.is_empty() && images.anims.outside.is_empty());
    charged_right(&images);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_evicted_animation_starts_again_from_its_first_frame() {
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-evict", "![a](anim.gif)", &[(1, 12, 8)]);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    paint_until(&mut images, &doc, &mut layout, (1, 12, 8), |i| {
        i.anim_deadline().is_some()
    });
    let first = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
    let due = images.anim_deadline().unwrap();
    assert_eq!(images.advance(due), vec![1]);
    // A frame that doesn't paint it: it is evicted like any slot (no step runs: its frame shown
    // wasn't painted).
    images.begin_frame();
    images.cache.evict_for(images.cache.cap);
    assert_eq!(anim_slots(&images), 0);
    charged_right(&images);
    paint_until(&mut images, &doc, &mut layout, (1, 12, 8), |i| {
        anim_slots(i) == 1
    });
    paint_until(&mut images, &doc, &mut layout, (1, 12, 8), |i| {
        i.anim_deadline().is_some()
    });
    let again = paint_one(&mut images, &doc, &mut layout, 1, 12, 8).unwrap();
    assert_eq!(again.data, first.data);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_streamed_animation_makes_its_next_frame_at_a_new_box() {
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-box", "![a](anim.webp)", &[(1, 800, 600)]);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    paint_until(&mut images, &doc, &mut layout, (1, 800, 600), |i| {
        i.anim_deadline().is_some()
    });
    // (Its frames would be more than are kept: they are streamed.)
    let charge = images.anims_held();
    assert!(
        charge <= anim::ANIM_ONE_MAX && charge > 2 * 800 * 600 * 4,
        "{charge}"
    );
    // Painted at a new box: the frame shown is the old one (scaled when painted) until the
    // next is made at the new box.
    let front = |i: &mut Images| {
        let at = i.anim_at(&i.key(1).unwrap()).unwrap();
        let b = i.playing(at).front().unwrap();
        (b.width, b.height)
    };
    paint_one(&mut images, &doc, &mut layout, 1, 820, 616).unwrap();
    assert_eq!(front(&mut images), (800, 600));
    paint_until(&mut images, &doc, &mut layout, (1, 820, 616), |i| {
        i.anim_deadline().is_some()
    });
    let due = images.anim_deadline().unwrap();
    assert_eq!(images.advance(due), vec![1]);
    paint_one(&mut images, &doc, &mut layout, 1, 820, 616).unwrap();
    assert_eq!(front(&mut images), (820, 616));
    charged_right(&images);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Paint the animations of `boxes` (see [`paint_placed`]) and show their frames as they are due,
/// the clock going with them, until `done` says so.
fn play_until(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32, i32)],
    mut done: impl FnMut(&mut Images) -> bool,
) {
    let mut now = Instant::now();
    images.set_anim_clock(Some(now));
    paint_placed_until(images, doc, layout, boxes, |i| {
        if done(i) {
            return true;
        }
        if let Some(due) = i.anim_deadline() {
            now = now.max(due);
            i.set_anim_clock(Some(now));
            i.advance(now);
        }
        false
    });
}

/// A GIF of `w` × `h` and `n` frames (forever) in which every pixel changes each frame, as in a
/// screen recording, and which compresses as one does.
fn demo_gif(w: u16, h: u16, n: usize) -> Vec<u8> {
    let palette: Vec<u8> = (0..64u8)
        .flat_map(|i| [i * 4, 255 - i * 4, i * 2])
        .collect();
    let mut out = Vec::new();
    {
        let mut e = ::gif::Encoder::new(&mut out, w, h, &palette).unwrap();
        e.set_repeat(::gif::Repeat::Infinite).unwrap();
        for i in 0..n {
            let buffer = (0..usize::from(h))
                .flat_map(|y| (0..usize::from(w)).map(move |x| ((x / 8 + y / 8 + i) % 64) as u8))
                .collect();
            let frame = ::gif::Frame {
                width: w,
                height: h,
                delay: 10,
                buffer: std::borrow::Cow::Owned(buffer),
                ..::gif::Frame::default()
            };
            e.write_frame(&frame).unwrap();
        }
    }
    out
}

/// The README demo GIF sizes play by themselves on a HiDPI screen, within one animation's limit:
/// 640 × 360 at 2× and 3× (frames made at the GIF's size, enlarged by the paint), and 800 × 600 at
/// 2× in the line's width (1440 × 1080, a fraction: one bitmap of the box, made again as each frame
/// is shown) or wider (1600 × 1200), each of them beside the other playing ones.
#[test]
fn demo_sized_animations_play_by_themselves_on_a_hidpi_screen() {
    let dir = scratch("anim-hidpi");
    std::fs::write(dir.join("demo.gif"), demo_gif(640, 360, 10)).unwrap();
    std::fs::write(dir.join("big.gif"), demo_gif(800, 600, 10)).unwrap();
    for (file, (w, h), made) in [
        ("demo.gif", (1280, 720), (640, 360)),
        ("demo.gif", (1920, 1080), (640, 360)),
        ("big.gif", (1440, 1080), (1440, 1080)),
        ("big.gif", (1600, 1200), (800, 600)),
    ] {
        let (doc, mut layout, mut images) = forced_page(&dir, &format!("![a]({file})"));
        let boxes = [(1, w, h)];
        pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
            use crate::render::Pixels;
            i.pixels(1, w, h).is_some()
        });
        let still = paint_one(&mut images, &doc, &mut layout, 1, w, h).unwrap();
        images.set_animations(true);
        let placed = [(1, w, h, 0)];
        let mut frames = Vec::new();
        play_until(&mut images, &doc, &mut layout, &placed, |i| {
            if anim_shows_frame(i, 1) {
                let at = i.anim_at(&i.key(1).unwrap()).unwrap();
                let front = as_painted(i.playing(at).front().unwrap(), w, h).data;
                if frames.last() != Some(&front) {
                    frames.push(front);
                }
            }
            frames.len() >= 4
        });
        assert_eq!(
            images.anim_state(1),
            Some(AnimState::Playing),
            "{file} at {w} × {h}"
        );
        assert!(!images.paused(1));
        let held = images.anims_held();
        assert!(held <= anim::ANIM_ONE_MAX, "{file} at {w} × {h}: {held}");
        // Its first frame is its still, a level apart at most (made on the canvas, then resampled).
        for (a, b) in frames[0].iter().zip(&still.data) {
            for s in [0, 8, 16, 24] {
                assert!(
                    ((a >> s) & 0xFF).abs_diff((b >> s) & 0xFF) <= 1,
                    "{file} at {w} × {h}"
                );
            }
        }
        assert!(frames.windows(2).all(|f| f[0] != f[1]));
        let at = images.anim_at(&images.key(1).unwrap()).unwrap();
        let front = images.playing(at).front().unwrap();
        assert_eq!((front.width, front.height), made, "{file} at {w} × {h}");
        charged_right(&images);
        eprintln!("{file} at {w} × {h}: {held}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An animation that has played its loops shows its badge again, and plays again from its first
/// frame on a click, or when the window's animations go off and on: one whose frames are kept
/// at once, one streamed read again.
#[test]
fn an_animation_played_to_its_end_plays_again_when_asked() {
    use crate::render::Pixels;
    let dir = scratch("anim-end");
    // `anim.gif` without its loop extension, and `anim.webp` with a loop count of 1: each plays
    // once.
    let gif = fixture("anim.gif");
    let at = gif.windows(11).position(|w| w == b"NETSCAPE2.0").unwrap() - 3;
    std::fs::write(dir.join("once.gif"), [&gif[..at], &gif[at + 19..]].concat()).unwrap();
    let mut webp = fixture("anim.webp");
    let at = webp.windows(4).position(|w| w == b"ANIM").unwrap() + 12;
    webp[at..at + 2].copy_from_slice(&1u16.to_le_bytes());
    std::fs::write(dir.join("once.webp"), webp).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](once.gif) ![b](once.webp)");
    let boxes = [(1, 12, 8), (2, 800, 600)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    for id in [1, 2] {
        assert_eq!(images.animated(id).unwrap().loops, Loops::Plays(1));
    }
    let still = images.pixels(1, 12, 8).unwrap().clone();
    let placed = [(1, 12, 8, 0), (2, 800, 600, 8)];
    let ended = |i: &mut Images| {
        [1, 2].iter().all(|&id| {
            let key = i.key(id).unwrap();
            i.anim_at(&key).is_some() && i.anim_state(id) == Some(AnimState::Paused)
        })
    };
    images.set_animations(true);
    for again in 0..2 {
        play_until(&mut images, &doc, &mut layout, &placed, ended);
        assert!(images.paused(1) && images.paused(2), "the badge shows");
        assert!(images.is_playing(1) && !images.animating());
        assert_eq!(images.anim_deadline(), None);
        let key = images.key(1).unwrap();
        let at = images.anim_at(&key).unwrap();
        assert!(images.playing(at).is_kept());
        let front = |i: &mut Images, at| as_painted(i.playing(at).front().unwrap(), 12, 8).data;
        assert_ne!(front(&mut images, at), still.data);
        if again == 0 {
            images.play(1);
            images.play(2);
        } else {
            images.set_animations(false);
            images.set_animations(true);
        }
        for id in [1, 2] {
            assert_eq!(
                images.anim_state(id),
                Some(AnimState::Playing),
                "image {id}"
            );
        }
        // The kept one shows its first frame at once; the streamed one is read again.
        let at = images.anim_at(&key).unwrap();
        assert_eq!(front(&mut images, at), still.data);
        assert!(images.anim_at(&images.key(2).unwrap()).is_none());
        play_until(&mut images, &doc, &mut layout, &placed, |i| {
            i.anim_deadline().is_some() && anim_shows_frame(i, 2)
        });
        charged_right(&images);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_image_loaded_past_the_limits_is_found_animated_and_plays_outside_the_cache() {
    let dir = scratch("anim-forced");
    std::fs::write(dir.join("anim.gif"), fixture("anim.gif")).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](anim.gif)");
    assert!(images.force(1, &doc, &mut layout));
    pump(&doc, &mut layout, &mut images, &[(1, 12, 8)], |i, _| {
        use crate::render::Pixels;
        i.pixels(1, 12, 8).is_some() && !i.forcing()
    });
    assert!(images.animated(1).is_some(), "found by the forced load");
    images.set_anim_clock(Some(Instant::now()));
    images.play(1);
    paint_until(&mut images, &doc, &mut layout, (1, 12, 8), |i| {
        i.anim_deadline().is_some()
    });
    assert_eq!((anim_slots(&images), images.anims.outside.len()), (0, 1));
    let due = images.anim_deadline().unwrap();
    assert_eq!(images.advance(due), vec![1]);
    // Not painted for two frames, it goes.
    images.begin_frame();
    images.begin_frame();
    images.poll(&doc, &mut layout);
    assert!(images.anims.outside.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Paint the boxes `(id, w, h, top)` as a frame of the window does (each placed, then its pixels
/// asked for; the steps of what it showed sent), and poll.
fn paint_placed(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32, i32)],
) {
    use crate::render::Pixels;
    images.begin_frame();
    for &(id, w, h, top) in boxes {
        images.placed(id, (0, top, w as i32, top + h as i32));
        let _ = images.pixels(id, w, h);
    }
    images.painted();
    images.poll(doc, layout);
}

/// [`paint_placed`] until `done` says so.
fn paint_placed_until(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32, i32)],
    mut done: impl FnMut(&mut Images) -> bool,
) {
    let t = Instant::now();
    while !done(images) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        paint_placed(images, doc, layout, boxes);
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Whether image `id`'s animation shows a frame of its own.
fn anim_shows_frame(images: &mut Images, id: u32) -> bool {
    let Some(key) = images.key(id) else {
        return false;
    };
    images
        .anim_at(&key)
        .is_some_and(|at| images.playing(at).front().is_some())
}

/// Two animations whose frames are due together are shown by one tick, and no tick comes
/// sooner than [`anim::CAP`] after the one before; nothing is due for an animation that wasn't
/// painted in the last frame, or is paused.
#[test]
fn due_frames_share_a_tick_and_ticks_are_capped() {
    let boxes = [(1, 12, 8), (2, 12, 8)];
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-tick", "![a](anim.gif) ![b](anim.gif)", &boxes);
    let t0 = Instant::now();
    images.set_anim_clock(Some(t0));
    images.set_animations(true);
    let placed = [(1, 12, 8, 0), (2, 12, 8, 20)];
    // (The deadline is the soonest due: both next frames are made before it is theirs.)
    let next_made = |i: &mut Images, id| {
        let key = i.key(id);
        key.and_then(|k| i.anim_at(&k))
            .is_some_and(|at| i.playing(at).due().is_some())
    };
    paint_placed_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1) && anim_shows_frame(i, 2) && next_made(i, 1) && next_made(i, 2)
    });
    let due = images.anim_deadline().unwrap();
    let mut shown = images.advance(due);
    shown.sort_unstable();
    assert_eq!(shown, vec![1, 2], "one tick for both");
    // Painted, their next frames are made; due at once, they still wait for the cap.
    paint_placed_until(&mut images, &doc, &mut layout, &placed, |i| {
        i.anim_deadline().is_some()
    });
    images.set_anim_clock(Some(due));
    let next = images.anim_deadline().unwrap();
    assert!(next >= due + anim::CAP, "{:?}", next - due);
    assert!(images.advance(due + anim::CAP / 2).is_empty());
    // Not painted in the last frame (off screen): nothing is due, and nothing is shown.
    images.begin_frame();
    assert_eq!(images.anim_deadline(), None);
    assert!(!images.animating());
    assert!(images.advance(next + Duration::from_secs(9)).is_empty());
    // Paused: nothing is due either.
    paint_placed(&mut images, &doc, &mut layout, &placed);
    assert!(images.animating());
    images.pause(1);
    images.pause(2);
    paint_placed(&mut images, &doc, &mut layout, &placed);
    assert_eq!(images.anim_deadline(), None);
    assert!(!images.animating());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Animations that can't all play within [`anim::ANIM_ALL_MAX`] play on screen first, then
/// from the top down: one above those playing takes the place of the lowest, which waits
/// (over the limits, a click plays it), and plays once the ones above it are off screen.
#[test]
fn animations_on_screen_play_first_from_the_top_down() {
    let size = (800, 600);
    let boxes: Vec<_> = (1..=4).map(|id| (id, size.0, size.1)).collect();
    let md = "![a](anim.webp) ![b](anim.webp) ![c](anim.webp) ![d](anim.webp)";
    let (dir, doc, mut layout, mut images) = anim_page("anim-order", md, &boxes);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = |id: u32| (id, size.0, size.1, (id as i32 - 1) * 610);
    // The lower three first: they fit.
    let lower: Vec<_> = (2..=4).map(at).collect();
    paint_placed_until(&mut images, &doc, &mut layout, &lower, |i| {
        (2..=4).all(|id| anim_shows_frame(i, id))
    });
    let one = images.anims_held() / 3;
    assert!(
        3 * one <= anim::ANIM_ALL_MAX && 4 * one > anim::ANIM_ALL_MAX,
        "{one}"
    );
    // The top one comes into view: it plays, and the lowest waits.
    let all: Vec<_> = (1..=4).map(at).collect();
    paint_placed_until(&mut images, &doc, &mut layout, &all, |i| {
        anim_shows_frame(i, 1) && i.anim_over(4)
    });
    assert!((1..=3).all(|id| anim_shows_frame(&mut images, id)));
    assert!(!anim_shows_frame(&mut images, 4));
    // It waits (it isn't over the limits for one).
    assert_eq!(images.anim_state(4), Some(AnimState::Waiting));
    assert!(images.paused(4));
    assert!(images.anims_held() <= anim::ANIM_ALL_MAX);
    charged_right(&images);
    // It isn't read again while it wouldn't fit.
    let serial = images.anims.serial;
    for _ in 0..5 {
        paint_placed(&mut images, &doc, &mut layout, &all);
    }
    assert_eq!(images.anims.serial, serial);
    // Scrolled down to it, the others off screen: it plays.
    paint_placed_until(&mut images, &doc, &mut layout, &[at(4)], |i| {
        anim_shows_frame(i, 4)
    });
    assert!(!images.anim_over(4));
    assert_eq!(images.anim_state(4), Some(AnimState::Playing));
    charged_right(&images);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An animation that waits because the cache has no room for it beside the pixels painted in
/// this frame (not because of [`anim::ANIM_ALL_MAX`]) isn't read again, nor are the animations
/// playing let go for it, while the room isn't there.
#[test]
fn an_animation_the_cache_has_no_room_for_is_not_read_again() {
    let size = (800, 600);
    let boxes = [(1, size.0, size.1), (2, size.0, size.1)];
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-cache-room", "![a](anim.webp) ![b](anim.webp)", &boxes);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = |id: u32| (id, size.0, size.1, (id as i32 - 1) * 610);
    paint_placed_until(&mut images, &doc, &mut layout, &[at(1)], |i| {
        anim_shows_frame(i, 1)
    });
    let one = images.anims_held();
    assert!(2 * one <= anim::ANIM_ALL_MAX, "{one}");
    // Room for the first, not for a second beside it.
    images.cache.cap = images.cache.bytes + one / 2;
    let both = [at(1), at(2)];
    // The poll that finds it waits asks for the paint that shows its badge.
    let painted = |images: &Images| images.anims.placed.iter().find(|p| p.0 == 2).unwrap().2;
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < Duration::from_secs(5), "it doesn't wait");
        place_only(&mut images, &both);
        images.painted();
        let before = painted(&images);
        let repaint = images.poll(&doc, &mut layout);
        if painted(&images) != before && painted(&images) == Some(AnimState::Waiting) {
            assert!(repaint, "its badge isn't asked to be painted");
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let serial = images.anims.serial;
    for _ in 0..5 {
        paint_placed(&mut images, &doc, &mut layout, &both);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(images.anims.serial, serial, "read again");
    assert!(anim_shows_frame(&mut images, 1), "the first plays on");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The page of [`big_above_playing`]: its directory, document, layout and images, the boxes
/// painted, and the serial of the last read.
type BigAbove = (PathBuf, Doc, Layout, Images, [(u32, u32, u32, i32); 2], u64);

/// A large animation (id 1) above one that plays (id 2), with the cache too full of what is
/// painted for it even without the one below, painted 50 times.
fn big_above_playing(name: &str) -> BigAbove {
    let size = (800, 600);
    let boxes = [(1, size.0, size.1), (2, size.0, size.1)];
    let dir = scratch(name);
    let mut big = fixture("anim.gif");
    big.resize(14 << 20, 0);
    std::fs::write(dir.join("big.gif"), &big).unwrap();
    std::fs::write(dir.join("anim.webp"), fixture("anim.webp")).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](big.gif) ![b](anim.webp)");
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = |id: u32| (id, size.0, size.1, (id as i32 - 1) * 610);
    let below = [at(2)];
    paint_placed_until(&mut images, &doc, &mut layout, &below, |i| {
        anim_shows_frame(i, 2)
    });
    let one = images.anims_held();
    let cost = std::fs::metadata(dir.join("big.gif")).unwrap().len() as usize
        + dir.join("big.gif").as_os_str().len()
        + STR_OVERHEAD
        + anim::MEASURE_FIXED;
    assert!(
        one + cost > anim::ANIM_ALL_MAX && cost < anim::ANIM_ALL_MAX,
        "{one} {cost}"
    );
    // Not room for its file beside what is painted, whether the one below stays or goes.
    let both = [at(1), at(2)];
    images.begin_frame();
    for &(id, w, h, top) in &both {
        images.placed(id, (0, top, w as i32, top + h as i32));
        let _ = images.pixels(id, w, h);
    }
    images.cache.cap =
        (images.cache.stuck() + images.cache.held() + cost - one).saturating_sub(1 << 20);
    let serial = images.anims.serial;
    for _ in 0..50 {
        paint_placed(&mut images, &doc, &mut layout, &both);
        std::thread::sleep(Duration::from_millis(5));
    }
    (dir, doc, layout, images, both, serial)
}

/// A large animation above one that plays, with the cache too full of what is painted for it
/// even without the one below: the one below isn't let go for it (and read again, and let go
/// again, with no end).
#[test]
fn an_animation_that_wont_fit_even_so_does_not_let_the_one_below_go() {
    let (dir, _doc, _layout, mut images, _both, serial) = big_above_playing("anim-evict-fail");
    assert_eq!(images.anims.serial, serial, "the one below was read again");
    assert!(anim_shows_frame(&mut images, 2), "the one below plays on");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The one that wouldn't fit shows as waiting, with its badge, and a click plays it past the
/// limits.
#[test]
fn an_animation_that_wont_fit_waits_and_plays_when_asked() {
    let (dir, doc, mut layout, mut images, both, serial) = big_above_playing("anim-evict-wait");
    assert_eq!(images.anim_state(1), Some(AnimState::Waiting));
    assert!(images.paused(1));
    assert!(images.anim_over(1));
    assert_eq!(images.anims.serial, serial);
    images.play(1);
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    paint_placed_until(&mut images, &doc, &mut layout, &both, |i| {
        i.anims
            .outside
            .first()
            .is_some_and(|p| p.1.front().is_some())
    });
    assert!(!images.paused(1));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A local animation whose file alone is over what all animations may hold shows as over the
/// limits, with its badge, even alone in the document, and a click plays it past them.
#[test]
fn a_file_over_the_limits_for_all_animations_plays_when_asked() {
    let size = (800, 600);
    let boxes = [(1, size.0, size.1)];
    let dir = scratch("anim-big-file");
    let mut big = fixture("anim.gif");
    big.resize(20 << 20, 0);
    std::fs::write(dir.join("big.gif"), &big).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](big.gif)");
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        i.pixels(1, size.0, size.1).is_some()
    });
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = [(1, size.0, size.1, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| i.anim_over(1));
    assert_eq!(images.anim_state(1), Some(AnimState::Over));
    assert!(images.paused(1));
    assert_eq!((anim_slots(&images), images.cache.anims), (0, 0));
    images.play(1);
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| {
        i.anims
            .outside
            .first()
            .is_some_and(|p| p.1.front().is_some())
    });
    assert!(!images.paused(1));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The boxes painted (and the pixels asked for) as in a frame, without its `painted` or poll.
fn place_only(images: &mut Images, boxes: &[(u32, u32, u32, i32)]) {
    use crate::render::Pixels;
    images.begin_frame();
    for &(id, w, h, top) in boxes {
        images.placed(id, (0, top, w as i32, top + h as i32));
        let _ = images.pixels(id, w, h);
    }
}

/// What `start_anim` charges for the animation of `file` in `dir` before it is read.
fn read_charge(dir: &Path, file: &str) -> usize {
    let path = dir.join(file);
    std::fs::metadata(&path).unwrap().len() as usize
        + path.as_os_str().len()
        + STR_OVERHEAD
        + anim::MEASURE_FIXED
}

/// An animation that waits with no room for what reading it holds, then gets room for that but
/// not for what it holds once measured, is read once, and not again on every paint (what it
/// holds at its box is known).
#[test]
fn an_animation_crowded_at_measure_is_not_read_again_for_room_it_wouldnt_fit() {
    let size = (800, 600);
    let boxes = [(1, size.0, size.1)];
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-crowd-replace", "![a](anim.webp)", &boxes);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = [(1, size.0, size.1, 0)];
    let cost = read_charge(&dir, "anim.webp");
    // No room for even reading it: it waits at that.
    place_only(&mut images, &at);
    anim_room_to(&mut images, cost / 2);
    let key1 = images.key(1).unwrap();
    let serial = images.anims.serial;
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| {
        i.anims.waits(&key1)
    });
    assert_eq!(images.anims.serial, serial, "read with no room");
    // Room for reading it, not for what it holds.
    anim_room_to(&mut images, 2 * cost);
    for _ in 0..30 {
        paint_placed(&mut images, &doc, &mut layout, &at);
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(images.anims.serial - serial <= 1, "read again and again");
    assert_eq!(images.anim_state(1), Some(AnimState::Waiting));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An animation waiting at its measured hold, with room for that but not for what `start_anim`
/// charges to read it (more, for a small file): it keeps waiting, with no repaint asked for
/// again and again (a busy loop while idle), and no read.
#[test]
fn a_waiting_animation_with_room_for_its_hold_only_asks_for_no_repaints() {
    let boxes = [(1, 12, 8)];
    let (dir, doc, mut layout, mut images) = anim_page("anim-wait-quiet", "![a](anim.gif)", &boxes);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = [(1, 12, 8, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| {
        anim_shows_frame(i, 1)
            && i.anim_at(&i.key(1).unwrap())
                .is_some_and(|a| !i.playing(a).running)
    });
    let key = images.key(1).unwrap();
    let held = |s: &Slot| s.key == key && matches!(s.data, Data::Anim(_));
    let hold = images
        .cache
        .slots
        .iter()
        .find(|s| held(s))
        .map(Slot::cost)
        .unwrap();
    let charge = read_charge(&dir, "anim.gif");
    assert!(hold < charge, "{hold} {charge}");
    // Let go, waiting at what it held (as `anim_measured` records it).
    let i = images.cache.slots.iter().position(held).unwrap();
    images.cache.remove(i);
    images.cache.trim();
    measured(&mut images, &key, hold, (12, 8));
    place_only(&mut images, &at);
    let base = images.cache.stuck() + images.cache.held();
    images.cache.cap = base + (hold + charge) / 2;
    images.repaint = false;
    let serial = images.anims.serial;
    let mut asked = 0;
    for _ in 0..50 {
        place_only(&mut images, &at);
        images.painted();
        if images.poll(&doc, &mut layout) {
            asked += 1;
        }
    }
    assert!(asked < 5, "{asked} repaints asked for in 50 paints");
    assert_eq!(images.anims.serial, serial, "read");
    assert_eq!(images.anim_state(1), Some(AnimState::Waiting));
    let _ = std::fs::remove_dir_all(&dir);
}

/// One that waits and then turns over the limits for all animations (its file does) is painted
/// again once, for its note, and then no more.
#[test]
fn a_waiting_animation_that_turns_over_is_painted_again_once() {
    let size = (800, 600);
    let boxes = [(1, size.0, size.1)];
    let dir = scratch("anim-wait-over");
    let mut big = fixture("anim.gif");
    big.resize(20 << 20, 0);
    std::fs::write(dir.join("big.gif"), &big).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](big.gif)");
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        i.pixels(1, size.0, size.1).is_some()
    });
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = [(1, size.0, size.1, 0)];
    // (Waiting, with an entry as if it had been smaller.)
    let key = images.key(1).unwrap();
    measured(&mut images, &key, 1, size);
    place_only(&mut images, &at);
    images.repaint = false;
    images.painted();
    assert_eq!(images.anim_state(1), Some(AnimState::Over));
    assert!(images.repaint, "its note is painted");
    images.repaint = false;
    for _ in 0..5 {
        place_only(&mut images, &at);
        images.painted();
    }
    assert!(!images.repaint, "painted again with nothing changed");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A click on an animation that shows as paused, with an entry of waiting left from before that
/// would fit now, plays it as any (within the limits, not past them).
#[test]
fn a_click_on_a_paused_animation_that_now_fits_plays_it_within_the_limits() {
    let boxes = [(1, 12, 8)];
    let (dir, doc, mut layout, mut images) = anim_page("anim-click-fits", "![a](anim.gif)", &boxes);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = [(1, 12, 8, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| {
        anim_shows_frame(i, 1)
            && i.anim_at(&i.key(1).unwrap())
                .is_some_and(|a| !i.playing(a).running)
    });
    // Let go, with an entry left from when it waited; animations off.
    let key = images.key(1).unwrap();
    let i = images
        .cache
        .slots
        .iter()
        .position(|s| s.key == key && matches!(s.data, Data::Anim(_)))
        .unwrap();
    images.cache.remove(i);
    images.cache.trim();
    measured(&mut images, &key, 1, (12, 8));
    images.set_animations(false);
    assert!(images.paused(1));
    images.play(1);
    assert!(
        !images.anims.forced.contains(&key),
        "played past the limits"
    );
    assert!(!images.anims.waits(&key));
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| {
        anim_shows_frame(i, 1)
    });
    assert!(images.anims.outside.is_empty(), "played uncharged");
    assert!(!images.anim_over(1));
    charged_right(&images);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two animations in view (ids 1 and 2, `anim.webp`) with the cache short of room for the
/// second beside the first, which plays: the second waits, and isn't read again. The cache's
/// cap is returned with the page and the boxes painted.
type Crowded = (
    PathBuf,
    Doc,
    Layout,
    Images,
    [(u32, u32, u32, i32); 2],
    usize,
);

fn second_crowded(name: &str) -> Crowded {
    let size = (800, 600);
    let boxes = [(1, size.0, size.1), (2, size.0, size.1)];
    let (dir, doc, mut layout, mut images) =
        anim_page(name, "![a](anim.webp) ![b](anim.webp)", &boxes);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = |id: u32| (id, size.0, size.1, (id as i32 - 1) * 610);
    paint_placed_until(&mut images, &doc, &mut layout, &[at(1)], |i| {
        anim_shows_frame(i, 1)
    });
    let one = images.anims_held();
    let cap = images.cache.cap;
    images.cache.cap = images.cache.bytes + one / 2;
    let both = [at(1), at(2)];
    let second = images.key(2).unwrap();
    paint_placed_until(&mut images, &doc, &mut layout, &both, |i| {
        i.anims.waits(&second)
    });
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    (dir, doc, layout, images, both, cap)
}

/// A waiting animation plays, from the cache, once the room it lacked is back (no click).
#[test]
fn a_waiting_animation_plays_once_the_room_is_back() {
    let (dir, doc, mut layout, mut images, both, cap) = second_crowded("anim-wait-room");
    images.cache.cap = cap;
    paint_placed_until(&mut images, &doc, &mut layout, &both, |i| {
        anim_shows_frame(i, 2)
    });
    let second = images.key(2).unwrap();
    assert!(!images.anims.waits(&second));
    assert!(!images.anims.forced.contains(&second));
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
    assert!(anim_shows_frame(&mut images, 1), "the first plays on");
    assert!(images.anims.outside.is_empty());
    charged_right(&images);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A click on a waiting animation, with the room still short, plays it past the limits, outside
/// the cache (the first plays on).
#[test]
fn a_click_on_a_waiting_animation_plays_it_past_the_room() {
    let (dir, doc, mut layout, mut images, both, _cap) = second_crowded("anim-wait-click");
    assert!(images.anim_over(2));
    images.play(2);
    let second = images.key(2).unwrap();
    assert!(images.anims.forced.contains(&second));
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
    paint_placed_until(&mut images, &doc, &mut layout, &both, |i| {
        i.anims
            .outside
            .first()
            .is_some_and(|p| p.1.front().is_some())
    });
    assert!(anim_shows_frame(&mut images, 1), "the first plays on");
    assert_eq!(anim_slots(&images), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The window's animations off show a waiting animation as paused, and on again as waiting:
/// what it waits for isn't forgotten, and it isn't read for nothing meanwhile.
#[test]
fn a_waiting_animation_is_paused_with_animations_off_and_waits_again_when_they_are_on() {
    let (dir, doc, mut layout, mut images, both, _cap) = second_crowded("anim-wait-toggle");
    let second = images.key(2).unwrap();
    images.set_animations(false);
    assert_eq!(images.anim_state(2), Some(AnimState::Paused));
    // (What it holds at its box is known: it isn't read to find out again.)
    assert!(images.anims.costs.iter().any(|c| c.0 == second));
    let serial = images.anims.serial;
    images.set_animations(true);
    for _ in 0..5 {
        paint_placed(&mut images, &doc, &mut layout, &both);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    assert_eq!(images.anims.serial, serial, "read for nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

/// One played past the limits goes when it isn't painted, and is read again (uncharged, still
/// outside the cache) when it is: it stays over the limits, and a click isn't needed again.
#[test]
fn an_animation_played_past_the_limits_is_read_again_when_painted_again() {
    let size = (800, 600);
    let dir = scratch("anim-forced-again");
    let mut big = fixture("anim.gif");
    big.resize(20 << 20, 0);
    std::fs::write(dir.join("big.gif"), &big).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](big.gif)");
    pump(
        &doc,
        &mut layout,
        &mut images,
        &[(1, size.0, size.1)],
        |i, _| {
            use crate::render::Pixels;
            i.pixels(1, size.0, size.1).is_some()
        },
    );
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = [(1, size.0, size.1, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| i.anim_over(1));
    images.play(1);
    let plays = |i: &mut Images| {
        i.anims
            .outside
            .first()
            .is_some_and(|p| p.1.front().is_some())
    };
    paint_placed_until(&mut images, &doc, &mut layout, &at, plays);
    paint_placed_until(&mut images, &doc, &mut layout, &[], |i| {
        i.anims.outside.is_empty()
    });
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    paint_placed_until(&mut images, &doc, &mut layout, &at, plays);
    assert_eq!(images.cache.anims, 0, "charged");
    assert_eq!(anim_slots(&images), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A new document forgets what was waiting, over the limits or chosen for its local images (the
/// ids mean other images now), and keeps what is known of the remote ones (their bodies stay).
#[test]
fn a_new_document_forgets_the_local_waiting_over_and_chosen_but_not_the_remote() {
    let (dir, _doc, _layout, mut images) =
        anim_page("anim-new-doc", "![a](anim.gif)", &[(1, 12, 8)]);
    let local = images.key(1).unwrap();
    let remote = Key::Remote("https://x.y/a.gif".into());
    for key in [&local, &remote] {
        measured(&mut images, key, 1, (12, 8));
        images.anims.forced.push(key.clone());
        images.anims.fetched.push(key.clone());
    }
    images.anims.refused.push((local.clone(), true));
    images.anims.refused.push((remote.clone(), true));
    // A remote one that the new document doesn't have and nothing caches isn't kept.
    #[cfg(feature = "remote-images")]
    {
        let gone = Key::Remote("https://x.y/gone.gif".into());
        images.anims.forced.push(gone.clone());
        images.anims.refused.push((gone, true));
    }
    // (The new document has the remote image: it holds what is known of it.)
    let doc = Doc::parse("![a](https://x.y/a.gif)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    images.attach(&doc, &mut layout);
    assert!(images.anims.costs.iter().all(|c| c.0 == remote) && images.anims.fetched.is_empty());
    assert_eq!(images.anims.forced, vec![remote.clone()]);
    assert_eq!(images.anims.refused, vec![(remote, true)]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An animation whose file can't be read as one when it is asked for (changed since its still
/// was made) isn't one: no badge, no state, and it isn't read again.
#[test]
fn an_animation_that_cant_be_read_isnt_one_and_isnt_read_again() {
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-broken", "![a](anim.gif)", &[(1, 12, 8)]);
    assert!(images.animated(1).is_some());
    std::fs::write(dir.join("anim.gif"), b"GIF89a not really").unwrap();
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = [(1, 12, 8, 0)];
    let key = images.key(1).unwrap();
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| {
        i.anims.refusal(&key) == Some(false)
    });
    assert_eq!(images.anim_state(1), None);
    assert!(!images.paused(1));
    let serial = images.anims.serial;
    for _ in 0..10 {
        paint_placed(&mut images, &doc, &mut layout, &at);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(images.anims.serial, serial, "read again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A click plays an animation over the limits only up to the ceiling for what is loaded past
/// them: one over that is no animation to play (nothing shows, nothing is read again).
#[test]
fn an_animation_over_the_ceiling_for_forced_loads_isnt_played_when_asked() {
    let size = (800, 600);
    let dir = scratch("anim-forced-ceiling");
    let mut big = fixture("anim.gif");
    big.resize(20 << 20, 0);
    std::fs::write(dir.join("big.gif"), &big).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](big.gif)");
    pump(
        &doc,
        &mut layout,
        &mut images,
        &[(1, size.0, size.1)],
        |i, _| {
            use crate::render::Pixels;
            i.pixels(1, size.0, size.1).is_some()
        },
    );
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    images.forced.ceiling = Some(1 << 10);
    let at = [(1, size.0, size.1, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| i.anim_over(1));
    images.play(1);
    let key = images.key(1).unwrap();
    paint_placed_until(&mut images, &doc, &mut layout, &at, |i| {
        i.anims.refusal(&key) == Some(false) && i.anims.out.is_empty()
    });
    let serial = images.anims.serial;
    for _ in 0..10 {
        paint_placed(&mut images, &doc, &mut layout, &at);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(images.anims.serial, serial, "read again and again");
    assert_eq!(images.anim_state(1), None);
    assert!(images.anims.outside.is_empty());
    assert_eq!(images.cache.anims, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The result of reading an animation, back after its document went, is let go: nothing is
/// made of it, held, waiting or refused, and its charge is returned.
#[test]
fn an_animation_read_for_a_document_that_went_is_let_go() {
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-read-gone", "![a](anim.gif)", &[(1, 12, 8)]);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    // Painted: its read is asked for, and the document goes at once.
    paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
    assert_eq!(images.anims.out.len(), 1);
    let doc = Doc::parse("![a](alpha.png)");
    let mut layout = Layout::new(&doc, crate::layout::Params::default());
    images.attach(&doc, &mut layout);
    let t = Instant::now();
    while !images.anims.out.is_empty() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    assert_eq!(anim_slots(&images), 0);
    assert!(images.anims.costs.is_empty() && images.anims.refused.is_empty());
    assert_eq!(images.cache.anims, 0);
    charged_right(&images);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An animation let go while it is in view is painted again at once (its still, and its
/// badge).
#[test]
fn an_animation_let_go_in_view_is_painted_again() {
    let size = (800, 600);
    let boxes = [(1, size.0, size.1), (2, size.0, size.1)];
    let (dir, doc, mut layout, mut images) =
        anim_page("anim-let-go", "![a](anim.webp) ![b](anim.webp)", &boxes);
    images.set_anim_clock(Some(Instant::now()));
    images.set_animations(true);
    let at = |id: u32| (id, size.0, size.1, (id as i32 - 1) * 610);
    let both = [at(1), at(2)];
    paint_placed_until(&mut images, &doc, &mut layout, &both, |i| {
        let idle = |i: &mut Images, id| {
            let at = i.anim_at(&i.key(id).unwrap());
            at.is_some_and(|at| !i.playing(at).running)
        };
        anim_shows_frame(i, 1) && anim_shows_frame(i, 2) && idle(i, 1) && idle(i, 2)
    });
    // Room for one of them only: the one below goes, and is painted again (its still, its
    // badge).
    let one = images.anim_slots_cost() / 2;
    anim_room_to(&mut images, one * 3 / 2);
    let mut asked = false;
    let t = Instant::now();
    while anim_slots(&images) == 2 {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        images.painted();
        asked = images.poll(&doc, &mut layout);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(asked, "the animation let go is painted again");
    assert!(anim_shows_frame(&mut images, 1));
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    let _ = std::fs::remove_dir_all(&dir);
}

/// One animated image used twice, with one copy in view: a tick of it paints the box of the
/// copy in view (not the whole window, as for an animation painted nowhere).
#[cfg(feature = "remote-images")]
#[test]
fn a_tick_of_an_animation_used_twice_paints_the_copy_in_view() {
    let mut images = Images::default();
    anim_remote(&mut images);
    images.set_animations(true);
    let md = "![a](https://x.y/anim.gif) ![b](https://x.y/anim.gif)";
    let (doc, mut layout) = open_and_settle(&mut images, md);
    let t0 = Instant::now();
    images.set_anim_clock(Some(t0));
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        anim_shows_frame(i, 1) && i.anim_deadline().is_some()
    });
    let due = images.anim_deadline().unwrap();
    let mut ids = images.advance(due);
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2], "both are images of it");
    assert_eq!(images.anim_placed(2), None, "one isn't in view");
    assert_eq!(images.anim_boxes(&ids), Some(vec![(0, 0, 12, 8)]));
    // Painted in none of its boxes, nothing is known of what it changed.
    images.begin_frame();
    assert_eq!(images.anim_boxes(&ids), None);
}

/// A fetcher of `anim.gif` that counts its fetches.
#[cfg(feature = "remote-images")]
fn anim_remote(images: &mut Images) -> Arc<std::sync::atomic::AtomicUsize> {
    let gif = Arc::new(fixture("anim.gif"));
    let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = fetches.clone();
    images.remote.set_fetcher(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        Ok((Arc::new(gif.to_vec()), 6, 4))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    fetches
}

/// Paint image 1 of a remote page at 12 × 8 (and ask for it) until `done`.
#[cfg(feature = "remote-images")]
fn paint_remote_until(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    mut done: impl FnMut(&mut Images) -> bool,
) {
    let t = Instant::now();
    while !done(images) {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        paint_placed(images, doc, layout, &[(1, 12, 8, 0)]);
        images.request(&[(1, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A remote animation that waits (its body is let go meanwhile, as the still is painted every
/// frame and the body isn't) is fetched again once room comes back, and a click plays it too.
#[cfg(feature = "remote-images")]
fn remote_body_gone(crowd: bool, click: bool) -> (bool, usize) {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    let key = images.key(1).unwrap();
    if crowd {
        // (It didn't fit beside others, now gone: it would fit.)
        measured(&mut images, &key, 1, (12, 8));
    }
    images.cache.discard_bytes(&url);
    images.set_animations(true);
    if click {
        images.play(1);
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
        images.request(&[(1, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
        if anim_shows_frame(&mut images, 1) {
            return (true, fetches.load(Ordering::Relaxed));
        }
    }
    (false, fetches.load(Ordering::Relaxed))
}

#[cfg(feature = "remote-images")]
#[test]
fn a_waiting_remote_animation_whose_body_went_is_fetched_again_and_plays() {
    assert_eq!(remote_body_gone(false, false), (true, 2), "control");
    assert_eq!(
        remote_body_gone(true, false),
        (true, 2),
        "waiting, fits now"
    );
    assert_eq!(remote_body_gone(true, true), (true, 2), "clicked");
}

/// Two remote animations in view, the second waiting for room, its body let go meanwhile and
/// room back: it plays (fetched again), and so does it when clicked.
#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_that_waited_plays_when_room_comes_back() {
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    images.set_animations(true);
    let md = "![a](https://x.y/a.gif) ![b](https://x.y/b.gif)";
    let (doc, mut layout) = open_and_settle(&mut images, md);
    let both = [(1, 12, 8, 0), (2, 12, 8, 20)];
    let t = Instant::now();
    while !anim_shows_frame(&mut images, 1) {
        assert!(t.elapsed() < Duration::from_secs(20));
        paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
        images.request(&[(1, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
    }
    let one = images.anims_held();
    let full_cap = images.cache.cap;
    // Both in view, the cache short of room for a second animation's frames.
    let key2 = images.key(2).unwrap();
    let body2: Arc<str> = "https://x.y/b.gif".into();
    // (Once what it holds is known: the second is read, then the room is cut.)
    let t = Instant::now();
    while !images.anims.costs.iter().any(|c| c.0 == key2) {
        assert!(t.elapsed() < Duration::from_secs(20), "never read");
        paint_placed(&mut images, &doc, &mut layout, &both);
        images.request(&[(1, 12, 8), (2, 12, 8)], 2);
        std::thread::sleep(Duration::from_millis(2));
    }
    anim_room_to(&mut images, one * 3 / 2);
    while !images.anims.waits(&key2) {
        assert!(t.elapsed() < Duration::from_secs(20), "never crowded");
        paint_placed(&mut images, &doc, &mut layout, &both);
        images.request(&[(1, 12, 8), (2, 12, 8)], 2);
        std::thread::sleep(Duration::from_millis(2));
    }
    // Room is made for something else while it waits: its body (the least recently used) goes,
    // its still (painted in this frame) stays.
    images.cache.cap = 0;
    paint_placed(&mut images, &doc, &mut layout, &[(2, 12, 8, 0)]);
    place_only(&mut images, &[(2, 12, 8, 0)]);
    images.cache.evict();
    assert!(
        images.cache.peek_bytes(&body2).is_none(),
        "its body is kept"
    );
    // Room is back, and the first is off screen.
    images.cache.cap = full_cap;
    let mut shows = false;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) && !shows {
        paint_placed(&mut images, &doc, &mut layout, &[(2, 12, 8, 0)]);
        images.request(&[(2, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
        shows = anim_shows_frame(&mut images, 2);
    }
    assert!(shows, "state {:?}", images.anim_state(2));
    assert!(
        fetches.load(Ordering::Relaxed) >= 3,
        "its body was fetched again"
    );
}

/// The body fetched again to play a remote animation comes while it is off screen and is let go
/// before it is in view again: it is fetched again then.
#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_body_that_came_while_off_screen_is_fetched_again_in_view() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    images.cache.discard_bytes(&url);
    images.set_animations(true);
    // It stays in view until a fetcher has taken the body (a queue is replaced by each request),
    // then it goes off screen, and the body comes while nothing is painted.
    paint_remote_until(&mut images, &doc, &mut layout, |_| {
        fetches.load(Ordering::Relaxed) == 2
    });
    let t = Instant::now();
    while images.cache.peek_bytes(&url).is_none() {
        assert!(t.elapsed() < Duration::from_secs(3), "no body came");
        paint_placed(&mut images, &doc, &mut layout, &[]);
        images.request(&[], 0);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
    images.cache.discard_bytes(&url);
    paint_remote_until(&mut images, &doc, &mut layout, |i| anim_shows_frame(i, 1));
    assert_eq!(fetches.load(Ordering::Relaxed), 3);
    assert_ne!(images.anim_state(1), Some(AnimState::Waiting));
}

/// Animations turned on with more remote animations in view than there are fetchers, their bodies
/// gone (they were let go while the animations were off, the default): the bodies queued are
/// kept in the queue, and all play.
#[cfg(feature = "remote-images")]
#[test]
fn more_remote_animations_than_fetchers_all_play_when_turned_on() {
    use crate::render::Pixels;
    let n = 8u32;
    let md: String = (1..=n)
        .map(|i| format!("![a](https://x.y/a{i}.gif)\n\n"))
        .collect();
    let gif = Arc::new(fixture("anim.gif"));
    let mut images = Images::default();
    let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = fetches.clone();
    images.remote.set_fetcher(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(30));
        Ok((Arc::new(gif.to_vec()), 6, 4))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    let (doc, mut layout) = open_and_settle(&mut images, &md);
    let placed: Vec<(u32, u32, u32, i32)> = (1..=n).map(|i| (i, 12, 8, i as i32 * 10)).collect();
    let boxes: Vec<(u32, u32, u32)> = (1..=n).map(|i| (i, 12, 8)).collect();
    let t = Instant::now();
    while !(1..=n).all(|i| images.pixels(i, 12, 8).is_some()) {
        assert!(t.elapsed() < Duration::from_secs(20), "stills never came");
        paint_placed(&mut images, &doc, &mut layout, &placed);
        images.request(&boxes, n as usize);
        std::thread::sleep(Duration::from_millis(2));
    }
    // (A decode still out holds its body: it is let go once that is back.)
    let urls: Vec<Arc<str>> = (1..=n)
        .map(|i| format!("https://x.y/a{i}.gif").into())
        .collect();
    let t = Instant::now();
    loop {
        for url in &urls {
            images.cache.discard_bytes(url);
        }
        if urls.iter().all(|u| images.cache.peek_bytes(u).is_none()) {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "bodies held");
        images.poll(&doc, &mut layout);
        std::thread::sleep(Duration::from_millis(2));
    }
    let base = fetches.load(Ordering::Relaxed);
    images.set_animations(true);
    let t = Instant::now();
    while !(1..=n).all(|i| anim_shows_frame(&mut images, i)) {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "{} of {n} play, states {:?}",
            (1..=n)
                .filter(|&i| anim_shows_frame(&mut images, i))
                .count(),
            (1..=n).map(|i| images.anim_state(i)).collect::<Vec<_>>()
        );
        paint_placed(&mut images, &doc, &mut layout, &placed);
        images.request(&boxes, n as usize);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        fetches.load(Ordering::Relaxed) - base,
        n as usize,
        "each fetched once"
    );
    assert!(images.anims.fetched.is_empty(), "asked for, and not back");
}

/// A remote animation whose body went, with no room in the cache to read it (its index): it waits
/// and its body isn't fetched again while there is no room (it would be dropped), and is, and
/// plays, once there is, or when the reader clicks it.
#[cfg(feature = "remote-images")]
fn remote_body_dropped_for_room(click: bool) {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    images.cache.discard_bytes(&url);
    images.set_animations(true);
    // The cache has room for what it holds and nothing more.
    let full = images.cache.cap;
    images.cache.cap = images.cache.bytes + 64;
    for _ in 0..50 {
        paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
        images.request(&[(1, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fetches.load(Ordering::Relaxed), 1, "fetched with no room");
    assert!(images.cache.peek_bytes(&url).is_none());
    assert!(!anim_shows_frame(&mut images, 1));
    assert_eq!(images.anim_state(1), Some(AnimState::Waiting));
    images.cache.cap = full;
    if click {
        images.play(1);
    }
    paint_remote_until(&mut images, &doc, &mut layout, |i| anim_shows_frame(i, 1));
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
}

#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_body_dropped_for_room_is_fetched_again_once_there_is_room() {
    remote_body_dropped_for_room(false);
}

#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_body_dropped_for_room_is_fetched_again_on_a_click() {
    remote_body_dropped_for_room(true);
}

/// A fetcher that brings the animated GIF, or fails while `offline` is set: the fetches made, and
/// the switch (neither is shared with another test).
#[cfg(feature = "remote-images")]
fn anim_remote_offline(
    images: &mut Images,
) -> (
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let gif = Arc::new(fixture("anim.gif"));
    let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let offline = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (count, down) = (fetches.clone(), offline.clone());
    images.remote.set_fetcher(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        if down.load(Ordering::Relaxed) {
            Err(crate::remote::Error::Failed)
        } else {
            Ok((Arc::new(gif.to_vec()), 6, 4))
        }
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    (fetches, offline)
}

/// The fetch of an animation's body fails (the reader went offline after the page loaded, then
/// turned animations on): the image keeps its still and its size, shows its badge, and isn't
/// fetched again on its own; a click asks again, and plays it once the network is back.
#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_whose_body_cant_be_fetched_keeps_its_still_and_a_click_retries() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let (fetches, offline) = anim_remote_offline(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(6, 4)));
    images.cache.discard_bytes(&url);
    offline.store(true, Ordering::Relaxed);
    images.set_animations(true);
    let kept = |images: &mut Images, layout: &Layout| {
        assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(6, 4)));
        assert_eq!(images.remote.size(&url), Some(Some((6, 4))));
        assert!(images.pixels(1, 12, 8).is_some(), "its still");
    };
    let quiet = |images: &mut Images, layout: &mut Layout, fetches_so_far: usize| {
        for _ in 0..50 {
            paint_placed(images, &doc, layout, &[(1, 12, 8, 0)]);
            images.request(&[(1, 12, 8)], 1);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            fetches.load(Ordering::Relaxed),
            fetches_so_far,
            "no retries"
        );
        kept(images, layout);
        assert_eq!(images.anim_state(1), Some(AnimState::Failed));
        assert!(images.paused(1), "the badge");
    };
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.anim_state(1) == Some(AnimState::Failed)
    });
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
    quiet(&mut images, &mut layout, 2);
    // A click while still offline asks once more, and fails the same way.
    images.play(1);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        fetches.load(Ordering::Relaxed) == 3 && i.anim_state(1) == Some(AnimState::Failed)
    });
    quiet(&mut images, &mut layout, 3);
    offline.store(false, Ordering::Relaxed);
    images.play(1);
    paint_remote_until(&mut images, &doc, &mut layout, |i| anim_shows_frame(i, 1));
    assert_eq!(fetches.load(Ordering::Relaxed), 4);
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    kept(&mut images, &layout);
    assert!(images.anims.failed.is_empty());
}

/// Turning animations on (`A`, or the toolbar's play button) asks again for the ones that failed,
/// once each per press; nothing is asked for in between.
#[cfg(feature = "remote-images")]
#[test]
fn turning_animations_on_retries_the_failed_ones_once_per_press() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let (fetches, offline) = anim_remote_offline(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    images.cache.discard_bytes(&url);
    offline.store(true, Ordering::Relaxed);
    images.set_animations(true);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.anim_state(1) == Some(AnimState::Failed)
    });
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
    // Off and on again while still offline: one more attempt, and Failed again.
    for attempts in [3, 4] {
        images.set_animations(false);
        images.set_animations(true);
        paint_remote_until(&mut images, &doc, &mut layout, |i| {
            fetches.load(Ordering::Relaxed) == attempts
                && i.anim_state(1) == Some(AnimState::Failed)
        });
        for _ in 0..50 {
            paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
            images.request(&[(1, 12, 8)], 1);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(fetches.load(Ordering::Relaxed), attempts, "no storm");
    }
    offline.store(false, Ordering::Relaxed);
    images.set_animations(false);
    images.set_animations(true);
    paint_remote_until(&mut images, &doc, &mut layout, |i| anim_shows_frame(i, 1));
    assert_eq!(fetches.load(Ordering::Relaxed), 5);
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
}

/// A body dropped for room is fetched again only if it would fit beside what decoding holds, as
/// it is judged when it comes: while a decode holds more than the room left, nothing is fetched
/// (it would come, be dropped, and be asked for again), and once the decode is over it is.
#[cfg(feature = "remote-images")]
#[test]
fn a_dropped_animation_body_isnt_fetched_again_while_a_decode_holds_its_room() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    // Its body was dropped for room when it came (as `drop_fetched` notes it).
    images.cache.discard_bytes(&url);
    let body = Arc::new(fixture("anim.gif"));
    let need = (
        Slot::bytes_cost(&url, &body),
        images.decode_estimate(&url, &body),
    );
    images.defer(&url, need);
    images.set_animations(true);
    let full = images.cache.cap;
    // A decode that holds 1 MiB is running; the cap leaves room for the body, and not beside it.
    images.worker = Some(Worker::spawn(images.tx.clone(), images.wake.clone()));
    let shared = images.worker.as_ref().unwrap().shared.clone();
    shared.0.lock().unwrap().busy = Some((Key::Local(99), 10, 10, 1 << 20));
    let (cost, decode) = images.deferred_need(&url).unwrap();
    let fits = |images: &Images, reserve| {
        let fetching = images.fetch_reserve(true);
        images.cache.would_fit(cost, decode, reserve, fetching)
    };
    // The least cap the allocation plays it at and the body fits without the decode, with room
    // for the bookkeeping the request makes (8 KiB): it fits there only without the decode.
    paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
    let key = Key::Remote(url.clone());
    images.cache.cap = images.cache.bytes;
    while !(fits(&images, 0) && images.anims.alloc.play.contains(&key)) {
        images.cache.cap += 256;
        let shown = images.anims_shown();
        images.reallocate(&shown);
    }
    images.cache.cap += 8 << 10;
    for _ in 0..100 {
        paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
        assert!(fits(&images, 0) && !fits(&images, 1 << 20));
        assert!(images.anims.alloc.play.contains(&key));
        images.request(&[(1, 12, 8)], 1);
        // (Nor asked for: the fetchers, held off by the decode here, would fetch it the moment
        // they are let go, to drop it again.)
        assert!(images.anims.fetched.is_empty(), "asked for beside a decode");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        fetches.load(Ordering::Relaxed),
        1,
        "fetched beside a decode"
    );
    // The decode is over, and the room is back.
    Worker::taken(&shared);
    images.cache.cap = full;
    paint_remote_until(&mut images, &doc, &mut layout, |i| anim_shows_frame(i, 1));
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
}

/// A key left in `fetched` (its queued fetch was replaced) while its still is evicted and its
/// body is fetched for the image itself: that body is judged for its decode, as any image's is.
#[cfg(feature = "remote-images")]
#[test]
fn a_body_fetched_for_its_image_is_judged_for_its_decode_whatever_is_left_in_fetched() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    let key = images.key(1).unwrap();
    let bytes = Arc::new(fixture("anim.gif"));
    images.anims.fetched.push(key);
    // The still is cached: the body is read for the animation, and decoded for nothing.
    assert_eq!(images.decode_estimate(&url, &bytes), 0);
    // The still is gone, the body is wanted for the image: its decode counts.
    while let Some(i) =
        (images.cache.slots.iter()).position(|s| matches!(&s.key, Key::Remote(u) if *u == url))
    {
        images.cache.remove(i);
    }
    assert_ne!(images.decode_estimate(&url, &bytes), 0);
}

/// A remote animated image big in its own size (its still is at its box, small) is judged on
/// what reading it holds, not on a decode of the body at that size: over the limits for one
/// animation, it shows so, and a click plays it.
#[cfg(feature = "remote-images")]
#[test]
fn a_large_remote_animation_is_over_the_limits_and_plays_when_clicked() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut gif = fixture("anim.gif");
    let (w, h) = (3000u16, 2000u16);
    gif[6..8].copy_from_slice(&w.to_le_bytes());
    gif[8..10].copy_from_slice(&h.to_le_bytes());
    let gif = Arc::new(gif);
    let mut images = Images::default();
    let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = fetches.clone();
    images.remote.set_fetcher(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        Ok((Arc::new(gif.to_vec()), w as u32, h as u32))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    images.cache.discard_bytes(&url);
    images.set_animations(true);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.anim_state(1) == Some(AnimState::Over)
    });
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
    // (Nothing is asked for again, on the paints that follow.)
    for _ in 0..30 {
        paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
        images.request(&[(1, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
    assert!(!anim_shows_frame(&mut images, 1));
    images.play(1);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.anims
            .outside
            .first()
            .is_some_and(|p| p.1.front().is_some())
            || anim_shows_frame(i, 1)
    });
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
}

/// A remote animation clicked to play and found past the ceiling for those clicked (it can't
/// play) isn't fetched again, each time its body is let go.
#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_that_cant_play_even_when_clicked_isnt_fetched_again() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    let key = images.key(1).unwrap();
    images.anims.refused.push((key.clone(), false));
    images.anims.forced.push(key);
    images.set_animations(true);
    let base = fetches.load(Ordering::Relaxed);
    for _ in 0..5 {
        images.cache.discard_bytes(&url);
        for _ in 0..20 {
            paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
            images.request(&[(1, 12, 8)], 1);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(
        fetches.load(Ordering::Relaxed),
        base,
        "fetched to be refused"
    );
}

/// A remote animation that waits for room it doesn't have, its body let go, doesn't have it
/// fetched again on every paint (nor at all, while it couldn't be played); a click plays it, past
/// the limits, from a body fetched again.
#[cfg(feature = "remote-images")]
#[test]
fn a_waiting_remote_animation_isnt_fetched_again_until_it_would_fit_or_is_clicked() {
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    let key = images.key(1).unwrap();
    // (It waits for more than the animations may hold.)
    measured(&mut images, &key, 1 << 40, (12, 8));
    images.cache.discard_bytes(&url);
    images.set_animations(true);
    for _ in 0..30 {
        paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
        images.request(&[(1, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fetches.load(Ordering::Relaxed), 1, "fetched again to wait");
    assert_eq!(images.anim_state(1), Some(AnimState::Waiting));
    images.play(1);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.anims
            .outside
            .first()
            .is_some_and(|p| p.1.front().is_some())
    });
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
}

/// A fetched animated image plays: its body is kept for it when animations are on, and, when
/// it is no longer cached (it was let go while they were off), it is fetched again to play it,
/// once, as images are fetched: only while remote images are on.
#[cfg(feature = "remote-images")]
#[test]
fn a_fetched_animation_plays() {
    use crate::render::Pixels;
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let md = "![a](https://x.y/anim.gif)";
    // On: the body that was decoded is kept, and played from.
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    images.set_animations(true);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some() && anim_shows_frame(i, 1)
    });
    assert_eq!(fetches.load(Ordering::Relaxed), 1);
    charged_right(&images);

    // Off while it was decoded: its body is let go (as any), and fetched again once they are on.
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let (doc, mut layout) = open_and_settle(&mut images, md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 12, 8).is_some()
    });
    assert!(images.animated(1).is_some());
    images.cache.discard_bytes(&url);
    assert!(images.cache.peek_bytes(&url).is_none());
    // With remote images off for the document, nothing is fetched for it.
    images.set_remote(false, &doc, &mut layout);
    images.set_animations(true);
    for _ in 0..20 {
        paint_placed(&mut images, &doc, &mut layout, &[(1, 12, 8, 0)]);
        images.request(&[(1, 12, 8)], 1);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fetches.load(Ordering::Relaxed), 1);
    assert!(!anim_shows_frame(&mut images, 1));
    images.set_remote(true, &doc, &mut layout);
    paint_remote_until(&mut images, &doc, &mut layout, |i| anim_shows_frame(i, 1));
    assert_eq!(fetches.load(Ordering::Relaxed), 2, "fetched again once");
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.anim_deadline().is_some()
    });
    assert_eq!(fetches.load(Ordering::Relaxed), 2);
    charged_right(&images);
}

#[cfg(feature = "remote-images")]
#[test]
fn a_cached_remote_animation_is_still_one_after_leaving_the_document_and_coming_back() {
    let mut images = Images::default();
    let fetches = anim_remote(&mut images);
    let md = "![a](https://x.y/anim.gif)";
    let (doc, mut layout) = open_and_settle(&mut images, md);
    pump(&doc, &mut layout, &mut images, &[(1, 40, 30)], |i, _| {
        i.pixels(1, 40, 30).is_some()
    });
    assert!(images.animated(1).is_some());
    let decodes = images.decodes;
    let url: Arc<str> = "https://x.y/anim.gif".into();
    open_and_settle(&mut images, "# Another document");
    assert!(
        images.cache.has_url(&url),
        "the first-frame pixels are still cached"
    );
    // (Known of the cached image, but not a reason for the other document's toolbar to show the
    // animations button.)
    assert!(!images.has_animated());
    let (doc, mut layout) = open_and_settle(&mut images, md);
    for _ in 0..20 {
        paint_placed(&mut images, &doc, &mut layout, &[(1, 40, 30, 0)]);
        images.request(&[(1, 40, 30)], 1);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(images.decodes, decodes, "cached still wasn't decoded again");
    assert_eq!(fetches.load(Ordering::Relaxed), 1);
    assert!(images.animated(1).is_some(), "still known to be animated");
    assert!(images.has_animated());
    // It plays from the cached still's place.
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    paint_placed_until(&mut images, &doc, &mut layout, &[(1, 40, 30, 0)], |i| {
        anim_shows_frame(i, 1)
    });
}

#[test]
fn a_waiting_animation_starts_when_a_kept_build_gives_back_room_and_only_the_changed_boxes_are_painted()
 {
    let dir = scratch("kept-room");
    std::fs::write(dir.join("large.gif"), demo_gif(1600, 1200, 3)).unwrap();
    std::fs::write(dir.join("small.gif"), fixture("anim.gif")).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](large.gif)\n\n![b](small.gif)");
    // (The first is a big source in a small box: kept once built. The second doesn't fit beside
    // it while it is built, and does after.)
    let boxes = [(1, 320, 240), (2, 1000, 667)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    let mut now = Instant::now();
    images.set_anim_clock(Some(now));
    let placed = [(1, 320, 240, 0), (2, 1000, 667, 250)];
    let key2 = images.key(2).unwrap();
    paint_placed_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1) && i.anims.waits(&key2)
    });
    paint_placed(&mut images, &doc, &mut layout, &placed);
    // The window of Windows and Wayland: a full paint only when poll says so, else just the
    // boxes `advance` changed, with no new frame begun.
    let t = Instant::now();
    let mut redraws = 0;
    while t.elapsed() < Duration::from_secs(2) && !anim_shows_frame(&mut images, 2) {
        if images.poll(&doc, &mut layout) {
            redraws += 1;
            paint_placed(&mut images, &doc, &mut layout, &placed);
        }
        if let Some(due) = images.anim_deadline() {
            now = now.max(due);
            images.set_anim_clock(Some(now));
            for id in images.advance(now) {
                let (_, w, h, top) = placed.iter().find(|p| p.0 == id).copied().unwrap();
                images.placed(id, (0, top, w as i32, top + h as i32));
                let _ = images.pixels(id, w, h);
            }
            images.painted();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let key1 = images.key(1).unwrap();
    let at = images.anim_at(&key1).unwrap();
    assert!(images.playing(at).is_kept(), "the first build finished");
    assert!(
        anim_shows_frame(&mut images, 2),
        "the waiting image starts when room is given back: {:?}",
        images.anim_state(2)
    );
    // Not asked for again and again meanwhile: a few redraws for the room, not one a poll.
    assert!(redraws < 100, "{redraws} redraws");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_waiting_animation_that_still_doesnt_fit_is_not_asked_for_when_room_is_given_back() {
    let dir = scratch("kept-no-room");
    std::fs::write(dir.join("a.gif"), demo_gif(1600, 1200, 3)).unwrap();
    std::fs::write(dir.join("b.gif"), fixture("anim.gif")).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](a.gif)\n\n![b](b.gif)");
    let boxes = [(1, 320, 240), (2, 1000, 667)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let placed = [(1, 320, 240, 0), (2, 1000, 667, 250)];
    let key2 = images.key(2).unwrap();
    paint_placed_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1) && i.anims.waits(&key2)
    });
    // It needs more than the first will give back: nothing is asked for when the first's build
    // is over, and after.
    let src = images.animated(2).unwrap().size;
    let size = anim::frame_size(src, (1000, 667));
    assert!(images.anims.cost(&key2, size).is_some());
    images
        .anims
        .note_cost(key2.clone(), size, usize::MAX / 2, &[]);
    let key1 = images.key(1).unwrap();
    place_only(&mut images, &placed);
    images.painted();
    let kept = |i: &mut Images| {
        let at = i.anim_at(&key1);
        at.is_some_and(|at| i.playing(at).is_kept())
    };
    paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(10),
        kept,
    );
    assert!(kept(&mut images), "the first build finished");
    let (redraws, _) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_millis(300),
        |_| false,
    );
    assert_eq!(redraws, 0, "an entry that can't fit asks for no redraw");
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `key`'s animation was measured to hold `slot` at the box `at` (what a read records).
fn measured(images: &mut Images, key: &Key, slot: usize, at: (u32, u32)) {
    let src = images
        .animated
        .iter()
        .find(|(k, _)| k == key)
        .map_or(at, |(_, info)| info.size);
    images
        .anims
        .note_cost(key.clone(), anim::frame_size(src, at), slot, &[]);
}

/// Set the cache's cap so that the room the allocation has for animations is `room`.
fn anim_room_to(images: &mut Images, room: usize) {
    images.cache.cap = images.cache.cap - images.anims_room_now() + room;
}

/// What the window of Windows and Wayland does for `time`: a full paint of `boxes` only when poll
/// says so, else just the boxes `advance` changed (placed again, with no new frame begun), when
/// they are due. Stops when `done` says so. The full paints asked for and the frames
/// shown.
fn paint_partially(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32, i32)],
    time: Duration,
    mut done: impl FnMut(&mut Images) -> bool,
) -> (usize, usize) {
    use crate::render::Pixels;
    let start = Instant::now();
    let (mut redraws, mut swaps) = (0, 0);
    while start.elapsed() < time && !done(images) {
        if images.poll(doc, layout) {
            redraws += 1;
            paint_placed(images, doc, layout, boxes);
        }
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.anim_deadline().is_some_and(|due| due <= now) {
            for id in images.advance(now) {
                swaps += 1;
                let (_, w, h, top) = boxes.iter().find(|b| b.0 == id).copied().unwrap();
                images.placed(id, (0, top, w as i32, top + h as i32));
                let _ = images.pixels(id, w, h);
            }
            images.painted();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    (redraws, swaps)
}

/// The animation above waits while the one below it runs a step, which can't be let go
/// meanwhile; the full paint for its waiting is spent then. When that step is over the one below
/// is let go for it, though no bytes come back: the poll that takes the step asks for the paint
/// that starts it, and nothing else does (the paints after are of the changed boxes).
#[test]
fn a_waiting_animation_starts_when_the_one_below_finishes_a_step_without_giving_bytes_back() {
    let dir = scratch("wait-below");
    let gif = demo_gif(800, 600, 10);
    std::fs::write(dir.join("a.gif"), &gif).unwrap();
    std::fs::write(dir.join("b.gif"), &gif).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![above](a.gif)\n\n![below](b.gif)");
    // (Each is one bitmap of 1440 × 1080: two don't fit beside each other.)
    let boxes = [(1, 1440, 1080), (2, 1440, 1080)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let (key1, key2) = (images.key(1).unwrap(), images.key(2).unwrap());
    let below = [(2, 1440, 1080, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &below, |i| {
        anim_shows_frame(i, 2)
    });
    // The anim thread's first wake, after the above one is measured, waits for the test: the
    // step of the one below is queued behind it, and runs when the wake is let go.
    let hold = Arc::new(AtomicBool::new(true));
    let entered = Arc::new(AtomicBool::new(false));
    let first = Arc::new(AtomicBool::new(true));
    let (h, e, f) = (hold.clone(), entered.clone(), first);
    images.wake = Arc::new(move || {
        if f.swap(false, Ordering::SeqCst) {
            e.store(true, Ordering::SeqCst);
            while h.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let release = Release(hold);
    let placed = [(1, 1440, 1080, 0), (2, 1440, 1080, 1100)];
    paint_placed(&mut images, &doc, &mut layout, &placed);
    let t = Instant::now();
    while !entered.load(Ordering::SeqCst) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
    let running = |i: &mut Images| {
        let at = i.anim_at(&key2);
        at.is_some_and(|at| i.playing(at).running)
    };
    assert!(running(&mut images));
    images.poll(&doc, &mut layout);
    // Measured, the one above plays and the one below doesn't (they don't fit beside each other):
    // it waits, while its step still runs.
    assert!(
        images.anims.alloc.play.contains(&key1),
        "the one above plays"
    );
    paint_placed(&mut images, &doc, &mut layout, &placed);
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    drop(release);
    let mut redraw = false;
    let t = Instant::now();
    while running(&mut images) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        redraw |= images.poll(&doc, &mut layout);
        std::thread::sleep(Duration::from_millis(1));
    }
    redraw |= images.poll(&doc, &mut layout);
    assert!(redraw, "the poll that took the step asks for a paint");
    paint_placed(&mut images, &doc, &mut layout, &placed);
    let (redraws, _) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(5),
        |i| anim_shows_frame(i, 1),
    );
    assert!(
        anim_shows_frame(&mut images, 1),
        "it starts: {:?}",
        images.anim_state(1)
    );
    assert!(redraws < 20, "{redraws} full paints");
    let _ = std::fs::remove_dir_all(&dir);
}

/// One that waits for the one above it, which plays on, is not asked for again and again as that
/// one's steps come in (no storm of repaints, no read of it): nothing is asked for while it
/// doesn't fit, and then it is not the poll that does.
#[test]
fn a_waiting_animation_that_cant_fit_causes_no_repaints_as_the_one_above_plays() {
    let dir = scratch("wait-above");
    let gif = demo_gif(800, 600, 10);
    std::fs::write(dir.join("a.gif"), &gif).unwrap();
    std::fs::write(dir.join("b.gif"), &gif).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![above](a.gif)\n\n![below](b.gif)");
    let boxes = [(1, 1440, 1080), (2, 1440, 1080)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let placed = [(1, 1440, 1080, 0), (2, 1440, 1080, 1100)];
    let key2 = images.key(2).unwrap();
    paint_placed_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1) && i.anims.waits(&key2)
    });
    // (The paint for the waiting, and the steps it was in the way of.)
    for _ in 0..3 {
        paint_placed(&mut images, &doc, &mut layout, &placed);
        std::thread::sleep(Duration::from_millis(20));
    }
    let serial = images.anims.serial;
    let (redraws, steps) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(1),
        |_| false,
    );
    assert!(steps >= 3, "the one above plays: {steps} frames");
    assert_eq!(redraws, 0, "full paints asked for");
    assert_eq!(images.anims.serial, serial, "read again");
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An animation painted that is then found over the limits (what it costs, once measured) asks
/// for the paint of its badge, once, with nothing else to prompt it.
#[test]
fn an_animation_found_over_the_limits_after_it_was_painted_asks_for_one_repaint() {
    let dir = scratch("over-painted");
    let mut gif = fixture("anim.gif");
    // The first frame stays normal; the second's scratch is over what one animation may hold.
    let index = anim::Index::new(&gif).unwrap();
    let second = index.frames[1].at as usize;
    gif[second + 5..second + 7].copy_from_slice(&4000u16.to_le_bytes());
    gif[second + 7..second + 9].copy_from_slice(&4000u16.to_le_bytes());
    std::fs::write(dir.join("over.gif"), gif).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](over.gif)");
    pump(&doc, &mut layout, &mut images, &[(1, 120, 80)], |i, _| {
        use crate::render::Pixels;
        i.pixels(1, 120, 80).is_some()
    });
    while images.poll(&doc, &mut layout) {}
    images.set_animations(true);
    place_only(&mut images, &[(1, 120, 80, 0)]);
    images.painted();
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    let t = Instant::now();
    let mut asked = 0;
    while !images.anim_over(1) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        asked += usize::from(images.poll(&doc, &mut layout));
        std::thread::sleep(Duration::from_millis(1));
    }
    asked += usize::from(images.poll(&doc, &mut layout));
    assert_eq!(asked, 1, "a repaint for the badge, once");
    paint_placed(&mut images, &doc, &mut layout, &[(1, 120, 80, 0)]);
    for _ in 0..20 {
        assert!(!images.poll(&doc, &mut layout), "nothing more to paint");
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A play-once animation whose streamed frames end while its last is shown asks for the paint
/// of its badge, once (nothing is due: no tick comes for it).
#[test]
fn a_streamed_animation_that_ends_asks_for_one_repaint() {
    let dir = scratch("end-painted");
    let mut webp = fixture("anim.webp");
    let at = webp.windows(4).position(|w| w == b"ANIM").unwrap() + 12;
    webp[at..at + 2].copy_from_slice(&1u16.to_le_bytes());
    std::fs::write(dir.join("once.webp"), webp).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![a](once.webp)");
    let boxes = [(1, 800, 600, 0)];
    pump(&doc, &mut layout, &mut images, &[(1, 800, 600)], |i, _| {
        use crate::render::Pixels;
        i.pixels(1, 800, 600).is_some()
    });
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    paint_placed(&mut images, &doc, &mut layout, &boxes);
    // (A window of Windows and Wayland: no paint but what is asked for, and the changed boxes.)
    let time = Duration::from_secs(10);
    paint_partially(&mut images, &doc, &mut layout, &boxes, time, |i| {
        i.anim_state(1) == Some(AnimState::Paused)
    });
    let (redraws, _) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &boxes,
        Duration::from_millis(200),
        |_| false,
    );
    assert_eq!(images.anim_state(1), Some(AnimState::Paused));
    assert_eq!(
        images.anims.placed[0].2,
        Some(AnimState::Paused),
        "its badge was painted"
    );
    assert!(redraws <= 1, "{redraws} repaints asked for");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "remote-images")]
#[test]
fn remote_animation_refusals_are_forgotten_with_their_documents_and_slots() {
    let mut gif = fixture("anim.gif");
    // The first frame stays normal; the second's scratch is over what one animation may hold.
    let index = anim::Index::new(&gif).unwrap();
    let second = index.frames[1].at as usize;
    gif[second + 5..second + 7].copy_from_slice(&4000u16.to_le_bytes());
    gif[second + 7..second + 9].copy_from_slice(&4000u16.to_le_bytes());
    let gif = Arc::new(gif);
    let mut images = Images::default();
    images
        .remote
        .set_fetcher(Arc::new(move |_, _| Ok((Arc::new(gif.to_vec()), 6, 4))));
    images.set_policy(crate::settings::RemoteImages::Always);
    images.set_animations(true);
    let mut first = None;
    for n in 0..64 {
        let url = format!("https://x.y/{n}/{}.gif", "x".repeat(16000));
        let md = format!("![x]({url})");
        let (doc, mut layout) = open_and_settle(&mut images, &md);
        paint_remote_until(&mut images, &doc, &mut layout, |i| i.anim_over(1));
        first.get_or_insert(md);
    }
    // A reload keeps what was found out of the document's image, and so does leaving it while its
    // still is cached.
    let md = first.unwrap();
    open_and_settle(&mut images, &md);
    assert!(images.anim_over(1), "over the limits, from before");
    open_and_settle(&mut images, "# No images");
    // The last of what holds the URLs goes: the slots are evicted.
    images.cache.cap = 0;
    images.begin_frame();
    images.cache.evict();
    images.forget_evicted();
    images.trim_records();
    assert_eq!(images.cache.slots.len(), 0);
    assert!(
        images.anims.refused.is_empty(),
        "no document, slot or job holds these remote images any more"
    );
    assert!(images.animated.is_empty());
    assert!(images.anims.forced.is_empty());
}

/// Two looping 800 × 600 animations (each one bitmap of 1440 × 1080: two don't fit beside each
/// other), the one below playing, then the one above in view: it waits for it.
fn waiting_above(name: &str, delay_cs: Option<u16>) -> (PathBuf, Doc, Layout, Images) {
    let dir = scratch(name);
    let mut gif = demo_gif(800, 600, 10);
    if let Some(cs) = delay_cs {
        for entry in anim::Index::new(&gif).unwrap().frames {
            // (The delay of the graphic control block before the frame's image.)
            let at = entry.at as usize;
            assert_eq!(&gif[at - 8..at - 6], &[0x21, 0xf9]);
            gif[at - 4..at - 2].copy_from_slice(&cs.to_le_bytes());
        }
    }
    std::fs::write(dir.join("a.gif"), &gif).unwrap();
    std::fs::write(dir.join("b.gif"), &gif).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![above](a.gif)\n\n![below](b.gif)");
    let boxes = [(1, 1440, 1080), (2, 1440, 1080)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    (dir, doc, layout, images)
}

/// The animation above waits for the one below, whose steps take longer than its frames' delays,
/// so it is never idle between them: the waiter doesn't depend on catching it so. Once one
/// waits that would fit if the one below were let go, the one below is asked for no more
/// steps; when its step comes back it is let go, the waiter is read, and plays, with the full
/// paints asked for a few, not one for each step.
#[test]
fn a_waiting_animation_starts_though_the_one_below_never_idles_between_steps() {
    let (dir, doc, mut layout, mut images) = waiting_above("wait-slow", Some(2));
    images.anims.player.slow_steps(Duration::from_millis(80));
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    // (The one below plays alone, as in view alone.)
    paint_placed_until(
        &mut images,
        &doc,
        &mut layout,
        &[(2, 1440, 1080, 1100)],
        |i| anim_shows_frame(i, 2),
    );
    let placed = [(1, 1440, 1080, 0), (2, 1440, 1080, 1100)];
    place_only(&mut images, &placed);
    images.painted();
    let (redraws, _) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(5),
        |i| anim_shows_frame(i, 1),
    );
    assert!(anim_shows_frame(&mut images, 1), "the one above plays");
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    assert!(redraws <= 6, "full paints asked for: {redraws}");
    // (The one below waits for it, as an animation does.)
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A waiter that is below the animation it waits for asks nothing of the one above: it plays on
/// at its pace.
#[test]
fn a_waiter_that_couldnt_fit_anyway_doesnt_stop_the_one_below() {
    let (dir, doc, mut layout, mut images) = waiting_above("wait-claim", None);
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let placed = [(1, 1440, 1080, 0), (2, 1440, 1080, 1100)];
    let key2 = images.key(2).unwrap();
    paint_placed_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1) && i.anims.waits(&key2)
    });
    // (The one that waits is below the one that holds the room: that is not let go for it.)
    let key1 = images.key(1).unwrap();
    place_only(&mut images, &placed);
    images.painted();
    let serial = images.anims.serial;
    let (_, steps) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_millis(500),
        |_| false,
    );
    assert!(steps >= 2, "the one above plays: {steps}");
    assert_eq!(images.anims.serial, serial);
    assert!(images.anim_at(&key1).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Paint `boxes` until `done`, as a window does ([`paint_partially`]), the first paint whole
/// (whatever poll says of it).
fn paint_boxes_until(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32, i32)],
    mut done: impl FnMut(&mut Images) -> bool,
) {
    place_only(images, boxes);
    images.painted();
    let t = Instant::now();
    while !done(images) {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out");
        paint_partially(
            images,
            doc,
            layout,
            boxes,
            Duration::from_millis(20),
            |_| false,
        );
    }
}

/// An animation that waits holds what it was read at for the box it had: when the box is
/// painted smaller (a zoom out, a resize, another scale) it is read again for that, and plays
/// if both now fit, with no click.
#[test]
fn a_waiting_animation_is_read_again_when_its_box_shrinks() {
    let dir = scratch("wait-shrink");
    std::fs::write(dir.join("a.gif"), demo_gif(900, 600, 10)).unwrap();
    std::fs::write(dir.join("b.gif"), demo_gif(800, 600, 10)).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![above](a.gif)\n\n![below](b.gif)");
    let big = [(1, 1800, 1200), (2, 1440, 1080)];
    pump(&doc, &mut layout, &mut images, &big, |i, _| {
        use crate::render::Pixels;
        big.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let key2 = images.key(2).unwrap();
    let wide = [(1, 1800, 1200, 0), (2, 1440, 1080, 1220)];
    paint_boxes_until(&mut images, &doc, &mut layout, &wide, |i| {
        anim_shows_frame(i, 1) && i.anims.waits(&key2)
    });
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    // Painted at half: what it needs is less, and fits beside the one above (that is made at
    // its own size, whatever the box).
    let small = [(1, 900, 600, 0), (2, 720, 540, 620)];
    paint_boxes_until(&mut images, &doc, &mut layout, &small, |i| {
        anim_shows_frame(i, 2)
    });
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
    assert!(anim_shows_frame(&mut images, 1));
    assert!(!images.anims.waits(&key2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The other way: both play at small boxes, and a zoom in that makes them not fit beside each
/// other leaves the one above playing and the other waiting, read once for its new box; neither
/// goes on repainting or reading as it waits.
#[test]
fn an_animation_that_no_longer_fits_after_a_zoom_in_waits_quietly() {
    let dir = scratch("wait-grow");
    std::fs::write(dir.join("a.gif"), demo_gif(900, 600, 10)).unwrap();
    std::fs::write(dir.join("b.gif"), demo_gif(800, 600, 10)).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![above](a.gif)\n\n![below](b.gif)");
    let small = [(1, 900, 600, 0), (2, 720, 540, 620)];
    let boxes: Vec<_> = small.iter().map(|&(id, w, h, _)| (id, w, h)).collect();
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    paint_boxes_until(&mut images, &doc, &mut layout, &small, |i| {
        anim_shows_frame(i, 1) && anim_shows_frame(i, 2)
    });
    let wide = [(1, 1800, 1200, 0), (2, 1440, 1080, 1220)];
    paint_boxes_until(&mut images, &doc, &mut layout, &wide, |i| {
        i.anim_state(1) == Some(AnimState::Playing)
            && anim_shows_frame(i, 1)
            && i.anim_state(2) == Some(AnimState::Waiting)
    });
    let key2 = images.key(2).unwrap();
    // (Let what the zoom set going settle.)
    paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &wide,
        Duration::from_millis(300),
        |_| false,
    );
    assert!(images.anims.waits(&key2));
    let serial = images.anims.serial;
    let (redraws, steps) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &wide,
        Duration::from_secs(1),
        |_| false,
    );
    assert!(steps >= 3, "the one above plays: {steps}");
    assert_eq!(redraws, 0, "full paints asked for");
    assert_eq!(images.anims.serial, serial, "read again");
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    let _ = std::fs::remove_dir_all(&dir);
}

/// What [`paint_partially`] does for `time`, counting the frames each image showed: a window that
/// paints whole only when poll asks, else the boxes `advance` changed.
fn swaps_by_image(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    boxes: &[(u32, u32, u32, i32)],
    time: Duration,
) -> std::collections::HashMap<u32, usize> {
    use crate::render::Pixels;
    let mut swaps = std::collections::HashMap::new();
    place_only(images, boxes);
    images.painted();
    let start = Instant::now();
    while start.elapsed() < time {
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.poll(doc, layout) {
            place_only(images, boxes);
            images.painted();
        }
        if images.anim_deadline().is_some_and(|due| due <= now) {
            for id in images.advance(now) {
                *swaps.entry(id).or_insert(0) += 1;
                let (_, w, h, top) = boxes.iter().find(|b| b.0 == id).copied().unwrap();
                images.placed(id, (0, top, w as i32, top + h as i32));
                let _ = images.pixels(id, w, h);
            }
            images.painted();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    swaps
}

/// Three looping animations, one above the other: two 800 × 600 ones at 1440 × 1080 (one bitmap
/// each, two don't fit beside each other) and a small streamed one (400 × 300) below them. With
/// all three in view the first plays, the second waits, the third plays.
/// The page of [`holder_waiter_below`]: its directory, document, layout and images, and the boxes
/// painted.
type ThreeHigh = (PathBuf, Doc, Layout, Images, [(u32, u32, u32, i32); 3]);

fn holder_waiter_below(name: &str) -> ThreeHigh {
    let dir = scratch(name);
    let big = demo_gif(800, 600, 10);
    std::fs::write(dir.join("a.gif"), &big).unwrap();
    std::fs::write(dir.join("b.gif"), &big).unwrap();
    std::fs::write(dir.join("c.gif"), demo_gif(400, 300, 20)).unwrap();
    let md = "![holder](a.gif)\n\n![waiter](b.gif)\n\n![below](c.gif)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    let boxes = [(1, 1440, 1080), (2, 1440, 1080), (3, 400, 300)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let placed = [
        (1, 1440, 1080, 0),
        (2, 1440, 1080, 1100),
        (3, 400, 300, 2200),
    ];
    let key2 = images.key(2).unwrap();
    paint_boxes_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1) && anim_shows_frame(i, 3) && i.anims.waits(&key2)
    });
    (dir, doc, layout, images, placed)
}

/// SOL4-M2: a click plays a waiting animation past the limits while the one below it runs a
/// step: it plays outside the cache and takes nothing from the others, which play on (it takes no
/// part in the allocation).
#[test]
fn playing_a_waiter_past_the_limits_leaves_the_others_playing() {
    let (dir, doc, mut layout, mut images, placed) = holder_waiter_below("forced-waiter");
    images.anims.player.slow_steps(Duration::from_millis(80));
    let (key2, key3) = (images.key(2).unwrap(), images.key(3).unwrap());
    // (The click comes while the step of the one below runs.)
    let t = Instant::now();
    while !images
        .anim_at(&key3)
        .is_some_and(|at| images.playing(at).running)
    {
        assert!(t.elapsed() < Duration::from_secs(10), "no step ran");
        paint_partially(
            &mut images,
            &doc,
            &mut layout,
            &placed,
            Duration::from_millis(5),
            |_| false,
        );
    }
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    images.play(2);
    assert!(images.anims.forced.contains(&key2));
    let swaps = swaps_by_image(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(2),
    );
    let outside = matches!(images.anim_at(&key2), Some(At::Outside(_)));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(outside, "played outside the cache");
    for id in 1..=3 {
        assert!(swaps.get(&id).copied().unwrap_or(0) >= 3, "{id}: {swaps:?}");
    }
}

/// SOL4-M3: the file of a waiting animation goes: the others play on, whatever it is now.
#[test]
fn a_waiter_whose_file_went_leaves_the_others_playing() {
    let (dir, doc, mut layout, mut images, placed) = holder_waiter_below("missing-waiter");
    std::fs::remove_file(dir.join("b.gif")).unwrap();
    let swaps = swaps_by_image(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(2),
    );
    let _ = std::fs::remove_dir_all(&dir);
    for id in [1, 3] {
        assert!(swaps.get(&id).copied().unwrap_or(0) >= 3, "{id}: {swaps:?}");
    }
}

/// SOL4-M3, the way it was found: the animation above, measured while the one below runs its
/// (slow) steps, has its file removed before it is read to play. It is refused (it can't be
/// read: no badge, read no more), and the one below plays again.
#[test]
fn an_animation_above_whose_file_went_gives_the_room_back() {
    let (dir, doc, mut layout, mut images) = waiting_above("missing-above", Some(2));
    images.anims.player.slow_steps(Duration::from_millis(80));
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let below = [(2, 1440, 1080, 1100)];
    paint_placed_until(&mut images, &doc, &mut layout, &below, |i| {
        anim_shows_frame(i, 2)
    });
    let placed = [(1, 1440, 1080, 0), (2, 1440, 1080, 1100)];
    let key1 = images.key(1).unwrap();
    let measured = |i: &Images| i.anims.costs.iter().any(|c| c.0 == key1);
    place_only(&mut images, &placed);
    images.painted();
    let t = Instant::now();
    while !measured(&images) {
        assert!(t.elapsed() < Duration::from_secs(10), "not measured");
        paint_partially(
            &mut images,
            &doc,
            &mut layout,
            &placed,
            Duration::from_millis(5),
            |_| false,
        );
    }
    std::fs::remove_file(dir.join("a.gif")).unwrap();
    let swaps = swaps_by_image(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(2),
    );
    let state = images.anim_state(1);
    let reads = images.anims.serial;
    for _ in 0..10 {
        paint_placed(&mut images, &doc, &mut layout, &placed);
    }
    let read_again = images.anims.serial != reads;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(swaps.get(&2).copied().unwrap_or(0) >= 3, "{swaps:?}");
    assert_eq!(state, None, "refused");
    assert!(!read_again, "read again");
}

/// The ways a waiting animation stops being in the way, the one below it then playing: it is
/// scrolled off, paused, all animations are turned off (and on again with it paused), or another
/// document comes (the reviewer's lifecycle probe, sol-4).
#[test]
fn the_one_below_a_waiter_plays_once_the_waiter_is_out_of_the_way() {
    for action in ["scroll", "pause", "off", "document"] {
        let (dir, doc, mut layout, mut images, placed) =
            holder_waiter_below(&format!("waiter-gone-{action}"));
        let key2 = images.key(2).unwrap();
        let (redraws, _) = paint_partially(
            &mut images,
            &doc,
            &mut layout,
            &placed,
            Duration::from_millis(300),
            |_| false,
        );
        assert_eq!(redraws, 0, "{action}: quiet while it waits");
        assert!(images.anims.waits(&key2));
        let swaps = match action {
            // (The holder off screen, the waiter at the top.)
            "scroll" => {
                let lower = [(2, 1440, 1080, 0), (3, 400, 300, 1100)];
                swaps_by_image(
                    &mut images,
                    &doc,
                    &mut layout,
                    &lower,
                    Duration::from_secs(1),
                )
            }
            "pause" => {
                images.pause(1);
                swaps_by_image(
                    &mut images,
                    &doc,
                    &mut layout,
                    &placed,
                    Duration::from_secs(1),
                )
            }
            "off" => {
                images.set_animations(false);
                let none = swaps_by_image(
                    &mut images,
                    &doc,
                    &mut layout,
                    &placed,
                    Duration::from_millis(300),
                );
                assert!(none.is_empty(), "off: {none:?}");
                assert_eq!(images.anim_deadline(), None);
                images.set_animations(true);
                images.pause(1);
                swaps_by_image(
                    &mut images,
                    &doc,
                    &mut layout,
                    &placed,
                    Duration::from_secs(1),
                )
            }
            _ => {
                let doc = Doc::parse("![above](a.gif)\n\n![below](b.gif)");
                let mut layout = Layout::new(&doc, crate::layout::Params::default());
                images.attach(&doc, &mut layout);
                pump(
                    &doc,
                    &mut layout,
                    &mut images,
                    &[(2, 1440, 1080)],
                    |i, _| {
                        use crate::render::Pixels;
                        i.pixels(2, 1440, 1080).is_some()
                    },
                );
                let lower = [(2, 1440, 1080, 0)];
                swaps_by_image(
                    &mut images,
                    &doc,
                    &mut layout,
                    &lower,
                    Duration::from_secs(1),
                )
            }
        };
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            swaps.get(&2).copied().unwrap_or(0) >= 3,
            "{action}: {swaps:?}"
        );
    }
}

/// SOL4-M1: one remote animation referenced at two sizes, waiting beside the one above it: each
/// size is its own cost, so neither box undoes what the other was measured to hold. It is read
/// once or twice, not on every paint, and asks for no stream of full paints.
#[cfg(feature = "remote-images")]
#[test]
fn a_waiting_remote_animation_used_at_two_sizes_stays_quiet() {
    use crate::render::Pixels;
    let dir = scratch("remote-two-sizes");
    std::fs::write(dir.join("a.gif"), demo_gif(900, 600, 10)).unwrap();
    let b = Arc::new(demo_gif(800, 600, 10));
    let md = "![holder](a.gif)\n\n![big](https://x.y/b.gif)\n\n![small](https://x.y/b.gif)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    images
        .remote
        .set_fetcher(Arc::new(move |_, _| Ok((b.clone(), 800, 600))));
    images.set_policy(crate::settings::RemoteImages::Always);
    images.set_remote(true, &doc, &mut layout);
    let boxes = [(1, 1800, 1200), (2, 1440, 1080), (3, 720, 540)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    paint_placed_until(&mut images, &doc, &mut layout, &[(1, 1800, 1200, 0)], |i| {
        anim_shows_frame(i, 1)
    });
    let placed = [
        (1, 1800, 1200, 0),
        (2, 1440, 1080, 1220),
        (3, 720, 540, 2320),
    ];
    let serial = images.anims.serial;
    // (The poll that took the holder's first frame asked for the paint that shows it, and makes
    // its next frame due: the window paints the page.)
    paint_placed(&mut images, &doc, &mut layout, &placed);
    let (full, swaps) = paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(2),
        |_| false,
    );
    let reads = images.anims.serial - serial;
    let state = images.anim_state(2);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(reads <= 2, "{reads} reads");
    assert!(full < 20, "{full} full paints");
    assert!(swaps >= 3, "the holder plays: {swaps}");
    assert_eq!(state, Some(AnimState::Waiting));
}

/// The allocation weighs only what is painted and known: with nothing changed, weighing it again
/// sets nothing going (no read, no animation let go), asks for no repaint, and gives the same
/// answer; [`anim::allocate`] is a function of its arguments.
#[test]
fn the_allocation_does_nothing_again_for_the_same_inputs() {
    let (dir, doc, mut layout, mut images, placed) = holder_waiter_below("alloc-idempotent");
    // (Settled: what the last paints set going is done.)
    paint_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_millis(300),
        |_| false,
    );
    let (effects, serial) = (images.anims.effects, images.anims.serial);
    let measures = images.anims.measures.len();
    let alloc = |i: &Images| {
        let a = &i.anims.alloc;
        (a.play.clone(), a.waiting.clone())
    };
    let before = alloc(&images);
    images.repaint = false;
    for _ in 0..20 {
        images.painted();
        images.pump_anims();
    }
    assert_eq!(images.anims.effects, effects, "set going again");
    assert_eq!(images.anims.serial, serial, "read again");
    assert_eq!(images.anims.measures.len(), measures, "measured again");
    assert!(!images.repaint, "a repaint asked for");
    assert_eq!(alloc(&images), before);
    let again = anim::allocate(images.anims.alloc.bids.clone(), images.anims.alloc.room);
    assert_eq!((again.play, again.waiting), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stills of animations shown are kept as painted: with two 800×600 animations at 1440×1080
/// (the second played past the limits) and a 400×300 one at 720×540 playing, the cache emptied,
/// the stills asked for again don't fit beside each other and a decode, and each one decoded
/// would evict the other (whose animation's frame, not the still, was painted), and be asked
/// for again, without end.
#[test]
fn the_stills_of_animations_shown_are_not_decoded_again_and_again() {
    let dir = scratch("anim-stills-kept");
    std::fs::write(dir.join("a.gif"), demo_gif(800, 600, 10)).unwrap();
    std::fs::write(dir.join("b.gif"), demo_gif(800, 600, 10)).unwrap();
    std::fs::write(dir.join("c.gif"), demo_gif(400, 300, 20)).unwrap();
    let md = "![a](a.gif)\n\n![b](b.gif)\n\n![c](c.gif)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    let boxes = [(1, 1440, 1080), (2, 1440, 1080), (3, 720, 540)];
    let placed = [
        (1, 1440, 1080, 0),
        (2, 1440, 1080, 1100),
        (3, 720, 540, 2200),
    ];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        use crate::render::Pixels;
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    play_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1) && anim_shows_frame(i, 3)
    });
    // (The second waits: a click plays it past the limits.)
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    images.play(2);
    play_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 2)
    });
    images.begin_frame();
    images.cache.evict_for(images.cache.cap);
    let decodes = images.stats().decodes;
    // (A window that paints whole, and asks for its images, when poll asks.)
    paint_placed(&mut images, &doc, &mut layout, &placed);
    images.request(&boxes, 3);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) {
        if images.poll(&doc, &mut layout) {
            paint_placed(&mut images, &doc, &mut layout, &placed);
            images.request(&boxes, 3);
        }
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.anim_deadline().is_some_and(|due| due <= now) {
            images.advance(now);
            place_only(&mut images, &placed);
            images.painted();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let decodes = images.stats().decodes - decodes;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(decodes <= 6, "{decodes} decodes");
}

/// `gif` without its loop extension: it plays once.
fn play_once(gif: &[u8]) -> Vec<u8> {
    let at = gif.windows(11).position(|w| w == b"NETSCAPE2.0").unwrap() - 3;
    [&gif[..at], &gif[at + 19..]].concat()
}

/// A reader's random walk over five animations (two that don't fit beside each other, a small
/// streamed one, a kept one, one that plays once) and, with remote images, a remote one whose
/// 6 MiB body counts in the cache (and fetching it again, the fetchers' reserve), with the
/// window's clock jumping to each frame's time: scrolls, zooms, clicks, `A`, a new document, the
/// cache emptied, a file that changes (grows, past the limits or not, or is put back), the
/// remote body dropped for room (and its measures forgotten, or not), the fetches past the limits
/// failing or not. After each, as the window paints (whole when poll asks, else the changed boxes): every
/// animation the allocation plays, or the reader plays past the limits, shows new frames, and
/// still does once settled (none is frozen); one that ended isn't played again unless the reader
/// asks (a click, `A`, a new document); the remote still stays; the full paints asked for are a
/// few for the change and then none; no image is measured twice at one size unless its file
/// changed; what the animations hold is within the limits.
#[test]
fn a_random_walk_keeps_every_animation_that_should_play_playing() {
    use crate::render::Pixels;
    let dir = scratch("anim-walk");
    let a = demo_gif(800, 600, 10);
    std::fs::write(dir.join("a.gif"), &a).unwrap();
    std::fs::write(dir.join("b.gif"), demo_gif(800, 600, 10)).unwrap();
    std::fs::write(dir.join("c.gif"), demo_gif(400, 300, 20)).unwrap();
    std::fs::write(dir.join("d.gif"), fixture("anim.gif")).unwrap();
    std::fs::write(dir.join("e.gif"), play_once(&demo_gif(300, 200, 4))).unwrap();
    let mut md =
        "![a](a.gif)\n\n![b](b.gif)\n\n![c](c.gif)\n\n![d](d.gif)\n\n![e](e.gif)".to_string();
    let mut base = vec![
        (1, 800, 600),
        (2, 800, 600),
        (3, 400, 300),
        (4, 60, 40),
        (5, 300, 200),
    ];
    let remote = cfg!(feature = "remote-images");
    #[cfg(feature = "remote-images")]
    let (url, body, fails): (Arc<str>, _, _) = (
        "https://x.y/f.gif".into(),
        Arc::new({
            let mut f = demo_gif(400, 300, 8);
            f.resize(6 << 20, 0);
            f
        }),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    if remote {
        md.push_str("\n\n![f](https://x.y/f.gif)");
        base.push((6, 400, 300));
    }
    let window = 2400;
    for seed in [3u64, 11, 29] {
        let (mut doc, mut layout, mut images) = forced_page(&dir, &md);
        #[cfg(feature = "remote-images")]
        {
            let (outside, fail) = (body.clone(), fails.clone());
            images.remote.forced_fetch = Arc::new(move |_, _| match fail.load(Ordering::Relaxed) {
                true => Err(Refusal::Failed),
                false => Ok(outside.to_vec()),
            });
            // (A body of its own: one the test holds too would look held by a decode.)
            let body = body.clone();
            images.remote.set_fetcher(Arc::new(move |_, _| {
                Ok((Arc::new(body.to_vec()), 400, 300))
            }));
            images.set_policy(crate::settings::RemoteImages::Always);
            images.set_remote(true, &doc, &mut layout);
        }
        std::fs::write(dir.join("a.gif"), &a).unwrap();
        let mut rng = seed;
        let mut next = |n: u64| {
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (rng >> 33) % n
        };
        let (mut zoom, mut scroll) = (1.8f64, 0i32);
        let mut now = Instant::now();
        images.set_anim_clock(Some(now));
        images.set_animations(true);
        let mut measured: Vec<(Key, (u32, u32))> = Vec::new();
        for step in 0..24 {
            // The boxes in view: stacked, 20 apart, `scroll` down.
            let mut top = 0;
            let mut placed = Vec::new();
            let mut wanted = Vec::new();
            for &(id, w, h) in &base {
                let (w, h) = ((w as f64 * zoom) as u32, (h as f64 * zoom) as u32);
                let y = top - scroll;
                if y + h as i32 > 0 && y < window {
                    placed.push((id, w, h, y));
                    wanted.push((id, w, h));
                }
                top += h as i32 + 20;
            }
            // Those that ended before the step: they stay so, unless the reader asks.
            let mut ended: Vec<Key> = (base.iter())
                .filter_map(|b| images.key(b.0))
                .filter(|k| images.anim_ended(k))
                .collect();
            let event = next(11);
            let what = match event {
                0 => {
                    scroll = [0, 600, 1200, 2000][next(4) as usize];
                    "scroll"
                }
                1 => {
                    zoom = [0.5, 1.0, 1.8][next(3) as usize];
                    "zoom"
                }
                2 | 3 => {
                    if let Some(&(id, ..)) = placed.get(next(placed.len().max(1) as u64) as usize) {
                        if images.anim_state(id) == Some(AnimState::Playing) {
                            images.pause(id);
                        } else {
                            images.play(id);
                            let key = images.key(id);
                            ended.retain(|k| Some(k) != key.as_ref());
                        }
                    }
                    "click"
                }
                4 => {
                    images.set_animations(!images.animations());
                    ended.clear();
                    "A"
                }
                5 => {
                    doc = Doc::parse(&md);
                    layout = Layout::new(&doc, crate::layout::Params::default());
                    images.attach(&doc, &mut layout);
                    #[cfg(feature = "remote-images")]
                    images.set_remote(true, &doc, &mut layout);
                    images.set_animations(true);
                    measured.clear();
                    ended.clear();
                    "document"
                }
                6 => {
                    images.begin_frame();
                    images.cache.evict_for(images.cache.cap);
                    "evict"
                }
                7 => {
                    // It grows (past the limit for all, or not), or is put back.
                    let mut grown = a.clone();
                    grown.resize([a.len(), 4 << 20, 40 << 20][next(3) as usize], 0);
                    std::fs::write(dir.join("a.gif"), grown).unwrap();
                    measured.retain(|m| m.0 != Key::Local(1));
                    "file"
                }
                8 if remote => {
                    // Its body was dropped for room when it came (as `drop_fetched` notes it).
                    #[cfg(feature = "remote-images")]
                    if images.anim_at(&Key::Remote(url.clone())).is_none() {
                        images.cache.discard_bytes(&url);
                        let need = (
                            Slot::bytes_cost(&url, &body),
                            images.decode_estimate(&url, &body),
                        );
                        images.defer(&url, need);
                        if next(2) == 0 {
                            let key = Key::Remote(url.clone());
                            images.anims.retain_costs(&|c| c.0 != key);
                            measured.retain(|m| m.0 != key);
                        }
                    }
                    "body dropped"
                }
                10 if remote => {
                    #[cfg(feature = "remote-images")]
                    fails.store(next(2) == 0, Ordering::Relaxed);
                    "forced fails"
                }
                _ => "nothing",
            };
            let paint = |images: &mut Images| {
                images.begin_frame();
                for &(id, w, h, top) in &placed {
                    images.placed(id, (0, top, w as i32, top + h as i32));
                    let _ = images.pixels(id, w, h);
                }
                images.painted();
                images.request(&wanted, wanted.len());
            };
            // The window settles: the stills come, what is to play starts, each animation that
            // plays shows frames.
            paint(&mut images);
            let mut swapped: Vec<u32> = Vec::new();
            let mut full = 0;
            let t = Instant::now();
            let should = |i: &Images| -> Vec<u32> {
                (placed.iter().map(|p| p.0))
                    .filter(|&id| {
                        let Some(key) = i.key(id) else {
                            return false;
                        };
                        i.animated(id).is_some()
                            && i.anims.wanted(&key)
                            && !i.anim_ended(&key)
                            && !i.anims.failed.contains(&key)
                            && (i.anims.alloc.play.contains(&key)
                                || (i.anims.forced.contains(&key)
                                    && i.anims.refusal(&key) != Some(false)))
                    })
                    .collect()
            };
            let ends = |i: &Images| {
                (placed.iter())
                    .filter(|p| i.key(p.0).is_some_and(|k| i.anim_ended(&k)))
                    .count()
            };
            // (Settled since a time, with the full paints and the ends then, and the allocation
            // (who plays, who waits): it is settled anew once that changes, e.g. a kept one that
            // is made gives its build's room back.)
            let alloc = |i: &Images| (i.anims.alloc.play.clone(), i.anims.alloc.waiting.clone());
            let mut settled: Option<(Instant, usize, usize, _)> = None;
            let (mut last, mut changes) = (alloc(&images), 0);
            let (decodes, serial) = (images.stats().decodes, images.anims.serial);
            let mut after: Vec<u32> = Vec::new();
            while t.elapsed() < Duration::from_secs(5) {
                // (A paint tells the allocation what is shown, e.g. an animation whose still just
                // came: only the poll after it weighs that. Nothing is settled before.)
                let quiet = !images.poll(&doc, &mut layout);
                if !quiet {
                    full += 1;
                    paint(&mut images);
                }
                if let Some(due) = images.anim_deadline() {
                    now = now.max(due);
                    images.set_anim_clock(Some(now));
                    for id in images.advance(now) {
                        swapped.push(id);
                        if settled.is_some() {
                            after.push(id);
                        }
                        if let Some(&(_, w, h, top)) = placed.iter().find(|p| p.0 == id) {
                            images.placed(id, (0, top, w as i32, top + h as i32));
                            let _ = images.pixels(id, w, h);
                        }
                    }
                    images.painted();
                }
                // What the animations hold is within the limits, whatever is going on.
                let held = images.anim_slots_cost();
                assert!(
                    held <= anim::ANIM_ALL_MAX,
                    "seed {seed} step {step} ({what}): {held}"
                );
                for s in &images.cache.slots {
                    if let Data::Anim(p) = &s.data {
                        assert!(p.cost <= anim::ANIM_ONE_MAX, "seed {seed} step {step}");
                    }
                }
                // One that ended isn't played again by itself.
                for key in &ended {
                    assert!(
                        !images.anims.alloc.play.contains(key) && images.anim_ended(key),
                        "seed {seed} step {step} ({what}): {key:?} played again"
                    );
                }
                // (Settled: the stills decoded at these boxes, or failed (a file grown past the
                // limits), nothing read, every animation that should play has shown a frame.)
                let shown = |p: &(u32, u32, u32, i32)| match images.key(p.0) {
                    Some(key) => images.cache.has(&key, p.1, p.2) || images.anim_at(&key).is_some(),
                    None => !images.is_waiting(p.0),
                };
                let all = quiet
                    && images.is_idle()
                    && !images.tables_waiting()
                    && placed.iter().all(shown)
                    && images.anims.out.is_empty()
                    && should(&images).iter().all(|id| swapped.contains(id));
                let play = alloc(&images);
                if play != last {
                    changes += 1;
                    last = play.clone();
                }
                match &settled {
                    Some((.., then)) if *then != play => {
                        settled = None;
                        after.clear();
                    }
                    None if all => settled = Some((Instant::now(), full, ends(&images), play)),
                    Some((at, ..)) if at.elapsed() > Duration::from_millis(150) => break,
                    _ => {}
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let states = || {
                (placed.iter())
                    .map(|p| (p.0, images.anim_state(p.0)))
                    .collect::<Vec<_>>()
            };
            let frozen: Vec<u32> = should(&images)
                .into_iter()
                .filter(|id| !swapped.contains(id))
                .collect();
            assert!(
                frozen.is_empty(),
                "seed {seed} step {step} ({what}): frozen {frozen:?}, states {:?}",
                states()
            );
            // (Still playing once settled: those that end may have ended meanwhile.)
            let stopped: Vec<u32> = should(&images)
                .into_iter()
                .filter(|id| !after.contains(id))
                .collect();
            assert!(
                stopped.is_empty(),
                "seed {seed} step {step} ({what}): stopped {stopped:?}, states {:?}",
                states()
            );
            // (One that ends meanwhile has its badge painted.)
            let (_, before, ended_then, _) = (settled.as_ref())
                .unwrap_or_else(|| panic!("seed {seed} step {step} ({what}): never settles"));
            let (before, ended_then) = (*before, *ended_then);
            assert!(
                full - before <= ends(&images) - ended_then,
                "seed {seed} step {step} ({what}): {} full paints once settled, {:?}",
                full - before,
                states()
            );
            // (A few for the change: one an image (its animation let go, its still shown), one
            // for each still decoded (a still evicted while its animation shows is decoded
            // again), one for each animation read (its first frame), one for each that ended,
            // one for each time the allocation changed, which changes badges. A scroll or zoom
            // shows in the step after it: an animation made at a new box that needs it made
            // anew (e.g. as one bitmap, [`anim::Engine::lean`]) is let go and read again.)
            let decodes = images.stats().decodes - decodes;
            let reads = (images.anims.serial - serial) as usize;
            assert!(
                full <= 2 + placed.len() + decodes + reads + ends(&images) + changes,
                "seed {seed} step {step} ({what}): {full} full paints, {decodes} decodes, \
                 {reads} reads, {changes} changes"
            );
            // (An animation that failed keeps its still.)
            assert!(
                !matches!(layout.image_size(6), Some(ImageSize::Refused(_))),
                "seed {seed} step {step} ({what}): the remote still went"
            );
            for m in std::mem::take(&mut images.anims.measures) {
                assert!(
                    !measured.contains(&m),
                    "seed {seed} step {step} ({what}): {m:?} measured again"
                );
                measured.push(m);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// sol-5 M1: an animation that played its loops stays ended once its slot is let go for the one
/// below: it isn't read and played again (no stream of reads and paints), and the one below
/// plays. A click plays it again.
#[test]
fn an_ended_animation_let_go_for_room_stays_ended() {
    use crate::render::Pixels;
    let dir = scratch("anim-ended-let-go");
    std::fs::write(dir.join("a.gif"), play_once(&demo_gif(1200, 900, 3))).unwrap();
    std::fs::write(dir.join("b.gif"), demo_gif(1200, 900, 3)).unwrap();
    let (doc, mut layout, mut images) = forced_page(&dir, "![once](a.gif)\n\n![loop](b.gif)");
    let boxes = [(1, 1440, 1080), (2, 1440, 1080)];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    let upper = [(1, 1440, 1080, 0)];
    play_until(&mut images, &doc, &mut layout, &upper, |i| {
        i.anim_ended(&i.key(1).unwrap())
    });
    assert_eq!(images.anim_state(1), Some(AnimState::Paused));
    let placed = [(1, 1440, 1080, 0), (2, 1440, 1080, 1100)];
    place_only(&mut images, &placed);
    images.painted();
    let serial = images.anims.serial;
    let key = images.key(1).unwrap();
    let (start, mut full, mut swaps, mut played) = (Instant::now(), 0, [0usize; 2], false);
    while start.elapsed() < Duration::from_secs(2) {
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.poll(&doc, &mut layout) {
            full += 1;
            place_only(&mut images, &placed);
            images.painted();
        }
        played |= images.anims.alloc.play.contains(&key);
        if images.anim_deadline().is_some_and(|d| d <= now) {
            for id in images.advance(now) {
                swaps[id as usize - 1] += 1;
                let (_, w, h, top) = placed.iter().find(|b| b.0 == id).copied().unwrap();
                images.placed(id, (0, top, w as i32, top + h as i32));
                let _ = images.pixels(id, w, h);
            }
            images.painted();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!played, "the ended one is played again");
    assert!(images.anims.serial <= serial + 1, "read again");
    assert_eq!(swaps[0], 0, "{swaps:?}");
    assert!(swaps[1] >= 3, "{swaps:?}");
    assert!(full < 10, "{full} full paints");
    assert_eq!(images.anim_state(1), Some(AnimState::Paused));
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
    // A click plays it again (it is read again: its slot went).
    images.play(1);
    assert!(!images.anim_ended(&key));
    paint_placed_until(&mut images, &doc, &mut layout, &placed, |i| {
        anim_shows_frame(i, 1)
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// sol-5 M2: the file of an animation above, measured, grows past the limits before it is read
/// to play: it is weighed again (refused, over the limits), and the one below keeps playing.
#[test]
fn an_animation_above_whose_file_grew_gives_the_room_back() {
    let (dir, doc, mut layout, mut images) = waiting_above("anim-grown-above", Some(2));
    images.anims.player.slow_steps(Duration::from_millis(80));
    images.set_animations(true);
    images.set_anim_clock(Some(Instant::now()));
    let below = [(2, 1440, 1080, 1100)];
    paint_placed_until(&mut images, &doc, &mut layout, &below, |i| {
        anim_shows_frame(i, 2)
    });
    let placed = [(1, 1440, 1080, 0), (2, 1440, 1080, 1100)];
    let key = images.key(1).unwrap();
    place_only(&mut images, &placed);
    images.painted();
    let t = Instant::now();
    while !images.anims.costs.iter().any(|c| c.0 == key) {
        assert!(t.elapsed() < Duration::from_secs(10), "not measured");
        let d = Duration::from_millis(5);
        paint_partially(&mut images, &doc, &mut layout, &placed, d, |_| false);
    }
    let mut grown = std::fs::read(dir.join("a.gif")).unwrap();
    grown.resize(40 << 20, 0);
    std::fs::write(dir.join("a.gif"), grown).unwrap();
    let swaps = swaps_by_image(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        Duration::from_secs(2),
    );
    let state = images.anim_state(1);
    let reads = images.anims.serial;
    for _ in 0..10 {
        paint_placed(&mut images, &doc, &mut layout, &placed);
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(swaps.get(&2).copied().unwrap_or(0) >= 3, "{swaps:?}");
    assert_eq!(state, Some(AnimState::Over));
    assert_eq!(images.anims.serial, reads, "read again");
}

/// sol-5 M3: a cached remote animation whose read (its index) has no room in the cache waits
/// (its badge shows), rather than playing with no frame, and a click plays it past the limits.
#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_whose_read_has_no_room_waits_and_a_click_plays_it() {
    use crate::render::Pixels;
    let url: Arc<str> = "https://x.y/anim.gif".into();
    let mut images = Images::default();
    let _fetches = anim_remote(&mut images);
    let md = format!(
        "<img src=\"https://x.y/anim.gif\" width=120 height=80>\n\n\
         <img src=\"{}/fixtures/images/alpha.png\" width=1408 height=1480>",
        env!("CARGO_MANIFEST_DIR")
    );
    let (doc, mut layout) = open_and_settle(&mut images, &md);
    paint_remote_until(&mut images, &doc, &mut layout, |i| {
        i.pixels(1, 240, 160).is_some()
    });
    assert!(images.cache.peek_bytes(&url).is_some());
    pump(&doc, &mut layout, &mut images, &[(1, 240, 160)], |i, _| {
        i.cache.has(&Key::Remote(url.clone()), 240, 160) && i.is_idle()
    });
    // A still painted that takes the rest of the cache (the ordinary 32 MiB), as one at HiDPI can.
    let (sw, sh) = (2816, 2960);
    let data = vec![0xff00_0000; sw as usize * sh as usize];
    let still = Bitmap {
        width: sw,
        height: sh,
        data,
    };
    images.cache.insert(Key::Local(2), Data::Pixels(still));
    let paint = |i: &mut Images, layout: &mut Layout| {
        i.begin_frame();
        i.placed(1, (0, 0, 240, 160));
        let _ = i.pixels(1, 240, 160);
        assert!(i.pixels(2, sw, sh).is_some());
        i.painted();
        i.poll(&doc, layout);
    };
    images.set_animations(true);
    for _ in 0..20 {
        paint(&mut images, &mut layout);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(images.anim_state(1), Some(AnimState::Waiting));
    assert!(images.anim_over(1));
    images.pause(1);
    images.play(1);
    let t = Instant::now();
    while !anim_shows_frame(&mut images, 1) {
        assert!(t.elapsed() < Duration::from_secs(5), "no frame");
        paint(&mut images, &mut layout);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(images.anims.forced.contains(&images.key(1).unwrap()));
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
}

/// The page of sol-5 M4 and sol-6 M2: a real 800×600, 90-frame, 8 MB remote GIF at 1440×1080
/// that played, then a 1800×1800 still below it that its body was dropped for, normal limits.
/// After a second of frames ([`body_went_frame`]) the body is gone and it waits. The forced
/// loader fetches with `forced`.
#[cfg(feature = "remote-images")]
fn body_went(dir: &Path, forced: crate::remote::ForcedFetch) -> (Doc, Layout, Images) {
    use crate::render::Pixels;
    std::fs::write(dir.join("still.png"), fixture("alpha.png")).unwrap();
    let md = "![gif](https://x.y/a.gif)\n\n![still](still.png)";
    let (doc, mut layout, mut images) = forced_page(dir, md);
    let gif = Arc::new(demo_gif(800, 600, 90));
    images.remote.forced_fetch = forced;
    images
        .remote
        .set_fetcher(Arc::new(move |_, _| Ok((Arc::new(gif.to_vec()), 800, 600))));
    images.set_policy(crate::settings::RemoteImages::Always);
    images.set_remote(true, &doc, &mut layout);
    images.set_animations(true);
    pump(
        &doc,
        &mut layout,
        &mut images,
        &BODY_WENT_BOXES[..1],
        |i, _| i.pixels(1, 1440, 1080).is_some(),
    );
    let alone = [(1, 1440, 1080, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &alone, |i| {
        anim_shows_frame(i, 1)
    });
    pump(&doc, &mut layout, &mut images, &BODY_WENT_BOXES, |i, _| {
        BODY_WENT_BOXES
            .iter()
            .all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1) {
        body_went_frame(&mut images, &doc, &mut layout);
    }
    let url: Arc<str> = "https://x.y/a.gif".into();
    assert!(images.cache.peek_bytes(&url).is_none());
    assert!(!anim_shows_frame(&mut images, 1));
    assert_eq!(images.anim_state(1), Some(AnimState::Waiting));
    (doc, layout, images)
}

/// The boxes of [`body_went`].
#[cfg(feature = "remote-images")]
const BODY_WENT_BOXES: [(u32, u32, u32); 2] = [(1, 1440, 1080), (2, 1800, 1800)];

/// A frame of [`body_went`]'s page: painted whole, its images asked for, the poll, the next
/// frame shown if it is due.
#[cfg(feature = "remote-images")]
fn body_went_frame(images: &mut Images, doc: &Doc, layout: &mut Layout) {
    place_only(images, &[(1, 1440, 1080, 0), (2, 1800, 1800, 1100)]);
    images.painted();
    images.request(&BODY_WENT_BOXES, 2);
    images.poll(doc, layout);
    let now = Instant::now();
    images.set_anim_clock(Some(now));
    if images.anim_deadline().is_some_and(|d| d <= now) {
        images.advance(now);
    }
    std::thread::sleep(Duration::from_millis(2));
}

/// sol-5 M4: a remote animation whose body was dropped for room, and that wouldn't fit beside
/// it, waits (its body is weighed with it); a click fetches its body past the limits, as a
/// click on a still does, and it plays outside the cache.
#[cfg(feature = "remote-images")]
#[test]
fn a_remote_animation_whose_body_went_waits_and_a_click_fetches_it_past_the_limits() {
    let dir = scratch("anim-body-went");
    let gif = demo_gif(800, 600, 90);
    let (doc, mut layout, mut images) = body_went(&dir, Arc::new(move |_, _| Ok(gif.clone())));
    let frame = body_went_frame;
    let key = images.key(1).unwrap();
    images.pause(1);
    images.play(1);
    assert!(images.anims.forced.contains(&key));
    let t = Instant::now();
    while !anim_shows_frame(&mut images, 1) {
        assert!(t.elapsed() < Duration::from_secs(5), "no frame");
        frame(&mut images, &doc, &mut layout);
    }
    assert!(matches!(images.anim_at(&key), Some(At::Outside(_))));
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    let _ = std::fs::remove_dir_all(&dir);
}

/// sol-6 M2: when the body a click fetches past the limits for an animation doesn't come, the
/// animation fails, not the image: its still and its size stay, it isn't fetched again by
/// itself, and a click asks again (and plays it, once the body comes).
#[cfg(feature = "remote-images")]
#[test]
fn an_animation_whose_body_fetched_past_the_limits_fails_keeps_its_still() {
    let dir = scratch("anim-body-fails");
    let gif = demo_gif(800, 600, 90);
    let (attempts, fails) = (
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    );
    let (tries, fail) = (attempts.clone(), fails.clone());
    let forced: crate::remote::ForcedFetch = Arc::new(move |_, _| {
        tries.fetch_add(1, Ordering::Relaxed);
        if fail.load(Ordering::Relaxed) {
            Err(Refusal::Failed)
        } else {
            Ok(gif.clone())
        }
    });
    let (doc, mut layout, mut images) = body_went(&dir, forced);
    let frame = body_went_frame;
    images.play(1);
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(500) {
        frame(&mut images, &doc, &mut layout);
    }
    // (Not fetched again by itself: a request made before a load's result is taken may ask for
    // it once more, and that is all.)
    let tried = attempts.load(Ordering::Relaxed);
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(500) {
        frame(&mut images, &doc, &mut layout);
    }
    assert!(tried >= 1);
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        tried,
        "fetched again by itself"
    );
    assert_eq!(layout.image_size(1), Some(ImageSize::Pixels(800, 600)));
    assert_eq!(images.anim_state(1), Some(AnimState::Failed));
    assert!(images.pixels(1, 1440, 1080).is_some(), "its still went");
    // A click asks again.
    fails.store(false, Ordering::Relaxed);
    images.play(1);
    let t = Instant::now();
    while !anim_shows_frame(&mut images, 1) {
        assert!(t.elapsed() < Duration::from_secs(5), "no frame");
        frame(&mut images, &doc, &mut layout);
    }
    assert!(attempts.load(Ordering::Relaxed) > tried);
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    let _ = std::fs::remove_dir_all(&dir);
}

/// sol-6 M3: a paused remote animation's body is let go with it. A real remote 800×600,
/// 120-frame GIF (10 MB, its player 8.8 MB) played, then paused, a local 1200×900 three-frame one
/// below it played (12.4 MB), both at 1440×1080, and a small still below that, normal limits:
/// the paused one's body is no room the local one can't have, so it is let go for it, and the
/// local one plays. (sol-7 m1: the still is small so that the local one's read fits beside the
/// paused one, which is still held when the local one's player needs the room: with a large
/// still, the read let it go before, and the test passed without the body's release.)
#[cfg(feature = "remote-images")]
#[test]
fn a_paused_remote_animation_lets_its_body_go_for_one_that_plays() {
    use crate::render::Pixels;
    let dir = scratch("anim-paused-body");
    std::fs::write(dir.join("b.gif"), demo_gif(1200, 900, 3)).unwrap();
    std::fs::write(dir.join("still.png"), fixture("alpha.png")).unwrap();
    let md = "![remote](https://x.y/a.gif)\n\n![local](b.gif)\n\n![still](still.png)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    let body = Arc::new(demo_gif(800, 600, 120));
    images.remote.set_fetcher(Arc::new(move |_, _| {
        Ok((Arc::new(body.to_vec()), 800, 600))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    images.set_remote(true, &doc, &mut layout);
    let boxes = [(1, 1440, 1080), (2, 1440, 1080), (3, 100, 100)];
    pump(&doc, &mut layout, &mut images, &boxes[..2], |i, _| {
        boxes[..2]
            .iter()
            .all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    let placed = [
        (1, 1440, 1080, 0),
        (2, 1440, 1080, 1100),
        (3, 100, 100, 2200),
    ];
    images.set_animations(true);
    images.pause(2);
    paint_placed_until(&mut images, &doc, &mut layout, &placed[..2], |i| {
        anim_shows_frame(i, 1)
    });
    let frame = |images: &mut Images, doc: &Doc, layout: &mut Layout| {
        place_only(images, &placed);
        images.painted();
        images.request(&boxes, 3);
        images.poll(doc, layout);
        std::thread::sleep(Duration::from_millis(2));
    };
    let t = Instant::now();
    while images.pixels(3, 100, 100).is_none()
        || images.worker.as_ref().is_some_and(|w| w.busy_cost() > 0)
    {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "the still doesn't load"
        );
        frame(&mut images, &doc, &mut layout);
    }
    // (It holds its body: the local one's read fits beside it, and only its player needs it.)
    assert!(images.anim_at(&images.key(1).unwrap()).is_some());
    images.pause(1);
    images.play(2);
    assert!(!images.anims.forced.contains(&Key::Local(2)));
    let mut swaps = 0;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) {
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.poll(&doc, &mut layout) || t.elapsed() < Duration::from_millis(5) {
            place_only(&mut images, &placed);
            images.painted();
            images.request(&boxes, 3);
        }
        if images.anim_deadline().is_some_and(|d| d <= now) {
            swaps += images.advance(now).iter().filter(|&&id| id == 2).count();
            place_only(&mut images, &placed);
            images.painted();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(swaps >= 3, "the local one made {swaps} frames");
    assert_eq!(images.anim_state(1), Some(AnimState::Paused));
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
    let _ = std::fs::remove_dir_all(&dir);
}

/// For `time`, a window that paints `placed` whole (and asks for `boxes`) when poll says so, else
/// the boxes of the frames shown, when they are due: the frames shown, by image (1 to 3).
#[cfg(feature = "remote-images")]
fn play_partially(
    images: &mut Images,
    doc: &Doc,
    layout: &mut Layout,
    placed: &[(u32, u32, u32, i32)],
    boxes: &[(u32, u32, u32)],
    time: Duration,
) -> [usize; 3] {
    let mut swaps = [0; 3];
    let t = Instant::now();
    while t.elapsed() < time {
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.poll(doc, layout) {
            place_only(images, placed);
            images.painted();
            images.request(boxes, boxes.len());
        }
        if images.anim_deadline().is_some_and(|d| d <= now) {
            for id in images.advance(now) {
                swaps[id as usize - 1] += 1;
            }
            place_only(images, placed);
            images.painted();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    swaps
}

/// sol-7 M1: a remote animation played past the limits from its cached body holds that body
/// (in the cache, outside the animations' limit): it is no room another animation can have. A
/// real looping 1300×1000, 40-frame remote GIF (9.9 MB) played past the limits, a local 800×600
/// ten-frame one (9.7 MB played) and a still, all at 1440×1080 in a 32 MiB cache: the local one
/// waits (it doesn't seem to play without a frame), and a click plays it past the limits.
#[cfg(feature = "remote-images")]
#[test]
fn a_body_played_past_the_limits_is_no_room_for_another_animation() {
    use crate::render::Pixels;
    let dir = scratch("anim-forced-body");
    std::fs::write(dir.join("b.gif"), demo_gif(800, 600, 10)).unwrap();
    std::fs::write(dir.join("still.png"), fixture("alpha.png")).unwrap();
    let md = "![remote](https://x.y/a.gif)\n\n![local](b.gif)\n\n![still](still.png)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    let body = Arc::new(demo_gif(1300, 1000, 40));
    images.remote.set_fetcher(Arc::new(move |_, _| {
        Ok((Arc::new(body.to_vec()), 1300, 1000))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    images.set_remote(true, &doc, &mut layout);
    pump(
        &doc,
        &mut layout,
        &mut images,
        &[(1, 1440, 1080)],
        |i, _| i.pixels(1, 1440, 1080).is_some(),
    );
    images.set_animations(true);
    let alone = [(1, 1440, 1080, 0)];
    paint_placed_until(&mut images, &doc, &mut layout, &alone, |i| {
        i.anim_state(1) == Some(AnimState::Over)
    });
    images.play(1);
    paint_placed_until(&mut images, &doc, &mut layout, &alone, |i| {
        anim_shows_frame(i, 1)
    });
    // (Played from the cached body, which it holds.)
    let url: Arc<str> = "https://x.y/a.gif".into();
    assert!(matches!(
        images.anim_at(&Key::Remote(url.clone())),
        Some(At::Outside(_))
    ));
    assert!(
        images
            .cache
            .peek_bytes(&url)
            .is_some_and(|b| Arc::strong_count(b) > 1)
    );
    let boxes = [(1, 1440, 1080), (2, 1440, 1080), (3, 1440, 1080)];
    let placed = [
        (1, 1440, 1080, 0),
        (2, 1440, 1080, 1100),
        (3, 1440, 1080, 2200),
    ];
    pump(&doc, &mut layout, &mut images, &boxes, |i, _| {
        boxes.iter().all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    // (The remote one shows its frame, but its still is painted as well: `pump` paints nothing
    // its polls see, so that still may have gone for the others', to be asked for again once
    // painted. Until it comes the local one has room, and may be read and play. The page is
    // weighed settled: the three stills cached, the local one waiting and let go.)
    let t = Instant::now();
    loop {
        place_only(&mut images, &placed);
        images.painted();
        let settled = boxes
            .iter()
            .all(|&(id, w, h)| images.key(id).is_some_and(|k| images.cache.has(&k, w, h)));
        if settled
            && images.anim_state(2) == Some(AnimState::Waiting)
            && images.anim_at(&Key::Local(2)).is_none()
        {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        images.request(&boxes, boxes.len());
        std::thread::sleep(Duration::from_millis(2));
        images.poll(&doc, &mut layout);
    }
    let swaps = play_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        &boxes,
        Duration::from_secs(2),
    );
    assert!(
        images.anims.alloc.waiting.contains(&Key::Local(2)),
        "it is let play"
    );
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    assert!(images.anim_over(2));
    assert_eq!(swaps[1], 0);
    assert!(swaps[0] >= 3, "the remote one made {} frames", swaps[0]);
    // A click plays it past the limits, beside the remote one.
    images.play(2);
    let swaps = play_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        &boxes,
        Duration::from_secs(2),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(swaps[1] >= 3, "the local one made {} frames", swaps[1]);
    assert!(swaps[0] >= 3, "the remote one made {} frames", swaps[0]);
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
}

/// The page of the stalled tests: a remote still (its body 1 MiB, which the allocation doesn't
/// weigh) above an 800×600, ten-frame local GIF, both painted (1440×1080 for the GIF), the GIF's
/// player measured and let go. The test holds the still's body, and the room is cut to the GIF
/// player's cost + half that body: the allocation plays it, but its room isn't there. The page's
/// dir (`name`), doc, layout and images, the held body, and the player's cost.
#[cfg(feature = "remote-images")]
fn stalled_page(name: &str) -> (std::path::PathBuf, Doc, Layout, Images, Bytes, usize) {
    let dir = scratch(name);
    std::fs::write(dir.join("b.gif"), demo_gif(800, 600, 10)).unwrap();
    let md = "![remote](https://x.y/s.gif)\n\n![local](b.gif)";
    let (doc, mut layout, mut images) = forced_page(&dir, md);
    let mut body = demo_gif(400, 300, 1);
    body.resize(1 << 20, 0);
    let body = Arc::new(body);
    images.remote.set_fetcher(Arc::new(move |_, _| {
        Ok((Arc::new(body.to_vec()), 400, 300))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    images.set_remote(true, &doc, &mut layout);
    pump(&doc, &mut layout, &mut images, &STALLED_BOXES, |i, _| {
        (STALLED_BOXES.iter()).all(|&(id, w, h)| i.pixels(id, w, h).is_some())
    });
    images.set_animations(true);
    play_until(&mut images, &doc, &mut layout, &STALLED_PLACED, |i| {
        anim_shows_frame(i, 2)
    });
    let key = Key::Local(2);
    let at = images.anim_at(&key).unwrap();
    images.drop_anim(at);
    let url: Arc<str> = "https://x.y/s.gif".into();
    let held = images.cache.peek_bytes(&url).unwrap().clone();
    let cost = images.anims.cost(&key, (1440, 1080)).unwrap();
    anim_room_to(&mut images, cost + Slot::bytes_cost(&url, &held) / 2);
    (dir, doc, layout, images, held, cost)
}

#[cfg(feature = "remote-images")]
const STALLED_BOXES: [(u32, u32, u32); 2] = [(1, 100, 100), (2, 1440, 1080)];
#[cfg(feature = "remote-images")]
const STALLED_PLACED: [(u32, u32, u32, i32); 2] = [(1, 100, 100, 0), (2, 1440, 1080, 200)];

/// sol-7 M1: an animation the allocation plays whose room is held by what it doesn't weigh
/// (here the test holds a remote still's body), with nothing under way to give it back, waits:
/// it doesn't say it plays while no frame comes. A click plays it past the limits.
#[cfg(feature = "remote-images")]
#[test]
fn an_animation_that_cant_be_given_its_room_waits() {
    let (dir, doc, mut layout, mut images, held, _) = stalled_page("anim-stalled");
    let (placed, boxes) = (STALLED_PLACED, STALLED_BOXES);
    let key = Key::Local(2);
    let swaps = play_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        &boxes,
        Duration::from_secs(1),
    );
    assert!(images.anims.alloc.play.contains(&key));
    assert_eq!(swaps[1], 0);
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    assert!(images.anim_over(2));
    images.play(2);
    let swaps = play_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        &boxes,
        Duration::from_secs(1),
    );
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(swaps[1] >= 3, "the local one made {} frames", swaps[1]);
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
}

/// sol-8 m1: one that waited for room it couldn't be given, paused while that room comes back,
/// plays within the limits when the reader plays it again, not past them: the old denial is
/// stale.
#[cfg(feature = "remote-images")]
#[test]
fn a_paused_animation_whose_room_came_back_plays_within_the_limits() {
    let (dir, doc, mut layout, mut images, held, cost) = stalled_page("anim-stalled-paused");
    let (placed, boxes) = (STALLED_PLACED, STALLED_BOXES);
    let key = Key::Local(2);
    let _ = play_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        &boxes,
        Duration::from_secs(1),
    );
    assert_eq!(images.anim_state(2), Some(AnimState::Waiting));
    images.pause(2);
    drop(held);
    let _ = play_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        &boxes,
        Duration::from_millis(200),
    );
    assert_eq!(images.anim_state(2), Some(AnimState::Paused));
    assert!(images.anim_room(cost, 0), "the room didn't come back");
    images.play(2);
    let forced = images.anims.forced.contains(&key);
    let swaps = play_partially(
        &mut images,
        &doc,
        &mut layout,
        &placed,
        &boxes,
        Duration::from_secs(1),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!forced, "it was played past the limits");
    assert!(images.anims.stalled.is_empty());
    assert!(swaps[1] >= 3, "the local one made {} frames", swaps[1]);
    assert_eq!(images.anim_state(2), Some(AnimState::Playing));
}

/// sol-7 M2: an animation played past the limits is made at a new box only within the forced
/// ceiling (here 24 MiB), beside the frame it shows meanwhile: a real 1300×1000, 40-frame GIF
/// played at 1440×1080 (19.1 MB), then painted at 3000×3000 (85 MB), plays on at 1440×1080; a
/// smaller box it fits at, it is made at.
#[cfg(feature = "remote-images")]
#[test]
fn an_animation_played_past_the_limits_grows_only_within_the_ceiling() {
    use crate::render::Pixels;
    let dir = scratch("anim-forced-resize");
    let (doc, mut layout, mut images) = forced_page(&dir, "![remote](https://x.y/a.gif)");
    let body = Arc::new(demo_gif(1300, 1000, 40));
    images.remote.set_fetcher(Arc::new(move |_, _| {
        Ok((Arc::new(body.to_vec()), 1300, 1000))
    }));
    images.set_policy(crate::settings::RemoteImages::Always);
    images.set_remote(true, &doc, &mut layout);
    images.set_animations(true);
    let ceiling = 24 << 20;
    images.set_forced_limits(Some(ceiling), forced::KEPT_BYTES);
    let old = [(1, 1440, 1080, 0)];
    pump(
        &doc,
        &mut layout,
        &mut images,
        &[(1, 1440, 1080)],
        |i, _| i.pixels(1, 1440, 1080).is_some(),
    );
    paint_placed_until(&mut images, &doc, &mut layout, &old, |i| {
        i.anim_state(1) == Some(AnimState::Over)
    });
    images.play(1);
    paint_placed_until(&mut images, &doc, &mut layout, &old, |i| {
        anim_shows_frame(i, 1)
    });
    let key = images.key(1).unwrap();
    let size = |i: &mut Images| {
        let at = i.anim_at(&key).unwrap();
        let p = i.playing(at);
        (p.size, p.cost, p.front().map(|b| (b.width, b.height)))
    };
    let (at_old, initial, _) = size(&mut images);
    assert!(initial <= ceiling);
    // (Played on at its box, its frames enlarged to the new one.)
    let large = [(1, 3000, 3000, 0)];
    let (mut peak, mut swaps) = (initial, 0);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1) {
        place_only(&mut images, &large);
        images.painted();
        images.poll(&doc, &mut layout);
        let (at, cost, front) = size(&mut images);
        assert_eq!(
            (at, front),
            (at_old, Some(at_old)),
            "made at a box past the ceiling"
        );
        peak = peak.max(cost);
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.anim_deadline().is_some_and(|d| d <= now) {
            swaps += images.advance(now).len();
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(peak <= ceiling, "{peak} held");
    assert!(swaps >= 3, "{swaps} frames");
    assert_eq!(images.anim_state(1), Some(AnimState::Playing));
    // A smaller box is within it: it is made there.
    let small = [(1, 720, 540, 0)];
    let t = Instant::now();
    while size(&mut images).2 == Some(at_old) {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "not made at the smaller box"
        );
        place_only(&mut images, &small);
        images.painted();
        images.poll(&doc, &mut layout);
        assert!(size(&mut images).1 <= ceiling);
        let now = Instant::now();
        images.set_anim_clock(Some(now));
        if images.anim_deadline().is_some_and(|d| d <= now) {
            images.advance(now);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
