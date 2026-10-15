use super::*;

fn base() -> Option<&'static Path> {
    Some(Path::new("/docs/guide"))
}

fn doc(path: &str, fragment: Option<&str>) -> Target {
    Target::Doc {
        path: PathBuf::from(path),
        fragment: fragment.map(str::to_owned),
    }
}

#[test]
fn anchors() {
    assert_eq!(classify("#intro", base()), Target::Anchor("intro".into()));
    assert_eq!(classify("#", base()), Target::Anchor(String::new()));
    assert_eq!(
        classify("#caf%C3%A9-%E2%9C%93", base()),
        Target::Anchor("café-✓".into())
    );
    assert_eq!(classify("  #x  ", base()), Target::Anchor("x".into()));
    // Anchors work without a file too (stdin).
    assert_eq!(classify("#x", None), Target::Anchor("x".into()));
}

#[test]
fn relative_markdown_files() {
    assert_eq!(
        classify("other.md", base()),
        doc("/docs/guide/other.md", None)
    );
    assert_eq!(
        classify("sub/Page.MARKDOWN#Part%202", base()),
        doc("/docs/guide/sub/Page.MARKDOWN", Some("Part 2"))
    );
    assert_eq!(
        classify("../up.md?plain=1#x", base()),
        doc("/docs/guide/../up.md", Some("x"))
    );
    assert_eq!(
        classify("my%20notes.mdown", base()),
        doc("/docs/guide/my notes.mdown", None)
    );
    assert_eq!(
        classify("./a.mkd", base()),
        doc("/docs/guide/./a.mkd", None)
    );
}

#[test]
fn external_urls() {
    for url in [
        "https://example.com/x?y#z",
        "http://localhost:3000",
        "HTTPS://EXAMPLE.COM",
        "mailto:me@example.com",
    ] {
        assert_eq!(classify(url, base()), Target::External(url.into()), "{url}");
    }
    assert_eq!(
        classify(" https://a.b ", base()),
        Target::External("https://a.b".into())
    );
}

/// `javascript:`, `file:` and URLs starting with `-` are inert.
#[test]
fn dangerous_links_are_inert() {
    for href in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        " javascript:alert(1)",
        "java\nscript:alert(1)",
        "java\tscript:alert(1)",
        "javascript\u{0}:x",
        "vbscript:msgbox",
        "data:text/html,<script>alert(1)</script>",
        "file:///etc/passwd",
        "FILE:/etc/passwd",
        "ftp://example.com/x.md",
        "-x",
        "--help",
        "-rf.md",
        "--version.md",
        "/etc/passwd",
        "/abs/doc.md",
        "//evil.example/x.md",
        "\\\\server\\share\\x.md",
        "C:\\Windows\\x.md",
        "c:/x.md",
        "x.md\u{0}.txt",
        "x%00.md",
        "",
        "   ",
        "?q=1",
        "image.png",
        "script.sh",
        "README",
        "dir/",
        "https://a.b/\u{7}bell",
        "mailto:a@b\nBcc: c@d",
    ] {
        let t = classify(href, base());
        // Relative paths starting with `-` resolve under the base: never an argument.
        let resolved_ok = matches!(&t, Target::Doc { path, .. } if path.starts_with("/docs/guide"));
        assert!(t == Target::Ignore || resolved_ok, "{href:?} → {t:?}");
    }
    // A leading `-` file name is resolved against the document, so it can't be read as an
    // option by anything it's passed to.
    assert_eq!(classify("-rf.md", base()), doc("/docs/guide/-rf.md", None));
    // Without a base, relative links do nothing.
    assert_eq!(classify("other.md", None), Target::Ignore);
}

