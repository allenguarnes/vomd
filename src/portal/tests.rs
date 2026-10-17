use super::*;

/// One message read by the fake bus: (serial, member, body).
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

/// A message from the bus: `kind`, replying to `reply_to` (0: none), with `member` (signals)
/// and a body of signature `sig`.
fn bus_msg(kind: u8, reply_to: u32, member: &str, sig: &str, body: &[u8]) -> Vec<u8> {
    bus_msg_to(kind, reply_to, member, sig, body, "")
}

/// `bus_msg` addressed to `dest` (none if empty).
fn bus_msg_to(
    kind: u8,
    reply_to: u32,
    member: &str,
    sig: &str,
    body: &[u8],
    dest: &str,
) -> Vec<u8> {
    let mut m = Writer::default();
    m.0.extend_from_slice(&[b'l', kind, 0, 1]);
    m.u32(body.len() as u32);
    m.u32(1000 + reply_to);
    m.u32(0);
    let start = m.0.len();
    if reply_to != 0 {
        m.pad(8);
        m.0.push(5);
        m.sig("u");
        m.u32(reply_to);
    }
    if !member.is_empty() {
        m.field(1, "o", PORTAL_PATH);
        m.field(2, "s", SETTINGS);
        m.field(3, "s", member);
    }
    if !dest.is_empty() {
        m.field(6, "s", dest);
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

/// A body holding `value` in `wraps` + 1 variants.
fn variant_u(value: u32, wraps: usize) -> Vec<u8> {
    let mut w = Writer::default();
    for _ in 0..wraps {
        w.sig("v");
    }
    w.sig("u");
    w.u32(value);
    w.0
}

fn changed(value: u32, key: &str) -> Vec<u8> {
    let mut w = Writer::default();
    w.str(NAMESPACE);
    w.str(key);
    // The variant's signature follows the second string unpadded; its value aligns to 4 from
    // the start of the body.
    w.sig("u");
    w.u32(value);
    w.0
}

/// Run a fake session bus on one end of a socket pair: authenticate, answer each call with
/// `answer(serial, member) -> messages to send`, then send `after` once `end` is answered and
/// close. `AddMatch` must name the portal as sender.
fn fake_bus(
    answer: impl Fn(u32, &str) -> Vec<Vec<u8>> + Send + 'static,
    end: &'static str,
    after: Vec<Vec<u8>>,
) -> UnixStream {
    fake_bus_nth(answer, end, 1, after)
}

/// `fake_bus` that ends at the `nth` call of `end`.
fn fake_bus_nth(
    answer: impl Fn(u32, &str) -> Vec<Vec<u8>> + Send + 'static,
    end: &'static str,
    nth: usize,
    after: Vec<Vec<u8>>,
) -> UnixStream {
    let (client, mut server) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        let mut auth = Vec::new();
        let mut b = [0u8];
        while !auth.ends_with(b"\r\n") {
            server.read_exact(&mut b).unwrap();
            auth.push(b[0]);
        }
        assert!(auth.starts_with(b"\0AUTH EXTERNAL "), "{auth:?}");
        server.write_all(b"OK 0123456789abcdef\r\n").unwrap();
        let mut begin = [0u8; 7];
        server.read_exact(&mut begin).unwrap();
        assert_eq!(&begin, b"BEGIN\r\n");
        let mut ends = 0;
        while let Some((serial, member, body)) = read_call(&mut server) {
            if member == "AddMatch" {
                let rule = Reader::new(&body).str().unwrap().to_owned();
                assert_eq!(rule, MATCH_RULE);
            }
            for m in answer(serial, &member) {
                server.write_all(&m).unwrap();
            }
            ends += usize::from(member == end);
            if member == end && ends == nth {
                for m in &after {
                    server.write_all(m).unwrap();
                }
                return; // closing ends the client's watch
            }
        }
    });
    client
}

#[test]
fn reads_the_preference_and_follows_changes() {
    let stream = fake_bus(
        |serial, member| match member {
            // A signal the client must skip arrives before the reply.
            "Hello" => vec![
                bus_msg(SIGNAL, 0, "NameAcquired", "", &[]),
                bus_msg(RETURN, serial, "", "", &[]),
            ],
            "ReadOne" => vec![bus_msg(RETURN, serial, "", "v", &variant_u(1, 0))],
            _ => vec![bus_msg(RETURN, serial, "", "", &[])],
        },
        "ReadOne",
        vec![
            bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(2, KEY)),
            // Another key of the namespace is not ours.
            bus_msg(
                SIGNAL,
                0,
                "SettingChanged",
                "ssv",
                &changed(1, "accent-color"),
            ),
            bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(0, KEY)),
            bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(1, KEY)),
        ],
    );
    let mut seen = Vec::new();
    run(Bus::over(stream).expect("connected"), |v| seen.push(v));
    // The bus closing at the end is reported as no preference.
    assert_eq!(seen, [Some(true), Some(false), None, Some(true), None]);
}

