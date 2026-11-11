//! macOS: the files Finder and the Dock hand over, Open's panel, and the memory figures, through
//! AppKit (the objc2 crates winit already builds) and libSystem.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{ffi, sel};
use objc2_app_kit::{NSApplication, NSModalResponse, NSModalResponseOK, NSOpenPanel, NSView};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};
use winit::raw_window_handle::RawWindowHandle;

/// What files handed over are given to.
type Deliver = Box<dyn Fn(Vec<PathBuf>)>;

thread_local! {
    /// Where [`open_urls`] sends the files it is given (set on the main thread, where AppKit
    /// calls it).
    static DELIVER: RefCell<Option<Deliver>> = const { RefCell::new(None) };
}

/// Send the files the system hands over to `deliver`, on the main thread: Finder's Open With, a
/// double-click, a drop on the Dock icon, `open -a VOMD file.md`. They come as an Apple event,
/// never in the command line, and AppKit gives them to its delegate's `application:openURLs:`,
/// which winit's delegate doesn't have: it is added to that class here (AppKit asks the delegate
/// whether it has the method each time files come). Call it after the event loop is built (the
/// delegate is set then) and before it runs (the file VOMD was started for comes while it
/// starts). Does nothing off the main thread or without a delegate.
pub fn on_open_files(deliver: impl Fn(Vec<PathBuf>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: read on the main thread.
    let Some(delegate) = (unsafe { app.delegate() }) else {
        return;
    };
    DELIVER.with(|d| *d.borrow_mut() = Some(Box::new(deliver)));
    // SAFETY: a live object.
    let class: &AnyClass = unsafe { &*Retained::as_ptr(&delegate).cast::<AnyObject>() }.class();
    type Imp = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>);
    let imp: Imp = open_urls;
    // SAFETY: the method's types are what the encoding says (void; self, _cmd, two objects), and
    // a class that has it already keeps its own (`class_addMethod` adds, never replaces).
    unsafe {
        ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut().cast(),
            sel!(application:openURLs:).as_ptr(),
            Some(std::mem::transmute::<Imp, unsafe extern "C" fn()>(imp)),
            c"v@:@@".as_ptr(),
        );
    }
}

/// `-[delegate application:openURLs:]`: the local files of `urls`, delivered.
unsafe extern "C" fn open_urls(
    _: *mut AnyObject,
    _: Sel,
    _: *mut AnyObject,
    urls: *mut NSArray<NSURL>,
) {
    // SAFETY: AppKit passes an array of URLs, alive for the call.
    let Some(urls) = (unsafe { urls.as_ref() }) else {
        return;
    };
    let paths: Vec<PathBuf> = urls.iter().filter_map(file_path).collect();
    if !paths.is_empty() {
        DELIVER.with(|d| {
            if let Some(deliver) = &*d.borrow() {
                deliver(paths);
            }
        });
    }
}

/// The path of a `file:` URL (`None` for others).
fn file_path(url: &NSURL) -> Option<PathBuf> {
    // SAFETY: plain getters.
    unsafe { url.isFileURL().then(|| url.path()).flatten() }.map(|p| PathBuf::from(p.to_string()))
}

/// Ask for a Markdown file to open, starting in `folder`, with the system's Open panel: a sheet
/// on `window` (a panel of its own without one), so neither a thread nor a modal loop of its own
/// is needed. Returns at once; `done` is called on the main thread when the panel closes, with
/// the file picked (`None`: cancelled). Main thread only (`Err` elsewhere, at once).
pub fn choose_file(
    window: Option<RawWindowHandle>,
    title: &str,
    folder: Option<&Path>,
    done: impl FnOnce(Result<Option<PathBuf>, String>) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(Err(
            "Open's panel can only be shown from the main thread".into()
        ));
        return;
    };
    // SAFETY: AppKit calls on the main thread with live objects; the view is the window's own
    // (winit's), alive as long as the window.
    unsafe {
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setMessage(Some(&NSString::from_str(title)));
        let types: Vec<Retained<NSString>> = super::MARKDOWN_GLOBS
            .iter()
            .map(|g| NSString::from_str(g.trim_start_matches("*.")))
            .collect();
        // (The string API: `allowedContentTypes` needs the UniformTypeIdentifiers bindings.)
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_vec(types)));
        if let Some(folder) = folder {
            let path = NSString::from_str(&folder.to_string_lossy());
            panel.setDirectoryURL(Some(&NSURL::fileURLWithPath_isDirectory(&path, true)));
        }
        let done = Cell::new(Some(done));
        let picked = panel.clone();
        let handler = block2::RcBlock::new(move |response: NSModalResponse| {
            let path = (response == NSModalResponseOK)
                .then(|| picked.URL())
                .flatten()
                .and_then(|url| file_path(&url));
            if let Some(done) = done.take() {
                done(Ok(path));
            }
        });
        let ns_window = match window {
            Some(RawWindowHandle::AppKit(h)) => h.ns_view.cast::<NSView>().as_ref().window(),
            _ => None,
        };
        match ns_window {
            Some(w) => panel.beginSheetModalForWindow_completionHandler(&w, &handler),
            None => panel.beginWithCompletionHandler(&handler),
        }
    }
}

/// This process's resource usage (`proc_pid_rusage`, `RUSAGE_INFO_V4`) as 64-bit words: the
/// struct is 16 bytes of UUID then 35 counters, all `u64`.
fn rusage() -> Option<[u64; 37]> {
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut [u64; 37]) -> i32;
    }
    const RUSAGE_INFO_V4: i32 = 4;
    let mut info = [0; 37];
    // SAFETY: the buffer is the size of a rusage_info_v4.
    let ok = unsafe { proc_pid_rusage(std::process::id() as i32, RUSAGE_INFO_V4, &mut info) } == 0;
    ok.then_some(info)
}

/// The physical footprint now and its lifetime peak, in KB: Activity Monitor's figures. (The
/// resident size also counts the shared framework pages every application maps.)
pub fn footprint_kb() -> (u64, u64) {
    // ri_phys_footprint and ri_lifetime_max_phys_footprint.
    rusage().map_or((0, 0), |r| (r[9] / 1024, r[30] / 1024))
}
