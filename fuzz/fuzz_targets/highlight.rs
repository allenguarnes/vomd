//! Bytes → a code block, lexed: never panics, the runs are non-empty, contiguous, cover the
//! text and fall on character boundaries, a cut at a line end (where VOMD cuts an oversized
//! block) changes nothing, and an input stays fast (see [`BUDGET`]). The first byte picks the
//! language, the next four are a state to start from (any `u32` must be safe), the sixth where
//! to cut, and the rest is the text.
#![no_main]

use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use vomd::highlight::{LANGS, Tok, lex};

/// The most lexing an input may take, all three times. The lexer does about 100 MB/s in a
/// release build, so 16 KiB takes well under a millisecond; this build's instrumentation and
/// checks are many times slower, and AddressSanitizer on a CI runner slower still, so the bound
/// is loose: it is there to catch work that grows faster than the text.
const BUDGET: Duration = Duration::from_millis(200);

/// Lex and check the runs; the runs and the state at the end.
fn runs(lang: u8, text: &str, state: u32) -> (Vec<(usize, usize, Tok)>, u32) {
    let mut out: Vec<(usize, usize, Tok)> = Vec::new();
    let end = lex(lang, text, state, &mut |s, e, t| {
        assert_eq!(s, out.last().map_or(0, |r| r.1), "a gap or an overlap");
        assert!(e > s, "an empty run");
        assert!(e <= text.len() && text.is_char_boundary(s) && text.is_char_boundary(e));
        assert!(out.last().is_none_or(|r| r.2 != t), "runs not merged");
        out.push((s, e, t));
    });
    assert_eq!(out.last().map_or(0, |r| r.1), text.len(), "not covered");
    (out, end)
}

/// Lex from `state`, then from a block's start whole and cut at the first line end at or after
/// a point picked by `c`: the time it took.
fn check(lang: u8, text: &str, state: u32, c: u8) -> Duration {
    let t = Instant::now();
    runs(lang, text, state);
    let (whole, end) = runs(lang, text, 0);
    let from = text.len() * usize::from(c) / 256;
    if let Some(p) = text.as_bytes()[from..].iter().position(|&b| b == b'\n') {
        let cut = from + p + 1;
        let (mut joined, state) = runs(lang, &text[..cut], 0);
        let (tail, tail_end) = runs(lang, &text[cut..], state);
        for (s, e, t) in tail {
            match joined.last_mut() {
                Some(last) if last.2 == t && last.1 == cut + s => last.1 = cut + e,
                _ => joined.push((cut + s, cut + e, t)),
            }
        }
        assert_eq!(joined, whole, "cut at {cut}");
        assert_eq!(tail_end, end, "cut at {cut}");
    }
    t.elapsed()
}

fuzz_target!(|data: &[u8]| {
    let Some((&[l, s0, s1, s2, s3, c], rest)) = data.split_first_chunk() else {
        return;
    };
    let lang = (usize::from(l) % LANGS.len()) as u8 + 1;
    let state = u32::from_le_bytes([s0, s1, s2, s3]);
    let text = String::from_utf8_lossy(rest);
    // Once more if too slow: a slow run can be the machine's.
    if check(lang, &text, state, c) > BUDGET {
        let again = check(lang, &text, state, c);
        assert!(again <= BUDGET, "{} ms", again.as_secs_f64() * 1e3);
    }
});
