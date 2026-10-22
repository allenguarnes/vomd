//! The Windows halves of `platform.rs`: the console for `--help`, the Open dialog, the shell's
//! default applications and the memory counters. Plain Win32 calls, declared here by hand (as
//! the Linux code declares its libc calls), so no crate is added for them.

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

type Handle = *mut c_void;
type Hwnd = *mut c_void;

#[repr(C)]
struct OpenFileNameW {
    struct_size: u32,
    owner: Hwnd,
    instance: Handle,
    filter: *const u16,
    custom_filter: *mut u16,
    max_cust_filter: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_file_title: u32,
    initial_dir: *const u16,
    title: *const u16,
    flags: u32,
    file_offset: u16,
    file_extension: u16,
    def_ext: *const u16,
    cust_data: isize,
    hook: *mut c_void,
    template_name: *const u16,
    reserved_ptr: *mut c_void,
    reserved: u32,
    flags_ex: u32,
}

#[repr(C)]
struct ShellExecuteInfoW {
    size: u32,
    mask: u32,
    hwnd: Hwnd,
    verb: *const u16,
    file: *const u16,
    parameters: *const u16,
    directory: *const u16,
    show: i32,
    inst_app: Handle,
    id_list: *mut c_void,
    class: *const u16,
    hkey_class: Handle,
    hot_key: u32,
    icon_or_monitor: Handle,
    process: Handle,
}

#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCounters {
    size: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn AttachConsole(process: u32) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn K32GetProcessMemoryInfo(
        process: Handle,
        counters: *mut ProcessMemoryCounters,
        cb: u32,
    ) -> i32;
    fn GetStdHandle(which: u32) -> Handle;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SystemParametersInfoW(action: u32, param: u32, value: *mut c_void, ini: u32) -> i32;
    fn MessageBoxW(owner: Hwnd, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
    fn CommDlgExtendedError() -> u32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    fn CoUninitialize();
}

const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
const SPI_GETWORKAREA: u32 = 0x30;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
const OFN_FILEMUSTEXIST: u32 = 0x1000;
const OFN_PATHMUSTEXIST: u32 = 0x800;
const OFN_EXPLORER: u32 = 0x80000;
const OFN_NOCHANGEDIR: u32 = 0x8;
const OFN_HIDEREADONLY: u32 = 0x4;
const OFN_DONTADDTORECENT: u32 = 0x0200_0000;
const FNERR_BUFFERTOOSMALL: u32 = 0x3003;
const SEE_MASK_NOASYNC: u32 = 0x100;
const SEE_MASK_FLAG_NO_UI: u32 = 0x400;
const SW_SHOWNORMAL: i32 = 1;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const STD_ERROR_HANDLE: u32 = -12i32 as u32;
const MB_ICONERROR: u32 = 0x10;

/// `s` as a NUL-terminated UTF-16 string.
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// COM for the thread a shell call runs on, apartment-threaded as the shell's handlers expect,
/// undone when dropped.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: a plain call; a failure (the thread already set up otherwise) is only noted,
        // so that it isn't undone.
        let hr = unsafe {
            CoInitializeEx(
                std::ptr::null_mut(),
                COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
            )
        };
        Com(hr >= 0)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// See `platform::attach_console`: a program of the GUI subsystem has no console, so `--help`
/// typed in one prints only once this one is attached to it. A no-op when its output goes
/// somewhere already (redirected to a pipe or a file), or when no console started it.
pub fn attach_console() {
    // SAFETY: plain calls. Without a console the handle is null unless it was redirected, and
    // attaching fails harmlessly when there is no parent console.
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// See `platform::fatal`: `message` in a message box if stderr has no handle (nothing would show
/// what was written to it).
pub fn show_if_unseen(message: &str) {
    // SAFETY: a plain call.
    if !unsafe { GetStdHandle(STD_ERROR_HANDLE) }.is_null() {
        return;
    }
    let text = wide(OsStr::new(
        message.strip_prefix("vomd: ").unwrap_or(message),
    ));
    let caption = wide(OsStr::new("VOMD"));
    // SAFETY: NUL-terminated strings that outlive the call; no owner window.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONERROR,
        )
    };
}

/// The window an Open dialog belongs to, from `platform::parent_of`: `win32:<hwnd in hex>`.
fn owner(parent: &str) -> Hwnd {
    parent
        .strip_prefix("win32:")
        .and_then(|h| usize::from_str_radix(h, 16).ok())
        .map_or(std::ptr::null_mut(), |h| h as Hwnd)
}

/// The longest path the Open dialog may return, in UTF-16 units: the long paths Windows allows.
const MAX_PICKED: usize = 32_768;

