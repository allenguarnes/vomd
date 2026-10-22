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
fn wide_is_a_bool_and_bad_values_are_ignored() {
    assert!(!Prefs::default().wide);
    for (v, want) in [("true", true), ("On", true), ("false", false), ("0", false)] {
        let p = Prefs::from_settings(&Settings::parse(&format!("wide = {v}")));
        assert_eq!(p.wide, want, "{v}");
    }
    for v in ["maybe", "", "2"] {
        let p = Prefs::from_settings(&Settings::parse(&format!("wide = {v}")));
        assert!(!p.wide, "{v:?} is ignored");
    }
    assert!(TEMPLATE.contains("# wide = false"));
}

/// `primary_selection` is on or off like the other switches, is on without a line for it, and
/// every value that isn't a switch leaves the default.
#[test]
fn primary_selection_is_a_switch_with_a_platform_default() {
    let p = |v: &str| Prefs::from_settings(&Settings::parse(&format!("primary_selection = {v}")));
    for (v, want) in [
        ("on", true),
        ("OFF", false),
        ("true", true),
        ("no", false),
        ("1", true),
        ("0", false),
    ] {
        assert_eq!(p(v).primary_selection, want, "{v}");
    }
    for v in ["maybe", "", "2", "middle"] {
        assert_eq!(p(v).primary_selection, DEFAULT_PRIMARY, "{v:?} is ignored");
    }
    assert_eq!(Prefs::default().primary_selection, DEFAULT_PRIMARY);
    assert_eq!(
        Prefs::from_settings(&Settings::default()).primary_selection,
        DEFAULT_PRIMARY
    );
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
    const {
        assert!(DEFAULT_PRIMARY, "on where there is a primary selection")
    };
    // Windows has no primary selection: its template leaves the setting out.
    #[cfg(windows)]
    {
        assert!(!TEMPLATE.contains("primary_selection"));
        return;
    }
    // The template documents it at that default, and writing it replaces the template's line
    // and keeps the rest of the file.
    #[cfg_attr(windows, allow(unreachable_code))]
    let line = format!(
        "# primary_selection = {}",
        if DEFAULT_PRIMARY { "on" } else { "off" }
    );
    assert!(TEMPLATE.lines().any(|l| l == line));
    let mut s = Settings::parse(TEMPLATE);
    assert!(s.set(
        "primary_selection",
        if DEFAULT_PRIMARY { "off" } else { "on" }
    ));
    assert!(!s.text().contains(&line));
    assert_eq!(s.text().lines().count(), TEMPLATE.lines().count());
    assert_eq!(Prefs::from_settings(&s).primary_selection, !DEFAULT_PRIMARY);
}

