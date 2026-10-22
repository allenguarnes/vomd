//! The Windows clipboard: what Ctrl+C copies, made when a program pastes it, and the text a paste
//! reads for the find bar. Windows has no primary selection, so only the clipboard is served.
//!
//! A thread of its own owns a message-only window, the clipboard's owner when we copy. A copy
//! empties the clipboard and announces Unicode text with no data (delayed rendering): the text is
//! made only when a program asks for it (`WM_RENDERFORMAT`), and made at once if the window ends
//! while the clipboard is still ours (`WM_RENDERALLFORMATS`), so it outlives VOMD. Lines end in
//! CR LF, as Windows programs expect. A paste of our own copy is answered from what we offer; a
//! paste of another program's text is read on a worker, which takes at most [`READ_CAP`] bytes and
//! is given up on after [`READ_WAIT`] (its owner may be making it, or be hung). Nothing runs until
//! the first copy or paste, and the thread sleeps in `GetMessageW` until a message comes.
//! Plain Win32 calls, declared here by hand.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::mpsc::{Receiver, Sender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{READ_CAP, READ_WAIT, Reply, Which};
use crate::copy::Content;

type Hwnd = *mut c_void;
type Handle = *mut c_void;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    wnd_proc: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_sm: Handle,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn PostQuitMessage(code: i32);
    fn OpenClipboard(owner: Hwnd) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, data: Handle) -> Handle;
    fn GetClipboardData(format: u32) -> Handle;
    fn GetClipboardOwner() -> Hwnd;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalFree(mem: Handle) -> Handle;
    fn GlobalLock(mem: Handle) -> *mut c_void;
    fn GlobalUnlock(mem: Handle) -> i32;
    fn GlobalSize(mem: Handle) -> usize;
}

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x2;
const HWND_MESSAGE: Hwnd = -3isize as Hwnd;
const WM_DESTROY: u32 = 0x0002;
const WM_RENDERFORMAT: u32 = 0x0305;
const WM_RENDERALLFORMATS: u32 = 0x0306;
const WM_DESTROYCLIPBOARD: u32 = 0x0307;
/// A request is waiting in the channel.
const WM_REQUEST: u32 = 0x8000; // WM_APP

/// How long the window's closing waits for the text to be made for the clipboard to keep.
const SAVE_WAIT: Duration = Duration::from_secs(5);
/// Another program may have the clipboard open for a moment: how many times, how far apart, an
/// open is tried.
const OPEN_TRIES: u32 = 20;
const OPEN_PAUSE: Duration = Duration::from_millis(5);

enum Request {
    Set(Content),
    Read(Reply),
    /// The window is closing: end, leaving the text with the clipboard, and say so.
    Quit(Option<Sender<()>>),
}

/// The thread that owns our clipboard window.
pub struct Owner {
    hwnd: usize,
    requests: Sender<Request>,
    /// Sent [`Request::Quit`] already (by [`Owner::save`]).
    quit: Mutex<bool>,
}