/// A path that only becomes absolute once decoded would replace the document's directory in
/// `Path::join`.
#[test]
fn encoded_absolute_paths_are_inert() {
    for href in [
        "%2Ftmp%2Fx.md",
        "%2ftmp%2fx.md#top",
        "%5Ctmp%5Cx.md",
        "%2F%2Fhost%2Fx.md",
        "%2F%2Fhost/x.md",
        "%5C%5Cserver%5Cshare%5Cx.md",
        "C%3A%5Cx.md",
        "c%3A/x.md",
        "x%0A.md",
    ] {
        assert_eq!(classify(href, base()), Target::Ignore, "{href}");
    }
    // Climbing out of the directory is still a relative link.
    assert_eq!(
        classify("..%2F..%2Fx.md", base()),
        doc("/docs/guide/../../x.md", None)
    );
    assert_eq!(
        classify("sub%2Fx.md", base()),
        doc("/docs/guide/sub/x.md", None)
    );
}

#[test]
fn mailto_with_attachments_is_inert() {
    for url in [
        "mailto:a@b.c?attach=/etc/passwd",
        "mailto:a@b.c?subject=hi&Attach=%2Fhome%2Fme%2Fkey",
        "mailto:a@b.c?subject=hi&ATTACHMENT=/x",
        "mailto:a@b.c?%61ttach=/etc/passwd",
        "mailto:a@b.c?%41%74tachment=/etc/passwd",
        "mailto:a@b.c?%2561ttach=/etc/passwd",
        "MAILTO:a@b.c?body=x;attach=/etc/passwd",
        "mailto:a@b.c?attach",
        "mailto:?to=a@b.c&attachments=/x#frag",
    ] {
        assert_eq!(classify(url, base()), Target::Ignore, "{url}");
    }
    for url in [
        "mailto:a@b.c",
        "mailto:a@b.c?subject=attach%20the%20file&body=attach=x",
        "mailto:a@b.c?subject=hi#attach=x",
        "https://example.com/?attach=1",
    ] {
        assert_eq!(classify(url, base()), Target::External(url.into()), "{url}");
    }
}

