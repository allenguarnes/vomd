use super::*;

#[test]
fn parses_and_ignores_garbage() {
    let s = Settings::parse(
        "# comment\n\
         theme = dark\n\
         ; also a comment\n\
         garbage line\n\
         = no key\n\
         bad key = 1\n\
         zoom=150\n\
         \tfont_size\t=\t18  \n\
         empty =\n\
         path = /a/b#c.ttf\n",
    );
    assert_eq!(s.get("theme"), Some("dark"));
    assert_eq!(s.get("zoom"), Some("150"));
    assert_eq!(s.get("font_size"), Some("18"));
    assert_eq!(s.get("empty"), Some(""));
    assert_eq!(
        s.get("path"),
        Some("/a/b#c.ttf"),
        "# only starts a comment at line start"
    );
    assert_eq!(s.get("bad key"), None);
    assert_eq!(s.get("garbage line"), None);
    assert_eq!(s.get("missing"), None);
}

#[test]
fn later_lines_win_and_receive_writes() {
    let mut s = Settings::parse("zoom = 120\nzoom = 130\n");
    assert_eq!(s.get("zoom"), Some("130"));
    assert!(s.set("zoom", "140"));
    assert_eq!(s.text(), "zoom = 120\nzoom = 140\n");
}

#[test]
fn round_trip_preserves_everything_but_the_changed_value() {
    let text = "# My settings\n\n  theme   =   light   \n# zoom = 100\nunknown_key = keep me\nnot a setting\nzoom = 110\n";
    let mut s = Settings::parse(text);
    assert_eq!(s.text(), text, "unchanged text round-trips");
    assert!(!s.set("theme", "light"), "same value: no change");
    assert!(s.set("theme", "dark"));
    assert!(s.set("zoom", "120"));
    assert!(s.set("window_width", "800"));
    assert_eq!(
        s.text(),
        "# My settings\n\n  theme   =   dark   \n# zoom = 100\nunknown_key = keep me\nnot a setting\nzoom = 120\nwindow_width = 800\n"
    );
    assert!(s.set("unknown_key", "x"));
    assert_eq!(Settings::parse(&s.text()).get("unknown_key"), Some("x"));
}

#[test]
fn setting_an_empty_value() {
    let mut s = Settings::parse("zoom =\nfont_size=\n");
    assert!(s.set("zoom", "90"));
    assert!(s.set("font_size", "20"));
    assert_eq!(s.text(), "zoom = 90\nfont_size= 20\n");
    let p = Prefs::from_settings(&s);
    assert_eq!((p.zoom, p.font_size), (90, 20.0));
}

#[test]
fn defaults_are_not_written() {
    let mut s = Settings::default();
    assert!(!s.set_unless_default("zoom", "100", "100"));
    assert_eq!(s.text(), "");
    assert!(s.set_unless_default("zoom", "110", "100"));
    // Once present, going back to the default is a real change.
    assert!(s.set_unless_default("zoom", "100", "100"));
    assert_eq!(s.get("zoom"), Some("100"));
}