/// The signals are asked for before the setting is read. One that arrives while a reply is
/// awaited (the `AddMatch` one, then the `ReadOne` one) isn't known to be newer than the read:
/// the read is reported at once, and if the signal disagrees the setting is read once more, its
/// answer, the portal's current value, reported if it differs.
#[test]
fn a_change_while_starting_is_settled_by_reading_once_more() {
    // (signal during AddMatch, signal during the first ReadOne, what each ReadOne returns in
    // turn, the number of reads made, what is reported)
    for (during_add, during_read, reads, made, expected) in [
        // The signal is older than the read (the portal changed again before it was delivered).
        (Some(1), None, [2, 2], 2, vec![Some(false), None]),
        (Some(1), Some(0), [2, 0], 2, vec![Some(false), None, None]),
        // The read is older than the signal.
        (
            None,
            Some(2),
            [1, 2],
            2,
            vec![Some(true), Some(false), None],
        ),
        // The read, taken again, holds the newer value.
        (None, Some(2), [1, 1], 2, vec![Some(true), None]),
        // Agreeing, or nothing buffered: one read.
        (Some(1), None, [1, 2], 1, vec![Some(true), None]),
        (None, None, [2, 1], 1, vec![Some(false), None]),
    ] {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let stream = fake_bus_nth(
            move |serial, member| {
                let signal = |v| bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(v, KEY));
                let ok = bus_msg(RETURN, serial, "", "", &[]);
                match member {
                    "AddMatch" => during_add.map(signal).into_iter().chain([ok]).collect(),
                    "ReadOne" => {
                        let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let read = bus_msg(RETURN, serial, "", "v", &variant_u(reads[n % 2], 0));
                        // Only the first read is overtaken by a signal.
                        let before = if n == 0 { during_read } else { None };
                        before.map(signal).into_iter().chain([read]).collect()
                    }
                    _ => vec![ok],
                }
            },
            "ReadOne",
            // The bus is closed after the last read the case should make.
            made,
            Vec::new(),
        );
        let mut seen = Vec::new();
        run(Bus::over(stream).expect("connected"), |v| seen.push(v));
        let case = format!("{during_add:?} {during_read:?} {reads:?}");
        assert_eq!(seen, expected, "{case}");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            made,
            "{case}"
        );
    }
}

/// A portal that contradicts every answer (a conflicting signal before each reply) can't keep
/// the thread reading or hold back the first value: the first read is reported before any
/// further call, at most one more read follows, and then the signals are followed.
#[test]
fn startup_reconciliation_is_bounded() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let stream = fake_bus_nth(
        move |serial, member| match member {
            "ReadOne" => {
                counted.fetch_add(1, SeqCst);
                vec![
                    bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(2, KEY)),
                    bus_msg(RETURN, serial, "", "v", &variant_u(1, 0)),
                ]
            }
            _ => vec![bus_msg(RETURN, serial, "", "", &[])],
        },
        "ReadOne",
        // Bounded: the bus ends after the second read, the last one allowed.
        2,
        vec![bus_msg(
            SIGNAL,
            0,
            "SettingChanged",
            "ssv",
            &changed(2, KEY),
        )],
    );
    let mut seen = Vec::new();
    run(Bus::over(stream).expect("connected"), |v| {
        seen.push((v, calls.load(SeqCst)));
    });
    // The first value arrives with one read made, the signal after the reads is followed, and
    // the end of the bus is reported.
    assert_eq!(seen, [(Some(true), 1), (Some(false), 2), (None, 2)]);
}

/// A signal addressed to this connection is delivered whatever the match rule says, so any
/// client can send one: only the portal's broadcasts count, at startup and after.
#[test]
fn a_directed_signal_is_not_the_portals() {
    let rogue = |v| {
        bus_msg_to(
            SIGNAL,
            0,
            "SettingChanged",
            "ssv",
            &changed(v, KEY),
            ":1.99",
        )
    };
    let stream = fake_bus(
        move |serial, member| match member {
            "AddMatch" => vec![rogue(1), bus_msg(RETURN, serial, "", "", &[])],
            "ReadOne" => vec![rogue(1), bus_msg(RETURN, serial, "", "v", &variant_u(2, 0))],
            _ => vec![bus_msg(RETURN, serial, "", "", &[])],
        },
        "ReadOne",
        vec![
            rogue(1),
            bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(1, KEY)),
            rogue(0),
        ],
    );
    let mut seen = Vec::new();
    run(Bus::over(stream).expect("connected"), |v| seen.push(v));
    assert_eq!(seen, [Some(false), Some(true), None]);
}

