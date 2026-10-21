//! The X11 clipboard: ownership of CLIPBOARD and PRIMARY, and the answers to the programs that
//! paste them.
//!
//! winit reads every event through Xlib on its own connection and drops the selection ones, so
//! this has a connection of its own to the same display (XCB through `x11rb`, which winit and
//! softbuffer already load, and which `dlopen`s `libxcb` as they do). A thread blocked in
//! `wait_for_event` serves it: it takes a selection when a copy asks, and answers the requests
//! that follow, with the text made as it is sent (in pieces, over INCR, when it is long).
//! The same thread reads a paste: it asks the owner of a selection to convert it into a property
//! of our window (`UTF8_STRING`, else `STRING`), takes the answer, whole or over INCR, up to a
//! limit, and gives up after a time; a selection that is ours is answered from what we offer.
//! Nothing runs until the first copy or paste, and nothing wakes while no program asks.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent,
    Timestamp, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{CURRENT_TIME, NONE};

use super::{READ_CAP, READ_WAIT, Reply, Which, decode, latin1};
use crate::copy::{CHUNK, Chunks, Content};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD, TARGETS, TIMESTAMP, MULTIPLE, SAVE_TARGETS, INCR, UTF8_STRING, TEXT,
        ATOM_PAIR, CLIPBOARD_MANAGER,
        TEXT_PLAIN_UTF8: b"text/plain;charset=utf-8",
        TEXT_PLAIN: b"text/plain",
        // Our own property, only to learn the server's time (see `Owner::set`).
        VOMD_SELECTION,
        // Where the owner of a selection puts the text we ask it for (see `Server::ask`): the first
        // of the names `Pastes` rotates through.
        VOMD_PASTE,
    }
}

/// A requestor that doesn't take the next piece of an INCR transfer in this long is given up on.
const STALLED: Duration = Duration::from_secs(5);
/// How often a thread with a transfer open looks at the clock (it sleeps in between).
const POLL: Duration = Duration::from_millis(10);
/// How many property names pastes rotate through (a name, once interned, is never freed).
const PASTE_NAMES: usize = 4;
/// How long a property that a paste gave up on is left alone, with no new write to it, before it
/// serves another paste.
const QUIET: Duration = Duration::from_secs(30);

/// How long the window waits, as it closes, for a clipboard manager to take the clipboard.
const SAVE_WAIT: Duration = Duration::from_secs(1);

/// What the main thread and the thread that serves the selections tell each other.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when the clipboard manager has answered a request to save the clipboard.
    saved: Condvar,
}

#[derive(Default)]
struct State {
    /// Content to take a selection with (by [`Which`]), until the thread has.
    pending: [Option<Content>; 2],
    /// The owner is gone: the thread ends.
    stop: bool,
    /// The thread owns CLIPBOARD.
    clipboard: bool,
    /// The clipboard manager answered.
    saved: bool,
    /// Pastes to read, until the thread has begun them.
    reads: Vec<(Which, Reply)>,
    /// The thread has ended: a paste finds nothing.
    dead: bool,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn index(which: Which) -> usize {
    match which {
        Which::Clipboard => 0,
        Which::Primary => 1,
    }
}

/// The handle the window keeps: copies go through it, to the thread.
pub struct Owner {
    conn: Arc<XCBConnection>,
    win: Window,
    atoms: Atoms,
    shared: Arc<Shared>,
}

impl Owner {
    /// Connect, make the window that owns the selections, and start the thread.
    pub fn start() -> Result<Owner, String> {
        let (conn, screen) = XCBConnection::connect(None).map_err(why)?;
        let conn = Arc::new(conn);
        let root = conn.setup().roots[screen].root;
        let win = conn.generate_id().map_err(why)?;
        // An unmapped 1×1 window that only receives events.
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
        .map_err(why)?;
        let atoms = Atoms::new(&*conn).map_err(why)?.reply().map_err(why)?;
        let shared = Arc::new(Shared::default());
        let server = Server {
            conn: conn.clone(),
            win,
            atoms,
            shared: shared.clone(),
            owned: [None, None],
            transfers: Transfers::default(),
            reading: None,
            paste: atoms.VOMD_PASTE,
            names: Pastes::new(atoms.VOMD_PASTE),
            queued: None,
        };
        std::thread::Builder::new()
            .name("vomd-clipboard".into())
            .spawn(move || server.run())
            .map_err(why)?;
        Ok(Owner {
            conn,
            win,
            atoms,
            shared,
        })
    }

