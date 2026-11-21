//! macOS: the files Finder and the Dock hand over, Open's panel, and the memory figures, through
//! AppKit (the objc2 crates winit already builds) and libSystem.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{ffi, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDidFinishLaunchingNotification,
    NSApplicationLaunchIsDefaultLaunchKey, NSDocumentController, NSMenu, NSMenuItem,
    NSModalResponse, NSModalResponseOK, NSOpenPanel, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSAppleEventManager, NSArray, NSNotification, NSNotificationCenter, NSNumber,
    NSString, NSURL, ns_string,
};
use winit::raw_window_handle::RawWindowHandle;

/// What files handed over, and the File menu's commands, are given to.
type Deliver = Box<dyn Fn(Vec<PathBuf>)>;
type MenuDeliver = Box<dyn Fn(MenuCommand)>;

/// What an item of the File menu asks for.
#[derive(Debug)]
pub enum MenuCommand {
    /// Open… (⌘O).
    Open,
    /// An item of Open Recent: its file.
    OpenRecent(PathBuf),
    /// Open Recent ▸ Clear Menu.
    Clear,
}

thread_local! {
    /// Where [`open_urls`] sends the files it is given (set on the main thread, where AppKit
    /// calls it).
    static DELIVER: RefCell<Option<Deliver>> = const { RefCell::new(None) };
    /// Whether AppKit called the launch a default one (see [`default_launch`]).
    static DEFAULT_LAUNCH: Cell<Option<bool>> = const { Cell::new(None) };
    /// Where the File menu's items send what they ask for, the object they are sent to (winit's
    /// delegate), Open Recent's submenu, and the files of its items by their tags.
    static MENU: RefCell<Option<MenuDeliver>> = const { RefCell::new(None) };
    static MENU_TARGET: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static RECENT_MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    static FILE_MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    static RECENT_FILES: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// A method of one object argument (`-(void)name:(id)sender`), as added to winit's delegate.
type Action = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject);

/// Add `imp` to `class` as `sel`, unless the class has such a method already
/// (`class_addMethod` adds, never replaces).
///
/// SAFETY: `imp` takes what the encoding says: self, _cmd, and one object.
unsafe fn add_action(class: &AnyClass, sel: Sel, imp: Action) {
    // SAFETY: as the caller says; `v@:@` is void (self, _cmd, an object).
    unsafe {
        ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut().cast(),
            sel.as_ptr(),
            Some(std::mem::transmute::<Action, unsafe extern "C" fn()>(imp)),
            c"v@:@".as_ptr(),
        );
    }
}

/// Add a File menu after the application's menu that winit made: "Open… ⌘O" and "Open Recent",
/// which [`set_recent_menu`] fills. Its items call methods added to winit's delegate, which send
/// what was chosen to `deliver`, on the main thread. Call it once the event loop runs (winit
/// makes its menu when the application has finished launching, before `resumed`). Does nothing
/// off the main thread or without the delegate and the menu.
///
/// The menu is VOMD's own, fed by its list ([`crate::recent`]), not AppKit's automatic Open
/// Recent: that one takes only the types the bundle declares, so `README` or a `.txt` would be
/// missing from it, and it couldn't clear VOMD's list. AppKit fills a menu only if it has an
/// item with its own `clearRecentDocuments:` action, which this one hasn't, so it leaves this
/// one alone. The ⌘O of the menu takes the key before the window sees it: Open's dialog comes
/// from here then, as from Ctrl+O and the toolbar.
pub fn add_file_menu(deliver: impl Fn(MenuCommand) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: read on the main thread.
    let (Some(delegate), Some(bar)) = (unsafe { app.delegate() }, unsafe { app.mainMenu() }) else {
        return;
    };
    // SAFETY: a live object; retaining it again as `AnyObject` keeps it alive as well.
    let target: Retained<AnyObject> =
        unsafe { Retained::retain(Retained::as_ptr(&delegate).cast::<AnyObject>().cast_mut()) }
            .expect("a live delegate");
    let class = target.class();
    // SAFETY: each function takes self, _cmd and the item that was chosen.
    unsafe {
        add_action(class, sel!(vomdOpen:), menu_open);
        add_action(class, sel!(vomdOpenRecent:), menu_open_recent);
        add_action(class, sel!(vomdClearRecent:), menu_clear_recent);
    }
    MENU.with(|m| *m.borrow_mut() = Some(Box::new(deliver)));
    // SAFETY: AppKit's menus, made and changed on the main thread; the items' target is the
    // delegate, which lives as long as the application (and is kept in MENU_TARGET too).
    unsafe {
        let file = NSMenu::initWithTitle(mtm.alloc(), ns_string!("File"));
        // Greyed out by hand while Open's sheet is up ([`set_open_enabled`]).
        file.setAutoenablesItems(false);
        let open = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            ns_string!("Open…"),
            Some(sel!(vomdOpen:)),
            ns_string!("o"),
        );
        open.setTarget(Some(&target));
        file.addItem(&open);
        let recent = NSMenu::initWithTitle(mtm.alloc(), ns_string!("Open Recent"));
        // Clear Menu is greyed out by hand while the list is empty.
        recent.setAutoenablesItems(false);
        let recent_item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            ns_string!("Open Recent"),
            None,
            ns_string!(""),
        );
        recent_item.setSubmenu(Some(&recent));
        file.addItem(&recent_item);
        let file_item = NSMenuItem::new(mtm);
        file_item.setSubmenu(Some(&file));
        bar.insertItem_atIndex(&file_item, 1);
        RECENT_MENU.with(|m| *m.borrow_mut() = Some(recent));
        FILE_MENU.with(|m| *m.borrow_mut() = Some(file));
    }
    MENU_TARGET.with(|t| *t.borrow_mut() = Some(target));
    set_recent_menu(&[], &[]);
}

