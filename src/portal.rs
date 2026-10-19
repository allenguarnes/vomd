//! The XDG desktop portal over the session D-Bus, through a minimal client (no D-Bus crate:
//! one connection, EXTERNAL authentication).
//!
//! The desktop's light/dark preference: the `org.freedesktop.appearance` `color-scheme`
//! setting. winit reports no theme on Wayland, and KDE sets no `GTK_THEME`, so this is how a
//! Plasma or GNOME dark mode is seen: `ReadOne` (`Read` on portals before version 2), then the
//! `SettingChanged` signal for changes while the window is open.
//!
//! The file chooser for Open is in [`chooser`].

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

mod chooser;
pub use chooser::{Choice, choose_file};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
const BUS: &str = "org.freedesktop.DBus";

const RETURN: u8 = 2;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;

/// The most a message from the bus may hold (header fields or body); anything larger ends
/// the connection instead of being read into memory.
const MAX_PART: u32 = 1 << 20;

/// Watch the preference: `f` gets the current value, then each change, until the bus goes
/// away, which is reported as `None` too. `None` is no preference, or no bus or portal to ask
/// (use the other sources).
pub fn watch(mut f: impl FnMut(Option<bool>)) {
    match Bus::connect() {
        Some(bus) => run(bus, f),
        None => f(None),
    }
}

fn run(mut bus: Bus, mut f: impl FnMut(Option<bool>)) {
    // Subscribe before reading, so no change falls between the two. The portal's signals reach
    // us in the order it sent them, so the last one is the latest change, and one that arrives
    // after a reply is newer than what the reply was read from and is simply applied. A signal
    // that arrives before the reply is the one case that can't be ordered: the portal's frontend
    // answers a read on a worker thread, so the reply may be older than a signal it overtook, or
    // newer than it. `follow` settles that by reading again, after a delay that grows while
    // reads keep being disturbed.
    let watching = bus.add_match();
    let value = bus.read_setting();
    // The first value, reported at once: the window waits for it.
    f(value);
    if watching {
        bus.follow(value, &mut f);
        // The bus went away: a value it last gave no longer holds, use the other sources.
        f(None);
    }
}

/// The wait before the first re-read of a read that a signal raced, doubled each time the next
/// is raced again, up to the limit.
const REREAD: Duration = Duration::from_millis(50);
const REREAD_MAX: Duration = Duration::from_secs(5);

/// The signals wanted: `color-scheme` changes, from the portal only (the bus matches the name
/// against its owner, so no other client can pass one off as the portal's broadcast). The bus
/// delivers a signal addressed to this connection whatever the rule says; `setting_changed`
/// refuses those, the portal broadcasting.
const MATCH_RULE: &str = concat!(
    "type='signal',sender='org.freedesktop.portal.Desktop',",
    "interface='org.freedesktop.portal.Settings',member='SettingChanged',",
    "path='/org/freedesktop/portal/desktop',",
    "arg0='org.freedesktop.appearance',arg1='color-scheme'"
);