#[test]
fn prefs_defaults_and_bad_values() {
    assert_eq!(Prefs::from_settings(&Settings::default()), Prefs::default());
    let s = Settings::parse(
        "theme = purple\nzoom = 999\ntoolbar = maybe\ntoc = yes\nwindow_width = 50\n\
         window_height = 700\nremote_images = ALWAYS\nfont_size = NaN\nline_width = 120\n\
         font_sans = Noto Sans\nfont_mono = default\n",
    );
    let p = Prefs::from_settings(&s);
    let d = Prefs::default();
    assert_eq!(p.theme, d.theme);
    assert_eq!(p.zoom, d.zoom, "out of range is skipped");
    assert_eq!(p.toolbar, d.toolbar);
    assert!(p.toc);
    assert_eq!(p.window, (d.window.0, 700));
    assert_eq!(p.remote_images, RemoteImages::Always);
    assert_eq!(p.font_size, d.font_size);
    assert_eq!(p.line_width, 120.0);
    assert_eq!(p.font_sans.as_deref(), Some("Noto Sans"));
    assert_eq!(p.font_mono, None);
    for (v, want) in [
        ("Dark", ThemeMode::Dark),
        ("LIGHT", ThemeMode::Light),
        ("system", ThemeMode::System),
    ] {
        let p = Prefs::from_settings(&Settings::parse(&format!("theme = {v}")));
        assert_eq!(p.theme, want);
        assert_eq!(theme_value(want), v.to_ascii_lowercase());
    }
    for (v, ok) in [
        ("50", true),
        ("300", true),
        ("49", false),
        ("301", false),
        ("1e2", false),
        ("-0", false),
    ] {
        let p = Prefs::from_settings(&Settings::parse(&format!("zoom = {v}")));
        assert_eq!(p.zoom != 100 || v == "100", ok, "{v}");
    }
    let p = Prefs::from_settings(&Settings::parse("font_size = inf\nline_width = 40\n"));
    assert_eq!((p.font_size, p.line_width), (16.0, 40.0));
}

#[test]
fn font_paths_round_trip() {
    let fp = FontPaths {
        family: "Noto Sans".into(),
        faces: [
            Some(("/usr/share/fonts/NotoSans-Regular.ttf".into(), 0)),
            Some(("/usr/share/fonts/NotoSans-Bold.ttf".into(), 0)),
            None,
            Some(("/usr/share/fonts/Noto.ttc".into(), 3)),
        ],
    };
    let v = fp.value().unwrap();
    assert_eq!(
        v,
        "Noto Sans | /usr/share/fonts/NotoSans-Regular.ttf | /usr/share/fonts/NotoSans-Bold.ttf |  | /usr/share/fonts/Noto.ttc@3"
    );
    assert_eq!(FontPaths::parse(&v), Some(fp.clone()));
    // A file name ending in @digits keeps its name.
    let odd = FontPaths {
        family: "X".into(),
        faces: [Some(("/f/v@2".into(), 0)), None, None, None],
    };
    assert_eq!(FontPaths::parse(&odd.value().unwrap()), Some(odd));
    assert_eq!(FontPaths::parse("Family"), None, "regular face required");
    assert_eq!(FontPaths::parse(" | /a.ttf"), None, "family required");
    assert!(FontPaths::parse("F | /a.ttf").unwrap().family_is(" f "));
    let bad = FontPaths {
        family: "X".into(),
        faces: [Some(("/a|b.ttf".into(), 0)), None, None, None],
    };
    assert_eq!(bad.value(), None);
}

