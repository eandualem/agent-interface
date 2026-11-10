//! What first run fixes: Git or tmux with the system's own installer and an
//! agent tool with its official script, each on the person's click; and the
//! agent service, which first run starts by itself: restarted once when it
//! doesn't answer, or set up the way the installer does when it never was.
//! Every fix runs off the UI thread with a deadline, and a failure explains
//! itself.

use std::{
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Long enough for a package manager or an installer script on a slow line.
const LIMIT: Duration = Duration::from_secs(15 * 60);
/// How much of a failed command's output is kept to explain it.
const TAIL: usize = 16 * 1024;
/// Agent Backbone's own setup waits up to 30 s for its service; this leaves
/// room for a slow first start.
const SETUP_LIMIT: Duration = Duration::from_secs(5 * 60);

/// How a fix ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It finished; check the computer again.
    Done,
    /// The system's own installer opened and finishes there; keep checking.
    Opened,
    /// It could not be done, and why, in a sentence.
    Failed(String),
}

/// Find a command in the PATH setup found, or in the usual system places.
fn find(command: &str, path_env: &str) -> Option<PathBuf> {
    std::env::split_paths(path_env)
        .chain(
            [
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/opt/homebrew/bin",
                "/usr/local/bin",
            ]
            .map(PathBuf::from),
        )
        .map(|directory| directory.join(command))
        .find(|path| path.is_file())
}

/// Install Git or tmux, by the name both the command and its package share.
pub fn package(name: &'static str, path_env: &str) -> Outcome {
    if cfg!(target_os = "macos") {
        if name == "git" {
            // Apple's own installer for the command line tools, which include Git.
            return match run(Command::new("/usr/bin/xcode-select").arg("--install"), path_env) {
                Ok(()) => Outcome::Opened,
                Err(_) => Outcome::Failed(
                    "The command line tools installer could not open. Open Software Update, then try again.".into(),
                ),
            };
        }
        let Some(brew) = find("brew", path_env) else {
            return Outcome::Failed(format!(
                "{name} installs with Homebrew, which isn't on this Mac yet. Install Homebrew from brew.sh, then try again."
            ));
        };
        return finished(run(Command::new(brew).args(["install", name]), path_env));
    }
    // Linux: the distribution's package manager, behind the desktop's own
    // password prompt. Without that prompt there is no safe way to ask.
    let managers: [(&str, &[&str]); 4] = [
        ("apt-get", &["install", "-y"]),
        ("dnf", &["install", "-y"]),
        ("pacman", &["-S", "--noconfirm"]),
        ("zypper", &["--non-interactive", "install"]),
    ];
    let Some((manager, arguments)) = managers
        .iter()
        .find_map(|(manager, arguments)| find(manager, path_env).map(|path| (path, *arguments)))
    else {
        return Outcome::Failed(format!(
            "This system's package manager isn't one setup knows. Install {name} with it, and setup notices."
        ));
    };
    let Some(prompt) = find("pkexec", path_env) else {
        return Outcome::Failed(format!(
            "Installing {name} needs an administrator password, and this desktop has no prompt for it. Install {name} from a terminal, and setup notices."
        ));
    };
    let mut command = Command::new(prompt);
    command.arg(manager).args(arguments).arg(name);
    finished(run(&mut command, path_env))
}

/// Install an agent tool with its official command.
pub fn tool(install: &str, path_env: &str) -> Outcome {
    finished(run(Command::new("/bin/sh").args(["-c", install]), path_env))
}

