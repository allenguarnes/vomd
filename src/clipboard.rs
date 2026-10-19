//! The clipboard: what Ctrl+C copies (CLIPBOARD) and what selecting text offers (PRIMARY, on
//! X11 and Wayland), served to the programs that paste it.
//!
//! The window system's own libraries (winit's) don't expose what an owner of a selection must
//! answer, so each backend has a connection of its own to the same display: an XCB connection on
//! X11 (`x11.rs`), and a second event queue on winit's `wl_display` on Wayland (`wayland.rs`).
//! A backend starts on the first copy (a window that never copies pays for nothing), and serves
//! the selections from a thread of its own that sleeps until a program asks, so an idle window
//! stays idle. What is served is a [`Content`], which makes its text as it is asked for.
//! Windows and macOS have no backend yet: a copy there does nothing.

use winit::raw_window_handle::RawDisplayHandle;

use crate::copy::Content;

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod wayland;
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

/// The selections a copy goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// The clipboard: Ctrl+C, pasted with Ctrl+V.
    Clipboard,
    /// The primary selection: what was selected last, pasted with the middle button.
    Primary,
}

/// A backend before its first copy, running, or unable to start (it says so once, and a copy
/// then does nothing).
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
#[derive(Debug)]
enum Backend<T> {
    Idle,
    Running(T),
    Failed,
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
impl<T> Backend<T> {
    /// The running backend, started with `start` the first time (a failure is reported once, on
    /// a terminal).
    fn get(&mut self, start: impl FnOnce() -> Result<T, String>) -> Option<&T> {
        if matches!(self, Backend::Idle) {
            *self = match start() {
                Ok(backend) => Backend::Running(backend),
                Err(why) => {
                    if crate::platform::stderr_is_tty() {
                        eprintln!("vomd: no clipboard: {why}");
                    }
                    Backend::Failed
                }
            };
        }
        match self {
            Backend::Running(backend) => Some(backend),
            _ => None,
        }
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
        }
    }

    /// Offer `content` as the selection `which`, in place of what was offered before. A program
    /// that pastes it gets the text from the thread that serves the selection; it stays on offer
    /// until another program claims the selection or the window closes.
    pub fn set(&mut self, which: Which, content: Content) {
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        match self.display {
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => {
                if let Some(x11) = self.x11.get(x11::Owner::start) {
                    x11.set(which, content);
                }
            }
            RawDisplayHandle::Wayland(handle) => {
                let display = handle.display.as_ptr();
                if let Some(wayland) = self.wayland.get(|| wayland::Owner::start(display)) {
                    wayland.set(which, content);
                }
            }
            _ => {}
        }
        #[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
        let _ = (which, content);
    }

    /// The window is closing: if the clipboard is ours, let a clipboard manager keep it, which
    /// X11 needs (a selection goes with its owner) and a manager may do (the desktop's own
    /// do, and Wayland's data-control ones copy it themselves). Waits a second at the most.
    pub fn save(&self) {
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        if let Backend::Running(x11) = &self.x11 {
            x11.save();
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
