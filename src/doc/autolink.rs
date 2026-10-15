//! GFM extended autolinks (GitHub Flavored Markdown spec §6.9): `www.` addresses, `http://`,
//! `https://` and `ftp://` URLs, and email addresses written as plain text become links, as on
//! GitHub (cmark-gfm's rules). pulldown-cmark doesn't do this, so the builder runs [`find`] over
//! plain text runs.

use std::ops::Range;

/// Byte ranges of text that came from character references (`&copy;`, `&#35;`): pulldown-cmark
/// decodes them, and cmark-gfm looks for links in the source, where `&` and `;` are there. See
/// [`find`].
#[derive(Clone, Copy)]
pub struct Entities<'a>(pub &'a [Range<usize>]);

impl Entities<'_> {
    /// Whether the character at `i` is part of an entity.
    fn contains(self, i: usize) -> bool {
        let k = self.0.partition_point(|r| r.end <= i);
        self.0.get(k).is_some_and(|r| r.start <= i)
    }

    /// Whether an entity ends at `i`.
    fn ends_at(self, i: usize) -> bool {
        let k = self.0.partition_point(|r| r.end < i);
        self.0.get(k).is_some_and(|r| r.end == i)
    }

    /// Where the first entity starting at or after `i` starts.
    fn next_start(self, i: usize) -> Option<usize> {
        let k = self.0.partition_point(|r| r.start < i);
        self.0.get(k).map(|r| r.start)
    }
}

