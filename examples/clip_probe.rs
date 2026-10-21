//! A program that asks X11 selections for things `xclip` and `xsel` don't, for
//! `scripts/clipboard-check.sh`: `clip_probe get|multiple|timestamp|early SELECTION [TARGET]`,
//! `clip_probe hold SELECTION`, which owns a selection and never answers for it, and
//! `clip_probe manager FILE`, which is a clipboard manager that saves the clipboard to FILE
//! when asked (freedesktop's clipboard manager protocol). Run it only against a server that
//! the script started. It prints what it got (`get`: the text, whole, INCR or not).
//!
//! `clip_probe read SELECTION [OWN]` is the other way round, on X11 and on Wayland: it opens a
//! window of its own and reads the selection (CLIPBOARD or PRIMARY) with VOMD's own code (see
//! `vomd::clipboard::Clipboard::read`), as a paste into the find bar does, and prints the text it
//! got, or says `nothing` on stderr and exits with 1. With OWN it first offers that text as the
//! selection itself, so it reads what it owns. `clip_probe reread SELECTION` reads it twice with
//! the same clipboard, one read after the other, and prints both, with a NUL byte between.
//! `clip_probe again SELECTION` is that too, but a first read that finds nothing is forgiven (it
//! prints nothing for it), and the window stays open for five seconds after the second.
//!
//! `clip_probe slow SELECTION TEXT DELAY_MS` owns a selection and gives TEXT for it over INCR, four
//! bytes at a time, waiting DELAY_MS after the reader deletes each piece (a slow owner), and
//! prints `ended` after the last. `clip_probe stale SELECTION TEXT` answers its first request
//! three seconds late, over INCR (prints `ended` when the reader has taken all of that), and the
//! ones after at once with TEXT (a stalled owner that wakes up after a paste gave up on it).

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
    use std::collections::VecDeque;
    use std::io::Write;
    use std::time::{Duration, Instant};

    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
        PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent,
        Window, WindowClass,
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
        if arg(1) == "read" {
            return super::window::read(arg(2), args.get(3).map(String::as_str), 1);
        }
        if arg(1) == "reread" {
            return super::window::read(arg(2), None, 2);
        }
        if arg(1) == "again" {
            return super::window::again(arg(2));
        }
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
            "hold" => hold(&probe, arg(2)),
            "slow" => {
                let delay = Duration::from_millis(arg(4).parse().expect("a delay in ms"));
                serve(&probe, arg(2), arg(3), Some(delay));
            }
            "stale" => serve(&probe, arg(2), arg(3), None),
            other => panic!("unknown mode {other:?}"),
        }
    }

    /// Own the selection and never answer a request for it, like a program that is stuck. Prints
    /// `ready` once it owns the selection.
    fn hold(probe: &Probe, selection: &str) {
        let selection = probe.atom(selection);
        probe
            .conn
            .set_selection_owner(probe.win, selection, CURRENT_TIME)
            .unwrap();
        probe.conn.flush().unwrap();
        println!("ready");
        std::io::stdout().flush().unwrap();
        loop {
            probe.conn.wait_for_event().unwrap();
        }
    }

    /// An INCR transfer the reader is taking: the pieces to come, and where they go.
    struct Transfer {
        requestor: Window,
        property: Atom,
        pieces: VecDeque<Vec<u8>>,
    }

    /// Own `selection` and answer requests for UTF8_STRING over INCR, four bytes of `text` at a
    /// time, `delay` after the reader deletes the property each time (`slow`). Without a delay
    /// (`stale`) the first request is answered three seconds late, with a text that is not `text`,
    /// and the ones after at once, with `text`, whole. Prints `ready` once it owns the selection,
    /// and `ended` when it has written the empty piece that ends a transfer.
    fn serve(probe: &Probe, selection: &str, text: &str, delay: Option<Duration>) {
        let selection = probe.atom(selection);
        let (utf8, incr) = (probe.atom("UTF8_STRING"), probe.atom("INCR"));
        probe
            .conn
            .set_selection_owner(probe.win, selection, CURRENT_TIME)
            .unwrap();
        probe.conn.flush().unwrap();
        println!("ready");
        std::io::stdout().flush().unwrap();
        let notify = |r: &SelectionRequestEvent, property: Atom| {
            let event = SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: r.time,
                requestor: r.requestor,
                selection: r.selection,
                target: r.target,
                property,
            };
            probe
                .conn
                .send_event(false, r.requestor, EventMask::NO_EVENT, event)
                .unwrap();
            probe.conn.flush().unwrap();
        };
        let start = |r: &SelectionRequestEvent, text: &str| {
            let mask = EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY;
            probe
                .conn
                .change_window_attributes(
                    r.requestor,
                    &ChangeWindowAttributesAux::new().event_mask(mask),
                )
                .unwrap();
            probe
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    r.requestor,
                    r.property,
                    incr,
                    &[text.len() as u32],
                )
                .unwrap();
            notify(r, r.property);
            Transfer {
                requestor: r.requestor,
                property: r.property,
                pieces: text.as_bytes().chunks(4).map(<[u8]>::to_vec).collect(),
            }
        };
        let mut transfer: Option<Transfer> = None;
        let mut late: Option<(Instant, SelectionRequestEvent)> = None;
        let mut first = true;
        loop {
            let Some(event) = probe.conn.poll_for_event().unwrap() else {
                if late.as_ref().is_some_and(|(at, _)| Instant::now() >= *at) {
                    let (_, r) = late.take().unwrap();
                    transfer = Some(start(&r, "an answer that came too late"));
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            match event {
                Event::SelectionRequest(r) if r.selection == selection && r.target == utf8 => {
                    match delay {
                        Some(_) => transfer = Some(start(&r, text)),
                        None if std::mem::take(&mut first) => {
                            late = Some((Instant::now() + Duration::from_secs(3), r));
                        }
                        None => {
                            probe
                                .conn
                                .change_property8(
                                    PropMode::REPLACE,
                                    r.requestor,
                                    r.property,
                                    utf8,
                                    text.as_bytes(),
                                )
                                .unwrap();
                            notify(&r, r.property);
                        }
                    }
                }
                Event::SelectionRequest(r) => notify(&r, NONE),
                Event::PropertyNotify(e) if e.state == Property::DELETE => {
                    let Some(t) = transfer.as_mut() else { continue };
                    if (e.window, e.atom) != (t.requestor, t.property) {
                        continue;
                    }
                    std::thread::sleep(delay.unwrap_or_default());
                    let piece = t.pieces.pop_front().unwrap_or_default();
                    probe
                        .conn
                        .change_property8(PropMode::REPLACE, e.window, e.atom, utf8, &piece)
                        .unwrap();
                    probe.conn.flush().unwrap();
                    if piece.is_empty() {
                        println!("ended");
                        std::io::stdout().flush().unwrap();
                        transfer = None;
                    }
                }
                _ => {}
            }
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

/// `read`: a window that reads a selection with VOMD's own code.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod window {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use vomd::clipboard::{Clipboard, Which};
    use vomd::copy::Content;
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    use winit::window::{Window, WindowId};

    /// How long the window waits to be shown and focused, and then for the read.
    const WAIT: Duration = Duration::from_secs(15);

    struct App {
        which: Which,
        own: Option<String>,
        window: Option<Arc<Window>>,
        surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
        clipboard: Option<Clipboard>,
        wayland: bool,
        asked: bool,
        /// The reads still to make after the one under way.
        more: u32,
        /// A read that finds nothing is not the end (`again`), and the window stays open a while
        /// after the last one.
        forgive: bool,
        /// When to exit, once the last read is over.
        leave: Option<Instant>,
        proxy: winit::event_loop::EventLoopProxy<Option<String>>,
        deadline: Instant,
    }

    impl App {
        /// Ask for the selection, once: after a Wayland compositor has given the window the
        /// keyboard (it tells the clients that have it of the selections), straight away on X11.
        fn ask(&mut self) {
            if std::mem::replace(&mut self.asked, true) {
                return;
            }
            let (Some(window), which) = (self.window.as_ref(), self.which) else {
                return;
            };
            let handle = window.display_handle().expect("a display").as_raw();
            let mut clipboard = Clipboard::new(handle);
            if let Some(own) = self.own.take() {
                clipboard
                    .set(which, Content::Text(own.into()))
                    .expect("the clipboard offers");
            }
            self.clipboard = Some(clipboard);
            self.read();
        }

        fn read(&mut self) {
            let proxy = self.proxy.clone();
            if let Some(clipboard) = self.clipboard.as_mut() {
                clipboard.read(self.which, move |text| {
                    let _ = proxy.send_event(text);
                });
            }
            self.deadline = Instant::now() + WAIT;
        }
    }

    impl ApplicationHandler<Option<String>> for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            let attrs = Window::default_attributes().with_title("clip_probe");
            let window = Arc::new(event_loop.create_window(attrs).expect("a window"));
            self.wayland = matches!(
                window.display_handle().map(|h| h.as_raw()),
                Ok(RawDisplayHandle::Wayland(_))
            );
            let surface = softbuffer::Context::new(window.clone())
                .and_then(|cx| softbuffer::Surface::new(&cx, window.clone()));
            self.surface = surface.ok();
            // (A Wayland window is mapped by its first buffer, which is made when it is asked to
            // be drawn: winit doesn't ask by itself.)
            window.request_redraw();
            self.window = Some(window);
            self.deadline = Instant::now() + WAIT;
        }

        fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => {
                    let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
                        return;
                    };
                    let size = window.inner_size();
                    if let (Some(w), Some(h)) = (
                        std::num::NonZeroU32::new(size.width),
                        std::num::NonZeroU32::new(size.height),
                    ) && surface.resize(w, h).is_ok()
                        && let Ok(mut buffer) = surface.buffer_mut()
                    {
                        buffer.fill(0x00ff_ffff);
                        let _ = buffer.present();
                    }
                    if !self.wayland {
                        self.ask();
                    }
                }
                WindowEvent::Focused(true) if self.wayland => self.ask(),
                _ => {}
            }
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, text: Option<String>) {
            match text {
                Some(text) => {
                    use std::io::Write;
                    let mut out = std::io::stdout();
                    out.write_all(text.as_bytes()).unwrap();
                    out.flush().unwrap();
                    if self.more > 0 {
                        self.more -= 1;
                        out.write_all(b"\0").unwrap();
                        out.flush().unwrap();
                        return self.read();
                    }
                }
                None if self.forgive && self.more > 0 => {
                    self.more -= 1;
                    use std::io::Write;
                    let mut out = std::io::stdout();
                    out.write_all(b"\0").unwrap();
                    out.flush().unwrap();
                    return self.read();
                }
                None => {
                    eprintln!("nothing");
                    std::process::exit(1);
                }
            }
            if self.forgive {
                self.leave = Some(Instant::now() + Duration::from_secs(5));
                self.deadline = Instant::now() + Duration::from_secs(60);
                return;
            }
            event_loop.exit();
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            if let Some(leave) = self.leave {
                if Instant::now() >= leave {
                    event_loop.exit();
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(leave));
                return;
            }
            if Instant::now() >= self.deadline {
                eprintln!("no answer");
                std::process::exit(2);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
        }
    }

    pub fn again(selection: &str) {
        run(selection, None, 2, true);
    }

    pub fn read(selection: &str, own: Option<&str>, reads: u32) {
        run(selection, own, reads, false);
    }

    fn run(selection: &str, own: Option<&str>, reads: u32, forgive: bool) {
        let which = match selection {
            "CLIPBOARD" => Which::Clipboard,
            "PRIMARY" => Which::Primary,
            other => panic!("unknown selection {other:?}"),
        };
        let event_loop = EventLoop::<Option<String>>::with_user_event()
            .build()
            .expect("an event loop");
        let mut app = App {
            which,
            own: own.map(str::to_owned),
            window: None,
            surface: None,
            clipboard: None,
            wayland: false,
            asked: false,
            more: reads - 1,
            forgive,
            leave: None,
            proxy: event_loop.create_proxy(),
            deadline: Instant::now() + WAIT,
        };
        event_loop.run_app(&mut app).expect("the loop ran");
    }
}
