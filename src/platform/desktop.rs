//! Opening a local file in the default application for its type, through the freedesktop MIME
//! database. `xdg-open` alone can't be trusted with an application that runs in a terminal
//! (`Terminal=true`, e.g. Neovim): outside a desktop it knows (Hyprland, Sway, …) it runs the
//! application in the terminal `xdg-open` was started from, and VOMD has none, so the editor
//! would wait unseen. Such an application is started through `xdg-terminal-exec` instead.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The keys of a desktop file's `[Desktop Entry]` group that opening needs.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub exec: String,
    pub terminal: bool,
}

/// The `[Desktop Entry]` group's `Name`, `Exec` and `Terminal` (other groups, such as actions,
/// and localized keys don't count). `None` without an `Exec`.
pub fn parse_entry(text: &str) -> Option<Entry> {
    let (mut group, mut name, mut exec, mut terminal) = (false, None, None, false);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            group = line == "[Desktop Entry]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| group) else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim().to_owned()),
            "Exec" => exec = Some(value.trim().to_owned()),
            "Terminal" => terminal = value.trim() == "true",
            _ => {}
        }
    }
    Some(Entry {
        name: name.unwrap_or_default(),
        exec: exec?,
        terminal,
    })
}

/// The command line `exec` (a desktop file's `Exec`) gives for `file`, by the Desktop Entry
/// spec: arguments are split on spaces, a quoted one keeps its spaces and takes `\"`, `` \` ``,
/// `\$` and `\\` escapes; `%f`, `%F`, `%u` and `%U` become the file (the first of them; any other
/// is dropped), `%%` is a `%`, and the other field codes are dropped. Without a file code, the
/// file is appended. `None` for an unclosed quote or no program.
pub fn exec_argv(exec: &str, file: &OsStr) -> Option<Vec<OsString>> {
    let mut words: Vec<(String, bool)> = Vec::new();
    let mut chars = exec.chars().peekable();
    loop {
        while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut word = String::new();
        if first == '"' {
            chars.next();
            loop {
                match chars.next()? {
                    '"' => break,
                    '\\' => match chars.next()? {
                        c @ ('"' | '`' | '$' | '\\') => word.push(c),
                        c => {
                            word.push('\\');
                            word.push(c);
                        }
                    },
                    c => word.push(c),
                }
            }
            // Field codes aren't expanded inside quotes.
            words.push((word, true));
        } else {
            while let Some(c) = chars.next_if(|c| *c != ' ' && *c != '\t') {
                word.push(c);
            }
            words.push((word, false));
        }
    }
    // The program comes first, never from a field code.
    if words
        .first()
        .is_none_or(|(w, quoted)| !quoted && w.len() == 2 && w.starts_with('%'))
    {
        return None;
    }
    let mut argv = Vec::new();
    let mut placed = false;
    for (word, quoted) in words {
        if quoted {
            argv.push(OsString::from(word));
            continue;
        }
        match word.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                if !std::mem::replace(&mut placed, true) {
                    argv.push(file.to_owned());
                }
            }
            w if w.len() == 2 && w.starts_with('%') && w != "%%" => {}
            w => argv.push(OsString::from(w.replace("%%", "%"))),
        }
    }
    if !placed {
        argv.push(file.to_owned());
    }
    Some(argv)
}

/// The applications directories, most important first: `$XDG_DATA_HOME` (default
/// `~/.local/share`), then `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`).
fn applications_dirs() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let mut dirs = Vec::new();
    match env("XDG_DATA_HOME") {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = env("HOME") {
                dirs.push(Path::new(&home).join(".local/share"));
            }
        }
    }
    let system = env("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(std::env::split_paths(&system).filter(|p| p.is_absolute()));
    dirs.into_iter().map(|d| d.join("applications")).collect()
}

/// The desktop file for desktop file ID `id` in `dirs` (the first that has it). A `-` in the ID
/// may stand for a subdirectory: `kde-kate.desktop` is also `kde/kate.desktop`.
pub fn find_entry(id: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return None;
    }
    dirs.iter().find_map(|dir| {
        let mut candidates = vec![dir.join(id)];
        for (i, _) in id.match_indices('-') {
            candidates.push(dir.join(id[..i].replace('-', "/")).join(&id[i + 1..]));
        }
        candidates.into_iter().find(|p| p.is_file())
    })
}

/// One line of output from `xdg-mime query <what> <arg>`; `None` if it can't run or says
/// nothing.
fn xdg_mime(what: &str, arg: &OsStr) -> Option<String> {
    let out = Command::new("xdg-mime")
        .args(["query", what])
        .arg(arg)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !line.is_empty()).then_some(line)
}

fn spawn(program: &str, args: &[OsString]) -> io::Result<Child> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// What started the application: `xdg-open`, whose exit status tells whether it could, or a
/// terminal for one that runs in it (its status is the editor's, which says nothing about
/// the opening).
pub enum Opener {
    XdgOpen,
    Terminal,
}