/// The autolinks in `s`: byte range of the link text and the link target. `prev` is the
/// character before `s` in the source (a space at the start of a block); whether a link may
/// start depends on the character before it.
///
/// `ents` are the entities in `s`, in order. cmark-gfm sees them as written (`;` after the
/// name), we only see what they decode to, so they are treated as the written text would be
/// where it is cheap to: a link doesn't start in one or after one (`;` is not a boundary), a
/// domain ends at one, and one at the end of a link is trimmed (`&copy;`, `&amp;`). An entity
/// inside the path stays in the link, decoded, unless it decodes to whitespace or `<`, which end
/// it (cmark-gfm links the written text, spaces and all). Email addresses are the exception:
/// cmark-gfm finds them in the decoded text, as we do.
pub fn find(s: &str, prev: char, ents: Entities) -> Vec<(Range<usize>, String)> {
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
            b'w' | b'W' | b'h' | b'H' | b'f' | b'F' if !ents.contains(i) => {
                let before = if i == 0 {
                    prev
                } else if ents.ends_at(i) {
                    ';'
                } else {
                    s[..i].chars().next_back().unwrap_or(' ')
                };
                if let Some((end, href)) = at(s, i, before, &mut bad, ents) {
                    out.push((i..end, href));
                    i = end;
                    continue;
                }
            }
            // Emails can start mid-word (`a.b@c.d`): back up over the local part. cmark-gfm finds
            // them in the decoded text, so entities mean nothing here (`a&#64;b.com` is one).
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
fn at(s: &str, i: usize, before: char, bad: &mut Scan, ents: Entities) -> Option<(usize, String)> {
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
    // The prefix is written out, not decoded from entities.
    if ents.next_start(i).is_some_and(|e| e < i + prefix.max(4)) {
        return None;
    }
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
    // The domain ends where an entity starts.
    let cap = ents.next_start(i + prefix).unwrap_or(s.len());
    let domain_end = match domain(&s[..cap], i + prefix, prefix > 0) {
        Ok(end) => end,
        Err(scan) => {
            *bad = scan;
            return None;
        }
    };
    let end = trim_end(s, i, path_end(s, domain_end), ents);
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
/// `?!.,:*_~'"`, an unbalanced `)`, and an entity-like `&name;` at the end, all as cmark-gfm's
/// `autolink_delim` has it. A trailing entity is dropped whole, whatever its name.
fn trim_end(s: &str, start: usize, mut end: usize, ents: Entities) -> usize {
    // Counted once: only a trimmed `)` (or an entity holding one) changes them, so the loop is
    // linear.
    let mut open = s[start..end].matches('(').count();
    let mut close = s[start..end].matches(')').count();
    // The entities in the link, last first: `k` entities start before `end`.
    let mut k = ents.0.partition_point(|r| r.start < end);
    loop {
        while k > 0 && ents.0[k - 1].start >= end {
            k -= 1;
        }
        if k > 0 && ents.0[k - 1].end == end && ents.0[k - 1].start > start {
            let entity = ents.0[k - 1].clone();
            for c in s[entity.clone()].chars() {
                match c {
                    '(' => open = open.saturating_sub(1),
                    ')' => close = close.saturating_sub(1),
                    _ => {}
                }
            }
            end = entity.start;
            k -= 1;
            continue;
        }
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
                // `&hl;` at the end (`&` and letters): drop the whole run, else just the `;`.
                // The letters are scanned once: when there is no `&`, the next character is
                // the last of them and ends the loop.
                let b = s.as_bytes();
                let mut name = end - 2;
                while name > start && b[name].is_ascii_alphabetic() {
                    name -= 1;
                }
                end = if name < end - 2 && b[name] == b'&' && !ents.contains(name) {
                    name
                } else {
                    end - 1
                };
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
        find(s, ' ', Entities(&[]))
            .into_iter()
            .map(|(r, h)| (&s[r], h))
            .collect()
    }

    /// `src` with the entities in `{}` (the braces go): the text and the entities' ranges, as the
    /// builder would hand them over (`{&}` is what `&amp;` decodes to).
    fn with_ents(src: &str) -> (String, Vec<Range<usize>>) {
        let (mut text, mut ents, mut start) = (String::new(), Vec::new(), 0);
        for c in src.chars() {
            match c {
                '{' => start = text.len(),
                '}' => ents.push(start..text.len()),
                _ => text.push(c),
            }
        }
        (text, ents)
    }

    /// The link texts of `src` with entities (see `with_ents`), for a source that starts a block.
    fn ent_links(src: &str) -> Vec<String> {
        let (text, ents) = with_ents(src);
        let found = find(&text, ' ', Entities(&ents));
        found.into_iter().map(|(r, _)| text[r].to_owned()).collect()
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

    /// cmark-gfm's `;` rule, checked against its output: the `;` goes, and with it a `&` and the
    /// letters before it, nothing else (no digits: `&x1;` is not an entity reference).
    #[test]
    fn trailing_semicolons_follow_cmark_gfm() {
        for (src, link) in [
            ("https://example.com/path;", "https://example.com/path"),
            ("www.example.com/path;", "www.example.com/path"),
            ("www.example.com/path&x1;", "www.example.com/path&x1"),
            ("www.example.com/path&hl;", "www.example.com/path"),
            ("www.example.com/path&hl;;", "www.example.com/path"),
            ("www.example.com/path;;;", "www.example.com/path"),
            ("www.example.com/path&;", "www.example.com/path&"),
            ("www.example.com/path1;", "www.example.com/path1"),
            ("www.example.com/path;?;", "www.example.com/path"),
            ("www.example.com/&ab;cd;", "www.example.com/&ab;cd"),
            ("www.example.com/ab;", "www.example.com/ab"),
            ("www.example.com/a&amp&b;", "www.example.com/a&amp"),
            ("www.example.com/&é;", "www.example.com/&é"),
            ("https://a.b&c;", "https://a.b"),
        ] {
            assert_eq!(links(src)[0].0, link, "{src}");
        }
    }

    /// The text of an entity (`{&}` for `&amp;`), as cmark-gfm's links see it written.
    #[test]
    fn entities_end_and_stay_in_links_like_cmark_gfm() {
        // At the end of a link an entity is trimmed, whatever it decodes to.
        for src in [
            "https://example.com/path{©}",
            "https://example.com/path{&}",
            "https://example.com/path{;}",
            "https://example.com/path{.}",
            "https://example.com/path{&}{&}",
            "https://example.com/path{&};",
            "https://example.com/path{&}.",
            "https://example.com/path.{&}?",
            "https://example.com/path{&}{.}",
            "https://example.com/path{ }x",
            "https://example.com/path{<}x",
        ] {
            assert_eq!(ent_links(src), ["https://example.com/path"], "{src}");
        }
        assert_eq!(ent_links("www.a.com/{&};"), ["www.a.com/"]);
        // Parentheses in an entity count as the written ones.
        assert_eq!(ent_links("https://a.b/({)}"), ["https://a.b/("]);
        assert_eq!(ent_links("https://a.b/(x{)}"), ["https://a.b/(x"]);
        // In the middle of a path one stays, decoded, except whitespace and `<`.
        assert_eq!(
            ent_links("https://x.y/?a=1{&}b=2"),
            ["https://x.y/?a=1&b=2"]
        );
        assert_eq!(ent_links("www.a.com/{é}x"), ["www.a.com/éx"]);
        assert_eq!(ent_links("https://x.y/a{ }b"), ["https://x.y/a"]);
        // The `&` of `&amp;copy;` isn't the start of `&copy;`.
        assert_eq!(ent_links("https://a.b/x{&}copy;"), ["https://a.b/x&copy"]);
        // A domain ends at an entity, the path goes on.
        assert_eq!(ent_links("www.caf{é}.com/x"), ["www.café.com/x"]);
        assert_eq!(ent_links("https://caf{é}.com/x"), ["https://café.com/x"]);
        assert!(ent_links("www.{é}.com").is_empty());
        assert_eq!(ent_links("http://a{é}b_c"), ["http://aéb_c"]);
        // A link can't be written with entities or follow one the way text follows text.
        for src in [
            "{h}ttp://a.b",
            "ht{t}p://a.b",
            "http{:}//a.b",
            "http:{/}/a.b",
            "{w}ww.a.com",
            "ww{w}.a.com",
            "www{.}a.com",
            "{ }www.a.com",
            "x{&}www.a.com",
            "{@}a.b@c.com",
        ] {
            let found = ent_links(src);
            assert!(
                found
                    .iter()
                    .all(|l| !l.starts_with("www") && !l.contains("://")),
                "{src}: {found:?}"
            );
        }
        assert_eq!(ent_links("{x}https://a.b"), ["https://a.b"]);
        // Emails: the domain ends at an entity, the local part may hold one.
        assert_eq!(ent_links("a@b.co{é}x"), ["a@b.co"]);
        assert_eq!(ent_links("a{.}b@c.com"), ["a.b@c.com"]);
        assert_eq!(ent_links("a@b.com{&}"), ["a@b.com"]);
        // The entity before a link doesn't hide it.
        assert_eq!(ent_links("x{&} https://a.b/c{&}"), ["https://a.b/c"]);
        // Plain text is as before.
        assert_eq!(ent_links("see https://a.b/c, ok"), ["https://a.b/c"]);
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
        assert!(find("https://a.b", 'x', Entities(&[])).is_empty());
        assert_eq!(find("https://a.b", '`', Entities(&[])).len(), 1);
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
                find("www.a.com", prev, Entities(&[])).len(),
                usize::from(linked),
                "{prev:?}"
            );
        }
        // Later positions look at the text itself.
        assert_eq!(find("`c` www.a.com", '>', Entities(&[])).len(), 1);
        assert!(find("c`www.a.com", ' ', Entities(&[])).is_empty());
    }

    #[test]
    fn pathological_trailing_parentheses_are_trimmed_in_linear_time() {
        // `n` is the length of the longest runs.
        crate::doc::assert_linear("trailing parentheses", 50_000, |n| {
            for url in ["www.a.bc/", "https://a.bc/"] {
                let all = format!("{url}{}", ")".repeat(n));
                assert_eq!(links(&all)[0].0, url);
                let alternating = format!("{url}{}", ".)".repeat(n / 2));
                assert_eq!(links(&alternating)[0].0, url);
                // Balanced pairs stay, the rest goes.
                let paired = format!("{url}{}{}", "()".repeat(n / 4), ")".repeat(n / 4));
                assert_eq!(links(&paired)[0].0, format!("{url}{}", "()".repeat(n / 4)));
                // Many lines of it, as a document of such lines would be.
                let lines = format!("{url}{}\n", ")".repeat(2_000)).repeat(n / 2_000);
                assert_eq!(links(&lines).len(), n / 2_000);
                // And entity-like endings and long runs of other trailing punctuation.
                let entities = format!("{url}{}", "&a;".repeat(n / 4));
                assert_eq!(links(&entities)[0].0, url);
                let semicolons = format!("{url}{}", ";".repeat(n));
                assert_eq!(links(&semicolons)[0].0, url);
                let doubled = format!("{url}{}", "&a;;".repeat(n / 4));
                assert_eq!(links(&doubled)[0].0, url);
                // A long run of letters before a `;` is read once, whether or not an `&` starts
                // it.
                let name = format!("{url}&{};", "a".repeat(n));
                assert_eq!(links(&name)[0].0, url);
                let letters = format!("{url}{};", "a".repeat(n));
                assert_eq!(links(&letters)[0].0, &letters[..letters.len() - 1]);
                let each = format!("{url}{}", "a;".repeat(n / 2));
                assert_eq!(links(&each)[0].0, &each[..each.len() - 1]);
                // Entities from the parser: a long run of them at the end, or one after another
                // with a link between.
                let amps = format!("{url}{}", "{&}".repeat(n / 2));
                assert_eq!(ent_links(&amps), [url]);
                let parens = format!("{url}{}", "{)}".repeat(n / 2));
                assert_eq!(ent_links(&parens), [url]);
                let paired = format!("{url}{}", "({)}".repeat(n / 4));
                let kept = format!("{url}{}{}", "()".repeat(n / 4 - 1), "(");
                assert_eq!(ent_links(&paired), [kept]);
                let between = format!("{url}{{&}} ").repeat(n / 4);
                assert_eq!(ent_links(&between).len(), n / 4);
                let punct = format!("{url}{}", "?!.,:*_~'\"".repeat(n / 10));
                assert_eq!(links(&punct)[0].0, url);
            }
        });
    }

    /// Failed candidates cost what they read, never the rest of the run: every scan in this
    /// module is linear over a whole text run (one line is one run), whatever the input.
    #[test]
    fn failing_candidates_are_scanned_in_linear_time() {
        // Each case at a quarter of the size and at full size (a megabyte): the time may grow
        // about fourfold, not sixteenfold.
        let cases = |mb: usize| -> Vec<(&str, String)> {
            vec![
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
                ("alphabetic runs", format!("www.a.bc/&{}", "a".repeat(mb))),
                (
                    "alphabetic runs and semicolons",
                    "www.a.bc/aaaa;".repeat(mb / 14),
                ),
                ("entity-like runs", "www.a.bc/&aa;;;".repeat(mb / 15)),
                ("unicode", "é@é.é www.é_é.é_é ".repeat(mb / 28)),
            ]
        };
        let mb = 1 << 20;
        let (small, large) = (cases(mb / 4), cases(mb));
        for ((name, small), (_, large)) in small.iter().zip(&large) {
            assert!(large.len() >= mb / 2, "{name}: {}", large.len());
            let run = |src: &str| {
                for prev in [' ', 'a'] {
                    let _ = find(src, prev, Entities(&[]));
                }
            };
            crate::doc::assert_scales_linearly(name, || run(small), || run(large));
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

    /// cmark-gfm finds emails in the decoded text: an entity may be the `@`, a `.`, a letter of
    /// either part, and doesn't cut the address short (unlike the guards on URLs and `www.`).
    #[test]
    fn emails_ignore_entities() {
        for src in [
            "a{@}b.com",
            "a@b{.}com",
            "a@b.c{o}m",
            "a{.}b@c.com",
            "x{a}@b.com",
            "a@{b}.com",
            "{a}{@}{b.com}",
            "a@b.c{o}{m}",
        ] {
            let (text, ents) = with_ents(src);
            assert_eq!(ent_links(src), std::slice::from_ref(&text), "{src}");
            assert_eq!(
                find(&text, ' ', Entities(&ents))[0].1,
                format!("mailto:{text}"),
                "{src}"
            );
        }
        // Where the domain ends because the next character isn't one, an entity ends it too.
        assert_eq!(ent_links("a@b.co{é}x"), ["a@b.co"]);
        assert_eq!(ent_links("a@b.co{&}x"), ["a@b.co"]);
        // The restrictions for the other links stay.
        assert!(ent_links("{h}ttp://a.b").is_empty());
        assert!(ent_links("{w}ww.a.com").is_empty());
        assert_eq!(ent_links("www.a.co{m}"), ["www.a.co"]);
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
                let _ = find(&s[i..], ' ', Entities(&[]));
            }
        }
    }
}
