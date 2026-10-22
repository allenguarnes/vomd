//! The clipboard: what Ctrl+C copies (CLIPBOARD) and what selecting text offers (PRIMARY, on
//! X11 and Wayland), served to the programs that paste it, and what they have offered, read for
//! the find bar's field.
//!
//! The window system's own libraries (winit's) don't expose what an owner of a selection must
//! answer, so each backend has a connection of its own to the same display: an XCB connection on
//! X11 (`x11.rs`), and a second event queue on winit's `wl_display` on Wayland (`wayland.rs`).
//! A backend starts on the first copy or paste (a window that never uses the clipboard pays for
//! nothing), and serves the selections from a thread of its own that sleeps until a program asks,
//! so an idle window stays idle. What is served is a [`Content`], which makes its text as it is
//! asked for. A paste is read by that thread too (or a worker of it), never by the window's: it
//! asks the owner of the selection for text, takes at most [`READ_CAP`] bytes of it, gives up
//! after [`READ_WAIT`], and hands the text to a callback (see [`Clipboard::read`]). On Windows
//! (`windows.rs`) a thread of its own owns the clipboard through a message-only window, and makes
//! the text when a program asks for it too; Windows has no primary selection. macOS has no
//! backend yet: a copy there does nothing, and a paste finds nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use winit::raw_window_handle::RawDisplayHandle;

use crate::copy::Content;

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod wayland;
#[cfg(windows)]
mod windows;
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod x11;

/// The types of text a Wayland program may ask a selection for (the X11 ones are atoms; see
/// `x11.rs`), the one that is plain UTF-8 text first.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "TEXT",
    "STRING",
    "text/plain",
];

/// The most bytes of a selection a paste reads. The find bar's field takes one short line, and a
/// program may offer a whole book.
pub const READ_CAP: usize = 64 * 1024;
/// How long a paste waits for the owner of the selection to answer.
pub const READ_WAIT: Duration = Duration::from_secs(2);

/// The selections a copy goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// The clipboard: Ctrl+C, pasted with Ctrl+V.
    Clipboard,
    /// The primary selection: what was selected last, pasted with the middle button.
    Primary,
}

/// A backend before its first copy, running, or unable to start (it says so once, and a copy
/// then does nothing; the reason is kept for whoever asks).
#[cfg(any(windows, all(unix, not(any(target_os = "macos", target_os = "ios")))))]
#[derive(Debug)]
enum Backend<T> {
    Idle,
    Running(T),
    Failed(String),
}

#[cfg(any(windows, all(unix, not(any(target_os = "macos", target_os = "ios")))))]
impl<T> Backend<T> {
    /// The running backend, started with `start` the first time (a failure is reported once, on
    /// a terminal). `Err` is why there is none.
    fn get(&mut self, start: impl FnOnce() -> Result<T, String>) -> Result<&T, &str> {
        if matches!(self, Backend::Idle) {
            *self = match start() {
                Ok(backend) => Backend::Running(backend),
                Err(why) => {
                    if crate::platform::stderr_is_tty() {
                        eprintln!("vomd: no clipboard: {why}");
                    }
                    Backend::Failed(why)
                }
            };
        }
        match self {
            Backend::Running(backend) => Ok(backend),
            Backend::Failed(why) => Err(why),
            Backend::Idle => Err("not started"),
        }
    }
}

/// Where the text of a paste goes when it is read: the callback of [`Clipboard::read`], called
/// once, from whichever thread read it, with `None` if there was nothing to read. Dropped
/// unanswered (the backend's thread ended) it says that too, so a read always ends.
pub struct Reply(Option<Box<dyn FnOnce(Option<String>) + Send>>);

impl Reply {
    fn new(done: impl FnOnce(Option<String>) + Send + 'static) -> Reply {
        Reply(Some(Box::new(done)))
    }

    /// The text read, if there was any (nothing at all is no text).
    #[cfg_attr(
        not(any(windows, all(unix, not(any(target_os = "macos", target_os = "ios"))))),
        allow(dead_code)
    )]
    fn send(mut self, text: Option<String>) {
        if let Some(done) = self.0.take() {
            done(text.filter(|t| !t.is_empty()));
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            done(None);
        }
    }
}

