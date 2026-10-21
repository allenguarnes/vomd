use super::super::{Limits, NO_CANCEL, decode as decode_any, header_of, info_of};
use super::*;

// Shared with tests/image_memory.rs.
#[path = "many_groups.rs"]
mod many_groups;
use many_groups::many_groups;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/images")
            .join(name),
    )
    .unwrap()
}

/// Red, green, blue and alpha of a premultiplied `0xAARRGGBB`.
fn rgba(p: u32) -> [u32; 4] {
    [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF, p >> 24]
}

/// Whether `p` is within `d` of `want` in every channel.
fn near(p: u32, want: [u32; 4], d: u32) -> bool {
    rgba(p).iter().zip(want).all(|(a, b)| a.abs_diff(b) <= d)
}

#[test]
fn headers_give_the_size_and_what_decoding_holds() {
    for (name, size, channels) in [
        ("lossy.webp", (6, 4), 3),
        ("lossy-alpha.webp", (6, 4), 4),
        ("lossless-alpha.webp", (6, 4), 4),
        ("anim.webp", (6, 4), 4),
        ("big-lossy.webp", (1024, 768), 3),
        ("big-anim.webp", (1024, 768), 4),
    ] {
        let h = info_of(&fixture(name)).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(
            (h.format, h.w, h.h),
            (Format::Webp, size.0, size.1),
            "{name}"
        );
        assert_eq!(h.out_channels, channels, "{name}");
        assert!(
            h.held >= 2 * h.w as usize * h.h as usize,
            "{name}: {}",
            h.held
        );
        assert_eq!(h.orientation, 1, "{name}");
    }
    // EXIF orientation 5 swaps the sides.
    let exif = info_of(&fixture("exif-5.webp")).unwrap();
    assert_eq!(exif.orientation, 5);
    assert_eq!(
        header_of(&fixture("exif-5.webp")),
        Some((Format::Webp, 4, 6))
    );
    // Cut short in the header, or not a WebP.
    let lossy = fixture("lossy.webp");
    for cut in 0..30 {
        assert_eq!(header_of(&lossy[..cut]), None, "cut at {cut}");
    }
    for junk in [
        &b"RIFF\0\0\0\0WEBPVP9 \0\0\0\0"[..],
        b"RIFF\0\0\0\0WAVEfmt ",
        b"RIFX",
    ] {
        assert_eq!(header_of(junk), None);
    }
}

#[test]
fn every_kind_decodes() {
    let (red, blue, clear) = ([255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 0, 0]);
    let lossy = decode_any(&fixture("lossy.webp"), 6, 4).unwrap();
    assert!(near(lossy.data[0], red, 40), "{:08x}", lossy.data[0]);
    assert!(near(lossy.data[5], blue, 40), "{:08x}", lossy.data[5]);
    for name in ["lossy-alpha.webp", "lossless-alpha.webp", "anim.webp"] {
        let b = decode_any(&fixture(name), 6, 4).unwrap();
        // Red on the left, clear on the right (the first frame of the animation, not the blue
        // second one).
        for y in 0..4 {
            for x in 0..6 {
                let (want, d) = if x < 3 { (red, 40) } else { (clear, 8) };
                let p = b.data[y * 6 + x];
                assert!(near(p, want, d), "{name} at {x}, {y}: {p:08x}");
            }
        }
    }
    // Transposed by its EXIF orientation: the red half is on top.
    let exif = decode_any(&fixture("exif-5.webp"), 4, 6).unwrap();
    assert_eq!((exif.width, exif.height), (4, 6));
    assert!(near(exif.data[0], red, 40) && near(exif.data[23], blue, 40));
    let big = decode_any(&fixture("big-lossy-alpha.webp"), 100, 75).unwrap();
    assert!(
        near(big.data[0], [0x1A, 0x33, 0x4D, 0x80], 8),
        "{:08x}",
        big.data[0]
    );
}

#[test]
fn broken_webps_are_refused() {
    for name in [
        "lossy.webp",
        "lossless-alpha.webp",
        "lossy-alpha.webp",
        "anim.webp",
    ] {
        let webp = fixture(name);
        let h = info_of(&webp).unwrap();
        // Cut short past the header, in the first frame.
        let end = if name == "anim.webp" {
            86
        } else {
            webp.len() - 4
        };
        for cut in (34..end).step_by(3) {
            let b = &webp[..cut];
            if info_of(b).is_some() {
                assert!(
                    decode(b, &h, 6, 4, Sampling::Area, &Limits::normal(), &NO_CANCEL).is_err(),
                    "{name} cut at {cut}"
                );
            }
        }
        // A header that says another size than the image has.
        let mut other = h;
        other.w += 1;
        assert_eq!(
            decode(
                &webp,
                &other,
                6,
                4,
                Sampling::Area,
                &Limits::normal(),
                &NO_CANCEL
            ),
            Err(None),
            "{name}"
        );
        let stop = AtomicBool::new(true);
        assert_eq!(
            decode(&webp, &h, 6, 4, Sampling::Area, &Limits::normal(), &stop),
            Ok(None),
            "{name}: cancelled"
        );
    }
}