/// Open… and Open Recent can be chosen (not while Open's sheet is up: a file chosen meanwhile
/// would be replaced by the sheet's).
pub fn set_open_enabled(on: bool) {
    FILE_MENU.with(|m| {
        if let Some(menu) = &*m.borrow() {
            // SAFETY: the menu's items, changed on the main thread (where the menu was made).
            for item in unsafe { menu.itemArray() } {
                unsafe { item.setEnabled(on) };
            }
        }
    });
}

/// Fill Open Recent with `files`, titled `titles` (newest first), then Clear Menu.
pub fn set_recent_menu(files: &[PathBuf], titles: &[String]) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    RECENT_FILES.with(|r| *r.borrow_mut() = files.to_vec());
    let target = MENU_TARGET.with(|t| t.borrow().clone());
    RECENT_MENU.with(|m| {
        let (Some(menu), Some(target)) = (&*m.borrow(), target) else {
            return;
        };
        let item = |title: &NSString, action: Sel| {
            // SAFETY: made on the main thread; the target outlives the item (see above).
            unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    title,
                    Some(action),
                    ns_string!(""),
                );
                item.setTarget(Some(&target));
                item
            }
        };
        // SAFETY: the menu is changed on the main thread.
        unsafe {
            menu.removeAllItems();
            for (i, title) in titles.iter().enumerate().take(files.len()) {
                let it = item(&NSString::from_str(title), sel!(vomdOpenRecent:));
                it.setTag(i as isize);
                menu.addItem(&it);
            }
            if !files.is_empty() {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            let clear = item(ns_string!("Clear Menu"), sel!(vomdClearRecent:));
            clear.setEnabled(!files.is_empty());
            menu.addItem(&clear);
        }
    });
}

/// Give a File menu item's command to [`add_file_menu`]'s receiver.
fn menu_send(command: MenuCommand) {
    MENU.with(|m| {
        if let Some(deliver) = &*m.borrow() {
            deliver(command);
        }
    });
}

/// `-[delegate vomdOpen:]`: File ▸ Open….
unsafe extern "C" fn menu_open(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    menu_send(MenuCommand::Open);
}

/// `-[delegate vomdOpenRecent:]`: an item of Open Recent, its file by its tag.
unsafe extern "C" fn menu_open_recent(_: *mut AnyObject, _: Sel, sender: *mut AnyObject) {
    // SAFETY: AppKit sends the NSMenuItem chosen, alive for the call.
    let Some(item) = (unsafe { sender.cast::<NSMenuItem>().as_ref() }) else {
        return;
    };
    // SAFETY: a plain getter, on the main thread.
    let tag = unsafe { item.tag() };
    let file = RECENT_FILES.with(|r| {
        usize::try_from(tag)
            .ok()
            .and_then(|i| r.borrow().get(i).cloned())
    });
    if let Some(file) = file {
        menu_send(MenuCommand::OpenRecent(file));
    }
}

