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
/// `answer(member) -> messages to send`, then send `after` once AddMatch is answered.
fn fake_bus(
    answer: impl Fn(u32, &str) -> Vec<Vec<u8>> + Send + 'static,
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
        while let Some((serial, member, _)) = read_call(&mut server) {
            for m in answer(serial, &member) {
                server.write_all(&m).unwrap();
            }
            if member == "AddMatch" {
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
        ],
    );
    let mut bus = Bus::over(stream).expect("connected");
    assert_eq!(bus.read_setting(), Some(true));
    let mut seen = Vec::new();
    bus.follow(|v| seen.push(v));
    assert_eq!(seen, [Some(false), None]);
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
    };
    assert!(bus.recv().is_err());
}

#[test]
fn values_and_addresses() {
    assert_eq!(scheme(1), Some(true));
    assert_eq!(scheme(2), Some(false));
    assert_eq!(scheme(0), None);
    assert_eq!(scheme(7), None);
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
