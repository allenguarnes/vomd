//! The Debian package and the AppImage, made from the release tarball's staged files (`dist`).
//! Both are assembled with plain tools (`tar`, `xz`, `ar`, `mksquashfs`), no packaging
//! toolchain: the `.deb` builds on any Linux, and the AppImage is the pinned type 2 runtime
//! followed by a SquashFS of the AppDir, which is all `appimagetool` makes.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::root;

/// The libraries VOMD loads at run time (`dlopen`, for Wayland or X11, whichever the session
/// is), so neither `ldd` nor `dpkg-shlibdeps` sees them, as Debian packages. RELR relocations
/// need glibc 2.36 (Debian 12, Ubuntu 23.04).
const DEB_DEPENDS: &str = "libc6 (>= 2.36), libgcc-s1, libwayland-client0, libxkbcommon0, \
                           libx11-6, libx11-xcb1, libxcb1, libxcursor1, libxi6, libxkbcommon-x11-0";
const DEB_RECOMMENDS: &str = "curl, xdg-utils, xdg-desktop-portal-gtk | zenity | kdialog";
const DEB_SUGGESTS: &str = "fonts-noto-cjk, fonts-noto-color-emoji";

/// The AppImage runtime (AppImage/type2-runtime): static, with FUSE 3 built in and
/// `--appimage-extract-and-run` for systems without FUSE. Pinned by release and checksum.
const RUNTIME_URL: &str =
    "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64";
const RUNTIME_SHA256: &str = "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d";

fn run(cmd: &mut Command) -> bool {
    match cmd.status() {
        Ok(s) if s.success() => true,
        Ok(s) => {
            eprintln!("{cmd:?}: {s}");
            false
        }
        Err(e) => {
            eprintln!("{cmd:?}: {e}");
            false
        }
    }
}

/// Seconds since the epoch for every timestamp in the packages: `SOURCE_DATE_EPOCH`, else the
/// last commit's time, so the same commit gives the same files.
pub fn source_date() -> String {
    if let Ok(v) = std::env::var("SOURCE_DATE_EPOCH") {
        return v;
    }
    Command::new("git")
        .current_dir(root())
        .args(["log", "-1", "--format=%ct"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0".into())
}

fn sha256_of(path: &Path) -> Option<String> {
    let out = Command::new("sha256sum").arg(path).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.split_whitespace().next()?.to_owned())
}

/// Writes `<file>.sha256` next to `file`, in `sha256sum`'s format.
pub fn write_sum(file: &Path) -> bool {
    let Some(sum) = sha256_of(file) else {
        eprintln!("sha256sum failed for {}", file.display());
        return false;
    };
    let name = file.file_name().unwrap().to_string_lossy();
    let line = format!("{sum}  {name}\n");
    let out = PathBuf::from(format!("{}.sha256", file.display()));
    fs::write(&out, &line).expect("write the checksum");
    print!("{line}");
    true
}

/// Copies `from` to `to` (creating its directory) with `mode`.
fn put(from: &Path, to: &Path, mode: u32) {
    fs::create_dir_all(to.parent().unwrap()).expect("create a package directory");
    fs::copy(from, to).unwrap_or_else(|e| panic!("copy {}: {e}", from.display()));
    fs::set_permissions(to, fs::Permissions::from_mode(mode)).expect("set the mode");
}

/// Every directory under `dir` 0755 (a umask of 077 must not leak into the packages).
fn open_dirs(dir: &Path) {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).expect("set the mode");
    for e in fs::read_dir(dir)
        .expect("read a package directory")
        .flatten()
    {
        let t = e.file_type().unwrap();
        if t.is_dir() && !t.is_symlink() {
            open_dirs(&e.path());
        }
    }
}

