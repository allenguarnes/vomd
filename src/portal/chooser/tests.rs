use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use super::*;

const UNIQUE: &str = ":1.42";
const PORTAL_NAME: &str = ":1.7";

/// A message from the bus: `kind`, replying to `reply_to` (0: none), with these header fields
/// (empty: absent) and a body of signature `sig`.
fn msg(
    kind: u8,
    reply_to: u32,
    path: &str,
    member: &str,
    sender: &str,
    sig: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut m = Writer::default();
    m.0.extend_from_slice(&[b'l', kind, 0, 1]);
    m.u32(body.len() as u32);
    m.u32(5000 + reply_to);
    m.u32(0);
    let start = m.0.len();
    if reply_to != 0 {
        m.pad(8);
        m.0.push(5);
        m.sig("u");
        m.u32(reply_to);
    }
    if !path.is_empty() {
        m.field(1, "o", path);
    }
    if !member.is_empty() {
        m.field(2, "s", REQUEST);
        m.field(3, "s", member);
    }
    if !sender.is_empty() {
        m.field(7, "s", sender);
    }
    if !sig.is_empty() {
        m.field(8, "g", sig);
    }
    let len = (m.0.len() - start) as u32;
    m.0[12..16].copy_from_slice(&len.to_le_bytes());
    m.pad(8);
    m.0.extend_from_slice(body);
    m.0
}

/// One call read by the fake bus: (serial, member, body).
fn read_call(s: &mut UnixStream) -> Option<(u32, String, Vec<u8>)> {
    let mut fixed = [0u8; 16];
    s.read_exact(&mut fixed).ok()?;
    let word = |i: usize| u32::from_le_bytes(fixed[i..i + 4].try_into().unwrap());
    let (body_len, serial, fields_len) = (word(4), word(8), word(12));
    let mut fields = vec![0; (fields_len as usize).next_multiple_of(8)];
    s.read_exact(&mut fields).ok()?;
    let mut body = vec![0; body_len as usize];
    s.read_exact(&mut body).ok()?;
    let mut r = Reader::new(&fields[..fields_len as usize]);
    let mut member = String::new();
    while r.pos < fields_len as usize {
        r.align(8);
        let code = r.byte()?;
        match r.sig()? {
            "g" => _ = r.sig()?,
            _ => {
                let v = r.str()?;
                if code == 3 {
                    member = v.to_owned();
                }
            }
        }
    }
    Some((serial, member, body))
}

/// A fake session bus: `Hello` names us `UNIQUE`, `AddMatch` succeeds (the rules are sent to
/// `rules`), and `OpenFile` is answered by `open_file(serial, body)`; an empty message in its
/// answer hangs up.
fn fake(
    open_file: impl Fn(u32, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
    rules: std::sync::mpsc::Sender<String>,
) -> UnixStream {
    let (client, mut server) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        let mut auth = Vec::new();
        let mut b = [0u8];
        while !auth.ends_with(b"\r\n") {
            server.read_exact(&mut b).unwrap();
            auth.push(b[0]);
        }
        server.write_all(b"OK 0123456789abcdef\r\n").unwrap();
        let mut begin = [0u8; 7];
        server.read_exact(&mut begin).unwrap();
        while let Some((serial, member, body)) = read_call(&mut server) {
            let out = match member.as_str() {
                "Hello" => {
                    let mut w = Writer::default();
                    w.str(UNIQUE);
                    vec![msg(
                        RETURN,
                        serial,
                        "",
                        "",
                        "org.freedesktop.DBus",
                        "s",
                        &w.0,
                    )]
                }
                "AddMatch" => {
                    let _ = rules.send(Reader::new(&body).str().unwrap().to_owned());
                    vec![msg(RETURN, serial, "", "", "org.freedesktop.DBus", "", &[])]
                }
                "OpenFile" => open_file(serial, &body),
                _ => vec![msg(ERROR, serial, "", "", "", "", &[])],
            };
            for m in out {
                if m.is_empty() || server.write_all(&m).is_err() {
                    return;
                }
            }
        }
    });
    client
}

const TOKEN: &str = "t1";

fn handle() -> String {
    request_path(UNIQUE, TOKEN)
}