/// Restart Agent Backbone's service the way the installer starts it.
pub fn restart_backbone(path_env: &str) -> Outcome {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Outcome::Failed("The home folder is unknown.".into());
    };
    if cfg!(target_os = "macos") {
        let plist = home.join("Library/LaunchAgents/dev.agent-backbone.plist");
        if !plist.is_file() {
            return Outcome::Failed(not_installed());
        }
        if let Err(reason) = owned(&plist, &home) {
            return Outcome::Failed(reason);
        }
        let domain = format!("gui/{}", uzers::get_current_uid());
        // Stopping fails when it was not running; starting is what counts.
        let _ = run(
            Command::new("/bin/launchctl")
                .args(["bootout", &domain])
                .arg(&plist),
            path_env,
        );
        return finished(run(
            Command::new("/bin/launchctl")
                .args(["bootstrap", &domain])
                .arg(&plist),
            path_env,
        ));
    }
    let unit = home.join(".config/systemd/user/agent-backbone.service");
    if !unit.is_file() {
        return Outcome::Failed(not_installed());
    }
    if let Err(reason) = owned(&unit, &home) {
        return Outcome::Failed(reason);
    }
    let Some(systemctl) = find("systemctl", path_env) else {
        return Outcome::Failed("This system has no service manager setup can use.".into());
    };
    finished(run(
        Command::new(systemctl).args(["--user", "restart", "agent-backbone.service"]),
        path_env,
    ))
}

/// Set Agent Backbone up the way the installer does, when it never was: its
/// data, its Default profile and its login service. Safe to repeat; existing
/// data is kept.
pub fn initialize_backbone(backbone: &Path, path_env: &str) -> Result<(), String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("The home folder is unknown.")?;
    let data = data_dir(&home);
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let request = serde_json::json!({
        "schema_version": 1,
        "request_id": request_id,
        "action": "initialize",
        "data_dir": data,
        "service": "user",
        "starter_profile": "core-v1",
    });
    let mut file = tempfile::NamedTempFile::new()
        .map_err(|_| "Setup could not create a temporary file.".to_owned())?;
    std::io::Write::write_all(&mut file, request.to_string().as_bytes())
        .map_err(|_| "Setup could not write its request.".to_owned())?;
    let output =
        tempfile::tempfile().map_err(|_| "Setup could not create a temporary file.".to_owned())?;
    let mut child = Command::new(backbone)
        .args(["setup", "--file"])
        .arg(file.path())
        .arg("--jsonl")
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(
            output
                .try_clone()
                .map_err(|_| "Setup could not create a temporary file.".to_owned())?,
        )
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Agent Backbone's setup could not start: {error}."))?;
    let deadline = Instant::now() + SETUP_LIMIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Agent Backbone's setup took too long and was stopped.".into());
            }
        }
    }
    let result = last_line(&output);
    let event: serde_json::Value = serde_json::from_str(&result).unwrap_or_default();
    match (event["type"].as_str(), event["status"].as_str()) {
        (Some("result"), Some("completed")) => Ok(()),
        (Some("result"), _) => Err(format!(
            "Agent Backbone's setup stopped: {}",
            event["message"]
                .as_str()
                .or(event["code"].as_str())
                .unwrap_or("no reason given")
        )),
        _ => Err("Agent Backbone's setup did not report a result.".into()),
    }
}

/// The data folder this app's Agent Backbone uses.
fn data_dir(home: &Path) -> PathBuf {
    std::env::var_os("BACKBONE_DATA_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/agent-backbone"))
}

/// Whether the installed service serves this app's data folder. Its
/// definition names the folder in `BACKBONE_DATA_DIR`, or uses the default
/// when it doesn't. A service for another folder is left alone.
fn owned(definition: &Path, home: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(definition)
        .map_err(|_| "The agent service's definition could not be read.".to_owned())?;
    let named = text
        .lines()
        .map(str::trim)
        .enumerate()
        .find_map(|(index, line)| {
            if let Some(rest) = line
                .trim_start_matches("Environment=")
                .trim_matches('"')
                .strip_prefix("BACKBONE_DATA_DIR=")
            {
                return Some(rest.trim_matches('"').to_owned());
            }
            // A launchd plist: the key, then its string on the next line.
            (line == "<key>BACKBONE_DATA_DIR</key>").then(|| {
                text.lines()
                    .nth(index + 1)
                    .unwrap_or_default()
                    .trim()
                    .trim_start_matches("<string>")
                    .trim_end_matches("</string>")
                    .to_owned()
            })
        });
    let expected = data_dir(home);
    let served = named.map_or_else(|| home.join(".local/share/agent-backbone"), PathBuf::from);
    if served == expected {
        Ok(())
    } else {
        Err(format!(
            "The installed agent service serves {}, not {}, so it was left alone.",
            served.display(),
            expected.display()
        ))
    }
}

