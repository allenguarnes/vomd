//! Everything OS-specific. Other modules use only portable std. Linux and Windows are
//! supported; the macOS branches compile but are untested.

/// The system theme when winit can't tell (`Window::theme()` is `None`). On Linux, dark if
/// `GTK_THEME` names a dark variant (`Adwaita:dark`), otherwise light.
pub fn fallback_dark() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        gtk_theme_dark(std::env::var("GTK_THEME").ok().as_deref())
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        // winit reports the theme on Windows and macOS: nothing to fall back to.
        false
    }
}

/// `GTK_THEME` is `name[:variant]`; the variant decides.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn gtk_theme_dark(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().ends_with(":dark"))
}

/// Let `--help` and `--version` print when started from a terminal. A no-op except on Windows,
/// where the GUI subsystem detaches the console.
pub fn attach_console() {
    #[cfg(windows)]
    windows::attach_console();
}

#[cfg(all(unix, not(target_os = "macos")))]
mod desktop;
#[cfg(windows)]
mod windows;

/// The file name patterns Open offers as Markdown (the first filter of its dialog).
pub const MARKDOWN_GLOBS: [&str; 5] = ["*.md", "*.markdown", "*.mdown", "*.mkd", "*.mkdn"];

/// The title of Open's dialog.
const OPEN_TITLE: &str = "Open a Markdown file";

/// Ask the user for a file to open, starting in `folder`: on Linux the desktop portal's file
/// chooser, else `zenity` or `kdialog`; on Windows the common Open dialog. `parent` names the
/// window the dialog belongs to (`x11:<id>` for the portal, `win32:<hwnd>` on Windows; empty for
/// none). Blocks until the dialog closes: run it off the main thread.
/// `Ok(None)` when the user picked nothing; `Err` says, in words for the window, why there was
/// no dialog.
pub fn choose_file(
    parent: &str,
    folder: Option<&std::path::Path>,
) -> Result<Option<std::path::PathBuf>, String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use crate::portal::Choice;
        match crate::portal::choose_file(OPEN_TITLE, parent, folder) {
            Choice::Picked(path) => Ok(Some(path)),
            Choice::Cancelled => Ok(None),
            Choice::Failed(why) => Err(why),
            Choice::Unavailable => desktop::choose_with_tools(OPEN_TITLE, folder),
        }
    }
    #[cfg(windows)]
    {
        windows::choose_file(OPEN_TITLE, parent, folder)
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = (parent, folder, OPEN_TITLE);
        Err("Open has no file dialog on this system yet".into())
    }
}