fn handle_reply(serial: u32, path: &str) -> Vec<u8> {
    let mut w = Writer::default();
    w.str(path);
    msg(RETURN, serial, "", "", PORTAL_NAME, "o", &w.0)
}

/// A `Response` body: `code`, then a `current_filter` result (to be skipped) and `uris`.
fn response_body(code: u32, uris: &[&str]) -> Vec<u8> {
    let mut w = Writer::default();
    w.u32(code);
    let results = w.begin_array(8);
    w.entry("current_filter", "(sa(us))");
    w.pad(8);
    w.str("Markdown");
    let list = w.begin_array(8);
    w.pad(8);
    w.u32(0);
    w.str("*.md");
    w.end_array(list);
    w.entry("uris", "as");
    let a = w.begin_array(4);
    for u in uris {
        w.str(u);
    }
    w.end_array(a);
    w.end_array(results);
    w.0
}

fn signal(path: &str, sender: &str, body: &[u8]) -> Vec<u8> {
    msg(SIGNAL, 0, path, "Response", sender, "ua{sv}", body)
}

fn choose(
    open_file: impl Fn(u32, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
) -> (Choice, Vec<String>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let stream = fake(open_file, tx);
    let mut bus = Bus::over(stream).expect("connected");
    let choice = super::open_file(&mut bus, "Open", Some(Path::new("/home/a")), TOKEN);
    (choice, rx.try_iter().collect())
}

#[test]
fn a_picked_file() {
    let (choice, rules) = choose(|serial, _| {
        vec![
            handle_reply(serial, &handle()),
            signal(
                &handle(),
                PORTAL_NAME,
                &response_body(0, &["file:///home/a%20b/n%C3%A9.md", "file:///x.md"]),
            ),
        ]
    });
    assert_eq!(choice, Choice::Picked(PathBuf::from("/home/a b/né.md")));
    // Subscribed to the request's path before calling.
    assert_eq!(rules, [response_rule(&handle())]);
}

#[test]
fn cancelled_and_failed_dialogs() {
    let (choice, _) = choose(|serial, _| {
        vec![
            handle_reply(serial, &handle()),
            signal(&handle(), PORTAL_NAME, &response_body(1, &[])),
        ]
    });
    assert_eq!(choice, Choice::Cancelled);
    // Some backends send 2 for Escape: a cancel, not a reason to ask another way.
    let (choice, _) = choose(|serial, _| {
        vec![
            handle_reply(serial, &handle()),
            signal(&handle(), PORTAL_NAME, &response_body(2, &[])),
        ]
    });
    assert_eq!(choice, Choice::Cancelled);
    let (choice, _) = choose(|serial, _| {
        vec![
            handle_reply(serial, &handle()),
            signal(&handle(), PORTAL_NAME, &response_body(7, &[])),
        ]
    });
    assert!(matches!(choice, Choice::Failed(_)), "{choice:?}");
    // Success with no file is no file.
    let (choice, _) = choose(|serial, _| {
        vec![
            handle_reply(serial, &handle()),
            signal(&handle(), PORTAL_NAME, &response_body(0, &[])),
        ]
    });
    assert_eq!(choice, Choice::Cancelled);
}

#[test]
fn only_the_portals_response_counts() {
    let (choice, _) = choose(|serial, _| {
        vec![
            handle_reply(serial, &handle()),
            // Another client's signal on the same path, and the portal's on another path.
            signal(
                &handle(),
                ":1.99",
                &response_body(0, &["file:///etc/shadow"]),
            ),
            signal(
                &request_path(UNIQUE, "other"),
                PORTAL_NAME,
                &response_body(0, &["file:///etc/passwd"]),
            ),
            signal(
                &handle(),
                PORTAL_NAME,
                &response_body(0, &["file:///home/a/r.md"]),
            ),
        ]
    });
    assert_eq!(choice, Choice::Picked(PathBuf::from("/home/a/r.md")));
}

#[test]
fn a_response_before_the_reply_is_kept() {
    let (choice, _) = choose(|serial, _| {
        vec![
            signal(
                &handle(),
                PORTAL_NAME,
                &response_body(0, &["file:///home/a/r.md"]),
            ),
            handle_reply(serial, &handle()),
        ]
    });
    assert_eq!(choice, Choice::Picked(PathBuf::from("/home/a/r.md")));
}

#[test]
fn an_old_portals_own_request_path() {
    let old = format!("{PORTAL_PATH}/request/x/y");
    let path = old.clone();
    let (choice, rules) = choose(move |serial, _| {
        vec![
            handle_reply(serial, &path),
            signal(
                &path,
                PORTAL_NAME,
                &response_body(0, &["file:///home/a/r.md"]),
            ),
        ]
    });
    assert_eq!(choice, Choice::Picked(PathBuf::from("/home/a/r.md")));
    assert_eq!(rules, [response_rule(&handle()), response_rule(&old)]);
}

#[test]
fn no_file_chooser_is_unavailable() {
    let (choice, _) = choose(|serial, _| vec![msg(ERROR, serial, "", "", PORTAL_NAME, "", &[])]);
    assert_eq!(choice, Choice::Unavailable);
}

#[test]
fn a_location_that_is_not_a_file_fails() {
    for uri in ["https://x/a.md", "file://host/a.md", "/home/a/r.md"] {
        let (choice, _) = choose(move |serial, _| {
            vec![
                handle_reply(serial, &handle()),
                signal(&handle(), PORTAL_NAME, &response_body(0, &[uri])),
            ]
        });
        assert!(matches!(choice, Choice::Failed(_)), "{uri}: {choice:?}");
    }
}

#[test]
fn a_connection_lost_after_the_request_fails_but_before_it_is_unavailable() {
    // Hung up once the request is taken: its dialog may be up, so no other way is tried.
    let (choice, _) = choose(|serial, _| vec![handle_reply(serial, &handle()), Vec::new()]);
    assert!(matches!(choice, Choice::Failed(_)), "{choice:?}");
    // Hung up before the reply: the portal never had it.
    let (choice, _) = choose(|_, _| vec![Vec::new()]);
    assert_eq!(choice, Choice::Unavailable);
}

#[test]
fn the_request_is_well_formed() {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = choose(move |serial, body| {
        tx.send(body.to_vec()).unwrap();
        vec![
            handle_reply(serial, &handle()),
            signal(&handle(), PORTAL_NAME, &response_body(1, &[])),
        ]
    });
    let body = rx.recv().unwrap();
    // The whole body is one `ssa{sv}`, and every option is there.
    let mut r = Reader::new(&body);
    let mut sig: &[u8] = b"ssa{sv}";
    for _ in 0..3 {
        r.skip(&mut sig, 0).unwrap();
    }
    assert_eq!(r.pos, body.len());
    let mut r = Reader::new(&body);
    assert_eq!((r.str(), r.str()), (Some(""), Some("Open")));
    let n = r.u32().unwrap() as usize;
    r.align(8);
    let end = r.pos + n;
    let mut keys = Vec::new();
    while r.pos < end {
        r.align(8);
        let key = r.str().unwrap().to_owned();
        let sig = r.sig().unwrap().to_owned();
        match key.as_str() {
            "handle_token" => assert_eq!(r.str(), Some(TOKEN)),
            "current_folder" => {
                let len = r.u32().unwrap() as usize;
                assert_eq!(r.take(len), Some(&b"/home/a\0"[..]));
            }
            _ => r.skip(&mut sig.as_bytes(), 0).unwrap(),
        }
        keys.push((key, sig));
    }
    assert_eq!(
        keys,
        [
            ("handle_token".to_owned(), "s".to_owned()),
            ("filters".to_owned(), "a(sa(us))".to_owned()),
            ("current_folder".to_owned(), "ay".to_owned()),
        ]
    );
}

#[test]
fn file_uris() {
    assert_eq!(file_path("file:///a/b.md"), Some(PathBuf::from("/a/b.md")));
    assert_eq!(
        file_path("file://localhost/a%2Fb%25.md"),
        Some(PathBuf::from("/a/b%.md"))
    );
    // Not UTF-8 is still a path.
    assert_eq!(
        file_path("file:///a/%FF.md"),
        Some(PathBuf::from(OsString::from_vec(b"/a/\xff.md".to_vec())))
    );
    assert_eq!(file_path("file://host/a.md"), None);
    assert_eq!(file_path("https://x/a.md"), None);
    assert_eq!(file_path("file:///a/%0.md"), None);
    assert_eq!(file_path("file:///a/%00.md"), None);
}
