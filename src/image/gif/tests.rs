use super::super::{NO_CANCEL, decode as decode_any, header_of, info_of};
use super::*;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/images")
            .join(name),
    )
    .unwrap()
}

/// `0xAARRGGBB` of `b` at `x`, `y`.
fn at(b: &Bitmap, x: u32, y: u32) -> u32 {
    b.data[(y * b.width + x) as usize]
}

/// Where `gif`'s image descriptor starts (the first `0x2C` after its color table and
/// extensions, as the fixtures here have them).
fn image_at(gif: &[u8]) -> usize {
    walk(&mut std::io::Cursor::new(&gif[2..])).unwrap().image as usize
}

/// An extension of `label` holding `len` bytes, in sub-blocks of 255.
fn extension(label: u8, len: usize) -> Vec<u8> {
    let mut out = vec![0x21, label];
    let mut left = len;
    while left > 0 {
        let n = left.min(255);
        out.push(n as u8);
        out.extend(std::iter::repeat_n(b'x', n));
        left -= n;
    }
    out.push(0);
    out
}

#[test]
fn headers_give_the_screen_and_what_the_first_frame_costs() {
    let anim = info_of(&fixture("anim.gif")).unwrap();
    assert_eq!(
        (anim.format, anim.w, anim.h, anim.interlaced),
        (Format::Gif, 6, 4, false)
    );
    assert_eq!(anim.held, 0);
    let offset = info_of(&fixture("offset.gif")).unwrap();
    assert_eq!((offset.w, offset.h, offset.interlaced), (10, 8, true));
    assert_eq!(offset.held, 4 * 6, "the interlaced frame is read whole");
    // A screen of 0 × 0 takes the first frame's size.
    assert_eq!(
        header_of(&fixture("zero-screen.gif")),
        Some((Format::Gif, 3, 2))
    );
    // Cut short anywhere before the image descriptor's end, or not a GIF: none.
    let gif = fixture("offset.gif");
    for cut in 0..image_at(&gif) + 10 {
        assert_eq!(header_of(&gif[..cut]), None, "cut at {cut}");
    }
    for junk in [&b"GIF88a\x01\0\x01\0\0\0\0"[..], b"GIF89", b"GIF89a"] {
        assert_eq!(header_of(junk), None);
    }
    // No image at all, or a frame with no pixels.
    let mut none = gif[..image_at(&gif)].to_vec();
    none.push(0x3B);
    assert_eq!(header_of(&none), None);
    let mut empty = gif.clone();
    let i = image_at(&gif);
    empty[i + 5..i + 7].copy_from_slice(&0u16.to_le_bytes());
    assert_eq!(header_of(&empty), None);
}

#[test]
fn the_first_frame_is_shown_with_its_transparency() {
    let anim = decode_any(&fixture("anim.gif"), 6, 4).unwrap();
    for y in 0..4 {
        for x in 0..6 {
            let want = if x < 3 { 0xFFFF_0000 } else { 0 };
            assert_eq!(
                at(&anim, x, y),
                want,
                "{x}, {y}: the first frame, not the blue one"
            );
        }
    }
    // Interlaced, at an offset, with its own palette: rows red, green, blue, yellow, white,
    // black, from row 1 and column 3, on a clear screen.
    let offset = decode_any(&fixture("offset.gif"), 10, 8).unwrap();
    let rows = [
        0xFFFF_0000,
        0xFF00_FF00,
        0xFF00_00FF,
        0xFFFF_FF00,
        0xFFFF_FFFF,
        0xFF00_0000,
    ];
    for y in 0..8 {
        for x in 0..10 {
            let inside = (3..7).contains(&x) && (1..7).contains(&y);
            let want = if inside { rows[y as usize - 1] } else { 0 };
            assert_eq!(at(&offset, x, y), want, "{x}, {y}");
        }
    }
    let zero = decode_any(&fixture("zero-screen.gif"), 3, 2).unwrap();
    assert!(zero.data.iter().all(|&p| p == 0xFFFF_0000));
    // Resampled like any image.
    let small = decode_any(&fixture("anim.gif"), 2, 1).unwrap();
    assert_eq!(small.data, [0xFFFF_0000, 0]);
}

/// Extensions before the image are not given to the decoder: a big XMP block, an ICC profile,
/// a comment and one of a kind it doesn't know all decode the same, and the graphic control's
/// transparency stays.
#[test]
fn extensions_before_the_image_are_left_out() {
    let anim = fixture("anim.gif");
    let want = decode_any(&anim, 6, 4).unwrap();
    let i = image_at(&anim);
    let control = anim[..i]
        .windows(2)
        .rposition(|w| w == [0x21, 0xF9])
        .unwrap();
    let mut xmp = vec![0x21, 0xFF, 11];
    xmp.extend(b"XMP DataXMP");
    xmp.extend(&extension(0, 200_000)[2..]);
    let mut icc = vec![0x21, 0xFF, 11];
    icc.extend(b"ICCRGBG1012");
    icc.extend(&extension(0, 100_000)[2..]);
    for put in [xmp, icc, extension(0xFE, 70_000), extension(0x99, 10)] {
        let mut gif = anim[..control].to_vec();
        gif.extend(&put);
        gif.extend(&anim[control..]);
        assert_eq!(header_of(&gif), Some((Format::Gif, 6, 4)));
        assert_eq!(decode_any(&gif, 6, 4).as_ref(), Some(&want));
    }
}

/// A first frame that reaches past the screen makes the screen bigger, as in a browser.
#[test]
fn a_frame_past_the_screen_makes_it_bigger() {
    let mut gif = fixture("offset.gif");
    gif[6..10].copy_from_slice(&[5, 0, 3, 0]);
    assert_eq!(header_of(&gif), Some((Format::Gif, 7, 7)));
    let b = decode_any(&gif, 7, 7).unwrap();
    assert_eq!(at(&b, 3, 1), 0xFFFF_0000);
    assert_eq!(at(&b, 6, 6), 0xFF00_0000);
    assert_eq!(at(&b, 2, 1), 0);
}

#[test]
fn broken_or_cut_gifs_are_refused() {
    for name in ["anim.gif", "offset.gif", "zero-screen.gif"] {
        let gif = fixture(name);
        let i = image_at(&gif);
        // Cut in the first frame's data, or with garbage for it.
        let flags = gif[i + 9];
        let mut end = i
            + 10
            + if flags & 0x80 != 0 {
                3 << ((flags & 7) + 1)
            } else {
                0
            }
            + 1;
        while gif[end] != 0 {
            end += 1 + usize::from(gif[end]);
        }
        for cut in i + 10..end - 1 {
            assert_eq!(decode_any(&gif[..cut], 2, 2), None, "{name} cut at {cut}");
        }
        let mut junk = gif[..i + 10].to_vec();
        junk.extend([2, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0x3B]);
        assert_eq!(decode_any(&junk, 2, 2), None, "{name}: garbage");
    }
}

#[test]
fn a_cancelled_decode_stops() {
    let stop = AtomicBool::new(true);
    for name in ["anim.gif", "offset.gif"] {
        let gif = fixture(name);
        assert_eq!(decode(&gif, 4, 4, Sampling::Area, &stop), None, "{name}");
        assert!(
            decode(&gif, 4, 4, Sampling::Area, &NO_CANCEL).is_some(),
            "{name}"
        );
    }
}