    /// Offer `content` as the selection `which`. ICCCM wants the time of the event that caused
    /// the copy, which winit doesn't pass on and `CurrentTime` isn't allowed for; so this leaves
    /// the content and makes the server stamp an event (a zero-length append to a property of the
    /// window, the usual way to learn its time), and the thread claims the selection with that.
    pub fn set(&self, which: Which, content: Content) {
        self.shared.state().pending[index(which)] = Some(content);
        self.wake();
    }

    /// Read the selection `which` for a paste. The thread does it once it is woken, and `reply`
    /// gets the text (or nothing: see [`super::Clipboard::read`]).
    pub fn read(&self, which: Which, reply: Reply) {
        let mut state = self.shared.state();
        if state.dead {
            drop(state);
            reply.send(None);
            return;
        }
        state.reads.push((which, reply));
        drop(state);
        self.wake();
    }

    /// As the window closes: if this owns the clipboard and a clipboard manager runs, ask it to
    /// keep the clipboard (freedesktop's clipboard manager protocol), which it does by asking
    /// for the text; the thread answers it while this waits, a second at the most.
    pub fn save(&self) {
        let manager = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD_MANAGER)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.owner != NONE);
        let mut state = self.shared.state();
        if !manager || !state.clipboard {
            return;
        }
        state.saved = false;
        let a = self.atoms;
        let _ = self.conn.change_property32(
            PropMode::REPLACE,
            self.win,
            a.SAVE_TARGETS,
            AtomEnum::ATOM,
            &[a.UTF8_STRING],
        );
        let _ = self.conn.convert_selection(
            self.win,
            a.CLIPBOARD_MANAGER,
            a.SAVE_TARGETS,
            a.SAVE_TARGETS,
            CURRENT_TIME,
        );
        let _ = self.conn.flush();
        let _ = self
            .shared
            .saved
            .wait_timeout_while(state, SAVE_WAIT, |s| !s.saved);
    }

    fn wake(&self) {
        let _ = self.conn.change_property8(
            PropMode::APPEND,
            self.win,
            self.atoms.VOMD_SELECTION,
            AtomEnum::STRING,
            &[],
        );
        let _ = self.conn.flush();
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.state().stop = true;
        self.wake();
    }
}

/// A selection the thread owns: what is on offer, and since when (for `TIMESTAMP`, and to refuse
/// a request made before it).
struct Owned {
    content: Content,
    time: Timestamp,
}

/// The pieces of an INCR transfer that are still to come.
struct Transfer {
    chunks: Chunks,
    /// The type the pieces are given (what the program asked for).
    ty: Atom,
    latin1: bool,
    /// When the requestor last took a piece.
    since: Instant,
}

/// The INCR transfers in progress, by requestor window and property. A transfer goes on a piece
/// at a time: the requestor reads a property and deletes it, and the deletion is the cue for the
/// next piece; an empty one ends it.
#[derive(Default)]
struct Transfers {
    open: HashMap<(Window, Atom), Transfer>,
}

impl Transfers {
    fn start(&mut self, window: Window, property: Atom, transfer: Transfer) {
        self.open.insert((window, property), transfer);
    }

    fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// The property was deleted: the type and bytes of its next piece, empty for the last,
    /// which closes the transfer. `None` if no transfer waits on that property.
    fn next(&mut self, window: Window, property: Atom, now: Instant) -> Option<(Atom, Vec<u8>)> {
        let transfer = self.open.get_mut(&(window, property))?;
        transfer.since = now;
        let ty = transfer.ty;
        let Some(piece) = transfer.chunks.next() else {
            self.open.remove(&(window, property));
            return Some((ty, Vec::new()));
        };
        Some((
            ty,
            if transfer.latin1 {
                latin1(&piece)
            } else {
                piece.into_bytes()
            },
        ))
    }