/// A header name nested in percent escapes used to be decoded again and again, losing two
/// bytes a round: seconds for a link of 100 KB. The work is bounded now, and a name that can't
/// be checked completely counts as an attachment.
#[test]
fn nested_mailto_names_are_bounded() {
    let start = std::time::Instant::now();
    for n in [100, 20_000, 60_000, 120_000] {
        let url = format!("mailto:a@b.c?%{}61ttach=/etc/passwd", "25".repeat(n));
        assert_eq!(classify(&url, base()), Target::Ignore, "{n}");
        let url = format!("mailto:a@b.c?subject=hi&%{}61ttach=/x", "25".repeat(n));
        assert_eq!(classify(&url, base()), Target::Ignore, "{n}");
    }
    // Long names, many pairs, a long value, and one long unbroken query: linear work.
    let long = format!("mailto:a@b.c?{}=x", "n".repeat(200_000));
    assert_eq!(classify(&long, base()), Target::Ignore);
    let pairs = format!("mailto:a@b.c?{}", "s=1&".repeat(100_000));
    assert_eq!(classify(&pairs, base()), Target::External(pairs.clone()));
    let body = format!("mailto:a@b.c?body={}", "x%20".repeat(100_000));
    assert_eq!(classify(&body, base()), Target::External(body.clone()));
    // Up to the round limit the names are still decoded.
    let mut name = String::from("attach");
    for _ in 0..MAILTO_DECODE_ROUNDS {
        name = name.replace('%', "%25").replacen('a', "%61", 1);
        assert_eq!(
            classify(&format!("mailto:a@b.c?{name}=/x"), base()),
            Target::Ignore,
            "{name}"
        );
    }
    // Other links with huge text: one pass each.
    let big = "%2525".repeat(40_000);
    classify(&format!("https://example.com/{big}#{big}"), base());
    classify(&format!("sub/{big}.md#{big}"), base());
    classify(&format!("#{big}"), base());
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

/// Bidi controls reorder the text around them (`report\u{202E}fdp.exe` reads `reportexe.pdf`):
/// an external link holding one is never opened.
#[test]
fn bidi_controls_make_external_links_inert() {
    for c in [
        '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}',
        '\u{2068}', '\u{2069}', '\u{200E}', '\u{200F}', '\u{061C}',
    ] {
        for url in [
            format!("https://example.invalid/report{c}fdp.exe"),
            format!("http://exam{c}ple.com"),
            format!("mailto:a@b.c?subject=x{c}y"),
            format!("https://example.com/#a{c}"),
        ] {
            assert_eq!(classify(&url, base()), Target::Ignore, "{url:?}");
        }
    }
    // Right-to-left text itself is fine, and so are zero-width joiners in names that need them.
    for url in [
        "https://example.com/مثال/اختبار",
        "https://مثال.example/path",
        "https://example.com/שלום",
        "https://example.com/می\u{200C}خواهم",
    ] {
        assert_eq!(classify(url, base()), Target::External(url.into()), "{url}");
    }
    assert!(is_bidi_control('\u{202E}') && !is_bidi_control('א') && !is_bidi_control('\u{200D}'));
    assert!(is_invisible('\u{200D}') && is_invisible('\u{202E}') && !is_invisible('ا'));
    // A relative link or an anchor is no external URL: nothing there to be mistaken for one.
    assert_eq!(
        classify("#a\u{202E}b", base()),
        Target::Anchor("a\u{202E}b".into())
    );
}

#[test]
fn schemes() {
    assert_eq!(scheme("http://x"), Some("http"));
    assert_eq!(scheme("a+b-c.d:x"), Some("a+b-c.d"));
    assert_eq!(scheme("1abc:x"), None);
    assert_eq!(scheme("no scheme"), None);
    assert_eq!(scheme("a b:c"), None);
    assert_eq!(scheme(":x"), None);
}

#[test]
fn percent_decoding() {
    assert_eq!(percent_decode("a%20b"), "a b");
    assert_eq!(percent_decode("%e2%9c%93"), "✓");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%zz%4"), "%zz%4");
    assert_eq!(percent_decode("%ff"), "\u{fffd}");
}

#[test]
fn anchor_lookup() {
    let d = Doc::parse("# Intro\n\nText[^n].\n\n## Café Menu\n\n## Intro\n\n[^n]: Note.\n");
    let block = |slug: &str| d.anchors.iter().find(|(s, _)| s == slug).unwrap().1;
    assert_eq!(find_anchor(&d, ""), Some(0));
    assert_eq!(find_anchor(&d, "intro"), Some(block("intro")));
    assert_eq!(find_anchor(&d, "intro-1"), Some(block("intro-1")));
    assert_eq!(find_anchor(&d, "café-menu"), Some(block("café-menu")));
    assert_eq!(find_anchor(&d, "user-content-intro"), Some(block("intro")));
    assert_eq!(find_anchor(&d, "INTRO"), Some(block("intro")));
    assert_eq!(find_anchor(&d, "fn-n"), Some(block("fn-n")));
    assert_eq!(find_anchor(&d, "nowhere"), None);
}

#[test]
fn base_dirs() {
    assert_eq!(
        base_dir(&Input::File("/a/b/c.md".into())),
        Some(PathBuf::from("/a/b"))
    );
    assert_eq!(
        base_dir(&Input::File("c.md".into())),
        Some(PathBuf::from("."))
    );
    assert_eq!(base_dir(&Input::Stdin), None);
    assert_eq!(base_dir(&Input::None), None);
    // A file that is gone still has a directory: opening it reports why.
    assert_eq!(
        base_dir(&Input::File("/nonexistent-vomd-dir/c.md".into())),
        Some(PathBuf::from("/nonexistent-vomd-dir"))
    );
}

/// `cat p.md | vomd /dev/stdin`, `vomd <(cmd)`: the document can't be read again, so a relative
/// link must not replace it (Back couldn't return); anchors still work.
#[cfg(unix)]
#[test]
fn relative_links_are_inert_in_a_piped_document() {
    let classify_in = |input: &Input, href: &str| classify(href, base_dir(input).as_deref());
    let null = Input::File("/dev/null".into());
    assert_eq!(base_dir(&null), None);
    assert_eq!(classify_in(&null, "other.md"), Target::Ignore);
    assert_eq!(classify_in(&null, "#x"), Target::Anchor("x".into()));
    let dir = std::env::temp_dir().join(format!("vomd-nav-pipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("pipe.md");
    if std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success())
    {
        let input = Input::File(fifo);
        assert_eq!(classify_in(&input, "other.md"), Target::Ignore);
        assert_eq!(classify_in(&input, "#x"), Target::Anchor("x".into()));
    }
    // A regular file still resolves against its directory.
    let file = dir.join("a.md");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(
        classify_in(&Input::File(file), "other.md"),
        doc(&dir.join("other.md").to_string_lossy(), None)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn back_stack_is_bounded() {
    let mut s = BackStack::default();
    assert!(s.pop().is_none() && s.is_empty());
    for i in 0..40u32 {
        s.push(Entry {
            input: Input::File(format!("{i}.md").into()),
            anchor: Anchor::TOP,
        });
    }
    assert_eq!(s.len(), BACK_LIMIT);
    assert_eq!(s.pop().unwrap().input, Input::File("39.md".into()));
    let mut last = None;
    while let Some(e) = s.pop() {
        last = Some(e.input);
    }
    assert_eq!(
        last,
        Some(Input::File("8.md".into())),
        "the oldest were dropped"
    );
}

fn entry(input: Input) -> Entry {
    Entry {
        input,
        anchor: Anchor::TOP,
    }
}

#[test]
fn leaving_stdin_forgets_it() {
    let mut s = BackStack::default();
    // Jumps inside stdin come back to it.
    s.leave(entry(Input::Stdin), &Input::Stdin);
    s.leave(entry(Input::Stdin), &Input::Stdin);
    assert_eq!(s.len(), 2);
    // A dropped file replaces it: stdin can't be read again, so nothing points to it.
    s.leave(entry(Input::Stdin), &Input::File("c.md".into()));
    assert!(s.is_empty());
    // a.md → b.md → drop c.md: Back returns to b.md, then a.md.
    s.leave(
        entry(Input::File("a.md".into())),
        &Input::File("b.md".into()),
    );
    s.leave(
        entry(Input::File("b.md".into())),
        &Input::File("c.md".into()),
    );
    assert_eq!(s.pop().unwrap().input, Input::File("b.md".into()));
    assert_eq!(s.pop().unwrap().input, Input::File("a.md".into()));
    // Only stdin entries go; the rest of the history stays.
    s.leave(
        entry(Input::File("a.md".into())),
        &Input::File("b.md".into()),
    );
    s.leave(entry(Input::Stdin), &Input::File("c.md".into()));
    assert_eq!(s.len(), 1);
    assert_eq!(s.pop().unwrap().input, Input::File("a.md".into()));
    // The empty window is nothing to come back to.
    s.leave(entry(Input::None), &Input::File("a.md".into()));
    assert!(s.is_empty());
}

/// A pipe or device named on the command line is read once, like stdin.
#[cfg(unix)]
#[test]
fn leaving_a_pipe_forgets_it() {
    let pipe = Input::File("/dev/null".into());
    let mut s = BackStack::default();
    // Jumps inside it come back to it.
    s.leave(entry(pipe.clone()), &pipe);
    assert_eq!(s.len(), 1);
    s.leave(
        entry(Input::File("a.md".into())),
        &Input::File("b.md".into()),
    );
    // Leaving it for a document forgets its entries and keeps the others.
    s.leave(entry(pipe), &Input::File("c.md".into()));
    assert_eq!(s.len(), 1);
    assert_eq!(s.pop().unwrap().input, Input::File("a.md".into()));
}
