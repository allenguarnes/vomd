//! Bytes → Doc IR: never panics, and the segmented parse equals the one parser's.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vomd::doc::Doc;

fuzz_target!(|data: &[u8]| {
    // The first byte picks the segment size, so small inputs are cut too (documents are cut
    // only past 1 MB in use, at about 16 KB).
    let Some((&b, rest)) = data.split_first() else {
        return;
    };
    let src = String::from_utf8_lossy(rest);
    let doc = Doc::parse(&src);
    for span in &doc.spans {
        let _ = doc.span_text(span);
    }
    let budget = 16 + usize::from(b) * 8;
    if let Some((whole, parts)) = Doc::fuzz_segments(&src, budget) {
        assert!(whole == parts, "segmented parse at {budget} differs");
    }
});