/// Decodes `bytes` with image-webp, the Huffman codes given `limit` bytes.
fn decode_limited(bytes: &[u8], limit: usize) -> Result<Vec<u8>, image_webp::DecodingError> {
    let mut d = image_webp::WebPDecoder::new(Cursor::new(bytes))?;
    d.set_huffman_limit(limit);
    let mut data = vec![0; d.output_buffer_size().unwrap()];
    d.read_image(&mut data).map(|()| data)
}

#[test]
fn huffman_codes_are_bounded() {
    // 40 groups of codes, 800 KB: within the limit, and decoded.
    let fits = many_groups(40, 4);
    assert_eq!(decode_limited(&fits, usize::MAX).unwrap(), [0; 48]);
    assert_eq!(decode_limited(&fits, huffman_limit(4, 4)).unwrap(), [0; 48]);
    let b = decode_any(&fits, 4, 4).unwrap();
    assert!(b.data.iter().all(|&p| p == 0xFF00_0000));
    // 1,024 groups, 20 MB from 59 KB: a valid image, whose decode fails at the limit.
    let over = many_groups(1024, 4);
    assert!(over.len() < 60 << 10, "{}", over.len());
    assert!(decode_limited(&over, usize::MAX).is_ok());
    assert!(matches!(
        decode_limited(&over, huffman_limit(4, 4)),
        Err(image_webp::DecodingError::MemoryLimitExceeded)
    ));
    assert!(decode_any(&over, 4, 4).is_none());
    // 65,536 groups, 1.3 GB of tables from 3.7 MB: refused for their number, before one is read,
    // and quickly.
    let most = many_groups(65_536, 4);
    let start = std::time::Instant::now();
    assert!(matches!(
        decode_limited(&most, huffman_limit(4, 4)),
        Err(image_webp::DecodingError::MemoryLimitExceeded)
    ));
    assert!(decode_any(&most, 4, 4).is_none());
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    // The fuzz seed is the image over the limit.
    let seed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/seeds/image/webp-lossless-many-huffman-groups.bin");
    assert_eq!(std::fs::read(seed).unwrap(), over);
}

#[test]
fn the_decoder_refuses_an_animation_frame_of_another_size() {
    // `header` refuses it first; the vendored decoder doesn't panic on it either.
    let seed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/seeds/image/webp-animation-alpha-frame-of-another-size.bin");
    let bytes = std::fs::read(seed).unwrap();
    assert!(info_of(&bytes).is_none());
    assert!(matches!(
        decode_limited(&bytes, usize::MAX),
        Err(image_webp::DecodingError::InconsistentImageSizes)
    ));
}

/// The chunks of a WebP file (tag and data), without their padding.
fn chunks(webp: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 12;
    while i < webp.len() {
        let len = u32::from_le_bytes(webp[i + 4..i + 8].try_into().unwrap()) as usize;
        out.push((
            webp[i..i + 4].try_into().unwrap(),
            webp[i + 8..i + 8 + len].to_vec(),
        ));
        i += 8 + len + (len & 1);
    }
    out
}