/// Every regular file under `dir`, relative to it, sorted.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir)
            .expect("read a package directory")
            .flatten()
        {
            let t = e.file_type().unwrap();
            if t.is_dir() {
                walk(base, &e.path(), out);
            } else if t.is_file() {
                out.push(e.path().strip_prefix(base).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// A reproducible tar of `dir`'s contents (entries `./…`, owned by root) compressed with xz.
fn tar_xz(dir: &Path, out: &Path, epoch: &str) -> bool {
    run(Command::new("tar")
        .env("XZ_OPT", "-9")
        .args([
            "--format=gnu",
            "--sort=name",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
        ])
        .arg(format!("--mtime=@{epoch}"))
        .arg("-cJf")
        .arg(out)
        .arg("-C")
        .arg(dir)
        .arg("."))
}

/// The Debian copyright file: VOMD's license and those of the fonts and icons built into the
/// binary, in full.
fn copyright(stage: &Path) -> String {
    let read = |n: &str| fs::read_to_string(stage.join(n)).expect("a license file");
    let mut s = String::from(
        "VOMD (View-Only Markdown)\n\
         Source: https://github.com/allenguarnes/vomd\n\n\
         VOMD is under the MIT license:\n\n",
    );
    s.push_str(&read("LICENSE"));
    for (what, file) in [
        (
            "The binary embeds subsets of the Inter font (SIL Open Font License 1.1)",
            "LICENSE-Inter.txt",
        ),
        (
            "The binary embeds a subset of JetBrains Mono (SIL Open Font License 1.1)",
            "LICENSE-JetBrainsMono.txt",
        ),
        (
            "The binary embeds Lucide icons (ISC license)",
            "LICENSE-Lucide.txt",
        ),
    ] {
        s.push_str(&format!("\n\n{what}:\n\n"));
        s.push_str(&read(file));
    }
    s
}

/// Writes `bytes` to `out` compressed with `gzip -9n` (no name or time: Debian policy, and
/// reproducible), mode 0644.
pub fn gzip(bytes: &[u8], out: &Path) -> bool {
    fs::create_dir_all(out.parent().unwrap()).expect("create a package directory");
    let status = Command::new("sh")
        .arg("-c")
        .arg("gzip -9n > \"$1\"")
        .arg("sh")
        .arg(out)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            c.stdin.take().unwrap().write_all(bytes)?;
            c.wait()
        });
    if !status.is_ok_and(|s| s.success()) {
        eprintln!("gzip {} failed", out.display());
        return false;
    }
    fs::set_permissions(out, fs::Permissions::from_mode(0o644)).unwrap();
    true
}

/// The package's Debian changelog: one entry for this revision, pointing at the upstream one.
fn debian_changelog(revision: &str, epoch: &str) -> Option<String> {
    let out = Command::new("date")
        .args(["-u", "-R", "-d"])
        .arg(format!("@{epoch}"))
        .output()
        .ok()?;
    let date = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Some(format!(
        "vomd ({revision}) unstable; urgency=medium\n\n  \
         * Package of the upstream release; its changes are in changelog.gz.\n\n \
         -- Allen Guarnes <allen@ggbite.com>  {date}\n"
    ))
}

/// `dist/vomd_<version>-1_amd64.deb`: the binary, desktop entry, icon, man page, copyright and
/// changelogs. No maintainer scripts: dpkg's triggers update the icon cache, the desktop
/// database and the man database.
pub fn deb(stage: &Path, version: &str, dist: &Path) -> bool {
    let epoch = source_date();
    let revision = format!("{version}-1");
    let work = dist.join("deb");
    let _ = fs::remove_dir_all(&work);
    let data = work.join("data");
    let control = work.join("control");
    let doc = data.join("usr/share/doc/vomd");
    put(&stage.join("vomd"), &data.join("usr/bin/vomd"), 0o755);
    put(
        &stage.join("vomd.desktop"),
        &data.join("usr/share/applications/vomd.desktop"),
        0o644,
    );
    put(
        &stage.join("vomd.svg"),
        &data.join("usr/share/icons/hicolor/scalable/apps/vomd.svg"),
        0o644,
    );
    fs::create_dir_all(&doc).expect("create the doc directory");
    fs::write(doc.join("copyright"), copyright(stage)).expect("write copyright");
    fs::set_permissions(doc.join("copyright"), fs::Permissions::from_mode(0o644)).unwrap();
    let Some(changelog) = debian_changelog(&revision, &epoch) else {
        eprintln!("date failed");
        return false;
    };
    let read = |n: &str| fs::read(stage.join(n)).unwrap_or_else(|e| panic!("{n}: {e}"));
    if !gzip(&read("CHANGELOG.md"), &doc.join("changelog.gz"))
        || !gzip(changelog.as_bytes(), &doc.join("changelog.Debian.gz"))
        || !gzip(&read("vomd.1"), &data.join("usr/share/man/man1/vomd.1.gz"))
    {
        return false;
    }
    open_dirs(&data);

    let files = files_under(&data);
    let bytes: u64 = files
        .iter()
        .map(|f| fs::metadata(data.join(f)).unwrap().len())
        .sum();
    let mut md5 = String::new();
    for f in &files {
        let out = Command::new("md5sum")
            .current_dir(&data)
            .arg(f)
            .output()
            .expect("md5sum");
        md5.push_str(&String::from_utf8_lossy(&out.stdout));
    }
    fs::create_dir_all(&control).unwrap();
    fs::write(
        control.join("control"),
        format!(
            "Package: vomd\n\
             Version: {revision}\n\
             Architecture: amd64\n\
             Maintainer: Allen Guarnes <allen@ggbite.com>\n\
             Installed-Size: {}\n\
             Depends: {DEB_DEPENDS}\n\
             Recommends: {DEB_RECOMMENDS}\n\
             Suggests: {DEB_SUGGESTS}\n\
             Section: text\n\
             Priority: optional\n\
             Homepage: https://github.com/allenguarnes/vomd\n\
             Description: tiny, fast Markdown viewer\n \
             VOMD (View-Only Markdown) opens Markdown documents for reading: CommonMark\n \
             and GitHub Flavored Markdown, light and dark themes following the system,\n \
             live reload, local and (on request) remote images. It never edits files\n \
             and never runs HTML.\n",
            bytes.div_ceil(1024)
        ),
    )
    .unwrap();
    fs::write(control.join("md5sums"), md5).unwrap();
    for f in ["control", "md5sums"] {
        fs::set_permissions(control.join(f), fs::Permissions::from_mode(0o644)).unwrap();
    }
    open_dirs(&control);
    fs::write(work.join("debian-binary"), "2.0\n").unwrap();
    if !tar_xz(&control, &work.join("control.tar.xz"), &epoch)
        || !tar_xz(&data, &work.join("data.tar.xz"), &epoch)
    {
        return false;
    }
    let deb = dist.join(format!("vomd_{revision}_amd64.deb"));
    let _ = fs::remove_file(&deb);
    // `ar` in deterministic mode (zero times and owners); the member order is the format's.
    let ok = run(Command::new("ar")
        .current_dir(&work)
        .arg("rcD")
        .arg(&deb)
        .args(["debian-binary", "control.tar.xz", "data.tar.xz"]));
    let _ = fs::remove_dir_all(&work);
    if !ok {
        return false;
    }
    println!("wrote {}", deb.display());
    write_sum(&deb)
}

/// The pinned runtime, downloaded once into `target/appimage/` (or `VOMD_APPIMAGE_RUNTIME`, a
/// local copy), and checked against its checksum either way.
fn runtime() -> Option<PathBuf> {
    let path = match std::env::var_os("VOMD_APPIMAGE_RUNTIME") {
        Some(p) => PathBuf::from(p),
        None => {
            let dir = root().join("target/appimage");
            let path = dir.join(format!("runtime-x86_64-{}", &RUNTIME_SHA256[..12]));
            if !path.exists() {
                fs::create_dir_all(&dir).ok()?;
                let part = path.with_extension("part");
                if !run(Command::new("curl")
                    .args(["-fsSL", "--retry", "3", "-o"])
                    .arg(&part)
                    .arg(RUNTIME_URL))
                {
                    eprintln!("cannot download the AppImage runtime");
                    return None;
                }
                fs::rename(&part, &path).ok()?;
            }
            path
        }
    };
    match sha256_of(&path) {
        Some(sum) if sum == RUNTIME_SHA256 => Some(path),
        other => {
            eprintln!(
                "{}: sha256 {other:?}, expected {RUNTIME_SHA256}",
                path.display()
            );
            None
        }
    }
}

/// `dist/vomd-<version>-x86_64.AppImage`: the binary, desktop entry and icon in an AppDir, no
/// libraries (the display libraries come from the system, like the other packages).
pub fn appimage(stage: &Path, version: &str, dist: &Path) -> bool {
    let Some(runtime) = runtime() else {
        return false;
    };
    let epoch = source_date();
    let appdir = dist.join("vomd.AppDir");
    let _ = fs::remove_dir_all(&appdir);
    put(&stage.join("vomd"), &appdir.join("usr/bin/vomd"), 0o755);
    put(
        &stage.join("vomd.desktop"),
        &appdir.join("usr/share/applications/vomd.desktop"),
        0o644,
    );
    put(
        &stage.join("vomd.svg"),
        &appdir.join("usr/share/icons/hicolor/scalable/apps/vomd.svg"),
        0o644,
    );
    for f in [
        "LICENSE",
        "LICENSE-Inter.txt",
        "LICENSE-JetBrainsMono.txt",
        "LICENSE-Lucide.txt",
    ] {
        put(
            &stage.join(f),
            &appdir.join("usr/share/licenses/vomd").join(f),
            0o644,
        );
    }
    // The AppImage spec: the desktop entry and icon at the top, `.DirIcon`, and `AppRun`, which
    // can be the binary itself (VOMD needs no environment set up).
    for (link, target) in [
        ("AppRun", "usr/bin/vomd"),
        ("vomd.desktop", "usr/share/applications/vomd.desktop"),
        ("vomd.svg", "usr/share/icons/hicolor/scalable/apps/vomd.svg"),
        (".DirIcon", "vomd.svg"),
    ] {
        symlink(target, appdir.join(link)).expect("symlink in the AppDir");
    }
    open_dirs(&appdir);
    let squashfs = dist.join("vomd.squashfs");
    let _ = fs::remove_file(&squashfs);
    let ok = run(Command::new("mksquashfs")
        .env("SOURCE_DATE_EPOCH", &epoch)
        .arg(&appdir)
        .arg(&squashfs)
        .args([
            "-root-owned",
            "-noappend",
            "-no-xattrs",
            "-comp",
            "zstd",
            "-Xcompression-level",
            "19",
            "-quiet",
            "-no-progress",
        ]));
    let _ = fs::remove_dir_all(&appdir);
    if !ok {
        let _ = fs::remove_file(&squashfs);
        eprintln!("mksquashfs failed (Debian/Ubuntu: squashfs-tools)");
        return false;
    }
    let out = dist.join(format!("vomd-{version}-x86_64.AppImage"));
    let mut image = fs::read(&runtime).expect("read the runtime");
    image.extend(fs::read(&squashfs).expect("read the squashfs"));
    let _ = fs::remove_file(&squashfs);
    fs::write(&out, image).expect("write the AppImage");
    fs::set_permissions(&out, fs::Permissions::from_mode(0o755)).unwrap();
    println!("wrote {}", out.display());
    write_sum(&out)
}
