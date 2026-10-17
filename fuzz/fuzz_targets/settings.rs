//! settings.ini text → settings and preferences: never panics, and writing back loses nothing.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vomd::settings::{Prefs, Settings};

const KEYS: &[&str] = &[
    "theme",
    "zoom",
    "toolbar",
    "toc",
    "font_size",
    "line_width",
    "font_sans",
    "font_mono",
    "font_sans_path",
    "font_mono_path",
    "remote_images",
    "wide",
    "window_width",
    "window_height",
];

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let s = Settings::parse(&text);
    let prefs = Prefs::from_settings(&s);
    // What is written reads back the same.
    let again = Settings::parse(&s.text());
    for key in KEYS {
        assert_eq!(s.get(key), again.get(key), "{key}");
    }
    assert!(Prefs::from_settings(&again) == prefs);
    // A write changes its key and nothing else.
    let mut t = s.clone();
    t.set("zoom", "150");
    let t = Settings::parse(&t.text());
    assert_eq!(t.get("zoom"), Some("150"));
    for key in KEYS.iter().filter(|k| **k != "zoom") {
        assert_eq!(s.get(key), t.get(key), "{key}");
    }
});
