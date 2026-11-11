//! The macOS clipboard: the general pasteboard (`NSPasteboard`), through the AppKit bindings winit
//! already builds. A copy writes the text at once, as one string (made on the window's thread: a
//! long selection is made then, not when a program pastes it), and the pasteboard keeps it after
//! VOMD ends. A paste is read on a thread of its own, which takes at most [`READ_CAP`] bytes of
//! the text and is given up on after [`READ_WAIT`] (another program may make its text only when
//! asked, or be hung). macOS has no primary selection: a copy to it does nothing, and a paste of
//! it finds nothing. Nothing runs until the first copy or paste, and no thread is kept.

use std::sync::mpsc::sync_channel;

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use super::{READ_CAP, READ_WAIT, Reply, Which};
use crate::copy::Content;

/// The pasteboard, for [`super::Backend`].
pub struct Owner;

impl Owner {
    /// Nothing to start: the pasteboard is the system's.
    pub fn start() -> Result<Owner, String> {
        Ok(Owner)
    }

    /// Put `content`'s text on the clipboard, in place of what was there.
    pub fn set(&self, which: Which, content: Content) {
        if which != Which::Clipboard {
            return;
        }
        let text: String = content.chunks().collect();
        let text = NSString::from_str(&text);
        // SAFETY: the general pasteboard, written with a string of the type it is said to be.
        unsafe {
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            board.setString_forType(&text, NSPasteboardTypeString);
        }
    }

    /// Read the clipboard's text for a paste, off the window's thread, and give it to `reply`.
    pub fn read(&self, which: Which, reply: Reply) {
        if which != Which::Clipboard {
            reply.send(None);
            return;
        }
        let (tx, rx) = sync_channel(1);
        std::thread::spawn(move || {
            let _ = tx.send(read_text());
        });
        std::thread::spawn(move || reply.send(rx.recv_timeout(READ_WAIT).ok().flatten()));
    }
}

/// The text on the clipboard, at most [`READ_CAP`] bytes of it (cut on a character's boundary).
fn read_text() -> Option<String> {
    // SAFETY: the general pasteboard, read as a string.
    let text = unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString) }?;
    // A long text is cut before it is copied out: READ_CAP UTF-16 units cover at least READ_CAP
    // bytes, and the cut doesn't fall inside a surrogate pair (the Rust cut below is exact).
    // SAFETY: the indexes are inside the string (`cut < length`).
    let text = unsafe {
        let mut cut = READ_CAP;
        if text.length() > cut {
            if (0xDC00..0xE000).contains(&text.characterAtIndex(cut)) {
                cut -= 1;
            }
            text.substringToIndex(cut)
        } else {
            text
        }
    };
    let mut text = text.to_string();
    if text.len() > READ_CAP {
        let mut cut = READ_CAP;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some(text)
}