    /// The requestor window is gone: its transfers end.
    fn closed(&mut self, window: Window) {
        self.open.retain(|&(w, _), _| w != window);
    }

    /// Give up on the transfers whose requestor stopped taking pieces. Returns how many.
    fn expire(&mut self, now: Instant) -> usize {
        let before = self.open.len();
        self.open
            .retain(|_, t| now.saturating_duration_since(t.since) < STALLED);
        before - self.open.len()
    }
}

/// The properties of our window that pastes are asked to answer into. A paste that is given up on
/// (it timed out, or stopped in the middle of an INCR transfer) may still be answered: the owner
/// writes into the property when it gets to it, and that must not be taken for a later paste's
/// text, nor start a transfer in it. So such a property is retired: left alone, and emptied when
/// the owner does write to it, until it has been quiet for [`QUIET`]. An atom is never freed by
/// the server, so there are a few names only, and a paste finds nothing if all of them are
/// retired.
struct Pastes {
    /// The names interned so far, and when each was retired (`None`: it is good to use).
    names: Vec<(Atom, Option<Instant>)>,
}

impl Pastes {
    fn new(first: Atom) -> Pastes {
        Pastes {
            names: vec![(first, None)],
        }
    }

    /// A name for a paste: one that is not retired, or quiet since long ago (which is good again).
    fn take(&mut self, now: Instant) -> Option<Atom> {
        let (atom, retired) = self.names.iter_mut().find(|(_, retired)| {
            retired.is_none_or(|at| now.saturating_duration_since(at) >= QUIET)
        })?;
        *retired = None;
        Some(*atom)
    }

    /// Whether a name can still be interned.
    fn room(&self) -> bool {
        self.names.len() < PASTE_NAMES
    }

    /// The name of the next one to intern, once there is room for it.
    fn next_name(&self) -> String {
        format!("VOMD_PASTE_{}", self.names.len())
    }

    fn add(&mut self, atom: Atom) {
        self.names.push((atom, None));
    }

    /// Give up on `atom`, or mark it written to again.
    fn retire(&mut self, atom: Atom, now: Instant) {
        if let Some((_, retired)) = self.names.iter_mut().find(|(a, _)| *a == atom) {
            *retired = Some(now);
        }
    }

    /// Whether `atom` is retired, not yet quiet.
    fn is_retired(&self, atom: Atom) -> bool {
        self.names
            .iter()
            .any(|&(a, retired)| a == atom && retired.is_some())
    }

    /// Whether `atom` is one of the names.
    fn has(&self, atom: Atom) -> bool {
        self.names.iter().any(|&(a, _)| a == atom)
    }
}

/// A paste being read from another program's selection.
struct Reading {
    /// Where the text goes, until it is sent.
    reply: Option<Reply>,
    selection: Atom,
    /// The server's time of the request, which the owner's answer repeats: a late answer to a
    /// paste that timed out is not taken for the next one's.
    time: Timestamp,
    /// The type last asked for, and how many have been (`UTF8_STRING`, then `STRING`).
    target: Atom,
    asked: usize,
    deadline: Instant,
    /// The owner is sending the text in pieces: each is a new value of our property.
    incr: bool,
    /// The empty piece that ends an INCR transfer has come.
    ended: bool,
    /// The text is sent (it hit the cap): the pieces still to come are taken and thrown away, so
    /// that the owner can end the transfer.
    draining: bool,
    data: Vec<u8>,
    /// The text is `STRING`, which is Latin-1.
    latin1: bool,
}

impl Reading {
    /// Whether a `SelectionNotify` for `selection` and `target`, stamped `time`, answers this.
    fn answered_by(&self, selection: Atom, target: Atom, time: Timestamp) -> bool {
        self.selection == selection && self.target == target && self.time == time
    }

    /// Send the text, if that has not been done.
    fn answer(&mut self, text: Option<String>) {
        if let Some(reply) = self.reply.take() {
            reply.send(text);
        }
    }