/// A change of the setting in the file is a change of the preferences (applied on focus, like
/// the others), and a save of something else leaves its line alone.
#[test]
fn primary_selection_is_reloaded_and_survives_saves() {
    let dir = temp_dir("primary");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.ini");
    std::fs::write(&path, "# mine\nprimary_selection = off\nzoom = 120\n").unwrap();
    let mut store = Store::open(Some(path.clone()));
    assert!(!store.prefs().primary_selection);
    store.set("zoom", "130", Instant::now());
    store.flush().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("# mine\nprimary_selection = off\nzoom = 130"),
        "{text}"
    );
    // Edited in an editor: the next look at the file sees it.
    std::fs::write(&path, "primary_selection = on\nzoom = 130\n").unwrap();
    let prefs = store.refresh().expect("the file changed");
    assert!(prefs.primary_selection);
    std::fs::remove_dir_all(&dir).unwrap();
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
    assert!(!p.wide, "off by default");
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
    // A family name with a `|` would read back as a shorter one: not cached at all.
    let piped = FontPaths {
        family: "A | B".into(),
        faces: [Some(("/a.ttf".into(), 0)), None, None, None],
    };
    assert_eq!(piped.value(), None);
    let mut st = Store::open(None);
    st.record_fonts(false, &piped, Instant::now());
    assert_eq!(st.settings().get("font_sans_path"), None);
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
        Loaded {
            settings: Settings::default(),
            stamp: None,
            faithful: true
        },
        "missing: defaults"
    );
    let mut s = Settings::parse("# keep\ntheme = dark\n");
    s.set("zoom", "130");
    let st = save(&path, &s).unwrap();
    assert!(st.is_some());
    let left: Vec<_> = std::fs::read_dir(dir.join("vomd")).unwrap().collect();
    assert_eq!(left.len(), 1, "no temporary file left");
    let back = load(&path);
    assert_eq!((back.settings, back.stamp, back.faithful), (s, st, true));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# keep\ntheme = dark\nzoom = 130\n"
    );
    // Oversized files are ignored, not parsed.
    std::fs::write(&path, "zoom = 150\n".repeat(30_000)).unwrap();
    let big = load(&path);
    assert_eq!((big.settings, big.faithful), (Settings::default(), false));
    // Invalid UTF-8 is replaced, never fatal.
    std::fs::write(&path, b"zoom = 150\n\xff\xfe = x\n").unwrap();
    let odd = load(&path);
    assert_eq!(
        (odd.settings.get("zoom"), odd.faithful),
        (Some("150"), false)
    );
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
        "wide",
        "primary_selection",
        "font_sans",
        "font_mono",
    ] {
        // (Windows has no primary selection, and its template no line for it.)
        if cfg!(windows) && key == "primary_selection" {
            assert!(s.get(key).is_none());
            continue;
        }
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
                .any(|k| split(l).is_some_and(|(kr, _)| l[kr].eq_ignore_ascii_case(k)))
                && !TEMPLATE.lines().any(|t| t == *l)
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
    // An edit by hand is picked up on refresh.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, "# mine\ntheme = light\nzoom = 999\n").unwrap();
    let p = st.refresh().expect("changed on disk");
    assert_eq!((p.theme, p.zoom), (ThemeMode::Light, 100));
    assert_eq!(st.deadline(), None);
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

#[test]
fn store_saves_the_wide_toggle_and_keeps_comments() {
    let dir = temp_dir("wide");
    let path = dir.join("settings.ini");
    std::fs::create_dir_all(&dir).unwrap();
    let file = "# my settings\ntheme = dark\n\n# wide = false\nzoom = 120\n";
    std::fs::write(&path, file).unwrap();
    let mut st = Store::open(Some(path.clone()));
    assert!(!st.prefs().wide);
    st.set("wide", "true", Instant::now());
    st.flush().unwrap();
    // The template's commented default is replaced in place; the rest is untouched.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# my settings\ntheme = dark\n\nwide = true\nzoom = 120\n"
    );
    // Read again by another run: the value is kept, and so are the comments.
    let mut again = Store::open(Some(path.clone()));
    assert!(again.prefs().wide);
    assert_eq!(again.prefs().zoom, 120);
    again.set("wide", "false", Instant::now());
    again.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# my settings\ntheme = dark\n\nwide = false\nzoom = 120\n"
    );
    assert!(!Store::open(Some(path)).prefs().wide);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn toc_is_a_bool_saved_like_the_wide_toggle() {
    assert!(!Prefs::default().toc);
    for (v, want) in [("true", true), ("ON", true), ("1", true), ("no", false)] {
        let p = Prefs::from_settings(&Settings::parse(&format!("toc = {v}")));
        assert_eq!(p.toc, want, "{v}");
    }
    for v in ["", "maybe", "2"] {
        let p = Prefs::from_settings(&Settings::parse(&format!("toc = {v}")));
        assert!(!p.toc, "{v:?} is ignored");
    }
    assert!(TEMPLATE.contains("# toc = false"));
    assert!(TEMPLATE.contains("(C toggles it)"));
    let dir = temp_dir("toc");
    let path = dir.join("settings.ini");
    std::fs::create_dir_all(&dir).unwrap();
    let file = "# my settings\ntheme = dark\n\n# toc = false\nzoom = 120\n";
    std::fs::write(&path, file).unwrap();
    let mut st = Store::open(Some(path.clone()));
    assert!(!st.prefs().toc);
    st.set("toc", "true", Instant::now());
    st.flush().unwrap();
    // The template's commented default is replaced in place; the rest is untouched.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# my settings\ntheme = dark\n\ntoc = true\nzoom = 120\n"
    );
    // Read again by another run: the value is kept.
    let mut again = Store::open(Some(path.clone()));
    assert!(again.prefs().toc);
    assert_eq!(again.prefs().zoom, 120);
    again.set("toc", "false", Instant::now());
    again.flush().unwrap();
    assert!(!Store::open(Some(path)).prefs().toc);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- Parser details ---------------------------------------------------------------------------