#[test]
fn cached_paths_only_count_for_their_family() {
    let s = Settings::parse(
        "font_sans = Noto Sans\nfont_sans_path = Noto Sans | /a.ttf\n\
         font_mono = Fira Mono\nfont_mono_path = DejaVu Sans Mono | /b.ttf\n",
    );
    let p = Prefs::from_settings(&s);
    assert!(p.font_sans_path.is_some());
    assert_eq!(p.font_mono_path, None, "cached for another family");
    let s = Settings::parse("font_sans_path = Noto Sans | /a.ttf\n");
    assert_eq!(
        Prefs::from_settings(&s).font_sans_path,
        None,
        "family is default"
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn xdg_paths() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    };
    assert_eq!(
        path_from(env(&[("XDG_CONFIG_HOME", "/cfg"), ("HOME", "/home/u")])),
        Some("/cfg/vomd/settings.ini".into())
    );
    assert_eq!(
        path_from(env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/u")])),
        Some("/home/u/.config/vomd/settings.ini".into())
    );
    assert_eq!(
        path_from(env(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/u")])),
        Some("/home/u/.config/vomd/settings.ini".into()),
        "a relative XDG_CONFIG_HOME is ignored (XDG spec)"
    );
    assert_eq!(path_from(env(&[])), None);
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vomd-settings-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn save_is_atomic_and_load_reads_it_back() {
    let dir = temp_dir("save");
    let path = dir.join("vomd/settings.ini");
    assert_eq!(
        load(&path),
        (Settings::default(), None),
        "missing: defaults"
    );
    let mut s = Settings::parse("# keep\ntheme = dark\n");
    s.set("zoom", "130");
    let st = save(&path, &s).unwrap();
    assert!(st.is_some());
    assert!(!dir.join("vomd/settings.ini.tmp").exists());
    let (back, st2) = load(&path);
    assert_eq!(back, s);
    assert_eq!(st2, st);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# keep\ntheme = dark\nzoom = 130\n"
    );
    // Oversized files are ignored, not parsed.
    std::fs::write(&path, "zoom = 150\n".repeat(30_000)).unwrap();
    assert_eq!(load(&path).0, Settings::default());
    // Invalid UTF-8 is replaced, never fatal.
    std::fs::write(&path, b"zoom = 150\n\xff\xfe = x\n").unwrap();
    assert_eq!(load(&path).0.get("zoom"), Some("150"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn template_is_all_defaults() {
    let s = Settings::parse(TEMPLATE);
    assert_eq!(
        Prefs::from_settings(&s),
        Prefs::default(),
        "everything is commented out"
    );
    // Uncommenting each line gives the documented default.
    let uncommented: String = TEMPLATE
        .lines()
        .map(|l| match l.strip_prefix("# ") {
            Some(rest) if split(rest).is_some() => format!("{rest}\n"),
            _ => format!("{l}\n"),
        })
        .collect();
    let s = Settings::parse(&uncommented);
    for key in [
        "theme",
        "zoom",
        "toolbar",
        "toc",
        "window_width",
        "window_height",
        "remote_images",
        "font_size",
        "line_width",
        "font_sans",
        "font_mono",
    ] {
        assert!(s.get(key).is_some(), "{key} documented");
    }
    assert_eq!(Prefs::from_settings(&s), Prefs::default());
    let dir = temp_dir("template");
    let path = dir.join("settings.ini");
    assert!(ensure_template(&path).unwrap());
    assert!(!ensure_template(&path).unwrap(), "never overwrites");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn saver_debounces() {
    let t0 = Instant::now();
    let mut s = Saver::default();
    assert!(!s.dirty() && s.deadline().is_none());
    s.mark(t0);
    s.mark(t0 + Duration::from_millis(300));
    assert_eq!(s.deadline(), Some(t0 + Duration::from_millis(800)));
    assert!(!s.take_due(t0 + Duration::from_millis(700)));
    assert!(s.take_due(t0 + Duration::from_millis(800)));
    assert!(!s.dirty());
    s.mark(t0);
    assert!(s.take(), "close saves at once");
    assert!(!s.take());
}

/// Random lines (keys, values, comments, junk, odd whitespace and `=`s) never panic, and any
/// sequence of writes reads back exactly while every other line is untouched.
#[test]
fn fuzz_round_trip() {
    let n: usize = std::env::var("VOMD_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |m: usize| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % m as u64) as usize
    };
    let pieces = [
        "zoom",
        "theme",
        "a_b",
        "=",
        " ",
        "\t",
        "#",
        ";",
        "dark",
        "100",
        "é",
        "key with space",
        "==",
        "\u{feff}",
        "x=y=z",
        "",
        "/path/#1",
        "\r",
    ];
    let keys = ["zoom", "theme", "font_size", "new_key"];
    let values = ["1", "dark", "", "/a b/c#d", "x=y"];
    for _ in 0..n {
        let mut text = String::new();
        for _ in 0..next(8) {
            for _ in 0..next(6) {
                text.push_str(pieces[next(pieces.len())]);
            }
            text.push('\n');
        }
        let mut s = Settings::parse(&text);
        let _ = Prefs::from_settings(&s);
        let before = s.text();
        let mut expect: Vec<(&str, &str)> = Vec::new();
        for _ in 0..next(4) {
            let (k, v) = (keys[next(keys.len())], values[next(values.len())]);
            s.set(k, v);
            expect.retain(|(ek, _)| *ek != k);
            expect.push((k, v));
        }
        let back = Settings::parse(&s.text());
        for (k, v) in &expect {
            assert_eq!(back.get(k), Some(*v), "{text:?} then {expect:?}");
        }
        // Lines that held none of the written keys are unchanged.
        let keep = |l: &&str| {
            !keys
                .iter()
                .any(|k| split(l).is_some_and(|(kr, _)| &l[kr] == *k))
        };
        let old: Vec<&str> = before.lines().filter(keep).collect();
        let new_text = s.text();
        let new: Vec<&str> = new_text.lines().filter(keep).collect();
        assert_eq!(old, new, "{text:?}");
    }
}

#[test]
fn store_without_a_path_never_writes() {
    let mut st = Store::open(None);
    let t0 = Instant::now();
    st.record(ThemeMode::Dark, 150, t0);
    assert_eq!(st.deadline(), None);
    st.record_window((640, 480), t0);
    st.flush().unwrap();
    assert!(st.refresh().is_none());
    assert_eq!(
        st.prefs().theme,
        ThemeMode::Dark,
        "kept in memory for this run"
    );
}

#[test]
fn store_records_saves_and_refreshes() {
    let dir = temp_dir("store");
    let path = dir.join("vomd/settings.ini");
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    assert_eq!(st.prefs(), Prefs::default());
    // No change, no file.
    st.record(ThemeMode::System, 100, t0);
    st.record_window(DEFAULT_WINDOW, t0);
    assert_eq!(st.deadline(), None);
    st.flush().unwrap();
    assert!(!path.exists(), "a missing file isn't created for defaults");
    // A change is saved after the delay.
    st.record(ThemeMode::Dark, 100, t0);
    st.poll(t0 + Duration::from_millis(100)).unwrap();
    assert!(!path.exists(), "debounced");
    st.record(ThemeMode::Dark, 120, t0 + Duration::from_millis(200));
    st.poll(t0 + SAVE_DELAY + Duration::from_millis(200))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "theme = dark\nzoom = 120\n"
    );
    assert!(
        st.refresh().is_none(),
        "our own write isn't an external change"
    );
    // Window size at close.
    st.record_window((800, 600), t0);
    st.flush().unwrap();
    assert_eq!(st.prefs().window, (800, 600));
    // An edit by hand is picked up on refresh, and wins over unsaved changes.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, "# mine\ntheme = light\nzoom = 999\n").unwrap();
    st.record(ThemeMode::Dark, 130, t0);
    let p = st.refresh().expect("changed on disk");
    assert_eq!((p.theme, p.zoom), (ThemeMode::Light, 100));
    assert_eq!(st.deadline(), None, "pending changes dropped");
    // The view now shows zoom 100 (the default for the bad value): that isn't a change.
    st.record(ThemeMode::Light, 100, t0);
    assert_eq!(st.deadline(), None);
    st.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\ntheme = light\nzoom = 999\n"
    );
    // A real change rewrites only its line.
    st.record(ThemeMode::Light, 110, t0);
    st.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\ntheme = light\nzoom = 110\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn store_records_font_paths() {
    let dir = temp_dir("fonts");
    let path = dir.join("settings.ini");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, "font_sans = Inter\n").unwrap();
    let mut st = Store::open(Some(path.clone()));
    let fp = FontPaths {
        family: "Inter".into(),
        faces: [Some(("/x/Inter-Regular.ttf".into(), 0)), None, None, None],
    };
    st.record_fonts(false, &fp, Instant::now());
    st.flush().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "font_sans = Inter\nfont_sans_path = Inter | /x/Inter-Regular.ttf |  |  |\n"
    );
    assert_eq!(st.prefs().font_sans_path, Some(fp));
    std::fs::remove_dir_all(&dir).unwrap();
}