/// `-[delegate vomdClearRecent:]`: Open Recent ▸ Clear Menu.
unsafe extern "C" fn menu_clear_recent(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    menu_send(MenuCommand::Clear);
}

/// Tell the system `path` was opened, for the Dock icon's menu of recent items (and the count
/// System Settings keeps): Apple's way for an application that isn't NSDocument-based. A click
/// there comes back as `application:openURLs:` ([`on_open_files`]).
pub fn note_recent(path: &Path) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: a URL made of a string, and AppKit's shared controller, on the main thread.
    unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSDocumentController::sharedDocumentController(mtm).noteNewRecentDocumentURL(&url);
    }
}

/// Empty the system's recent items of VOMD (the Dock icon's menu): the list was cleared.
pub fn clear_recent_documents() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: AppKit's shared controller, on the main thread.
    unsafe { NSDocumentController::sharedDocumentController(mtm).clearRecentDocuments(None) };
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

/// Whether this is a default launch, as AppKit said when it finished launching: `Some(true)` for
/// one from the Dock, Launchpad or `open -a VOMD`, `Some(false)` for one to open a file (Finder,
/// `open -a VOMD file.md`), whose file comes as an Apple event after the window is made (with the
/// macOS 26 SDK, after its first frame could be painted), but also for any launch while AppKit
/// has saved state ("Close windows when quitting an app" off; see [`opening_plainly`]). `None`
/// before AppKit said, or if it didn't. Main thread only (`None` elsewhere).
pub fn default_launch() -> Option<bool> {
    DEFAULT_LAUNCH.with(Cell::get)
}

/// Note what NSApplicationDidFinishLaunchingNotification says of the launch, for
/// [`default_launch`]. Call before the event loop runs: the notification comes while it starts,
/// next to winit's `resumed` (before or after it), and before the first redraw is handled.
pub fn watch_launch() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let block = block2::RcBlock::new(|note: NonNull<NSNotification>| {
        // SAFETY: AppKit passes a live notification, on the main thread (no queue is given); the
        // value of NSApplicationLaunchIsDefaultLaunchKey is an NSNumber.
        let default = unsafe {
            note.as_ref()
                .userInfo()
                .and_then(|info| info.objectForKey(NSApplicationLaunchIsDefaultLaunchKey))
                .map(|v| Retained::cast::<NSNumber>(v).boolValue())
        };
        DEFAULT_LAUNCH.with(|d| d.set(default));
    });
    // SAFETY: the name is AppKit's constant. The center copies the block and keeps the observer
    // it returns until it is removed, which it never is (the notification comes once): the
    // returned token is only needed to remove it, so it is dropped.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidFinishLaunchingNotification),
            None,
            None,
            &block,
        );
    }
}

/// The Apple event AppKit is handling is kAEOpenApplication (`aevt`/`oapp`): so it is in a launch
/// from the Dock, Launchpad, `open -a VOMD` or a shell, including one AppKit calls a non-default
/// launch because it has saved state (windows kept at quit). A launch to open a file has none while
/// winit's `resumed` runs (its `odoc` event comes later), which is where this is read. Main thread
/// only (`false` elsewhere).
pub fn opening_plainly() -> bool {
    /// `kCoreEventClass` and `kAEOpenApplication`.
    const AEVT: u32 = u32::from_be_bytes(*b"aevt");
    const OAPP: u32 = u32::from_be_bytes(*b"oapp");
    if MainThreadMarker::new().is_none() {
        return false;
    }
    // SAFETY: called on the main thread; `eventClass` and `eventID` return four-character codes
    // (`AEEventClass` and `AEEventID`, both 32-bit unsigned).
    unsafe {
        NSAppleEventManager::sharedAppleEventManager()
            .currentAppleEvent()
            .is_some_and(|event| {
                let class: u32 = msg_send![&event, eventClass];
                let id: u32 = msg_send![&event, eventID];
                (class, id) == (AEVT, OAPP)
            })
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
