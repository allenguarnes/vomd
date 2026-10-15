//! Command line, hand-rolled: `vomd [FILE] | - | --help | --version | --no-settings`.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: vomd [OPTIONS] [FILE]

View a Markdown file. With no FILE, opens an empty window; with -, reads stdin.

Options:
  -h, --help         Print this help
  -V, --version      Print the version
      --no-settings  Ignore the settings file
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// No document: the window asks for one.
    None,
    Stdin,
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cli {
    Help,
    Version,
    Run { input: Input, no_settings: bool },
}

/// Parse the arguments after the program name. `--` ends options, so a file named `-x` opens
/// with `vomd -- -x`. Paths are kept as `OsString`s: they need not be UTF-8.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli, String> {
    let mut input = Input::None;
    let mut no_settings = false;
    let mut options = true;
    for arg in args {
        let operand = if options {
            match arg.to_str() {
                Some("-h" | "--help") => return Ok(Cli::Help),
                Some("-V" | "--version") => return Ok(Cli::Version),
                Some("--no-settings") => {
                    no_settings = true;
                    continue;
                }
                Some("--") => {
                    options = false;
                    continue;
                }
                Some("-") => Input::Stdin,
                Some(s) if s.starts_with('-') => return Err(format!("unknown option: {s}")),
                _ => Input::File(arg.into()),
            }
        } else {
            Input::File(arg.into())
        };
        if input != Input::None {
            return Err("only one FILE can be opened per window".into());
        }
        input = operand;
    }
    Ok(Cli::Run { input, no_settings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, String> {
        parse(args.iter().map(OsString::from))
    }

    fn run(input: Input, no_settings: bool) -> Result<Cli, String> {
        Ok(Cli::Run { input, no_settings })
    }

    #[test]
    fn inputs() {
        assert_eq!(p(&[]), run(Input::None, false));
        assert_eq!(p(&["a.md"]), run(Input::File("a.md".into()), false));
        assert_eq!(p(&["-"]), run(Input::Stdin, false));
        assert_eq!(
            p(&["--no-settings", "a.md"]),
            run(Input::File("a.md".into()), true)
        );
        assert_eq!(
            p(&["a.md", "--no-settings"]),
            run(Input::File("a.md".into()), true)
        );
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(p(&["-h"]), Ok(Cli::Help));
        assert_eq!(p(&["a.md", "--help"]), Ok(Cli::Help));
        assert_eq!(p(&["-V"]), Ok(Cli::Version));
        assert_eq!(p(&["--version", "x", "y"]), Ok(Cli::Version));
    }

    #[test]
    fn double_dash_ends_options() {
        assert_eq!(p(&["--", "-x.md"]), run(Input::File("-x.md".into()), false));
        assert_eq!(p(&["--", "-"]), run(Input::File("-".into()), false));
        assert_eq!(
            p(&["--", "--help"]),
            run(Input::File("--help".into()), false)
        );
    }

    #[test]
    fn errors() {
        assert!(p(&["--bogus"]).unwrap_err().contains("--bogus"));
        assert!(p(&["-x"]).is_err());
        assert!(p(&["a.md", "b.md"]).is_err());
        assert!(p(&["-", "a.md"]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![b'a', 0xff, b'.', b'm', b'd']);
        assert_eq!(
            parse([raw.clone()]),
            run(Input::File(PathBuf::from(raw)), false)
        );
    }
}
