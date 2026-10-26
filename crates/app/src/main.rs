//! Start the native workspace and terminal window.

#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

const USAGE: &str = "Usage: agent-interface [--data-dir PATH]\n       agent-interface [--keymap PATH | --no-keymap] --print-keymap <help|toml>\n       agent-interface [--data-dir PATH] artifact publish FILE.html --title TITLE [--author NAME]\n\nOpen saved workspaces, print the effective keymap, or publish a self-contained\nHTML artifact to the library without opening a window.\n";

fn main() -> ExitCode {
    match options(std::env::args_os().skip(1)) {
        Ok(Options::Open(directory)) => {
            if let Err(error) = agent_interface_app::desktop::run(directory) {
                eprintln!("agent-interface: {}", error.escape_debug());
                return ExitCode::FAILURE;
            }
        }
        Ok(Options::Keymap {
            path,
            disabled,
            toml,
        }) => {
            let text = match agent_interface_app::keymap::load(path.as_deref(), disabled) {
                Ok(map) if toml => map.to_toml(),
                Ok(map) => map.help(),
                Err(error) => {
                    eprintln!("agent-interface: {}", error.to_string().escape_debug());
                    return ExitCode::FAILURE;
                }
            };
            match io::stdout().lock().write_all(text.as_bytes()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                Err(error) => {
                    eprintln!("agent-interface: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Ok(Options::Help) => print!("{USAGE}"),
        Ok(Options::Artifact {
            directory,
            file,
            title,
            author,
        }) => {
            match agent_interface_app::artifacts::publish(
                directory.as_deref(),
                &file,
                &title,
                &author,
            ) {
                Ok(entry) => println!(
                    "{}",
                    serde_json::to_string(&entry).expect("artifact metadata")
                ),
                Err(error) => {
                    eprintln!("agent-interface: {}", error.to_string().escape_debug());
                    return ExitCode::FAILURE;
                }
            }
        }
        Ok(Options::Version) => println!("agent-interface {}", env!("CARGO_PKG_VERSION")),
        Err(error) => {
            eprintln!("agent-interface: {}\n{USAGE}", error.escape_debug());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

#[derive(Debug, PartialEq, Eq)]
enum Options {
    Open(Option<PathBuf>),
    Artifact {
        directory: Option<PathBuf>,
        file: PathBuf,
        title: String,
        author: String,
    },
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
    let (mut artifact, mut title, mut author) = (None, None, None);
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Ok(Options::Help);
        }
        if argument == "--version" || argument == "-V" {
            return Ok(Options::Version);
        }
        if argument == "artifact" && artifact.is_none() {
            if arguments.next().as_deref() != Some(std::ffi::OsStr::new("publish")) {
                return Err("artifact requires publish FILE.html --title TITLE".into());
            }
            artifact = Some(PathBuf::from(
                arguments.next().ok_or("artifact publish requires a file")?,
            ));
        } else if argument == "--title" || argument == "--author" {
            let value = arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or("--title and --author require UTF-8 text")?;
            if argument == "--title" {
                title = Some(value);
            } else {
                author = Some(value);
            }
        } else if argument == "--no-keymap" {
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
    if let Some(file) = artifact {
        if keymap.is_some() || disabled || format.is_some() {
            return Err("artifact publication cannot be combined with keymap options".into());
        }
        return Ok(Options::Artifact {
            directory,
            file,
            title: title.ok_or("artifact publish requires --title")?,
            author: author
                .or_else(|| std::env::var("BACKBONE_AGENT").ok())
                .unwrap_or_else(|| "local".into()),
        });
    }
    if title.is_some() || author.is_some() {
        return Err("--title and --author require artifact publish".into());
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

    #[test]
    fn artifact_publication_keeps_the_library_and_author_explicit() {
        let parse = |args: &[&str]| options(args.iter().map(OsString::from));
        assert_eq!(
            parse(&[
                "--data-dir",
                "library",
                "artifact",
                "publish",
                "report.html",
                "--title",
                "A report",
                "--author",
                "worker",
            ]),
            Ok(Options::Artifact {
                directory: Some("library".into()),
                file: "report.html".into(),
                title: "A report".into(),
                author: "worker".into(),
            })
        );
        assert!(parse(&["artifact", "publish", "report.html"]).is_err());
        assert!(parse(&["--title", "A report"]).is_err());
        assert!(
            parse(&[
                "artifact",
                "publish",
                "report.html",
                "--title",
                "A report",
                "--print-keymap",
                "help",
            ])
            .is_err()
        );
    }
}