    /// Take a piece of the text, at most up to [`READ_CAP`] in all. True once the text is whole:
    /// the piece was the empty one that ends an INCR transfer, or the cap is reached.
    fn push(&mut self, piece: &[u8]) -> bool {
        let room = READ_CAP - self.data.len();
        self.data.extend_from_slice(&piece[..piece.len().min(room)]);
        piece.is_empty() || self.data.len() >= READ_CAP
    }
}

/// The thread that owns the selections and answers for them.
struct Server {
    conn: Arc<XCBConnection>,
    win: Window,
    atoms: Atoms,
    shared: Arc<Shared>,
    /// CLIPBOARD and PRIMARY, by [`index`].
    owned: [Option<Owned>; 2],
    transfers: Transfers,
    reading: Option<Reading>,
    /// The property the paste in hand is asked to be put in (the last one's, between pastes).
    paste: Atom,
    names: Pastes,
    /// A paste asked for while the last one's transfer is drained, which begins when it is over.
    queued: Option<(Which, Reply, Timestamp)>,
}

impl Server {
    /// Serve until the owner is gone or the connection breaks, and then see to it that no paste
    /// waits for an answer that can't come.
    fn run(mut self) {
        self.serve();
        let reads = {
            let mut state = self.shared.state();
            state.dead = true;
            std::mem::take(&mut state.reads)
        };
        // (Dropped unanswered, a paste says there was nothing.)
        drop(reads);
        drop((self.reading.take(), self.queued.take()));
    }

    fn serve(&mut self) {
        loop {
            // Blocked until a program asks (or a copy wakes it): no timer while no transfer is
            // open and no paste waits. With one open, it looks at the clock between events.
            let event = if self.transfers.is_empty() && self.reading.is_none() {
                self.conn.wait_for_event().ok()
            } else {
                match self.conn.poll_for_event() {
                    Ok(Some(event)) => Some(event),
                    Ok(None) => {
                        let now = Instant::now();
                        self.transfers.expire(now);
                        if self.reading.as_ref().is_some_and(|r| now >= r.deadline) {
                            self.finish(None);
                        }
                        std::thread::sleep(POLL);
                        continue;
                    }
                    Err(_) => None,
                }
            };
            // A broken connection ends the thread: the server has dropped the selections.
            let Some(event) = event else { return };
            if !self.handle(event) {
                return;
            }
            let _ = self.conn.flush();
        }
    }

    /// The selection atom of `index`.
    fn selection(&self, index: usize) -> Atom {
        if index == 0 {
            self.atoms.CLIPBOARD
        } else {
            AtomEnum::PRIMARY.into()
        }
    }

    fn index_of(&self, selection: Atom) -> Option<usize> {
        (0..2).find(|&i| self.selection(i) == selection)
    }