/// Without the subscription there is nothing to follow or to end: the read is the answer.
#[test]
fn a_refused_subscription_reads_once() {
    let stream = fake_bus(
        |serial, member| match member {
            "AddMatch" => vec![bus_msg(ERROR, serial, "", "", &[])],
            "ReadOne" => vec![bus_msg(RETURN, serial, "", "v", &variant_u(1, 0))],
            _ => vec![bus_msg(RETURN, serial, "", "", &[])],
        },
        "ReadOne",
        Vec::new(),
    );
    let mut seen = Vec::new();
    run(Bus::over(stream).expect("connected"), |v| seen.push(v));
    assert_eq!(seen, [Some(true)]);
}

#[test]
fn falls_back_to_read_on_old_portals() {
    let stream = fake_bus(
        |serial, member| match member {
            "ReadOne" => vec![bus_msg(ERROR, serial, "", "", &[])],
            // `Read` wraps the value in one more variant.
            "Read" => vec![bus_msg(RETURN, serial, "", "v", &variant_u(2, 1))],
            _ => vec![bus_msg(RETURN, serial, "", "", &[])],
        },
        "Read",
        Vec::new(),
    );
    let mut bus = Bus::over(stream).expect("connected");
    assert_eq!(bus.read_setting(), Some(false));
}

#[test]
fn no_portal_is_no_preference() {
    let stream = fake_bus(
        |serial, member| match member {
            "Hello" => vec![bus_msg(RETURN, serial, "", "", &[])],
            _ => vec![bus_msg(ERROR, serial, "", "", &[])],
        },
        "Read",
        Vec::new(),
    );
    let mut bus = Bus::over(stream).expect("connected");
    assert_eq!(bus.read_setting(), None);
}

#[test]
fn a_rejected_login_is_no_bus() {
    let (client, mut server) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        let mut b = [0u8; 64];
        let _ = server.read(&mut b);
        let _ = server.write_all(b"REJECTED EXTERNAL\r\n");
    });
    assert!(Bus::over(client).is_none());
}

#[test]
fn oversized_messages_end_the_connection() {
    let (client, mut server) = UnixStream::pair().unwrap();
    let mut huge = bus_msg(RETURN, 1, "", "", &[]);
    huge[4..8].copy_from_slice(&(MAX_PART + 1).to_le_bytes());
    server.write_all(&huge).unwrap();
    let mut bus = Bus {
        stream: client,
        serial: 0,
        dead: false,
        changed: None,
    };
    assert!(bus.recv().is_err());
}

#[test]
fn values_and_addresses() {
    assert_eq!(scheme(1), Some(true));
    assert_eq!(scheme(2), Some(false));
    assert_eq!(scheme(0), None);
    assert_eq!(scheme(7), None);
    // Only the portal's own signals are asked for.
    assert!(MATCH_RULE.contains("sender='org.freedesktop.portal.Desktop'"));
    assert_eq!(
        unescape("/run/user/1000/bus").unwrap(),
        b"/run/user/1000/bus"
    );
    assert_eq!(unescape("a%2cb%3D").unwrap(), b"a,b=");
    assert!(unescape("bad%2").is_none());
    assert!(open("tcp:host=localhost,port=1").is_none());
    assert!(open("unix:path=/nonexistent/vomd-test-bus").is_none());
    // Variants nested deeper than any portal sends are refused.
    assert_eq!(Reader::new(&variant_u(1, 5)).variant_u32(0), None);
}

/// Only the login and `Hello` have a time limit: a portal that takes longer to start must
/// still be heard.
#[test]
fn the_portal_is_waited_for() {
    let stream = fake_bus(
        |serial, member| match member {
            "ReadOne" => {
                std::thread::sleep(Duration::from_millis(300));
                vec![bus_msg(RETURN, serial, "", "v", &variant_u(1, 0))]
            }
            _ => vec![bus_msg(RETURN, serial, "", "", &[])],
        },
        "ReadOne",
        Vec::new(),
    );
    let mut bus = Bus::over(stream).expect("connected");
    assert_eq!(bus.stream.read_timeout().unwrap(), None);
    assert_eq!(bus.read_setting(), Some(true));
}

/// A failed read can leave half a message behind: the stream isn't used again, so nothing
/// after it is taken for a reply or a change.
#[test]
fn a_failed_read_ends_the_connection() {
    let stream = fake_bus(
        |serial, member| match member {
            "ReadOne" => {
                let mut huge = bus_msg(RETURN, serial, "", "", &[]);
                huge[4..8].copy_from_slice(&(MAX_PART + 1).to_le_bytes());
                // Whatever follows is a valid change that must not be seen.
                vec![
                    huge,
                    bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(1, KEY)),
                ]
            }
            _ => vec![bus_msg(RETURN, serial, "", "", &[])],
        },
        "ReadOne",
        Vec::new(),
    );
    let mut bus = Bus::over(stream).expect("connected");
    assert_eq!(bus.read_setting(), None);
    assert!(bus.dead);
    let mut seen = Vec::new();
    bus.follow(|v| seen.push(v));
    assert!(seen.is_empty());
}