fn not_installed() -> String {
    "Agent Backbone's service isn't installed. Run the Agent Interface installer again to add it."
        .into()
}

fn finished(result: Result<(), String>) -> Outcome {
    match result {
        Ok(()) => Outcome::Done,
        Err(reason) => Outcome::Failed(reason),
    }
}

/// Run a command to completion with setup's PATH. Output goes to a file, as
/// in the probe, so a background process cannot hold a pipe open.
fn run(command: &mut Command, path_env: &str) -> Result<(), String> {
    let file =
        tempfile::tempfile().map_err(|_| "Setup could not create a temporary file.".to_owned())?;
    let output = |file: &std::fs::File| -> Result<Stdio, String> {
        file.try_clone()
            .map(Stdio::from)
            .map_err(|_| "Setup could not create a temporary file.".to_owned())
    };
    let mut child = command
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(output(&file)?)
        .stderr(output(&file)?)
        .spawn()
        .map_err(|error| format!("It could not start: {error}."))?;
    let deadline = Instant::now() + LIMIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    match status {
        Some(status) if status.success() => Ok(()),
        Some(_) => Err(explain(&last_line(&file))),
        None => Err("It took too long and was stopped. Try again.".into()),
    }
}

/// The last meaningful line a failed command printed.
fn last_line(file: &std::fs::File) -> String {
    let length = file.metadata().map_or(0, |info| info.len());
    let start = length.saturating_sub(TAIL as u64);
    let mut bytes = vec![0; (length - start) as usize];
    let read = file.read_at(&mut bytes, start).unwrap_or(0);
    bytes.truncate(read);
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(TAIL).collect())
        .unwrap_or_default()
}

/// A failed command's explanation, in a sentence the window can show.
fn explain(line: &str) -> String {
    if line.is_empty() {
        return "It stopped with an error. Try again.".into();
    }
    if line.contains("npm: not found") || line.contains("npm: command not found") {
        return "It needs Node.js 20 or newer with npm. Install Node.js, then try again.".into();
    }
    let line: String = line.chars().take(200).collect();
    format!("It stopped with an error: {line}")
}

/// Shorten a path under the home folder to `~`.
pub fn home_relative(path: &Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path() -> (tempfile::TempDir, String) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().display().to_string();
        (directory, format!("{path}:/usr/bin:/bin"))
    }

    #[test]
    fn a_tool_installer_that_succeeds_is_done() {
        let (_directory, path) = temporary_path();
        assert_eq!(tool("printf installed", &path), Outcome::Done);
    }

    #[test]
    fn a_failing_installer_explains_itself_with_its_last_line() {
        let (_directory, path) = temporary_path();
        let outcome = tool(
            "printf 'downloading\\nno space left on device\\n' >&2; exit 3",
            &path,
        );
        assert_eq!(
            outcome,
            Outcome::Failed("It stopped with an error: no space left on device".into())
        );
        let silent = tool("exit 1", &path);
        assert_eq!(
            silent,
            Outcome::Failed("It stopped with an error. Try again.".into())
        );
    }

    #[test]
    fn a_missing_npm_says_what_to_install() {
        let (_directory, path) = temporary_path();
        let outcome = tool("echo 'sh: 1: npm: not found' >&2; exit 127", &path);
        assert_eq!(
            outcome,
            Outcome::Failed(
                "It needs Node.js 20 or newer with npm. Install Node.js, then try again.".into()
            )
        );
    }

    #[test]
    fn installers_see_the_path_setup_found() {
        let (directory, path) = temporary_path();
        let marker = directory.path().join("seen");
        let script = directory.path().join("fixture-tool");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(tool("fixture-tool", &path), Outcome::Done);
        assert!(marker.is_file());
    }
}