/// `color-scheme`: 1 prefers dark, 2 prefers light, 0 (or anything else) no preference.
fn scheme(value: u32) -> Option<bool> {
    match value {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

struct Bus {
    stream: UnixStream,
    serial: u32,
    /// A write or read failed, perhaps midway through a message: the stream is not used again.
    dead: bool,
    /// The last `SettingChanged` that arrived while the latest `read_setting` waited for its
    /// reply: it may be older or newer than the value read.
    raced: Option<Option<bool>>,
    /// This connection's unique name (`:1.42`), from `Hello`.
    unique: Option<String>,
}

/// How long the login and `Hello` may take. Later calls and the signals wait without a limit:
/// the portal may take seconds to start at login, and the thread has nothing else to do.
const HANDSHAKE: Duration = Duration::from_secs(5);

impl Bus {
    /// The session bus from `DBUS_SESSION_BUS_ADDRESS` (or `$XDG_RUNTIME_DIR/bus`),
    /// authenticated and greeted.
    fn connect() -> Option<Bus> {
        let stream = match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
            Ok(addr) => addr.split(';').find_map(open)?,
            Err(_) => {
                let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
                UnixStream::connect(std::path::Path::new(&dir).join("bus")).ok()?
            }
        };
        Bus::over(stream)
    }

    fn over(mut stream: UnixStream) -> Option<Bus> {
        // A bus that never answers mustn't hold the thread for good.
        stream.set_read_timeout(Some(HANDSHAKE)).ok()?;
        // SAFETY: getuid takes nothing and can't fail.
        let uid = unsafe { getuid() };
        let mut auth = b"\0AUTH EXTERNAL ".to_vec();
        for digit in uid.to_string().bytes() {
            // The uid in decimal, each digit as two hex digits ('0' is "30").
            auth.extend_from_slice(&[b'3', digit]);
        }
        auth.extend_from_slice(b"\r\n");
        stream.write_all(&auth).ok()?;
        if !read_line(&mut stream)?.starts_with("OK ") {
            return None;
        }
        stream.write_all(b"BEGIN\r\n").ok()?;
        let mut bus = Bus {
            stream,
            serial: 0,
            dead: false,
            raced: None,
            unique: None,
        };
        let hello = bus.call(BUS, "/org/freedesktop/DBus", BUS, "Hello", "", &[])?;
        if hello.kind != RETURN {
            return None;
        }
        bus.unique = Reader::new(&hello.body).str().map(str::to_owned);
        // The portal may be started by the first call to it, which can take a while.
        bus.stream.set_read_timeout(None).ok()?;
        Some(bus)
    }

    /// The current `color-scheme`; `None` too when it couldn't be read.
    fn read_setting(&mut self) -> Option<bool> {
        self.read().flatten()
    }

    /// The current `color-scheme`, or `None` if it couldn't be read. Notes in `raced` the last
    /// change signalled while waiting for the reply.
    fn read(&mut self) -> Option<Option<bool>> {
        self.raced = None;
        let args = [NAMESPACE, KEY];
        let mut reply = self.call(PORTAL, PORTAL_PATH, SETTINGS, "ReadOne", "ss", &args)?;
        if reply.kind == ERROR {
            reply = self.call(PORTAL, PORTAL_PATH, SETTINGS, "Read", "ss", &args)?;
        }
        if reply.kind != RETURN {
            return None;
        }
        Some(Reader::new(&reply.body).variant_u32(0).and_then(scheme))
    }

    /// Whether a signal raced the last read and said something else than its `value`: either
    /// could be the newer.
    fn disturbed(&self, value: Option<bool>) -> bool {
        self.raced.is_some_and(|signalled| signalled != value)
    }

    /// Ask for `SettingChanged` on `color-scheme`; whether the bus agreed.
    fn add_match(&mut self) -> bool {
        let added = self.call(
            BUS,
            "/org/freedesktop/DBus",
            BUS,
            "AddMatch",
            "s",
            &[MATCH_RULE],
        );
        added.is_some_and(|m| m.kind == RETURN)
    }

    /// Hand each change from `last` on to `f`, until the connection ends (or has failed).
    /// Signals are applied in order. If one raced the read that gave `last`, read again after
    /// `REREAD`, and if one races that read too, after twice as long, and so on up to
    /// `REREAD_MAX`: at most one read per period, and none once a read goes undisturbed. Then
    /// it blocks without waking.
    fn follow(&mut self, mut last: Option<bool>, mut f: impl FnMut(Option<bool>)) {
        let mut delay = REREAD;
        let mut due = self.disturbed(last).then(|| Instant::now() + delay);
        while !self.dead {
            if let Some(at) = due
                && !self.ready(at)
            {
                due = None;
                // A failed read leaves `last` as it was and ends the chain.
                if let Some(value) = self.read() {
                    if value != last {
                        last = value;
                        f(value);
                    }
                    if self.disturbed(value) {
                        delay = (delay * 2).min(REREAD_MAX);
                        due = Some(Instant::now() + delay);
                    } else {
                        delay = REREAD;
                    }
                }
                continue;
            }
            match self.recv() {
                Ok(msg) => {
                    if let Some(value) = setting_changed(&msg)
                        && value != last
                    {
                        last = value;
                        f(value);
                    }
                }
                Err(_) => self.dead = true,
            }
        }
    }

    /// Wait until a message has started to arrive (or the connection has ended or failed) or
    /// `at` has passed; whether a message is there to read. Peeks with a timeout instead of
    /// reading with one, which could expire inside a message and leave the stream midway.
    fn ready(&mut self, at: Instant) -> bool {
        loop {
            let left = at.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if self.stream.set_read_timeout(Some(left)).is_err() {
                self.dead = true;
                return true;
            }
            let peeked = peek(&self.stream);
            if self.stream.set_read_timeout(None).is_err() {
                self.dead = true;
                return true;
            }
            match peeked {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return false;
                }
                // Data, the end of the stream or a failure: `recv` takes it from here.
                _ => return true,
            }
        }
    }

    /// Call a method and wait for its reply (or error). Anything else that arrives first is
    /// dropped; the last `SettingChanged` among it is kept in `raced`, for `follow` to settle.
    /// `None` once the connection has failed, and from then on.
    fn call(
        &mut self,
        dest: &str,
        path: &str,
        iface: &str,
        member: &str,
        sig: &str,
        args: &[&str],
    ) -> Option<Msg> {
        let mut body = Writer::default();
        for a in args {
            body.str(a);
        }
        let serial = self.send(dest, path, iface, member, sig, &body.0)?;
        while !self.dead {
            match self.recv() {
                Ok(reply)
                    if matches!(reply.kind, RETURN | ERROR) && reply.reply_serial == serial =>
                {
                    return Some(reply);
                }
                Ok(other) => {
                    if let Some(value) = setting_changed(&other) {
                        self.raced = Some(value);
                    }
                }
                Err(_) => self.dead = true,
            }
        }
        None
    }

    /// Send a method call with `body` (of signature `sig`); its serial, `None` once the
    /// connection has failed.
    fn send(
        &mut self,
        dest: &str,
        path: &str,
        iface: &str,
        member: &str,
        sig: &str,
        body: &[u8],
    ) -> Option<u32> {
        if self.dead {
            return None;
        }
        self.serial += 1;
        let msg = method_call(self.serial, dest, path, iface, member, sig, body);
        self.dead = self.stream.write_all(&msg).is_err();
        (!self.dead).then_some(self.serial)
    }

    fn recv(&mut self) -> io::Result<Msg> {
        let bad = || io::Error::from(io::ErrorKind::InvalidData);
        let mut fixed = [0u8; 16];
        self.stream.read_exact(&mut fixed)?;
        // Messages come in the sender's byte order: a big-endian one (a big-endian host's bus)
        // ends the connection, which is no preference.
        if fixed[0] != b'l' {
            return Err(bad());
        }
        let word =
            |i: usize| u32::from_le_bytes([fixed[i], fixed[i + 1], fixed[i + 2], fixed[i + 3]]);
        let (body_len, fields_len) = (word(4), word(12));
        if body_len > MAX_PART || fields_len > MAX_PART {
            return Err(bad());
        }
        // The header fields, padded to 8 (they start at 16, so offsets keep their alignment).
        let mut fields = vec![0; (fields_len as usize).next_multiple_of(8)];
        self.stream.read_exact(&mut fields)?;
        let mut body = vec![0; body_len as usize];
        self.stream.read_exact(&mut body)?;
        let mut msg = Msg {
            kind: fixed[1],
            reply_serial: 0,
            member: String::new(),
            path: String::new(),
            sender: String::new(),
            directed: false,
            body,
        };
        let mut r = Reader::new(&fields[..fields_len as usize]);
        while r.pos < fields_len as usize {
            r.align(8);
            let code = r.byte().ok_or_else(bad)?;
            match (code, r.sig().ok_or_else(bad)?) {
                (1, "o") => msg.path = r.str().ok_or_else(bad)?.to_owned(),
                (3, "s") => msg.member = r.str().ok_or_else(bad)?.to_owned(),
                (7, "s") => msg.sender = r.str().ok_or_else(bad)?.to_owned(),
                (6, "s") => {
                    msg.directed = true;
                    _ = r.str().ok_or_else(bad)?;
                }
                (5, "u") => msg.reply_serial = r.u32().ok_or_else(bad)?,
                (_, "s" | "o") => _ = r.str().ok_or_else(bad)?,
                (_, "g") => _ = r.sig().ok_or_else(bad)?,
                (_, "u") => _ = r.u32().ok_or_else(bad)?,
                _ => return Err(bad()),
            }
        }
        Ok(msg)
    }
}