/// Start the default application for the local file `path` (blocks on `xdg-mime`: run it off
/// the main thread). Errors say why in words fit for the window.
pub fn open_file(path: &Path) -> io::Result<(Child, Opener)> {
    let xdg_open = || spawn("xdg-open", &[path.as_os_str().to_owned()]);
    // Without the MIME database (no xdg-mime), `xdg-open` is all there is.
    let Some(mime) = xdg_mime("filetype", path.as_os_str()) else {
        return Ok((xdg_open()?, Opener::XdgOpen));
    };
    let Some(id) = xdg_mime("default", OsStr::new(&mime)) else {
        return Err(io::Error::other(format!(
            "no application is set for {mime}"
        )));
    };
    let entry = find_entry(&id, &applications_dirs())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| parse_entry(&text));
    match entry {
        Some(entry) if entry.terminal => {
            let argv = exec_argv(&entry.exec, path.as_os_str())
                .ok_or_else(|| io::Error::other(format!("{id} has no usable Exec line")))?;
            spawn("xdg-terminal-exec", &argv)
                .map(|child| (child, Opener::Terminal))
                .map_err(|e| {
                    let app = if entry.name.is_empty() {
                        id
                    } else {
                        entry.name
                    };
                    if e.kind() == io::ErrorKind::NotFound {
                        io::Error::other(format!(
                            "{app} runs in a terminal, and xdg-terminal-exec isn't installed"
                        ))
                    } else {
                        e
                    }
                })
        }
        _ => Ok((xdg_open()?, Opener::XdgOpen)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(exec: &str) -> Option<Vec<String>> {
        exec_argv(exec, OsStr::new("/home/a b/settings.ini"))
            .map(|v| v.into_iter().map(|s| s.into_string().unwrap()).collect())
    }

    #[test]
    fn exec_field_codes() {
        let file = "/home/a b/settings.ini";
        assert_eq!(argv("nvim %F").unwrap(), ["nvim", file]);
        assert_eq!(argv("helix").unwrap(), ["helix", file]);
        // Only the first file code takes the file; other codes are dropped.
        assert_eq!(argv("ed %f %U %i %c %k").unwrap(), ["ed", file]);
        assert_eq!(
            argv("vim -c 'set ft=dosini' %u").unwrap(),
            ["vim", "-c", "'set", "ft=dosini'", file]
        );
        assert_eq!(
            argv("tool --pct=50%% %f").unwrap(),
            ["tool", "--pct=50%", file]
        );
        assert_eq!(argv("  nano\t%f  ").unwrap(), ["nano", file]);
    }

    #[test]
    fn exec_quoting() {
        let file = "/home/a b/settings.ini";
        assert_eq!(
            argv(r#""/opt/my editor/bin/ed" --title "a \"b\" \$c \\d" %f"#).unwrap(),
            ["/opt/my editor/bin/ed", "--title", r#"a "b" $c \d"#, file]
        );
        // Field codes stay as they are inside quotes, and the file is still appended.
        assert_eq!(argv(r#"sh "%f""#).unwrap(), ["sh", "%f", file]);
        assert_eq!(argv(r#"ed "unclosed"#), None);
        assert_eq!(argv(""), None);
        assert_eq!(argv("%f"), None);
    }

    #[test]
    fn desktop_entry_group_only() {
        let text = "\
# comment
[Desktop Entry]
Name=Neovim
Name[de]=Neovim (de)
Exec=nvim %F
Terminal=true

[Desktop Action new]
Exec=nvim --new
Terminal=false
";
        assert_eq!(
            parse_entry(text),
            Some(Entry {
                name: "Neovim".into(),
                exec: "nvim %F".into(),
                terminal: true,
            })
        );
        let gui = parse_entry("[Desktop Entry]\nExec=kate %U\nTerminal=false\n").unwrap();
        assert!(!gui.terminal);
        assert_eq!(parse_entry("[Desktop Entry]\nName=x\n"), None);
        assert_eq!(parse_entry("[Other]\nExec=x\n"), None);
    }

    #[test]
    fn finds_entries_by_id() {
        let base = std::env::temp_dir().join(format!("vomd-desktop-{}", std::process::id()));
        let (home, system) = (base.join("home"), base.join("system"));
        std::fs::create_dir_all(home.join("kde")).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(system.join("nvim.desktop"), "").unwrap();
        std::fs::write(home.join("nvim.desktop"), "").unwrap();
        std::fs::write(home.join("kde/kate.desktop"), "").unwrap();
        let dirs = [home.clone(), system.clone()];
        // The first directory that has it wins.
        assert_eq!(
            find_entry("nvim.desktop", &dirs),
            Some(home.join("nvim.desktop"))
        );
        assert_eq!(
            find_entry("kde-kate.desktop", &dirs),
            Some(home.join("kde/kate.desktop"))
        );
        assert_eq!(find_entry("missing.desktop", &dirs), None);
        assert_eq!(find_entry("../nvim.desktop", &dirs), None);
        assert_eq!(find_entry("", &dirs), None);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
