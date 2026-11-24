//! Bytes → SVG header, then parsing, the counts and the estimate, and drawing at 64 × 64 and
//! 512 × 512 within the normal limits: never panics, never hangs (run with `-timeout=5`), and
//! the size drawn is the box asked for. Then at 64 × 64 with text: in VOMD's faces and the
//! tests' color fonts as fallbacks (`fixtures/fonts`: a COLR font with a ligature, and a CBDT
//! one), as the window draws it once a scan has added faces.
#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use vomd::fonts::Fonts;
use vomd::image::{Format, SvgJob, decode, header_of};

static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let mut fonts = Fonts::new().unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fonts");
    for name in ["test-emoji-colr.ttf", "test-emoji-cbdt.ttf"] {
        fonts.add_fallback(&dir.join(name), 0).expect(name);
    }
    fonts
});

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
    // (A job charged too little is run again with what it said it needs, as the window does.)
    let mut job = SvgJob::of(data).unwrap().with_fonts(&FONTS);
    for _ in 0..4 {
        if let Some(drawn) = job.run(data, 64, 64) {
            if let Some(b) = drawn {
                assert_eq!((b.width, b.height, b.data.len()), (64, 64, 64 * 64));
            }
            break;
        }
    }
});