unsafe extern "C" {
    fn getuid() -> u32;
    fn recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> isize;
}

/// `UnixStream::peek` is not stable: wait for a byte (up to the stream's read timeout) without
/// taking it.
fn peek(stream: &UnixStream) -> io::Result<usize> {
    use std::os::fd::AsRawFd;
    const MSG_PEEK: i32 = 2;
    let mut byte = 0u8;
    // SAFETY: a valid descriptor, and a buffer of the one byte promised.
    let n = unsafe { recv(stream.as_raw_fd(), &mut byte, 1, MSG_PEEK) };
    usize::try_from(n).map_err(|_| io::Error::last_os_error())
}

/// One `unix:` entry of a bus address: `path=` or (Linux) `abstract=`, %-escaped.
fn open(entry: &str) -> Option<UnixStream> {
    let rest = entry.strip_prefix("unix:")?;
    for kv in rest.split(',') {
        if let Some(path) = kv.strip_prefix("path=") {
            use std::os::unix::ffi::OsStringExt;
            let path = std::ffi::OsString::from_vec(unescape(path)?);
            return UnixStream::connect(path).ok();
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(name) = kv.strip_prefix("abstract=") {
            use std::os::linux::net::SocketAddrExt;
            let addr = std::os::unix::net::SocketAddr::from_abstract_name(unescape(name)?).ok()?;
            return UnixStream::connect_addr(&addr).ok();
        }
    }
    None
}

/// A D-Bus address value: bytes with `%XX` escapes.
fn unescape(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    Some(out)
}

/// A line of the authentication exchange, up to `\r\n`.
fn read_line(stream: &mut UnixStream) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8];
    while !line.ends_with(b"\r\n") {
        if line.len() > 512 || stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        line.push(byte[0]);
    }
    String::from_utf8(line).ok()
}

