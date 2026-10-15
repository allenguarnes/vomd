//! GFM extended autolinks (GitHub Flavored Markdown spec §6.9): `www.` addresses, `http://` and
//! `https://` URLs, and email addresses written as plain text become links, as on GitHub.
//! pulldown-cmark doesn't do this, so the builder runs [`find`] over plain text runs.

use std::ops::Range;

/// The autolinks in `s`: byte range of the link text and the link target.
pub fn find(s: &str) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    // Every link needs one of these; most text has none, and the parser runs this on all of it.
    if !candidate(s) {
        return out;
    }
    let mut i = 0;
    while i < b.len() {
        // Links start with `w`, `h` or (for emails) contain `@`; only ASCII bytes are tested,
        // so `i` is on a character boundary whenever it is sliced at.
        match b[i] {
            b'w' | b'W' | b'h' | b'H' => {
                // At the beginning, after whitespace, or after `*`, `_`, `~` or `(`.
                let boundary = i == 0 || {
                    let prev = s[..i].chars().next_back().unwrap_or(' ');
                    prev.is_whitespace() || matches!(prev, '*' | '_' | '~' | '(')
                };
                if boundary && let Some((end, href)) = at(s, i) {
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

/// A `www.` or `http(s)://` autolink starting at `i`: its end and target.
fn at(s: &str, i: usize) -> Option<(usize, String)> {
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
    } else if lower("www.") {
        (0, "http://")
    } else {
        return None;
    };
    let domain_end = i + prefix + domain(&rest[prefix..])?;
    let end = trim_end(s, i, path_end(s, domain_end));
    Some((end, format!("{scheme}{}", &s[i..end])))
}

/// Length of a valid domain at the start of `s`: segments of alphanumerics, `_` and `-`
/// separated by periods, at least one period, and no `_` in the last two segments.
fn domain(s: &str) -> Option<usize> {
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
    if segments.len() < 2 || segments.iter().rev().take(2).any(|g| g.contains('_')) {
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
    loop {
        let link = &s[start..end];
        let Some(last) = link.chars().next_back() else {
            return end;
        };
        match last {
            '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"' => end -= 1,
            ')' => {
                let open = link.matches('(').count();
                let close = link.matches(')').count();
                if close > open {
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
/// alphanumerics and `.+-_`; the domain is like [`domain`] but the last character can't be
/// `-` or `_` (and a trailing `.` isn't part of it).
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
    if !dom.contains('.') || dom.ends_with(['-', '_']) || dom.starts_with('.') {
        return None;
    }
    Some((start, at + 1 + end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(s: &str) -> Vec<(&str, String)> {
        find(s).into_iter().map(|(r, h)| (&s[r], h)).collect()
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
        assert!(
            links("\"https://a.b/c\"").is_empty(),
            "a quote isn't a GFM boundary"
        );
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
        assert!(links("http://localhost").is_empty(), "needs a period");
        assert!(links("xwww.example.com").is_empty(), "not at a boundary");
        assert_eq!(links("*www.example.com*")[0].0, "www.example.com");
        assert!(links("no links here.").is_empty());
        assert!(links("ftp://x.y").is_empty());
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
                let _ = find(&s[i..]);
            }
        }
    }
}
