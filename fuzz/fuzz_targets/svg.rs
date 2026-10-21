//! Bytes → SVG header, then parsing, the counts and the estimate, and drawing at 64 × 64 and
//! 512 × 512 within the normal limits: never panics, never hangs (run with `-timeout=2`), and
//! the size drawn is the box asked for.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vomd::image::{Format, decode, header_of};

fuzz_target!(|data: &[u8]| {
    let Some((Format::Svg, w, h)) = header_of(data) else {
        return;
    };
    assert!(w > 0 && h > 0);
    for (bw, bh) in [(64, 64), (512, 512)] {
        if let Some(b) = decode(data, bw, bh) {
            assert_eq!((b.width, b.height), (bw, bh));
            assert_eq!(b.data.len(), (bw * bh) as usize);
        }
    }
});
