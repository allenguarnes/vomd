//! The Wayland clipboard: the selection (`wl_data_device`) and the primary selection
//! (`zwp_primary_selection_device_v1`), and the pipes the programs that paste them read from.
//!
//! It uses winit's connection: libwayland routes the events of an object to the queue of the
//! proxy that made it, so a queue of our own on the same `wl_display` sees only what we asked
//! for, and winit's sees none of it. Setting a selection takes the serial of an input event, and
//! winit doesn't pass those on, so we bind a keyboard and a pointer of our own: the compositor
//! sends them the same events, with the same serials, as the window's. A thread blocked on our
//! queue (dispatching it from winit's loop isn't an option: that loop runs no iteration when only
//! another queue got events) answers the compositor, and each paste gets a thread that writes
//! the text into its pipe, so nothing here blocks the window. The offers of the current selections
//! are kept, to read a paste: the thread asks the offer for text through a pipe of its own, and a
//! worker reads the pipe, up to a limit and a time.

use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use wayland_backend::client::Backend;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 as PrimaryManager,
    zwp_primary_selection_device_v1::{
        self as primary_device, ZwpPrimarySelectionDeviceV1 as PrimaryDevice,
    },
    zwp_primary_selection_offer_v1::{
        self as primary_offer, ZwpPrimarySelectionOfferV1 as PrimaryOffer,
    },
    zwp_primary_selection_source_v1::{
        self as primary_source, ZwpPrimarySelectionSourceV1 as PrimarySource,
    },
};

use super::{READ_CAP, READ_WAIT, Reply, TEXT_TYPES, Which, decode, latin1};
use crate::copy::Content;

/// Pastes that are being written at once. A paste past this gets an empty answer: pastes are a
/// person's doing, and a thread for each of an endless run of them is worse.
const WRITERS: usize = 4;

/// The handle the window keeps: copies go through it, to the thread.
pub struct Owner {
    conn: Connection,
    display: WlDisplay,
    qh: QueueHandle<State>,
    copies: Sender<Request>,
}

/// What the main thread asks the thread to do once the compositor has answered a `sync`.
enum Request {
    /// Offer content as a selection.
    Offer(Which, Content),
    /// Read a selection for a paste.
    Read(Which, Reply),
}