#[test]
fn bom_keeps_the_first_key_findable() {
    let mut s = Settings::parse("\u{feff}theme = dark\nzoom = 120\n");
    assert_eq!(s.get("theme"), Some("dark"));
    assert!(s.set("theme", "light"));
    assert_eq!(
        s.text(),
        "\u{feff}theme = light\nzoom = 120\n",
        "no duplicate"
    );
}

#[test]
fn keys_match_in_any_ascii_case() {
    let mut s = Settings::parse("Theme = dark\nZOOM=120\n");
    assert_eq!(s.get("theme"), Some("dark"));
    assert_eq!(Prefs::from_settings(&s).zoom, 120);
    assert!(s.set("theme", "light"));
    assert!(!s.set("zoom", "120"));
    assert!(s.set("zoom", "130"));
    assert_eq!(
        s.text(),
        "Theme = light\nZOOM=130\n",
        "spelling kept, no duplicate"
    );
}

#[test]
fn crlf_files_stay_crlf() {
    let text = "# mine\r\ntheme = dark\r\nzoom = 120\r\n";
    let mut s = Settings::parse(text);
    assert_eq!(s.get("theme"), Some("dark"), "no \\r in values");
    assert_eq!(s.text(), text);
    s.set("zoom", "130");
    s.set("toc", "true");
    assert_eq!(
        s.text(),
        "# mine\r\ntheme = dark\r\nzoom = 130\r\ntoc = true\r\n"
    );
    assert_eq!(Settings::parse("a = 1\nb = 2\n").text(), "a = 1\nb = 2\n");
}

#[test]
fn inline_comments_are_part_of_the_value() {
    let s = Settings::parse("theme = dark # night\n");
    assert_eq!(s.get("theme"), Some("dark # night"));
    assert_eq!(
        Prefs::from_settings(&s).theme,
        ThemeMode::System,
        "so it isn't a theme"
    );
    assert!(TEMPLATE.contains("Comments go on a line of"));
}

#[test]
fn a_commented_default_is_replaced_in_place() {
    let mut s = Settings::parse(TEMPLATE);
    s.set("theme", "dark");
    s.set("ZOOM", "120");
    let text = s.text();
    assert!(text.contains("\ntheme = dark\n") && !text.contains("# theme = system"));
    assert!(text.contains("\nZOOM = 120\n") && !text.contains("# zoom = 100"));
    assert_eq!(
        text.lines().count(),
        TEMPLATE.lines().count(),
        "nothing appended"
    );
    let p = Prefs::from_settings(&Settings::parse(&text));
    assert_eq!((p.theme, p.zoom), (ThemeMode::Dark, 120));
    // Only the template's own lines: other comments stay, the value goes at the end.
    let mut s = Settings::parse("# theme = system or so\n#theme = system\n");
    s.set("theme", "dark");
    assert_eq!(
        s.text(),
        "# theme = system or so\n#theme = system\ntheme = dark\n"
    );
    // Once set, later writes use that line.
    let mut s = Settings::parse(TEMPLATE);
    s.set("zoom", "120");
    s.set("zoom", "130");
    assert_eq!(s.get("zoom"), Some("130"));
    assert_eq!(s.text().matches("zoom = ").count(), 1);
}

#[test]
fn non_finite_numbers_are_rejected_by_the_range() {
    for v in ["NaN", "nan", "inf", "-inf", "infinity", "1e999"] {
        let p = Prefs::from_settings(&Settings::parse(&format!(
            "font_size = {v}\nline_width = {v}\n"
        )));
        assert_eq!((p.font_size, p.line_width), (16.0, 80.0), "{v}");
    }
}

