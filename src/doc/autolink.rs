//! GFM extended autolinks (GitHub Flavored Markdown spec §6.9): `www.` addresses, `http://`,
//! `https://` and `ftp://` URLs, and email addresses written as plain text become links, as on
//! GitHub (cmark-gfm's rules). pulldown-cmark doesn't do this, so the builder runs [`find`] over
//! plain text runs.

use std::ops::Range;

/// The autolinks in `s`: byte range of the link text and the link target. `prev` is the
/// character before `s` in the source (a space at the start of a block); whether a link may
/// start depends on the character before it.
pub fn find(s: &str, prev: char) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    // Every link needs one of these; most text has none, and the parser runs this on all of it.
    if !candidate(s) {
        return out;
    }
    let mut bad = Scan::default();
    let mut i = 0;
    while i < b.len() {
        // Links start with `w`, `h`, `f` or (for emails) contain `@`; only ASCII bytes are
        // tested, so `i` is on a character boundary whenever it is sliced at.
        match b[i] {
            b'w' | b'W' | b'h' | b'H' | b'f' | b'F' => {
                let before = if i == 0 {
                    prev
                } else {
                    s[..i].chars().next_back().unwrap_or(' ')
                };
                if let Some((end, href)) = at(s, i, before, &mut bad) {
                    out.push((i..end, href));
                    i = end;
                    continue;
                }
            }
            // Emails can start mid-word (`a.b@c.d`): back up over the local part.
            b'@' => {
                if let Some((start, end)) = email(s, i)
                    && out.last().is_none_or(|(r, _)| r.end <= start)
                {
                    out.push((start..end, format!("mailto:{}", &s[start..end])));
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Whether `s` could hold a link: an `@`, a `://` or a `www.`. One pass: the parser sees
/// the text as many short runs, so per-call setup (substring searchers) costs more than it
/// saves. This keeps autolinks at about 6 ms of the 5 MB fixture's parse.
fn candidate(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| match c {
        b'@' => true,
        b':' => b.get(i + 1..i + 3) == Some(b"//"),
        b'.' => i >= 3 && b[i - 3..i].eq_ignore_ascii_case(b"www"),
        _ => false,
    })
}

/// A `www.` or `http(s)://` or `ftp://` autolink starting at `i`, after the character `before`:
/// its end and target. As in cmark-gfm a `www.` address must follow whitespace or one of
/// `*_~(`, while a URL only can't follow a letter (which would make another scheme) and needs
/// no period in its domain (`http://localhost:3000`).
///
/// `bad` is the last failed domain scan; a `www.` inside it may fail without a scan of its own
/// (after `_`, say), which keeps `www._www._www._…` linear.
fn at(s: &str, i: usize, before: char, bad: &mut Scan) -> Option<(usize, String)> {
    let rest = &s[i..];
    let lower = |p: &str| {
        rest.as_bytes()
            .get(..p.len())
            .is_some_and(|b| b.eq_ignore_ascii_case(p.as_bytes()))
    };
    let (prefix, scheme) = if lower("https://") {
        (8, "")
    } else if lower("http://") {
        (7, "")
    } else if lower("ftp://") {
        (6, "")
    } else if lower("www.") {
        (0, "http://")
    } else {
        return None;
    };
    let boundary = if prefix > 0 {
        !before.is_ascii_alphabetic()
    } else {
        before.is_whitespace() || matches!(before, '*' | '_' | '~' | '(')
    };
    if !boundary {
        return None;
    }
    if prefix == 0 && bad.fails_www_at(i) {
        return None;
    }
    let domain_end = match domain(s, i + prefix, prefix > 0) {
        Ok(end) => end,
        Err(scan) => {
            *bad = scan;
            return None;
        }
    };
    let end = trim_end(s, i, path_end(s, domain_end));
    Some((end, format!("{scheme}{}", &s[i..end])))
}

/// A failed domain scan: the byte range it read and the last two segments it found (where
/// each starts, 0 for none, and whether it holds `_`).
#[derive(Default)]
struct Scan {
    from: usize,
    stop: usize,
    starts: [usize; 2],
    under: [bool; 2],
}

impl Scan {
    /// Whether a `www.` at `i` inside the scan fails too without scanning again. Its domain is
    /// `www` and then the segments after the period at `i + 3`, ending as this one did, so it
    /// fails with fewer than two segments or `_` in its last two.
    fn fails_www_at(&self, i: usize) -> bool {
        if i < self.from || i + 3 >= self.stop {
            return false;
        }
        let after = i + 4;
        if self.starts[0] >= after {
            self.under[0] || self.under[1]
        } else if self.starts[1] >= after {
            self.under[1]
        } else {
            true
        }
    }
}

/// The end of the valid domain starting at `from`: segments of alphanumerics, `_` and `-`
/// separated by periods, at least one period unless `short`, and no `_` in the last two
/// segments. Otherwise what was scanned, which stops at the first character that can't continue
/// the domain: a failure costs no more than the domain-like text it read.
fn domain(s: &str, from: usize, short: bool) -> Result<usize, Scan> {
    let mut scan = Scan {
        from,
        stop: s.len(),
        ..Scan::default()
    };
    let (mut segments, mut end, mut seg_start) = (0, 0, from);
    let mut cur = false; // `_` in the current segment
    // The sentinel ends the last segment when `s` is all domain characters.
    let chars = s[from..].char_indices().map(|(i, c)| (from + i, c));
    for (i, c) in chars.chain([(s.len(), ' ')]) {
        if c.is_alphanumeric() || matches!(c, '_' | '-') {
            cur |= c == '_';
            continue;
        }
        if i > seg_start {
            segments += 1;
            end = i; // a trailing period isn't part of the domain
            scan.starts = [scan.starts[1], seg_start];
            scan.under = [scan.under[1], cur];
        }
        if c != '.' || i == seg_start {
            scan.stop = i;
            break;
        }
        cur = false;
        seg_start = i + 1;
    }
    if segments < if short { 1 } else { 2 } || scan.under.contains(&true) {
        return Err(scan);
    }
    Ok(end)
}

/// Where the path after a domain ends: at whitespace or `<`.
fn path_end(s: &str, from: usize) -> usize {
    s[from..]
        .find(|c: char| c.is_whitespace() || c == '<')
        .map_or(s.len(), |n| from + n)
}

/// Trailing punctuation isn't part of a link (GFM "extended autolink path validation"):
/// `?!.,:*_~'"`, an unbalanced `)`, and an entity-like `&name;` at the end.
fn trim_end(s: &str, start: usize, mut end: usize) -> usize {
    // Counted once: only a trimmed `)` changes them (the other trimmed characters, entity-like
    // runs included, hold no parentheses), so the loop is linear.
    let open = s[start..end].matches('(').count();
    let mut close = s[start..end].matches(')').count();
    loop {
        let link = &s[start..end];
        let Some(last) = link.chars().next_back() else {
            return end;
        };
        match last {
            '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"' => end -= 1,
            ')' => {
                if close > open {
                    close -= 1;
                    end -= 1;
                } else {
                    return end;
                }
            }
            ';' => {
                // `&hl;` at the end: drop the whole entity-like run.
                let body = &link[..link.len() - 1];
                match body.rfind('&') {
                    Some(amp)
                        if amp + 1 < body.len()
                            && body[amp + 1..].bytes().all(|c| c.is_ascii_alphanumeric()) =>
                    {
                        end = start + amp;
                    }
                    _ => return end,
                }
            }
            _ => return end,
        }
    }
}

/// An email address around the `@` at `at`: its start and end. The local part is
/// alphanumerics and `.+-_`; the domain is like [`domain`] but the last character must be a
/// letter (a trailing `.` isn't part of it).
fn email(s: &str, at: usize) -> Option<(usize, usize)> {
    let local = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_');
    let start = s[..at]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| local(c))
        .last()
        .map(|(i, _)| i)?;
    let after = &s[at + 1..];
    let mut end = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
        .unwrap_or(after.len());
    while end > 0 && after[..end].ends_with('.') {
        end -= 1;
    }
    let dom = &after[..end];
    if !dom.contains('.')
        || !dom.ends_with(|c: char| c.is_ascii_alphabetic())
        || dom.starts_with('.')
    {
        return None;
    }
    Some((start, at + 1 + end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(s: &str) -> Vec<(&str, String)> {
        find(s, ' ').into_iter().map(|(r, h)| (&s[r], h)).collect()
    }

    #[test]
    fn www_and_urls() {
        assert_eq!(
            links("www.commonmark.org"),
            [("www.commonmark.org", "http://www.commonmark.org".into())]
        );
        assert_eq!(
            links("Visit www.commonmark.org/help for more information."),
            [(
                "www.commonmark.org/help",
                "http://www.commonmark.org/help".into()
            )]
        );
        assert_eq!(
            links("Visit www.commonmark.org."),
            [("www.commonmark.org", "http://www.commonmark.org".into())]
        );
        assert_eq!(
            links("Visit www.commonmark.org/a.b."),
            [(
                "www.commonmark.org/a.b",
                "http://www.commonmark.org/a.b".into()
            )]
        );
        assert_eq!(
            links(
                "http://commonmark.org and https://encrypted.google.com/search?q=Markup+(business)"
            ),
            [
                ("http://commonmark.org", "http://commonmark.org".into()),
                (
                    "https://encrypted.google.com/search?q=Markup+(business)",
                    "https://encrypted.google.com/search?q=Markup+(business)".into()
                )
            ]
        );
        assert_eq!(
            links("HTTPS://X.Y/z"),
            [("HTTPS://X.Y/z", "HTTPS://X.Y/z".into())]
        );
    }

    #[test]
    fn trailing_punctuation_and_parens() {
        assert_eq!(
            links("(www.google.com/search?q=Markup+(business))")[0].0,
            "www.google.com/search?q=Markup+(business)"
        );
        assert_eq!(
            links("www.google.com/search?q=Markup+(business)))")[0].0,
            "www.google.com/search?q=Markup+(business)"
        );
        assert_eq!(
            links("www.google.com/search?q=(business))+ok")[0].0,
            "www.google.com/search?q=(business))+ok"
        );
        assert_eq!(
            links("www.google.com/search?q=commonmark&hl=en")[0].0,
            "www.google.com/search?q=commonmark&hl=en"
        );
        assert_eq!(
            links("www.google.com/search?q=commonmark&hl;")[0].0,
            "www.google.com/search?q=commonmark"
        );
        assert_eq!(links("see https://a.b/c, then")[0].0, "https://a.b/c");
        // Quotes trail a URL (not part of it) and don't stop one from starting.
        assert_eq!(links("\"https://a.b/c\"")[0].0, "https://a.b/c");
        assert_eq!(
            links("www.commonmark.org/he<lp")[0].0,
            "www.commonmark.org/he"
        );
    }

    #[test]
    fn invalid_domains_and_boundaries() {
        assert!(links("www.").is_empty());
        assert!(
            links("www.a_b.c_d").is_empty(),
            "underscore in the last two segments"
        );
        assert_eq!(links("www.a_b.cd.ef")[0].0, "www.a_b.cd.ef");
        assert!(links("xwww.example.com").is_empty(), "not at a boundary");
        assert_eq!(links("*www.example.com*")[0].0, "www.example.com");
        assert!(links("no links here.").is_empty());
    }

    /// cmark-gfm's rules for URLs with a scheme: http, https and ftp, no period needed in the
    /// domain, and anything but a letter (which would make it another scheme) may come before.
    #[test]
    fn scheme_urls_follow_cmark_gfm() {
        for url in [
            "http://localhost:3000",
            "http://localhost",
            "ftp://x.y/z",
            "FTP://X.Y",
            "https://a.b/c",
        ] {
            assert_eq!(links(&format!("go to {url} now"))[0].0, url);
        }
        assert_eq!(links("URL:https://a.b/d")[0].0, "https://a.b/d");
        assert_eq!(links("(https://a.b/d)")[0].0, "https://a.b/d");
        assert_eq!(links("1https://a.b")[0].0, "https://a.b");
        assert_eq!(links("-http://a.b")[0].0, "http://a.b");
        assert_eq!(links("x'http://a.b/'x")[0].0, "http://a.b/'x");
        for none in [
            "xhttps://no.pe",
            "shttp://a.b",
            "gopher://a.b",
            "file://a.b",
            "http://",
            "https:// a.b",
            "ftp://_.x_",
            "http://a_b",
        ] {
            assert!(links(none).is_empty(), "{none}");
        }
        // The text before the URL is the caller's `prev` for the first character.
        assert!(find("https://a.b", 'x').is_empty());
        assert_eq!(find("https://a.b", '`').len(), 1);
    }

    /// `www.` addresses need whitespace, `*`, `_`, `~` or `(` before them, also across the
    /// caller's text runs (code spans and HTML tags end a run without being whitespace).
    #[test]
    fn www_boundary_uses_the_character_before_the_text() {
        for (prev, linked) in [
            (' ', true),
            ('\n', true),
            ('*', true),
            ('_', true),
            ('~', true),
            ('(', true),
            ('`', false),
            ('>', false),
            (')', false),
            (']', false),
            ('a', false),
        ] {
            assert_eq!(
                find("www.a.com", prev).len(),
                usize::from(linked),
                "{prev:?}"
            );
        }
        // Later positions look at the text itself.
        assert_eq!(find("`c` www.a.com", '>').len(), 1);
        assert!(find("c`www.a.com", ' ').is_empty());
    }

    #[test]
    fn pathological_trailing_parentheses_are_trimmed_in_linear_time() {
        let t = std::time::Instant::now();
        for url in ["www.a.bc/", "https://a.bc/"] {
            let all = format!("{url}{}", ")".repeat(200_000));
            assert_eq!(links(&all)[0].0, url);
            let alternating = format!("{url}{}", ".)".repeat(100_000));
            assert_eq!(links(&alternating)[0].0, url);
            // Balanced pairs stay, the rest goes.
            let paired = format!("{url}{}{}", "()".repeat(50_000), ")".repeat(50_000));
            assert_eq!(links(&paired)[0].0, format!("{url}{}", "()".repeat(50_000)));
            // Many lines of it, as a document of such lines would be.
            let lines = format!("{url}{}\n", ")".repeat(2_000)).repeat(100);
            assert_eq!(links(&lines).len(), 100);
            // And entity-like endings and long runs of other trailing punctuation.
            let entities = format!("{url}{}", "&a;".repeat(50_000));
            assert_eq!(links(&entities)[0].0, url);
            let punct = format!("{url}{}", "?!.,:*_~'\"".repeat(20_000));
            assert_eq!(links(&punct)[0].0, url);
        }
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            t.elapsed()
        );
    }

    /// Failed candidates cost what they read, never the rest of the run: every scan in this
    /// module is linear over a whole text run (one line is one run), whatever the input.
    #[test]
    fn failing_candidates_are_scanned_in_linear_time() {
        let mb = 1 << 20;
        let cases: Vec<(&str, String)> = vec![
            ("empty domains", "http:// ".repeat(mb / 7)),
            ("underscore domains", "http://_ ".repeat(mb / 9)),
            ("bare schemes", "https://".repeat(mb / 8)),
            ("ftp and spaces", "ftp://\t".repeat(mb / 6)),
            ("no periods", format!("www.{}", "a".repeat(mb))),
            ("failing www runs", "www.a ".repeat(mb / 6)),
            ("www underscores", "www._".repeat(mb / 5)),
            ("www underscore dots", "www._www._www.a_b".repeat(mb / 17)),
            ("www tildes", "www.a_b~".repeat(mb / 8)),
            ("www empty segments", "www.a..._".repeat(mb / 9)),
            ("www periods", format!("www.{}", ".".repeat(mb))),
            ("www dashes", format!("www.{}", "a-.".repeat(mb / 3))),
            ("www letters", "w".repeat(mb)),
            ("www dots", "www.".repeat(mb / 4)),
            ("scheme letters", "http".repeat(mb / 4)),
            ("scheme colons", "http:/".repeat(mb / 6)),
            ("local parts", "a".repeat(mb) + "@"),
            ("local parts and dots", "a.".repeat(mb / 2) + "@a"),
            ("local part plus signs", "+a".repeat(mb / 2) + "@b"),
            ("at signs", "@".repeat(mb)),
            ("a at a at", "a@".repeat(mb / 2)),
            ("a at a dot", "a@a.".repeat(mb / 4)),
            ("local parts between ats", "aaaa@".repeat(mb / 5)),
            ("domains that fail", "a@".to_string() + &"b".repeat(mb)),
            (
                "domains of dots",
                "a@".to_string() + &"b.".repeat(mb / 2) + "1",
            ),
            ("domain dots", "a@b".to_string() + &".".repeat(mb)),
            ("dashes", "a@b.c-".repeat(mb / 6)),
            ("emails apart", "a.b+c@d.e1 ".repeat(mb / 11)),
            ("emails joined", "a@b.co+".repeat(mb / 7)),
            ("emails in urls", "www.a.bc/x@y.zz ".repeat(mb / 16)),
            ("overlapping emails", "http://a.b/@c.de ".repeat(mb / 17)),
            ("urls", "http://a.b ".repeat(mb / 11)),
            ("url entities", "http://a.b/&a;&a;&a; ".repeat(mb / 21)),
            ("long entity", format!("http://a.b/&{};", "a".repeat(mb))),
            ("semicolons", format!("http://a.b/{}", ";".repeat(mb))),
            ("amps", format!("http://a.b/{}", "&;".repeat(mb / 2))),
            ("trimmed links", "www.a.bc?! ".repeat(mb / 11)),
            ("unicode", "é@é.é www.é_é.é_é ".repeat(mb / 28)),
        ];
        for (name, src) in cases {
            assert!(src.len() >= mb / 2, "{name}: {}", src.len());
            let t = std::time::Instant::now();
            for prev in [' ', 'a'] {
                let _ = find(&src, prev);
            }
            assert!(
                t.elapsed() < std::time::Duration::from_secs(2),
                "{name} took {:?}",
                t.elapsed()
            );
        }
    }

    /// A `www.` inside a failed domain scan (after `_`) is only skipped when it fails too.
    #[test]
    fn failed_domains_do_not_hide_later_links() {
        // `_www` ends the first domain badly, the second starts clean after the `_`.
        assert_eq!(links("www._www.x")[0].0, "www.x");
        assert_eq!(links("www._www.x/p?q.")[0].0, "www.x/p?q");
        assert_eq!(links("www.a_b._www.x/y")[0].0, "www.x/y");
        assert!(links("www._www._www.a_b").is_empty());
        assert!(links("www._www._").is_empty());
        assert!(links("www._www.x_y.cc").is_empty());
        assert!(links("www._www.").is_empty());
        assert_eq!(links("www.a_b.c_d www.e.fg")[0].0, "www.e.fg");
        assert_eq!(links("www.a_b.c_d_http://x.y")[0].0, "http://x.y");
        assert_eq!(links("http://_ www.e.fg")[0].0, "www.e.fg");
        assert_eq!(links("http:// http://a.b")[0].0, "http://a.b");
    }

    #[test]
    fn emails() {
        assert_eq!(
            links("foo@bar.baz"),
            [("foo@bar.baz", "mailto:foo@bar.baz".into())]
        );
        assert_eq!(
            links("hello@mail+xyz.example isn't valid, but hello+xyz@mail.example is."),
            [(
                "hello+xyz@mail.example",
                "mailto:hello+xyz@mail.example".into()
            )]
        );
        assert_eq!(links("a.b-c_d@a.b")[0].0, "a.b-c_d@a.b");
        assert_eq!(links("a.b-c_d@a.b.")[0].0, "a.b-c_d@a.b");
        assert!(links("a.b-c_d@a.b-").is_empty());
        assert!(links("a.b-c_d@a.b_").is_empty());
        assert!(links("@a.b").is_empty());
        assert!(links("a@b").is_empty());
        // The domain must end in a letter, as in cmark-gfm.
        assert!(links("a@b.c1").is_empty());
        assert!(links("a@b.c1.").is_empty());
        assert!(links("a@1.2").is_empty());
        assert_eq!(links("a@b.c1d")[0].0, "a@b.c1d");
        assert_eq!(links("a@b1.co")[0].0, "a@b1.co");
    }

    #[test]
    fn many_and_unicode() {
        let s = "é www.a.com — x@y.z, https://ü.de/ö.";
        let l = links(s);
        assert_eq!(l.len(), 3);
        assert_eq!(l[2].0, "https://ü.de/ö");
        // Never panics on arbitrary slices around multi-byte text.
        for i in 0..s.len() {
            if s.is_char_boundary(i) {
                let _ = find(&s[i..], ' ');
            }
        }
    }
}