/// A WebP file of `chunks`.
fn riff(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    for (tag, data) in chunks {
        body.extend(tag);
        body.extend((data.len() as u32).to_le_bytes());
        body.extend(data);
        body.extend(vec![0; data.len() & 1]);
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

#[test]
fn exif_before_the_image_is_read() {
    let mut c = chunks(&fixture("exif-5.webp"));
    let exif = c.pop().unwrap();
    c.insert(1, exif);
    let moved = riff(&c);
    assert_eq!(header_of(&moved), Some((Format::Webp, 4, 6)));
    assert_eq!(info_of(&moved).unwrap().orientation, 5);
    assert!(decode_any(&moved, 4, 6).is_some());
}

#[test]
fn an_animations_first_frame_is_charged_wherever_it_is() {
    let base = chunks(&fixture("big-anim.webp"));
    let plain = info_of(&riff(&base)).unwrap().held;
    // A frame with a MB of data more than it needs: its decoder copies the data.
    let mut padded = base.clone();
    padded[2].1.extend(vec![0; 1 << 20]);
    let charge = plain + 3 * (1 << 20);
    assert!(info_of(&riff(&padded)).unwrap().held >= charge);
    // The same behind an EXIF chunk (the flag set), which is read all the same.
    let tiff = chunks(&fixture("exif-5.webp")).pop().unwrap();
    padded[0].1[0] |= 0x08;
    padded.insert(2, tiff);
    let behind = info_of(&riff(&padded)).unwrap();
    assert!(behind.held >= charge, "{}", behind.held);
    assert_eq!(behind.orientation, 5);
    // A frame that isn't found, for more chunks before it than are looked through, or none that
    // is a frame, is not charged nothing: it isn't loaded.
    let mut far = base.clone();
    far.insert(2, (*b"JUNK", vec![0; 5 << 20]));
    assert!(info_of(&riff(&far)).is_none());
    let mut still = chunks(&fixture("exif-5.webp"));
    still[0].1[0] |= 0x02;
    assert!(info_of(&riff(&still)).is_none());
    // A frame that says it has more data than the file has is charged what the file has.
    let mut lying = riff(&base);
    lying[48..52].copy_from_slice(&(100u32 << 20).to_le_bytes());
    let held = info_of(&lying).unwrap().held;
    assert!(held < plain + (1 << 20), "{held}");
    // A big image behind many bytes of metadata: the image's own bytes aren't looked through.
    let mut big = chunks(&fixture("big-lossy.webp"));
    big.insert(
        0,
        (
            *b"VP8X",
            [
                &[0u8; 4][..],
                &1023u32.to_le_bytes()[..3],
                &767u32.to_le_bytes()[..3],
            ]
            .concat(),
        ),
    );
    big[1].1.extend(vec![0; 5 << 20]);
    assert_eq!(
        header_of(&riff(&big)).map(|h| (h.1, h.2)),
        Some((1024, 768))
    );
}

/// The `VP8 ` chunk's data of the lossy fixture, its frame header saying `w` × `h`.
fn vp8_of_size(w: u16, h: u16) -> Vec<u8> {
    let mut vp8 = chunks(&fixture("lossy.webp")).pop().unwrap().1;
    vp8[6..8].copy_from_slice(&w.to_le_bytes());
    vp8[8..10].copy_from_slice(&h.to_le_bytes());
    vp8
}

/// A `VP8X` chunk's data: `flags`, and a canvas of `w` × `h`.
fn vp8x(flags: u8, w: u32, h: u32) -> Vec<u8> {
    [
        &[flags, 0, 0, 0][..],
        &(w - 1).to_le_bytes()[..3],
        &(h - 1).to_le_bytes()[..3],
    ]
    .concat()
}

/// The frame header of an `ANMF` chunk, at the corner, `w` × `h`, and then `rest`.
fn anmf(w: u32, h: u32, rest: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = [
        &[0u8; 6][..],
        &(w - 1).to_le_bytes()[..3],
        &(h - 1).to_le_bytes()[..3],
        &[0; 4],
    ]
    .concat();
    for (tag, data) in rest {
        out.extend(tag);
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out.extend(vec![0; data.len() & 1]);
    }
    out
}

#[test]
fn a_lossy_frame_is_the_size_the_file_says_before_it_is_decoded() {
    // The header costs a canvas of 1 × 1, a frame says 16383 × 16383 (400 MB of planes).
    let lying = vp8_of_size(16_383, 16_383);
    let still = riff(&[(*b"VP8X", vp8x(0, 1, 1)), (*b"VP8 ", lying.clone())]);
    let alpha = chunks(&fixture("lossy-alpha.webp"));
    let anim = chunks(&fixture("anim.webp")).swap_remove(1);
    let still_alpha = riff(&[
        (*b"VP8X", vp8x(0x10, 6, 4)),
        alpha[1].clone(),
        (*b"VP8 ", lying.clone()),
    ]);
    let frame = riff(&[
        (*b"VP8X", vp8x(0x02, 1, 1)),
        anim.clone(),
        (*b"ANMF", anmf(1, 1, &[(*b"VP8 ", lying.clone())])),
    ]);
    let frame_alpha = riff(&[
        (*b"VP8X", vp8x(0x12, 1, 1)),
        anim.clone(),
        (
            *b"ANMF",
            anmf(1, 1, &[alpha[1].clone(), (*b"VP8 ", lying.clone())]),
        ),
    ]);
    for (what, bytes) in [
        ("still", &still),
        ("still with alpha", &still_alpha),
        ("frame", &frame),
        ("frame with alpha", &frame_alpha),
    ] {
        assert!(
            matches!(
                decode_limited(bytes, usize::MAX),
                Err(image_webp::DecodingError::InconsistentImageSizes)
            ),
            "{what}"
        );
        assert!(decode_any(bytes, 1, 1).is_none(), "{what}");
    }
    // The same files with the size they say are decoded.
    let right = vp8_of_size(6, 4);
    for (what, bytes) in [
        (
            "still",
            riff(&[(*b"VP8X", vp8x(0, 6, 4)), (*b"VP8 ", right.clone())]),
        ),
        (
            "frame",
            riff(&[
                (*b"VP8X", vp8x(0x02, 6, 4)),
                anim.clone(),
                (*b"ANMF", anmf(6, 4, &[(*b"VP8 ", right.clone())])),
            ]),
        ),
    ] {
        assert!(decode_any(&bytes, 6, 4).is_some(), "{what}");
    }
}