thread_local! {
    /// The clipboard thread's requests and what it offers (only the thread's window procedure
    /// uses them). Never borrowed across a clipboard call: those can call the window procedure
    /// back on this thread (`EmptyClipboard` sends `WM_DESTROYCLIPBOARD`).
    static REQUESTS: RefCell<Option<Receiver<Request>>> = const { RefCell::new(None) };
    static OFFERED: RefCell<Option<Content>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

impl Owner {
    /// Start the thread and its window. `Err` says why there is none.
    pub fn start() -> Result<Owner, String> {
        let (tx, rx) = channel();
        let (ready, started) = sync_channel(1);
        std::thread::Builder::new()
            .name("vomd-clipboard".into())
            .spawn(move || run(rx, ready))
            .map_err(|e| format!("cannot start the clipboard thread: {e}"))?;
        let hwnd = started
            .recv()
            .map_err(|_| "the clipboard thread ended".to_string())??;
        Ok(Owner {
            hwnd,
            requests: tx,
            quit: Mutex::new(false),
        })
    }

    fn send(&self, request: Request) -> bool {
        if self.requests.send(request).is_err() {
            return false;
        }
        // SAFETY: posting to a window handle is safe even if the window has gone (it fails).
        unsafe { PostMessageW(self.hwnd as Hwnd, WM_REQUEST, 0, 0) != 0 }
    }

    /// Offer `content` as the clipboard. Windows has no primary selection: that does nothing.
    pub fn set(&self, which: Which, content: Content) {
        if which == Which::Clipboard {
            self.send(Request::Set(content));
        }
    }

    /// Read the clipboard for a paste (the primary selection is never anything).
    pub fn read(&self, which: Which, reply: Reply) {
        if which == Which::Clipboard {
            self.send(Request::Read(reply));
        } else {
            reply.send(None);
        }
    }

    /// As the window closes: if the clipboard is ours, have its text made for the clipboard to
    /// keep after VOMD ends. Waits [`SAVE_WAIT`] at the most.
    pub fn save(&self) {
        let mut quit = self.quit.lock().unwrap_or_else(|e| e.into_inner());
        if std::mem::replace(&mut *quit, true) {
            return;
        }
        let (done, saved) = channel();
        if self.send(Request::Quit(Some(done))) {
            let _ = saved.recv_timeout(SAVE_WAIT);
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if !*self.quit.get_mut().unwrap_or_else(|e| e.into_inner()) {
            self.send(Request::Quit(None));
        }
    }
}

/// The clipboard thread: its window, then its messages until it is told to end.
fn run(requests: Receiver<Request>, ready: std::sync::mpsc::SyncSender<Result<usize, String>>) {
    let class = wide("vomd-clipboard");
    // SAFETY: plain calls with valid arguments: the class's strings outlive the window, which is
    // destroyed on this thread before it returns.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WndClassExW {
            size: size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon: std::ptr::null_mut(),
            cursor: std::ptr::null_mut(),
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: class.as_ptr(),
            icon_sm: std::ptr::null_mut(),
        };
        // A second start (a new backend in the same process) finds the class registered already,
        // which is no matter: the window is what counts.
        RegisterClassExW(&wc);
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        )
    };
    if hwnd.is_null() {
        let why = std::io::Error::last_os_error();
        let _ = ready.send(Err(format!("cannot make the clipboard's window: {why}")));
        return;
    }
    REQUESTS.with(|r| *r.borrow_mut() = Some(requests));
    let _ = ready.send(Ok(hwnd as usize));
    let mut msg: Msg = unsafe { std::mem::zeroed() };
    // SAFETY: `msg` is a valid MSG to fill, and the loop ends on WM_QUIT (0) or an error (-1).
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe { DispatchMessageW(&msg) };
    }
    // Whatever is still waiting is answered: a read finds nothing, a save is done.
    REQUESTS.with(|r| r.borrow_mut().take());
    OFFERED.with(|o| o.borrow_mut().take());
}

/// The next request, if one is waiting (the borrow ends before it is handled).
fn next_request() -> Option<Request> {
    REQUESTS.with(|r| r.borrow().as_ref().and_then(|rx| rx.try_recv().ok()))
}

extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_REQUEST => {
            while let Some(request) = next_request() {
                match request {
                    Request::Set(content) => offer(hwnd, content),
                    Request::Read(reply) => read(hwnd, reply),
                    Request::Quit(done) => {
                        // Destroying the window makes the text first if the clipboard is ours
                        // (WM_RENDERALLFORMATS), then ends the loop (WM_DESTROY).
                        // SAFETY: our own window, on its own thread.
                        unsafe { DestroyWindow(hwnd) };
                        if let Some(done) = done {
                            let _ = done.send(());
                        }
                        return 0;
                    }
                }
            }
            0
        }
        WM_RENDERFORMAT if wparam as u32 == CF_UNICODETEXT => {
            // The clipboard is open (by the program pasting): only the data is given.
            if let Some(text) = render() {
                put(text);
            }
            0
        }
        WM_RENDERALLFORMATS => {
            // SAFETY: plain calls; the clipboard is ours to fill only while we still own it.
            unsafe {
                if open(hwnd) {
                    if GetClipboardOwner() == hwnd
                        && let Some(text) = render()
                    {
                        put(text);
                    }
                    CloseClipboard();
                }
            }
            0
        }
        WM_DESTROYCLIPBOARD => {
            // Another program (or our next copy) emptied the clipboard: what we offered goes.
            OFFERED.with(|o| o.borrow_mut().take());
            0
        }
        WM_DESTROY => {
            // SAFETY: a plain call on the thread whose loop it ends.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: the default handling of every other message, with its own arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Open the clipboard as `owner` (null: as nobody), trying again for a moment while another
/// program has it open.
fn open(owner: Hwnd) -> bool {
    for i in 0..OPEN_TRIES {
        // SAFETY: a plain call.
        if unsafe { OpenClipboard(owner) } != 0 {
            return true;
        }
        if i + 1 < OPEN_TRIES {
            std::thread::sleep(OPEN_PAUSE);
        }
    }
    false
}

/// Take the clipboard and announce text to be made when it is asked for.
fn offer(hwnd: Hwnd, content: Content) {
    if !open(hwnd) {
        if crate::platform::stderr_is_tty() {
            eprintln!("vomd: cannot copy: another program has the clipboard open");
        }
        return;
    }
    // SAFETY: the clipboard is open by us. Emptying it sends WM_DESTROYCLIPBOARD to the previous
    // owner (perhaps us, which drops the old offer), so the new one is kept after.
    unsafe {
        EmptyClipboard();
        SetClipboardData(CF_UNICODETEXT, std::ptr::null_mut());
    }
    OFFERED.with(|o| *o.borrow_mut() = Some(content));
    // SAFETY: opened above.
    unsafe { CloseClipboard() };
}

/// The text offered, with CR LF line ends, NUL-terminated, as `CF_UNICODETEXT` is.
fn render() -> Option<Vec<u16>> {
    let chunks = OFFERED.with(|o| o.borrow().as_ref().map(Content::chunks))?;
    let mut text = Vec::new();
    let mut cr = false;
    for chunk in chunks {
        for unit in chunk.encode_utf16() {
            if unit == u16::from(b'\n') && !cr {
                text.push(u16::from(b'\r'));
            }
            cr = unit == u16::from(b'\r');
            text.push(unit);
        }
    }
    text.push(0);
    Some(text)
}

/// Give `text` to the open clipboard as its Unicode text.
fn put(text: Vec<u16>) {
    let bytes = text.len() * 2;
    // SAFETY: the memory is allocated for the text's length and written within it; once given to
    // the clipboard it is the clipboard's, and freed here only if it wasn't taken.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem.is_null() {
            return;
        }
        let at = GlobalLock(mem).cast::<u16>();
        if at.is_null() {
            GlobalFree(mem);
            return;
        }
        std::ptr::copy_nonoverlapping(text.as_ptr(), at, text.len());
        GlobalUnlock(mem);
        if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
            GlobalFree(mem);
        }
    }
}

/// A paste: our own copy is answered from what we offer (its first piece: the find bar takes a
/// line), another program's is read on a worker.
fn read(hwnd: Hwnd, reply: Reply) {
    // SAFETY: a plain call.
    if unsafe { GetClipboardOwner() } == hwnd {
        let first = OFFERED.with(|o| o.borrow().as_ref().map(|c| c.chunks().next()));
        if let Some(first) = first {
            reply.send(first);
            return;
        }
    }
    // Answered once, by the worker or by the timer, whichever is first.
    let reply = Arc::new(Mutex::new(Some(reply)));
    let answer = move |reply: &Mutex<Option<Reply>>, text: Option<String>| {
        if let Some(reply) = reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
            reply.send(text);
        }
    };
    let (theirs, timer) = (reply.clone(), reply);
    let spawned = std::thread::Builder::new()
        .name("vomd-paste".into())
        .spawn(move || answer(&theirs, read_foreign()));
    if spawned.is_err() {
        answer(&timer, None);
        return;
    }
    let _ = std::thread::Builder::new()
        .name("vomd-paste-timer".into())
        .spawn(move || {
            std::thread::sleep(READ_WAIT);
            answer(&timer, None);
        });
}

/// Another program's text on the clipboard, at most [`READ_CAP`] bytes of it.
fn read_foreign() -> Option<String> {
    if !open(std::ptr::null_mut()) {
        return None;
    }
    // SAFETY: the clipboard is open; the data handle is the clipboard's, read only while locked
    // and within its size, and the clipboard is closed before returning.
    let text = unsafe {
        let mem = GetClipboardData(CF_UNICODETEXT);
        let at = if mem.is_null() {
            std::ptr::null_mut()
        } else {
            GlobalLock(mem).cast::<u16>()
        };
        let text = (!at.is_null()).then(|| {
            // A UTF-8 byte is at most one UTF-16 unit: READ_CAP units are enough for READ_CAP
            // bytes of text.
            let units = (GlobalSize(mem) / 2).min(READ_CAP);
            let units = std::slice::from_raw_parts(at, units);
            let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            let mut text = String::from_utf16_lossy(&units[..len]);
            let mut cut = text.len().min(READ_CAP);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            GlobalUnlock(mem);
            text
        });
        CloseClipboard();
        text
    };
    text.map(|t| t.replace("\r\n", "\n"))
}