    /// Handle one event. False once the owner is gone.
    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::PropertyNotify(e)
                if e.window == self.win && e.atom == self.atoms.VOMD_SELECTION =>
            {
                return self.woken(e.time);
            }
            // The next piece of a paste that comes over INCR, or a late write to the property of
            // one that was given up on.
            Event::PropertyNotify(e)
                if e.window == self.win
                    && e.state == Property::NEW_VALUE
                    && self.names.has(e.atom) =>
            {
                self.written(e.atom);
            }
            Event::PropertyNotify(e) if e.state == Property::DELETE => {
                if let Some((ty, bytes)) = self.transfers.next(e.window, e.atom, Instant::now()) {
                    let _ =
                        self.conn
                            .change_property8(PropMode::REPLACE, e.window, e.atom, ty, &bytes);
                }
            }
            Event::SelectionClear(e) => {
                if let Some(i) = self.index_of(e.selection) {
                    self.owned[i] = None;
                    self.shared.state().clipboard = self.owned[0].is_some();
                }
            }
            // The clipboard manager's answer to a request to save the clipboard.
            Event::SelectionNotify(e) if e.selection == self.atoms.CLIPBOARD_MANAGER => {
                self.shared.state().saved = true;
                self.shared.saved.notify_all();
            }
            // The owner's answer to our request for a paste.
            Event::SelectionNotify(e)
                if e.requestor == self.win
                    && self
                        .reading
                        .as_ref()
                        .is_some_and(|r| r.answered_by(e.selection, e.target, e.time)) =>
            {
                self.read_notify(e.property);
            }
            Event::SelectionRequest(r) => self.request(&r),
            Event::DestroyNotify(e) => self.transfers.closed(e.window),
            _ => {}
        }
        true
    }

    /// Take the selections a copy left content for, with the server's time `time` (see
    /// [`Owner::set`]), then begin the pastes asked for. False if the owner is gone.
    fn woken(&mut self, time: Timestamp) -> bool {
        let (pending, reads) = {
            let mut state = self.shared.state();
            if state.stop {
                return false;
            }
            (
                [state.pending[0].take(), state.pending[1].take()],
                std::mem::take(&mut state.reads),
            )
        };
        for (i, content) in pending.into_iter().enumerate() {
            let Some(content) = content else { continue };
            let selection = self.selection(i);
            let _ = self.conn.set_selection_owner(self.win, selection, time);
            // Ours only if the server agrees: someone else may have a later claim.
            let owner = self
                .conn
                .get_selection_owner(selection)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| reply.owner);
            self.owned[i] = (owner == Some(self.win)).then_some(Owned { content, time });
        }
        self.shared.state().clipboard = self.owned[0].is_some();
        for (which, reply) in reads {
            self.begin_read(which, reply, time);
        }
        true
    }

    /// Begin reading the selection `which`: from what we offer if it is ours (the first piece of
    /// it, without a round trip), else by asking its owner, with the server's time `time`.
    fn begin_read(&mut self, which: Which, reply: Reply, time: Timestamp) {
        let i = index(which);
        if let Some(owned) = &self.owned[i] {
            reply.send(owned.content.chunks().next());
            return;
        }
        if self.reading.as_ref().is_some_and(|r| r.draining) {
            self.queued = Some((which, reply, time));
            return;
        }
        if self.reading.is_some() {
            reply.send(None);
            return;
        }
        let Some(paste) = self.lend() else {
            reply.send(None);
            return;
        };
        self.paste = paste;
        let mut reading = Reading {
            reply: Some(reply),
            selection: self.selection(i),
            time,
            target: NONE,
            asked: 0,
            deadline: Instant::now() + READ_WAIT,
            incr: false,
            ended: false,
            draining: false,
            data: Vec::new(),
            latin1: false,
        };
        if self.ask(&mut reading) {
            self.reading = Some(reading);
        } else {
            reading.answer(None);
        }
    }

    /// Ask the owner for the next type of text we know of. False if there is none left, or the
    /// request couldn't be made.
    fn ask(&self, r: &mut Reading) -> bool {
        let a = self.atoms;
        let Some(&target) = [a.UTF8_STRING, AtomEnum::STRING.into()].get(r.asked) else {
            return false;
        };
        (r.target, r.asked) = (target, r.asked + 1);
        let _ = self.conn.delete_property(self.win, self.paste);
        let asked = self
            .conn
            .convert_selection(self.win, r.selection, target, self.paste, r.time);
        asked.is_ok() && self.conn.flush().is_ok()
    }

    /// The paste is over: its text, or `None`. If it stopped before the owner was done (it never
    /// answered, or stalled in the middle of an INCR transfer, or did not end it in time after the
    /// cap), the owner may still write to the property, so that is retired, and the next paste
    /// gets another.
    fn finish(&mut self, text: Option<String>) {
        if let Some(mut r) = self.reading.take() {
            if !r.ended {
                self.names.retire(self.paste, Instant::now());
            }
            r.answer(text);
            if let Some((which, reply, time)) = self.queued.take() {
                self.begin_read(which, reply, time);
            }
        }
    }

    /// A property for the next paste: one that is good to use, else a new name if there is room
    /// for one, else none.
    fn lend(&mut self) -> Option<Atom> {
        if let Some(atom) = self.names.take(Instant::now()) {
            return Some(atom);
        }
        if !self.names.room() {
            return None;
        }
        let name = self.names.next_name();
        let atom = self
            .conn
            .intern_atom(false, name.as_bytes())
            .ok()
            .and_then(|cookie| cookie.reply().ok())?
            .atom;
        self.names.add(atom);
        Some(atom)
    }

    /// Something was written to the property `atom` of our window: the next piece of the paste
    /// in hand, if it is that one's, else a late write to a property that was given up on, which
    /// is deleted, so that the owner can go on and end its transfer.
    fn written(&mut self, atom: Atom) {
        if self.reading.is_some() && atom == self.paste {
            self.read_piece();
        } else if self.names.is_retired(atom) {
            let _ = self.conn.delete_property(self.win, atom);
            self.names.retire(atom, Instant::now());
        }
    }

    /// What is in our property, taken (up to the limit), which tells the owner that it may send
    /// more. The read deletes the property itself, and that is the one deletion: the owner writes
    /// its next piece as soon as it sees it, and a second deletion could take that piece away. Only
    /// the rest of a value longer than the limit, which a read leaves behind, is deleted here.
    fn take_property(&self) -> Option<(Atom, Vec<u8>)> {
        let words = u32::try_from(READ_CAP / 4 + 1).unwrap_or(u32::MAX);
        let reply = self
            .conn
            .get_property(true, self.win, self.paste, AtomEnum::ANY, 0, words)
            .ok()?
            .reply()
            .ok()?;
        if reply.bytes_after > 0 {
            let _ = self.conn.delete_property(self.win, self.paste);
        }
        Some((reply.type_, reply.value))
    }

    /// The owner answered the request for a paste: with the text in our property, or in pieces to
    /// come (INCR), or not at all, which tries the next type.
    fn read_notify(&mut self, property: Atom) {
        let Some(mut r) = self.reading.take() else {
            return;
        };
        if property == NONE {
            if self.ask(&mut r) {
                self.reading = Some(r);
            } else {
                r.answer(None);
            }
            return;
        }
        match self.take_property() {
            Some((ty, _)) if ty == self.atoms.INCR => {
                r.incr = true;
                self.reading = Some(r);
            }
            Some((ty, bytes)) => {
                r.latin1 = ty == Atom::from(AtomEnum::STRING);
                r.push(&bytes);
                r.answer(Some(decode(&r.data, r.latin1)));
            }
            None => r.answer(None),
        }
    }

    /// A piece of the paste came, over INCR: taken, and the text complete when it is the last
    /// or when the limit is reached. At the limit the text is sent, and the rest of the transfer is
    /// taken and dropped (for a moment, [`READ_WAIT`]) so that the owner may end it and serve the
    /// next paste.
    fn read_piece(&mut self) {
        if !self.reading.as_ref().is_some_and(|r| r.incr) {
            return;
        }
        let Some((ty, bytes)) = self.take_property() else {
            self.finish(None);
            return;
        };
        let Some(r) = self.reading.as_mut() else {
            return;
        };
        if r.draining {
            if bytes.is_empty() {
                r.ended = true;
                self.finish(None);
            }
            return;
        }
        r.latin1 = ty == Atom::from(AtomEnum::STRING);
        r.ended = bytes.is_empty();
        if r.push(&bytes) {
            let text = decode(&r.data, r.latin1);
            if r.ended {
                self.finish(Some(text));
            } else {
                r.answer(Some(text));
                (r.draining, r.deadline) = (true, Instant::now() + READ_WAIT);
            }
        }
    }

    /// Answer a program's request for a selection.
    fn request(&mut self, r: &SelectionRequestEvent) {
        // An obsolete requestor names no property: it means the target's.
        let property = if r.property == NONE {
            r.target
        } else {
            r.property
        };
        let served = self.index_of(r.selection).and_then(|i| {
            let owned = self.owned[i].as_ref()?;
            // A request made before we owned the selection is for someone else's content.
            (r.time == CURRENT_TIME || r.time >= owned.time)
                .then(|| (owned.content.clone(), owned.time))
        });
        let ok = match served {
            Some((content, time)) => {
                self.answer(&content, time, r.requestor, r.target, property, false)
            }
            None => false,
        };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: r.time,
            requestor: r.requestor,
            selection: r.selection,
            target: r.target,
            property: if ok { property } else { NONE },
        };
        let _ = self
            .conn
            .send_event(false, r.requestor, EventMask::NO_EVENT, notify);
    }

    /// Write the answer for `target` into `property` of `requestor`. False if it can't be
    /// answered. `nested`: this is one of the requests of a MULTIPLE.
    fn answer(
        &mut self,
        content: &Content,
        time: Timestamp,
        requestor: Window,
        target: Atom,
        property: Atom,
        nested: bool,
    ) -> bool {
        let a = self.atoms;
        if target == a.TARGETS {
            let list = targets(&a);
            let _ = self.conn.change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                AtomEnum::ATOM,
                &list,
            );
            true
        } else if target == a.TIMESTAMP {
            let _ = self.conn.change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                AtomEnum::INTEGER,
                &[time],
            );
            true
        } else if target == a.MULTIPLE && !nested {
            self.multiple(content, time, requestor, property)
        } else if [
            a.UTF8_STRING,
            a.TEXT_PLAIN_UTF8,
            a.TEXT,
            AtomEnum::STRING.into(),
            a.TEXT_PLAIN,
        ]
        .contains(&target)
        {
            self.text(content, target, requestor, property)
        } else {
            false
        }
    }

    /// MULTIPLE: the property holds (target, property) pairs, each answered like a request of
    /// its own; the ones that can't be get `None` for their property.
    fn multiple(
        &mut self,
        content: &Content,
        time: Timestamp,
        requestor: Window,
        property: Atom,
    ) -> bool {
        if property == NONE {
            return false;
        }
        let pairs = self
            .conn
            .get_property(
                false,
                requestor,
                property,
                self.atoms.ATOM_PAIR,
                0,
                u32::MAX / 4,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| Some(reply.value32()?.collect::<Vec<Atom>>()));
        let Some(mut pairs) = pairs else { return false };
        for pair in pairs.as_chunks_mut::<2>().0 {
            if !self.answer(content, time, requestor, pair[0], pair[1], true) {
                pair[1] = NONE;
            }
        }
        let _ = self.conn.change_property32(
            PropMode::REPLACE,
            requestor,
            property,
            self.atoms.ATOM_PAIR,
            &pairs,
        );
        true
    }

    /// Give the text as `target`: in the property at once if it is short, else start an INCR
    /// transfer of it. `TEXT` is answered as `UTF8_STRING` (as GTK does), `STRING` as Latin-1.
    fn text(&mut self, content: &Content, target: Atom, requestor: Window, property: Atom) -> bool {
        let a = self.atoms;
        let ty = if target == a.TEXT {
            a.UTF8_STRING
        } else {
            target
        };
        let as_latin1 = target == Atom::from(AtomEnum::STRING);
        if let Content::Text(text) = content
            && text.len() <= CHUNK
        {
            let bytes = if as_latin1 {
                latin1(text)
            } else {
                text.as_bytes().to_vec()
            };
            let _ = self
                .conn
                .change_property8(PropMode::REPLACE, requestor, property, ty, &bytes);
            return true;
        }
        // The requestor's deletions of the property (and its window going) are the cues.
        let mask = EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY;
        let _ = self.conn.change_window_attributes(
            requestor,
            &ChangeWindowAttributesAux::new().event_mask(mask),
        );
        // The property says how much is coming, at least.
        let size = u32::try_from(content.size_hint()).unwrap_or(u32::MAX);
        let _ =
            self.conn
                .change_property32(PropMode::REPLACE, requestor, property, a.INCR, &[size]);
        self.transfers.start(
            requestor,
            property,
            Transfer {
                chunks: content.chunks(),
                ty,
                latin1: as_latin1,
                since: Instant::now(),
            },
        );
        true
    }
}

/// The targets a selection answers `TARGETS` with.
fn targets(a: &Atoms) -> [Atom; 8] {
    [
        a.TARGETS,
        a.TIMESTAMP,
        a.MULTIPLE,
        a.SAVE_TARGETS,
        a.UTF8_STRING,
        a.TEXT_PLAIN_UTF8,
        AtomEnum::STRING.into(),
        a.TEXT,
    ]
}

fn why(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests;
