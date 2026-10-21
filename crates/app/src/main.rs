//! Start the native read-only workspace browser.

#![forbid(unsafe_code)]

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

const USAGE: &str = "Usage: tmux-workspaces [--data-dir PATH]\n\nBrowse saved workspaces and tabs without editing saved layouts or starting terminals.\n";

fn main() -> ExitCode {
    match options(std::env::args_os().skip(1)) {
        Ok(Options::Open(directory)) => {
            if let Err(error) = tmux_workspaces_app::desktop::run(directory) {
                eprintln!("tmux-workspaces: {}", error.escape_debug());
                return ExitCode::FAILURE;
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
    Help,
    Version,
}

fn options(mut arguments: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut directory = None;
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Ok(Options::Help);
        }
        if argument == "--version" || argument == "-V" {
            return Ok(Options::Version);
        }
        if argument == "--data-dir" {
            directory = Some(PathBuf::from(
                arguments.next().ok_or("--data-dir needs a path")?,
            ));
        } else if let Some(path) = argument.as_encoded_bytes().strip_prefix(b"--data-dir=") {
            use std::os::unix::ffi::OsStrExt;
            directory = Some(PathBuf::from(std::ffi::OsStr::from_bytes(path)));
        } else {
            return Err(format!("unknown argument: {}", argument.to_string_lossy()));
        }
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
