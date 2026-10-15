//! Navigation: what a link points to, finding a `#fragment` in the
//! document, and the back stack.
//!
//! Links are classified strictly: only `#anchors`, relative Markdown files, and `http(s):` /
//! `mailto:` URLs do anything. Every other scheme (`javascript:`, `file:`, `data:`, ...),
//! absolute paths and anything with control characters is inert.

use std::path::{Component, Path, PathBuf};

use crate::cli::Input;
use crate::doc::Doc;
use crate::layout::Anchor;
use crate::view::rereadable;

/// Most entries the back stack keeps; the oldest are dropped.
pub const BACK_LIMIT: usize = 32;

/// File extensions opened in the window when a relative link points at them.
const MARKDOWN: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// `#fragment` in the current document (percent-decoded; empty = the top).
    Anchor(String),
    /// A Markdown file (resolved against the current document's directory) and an optional
    /// fragment in it.
    Doc {
        path: PathBuf,
        fragment: Option<String>,
    },
    /// `http(s):` or `mailto:`, handed to the desktop.
    External(String),
    /// Does nothing.
    Ignore,
}

/// What `href` points to, for a document in `base` (its directory; `None` for stdin or no
/// document, where relative links are inert).
pub fn classify(href: &str, base: Option<&Path>) -> Target {
    // Browsers strip surrounding whitespace; anything with control characters inside (a
    // newline or tab that could split `java\nscript:`) is refused outright.
    let t = href.trim_matches(|c: char| c.is_ascii_whitespace());
    if t.is_empty() || t.chars().any(char::is_control) {
        return Target::Ignore;
    }
    if let Some(fragment) = t.strip_prefix('#') {
        return Target::Anchor(percent_decode(fragment));
    }
    if let Some(scheme) = scheme(t) {
        return match scheme.to_ascii_lowercase().as_str() {
            "mailto" if mailto_attaches(t) => Target::Ignore,
            "http" | "https" | "mailto" => Target::External(t.to_owned()),
            _ => Target::Ignore,
        };
    }
    // Protocol-relative (`//host/x`) and absolute paths have no meaning for a local file.
    if t.starts_with('/') || t.starts_with('\\') {
        return Target::Ignore;
    }
    let (path, fragment) = match t.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode(f))),
        None => (t, None),
    };
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    let path = percent_decode(path);
    // Decoding can produce what the checks above refused (`%2Fetc%2Fx.md`, `C%3A%5Cx.md`):
    // `Path::join` with an absolute path would replace the document's directory.
    let (Some(base), false) = (
        base,
        path.is_empty() || path.chars().any(char::is_control) || is_absolute(&path),
    ) else {
        return Target::Ignore;
    };
    let markdown = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MARKDOWN.iter().any(|m| m.eq_ignore_ascii_case(e)));
    if !markdown {
        return Target::Ignore;
    }
    Target::Doc {
        path: base.join(path),
        fragment,
    }
}

/// Whether `path` (decoded) is anchored somewhere other than the document's directory: a root,
/// a UNC or drive prefix, on any platform (a Windows path is no relative path on Linux either,
/// as far as a link is concerned).
fn is_absolute(path: &str) -> bool {
    let drive = path.as_bytes();
    path.starts_with(['/', '\\'])
        || (drive.len() >= 2 && drive[0].is_ascii_alphabetic() && drive[1] == b':')
        || Path::new(path)
            .components()
            .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
}

/// Whether a `mailto:` URL carries an `attach`/`attachment` header (RFC 6068 lets a link name
/// any header, and some mail clients attach the file the value names): such links are inert.
/// Names are compared decoded (as many times as it takes, so `%2561ttach` is no way round),
/// ignoring case.
fn mailto_attaches(url: &str) -> bool {
    let query = url.split('#').next().unwrap_or(url);
    let Some((_, query)) = query.split_once('?') else {
        return false;
    };
    query.split(['&', ';']).any(|pair| {
        let mut name = pair.split_once('=').map_or(pair, |(n, _)| n).to_owned();
        loop {
            let decoded = percent_decode(&name);
            if decoded == name {
                break;
            }
            name = decoded;
        }
        name.trim().to_ascii_lowercase().starts_with("attach")
    })
}

/// The URI scheme of `s` (RFC 3986: a letter, then letters, digits, `+`, `-` or `.`, then
/// `:`), if it has one.
fn scheme(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let name = &s[..colon];
    let mut chars = name.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(name)
}

/// Decode `%XX` escapes (invalid ones are kept as written; invalid UTF-8 is replaced).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The block a `#fragment` names: a heading slug or footnote anchor. Tries the fragment as
/// written, then without GitHub's `user-content-` prefix, then ignoring case. An empty
/// fragment is the top of the document.
pub fn find_anchor(doc: &Doc, fragment: &str) -> Option<u32> {
    if fragment.is_empty() {
        return Some(0);
    }
    let lookup = |f: &str| doc.anchors.iter().find(|(s, _)| s == f).map(|&(_, b)| b);
    let bare = fragment.strip_prefix("user-content-").unwrap_or(fragment);
    lookup(fragment).or_else(|| lookup(bare)).or_else(|| {
        let lower = bare.to_lowercase();
        doc.anchors
            .iter()
            .find(|(s, _)| s.to_lowercase() == lower)
            .map(|&(_, b)| b)
    })
}

/// The directory relative links in a document resolve against. None for a document that can't
/// be opened again (stdin, `/dev/stdin`, `<(cmd)`): following a link would replace it for good
/// ([`rereadable`]), and its directory (`/dev`, `/dev/fd`) holds nothing of the author's.
/// `#anchor` links still work there.
pub fn base_dir(input: &Input) -> Option<PathBuf> {
    match input {
        Input::File(path) if rereadable(input) => Some(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
        ),
        Input::File(_) | Input::Stdin | Input::None => None,
    }
}

/// A place to go back to: the document and the reading position in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub input: Input,
    pub anchor: Anchor,
}

/// The back stack: in memory, at most [`BACK_LIMIT`] entries.
#[derive(Clone, Debug, Default)]
pub struct BackStack {
    entries: Vec<Entry>,
}

impl BackStack {
    pub fn push(&mut self, entry: Entry) {
        if self.entries.len() == BACK_LIMIT {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    /// Record that the reader is leaving `here` for `next` (another document, or a place in the
    /// same one). A document that can't be opened again isn't worth coming back to
    /// ([`rereadable`]): stdin, and a pipe named on the command line, are read once (a second
    /// read would block or find them empty), so leaving one for another document forgets
    /// every entry of it, and an empty window has nothing to return to. Back then ends at the
    /// last document that can be reopened.
    pub fn leave(&mut self, here: Entry, next: &Input) {
        match here.input {
            Input::None => {}
            _ if *next != here.input && !rereadable(&here.input) => {
                self.entries.retain(|e| e.input != here.input);
            }
            _ => self.push(here),
        }
    }

    pub fn pop(&mut self) -> Option<Entry> {
        self.entries.pop()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests;
