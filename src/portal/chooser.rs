//! Open's file chooser: the portal's `org.freedesktop.portal.FileChooser.OpenFile`, which
//! shows the desktop's own dialog (KDE, GNOME, or the GTK one under Hyprland and Sway).
//!
//! The answer comes as a `Response` signal on a request object. Its path is known before the
//! call (`…/request/<our unique name>/<token>`), so the signal is subscribed to first and can't
//! be missed. A signal is only taken from the connection that answered the call: the bus stamps
//! senders, so no other client can pass off a file as the portal's choice.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{BUS, Bus, ERROR, Msg, PORTAL, PORTAL_PATH, RETURN, Reader, SIGNAL, Writer};

const FILE_CHOOSER: &str = "org.freedesktop.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// How long a method call may wait for its reply: the D-Bus default. The `Response` is not
/// held to it, the user may browse for as long as they like.
const REPLY_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(25)
};

/// The most `Response` signals kept from before the replies; older ones are dropped. Only the
/// portal's own gets answered (one, in practice), so a flood of them can't grow without bound.
/// (The portal's leaving is kept besides, once.)
const EARLY_MAX: usize = 16;

/// What the chooser gave.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Picked(PathBuf),
    /// The dialog was closed without a file.
    Cancelled,
    /// No portal file chooser (no bus, no portal, no backend with a file chooser, or the call
    /// was refused): try another way. Only before the portal has taken the request: after
    /// that its dialog may be up, and a second one would be wrong.
    Unavailable,
    /// The portal took the request but its answer can't be used (in words for the window).
    Failed(String),
}

/// Ask the portal for a file to open, starting in `folder`, as a dialog of the window `parent`
/// names (`x11:<id>`; empty for none). Blocks until the dialog closes: run it off the main
/// thread.
pub fn choose_file(title: &str, parent: &str, folder: Option<&Path>) -> Choice {
    match Bus::connect() {
        Some(mut bus) => open_file(&mut bus, title, parent, folder, &token()),
        None => Choice::Unavailable,
    }
}

fn failed(why: &str) -> Choice {
    Choice::Failed(why.into())
}