// ---- Saving against a file that changed -------------------------------------------------------

fn write_file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Let the file's modification time move on (the stamp also has the length, but not always).
fn tick() {
    std::thread::sleep(Duration::from_millis(20));
}

#[test]
fn saving_keeps_edits_made_since_the_file_was_read() {
    let dir = temp_dir("external");
    let path = dir.join("settings.ini");
    write_file(&path, "zoom = 110\n");
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    tick();
    // A hand edit while this window is open and unfocused: a key and a comment.
    write_file(&path, "# note to self\nzoom = 110\ntoc = true\n");
    st.record_window((800, 600), t0);
    st.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# note to self\nzoom = 110\ntoc = true\nwindow_width = 800\nwindow_height = 600\n"
    );
    // The merged-in edit reaches the view on the next focus although the stamp is ours.
    let p = st.refresh().expect("external changes were merged");
    assert!(p.toc);
    assert!(st.refresh().is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn two_stores_on_one_file_keep_each_others_keys() {
    let dir = temp_dir("two");
    let path = dir.join("settings.ini");
    write_file(&path, "# shared\n");
    let t0 = Instant::now();
    let mut a = Store::open(Some(path.clone()));
    let mut b = Store::open(Some(path.clone()));
    a.record(ThemeMode::Dark, 100, t0);
    b.record(ThemeMode::System, 130, t0);
    tick();
    a.flush().unwrap();
    tick();
    b.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# shared\ntheme = dark\nzoom = 130\n",
        "different keys: both survive"
    );
    // The same key: the last write wins, and a stale store doesn't write back old values.
    a.set("toc", "true", t0);
    b.set("toc", "false", t0);
    tick();
    a.flush().unwrap();
    tick();
    b.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# shared\ntheme = dark\nzoom = 130\ntoc = false\n"
    );
    // `a` still shows theme dark, zoom 100 internally: its next write doesn't revert zoom.
    a.set("toolbar", "false", t0);
    tick();
    a.flush().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("zoom = 130") && text.contains("toolbar = false"),
        "{text}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pending_changes_stay_on_top_when_the_file_is_refreshed() {
    let dir = temp_dir("pending-refresh");
    let path = dir.join("settings.ini");
    write_file(&path, "theme = light\n");
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    st.record(ThemeMode::Light, 130, t0);
    tick();
    write_file(&path, "# mine\ntheme = dark\nzoom = 999\ntoc = true\n");
    let p = st.refresh().expect("changed on disk");
    assert_eq!((p.theme, p.zoom, p.toc), (ThemeMode::Dark, 130, true));
    assert!(
        st.deadline().is_some(),
        "the unsaved zoom is still to be saved"
    );
    st.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\ntheme = dark\nzoom = 130\ntoc = true\n"
    );
    // A pending value the file now has is no change at all.
    st.set("toc", "false", t0);
    tick();
    write_file(&path, "toc = false\n");
    st.refresh().unwrap();
    assert_eq!(st.deadline(), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_merged_external_change_is_not_reverted_by_the_next_record() {
    let dir = temp_dir("merged-theme");
    let path = dir.join("settings.ini");
    write_file(&path, "theme = light\n");
    let t0 = Instant::now();
    let mut a = Store::open(Some(path.clone()));
    tick();
    write_file(&path, "theme = dark\n");
    a.record(ThemeMode::Light, 110, t0);
    a.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "theme = dark\nzoom = 110\n"
    );
    // The window applies what the save merged, as the app does after each save.
    let p = a.take_merged().expect("external changes were merged");
    assert_eq!((p.theme, p.zoom), (ThemeMode::Dark, 110));
    assert!(a.take_merged().is_none(), "reported once");
    a.record(p.theme, 120, t0);
    a.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "theme = dark\nzoom = 120\n",
        "the external theme survives"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_edit_with_the_same_stamp_is_not_overwritten_by_a_save() {
    let dir = temp_dir("same-stamp");
    let path = dir.join("settings.ini");
    write_file(&path, "# user note A\nzoom = 100\n");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    // Same length, and the old modification time put back (a coarse clock would do the same).
    write_file(&path, "# user note B\nzoom = 150\n");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(before)
        .unwrap();
    assert_eq!(stamp(&path), st.stamp, "the stamp can't tell");
    st.set("toc", "true", t0);
    st.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# user note B\nzoom = 150\ntoc = true\n"
    );
    let p = st.take_merged().expect("the edit was merged");
    assert_eq!((p.zoom, p.toc), (150, true));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- Files that can't be read faithfully ------------------------------------------------------

/// A store on `bytes` writes nothing: not on a change, nor when the file is replaced (then it
/// reads fine and writing works again).
fn assert_read_only(name: &str, bytes: &[u8]) {
    let dir = temp_dir(name);
    let path = dir.join("settings.ini");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    st.record(ThemeMode::Dark, 130, t0);
    st.record_window((640, 480), t0);
    st.set("toc", "true", t0);
    assert_eq!(st.deadline(), None, "{name}: nothing to save");
    st.flush().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}: untouched");
    assert_eq!(
        st.prefs().theme,
        ThemeMode::Dark,
        "{name}: still applies in memory"
    );
    tick();
    write_file(&path, "zoom = 90\n");
    assert_eq!(st.refresh().unwrap().zoom, 90);
    st.set("toc", "true", t0);
    st.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "zoom = 90\ntoc = true\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_oversized_file_is_never_overwritten() {
    assert_read_only("big", "zoom = 150\n".repeat(30_000).as_bytes());
}

