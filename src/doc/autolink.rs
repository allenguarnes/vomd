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
                if let Some((end, href)) = at(s, i, before) {
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
fn at(s: &str, i: usize, before: char) -> Option<(usize, String)> {
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
    let domain_end = i + prefix + domain(&rest[prefix..], prefix > 0)?;
    let end = trim_end(s, i, path_end(s, domain_end));
    Some((end, format!("{scheme}{}", &s[i..end])))
}

/// Length of a valid domain at the start of `s`: segments of alphanumerics, `_` and `-`
/// separated by periods, at least one period unless `short`, and no `_` in the last two
/// segments.
fn domain(s: &str, short: bool) -> Option<usize> {
    let mut len = 0;
    let mut segments: Vec<&str> = Vec::new();
    for seg in s.split('.') {
        let n = seg
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(seg.len());
        if n == 0 {
            break;
        }
        segments.push(&seg[..n]);
        len += n;
        if n < seg.len() || len + 1 > s.len() {
            break;
        }
        len += 1; // the period
    }
    // A trailing period isn't part of the domain.
    if s[..len].ends_with('.') {
        len -= 1;
    }
    let needed = if short { 1 } else { 2 };
    if segments.len() < needed || segments.iter().rev().take(2).any(|g| g.contains('_')) {
        return None;
    }
    Some(len)
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