/// A request token no other request of this process has had: the process id, a counter and
/// the time.
fn token() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        "vomd{}_{}_{nanos}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// The request object the portal will create for `token` (`:1.42` → `1_42`).
fn request_path(unique: &str, token: &str) -> String {
    let sender = unique.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

fn response_rule(path: &str) -> String {
    format!("type='signal',interface='{REQUEST}',member='Response',path='{path}'")
}

/// The rule for the bus's word that the connection `name` has left it.
fn gone_rule(name: &str) -> String {
    format!(
        "type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{name}'"
    )
}

/// Whether `m` says `owner`, the connection that held `name`, has left the bus
/// (`NameOwnerChanged`: name, old owner, new owner, the new one empty). Only the bus's own word
/// counts, and only about `owner`: the bus delivers a connection's own messages before the
/// word of its leaving, so an earlier owner's leave can come ahead of a new one's reply.
fn has_left(m: &Msg, name: &str, owner: &str) -> bool {
    let mut r = Reader::new(&m.body);
    m.kind == SIGNAL
        && m.member == "NameOwnerChanged"
        && m.sender == BUS
        && r.str() == Some(name)
        && r.str() == Some(owner)
        && r.str() == Some("")
}

/// Whether `m` is the bus's word that some owner of the portal's name has left, whoever it was.
fn is_leave(m: &Msg) -> bool {
    let mut r = Reader::new(&m.body);
    m.kind == SIGNAL
        && m.member == "NameOwnerChanged"
        && m.sender == BUS
        && r.str() == Some(PORTAL)
        && r.str().is_some()
        && r.str() == Some("")
}

/// `OpenFile`'s arguments (`ssa{sv}`): the parent window, the title, and the options: the
/// token, a Markdown filter and an all-files one, and the folder to start in.
fn open_file_body(title: &str, parent: &str, folder: Option<&Path>, token: &str) -> Vec<u8> {
    let mut w = Writer::default();
    w.str(parent);
    w.str(title);
    let options = w.begin_array(8);
    w.entry("handle_token", "s");
    w.str(token);
    w.entry("filters", "a(sa(us))");
    let filters = w.begin_array(8);
    let markdown = &crate::platform::MARKDOWN_GLOBS[..];
    for (name, patterns) in [("Markdown", markdown), ("All files", &["*"][..])] {
        w.pad(8);
        w.str(name);
        let list = w.begin_array(8);
        for p in patterns {
            w.pad(8);
            w.u32(0); // a glob, not a MIME type
            w.str(p);
        }
        w.end_array(list);
    }
    w.end_array(filters);
    if let Some(folder) = folder {
        use std::os::unix::ffi::OsStrExt;
        // A byte string ending in NUL (`ay`): paths needn't be UTF-8.
        w.entry("current_folder", "ay");
        let bytes = w.begin_array(1);
        w.0.extend_from_slice(folder.as_os_str().as_bytes());
        w.0.push(0);
        w.end_array(bytes);
    }
    w.end_array(options);
    w.0
}

/// Call `OpenFile` and wait for the `Response`.
fn open_file(
    bus: &mut Bus,
    title: &str,
    parent: &str,
    folder: Option<&Path>,
    token: &str,
) -> Choice {
    let Some(unique) = bus.unique.clone() else {
        return Choice::Unavailable;
    };
    let mut path = request_path(&unique, token);
    // Signals that come before the replies are kept: the portal's frontend may answer at once.
    let mut early = Vec::new();
    if !subscribe(bus, &response_rule(&path), &mut early) {
        return Choice::Unavailable;
    }
    // The wait for the `Response` has no limit, so a portal that dies would hold it for good:
    // its name losing its owner ends it. Subscribed before the call, so the portal can't go
    // between its reply and the subscription unseen.
    if !subscribe(bus, &gone_rule(PORTAL), &mut early) {
        return Choice::Unavailable;
    }
    let body = open_file_body(title, parent, folder, token);
    let Some(serial) = bus.send(
        PORTAL,
        PORTAL_PATH,
        FILE_CHOOSER,
        "OpenFile",
        "ssa{sv}",
        &body,
    ) else {
        return Choice::Unavailable;
    };
    let reply = match reply_to(bus, serial, &mut early) {
        Ok(reply) => reply,
        // The portal has the request and may still show its dialog: no second one.
        Err(NoReply::TimedOut) => return failed("the file dialog didn't answer"),
        Err(NoReply::Lost) => return Choice::Unavailable,
    };
    // An error: no portal, or no backend with a file chooser (`UnknownMethod`).
    if reply.kind != RETURN || reply.sender.is_empty() {
        return Choice::Unavailable;
    }
    // From here the portal has the request, and maybe a dialog up: no fallback.
    let Some(handle) = Reader::new(&reply.body).str().map(str::to_owned) else {
        return failed("the file dialog gave an unexpected reply");
    };
    // Portals before version 0.9 make up their own request path.
    if handle != path {
        if !subscribe(bus, &response_rule(&handle), &mut early) {
            return failed("the file dialog's answer couldn't be received");
        }
        path = handle;
    }
    let ours = |m: &Msg| m.member == "Response" && m.path == path && m.sender == reply.sender;
    if let Some(m) = early.iter().find(|m| ours(m)) {
        return response(&m.body);
    }
    if early.iter().any(|m| has_left(m, PORTAL, &reply.sender)) {
        return failed("the file dialog went away");
    }
    loop {
        match bus.recv() {
            Ok(m) if m.kind == SIGNAL && ours(&m) => return response(&m.body),
            Ok(m) if has_left(&m, PORTAL, &reply.sender) => {
                return failed("the file dialog went away");
            }
            Ok(_) => {}
            Err(_) => return failed("the connection to the file dialog was lost"),
        }
    }
}

/// Why a call has no reply.
enum NoReply {
    /// The connection failed.
    Lost,
    /// `REPLY_WAIT` passed.
    TimedOut,
}

/// The reply to call `serial`, keeping the latest `EARLY_MAX` `Response` signals that arrive
/// first in `early`, and each distinct word of the portal's leaving: whose it is, the caller
/// tells once it knows who replied.
fn reply_to(bus: &mut Bus, serial: u32, early: &mut Vec<Msg>) -> Result<Msg, NoReply> {
    let until = Instant::now() + REPLY_WAIT;
    loop {
        match bus.recv_by(until) {
            Ok(Some(m)) if matches!(m.kind, RETURN | ERROR) && m.reply_serial == serial => {
                return Ok(m);
            }
            Ok(Some(m)) if m.kind == SIGNAL && m.member == "Response" => {
                let is_response = |m: &Msg| m.member == "Response";
                if early.iter().filter(|m| is_response(m)).count() == EARLY_MAX
                    && let Some(oldest) = early.iter().position(is_response)
                {
                    early.remove(oldest);
                }
                early.push(m);
            }
            Ok(Some(m)) if is_leave(&m) => {
                if !early.iter().any(|e| is_leave(e) && e.body == m.body) {
                    early.push(m);
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => return Err(NoReply::TimedOut),
            Err(_) => return Err(NoReply::Lost),
        }
    }
}

/// Ask the bus for the signals `rule` names; whether it agreed in time.
fn subscribe(bus: &mut Bus, rule: &str, early: &mut Vec<Msg>) -> bool {
    let mut body = Writer::default();
    body.str(rule);
    let bus_path = "/org/freedesktop/DBus";
    bus.send(BUS, bus_path, BUS, "AddMatch", "s", &body.0)
        .and_then(|serial| reply_to(bus, serial, early).ok())
        .is_some_and(|m| m.kind == RETURN)
}

/// A `Response` (`ua{sv}`): 0 with the chosen `uris`, 1 when the user cancelled, 2 when the
/// dialog ended some other way. Some backends (xdg-desktop-portal-gtk 1.14 and 1.15, -gnome 43
/// and 46) send 2 for Escape or closing the window, so it counts as a cancel.
fn response(body: &[u8]) -> Choice {
    let unusable = || failed("the file dialog gave an answer that can't be used");
    let mut r = Reader::new(body);
    let Some(code) = r.u32() else {
        return unusable();
    };
    match code {
        0 => {}
        1 | 2 => return Choice::Cancelled,
        _ => return unusable(),
    }
    let Some(n) = r.u32() else {
        return unusable();
    };
    r.align(8);
    let Some(end) = r.pos.checked_add(n as usize) else {
        return unusable();
    };
    while r.pos < end {
        r.align(8);
        let (Some(key), Some(sig)) = (r.str(), r.sig()) else {
            return unusable();
        };
        if key == "uris" && sig == "as" {
            return match r.str_array().as_deref() {
                Some([uri, ..]) => file_path(uri).map_or_else(
                    || failed("the file dialog gave a location that isn't a local file"),
                    Choice::Picked,
                ),
                Some([]) => Choice::Cancelled,
                None => unusable(),
            };
        }
        let mut sig = sig.as_bytes();
        if r.skip(&mut sig, 0).is_none() || !sig.is_empty() {
            return unusable();
        }
    }
    Choice::Cancelled
}

/// The local path of a `file://` URI (empty host or `localhost`), `%XX` escapes decoded.
pub fn file_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let mut it = rest.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    // A NUL can't be in a path; `..` is fine, the user picked it.
    if bytes.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests;
