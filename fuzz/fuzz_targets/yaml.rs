//! Bytes → front matter (the lines between its fences), read as YAML and made into its table or
//! its error: never panics, the text it makes stays proportional to the input (an outline adds
//! at most a line's indentation and a bullet or separator per node), and an input stays fast
//! (see [`BUDGET`]).
#![no_main]

use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use vomd::doc::Doc;

/// The most reading and building may take. The reader goes through the text once, at hundreds
/// of MB/s in a release build, so 64 KiB takes well under a millisecond; this build's
/// instrumentation and AddressSanitizer on a CI runner are many times slower, so the bound is
/// loose: it is there to catch work that grows faster than the text.
const BUDGET: Duration = Duration::from_millis(500);

fn check(yaml: &str) -> Duration {
    let t = Instant::now();
    if let Some(bytes) = Doc::fuzz_front_matter(yaml) {
        assert!(
            bytes <= 256 * yaml.len() + 4096,
            "{bytes} bytes of text from {} bytes",
            yaml.len()
        );
    }
    t.elapsed()
}

fuzz_target!(|data: &[u8]| {
    let mut yaml = String::from_utf8_lossy(data).into_owned();
    // What VOMD reads: whole lines, none of them a closing `---` (which would end the front
    // matter there).
    if yaml.lines().any(|l| l.trim_end_matches([' ', '\t']) == "---") {
        return;
    }
    if !yaml.is_empty() && !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    // Once more if too slow: a slow run can be the machine's.
    if check(&yaml) > BUDGET {
        let again = check(&yaml);
        assert!(again <= BUDGET, "{} ms", again.as_secs_f64() * 1e3);
    }
});
