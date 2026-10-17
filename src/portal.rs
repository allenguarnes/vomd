//! The desktop's light/dark preference: the XDG desktop portal's
//! `org.freedesktop.appearance` `color-scheme` setting, read over the session D-Bus. winit
//! reports no theme on Wayland, and KDE sets no `GTK_THEME`, so this is how a Plasma or GNOME
//! dark mode is seen. A minimal client, no D-Bus crate: one connection, EXTERNAL
//! authentication, `ReadOne` (`Read` on portals before version 2), then the `SettingChanged`
//! signal for changes while the window is open.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

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
/// away. `None` is no preference, or no bus or portal to ask (use the other sources).
pub fn watch(mut f: impl FnMut(Option<bool>)) {
    let Some(mut bus) = Bus::connect() else {
        f(None);
        return;
    };
    f(bus.read_setting());
    bus.follow(f);
}

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
}

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
        // A portal that never answers mustn't hold the thread for good.
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
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
        let mut bus = Bus { stream, serial: 0 };
        let hello = bus.call(BUS, "/org/freedesktop/DBus", BUS, "Hello", "", &[])?;
        (hello.kind == RETURN).then_some(bus)
    }

    /// The current `color-scheme`.
    fn read_setting(&mut self) -> Option<bool> {
        let args = [NAMESPACE, KEY];
        let mut reply = self.call(PORTAL, PORTAL_PATH, SETTINGS, "ReadOne", "ss", &args)?;
        if reply.kind == ERROR {
            reply = self.call(PORTAL, PORTAL_PATH, SETTINGS, "Read", "ss", &args)?;
        }
        if reply.kind != RETURN {
            return None;
        }
        Reader::new(&reply.body).variant_u32(0).and_then(scheme)
    }

    /// Ask for `SettingChanged` on `color-scheme` and hand each new value to `f`, until the
    /// connection ends. Blocks without waking in between.
    fn follow(&mut self, mut f: impl FnMut(Option<bool>)) {
        let rule = concat!(
            "type='signal',interface='org.freedesktop.portal.Settings',",
            "member='SettingChanged',path='/org/freedesktop/portal/desktop',",
            "arg0='org.freedesktop.appearance',arg1='color-scheme'"
        );
        let added = self.call(BUS, "/org/freedesktop/DBus", BUS, "AddMatch", "s", &[rule]);
        if added.is_none_or(|m| m.kind != RETURN) || self.stream.set_read_timeout(None).is_err() {
            return;
        }
        while let Ok(msg) = self.recv() {
            if msg.kind == SIGNAL && msg.member == "SettingChanged" {
                let mut r = Reader::new(&msg.body);
                if r.str() == Some(NAMESPACE) && r.str() == Some(KEY) {
                    f(r.variant_u32(0).and_then(scheme));
                }
            }
        }
    }

    /// Call a method and wait for its reply (or error), skipping other messages.
    fn call(
        &mut self,
        dest: &str,
        path: &str,
        iface: &str,
        member: &str,
        sig: &str,
        args: &[&str],
    ) -> Option<Msg> {
        self.serial += 1;
        let serial = self.serial;
        let msg = method_call(serial, dest, path, iface, member, sig, args);
        self.stream.write_all(&msg).ok()?;
        loop {
            let reply = self.recv().ok()?;
            if matches!(reply.kind, RETURN | ERROR) && reply.reply_serial == serial {
                return Some(reply);
            }
        }
    }

    fn recv(&mut self) -> io::Result<Msg> {
        let bad = || io::Error::from(io::ErrorKind::InvalidData);
        let mut fixed = [0u8; 16];
        self.stream.read_exact(&mut fixed)?;
        // Messages come in the sender's byte order; every bus we run on is little-endian.
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
            body,
        };
        let mut r = Reader::new(&fields[..fields_len as usize]);
        while r.pos < fields_len as usize {
            r.align(8);
            let code = r.byte().ok_or_else(bad)?;
            match (code, r.sig().ok_or_else(bad)?) {
                (3, "s") => msg.member = r.str().ok_or_else(bad)?.to_owned(),
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
    body: Vec<u8>,
}

/// A method call with string arguments.
fn method_call(
    serial: u32,
    dest: &str,
    path: &str,
    iface: &str,
    member: &str,
    sig: &str,
    args: &[&str],
) -> Vec<u8> {
    let mut body = Writer::default();
    for a in args {
        body.str(a);
    }
    let mut m = Writer::default();
    m.0.extend_from_slice(&[b'l', 1, 0, 1]);
    m.u32(body.0.len() as u32);
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
    m.0.extend_from_slice(&body.0);
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

    /// A variant holding a `u`, possibly inside variants (`Read` wraps it once more).
    fn variant_u32(&mut self, depth: u8) -> Option<u32> {
        match self.sig()? {
            "u" => self.u32(),
            "v" if depth < 4 => self.variant_u32(depth + 1),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
