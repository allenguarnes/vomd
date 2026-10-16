//! Everything OS-specific. Other modules use only portable std. Non-Linux branches
//! compile but are untested until P14.

/// The system theme when winit can't tell (`Window::theme()` is `None`). On Linux, dark if
/// `GTK_THEME` names a dark variant (`Adwaita:dark`), otherwise light.
pub fn fallback_dark() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        gtk_theme_dark(std::env::var("GTK_THEME").ok().as_deref())
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        // TODO(P14): winit reports the theme on Windows and macOS; nothing to fall back to.
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
    // TODO(P14): AttachConsole(ATTACH_PARENT_PROCESS) on Windows.
}

/// Hand a file or URL to the desktop's default application (no shell; the target is
/// one argument, never part of a command line). Doesn't wait for the application.
pub fn open(target: &std::ffi::OsStr) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    // Defense in depth: callers only pass URLs with a scheme or absolute paths, but a target
    // starting with `-` must never reach a launcher as an option.
    if target.to_string_lossy().starts_with('-') {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    // TODO(P14): untested.
    let mut cmd = Command::new("open");
    #[cfg(windows)]
    // TODO(P14): untested.
    let mut cmd = {
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    let mut child = cmd
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reap it in the background so it doesn't linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
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
    // TODO(P14): other targets (Windows, other Linux architectures) open plainly.
    options.open(path)
}

/// Errors go to stderr only when someone can see them.
pub fn stderr_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}

/// Resident set size now in KB, for `VOMD_BENCH` (0 where unknown).
pub fn rss_kb() -> u64 {
    status_kb("VmRSS:")
}

/// Peak resident set size in KB, for `VOMD_BENCH` (0 where unknown).
pub fn peak_rss_kb() -> u64 {
    status_kb("VmHWM:")
}

/// A `/proc/self/status` field in kB (0 if unknown).
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn status_kb(field: &str) -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix(field))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
            })
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // TODO(P14): GetProcessMemoryInfo / task_info.
        0
    }
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

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_peak_rss() {
        assert!(peak_rss_kb() > 0);
        assert!((1..=peak_rss_kb()).contains(&rss_kb()));
    }
}