/// The common Open dialog (the Explorer one), modal to `parent`, starting in `folder`.
pub fn choose_file(
    title: &str,
    parent: &str,
    folder: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let _com = Com::init();
    let globs = super::MARKDOWN_GLOBS.join(";");
    // Pairs of a name and its patterns, each ending in a NUL, the list in a second one.
    let filter: Vec<u16> = format!("Markdown files ({globs})\0{globs}\0All files\0*.*\0\0")
        .encode_utf16()
        .collect();
    let title = wide(OsStr::new(title));
    let dir = folder.map(|f| wide(f.as_os_str()));
    let mut file = vec![0u16; MAX_PICKED];
    let mut ofn = OpenFileNameW {
        struct_size: size_of::<OpenFileNameW>() as u32,
        owner: owner(parent),
        instance: std::ptr::null_mut(),
        filter: filter.as_ptr(),
        custom_filter: std::ptr::null_mut(),
        max_cust_filter: 0,
        filter_index: 1,
        file: file.as_mut_ptr(),
        max_file: file.len() as u32,
        file_title: std::ptr::null_mut(),
        max_file_title: 0,
        initial_dir: dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
        title: title.as_ptr(),
        flags: OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_HIDEREADONLY
            | OFN_DONTADDTORECENT,
        file_offset: 0,
        file_extension: 0,
        def_ext: std::ptr::null(),
        cust_data: 0,
        hook: std::ptr::null_mut(),
        template_name: std::ptr::null(),
        reserved_ptr: std::ptr::null_mut(),
        reserved: 0,
        flags_ex: 0,
    };
    // SAFETY: every pointer in `ofn` points at a buffer that outlives the call, the strings are
    // NUL-terminated, and `max_file` is the length of `file`.
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        // SAFETY: a plain call, about the dialog this thread just ran.
        return match unsafe { CommDlgExtendedError() } {
            0 => Ok(None),
            FNERR_BUFFERTOOSMALL => Err("the chosen file's path is too long".into()),
            code => Err(format!("the file dialog failed (error {code:#x})")),
        };
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    let path = PathBuf::from(OsString::from_wide(&file[..len]));
    Ok(path.is_absolute().then_some(path))
}

/// Hand `target` to the shell's default verb (the default browser for a URL, the default
/// application for a file), returning once the shell has started it (not when the application
/// ends). With `ui`, the shell may ask (which application to use, when none is set); without,
/// that is an error instead.
fn shell_execute(target: &OsStr, ui: bool) -> io::Result<()> {
    let _com = Com::init();
    let file = wide(target);
    let verb = wide(OsStr::new("open"));
    let mut info = ShellExecuteInfoW {
        size: size_of::<ShellExecuteInfoW>() as u32,
        mask: SEE_MASK_NOASYNC | if ui { 0 } else { SEE_MASK_FLAG_NO_UI },
        hwnd: std::ptr::null_mut(),
        verb: verb.as_ptr(),
        file: file.as_ptr(),
        parameters: std::ptr::null(),
        directory: std::ptr::null(),
        show: SW_SHOWNORMAL,
        inst_app: std::ptr::null_mut(),
        id_list: std::ptr::null_mut(),
        class: std::ptr::null(),
        hkey_class: std::ptr::null_mut(),
        hot_key: 0,
        icon_or_monitor: std::ptr::null_mut(),
        process: std::ptr::null_mut(),
    };
    // SAFETY: the strings are NUL-terminated and outlive the call, and `size` is the struct's.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// See `platform::open`: on a thread of its own, since the shell may take a while (it starts the
/// browser or hands it the URL), and the window's thread must not wait for it.
pub fn open(target: &OsStr) -> io::Result<()> {
    let target = target.to_owned();
    std::thread::Builder::new()
        .name("vomd-open".into())
        .spawn(move || {
            if let Err(e) = shell_execute(&target, false)
                && super::stderr_is_tty()
            {
                eprintln!("vomd: cannot open {}: {e}", target.to_string_lossy());
            }
        })
        .map(|_| ())
}

/// See `platform::open_file`: the default application for `path`, as Explorer opens it (with the
/// "how do you want to open this file" dialog where there is none). Blocks until the shell has
/// started it.
pub fn open_file(path: &Path) -> Result<(), String> {
    shell_execute(path.as_os_str(), true).map_err(|e| e.to_string())
}

/// See `platform::work_area`. (VOMD is per-monitor DPI aware, so these are physical pixels.)
pub fn work_area() -> Option<(i32, i32, u32, u32)> {
    let mut r = [0i32; 4];
    // SAFETY: SPI_GETWORKAREA fills a RECT, four i32s.
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, r.as_mut_ptr().cast(), 0) };
    let (w, h) = (
        u32::try_from(r[2] - r[0]).ok()?,
        u32::try_from(r[3] - r[1]).ok()?,
    );
    (ok != 0 && w > 0 && h > 0).then_some((r[0], r[1], w, h))
}

/// `(current, peak)` working set in KB (0 where unknown).
pub fn working_set_kb() -> (u64, u64) {
    let mut c = ProcessMemoryCounters {
        size: size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: `c` is a valid, sized PROCESS_MEMORY_COUNTERS; the pseudo-handle needs no closing.
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.size) } == 0 {
        return (0, 0);
    }
    (
        (c.working_set_size / 1024) as u64,
        (c.peak_working_set_size / 1024) as u64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_parse_from_the_parent_name() {
        assert_eq!(owner("win32:1a2b") as usize, 0x1a2b);
        assert!(owner("").is_null());
        assert!(owner("x11:1a2b").is_null());
        assert!(owner("win32:zz").is_null());
    }

    #[test]
    fn reads_the_working_set() {
        let (now, peak) = working_set_kb();
        assert!(now > 0 && peak >= now);
    }
}
