//! Bytes → image header, decode and EXIF orientation: never panics, never more than a header
//! says, and the size decoded is the box asked for.
//!
//! Rasters are decoded up to 4,000,000 pixels, not VOMD's 64,000,000 (`MAX_PIXELS`): a JPEG of
//! 1.5 KB can claim 4149 × 6144, and decoding that three times took over the 10 s an input is
//! given under ASan (a photo of that size takes as long, so it isn't a slow path). Above the
//! limit, the header and the refusal are what is tested; the decoders run the same code on a
//! smaller image, and a side of `MAX_SOURCE_SIDE` still fits (65,535 × 61). SVGs are drawn under
//! the normal limits.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vomd::image::{Limits, NO_CANCEL, decode_with, exif_orientation, header_of};

const LIMITS: Limits = Limits {
    max_pixels: 4_000_000,
    ..Limits::normal()
};

fuzz_target!(|data: &[u8]| {
    assert!((1..=8).contains(&exif_orientation(data)));
    let Some((_, w, h)) = header_of(data) else {
        return;
    };
    assert!(w > 0 && h > 0);
    for (bw, bh) in [(1, 1), (7, 5), (w.min(64), h.min(64))] {
        if let Some(b) = decode_with(data, bw, bh, &LIMITS, &NO_CANCEL) {
            assert_eq!((b.width, b.height), (bw, bh));
            assert_eq!(b.data.len(), (bw * bh) as usize);
        }
    }
});