struct Msg {
    kind: u8,
    reply_serial: u32,
    member: String,
    /// The object path (signals, calls).
    path: String,
    /// The sending connection's unique name, as the bus stamped it.
    sender: String,
    /// Addressed to one connection (a destination field), not broadcast.
    directed: bool,
    body: Vec<u8>,
}

/// The new value of a broadcast `SettingChanged` signal for `color-scheme`.
fn setting_changed(msg: &Msg) -> Option<Option<bool>> {
    if msg.kind != SIGNAL || msg.directed || msg.member != "SettingChanged" {
        return None;
    }
    let mut r = Reader::new(&msg.body);
    (r.str() == Some(NAMESPACE) && r.str() == Some(KEY)).then(|| r.variant_u32(0).and_then(scheme))
}

/// A method call with `body`, whose signature is `sig`.
fn method_call(
    serial: u32,
    dest: &str,
    path: &str,
    iface: &str,
    member: &str,
    sig: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut m = Writer::default();
    m.0.extend_from_slice(&[b'l', 1, 0, 1]);
    m.u32(body.len() as u32);
    m.u32(serial);
    m.u32(0); // the fields' length, filled in below
    let start = m.0.len();
    for (code, ty, value) in [
        (1, "o", path),
        (6, "s", dest),
        (2, "s", iface),
        (3, "s", member),
    ] {
        m.field(code, ty, value);
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

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn pad(&mut self, n: usize) {
        self.0.resize(self.0.len().next_multiple_of(n), 0);
    }

    fn u32(&mut self, v: u32) {
        self.pad(4);
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
    }

    fn sig(&mut self, s: &str) {
        self.0.push(s.len() as u8);
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
    }

    /// Start an array whose elements align to `align`: the length is filled in by
    /// [`Writer::end_array`] from what was written in between. Returns where it goes and where
    /// the elements start.
    fn begin_array(&mut self, align: usize) -> (usize, usize) {
        self.u32(0);
        let at = self.0.len() - 4;
        self.pad(align);
        (at, self.0.len())
    }

    fn end_array(&mut self, (at, start): (usize, usize)) {
        let len = (self.0.len() - start) as u32;
        self.0[at..at + 4].copy_from_slice(&len.to_le_bytes());
    }

    /// A dictionary entry's key and the variant signature of its value (`{sv}`), which the
    /// caller then writes.
    fn entry(&mut self, key: &str, sig: &str) {
        self.pad(8);
        self.str(key);
        self.sig(sig);
    }

    fn field(&mut self, code: u8, ty: &str, value: &str) {
        self.pad(8);
        self.0.push(code);
        self.sig(ty);
        if ty == "g" {
            self.sig(value);
        } else {
            self.str(value);
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0 }
    }

    fn align(&mut self, n: usize) {
        self.pos = self.pos.next_multiple_of(n);
    }

    fn byte(&mut self) -> Option<u8> {
        let b = *self.b.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }

    fn u32(&mut self) -> Option<u32> {
        self.align(4);
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    /// A string (`s`, `o`): length, bytes, NUL.
    fn str(&mut self) -> Option<&'a str> {
        let n = self.u32()? as usize;
        let s = self.take(n)?;
        self.byte()?;
        std::str::from_utf8(s).ok()
    }

    /// A signature (`g`): one length byte, bytes, NUL.
    fn sig(&mut self) -> Option<&'a str> {
        let n = self.byte()? as usize;
        let s = self.take(n)?;
        self.byte()?;
        std::str::from_utf8(s).ok()
    }

    /// Skip one value of the complete type at the start of `sig` (which is advanced past it).
    /// Arrays are skipped by their length; structs and variants nest at most 32 deep.
    fn skip(&mut self, sig: &mut &[u8], depth: u8) -> Option<()> {
        if depth > 32 {
            return None;
        }
        let (&c, rest) = sig.split_first()?;
        *sig = rest;
        match c {
            b'y' => _ = self.take(1)?,
            b'b' | b'i' | b'u' | b'h' => _ = self.u32()?,
            b'n' | b'q' => {
                self.align(2);
                self.take(2)?;
            }
            b'x' | b't' | b'd' => {
                self.align(8);
                self.take(8)?;
            }
            b's' | b'o' => _ = self.str()?,
            b'g' => _ = self.sig()?,
            b'v' => {
                let mut inner = self.sig()?.as_bytes();
                self.skip(&mut inner, depth + 1)?;
                if !inner.is_empty() {
                    return None;
                }
            }
            b'a' => {
                let n = self.u32()? as usize;
                self.align(type_align(*sig.first()?));
                skip_type(sig, depth)?;
                self.take(n)?;
            }
            b'(' | b'{' => {
                self.align(8);
                let close = if c == b'(' { b')' } else { b'}' };
                while *sig.first()? != close {
                    self.skip(sig, depth + 1)?;
                }
                *sig = &sig[1..];
            }
            _ => return None,
        }
        Some(())
    }

    /// An array of strings (`as`).
    fn str_array(&mut self) -> Option<Vec<&'a str>> {
        let n = self.u32()? as usize;
        let end = self.pos.checked_add(n)?;
        let mut out = Vec::new();
        while self.pos < end {
            out.push(self.str()?);
        }
        (self.pos == end).then_some(out)
    }

    /// A variant holding a `u`, possibly inside variants (`Read` wraps it once more).
    fn variant_u32(&mut self, depth: u8) -> Option<u32> {
        match self.sig()? {
            "u" => self.u32(),
            "v" if depth < 4 => self.variant_u32(depth + 1),
            _ => None,
        }
    }
}

/// The alignment of a value whose type starts with `c`.
fn type_align(c: u8) -> usize {
    match c {
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b'h' | b's' | b'o' | b'a' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1,
    }
}

/// Advance `sig` past one complete type, without data.
fn skip_type(sig: &mut &[u8], depth: u8) -> Option<()> {
    if depth > 32 {
        return None;
    }
    let (&c, rest) = sig.split_first()?;
    *sig = rest;
    match c {
        b'a' => skip_type(sig, depth + 1),
        b'(' | b'{' => {
            let close = if c == b'(' { b')' } else { b'}' };
            while *sig.first()? != close {
                skip_type(sig, depth + 1)?;
            }
            *sig = &sig[1..];
            Some(())
        }
        b'y' | b'b' | b'n' | b'q' | b'i' | b'u' | b'x' | b't' | b'd' | b'h' | b's' | b'o'
        | b'g' | b'v' => Some(()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
