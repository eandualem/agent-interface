//! Browsers that agents drive with [agent-browser] on this machine.
//!
//! agent-browser keeps one daemon per session and writes its files next to
//! the session's socket as `<session>.<kind>`: `pid` (the daemon), `stream`
//! (the port of its frame stream) and `target` (the bound tab, with its
//! address). Reading these files finds the sessions without starting a
//! process or opening a connection, so a machine without agent-browser pays
//! one failed directory read per look. A session named exactly as an agent
//! belongs to that agent; every other session is left out.
//!
//! [agent-browser]: https://github.com/vercel-labs/agent-browser

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

pub mod stream;

/// One running agent-browser session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub name: String,
    /// The session's daemon.
    pub pid: u32,
    /// The port of its frame stream, once the daemon has started it.
    pub port: Option<u16>,
    /// The address of its tab, as agent-browser last recorded it.
    pub address: Option<String>,
}

/// The directory agent-browser keeps its session files in, chosen as
/// agent-browser chooses it without a namespace.
pub fn socket_dir() -> Option<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    set("AGENT_BROWSER_SOCKET_DIR")
        .map(PathBuf::from)
        .or_else(|| set("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("agent-browser")))
        .or_else(|| std::env::home_dir().map(|home| home.join(".agent-browser")))
}

/// The sessions whose daemon is running, by name.
pub fn sessions(dir: &Path) -> Vec<Session> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Session> = entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name();
            let name = file.to_str()?.strip_suffix(".pid")?;
            let read = |kind: &str| fs::read_to_string(dir.join(format!("{name}.{kind}"))).ok();
            let pid = read("pid")?.trim().parse::<u32>().ok()?;
            if !running(pid) {
                return None;
            }
            let port = read("stream").and_then(|port| port.trim().parse().ok());
            let address = read("target")
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|target| Some(target.get("url")?.as_str()?.to_owned()))
                .filter(|url| !url.is_empty());
            Some(Session {
                name: name.to_owned(),
                pid,
                port,
                address,
            })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Whether a process with this id exists. One owned by another user counts.
fn running(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(error) => error == rustix::io::Errno::PERM,
    }
}

/// Whether a session's browser has a window the person can be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// The Chrome process with the window.
    Shown(u32),
    /// The browser runs headless, or is not a Chrome the daemon started.
    None,
}

/// Find the window of the browser that the daemon `pid` started. agent-browser
/// records headless mode only in the daemon's memory, so this reads the
/// command line of the daemon's Chrome child, which carries `--headless`
/// when it has no window.
pub fn window(daemon: u32) -> Window {
    std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,command="])
        .output()
        .ok()
        .map_or(Window::None, |out| {
            window_in(&String::from_utf8_lossy(&out.stdout), daemon)
        })
}

fn window_in(processes: &str, daemon: u32) -> Window {
    processes
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let parent = fields.next()?.parse::<u32>().ok()?;
            let flags: Vec<&str> = fields.collect();
            // agent-browser starts Chrome with a debugging port or pipe.
            (parent == daemon
                && flags
                    .iter()
                    .any(|flag| flag.starts_with("--remote-debugging")))
            .then(|| {
                if flags.iter().any(|flag| flag.starts_with("--headless")) {
                    Window::None
                } else {
                    Window::Shown(pid)
                }
            })
        })
        .unwrap_or(Window::None)
}

/// Bring the windows of the Chrome process `pid` to the front. Only that
/// process is activated: no other Chrome window is touched, and no
/// permission is asked for.
#[cfg(target_os = "macos")]
pub fn bring_forward(pid: u32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    i32::try_from(pid)
        .ok()
        .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
        .is_some_and(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
}

/// Each agent's session: a session named exactly as one of `agents`.
/// Sessions that match no agent are left out.
pub fn by_agent<'a>(
    sessions: &[Session],
    agents: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, Session> {
    agents
        .into_iter()
        .filter_map(|agent| {
            let session = sessions.iter().find(|session| session.name == agent)?;
            Some((agent.to_owned(), session.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> Session {
        Session {
            name: name.into(),
            pid: 1,
            port: Some(9000),
            address: None,
        }
    }

    #[test]
    fn sessions_belong_to_agents_with_their_exact_name() {
        let found = [session("coder"), session("coder-2"), session("scratch")];
        let rows = by_agent(&found, ["coder", "reviewer", "Coder-2"]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows["coder"], found[0]);
    }

    #[test]
    fn only_a_chrome_started_with_a_window_can_be_shown() {
        let processes = "\
  10     1 /usr/local/bin/agent-browser daemon
  11    10 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --user-data-dir=/tmp/a
  20     1 /usr/local/bin/agent-browser daemon
  21    20 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --headless=new
  30     1 /usr/local/bin/agent-browser daemon
";
        assert_eq!(window_in(processes, 10), Window::Shown(11));
        assert_eq!(window_in(processes, 20), Window::None);
        assert_eq!(window_in(processes, 30), Window::None);
    }

    #[test]
    fn running_sessions_are_read_from_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |file: &str, text: &str| fs::write(dir.path().join(file), text).unwrap();
        write("coder.pid", &format!("{}\n", std::process::id()));
        write("coder.stream", "51234");
        write(
            "coder.target",
            r#"{"targetId":"T","url":"https://example.com/","pinned":false}"#,
        );
        // A closed session keeps only its tab binding.
        write(
            "closed.target",
            r#"{"targetId":"T","url":"https://example.com/"}"#,
        );
        // A daemon that has exited leaves no running session.
        write("gone.pid", &i32::MAX.to_string());
        assert_eq!(
            sessions(dir.path()),
            [Session {
                name: "coder".into(),
                pid: std::process::id(),
                port: Some(51234),
                address: Some("https://example.com/".into()),
            }]
        );
        assert!(sessions(&dir.path().join("missing")).is_empty());
    }
}
