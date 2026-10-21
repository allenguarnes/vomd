//! Bytes → image header, decode and EXIF orientation: never panics, never more than a header
//! says, and the size decoded is the box asked for.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vomd::image::{decode, exif_orientation, header_of};

fuzz_target!(|data: &[u8]| {
    assert!((1..=8).contains(&exif_orientation(data)));
    let Some((_, w, h)) = header_of(data) else {
        return;
    };
    assert!(w > 0 && h > 0);
    for (bw, bh) in [(1, 1), (7, 5), (w.min(64), h.min(64))] {
        if let Some(b) = decode(data, bw, bh) {
            assert_eq!((b.width, b.height), (bw, bh));
            assert_eq!(b.data.len(), (bw * bh) as usize);
        }
    }
});
