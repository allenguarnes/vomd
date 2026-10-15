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