/// Start the default application for the local file `path`, the way the desktop would (on
/// Linux also when it runs in a terminal), and wait for the launcher: it can block on
/// `xdg-mime` or the shell, so call it off the main thread. Errors say why in words fit for the
/// window: no application, no way to start it, or the launcher giving up.
pub fn open_file(path: &std::path::Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput).to_string());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (mut child, opener) = desktop::open_file(path).map_err(|e| e.to_string())?;
        // A terminal's exit status is the editor's, so it isn't judged.
        match child.wait() {
            Ok(status) if !status.success() && matches!(opener, desktop::Opener::XdgOpen) => {
                Err(format!("xdg-open failed ({status})"))
            }
            _ => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        windows::open_file(path)
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = launcher(path.as_os_str())
            .and_then(|mut c| c.wait())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Hand a file or URL to the desktop's default application (no shell; the target is
/// one argument, never part of a command line). Doesn't wait for the application.
pub fn open(target: &std::ffi::OsStr) -> std::io::Result<()> {
    // Defense in depth: callers only pass URLs with a scheme or absolute paths, but a target
    // starting with `-` must never reach a launcher as an option.
    if target.to_string_lossy().starts_with('-') {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    #[cfg(windows)]
    {
        windows::open(target)
    }
    #[cfg(not(windows))]
    {
        let mut child = launcher(target)?;
        // Reap it in the background so it doesn't linger as a zombie.
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

/// The platform's opener started on `target`, not waited for.
#[cfg(not(windows))]
fn launcher(target: &std::ffi::OsStr) -> std::io::Result<std::process::Child> {
    use std::process::{Command, Stdio};
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    // Untested.
    let mut cmd = Command::new("open");
    cmd.arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// The desktop's work area, the primary monitor less the taskbar, in physical pixels: x, y,
/// width and height. Windows only: elsewhere `None` (the window system places windows itself).
pub fn work_area() -> Option<(i32, i32, u32, u32)> {
    #[cfg(windows)]
    return windows::work_area();
    #[cfg(not(windows))]
    None
}

/// Where a window of outer size `size` at `pos` goes to be inside `area` (all physical pixels),
/// and the outer size it may have: no bigger than the area, moved in from an edge it crosses,
/// and centered vertically if it reached past the bottom (where the taskbar is).
pub fn fit_in(
    (ax, ay, aw, ah): (i32, i32, u32, u32),
    (x, y): (i32, i32),
    (w, h): (u32, u32),
) -> ((i32, i32), (u32, u32)) {
    let (w, h) = (w.min(aw), h.min(ah));
    let right = ax.saturating_add_unsigned(aw - w);
    let bottom = ay.saturating_add_unsigned(ah - h);
    let x = x.clamp(ax, right);
    let y = if y > bottom {
        ay.saturating_add_unsigned((ah - h) / 2)
    } else {
        y.max(ay)
    };
    ((x, y), (w, h))
}

/// Open `path` for reading without ever waiting for a writer: where the platform's `O_NONBLOCK`
/// is known (Linux on the architectures below, macOS, iOS and the BSDs) a FIFO opens at once
/// instead of blocking until something writes to it, and reading a regular file is unaffected.
/// Elsewhere this is a plain blocking open, so a caller that must not block on a FIFO has to
/// refuse it beforehand (`view::read_input` looks at `metadata(path)` first). The caller also
/// inspects the opened file (`File::metadata`), which is what makes the check immune to the
/// path changing after it was looked at, on the targets with the flag.
pub fn open_nonblocking(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(all(
        target_os = "linux",
        any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "powerpc64",
            target_arch = "s390x",
            target_arch = "loongarch64"
        )
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK is 0o4000 on these architectures (not on mips, sparc, alpha, parisc).
        options.custom_flags(0o4000);
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK is 0x0004 on Apple and every BSD.
        options.custom_flags(0x0004);
    }
    // Other targets open plainly. Windows has no FIFOs among its files (its named pipes are
    // `\\.\pipe\` paths, which aren't local files to VOMD), so nothing there can block.
    options.open(path)
}

/// Keeps glibc's malloc from raising its mmap threshold (review round 3): by default the
/// threshold goes up to the size of every mapped block freed (as far as 32 MB), after which blocks
/// of a few MB (fetched image bodies, decoded rows, a file read whole) come out of the heap, are
/// grown by copying (the old block and the new one at once) and, freed, leave holes the heap keeps:
/// 50 remote images of 4 MB held 41 MB of live allocations and 67 MB of resident memory. Pinned at
/// 256 KB, each of them is a mapping of its own, grown in place (`mremap`) and given back to the
/// system when freed, so resident memory follows the live allocations the image budget counts. A
/// no-op where the allocator isn't glibc's. Call it first, before any thread is started.
pub fn tune_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn mallopt(param: i32, value: i32) -> i32;
        }
        const M_MMAP_THRESHOLD: i32 = -3;
        // SAFETY: a libc function of plain integers, called before other threads start.
        unsafe {
            mallopt(M_MMAP_THRESHOLD, 256 << 10);
        }
    }
}

/// Say why VOMD can't go on: on stderr, and on Windows also in a message box when stderr goes
/// nowhere (no console was there to attach to and it isn't redirected), as for a program started
/// from Explorer or a shortcut, which would otherwise end without a word.
pub fn fatal(message: &str) {
    eprintln!("{message}");
    #[cfg(windows)]
    windows::show_if_unseen(message);
}

/// Errors go to stderr only when someone can see them.
pub fn stderr_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}

/// Resident set size now in KB, for `VOMD_BENCH` (0 where unknown). On Windows, the working
/// set.
pub fn rss_kb() -> u64 {
    #[cfg(windows)]
    return windows::working_set_kb().0;
    #[cfg(not(windows))]
    status_kb("VmRSS:")
}

/// Peak resident set size in KB, for `VOMD_BENCH` (0 where unknown). On Windows, the peak
/// working set.
pub fn peak_rss_kb() -> u64 {
    #[cfg(windows)]
    return windows::working_set_kb().1;
    #[cfg(not(windows))]
    status_kb("VmHWM:")
}

/// A `/proc/self/status` field in kB (0 if unknown).
#[cfg(not(windows))]
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn status_kb(field: &str) -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status").map_or(0, |s| status_field(&s, field))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Untested: task_info on macOS.
        0
    }
}

