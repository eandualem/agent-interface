//! The tmux-workspaces desktop app.
//!
//! There is no window yet: the terminal path is measured first, against the
//! milestone-0 gates in `docs/decisions/0013-milestone-0-benchmark-method.md`.

#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(text) => match io::stdout().lock().write_all(text.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("tmux-workspaces: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            // Paths and unexpected arguments can contain control characters too.
            eprintln!("tmux-workspaces: {}", error.escape_debug());
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<String, String> {
    if arguments.is_empty() {
        return Ok(format!(
            "tmux-workspaces {}: no window yet, see docs/ARCHITECTURE.md",
            env!("CARGO_PKG_VERSION")
        ) + "\n");
    }
    let (mut path, mut disabled, mut format) = (None, false, None);
    let mut args = arguments.into_iter();
    while let Some(argument) = args.next() {
        if argument == "--help" || argument == "-h" {
            return Ok("Usage: tmux-workspaces [--keymap PATH | --no-keymap] --print-keymap <help|toml>\n\nRead and print the effective keymap without opening a window or starting terminals.\n".into());
        }
        if argument == "--no-keymap" {
            disabled = true;
            continue;
        }
        if argument == "--keymap" {
            path = Some(PathBuf::from(
                args.next().ok_or("--keymap requires a path")?,
            ));
            continue;
        }
        if argument == "--print-keymap" {
            format = Some(args.next().ok_or("--print-keymap requires help or toml")?);
            continue;
        }
        // Split native bytes, so --keymap=PATH also preserves non-UTF-8 paths.
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        if let Some(value) = argument.as_bytes().strip_prefix(b"--keymap=") {
            path = Some(PathBuf::from(OsString::from_vec(value.to_vec())));
        } else if let Some(value) = argument.as_bytes().strip_prefix(b"--print-keymap=") {
            format = Some(OsString::from_vec(value.to_vec()));
        } else {
            return Err(format!(
                "unrecognized argument: {}",
                argument.to_string_lossy()
            ));
        }
    }
    let format = format.ok_or("no app window is available yet; use --print-keymap help or toml")?;
    if format != "help" && format != "toml" {
        return Err("--print-keymap requires help or toml".into());
    }
    let map =
        tmux_workspaces_app::keymap::load(path.as_deref(), disabled).map_err(|e| e.to_string())?;
    Ok(if format == "toml" {
        map.to_toml()
    } else {
        map.help()
    })
}