#[test]
fn a_non_utf8_file_is_never_overwritten() {
    assert_read_only("latin1", b"# caf\xe9\nzoom = 150\n");
    let dir = temp_dir("latin1-prefs");
    let path = dir.join("settings.ini");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, b"zoom = 150\n# caf\xe9\n").unwrap();
    assert_eq!(
        Store::open(Some(path)).prefs().zoom,
        150,
        "what parsed applies"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_never_overwritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("unreadable");
    let path = dir.join("settings.ini");
    write_file(&path, "theme = dark\nzoom = 150\n");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
    if std::fs::read(&path).is_ok() {
        eprintln!("skipped: permissions don't apply (root)");
        std::fs::remove_dir_all(&dir).unwrap();
        return;
    }
    let mut st = Store::open(Some(path.clone()));
    assert_eq!(st.prefs(), Prefs::default());
    st.record(ThemeMode::Light, 130, Instant::now());
    st.flush().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "theme = dark\nzoom = 150\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- Symlinks and permissions -----------------------------------------------------------------

#[cfg(unix)]
#[test]
fn saving_follows_a_symlink_and_keeps_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = temp_dir("symlink");
    let real = dir.join("dotfiles/vomd.ini");
    let link = dir.join("config/settings.ini");
    write_file(&real, "# mine\nzoom = 110\n");
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink("../dotfiles/vomd.ini", &link).unwrap();
    let mut st = Store::open(Some(link.clone()));
    assert_eq!(st.prefs().zoom, 110);
    st.record(ThemeMode::Dark, 110, Instant::now());
    st.flush().unwrap();
    assert!(
        std::fs::symlink_metadata(&link).unwrap().is_symlink(),
        "link kept"
    );
    assert_eq!(
        std::fs::read_to_string(&real).unwrap(),
        "# mine\nzoom = 110\ntheme = dark\n",
        "target updated"
    );
    let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "permissions kept");
    assert!(st.refresh().is_none(), "own write through the link");
    let names = |d: &Path| std::fs::read_dir(d).unwrap().count();
    assert_eq!(
        (names(real.parent().unwrap()), names(link.parent().unwrap())),
        (1, 1)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_gets_the_template_at_its_target() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("dangling");
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("settings.ini");
    symlink(dir.join("later/vomd.ini"), &link).unwrap();
    assert!(ensure_template(&link).unwrap());
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        std::fs::read_to_string(dir.join("later/vomd.ini")).unwrap(),
        TEMPLATE
    );
    assert!(!ensure_template(&link).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_read_only_file_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("frozen");
    let path = dir.join("settings.ini");
    write_file(&path, "# frozen\nzoom = 150\n");
    let mode = |m| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o444);
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    assert_eq!(st.prefs().zoom, 150, "still read");
    st.record(ThemeMode::Dark, 150, t0);
    assert_eq!(st.deadline(), None, "nothing to save");
    st.flush().unwrap();
    assert_eq!(
        save(&path, &Settings::default()).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    let text = "# frozen\nzoom = 150\n";
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o444
    );
    // Made read-only while a change waits: dropped, not retried.
    mode(0o644);
    let mut st = Store::open(Some(path.clone()));
    st.record(ThemeMode::Dark, 150, t0);
    mode(0o444);
    st.flush().unwrap();
    assert_eq!(st.deadline(), None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_read_only_store_recovers_after_chmod_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("chmod");
    let path = dir.join("settings.ini");
    write_file(&path, "zoom = 150\n");
    let mode = |m| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o444);
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    st.record(ThemeMode::Dark, 150, t0);
    assert!(st.refresh().is_none(), "unchanged and still read-only");
    assert_eq!(
        st.prefs().theme,
        ThemeMode::Dark,
        "this window's changes stay"
    );
    mode(0o644);
    let p = st
        .refresh()
        .expect("writable again, though stamp is the same");
    assert_eq!((p.theme, p.zoom), (ThemeMode::System, 150), "the file wins");
    st.record(ThemeMode::Light, 150, t0);
    st.flush().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "zoom = 150\ntheme = light\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- Failed saves -----------------------------------------------------------------------------