/// A field of a `/proc/self/status` read, in kB (0 if missing).
#[cfg(target_os = "linux")]
fn status_field(status: &str, field: &str) -> u64 {
    status
        .lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_refuses_option_like_targets() {
        let err = open(std::ffi::OsStr::new("--help")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(open(std::ffi::OsStr::new("-x")).is_err());
    }

    #[test]
    fn windows_fit_in_the_work_area() {
        let area = (0, 0, 1600, 950);
        // Inside already: left alone.
        assert_eq!(fit_in(area, (100, 50), (900, 800)), ((100, 50), (900, 800)));
        // Past the bottom: centered vertically, as tall as it was.
        assert_eq!(
            fit_in(area, (137, 130), (900, 900)),
            ((137, 25), (900, 900))
        );
        // Taller than the area: as tall as the area, at its top.
        assert_eq!(
            fit_in(area, (137, 130), (900, 1100)),
            ((137, 0), (900, 950))
        );
        // Past the right edge and above the top: moved in.
        assert_eq!(
            fit_in(area, (1200, -20), (900, 600)),
            ((700, 0), (900, 600))
        );
        // An area that doesn't start at the origin (a taskbar on the left or top).
        assert_eq!(
            fit_in((60, 40, 1000, 700), (0, 0), (500, 500)),
            ((60, 40), (500, 500))
        );
    }

    #[test]
    fn gtk_theme_variants() {
        assert!(gtk_theme_dark(Some("Adwaita:dark")));
        assert!(gtk_theme_dark(Some("Breeze:dark\n")));
        assert!(!gtk_theme_dark(Some("Adwaita")));
        assert!(!gtk_theme_dark(Some("Adwaita:light")));
        assert!(!gtk_theme_dark(Some("dark")));
        assert!(!gtk_theme_dark(None));
    }

    /// The open must return at once on a FIFO with no writer, and a regular file reads normally.
    #[cfg(all(
        target_os = "linux",
        any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "powerpc64",
            target_arch = "s390x",
            target_arch = "loongarch64"
        )
    ))]
    #[test]
    fn open_nonblocking_does_not_wait_for_a_writer() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("vomd-platform-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            let (tx, rx) = std::sync::mpsc::channel();
            let path = fifo.clone();
            std::thread::spawn(move || {
                let file = open_nonblocking(&path);
                let _ = tx.send(file.map(|f| f.metadata().map(|m| m.is_file())));
            });
            let regular = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("opening a FIFO blocked")
                .unwrap()
                .unwrap();
            assert!(!regular);
        }
        let file = dir.join("a.md");
        std::fs::write(&file, "# hi").unwrap();
        let mut text = String::new();
        let mut f = open_nonblocking(&file).unwrap();
        assert!(f.metadata().unwrap().is_file());
        f.read_to_string(&mut text).unwrap();
        assert_eq!(text, "# hi");
        assert!(open_nonblocking(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn reads_peak_rss() {
        assert!(rss_kb() > 0);
        assert!(peak_rss_kb() > 0);
        // Both from one reading. Read apart, the peak can be less than the size read before it:
        // Linux's VmHWM is the larger of the size now and a mark it updates only now and then,
        // so memory that tests running beside this one free in between can take both down.
        #[cfg(target_os = "linux")]
        let (now, peak) = {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            (
                status_field(&status, "VmRSS:"),
                status_field(&status, "VmHWM:"),
            )
        };
        #[cfg(windows)]
        let (now, peak) = windows::working_set_kb();
        assert!((1..=peak).contains(&now));
    }
}
