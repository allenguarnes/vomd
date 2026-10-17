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
    let (client, server) = UnixStream::pair().unwrap();
    serve(server, answer, end, nth, after);
    client
}

/// A fake bus that never ends the connection itself: it answers every call, and shutting the
/// returned handle closes the connection. For tests that watch what is reported while the
/// connection stays open.
fn open_bus(
    answer: impl Fn(u32, &str) -> Vec<Vec<u8>> + Send + 'static,
) -> (UnixStream, UnixStream) {
    let (client, server) = UnixStream::pair().unwrap();
    let closer = server.try_clone().unwrap();
    serve(server, answer, "", 1, Vec::new());
    (client, closer)
}

fn serve(
    mut server: UnixStream,
    answer: impl Fn(u32, &str) -> Vec<Vec<u8>> + Send + 'static,
    end: &'static str,
    nth: usize,
    after: Vec<Vec<u8>>,
) {
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
                // The client may have gone: the connection is being closed.
                if server.write_all(&m).is_err() {
                    return;
                }
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

fn signal(v: u32) -> Vec<u8> {
    bus_msg(SIGNAL, 0, "SettingChanged", "ssv", &changed(v, KEY))
}

fn read_reply(serial: u32, v: u32) -> Vec<u8> {
    bus_msg(RETURN, serial, "", "v", &variant_u(v, 0))
}

type Reads = std::sync::Arc<std::sync::Mutex<Vec<std::time::Instant>>>;

/// Note a `ReadOne` in `reads`; the number of reads so far.
fn note(reads: &Reads) -> usize {
    let mut r = reads.lock().unwrap();
    r.push(std::time::Instant::now());
    r.len()
}

/// Run `watch` on an open connection, with a channel of what it reports.
fn watch_open(
    stream: UnixStream,
) -> (
    std::sync::mpsc::Receiver<Option<bool>>,
    std::thread::JoinHandle<()>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let watcher = std::thread::spawn(move || {
        run(Bus::over(stream).expect("connected"), |v| {
            tx.send(v).unwrap()
        });
    });
    (rx, watcher)
}

const WAIT: Duration = Duration::from_secs(5);

/// A signal that arrives while `AddMatch` is answered is older than the read that follows it:
/// no re-read, and nothing reported for it.
#[test]
fn a_signal_before_the_read_is_older_than_it() {
    let reads = Reads::default();
    let counted = reads.clone();
    let (stream, closer) = open_bus(move |serial, member| match member {
        "AddMatch" => vec![signal(1), bus_msg(RETURN, serial, "", "", &[])],
        "ReadOne" => {
            note(&counted);
            vec![read_reply(serial, 2)]
        }
        _ => vec![bus_msg(RETURN, serial, "", "", &[])],
    });
    let (rx, watcher) = watch_open(stream);
    assert_eq!(rx.recv_timeout(WAIT), Ok(Some(false)));
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    assert_eq!(reads.lock().unwrap().len(), 1);
    closer.shutdown(std::net::Shutdown::Both).unwrap();
    assert_eq!(rx.recv_timeout(WAIT), Ok(None));
    watcher.join().unwrap();
}

/// The reviewer's scenario: the portal's frontend answers on a worker thread, so a newer signal
/// (dark) can overtake an older reply (light). The reply is reported at once, and a re-read
/// after the first delay finds dark, with no more reads once it is undisturbed.
#[test]
fn a_signal_that_overtook_the_reply_is_settled_by_a_re_read() {
    let reads = Reads::default();
    let counted = reads.clone();
    let (stream, closer) = open_bus(move |serial, member| match member {
        "ReadOne" => match note(&counted) {
            1 => vec![signal(1), read_reply(serial, 2)],
            _ => vec![read_reply(serial, 1)],
        },
        _ => vec![bus_msg(RETURN, serial, "", "", &[])],
    });
    let (rx, watcher) = watch_open(stream);
    assert_eq!(rx.recv_timeout(WAIT), Ok(Some(false)));
    let first = std::time::Instant::now();
    assert_eq!(rx.recv_timeout(WAIT), Ok(Some(true)));
    assert!(first.elapsed() >= Duration::from_millis(45));
    // Settled: nothing more on the open connection, and no third read.
    assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
    let at = reads.lock().unwrap().clone();
    assert_eq!(at.len(), 2);
    assert!(at[1] - at[0] >= Duration::from_millis(45));
    closer.shutdown(std::net::Shutdown::Both).unwrap();
    assert_eq!(rx.recv_timeout(WAIT), Ok(None));
    watcher.join().unwrap();
}

/// A signal before a reply that is the newer of the two (light, whose own signal follows the
/// reply): the re-read confirms the reply, nothing wrong is reported, and it ends there.
#[test]
fn a_reply_newer_than_the_signal_is_confirmed() {
    let reads = Reads::default();
    let counted = reads.clone();
    let (stream, closer) = open_bus(move |serial, member| match member {
        "ReadOne" => match note(&counted) {
            1 => vec![signal(1), read_reply(serial, 2), signal(2)],
            _ => vec![read_reply(serial, 2)],
        },
        _ => vec![bus_msg(RETURN, serial, "", "", &[])],
    });
    let (rx, watcher) = watch_open(stream);
    assert_eq!(rx.recv_timeout(WAIT), Ok(Some(false)));
    assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
    assert_eq!(reads.lock().unwrap().len(), 2);
    closer.shutdown(std::net::Shutdown::Both).unwrap();
    assert_eq!(rx.recv_timeout(WAIT), Ok(None));
    watcher.join().unwrap();
}

/// A portal that sends a conflicting signal before every reply gets no storm: the reads are
/// spaced by the doubling delay (50, 100, 200, 400 ms), so about five in the first second.
#[test]
fn a_contradicting_portal_is_read_with_a_growing_delay() {
    let reads = Reads::default();
    let counted = reads.clone();
    let (stream, closer) = open_bus(move |serial, member| match member {
        "ReadOne" => {
            note(&counted);
            vec![signal(2), read_reply(serial, 1)]
        }
        _ => vec![bus_msg(RETURN, serial, "", "", &[])],
    });
    let started = std::time::Instant::now();
    let (rx, watcher) = watch_open(stream);
    assert_eq!(rx.recv_timeout(WAIT), Ok(Some(true)));
    std::thread::sleep(Duration::from_millis(1000).saturating_sub(started.elapsed()));
    let at = reads.lock().unwrap().clone();
    assert!((4..=5).contains(&at.len()), "{} reads", at.len());
    for (i, pair) in at.windows(2).enumerate() {
        let least = Duration::from_millis(50 << i);
        assert!(pair[1] - pair[0] >= least - Duration::from_millis(5), "{i}");
    }
    // The reads all say dark and no signal is applied: nothing more was reported.
    assert!(rx.try_recv().is_err());
    closer.shutdown(std::net::Shutdown::Both).unwrap();
    assert_eq!(rx.recv_timeout(WAIT), Ok(None));
    watcher.join().unwrap();
}

/// No signal during the read: one read, and no timer afterwards, on an idle open connection.
#[test]
fn an_undisturbed_read_is_the_only_one() {
    let reads = Reads::default();
    let counted = reads.clone();
    let (stream, closer) = open_bus(move |serial, member| match member {
        "ReadOne" => {
            note(&counted);
            vec![read_reply(serial, 1)]
        }
        _ => vec![bus_msg(RETURN, serial, "", "", &[])],
    });
    let (rx, watcher) = watch_open(stream);
    assert_eq!(rx.recv_timeout(WAIT), Ok(Some(true)));
    assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
    assert_eq!(reads.lock().unwrap().len(), 1);
    closer.shutdown(std::net::Shutdown::Both).unwrap();
    assert_eq!(rx.recv_timeout(WAIT), Ok(None));
    watcher.join().unwrap();
}

/// Signals after the reply, even in the same batch, are applied in order with no extra read;
/// ones before it that end on the reply's value don't disturb it, and are not reported.
#[test]
fn signals_after_the_reply_are_applied_in_order() {
    let reads = Reads::default();
    let counted = reads.clone();
    let (stream, closer) = open_bus(move |serial, member| match member {
        "ReadOne" => {
            note(&counted);
            // Light, then dark, before the reply, which holds dark; light, none, dark, dark
            // after.
            vec![
                signal(2),
                signal(1),
                read_reply(serial, 1),
                signal(2),
                signal(0),
                signal(1),
                signal(1),
            ]
        }
        _ => vec![bus_msg(RETURN, serial, "", "", &[])],
    });
    let (rx, watcher) = watch_open(stream);
    let got: Vec<_> = (0..4).map(|_| rx.recv_timeout(WAIT).unwrap()).collect();
    assert_eq!(got, [Some(true), Some(false), None, Some(true)]);
    // Nothing more while the connection is open: converged, not just ended.
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    assert_eq!(reads.lock().unwrap().len(), 1);
    closer.shutdown(std::net::Shutdown::Both).unwrap();
    assert_eq!(rx.recv_timeout(WAIT), Ok(None));
    watcher.join().unwrap();
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
        raced: None,
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
    bus.follow(None, |v| seen.push(v));
    assert!(seen.is_empty());
}
