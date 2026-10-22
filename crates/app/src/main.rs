//! Start the native workspace and terminal window.

#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

const USAGE: &str = "Usage: tmux-workspaces [--data-dir PATH]\n       tmux-workspaces [--keymap PATH | --no-keymap] --print-keymap <help|toml>\n\nOpen saved workspaces and connect their existing tmux sessions, or print the\neffective keymap without opening a window.\n";

fn main() -> ExitCode {
    match options(std::env::args_os().skip(1)) {
        Ok(Options::Open(directory)) => {
            if let Err(error) = tmux_workspaces_app::desktop::run(directory) {
                eprintln!("tmux-workspaces: {}", error.escape_debug());
                return ExitCode::FAILURE;
            }
        }
        Ok(Options::Keymap {
            path,
            disabled,
            toml,
        }) => {
            let text = match tmux_workspaces_app::keymap::load(path.as_deref(), disabled) {
                Ok(map) if toml => map.to_toml(),
                Ok(map) => map.help(),
                Err(error) => {
                    eprintln!("tmux-workspaces: {}", error.to_string().escape_debug());
                    return ExitCode::FAILURE;
                }
            };
            match io::stdout().lock().write_all(text.as_bytes()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                Err(error) => {
                    eprintln!("tmux-workspaces: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Ok(Options::Help) => print!("{USAGE}"),
        Ok(Options::Version) => println!("tmux-workspaces {}", env!("CARGO_PKG_VERSION")),
        Err(error) => {
            eprintln!("tmux-workspaces: {}\n{USAGE}", error.escape_debug());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

#[derive(Debug, PartialEq, Eq)]
enum Options {
    Open(Option<PathBuf>),
    Keymap {
        path: Option<PathBuf>,
        disabled: bool,
        toml: bool,
    },
    Help,
    Version,
}

fn options(mut arguments: impl Iterator<Item = OsString>) -> Result<Options, String> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let mut directory = None;
    let (mut keymap, mut disabled, mut format) = (None, false, None);
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Ok(Options::Help);
        }
        if argument == "--version" || argument == "-V" {
            return Ok(Options::Version);
        }
        if argument == "--no-keymap" {
            disabled = true;
        } else if argument == "--keymap" {
            keymap = Some(PathBuf::from(
                arguments.next().ok_or("--keymap requires a path")?,
            ));
        } else if argument == "--print-keymap" {
            format = Some(
                arguments
                    .next()
                    .ok_or("--print-keymap requires help or toml")?,
            );
        } else if let Some(value) = argument.as_bytes().strip_prefix(b"--keymap=") {
            // Split native bytes, so --keymap=PATH preserves non-UTF-8 paths.
            keymap = Some(PathBuf::from(OsString::from_vec(value.to_vec())));
        } else if let Some(value) = argument.as_bytes().strip_prefix(b"--print-keymap=") {
            format = Some(OsString::from_vec(value.to_vec()));
        } else if argument == "--data-dir" {
            directory = Some(PathBuf::from(
                arguments.next().ok_or("--data-dir needs a path")?,
            ));
        } else if let Some(path) = argument.as_encoded_bytes().strip_prefix(b"--data-dir=") {
            directory = Some(PathBuf::from(std::ffi::OsStr::from_bytes(path)));
        } else {
            return Err(format!("unknown argument: {}", argument.to_string_lossy()));
        }
    }
    if let Some(format) = format {
        if format != "help" && format != "toml" {
            return Err("--print-keymap requires help or toml".into());
        }
        return Ok(Options::Keymap {
            path: keymap,
            disabled,
            toml: format == "toml",
        });
    }
    if keymap.is_some() || disabled {
        return Err("--keymap and --no-keymap need --print-keymap".into());
    }
    Ok(Options::Open(directory))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_paths_and_diagnostics_do_not_need_a_terminal() {
        let parse = |args: &[&str]| options(args.iter().map(OsString::from));
        assert_eq!(parse(&[]), Ok(Options::Open(None)));
        assert_eq!(parse(&["--help"]), Ok(Options::Help));
        assert_eq!(parse(&["--version"]), Ok(Options::Version));
        for args in [
            vec!["--data-dir", "library with spaces"],
            vec!["--data-dir=library with spaces"],
        ] {
            assert_eq!(
                parse(&args),
                Ok(Options::Open(Some("library with spaces".into())))
            );
        }
        assert!(parse(&["--data-dir"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
    }
}