/// The text of the bytes a selection gave: Latin-1 for `STRING`, else UTF-8, whose bad bytes (and
/// a character cut off by [`READ_CAP`]) show as `\u{fffd}`.
#[cfg_attr(
    not(all(unix, not(any(target_os = "macos", target_os = "ios")))),
    allow(dead_code)
)]
fn decode(bytes: &[u8], latin1: bool) -> String {
    if latin1 {
        bytes.iter().map(|&b| char::from(b)).collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// The clipboard of the window system the window is on.
pub struct Clipboard {
    #[cfg_attr(
        not(all(unix, not(any(target_os = "macos", target_os = "ios")))),
        allow(dead_code)
    )]
    display: RawDisplayHandle,
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
    x11: Backend<x11::Owner>,
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
    wayland: Backend<wayland::Owner>,
    #[cfg(windows)]
    windows: Backend<windows::Owner>,
    /// A read is under way: one at a time, as a person pastes.
    reading: Arc<AtomicBool>,
}

impl Clipboard {
    /// The clipboard for a window on `display`. It only notes the display: nothing is connected
    /// to, and no thread runs, until the first [`Clipboard::set`].
    pub fn new(display: RawDisplayHandle) -> Clipboard {
        Clipboard {
            display,
            #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
            x11: Backend::Idle,
            #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
            wayland: Backend::Idle,
            #[cfg(windows)]
            windows: Backend::Idle,
            reading: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Offer `content` as the selection `which`, in place of what was offered before. A program
    /// that pastes it gets the text from the thread that serves the selection; it stays on offer
    /// until another program claims the selection or the window closes. `Err` is why nothing was
    /// offered (the backend couldn't start, or this window system has none), short enough for a
    /// toast.
    pub fn set(&mut self, which: Which, content: Content) -> Result<(), String> {
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        match self.display {
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => {
                let x11 = self.x11.get(x11::Owner::start)?;
                x11.set(which, content);
                Ok(())
            }
            RawDisplayHandle::Wayland(handle) => {
                let display = handle.display.as_ptr();
                let wayland = self.wayland.get(|| wayland::Owner::start(display))?;
                wayland.set(which, content);
                Ok(())
            }
            _ => Err("this display has no clipboard support".into()),
        }
        #[cfg(windows)]
        {
            self.windows.get(windows::Owner::start)?.set(which, content);
            Ok(())
        }
        #[cfg(not(any(windows, all(unix, not(any(target_os = "macos", target_os = "ios"))))))]
        {
            let _ = (which, content);
            Err("this system has no clipboard support".into())
        }
    }

    /// Read the text of the selection `which`, for a paste, and call `done` with it, from another
    /// thread, once it is read: `None` if there is no owner, no text, nothing to ask (a window
    /// system with no clipboard here), or no answer in [`READ_WAIT`]. This never waits: a selection
    /// that is ours is answered from what we offer, and another program's is asked for by the
    /// backend's thread, which takes at most [`READ_CAP`] bytes of it. A read while another is
    /// under way finds nothing. Only the first line of the text matters to the one pasting it, so
    /// a long selection is cut where it is read.
    pub fn read(&mut self, which: Which, done: impl FnOnce(Option<String>) + Send + 'static) {
        let reading = self.reading.clone();
        if reading.swap(true, Ordering::AcqRel) {
            done(None);
            return;
        }
        let reply = Reply::new(move |text| {
            reading.store(false, Ordering::Release);
            done(text);
        });
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        match self.display {
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => {
                match self.x11.get(x11::Owner::start) {
                    Ok(x11) => x11.read(which, reply),
                    Err(_) => reply.send(None),
                }
            }
            RawDisplayHandle::Wayland(handle) => {
                let display = handle.display.as_ptr();
                match self.wayland.get(|| wayland::Owner::start(display)) {
                    Ok(wayland) => wayland.read(which, reply),
                    Err(_) => reply.send(None),
                }
            }
            _ => reply.send(None),
        }
        #[cfg(windows)]
        match self.windows.get(windows::Owner::start) {
            Ok(owner) => owner.read(which, reply),
            Err(_) => reply.send(None),
        }
        #[cfg(not(any(windows, all(unix, not(any(target_os = "macos", target_os = "ios"))))))]
        {
            let _ = which;
            drop(reply);
        }
    }

    /// The window is closing: if the clipboard is ours, let a clipboard manager keep it, which
    /// X11 needs (a selection goes with its owner) and a manager may do (the desktop's own
    /// do, and Wayland's data-control ones copy it themselves). Waits a second at the most. On
    /// Windows the text is made for the clipboard to keep (a few seconds at the most).
    pub fn save(&self) {
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        if let Backend::Running(x11) = &self.x11 {
            x11.save();
        }
        #[cfg(windows)]
        if let Backend::Running(owner) = &self.windows {
            owner.save();
        }
    }
}

/// `text` as Latin-1, which is what the `STRING` type is: a character it doesn't have is `?`.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

#[cfg(all(test, unix, not(any(target_os = "macos", target_os = "ios"))))]
mod tests;
