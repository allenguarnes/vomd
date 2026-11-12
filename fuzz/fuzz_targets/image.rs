//! Bytes → image header, decode and EXIF orientation: never panics, never more than a header
//! says, and the size decoded is the box asked for.
//!
//! Rasters are decoded up to 4,000,000 pixels, not VOMD's 64,000,000 (`MAX_PIXELS`): a JPEG of
//! 1.5 KB can claim 4149 × 6144, and decoding that three times took over the 10 s an input is
//! given under ASan (a photo of that size takes as long, so it isn't a slow path). Above the
//! limit, the header and the refusal are what is tested; the decoders run the same code on a
//! smaller image, and a side of `MAX_SOURCE_SIDE` still fits (65,535 × 61). SVGs are drawn under
//! the normal limits. A part of an image (the image viewer's full resolution) is what decoding
//! all of it makes there.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vomd::image::{
    Format, Limits, NO_CANCEL, decode_part, decode_with, exif_orientation, header_of,
};

const LIMITS: Limits = Limits {
    max_pixels: 4_000_000,
    ..Limits::normal()
};

fuzz_target!(|data: &[u8]| {
    assert!((1..=8).contains(&exif_orientation(data)));
    let Some((format, w, h)) = header_of(data) else {
        return;
    };
    assert!(w > 0 && h > 0);
    // Boxes smaller than the image, near its own size, and larger than it (the image viewer's).
    let boxes = [
        (1, 1),
        (7, 5),
        (w.min(64), h.min(64)),
        (w.min(512), h.min(512)),
        (w.min(64) * 3, h.min(64) * 3),
    ];
    for (bw, bh) in boxes {
        if let Some(b) = decode_with(data, bw, bh, &LIMITS, &NO_CANCEL) {
            assert_eq!((b.width, b.height), (bw, bh));
            assert_eq!(b.data.len(), (bw * bh) as usize);
        }
    }
    // A part of the image at a size (the viewer's full resolution): a raster's is the same
    // pixels as that part of all of it.
    let full = (w.min(256), h.min(256));
    let (at, size) = (
        (full.0 / 3, full.1 / 3),
        ((full.0 / 2).max(1), (full.1 / 2).max(1)),
    );
    if let Some(part) = decode_part(data, full, at, size, &LIMITS) {
        assert_eq!((part.width, part.height), size);
        if format != Format::Svg
            && let Some(all) = decode_with(data, full.0, full.1, &LIMITS, &NO_CANCEL)
        {
            for (y, row) in part.data.chunks(size.0 as usize).enumerate() {
                let from = (at.1 as usize + y) * full.0 as usize + at.0 as usize;
                assert_eq!(row, &all.data[from..from + size.0 as usize]);
            }
        }
    }
});