#[test]
fn a_failed_save_is_kept_and_retried() {
    let dir = temp_dir("retry");
    let path = dir.join("blocker/settings.ini");
    std::fs::create_dir_all(&dir).unwrap();
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    // A file where the directory must go: every save fails.
    std::fs::write(dir.join("blocker"), "in the way").unwrap();
    st.record(ThemeMode::Dark, 100, t0);
    let due = t0 + SAVE_DELAY;
    assert!(st.poll(due).is_err(), "the first failure is reported");
    assert_eq!(st.deadline(), Some(due + RETRY_DELAY), "retry later");
    assert!(st.poll(due + RETRY_DELAY).is_ok(), "repeats are quiet");
    assert!(st.deadline().is_some(), "but still retried");
    assert!(st.flush().is_ok() && st.deadline().is_some());
    std::fs::remove_file(dir.join("blocker")).unwrap();
    st.poll(due + 2 * RETRY_DELAY).unwrap();
    assert_eq!(st.deadline(), None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = dark\n");
    // Closing tries once more.
    std::fs::remove_file(&path).unwrap();
    st.set("toc", "true", t0);
    st.flush().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "toc = true\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- The gear action --------------------------------------------------------------------------

#[test]
fn the_template_gets_pending_changes_on_top() {
    let dir = temp_dir("gear");
    let path = dir.join("vomd/settings.ini");
    let t0 = Instant::now();
    let mut st = Store::open(Some(path.clone()));
    st.record(ThemeMode::Dark, 100, t0);
    st.set("toc", "true", t0);
    st.prepare_file().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut expect = Settings::parse(TEMPLATE);
    expect.set("theme", "dark");
    expect.set("toc", "true");
    assert_eq!(
        text,
        expect.text(),
        "the template with the changes in place"
    );
    assert!(text.contains("# Color theme"), "comments there");
    assert_eq!(st.deadline(), None, "saved");
    assert!(st.refresh().is_none());
    // With a file already: pending changes are merged into it, not into a template.
    let mut st = Store::open(Some(path.clone()));
    st.record(ThemeMode::Light, 100, t0);
    tick();
    write_file(&path, "# hand\ntoc = true\n");
    st.prepare_file().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# hand\ntoc = true\ntheme = light\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_gear_needs_a_settings_file() {
    let mut st = Store::open(None);
    assert!(st.path().is_none());
    let e = st.editor_file().unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::Unsupported);
}
