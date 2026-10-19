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

/// What else a fake bus does.
#[derive(Default)]
struct Opts {
    /// Sent after the bus has agreed to the `NameOwnerChanged` rule (a signal can't come
    /// before it).
    watch: Vec<Vec<u8>>,
    /// `AddMatch` is never answered.
    mute_add_match: bool,
}

/// A fake session bus: `Hello` names us `UNIQUE`, `AddMatch` succeeds (the rules are sent to
/// `rules`), and `OpenFile` is answered by `open_file(serial, body)`; an empty message in its
/// answer hangs up.
fn fake(
    open_file: impl Fn(u32, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
    rules: std::sync::mpsc::Sender<String>,
) -> UnixStream {
    fake_with(open_file, rules, Opts::default())
}

fn fake_with(
    open_file: impl Fn(u32, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
    rules: std::sync::mpsc::Sender<String>,
    opts: Opts,
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
                "AddMatch" if opts.mute_add_match => Vec::new(),
                "AddMatch" => {
                    let rule = Reader::new(&body).str().unwrap().to_owned();
                    let watching = rule.contains("NameOwnerChanged");
                    let _ = rules.send(rule);
                    let mut out =
                        vec![msg(RETURN, serial, "", "", "org.freedesktop.DBus", "", &[])];
                    if watching {
                        out.extend(opts.watch.iter().cloned());
                    }
                    out
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
const PARENT: &str = "x11:1a2b";

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

/// `sender`'s word that `old`, the owner of `name`, left it.
fn left(name: &str, old: &str, sender: &str) -> Vec<u8> {
    let mut w = Writer::default();
    for s in [name, old, ""] {
        w.str(s);
    }
    msg(
        SIGNAL,
        0,
        "/org/freedesktop/DBus",
        "NameOwnerChanged",
        sender,
        "sss",
        &w.0,
    )
}

fn choose(
    open_file: impl Fn(u32, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
) -> (Choice, Vec<String>) {
    choose_with(open_file, Opts::default())
}

fn choose_with(
    open_file: impl Fn(u32, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
    opts: Opts,
) -> (Choice, Vec<String>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let stream = fake_with(open_file, tx, opts);
    let mut bus = Bus::over(stream).expect("connected");
    let choice = super::open_file(&mut bus, "Open", PARENT, Some(Path::new("/home/a")), TOKEN);
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
    // Subscribed to the request's path and to the portal's leaving before calling.
    assert_eq!(rules, [response_rule(&handle()), gone_rule(PORTAL)]);
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
    assert_eq!(
        rules,
        [
            response_rule(&handle()),
            gone_rule(PORTAL),
            response_rule(&old),
        ]
    );
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
fn no_parent_is_an_empty_string() {
    let body = open_file_body("Open", "", None, TOKEN);
    let mut r = Reader::new(&body);
    assert_eq!((r.str(), r.str()), (Some(""), Some("Open")));
}

#[test]
fn a_portal_that_never_answers_the_call_fails() {
    // The call went out, so the portal may show its dialog: no other one is tried.
    let (choice, _) = choose(|_, _| Vec::new());
    assert_eq!(choice, failed("the file dialog didn't answer"));
}

#[test]
fn a_bus_that_never_answers_the_subscription_is_unavailable() {
    // Nothing was sent to the portal yet.
    let opts = Opts {
        mute_add_match: true,
        ..Opts::default()
    };
    let (choice, _) = choose_with(
        |_, _| unreachable!("no call without the subscription"),
        opts,
    );
    assert_eq!(choice, Choice::Unavailable);
}

/// `choose`, given a few seconds: a wait that never ends fails the test instead of hanging it.
fn choose_in_time(
    open_file: impl Fn(u32, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
    opts: Opts,
) -> Choice {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || _ = tx.send(choose_with(open_file, opts).0));
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the wait ended")
}

#[test]
fn a_portal_that_leaves_ends_the_wait() {
    let opts = Opts {
        watch: vec![left(PORTAL, PORTAL_NAME, BUS)],
        ..Opts::default()
    };
    let choice = choose_in_time(|serial, _| vec![handle_reply(serial, &handle())], opts);
    assert_eq!(choice, failed("the file dialog went away"));
}

#[test]
fn a_portal_that_leaves_right_after_replying_ends_the_wait() {
    let after = choose_in_time(
        |serial, _| {
            vec![
                handle_reply(serial, &handle()),
                left(PORTAL, PORTAL_NAME, BUS),
            ]
        },
        Opts::default(),
    );
    assert_eq!(after, failed("the file dialog went away"));
    // The leave of an owner other than the one that replied is no word about the reply's.
    let other_after = choose_in_time(
        |serial, _| {
            vec![
                handle_reply(serial, &handle()),
                left(PORTAL, ":1.3", BUS),
                signal(
                    &handle(),
                    PORTAL_NAME,
                    &response_body(0, &["file:///home/a/r.md"]),
                ),
            ]
        },
        Opts::default(),
    );
    assert_eq!(other_after, Choice::Picked(PathBuf::from("/home/a/r.md")));
}

#[test]
fn the_leave_of_an_earlier_portal_before_the_reply_is_not_the_repliers() {
    // The bus delivers a connection's own messages before the word of its leaving, so the leave
    // of the portal instance that was there before can come ahead of the reply of the new one
    // (usually started by D-Bus activation for the call): its dialog is the one that counts.
    let answer = || {
        signal(
            &handle(),
            PORTAL_NAME,
            &response_body(0, &["file:///home/a/r.md"]),
        )
    };
    let picked = Choice::Picked(PathBuf::from("/home/a/r.md"));
    let flood = || {
        (0..EARLY_MAX * 2).map(|i| {
            let path = request_path(UNIQUE, &format!("t{i}"));
            signal(&path, PORTAL_NAME, &response_body(1, &[]))
        })
    };
    let before = choose_in_time(
        move |serial, _| {
            vec![
                left(PORTAL, ":1.3", BUS),
                handle_reply(serial, &handle()),
                answer(),
            ]
        },
        Opts::default(),
    );
    assert_eq!(before, picked);
    let flooded = choose_in_time(
        move |serial, _| {
            let mut out = vec![left(PORTAL, ":1.3", BUS)];
            out.extend(flood());
            out.push(handle_reply(serial, &handle()));
            out.push(answer());
            out
        },
        Opts::default(),
    );
    assert_eq!(flooded, picked);
    // The replier's own leave, ahead of its reply or in the middle of a flood, still counts.
    let own = choose_in_time(
        |serial, _| {
            vec![
                left(PORTAL, PORTAL_NAME, BUS),
                handle_reply(serial, &handle()),
            ]
        },
        Opts::default(),
    );
    assert_eq!(own, failed("the file dialog went away"));
    let own_flooded = choose_in_time(
        move |serial, _| {
            let mut out = vec![left(PORTAL, PORTAL_NAME, BUS)];
            out.extend(flood());
            out.push(handle_reply(serial, &handle()));
            out
        },
        Opts::default(),
    );
    assert_eq!(own_flooded, failed("the file dialog went away"));
}

#[test]
fn an_answer_sent_before_the_portal_left_still_counts() {
    let choice = choose_in_time(
        |serial, _| {
            vec![
                handle_reply(serial, &handle()),
                signal(
                    &handle(),
                    PORTAL_NAME,
                    &response_body(0, &["file:///home/a/r.md"]),
                ),
                left(PORTAL, PORTAL_NAME, BUS),
            ]
        },
        Opts::default(),
    );
    assert_eq!(choice, Choice::Picked(PathBuf::from("/home/a/r.md")));
}

#[test]
fn only_the_bus_saying_the_portal_left_counts() {
    // Another name leaving, another client saying the portal did, and the portal's name
    // changing hands rather than leaving: the dialog's answer still comes.
    let mut gained = Writer::default();
    for s in [PORTAL, "", PORTAL_NAME] {
        gained.str(s);
    }
    let gained = msg(
        SIGNAL,
        0,
        "/org/freedesktop/DBus",
        "NameOwnerChanged",
        BUS,
        "sss",
        &gained.0,
    );
    let opts = Opts {
        watch: vec![
            left(":1.55", ":1.55", BUS),
            left(PORTAL, PORTAL_NAME, ":1.99"),
            gained,
            signal(
                &handle(),
                PORTAL_NAME,
                &response_body(0, &["file:///home/a/r.md"]),
            ),
        ],
        ..Opts::default()
    };
    let choice = choose_in_time(|serial, _| vec![handle_reply(serial, &handle())], opts);
    assert_eq!(choice, Choice::Picked(PathBuf::from("/home/a/r.md")));
}

#[test]
fn only_the_latest_early_responses_are_kept() {
    let other = |i: usize| request_path(UNIQUE, &format!("t{i}"));
    let calls = move |serial: u32, _: &[u8]| {
        let mut out: Vec<_> = (0..EARLY_MAX + 4)
            .map(|i| signal(&other(i), PORTAL_NAME, &response_body(1, &[])))
            .collect();
        // Not a `Response`: not kept.
        out.insert(3, msg(SIGNAL, 0, "/x", "Other", PORTAL_NAME, "", &[]));
        out.push(handle_reply(serial, &handle()));
        out
    };
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut bus = Bus::over(fake(calls, tx)).expect("connected");
    let serial = bus
        .send(
            PORTAL,
            PORTAL_PATH,
            FILE_CHOOSER,
            "OpenFile",
            "ssa{sv}",
            &[],
        )
        .unwrap();
    let mut early = Vec::new();
    assert!(reply_to(&mut bus, serial, &mut early).is_ok());
    // The oldest four went.
    let paths: Vec<_> = early.iter().map(|m| m.path.clone()).collect();
    let want: Vec<_> = (4..EARLY_MAX + 4)
        .map(|i| request_path(UNIQUE, &format!("t{i}")))
        .collect();
    assert_eq!(paths, want);
    // The one that counts is newest, so a flood before it doesn't lose it.
    let (choice, _) = choose(|serial, _| {
        let mut out: Vec<_> = (0..EARLY_MAX * 2)
            .map(|i| {
                let path = request_path(UNIQUE, &format!("t{i}"));
                signal(&path, PORTAL_NAME, &response_body(1, &[]))
            })
            .collect();
        out.push(signal(
            &handle(),
            PORTAL_NAME,
            &response_body(0, &["file:///home/a/r.md"]),
        ));
        out.push(handle_reply(serial, &handle()));
        out
    });
    assert_eq!(choice, Choice::Picked(PathBuf::from("/home/a/r.md")));
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
    assert_eq!((r.str(), r.str()), (Some(PARENT), Some("Open")));
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