impl Owner {
    /// Join the connection of the window (`display` is its `wl_display`, which outlives this),
    /// bind what a selection needs, and start the thread.
    pub fn start(display: *mut std::ffi::c_void) -> Result<Owner, String> {
        // SAFETY: winit's display is open for as long as its window, and this goes with the window.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let seat: WlSeat = globals
            .bind(&qh, 1..=7, ())
            .map_err(|_| "the compositor has no seat".to_owned())?;
        let manager: WlDataDeviceManager = globals
            .bind(&qh, 1..=3, ())
            .map_err(|_| "the compositor has no wl_data_device_manager".to_owned())?;
        // A compositor without the primary selection protocol just has no primary selection.
        let primary_manager: Option<PrimaryManager> = globals.bind(&qh, 1..=1, ()).ok();
        let (copies, requests) = channel();
        let mut state = State {
            qh: qh.clone(),
            device: manager.get_data_device(&seat, &qh, ()),
            primary_device: primary_manager
                .as_ref()
                .map(|m| m.get_device(&seat, &qh, ())),
            manager,
            primary_manager,
            keyboard: None,
            pointer: None,
            serial: 0,
            offers: Vec::new(),
            primary_offers: Vec::new(),
            selected: None,
            primary_selected: None,
            slots: [Slot::default(), Slot::default()],
            requests,
            writers: Arc::new(AtomicUsize::new(0)),
        };
        // The seat's capabilities say which of keyboard and pointer there are.
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("vomd-clipboard".into())
            .spawn(move || {
                // Blocked until the compositor says something. An error ends the thread, and
                // with it the offers: nobody answers for them any more.
                while queue.blocking_dispatch(&mut state).is_ok() {}
                if crate::platform::stderr_is_tty() {
                    eprintln!("vomd: the clipboard's connection ended");
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Owner {
            display: conn.display(),
            conn,
            qh,
            copies,
        })
    }

    /// Offer `content` as the selection `which`. The thread takes it once the compositor answers
    /// a `sync`: by then it has dispatched every event sent before it, among them the key press
    /// or button release that caused this copy, so the serial it has is that of the copy.
    pub fn set(&self, which: Which, content: Content) {
        self.request(Request::Offer(which, content));
    }

    /// Read the selection `which` for a paste, once the compositor has told the thread of the
    /// selections as they are now. `reply` gets the text, or nothing if the thread has ended.
    pub fn read(&self, which: Which, reply: Reply) {
        self.request(Request::Read(which, reply));
    }

    fn request(&self, request: Request) {
        // (A thread that has ended drops the request, and a dropped paste says there was nothing.)
        let _ = self.copies.send(request);
        self.display.sync(&self.qh, ());
        let _ = self.conn.flush();
    }
}

/// A selection: the source that is offered, and what it gives.
#[derive(Default)]
struct Slot {
    source: Option<Source>,
    content: Option<Content>,
}

enum Source {
    Data(WlDataSource),
    Primary(PrimarySource),
}

impl Source {
    fn destroy(&self) {
        match self {
            Source::Data(s) => s.destroy(),
            Source::Primary(s) => s.destroy(),
        }
    }
}

struct State {
    qh: QueueHandle<State>,
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    primary_manager: Option<PrimaryManager>,
    primary_device: Option<PrimaryDevice>,
    /// Ours, only to be sent the input events that carry serials (see the module).
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    /// The serial of the last input event.
    serial: u32,
    /// Offers of other programs' selections (and drags) that haven't been used up: each is
    /// let go of once the event that names it has come (see the `Dispatch` for the device).
    offers: Vec<WlDataOffer>,
    primary_offers: Vec<PrimaryOffer>,
    /// The offers of the current selections, which a paste reads.
    selected: Option<WlDataOffer>,
    primary_selected: Option<PrimaryOffer>,
    /// CLIPBOARD and PRIMARY, by [`super::Which`] order.
    slots: [Slot; 2],
    requests: Receiver<Request>,
    writers: Arc<AtomicUsize>,
}

impl State {
    /// Do what the main thread asked: take the selections it left content for, and begin the
    /// pastes it wants read.
    fn take(&mut self) {
        while let Ok(request) = self.requests.try_recv() {
            let (which, content) = match request {
                Request::Offer(which, content) => (which, content),
                Request::Read(which, reply) => {
                    self.read(which, reply);
                    continue;
                }
            };
            match which {
                Which::Clipboard => {
                    let source = self.manager.create_data_source(&self.qh, ());
                    for t in TEXT_TYPES {
                        source.offer(t.into());
                    }
                    self.device.set_selection(Some(&source), self.serial);
                    self.slots[0].replace(Source::Data(source), content);
                }
                Which::Primary => {
                    let (Some(manager), Some(device)) =
                        (&self.primary_manager, &self.primary_device)
                    else {
                        continue;
                    };
                    let source = manager.create_source(&self.qh, ());
                    for t in TEXT_TYPES {
                        source.offer(t.into());
                    }
                    device.set_selection(Some(&source), self.serial);
                    self.slots[1].replace(Source::Primary(source), content);
                }
            }
        }
    }

    /// Begin reading the selection `which`: from what we offer if it is ours, else from the offer
    /// of the program that owns it, through a pipe a worker reads.
    fn read(&self, which: Which, reply: Reply) {
        let i = match which {
            Which::Clipboard => 0,
            Which::Primary => 1,
        };
        if let Some(content) = &self.slots[i].content {
            reply.send(content.chunks().next());
            return;
        }
        let (mimes, offer): (Vec<String>, &dyn Receive) = match which {
            Which::Clipboard => match &self.selected {
                Some(o) => (mimes_of(o.data::<Mimes>()), o),
                None => return reply.send(None),
            },
            Which::Primary => match &self.primary_selected {
                Some(o) => (mimes_of(o.data::<Mimes>()), o),
                None => return reply.send(None),
            },
        };
        let Some(mime) = TEXT_TYPES
            .into_iter()
            .find(|t| mimes.iter().any(|m| m == t))
        else {
            return reply.send(None);
        };
        let Ok((pipe, writer)) = io::pipe() else {
            return reply.send(None);
        };
        offer.receive(mime, &writer);
        // Our end of the writing must go, or the pipe never ends for the reader.
        drop(writer);
        let as_latin1 = mime == "STRING";
        // If the worker can't start, the closure goes with the reply, which says there was nothing.
        let _ = std::thread::Builder::new()
            .name("vomd-read".into())
            .spawn(move || {
                let bytes = read_within(pipe, READ_CAP, READ_WAIT);
                reply.send(bytes.map(|b| decode(&b, as_latin1)));
            });
    }

    /// A program asked for the text of slot `i` as `mime`, into `fd`.
    fn send(&self, i: usize, mime: &str, fd: OwnedFd) {
        let (Some(content), Some(writer)) =
            (self.slots[i].content.clone(), Writer::start(&self.writers))
        else {
            // (Dropping the pipe is the empty answer.)
            return;
        };
        let as_latin1 = mime == "STRING";
        // If the thread can't start, the closure goes with the writer count it holds.
        let _ = std::thread::Builder::new()
            .name("vomd-paste".into())
            .spawn(move || {
                write_to(fd, &content, as_latin1);
                drop(writer);
            });
    }
}

/// One of the pastes being written, at most [`WRITERS`] at a time.
struct Writer(Arc<AtomicUsize>);

impl Writer {
    fn start(count: &Arc<AtomicUsize>) -> Option<Writer> {
        if count.fetch_add(1, Ordering::AcqRel) >= WRITERS {
            count.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Writer(count.clone()))
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Slot {
    /// Offer `source` with `content`, in place of what was.
    fn replace(&mut self, source: Source, content: Content) {
        if let Some(old) = self.source.replace(source) {
            old.destroy();
        }
        self.content = Some(content);
    }
}

/// A program that takes nothing from its pipe in this long (it holds the pipe open and doesn't
/// read) is given up on, as the X11 half gives up on a requestor, so it can't keep one of the
/// [`WRITERS`] and the text it was to get for ever.
const STALLED: Duration = Duration::from_secs(5);

/// Write the text into the pipe, a piece at a time, as it is made; the pipe holds the thread
/// back when the program reads slowly, an end of it that closes (the program gave up) ends the
/// writing, and so does a program that stops reading for [`STALLED`].
fn write_to(fd: OwnedFd, content: &Content, as_latin1: bool) {
    write_within(fd, content, as_latin1, STALLED);
}

fn write_within(fd: OwnedFd, content: &Content, as_latin1: bool, stalled: Duration) {
    let mut pipe = std::fs::File::from(fd);
    if set_nonblocking(&pipe).is_err() {
        return;
    }
    for piece in content.chunks() {
        let result = if as_latin1 {
            write_all(&mut pipe, &latin1(&piece), stalled)
        } else {
            write_all(&mut pipe, piece.as_bytes(), stalled)
        };
        if result.is_err() {
            return;
        }
    }
}

/// `write_all` to a non-blocking pipe that fails when `stalled` passes without a byte taken.
fn write_all(pipe: &mut std::fs::File, mut bytes: &[u8], stalled: Duration) -> io::Result<()> {
    let mut give_up = Instant::now() + stalled;
    while !bytes.is_empty() {
        match pipe.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                bytes = &bytes[n..];
                give_up = Instant::now() + stalled;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let left = give_up.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                wait_writable(pipe, left);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const O_NONBLOCK: i32 = 4;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const POLLOUT: i16 = 4;
#[cfg(any(target_os = "linux", target_os = "android"))]
type NFds = std::ffi::c_ulong;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
type NFds = std::ffi::c_uint;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn poll(fds: *mut PollFd, n: NFds, timeout_ms: i32) -> i32;
}

/// Make writes to the pipe fail with `WouldBlock` instead of waiting.
fn set_nonblocking(pipe: &std::fs::File) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: a valid descriptor, and the flags commands take an int.
    unsafe {
        let flags = fcntl(fd, F_GETFL);
        if flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Wait up to `limit` for the pipe to have room (or to have lost its reader, which the next
/// write reports); an error or an interrupt just ends the wait early.
fn wait_writable(pipe: &std::fs::File, limit: Duration) {
    let mut fd = PollFd {
        fd: pipe.as_raw_fd(),
        events: POLLOUT,
        revents: 0,
    };
    let ms = limit.as_millis().clamp(1, 1000) as i32;
    // SAFETY: one valid `pollfd`, as the count says.
    unsafe { poll(&mut fd, 1, ms) };
}

/// The types an offer says it has, as they come.
#[derive(Default)]
struct Mimes(Mutex<Vec<String>>);

fn mimes_of(mimes: Option<&Mimes>) -> Vec<String> {
    mimes.map_or_else(Vec::new, |m| {
        m.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    })
}

/// An offer of either kind, which can be asked for text.
trait Receive {
    fn receive(&self, mime: &str, fd: &dyn AsFd);
}

impl Receive for WlDataOffer {
    fn receive(&self, mime: &str, fd: &dyn AsFd) {
        WlDataOffer::receive(self, mime.into(), fd.as_fd());
    }
}

impl Receive for PrimaryOffer {
    fn receive(&self, mime: &str, fd: &dyn AsFd) {
        PrimaryOffer::receive(self, mime.into(), fd.as_fd());
    }
}

const POLLIN: i16 = 1;

/// Read the pipe up to `cap` bytes or its end: `None` if it doesn't end or fill within `limit`
/// (a program that holds the pipe open and writes nothing, or too slowly, is given up on).
fn read_within(mut pipe: io::PipeReader, cap: usize, limit: Duration) -> Option<Vec<u8>> {
    let end = Instant::now() + limit;
    let mut bytes = Vec::new();
    let mut piece = [0u8; 8192];
    while bytes.len() < cap {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        let mut fd = PollFd {
            fd: pipe.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        };
        let ms = left.as_millis().clamp(1, 1000) as i32;
        // SAFETY: one valid `pollfd`, as the count says.
        if unsafe { poll(&mut fd, 1, ms) } <= 0 {
            // An interrupt or the wait running out: look at the clock again.
            continue;
        }
        let room = piece.len().min(cap - bytes.len());
        match pipe.read(&mut piece[..room]) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&piece[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(bytes)
}

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        _: <WlCallback as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.take();
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        // A device that goes (a keyboard unplugged, or a virtual one that closes) takes its events
        // with it: when one comes back it is a new keyboard, whose events the old object
        // doesn't get, so the serials would go stale.
        if !caps.contains(wl_seat::Capability::Keyboard) {
            if let Some(old) = state.keyboard.take().filter(|k| k.version() >= 3) {
                old.release();
            }
        } else if state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        }
        if !caps.contains(wl_seat::Capability::Pointer) {
            if let Some(old) = state.pointer.take().filter(|p| p.version() >= 3) {
                old.release();
            }
        } else if state.pointer.is_none() {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // (The keymap's file is closed as the event is dropped.)
        if let wl_keyboard::Event::Enter { serial, .. } | wl_keyboard::Event::Key { serial, .. } =
            event
        {
            state.serial = serial;
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Enter { serial, .. } | wl_pointer::Event::Button { serial, .. } =
            event
        {
            state.serial = serial;
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The offers of other programs are of no use to a viewer, but an offer is named by the
        // event after it (the selection, a drag entering), so it is let go of when that has come:
        // a destroyed object can't be named.
        match event {
            wl_data_device::Event::DataOffer { id } => state.offers.push(id),
            // The offer the selection event names is the current selection, which a paste reads,
            // until the next one: the others are let go of.
            wl_data_device::Event::Selection { id } => {
                state.offers.retain(|o| {
                    let keep = id.as_ref() == Some(o);
                    if !keep {
                        o.destroy();
                    }
                    keep
                });
                if let Some(old) = std::mem::replace(&mut state.selected, state.offers.pop()) {
                    old.destroy();
                }
            }
            wl_data_device::Event::Leave | wl_data_device::Event::Drop => {
                state.offers.drain(..).for_each(|o| o.destroy());
            }
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Mimes::default()),
    ]);
}

impl Dispatch<WlDataOffer, Mimes> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        mimes: &Mimes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            mimes
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => state.send(0, &mime_type, fd),
            wl_data_source::Event::Cancelled => {
                source.destroy();
                state.slots[0].lose(|s| matches!(s, Source::Data(d) if d == source));
            }
            _ => {}
        }
    }
}

impl Dispatch<PrimaryManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &PrimaryManager,
        _: <PrimaryManager as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<PrimaryDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &PrimaryDevice,
        event: primary_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            primary_device::Event::DataOffer { offer } => state.primary_offers.push(offer),
            primary_device::Event::Selection { id } => {
                state.primary_offers.retain(|o| {
                    let keep = id.as_ref() == Some(o);
                    if !keep {
                        o.destroy();
                    }
                    keep
                });
                let old =
                    std::mem::replace(&mut state.primary_selected, state.primary_offers.pop());
                if let Some(old) = old {
                    old.destroy();
                }
            }
            _ => {}
        }
    }

    event_created_child!(State, PrimaryDevice, [
        primary_device::EVT_DATA_OFFER_OPCODE => (PrimaryOffer, Mimes::default()),
    ]);
}

impl Dispatch<PrimaryOffer, Mimes> for State {
    fn event(
        _: &mut Self,
        _: &PrimaryOffer,
        event: primary_offer::Event,
        mimes: &Mimes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let primary_offer::Event::Offer { mime_type } = event {
            mimes
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(mime_type);
        }
    }
}

impl Dispatch<PrimarySource, ()> for State {
    fn event(
        state: &mut Self,
        source: &PrimarySource,
        event: primary_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            primary_source::Event::Send { mime_type, fd } => state.send(1, &mime_type, fd),
            primary_source::Event::Cancelled => {
                source.destroy();
                state.slots[1].lose(|s| matches!(s, Source::Primary(p) if p == source));
            }
            _ => {}
        }
    }
}

impl Slot {
    /// Another program took the selection: what was on offer is dropped, unless the source that
    /// lost it isn't the current one (it was replaced meanwhile).
    fn lose(&mut self, is: impl Fn(&Source) -> bool) {
        if self.source.as_ref().is_some_and(is) {
            self.source = None;
            self.content = None;
        }
    }
}

#[cfg(test)]
mod tests;
