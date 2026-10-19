//! A program that asks X11 selections for things `xclip` and `xsel` don't, for
//! `scripts/clipboard-check.sh`: `clip_probe get|multiple|timestamp|early SELECTION [TARGET]` and
//! `clip_probe manager FILE`, which is a clipboard manager that saves the clipboard to FILE
//! when asked (freedesktop's clipboard manager protocol). Run it only against a server that
//! the script started. It prints what it got (`get`: the text, whole, INCR or not).

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn main() {
    probe::main();
}

#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
fn main() {
    eprintln!("clip_probe: X11 only");
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod probe {
    use std::io::Write;

    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
        SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, Window, WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as _;
    use x11rb::xcb_ffi::XCBConnection;
    use x11rb::{CURRENT_TIME, NONE};

    struct Probe {
        conn: XCBConnection,
        win: Window,
    }

    impl Probe {
        fn new() -> Probe {
            let (conn, screen) = XCBConnection::connect(None).expect("an X server");
            let root = conn.setup().roots[screen].root;
            let win = conn.generate_id().unwrap();
            conn.create_window(
                0,
                win,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .unwrap();
            Probe { conn, win }
        }

        fn atom(&self, name: &str) -> Atom {
            let cookie = self.conn.intern_atom(false, name.as_bytes()).unwrap();
            cookie.reply().unwrap().atom
        }

        fn name(&self, atom: Atom) -> String {
            let reply = self.conn.get_atom_name(atom).unwrap().reply().unwrap();
            String::from_utf8_lossy(&reply.name).into_owned()
        }

        /// Ask `selection` for `target` at `time` into `property`; the property if it said yes.
        fn convert(
            &self,
            selection: &str,
            target: Atom,
            property: Atom,
            time: u32,
        ) -> Option<Atom> {
            let selection = self.atom(selection);
            self.conn
                .convert_selection(self.win, selection, target, property, time)
                .unwrap();
            self.conn.flush().unwrap();
            loop {
                if let Event::SelectionNotify(e) = self.conn.wait_for_event().unwrap()
                    && e.selection == selection
                {
                    return (e.property != NONE).then_some(e.property);
                }
            }
        }

        /// The text of `selection` as `target`, taken a piece at a time if it comes by INCR.
        fn get(&self, selection: &str, target: &str) -> Option<Vec<u8>> {
            let (target, property, incr) =
                (self.atom(target), self.atom("PROBE"), self.atom("INCR"));
            self.convert(selection, target, property, CURRENT_TIME)?;
            let take = || {
                let cookie = self.conn.get_property(
                    true,
                    self.win,
                    property,
                    AtomEnum::ANY,
                    0,
                    u32::MAX / 4,
                );
                let reply = cookie.unwrap().reply().unwrap();
                self.conn.flush().unwrap();
                reply
            };
            let first = take();
            if first.type_ != incr {
                return Some(first.value);
            }
            let mut text = Vec::new();
            loop {
                let Event::PropertyNotify(e) = self.conn.wait_for_event().unwrap() else {
                    continue;
                };
                if e.atom == property && e.state == Property::NEW_VALUE {
                    let piece = take();
                    if piece.value.is_empty() {
                        return Some(text);
                    }
                    text.extend_from_slice(&piece.value);
                }
            }
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        let arg = |i: usize| args.get(i).map_or("", String::as_str);
        let probe = Probe::new();
        match arg(1) {
            "get" => match probe.get(arg(2), arg(3)) {
                Some(text) => std::io::stdout().write_all(&text).unwrap(),
                None => {
                    eprintln!("refused");
                    std::process::exit(1);
                }
            },
            "timestamp" => {
                let (target, property) = (probe.atom("TIMESTAMP"), probe.atom("PROBE"));
                probe
                    .convert(arg(2), target, property, CURRENT_TIME)
                    .expect("TIMESTAMP is answered");
                let reply = probe
                    .conn
                    .get_property(true, probe.win, property, AtomEnum::INTEGER, 0, 1)
                    .unwrap()
                    .reply()
                    .unwrap();
                println!("{}", reply.value32().unwrap().next().unwrap());
            }
            // A request from before the selection was owned is for someone else's content.
            "early" => {
                let (target, property) = (probe.atom("UTF8_STRING"), probe.atom("PROBE"));
                let answer = probe.convert(arg(2), target, property, 1);
                println!(
                    "{}",
                    if answer.is_none() {
                        "refused"
                    } else {
                        "answered"
                    }
                );
            }
            "multiple" => multiple(&probe, arg(2)),
            "manager" => manager(&probe, arg(2)),
            other => panic!("unknown mode {other:?}"),
        }
    }

    /// MULTIPLE: UTF8_STRING and STRING and a target that doesn't exist, in one request. Prints
    /// the pairs that came back (the one that can't be answered with `None` for its property) and
    /// the text of the first two.
    fn multiple(probe: &Probe, selection: &str) {
        let pairs = [
            (probe.atom("UTF8_STRING"), probe.atom("PROBE_1")),
            (probe.atom("STRING"), probe.atom("PROBE_2")),
            (probe.atom("NO_SUCH_TARGET"), probe.atom("PROBE_3")),
        ];
        let (multiple, atom_pair, property) = (
            probe.atom("MULTIPLE"),
            probe.atom("ATOM_PAIR"),
            probe.atom("PROBE"),
        );
        let flat: Vec<Atom> = pairs.iter().flat_map(|&(t, p)| [t, p]).collect();
        probe
            .conn
            .change_property32(PropMode::REPLACE, probe.win, property, atom_pair, &flat)
            .unwrap();
        probe
            .convert(selection, multiple, property, CURRENT_TIME)
            .expect("MULTIPLE is answered");
        let reply = probe
            .conn
            .get_property(true, probe.win, property, atom_pair, 0, 64)
            .unwrap()
            .reply()
            .unwrap();
        let back: Vec<Atom> = reply.value32().unwrap().collect();
        for pair in back.as_chunks::<2>().0 {
            let property = if pair[1] == NONE {
                "None".to_owned()
            } else {
                probe.name(pair[1])
            };
            println!("{} -> {property}", probe.name(pair[0]));
        }
        for (_, property) in &pairs[..2] {
            let reply = probe
                .conn
                .get_property(true, probe.win, *property, AtomEnum::ANY, 0, u32::MAX / 4)
                .unwrap()
                .reply()
                .unwrap();
            println!("{}", String::from_utf8_lossy(&reply.value));
        }
    }

    /// A clipboard manager: owns CLIPBOARD_MANAGER, and when a program asks it (SAVE_TARGETS) to
    /// save the clipboard, asks that program's window for the text, writes it to `file`, and says
    /// it did. Prints `ready` once it owns the selection.
    fn manager(probe: &Probe, file: &str) {
        let manager = probe.atom("CLIPBOARD_MANAGER");
        probe
            .conn
            .set_selection_owner(probe.win, manager, CURRENT_TIME)
            .unwrap();
        probe.conn.flush().unwrap();
        println!("ready");
        std::io::stdout().flush().unwrap();
        loop {
            let Event::SelectionRequest(r) = probe.conn.wait_for_event().unwrap() else {
                continue;
            };
            if r.selection != manager || r.target != probe.atom("SAVE_TARGETS") {
                continue;
            }
            let text = probe
                .get("CLIPBOARD", "UTF8_STRING")
                .expect("the owner answers");
            std::fs::write(file, text).unwrap();
            let notify = SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: r.time,
                requestor: r.requestor,
                selection: r.selection,
                target: r.target,
                property: r.property,
            };
            probe
                .conn
                .send_event(false, r.requestor, EventMask::NO_EVENT, notify)
                .unwrap();
            probe.conn.flush().unwrap();
            return;
        }
    }
}
