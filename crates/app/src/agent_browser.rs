//! Browsers that agents drive with [agent-browser] on this machine.
//!
//! agent-browser keeps one daemon per session and writes its files next to
//! the session's socket as `<session>.<kind>`: `pid` (the daemon) and
//! `target` (the bound tab, with its id and address). Reading these files
//! finds the sessions without starting a process or opening a connection, so
//! a machine without agent-browser pays one failed directory read per look.
//! A session named exactly as an agent belongs to that agent; every other
//! session is left out.
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
    /// The DevTools id of its tab, once it has one.
    pub target: Option<String>,
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
            let tab = read("target")
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
            let field = |key: &str| {
                tab.as_ref()
                    .and_then(|tab| Some(tab.get(key)?.as_str()?.to_owned()))
                    .filter(|text| !text.is_empty())
            };
            Some(Session {
                name: name.to_owned(),
                pid,
                target: field("targetId"),
                address: field("url"),
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
    /// The daemon's Chrome runs headless, with a profile folder of its own
    /// or, as far as can be told, the person's: none given, or Chrome's
    /// default folder.
    Headless { own_profile: bool },
    /// The daemon started no Chrome, as when it attached to a running
    /// browser or a provider's, so its window cannot be found.
    NotStarted,
}

/// Find the window of the browser that the daemon `pid` started. agent-browser
/// records headless mode only in the daemon's memory, so this reads the
/// command line of the daemon's Chrome child, which carries `--headless`
/// when it has no window.
pub fn window(daemon: u32) -> Window {
    let person = std::env::home_dir().map(|home| home.join(CHROME_PROFILE));
    window_in(&processes(), daemon, person.as_deref())
}

/// The person's Chrome profile folder on macOS, under their home folder.
const CHROME_PROFILE: &str = "Library/Application Support/Google/Chrome";

/// Every process, as `pid ppid command` lines.
fn processes() -> String {
    std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,command="])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// The Chrome that the daemon started, as its process id and command line.
fn chrome_in(processes: &str, daemon: u32) -> Option<(u32, &str)> {
    processes.lines().find_map(|line| {
        let line = line.trim_start();
        let (pid, rest) = line.split_once(char::is_whitespace)?;
        let rest = rest.trim_start();
        let (parent, command) = rest.split_once(char::is_whitespace)?;
        // agent-browser starts Chrome with a debugging port or pipe.
        (parent.parse::<u32>().ok()? == daemon && command.contains(" --remote-debugging"))
            .then_some((pid.parse().ok()?, command))
    })
}

/// The window of the daemon's Chrome; `person` is the person's own Chrome
/// profile folder, without which no profile counts as the agent's own.
fn window_in(processes: &str, daemon: u32, person: Option<&Path>) -> Window {
    // Symbolic links and relative parts can't make one folder look like two.
    let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    match chrome_in(processes, daemon) {
        Some((_, command)) if command.contains(" --headless") => Window::Headless {
            own_profile: profile_in(processes, daemon)
                .is_some_and(|profile| person.is_some_and(|person| real(&profile) != real(person))),
        },
        Some((pid, _)) => Window::Shown(pid),
        None => Window::NotStarted,
    }
}

/// The DevTools port of the Chrome that the daemon `daemon` started, from
/// the `DevToolsActivePort` file Chrome writes in its own profile.
pub fn devtools_port(daemon: u32) -> Option<u16> {
    port_in(&profile_in(&processes(), daemon)?)
}

/// The port in the `DevToolsActivePort` file of the Chrome profile `profile`.
fn port_in(profile: &Path) -> Option<u16> {
    let written = fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    written.lines().next()?.trim().parse().ok()
}

/// The profile folder of the daemon's Chrome, from its `--user-data-dir`.
fn profile_in(processes: &str, daemon: u32) -> Option<PathBuf> {
    let (_, command) = chrome_in(processes, daemon)?;
    let (_, rest) = command.split_once(" --user-data-dir=")?;
    // The folder ends where the next flag begins.
    let folder = rest.split(" --").next()?.trim();
    (!folder.is_empty()).then(|| PathBuf::from(folder))
}

/// Bring the windows of the Chrome process `pid` to the front. Only that
/// process is activated: no other Chrome window is touched, and no
/// permission is asked for. Called on the main thread.
#[cfg(target_os = "macos")]
pub fn bring_forward(pid: u32) -> bool {
    use objc2::{MainThreadMarker, runtime::NSObjectProtocol, sel};
    use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSRunningApplication};
    let Some(chrome) = i32::try_from(pid)
        .ok()
        .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
    else {
        return false;
    };
    let options = NSApplicationActivationOptions::empty();
    // Since macOS 14 activation is cooperative: the active app yields to the
    // app it activates.
    if let Some(main) = MainThreadMarker::new()
        && chrome.respondsToSelector(sel!(activateFromApplication:options:))
    {
        NSApplication::sharedApplication(main).yieldActivationToApplication(&chrome);
        return chrome
            .activateFromApplication_options(&NSRunningApplication::currentApplication(), options);
    }
    chrome.activateWithOptions(options)
}

/// `address` if it is a web page's, with a host: Open in Chrome opens
/// nothing else.
pub fn web_address(address: &str) -> Option<&str> {
    let (scheme, rest) = address.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let web = ["http", "https"]
        .iter()
        .any(|web| scheme.eq_ignore_ascii_case(web));
    (web && !host.is_empty() && !host.starts_with(':')).then_some(address)
}

/// Open the web page `address` in the person's Chrome, or in their default
/// browser when Chrome cannot be opened. Nothing but a web address is
/// opened, and it goes to `open` as an argument, never through a shell.
#[cfg(target_os = "macos")]
pub fn open_in_chrome(address: &str) {
    use std::process::{Command, Stdio};
    let Some(address) = web_address(address) else {
        return;
    };
    let open = |args: &[&str]| {
        Command::new("open")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    if !open(&chrome(address)) {
        open(&[address]);
    }
}

/// The arguments for `open` that show `address` in the person's Chrome. An
/// agent's headless Chrome registers as a running "Google Chrome" too, so
/// plain `open -a` could hand the address to it, where nobody sees it. `-n`
/// starts Chrome with the person's default profile instead, and Chrome's own
/// single-instance check passes the address to their running Chrome, or
/// starts it. Agents' Chromes run with their own profiles, so the address
/// never reaches one.
#[cfg(target_os = "macos")]
fn chrome(address: &str) -> [&str; 5] {
    ["-n", "-a", "Google Chrome", "--args", address]
}

/// Folders agent-browser is installed in when `PATH` lacks them, as for an
/// app started from the Finder.
const INSTALLED: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// How long agent-browser has to answer before it is stopped.
const ANSWER: std::time::Duration = std::time::Duration::from_secs(5);

/// The agent-browser command: on `PATH`, else where Homebrew installs it.
fn command() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        // An empty entry would mean the app's working folder.
        .filter(|folder| !folder.as_os_str().is_empty())
        .chain(INSTALLED.iter().map(PathBuf::from))
        .map(|folder| folder.join("agent-browser"))
        .find(|file| runnable(file))
}

/// Whether `file` is a file that may be run.
fn runnable(file: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(file).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// A browser's viewport: its size in CSS pixels and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// Give `session`'s browser `viewport`, through agent-browser itself, so its
/// own record of the page stays right. Blocks until agent-browser answers,
/// for at most `ANSWER`. A session that has ended gives `NotConnected`.
pub fn set_viewport(session: &Session, viewport: Viewport) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let error = |kind, text| Err(std::io::Error::new(kind, text));
    // The folder discovery found the session in.
    let Some(folder) = socket_dir() else {
        return error(std::io::ErrorKind::NotFound, "no agent-browser folder");
    };
    // agent-browser starts a new browser for a session that has ended, so
    // look again that this one still runs.
    if !sessions(&folder)
        .iter()
        .any(|running| running.name == session.name && running.pid == session.pid)
    {
        return error(std::io::ErrorKind::NotConnected, "the session ended");
    }
    let Some(program) = command() else {
        return error(std::io::ErrorKind::NotFound, "agent-browser not found");
    };
    let mut run = Command::new(program);
    run.args(viewport_args(&session.name, viewport))
        .env("AGENT_BROWSER_SOCKET_DIR", folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = run.spawn()?;
    let deadline = std::time::Instant::now() + ANSWER;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        // A daemon that never answers must not hold a thread for good.
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return error(std::io::ErrorKind::TimedOut, "agent-browser did not answer");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("agent-browser {status}")))
    }
}

/// The arguments of `set_viewport`. The scale is always given: without it
/// agent-browser sets it to 1. A whole scale is written as an integer.
fn viewport_args(session: &str, viewport: Viewport) -> [String; 7] {
    let scale = if viewport.scale.fract() == 0.0 {
        format!("{}", viewport.scale as u32)
    } else {
        viewport.scale.to_string()
    };
    [
        "--session".into(),
        session.into(),
        "set".into(),
        "viewport".into(),
        viewport.width.to_string(),
        viewport.height.to_string(),
        scale,
    ]
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
            target: Some("T".into()),
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
    fn only_a_runnable_file_is_taken_for_agent_browser() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("agent-browser");
        fs::write(&file, "").unwrap();
        assert!(!runnable(&file));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(runnable(&file));
        assert!(!runnable(folder.path()));
    }

    #[test]
    fn the_viewport_is_set_with_its_scale() {
        let viewport = |scale| Viewport {
            width: 900,
            height: 1300,
            scale,
        };
        assert_eq!(
            viewport_args("coder", viewport(2.)),
            ["--session", "coder", "set", "viewport", "900", "1300", "2"]
        );
        assert_eq!(viewport_args("coder", viewport(1.5))[6], "1.5");
    }

    #[test]
    fn only_web_addresses_are_opened() {
        for address in ["https://example.com/a?b", "HTTP://example.com"] {
            assert_eq!(web_address(address), Some(address));
        }
        for address in [
            "file:///etc/hosts",
            "javascript:alert(1)",
            "chrome://newtab/",
            "-a",
            "http://",
            "https://?q",
            "http:///x",
            "https://#top",
            "http://user@/x",
            "http://:8080/",
        ] {
            assert_eq!(web_address(address), None, "{address}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_address_goes_to_a_chrome_with_the_persons_own_profile() {
        assert_eq!(
            chrome("https://example.com/"),
            [
                "-n",
                "-a",
                "Google Chrome",
                "--args",
                "https://example.com/"
            ]
        );
    }

    #[test]
    fn only_a_chrome_started_with_a_window_can_be_shown() {
        let processes = "\
  10     1 /usr/local/bin/agent-browser daemon
  11    10 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --user-data-dir=/tmp/a
  20     1 /usr/local/bin/agent-browser daemon
  21    20 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --headless=new
  30     1 /usr/local/bin/agent-browser daemon
  40     1 /usr/local/bin/agent-browser daemon
  41    40 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --headless=new --user-data-dir=/tmp/b
";
        assert_eq!(window_in(processes, 10, None), Window::Shown(11));
        assert_eq!(
            window_in(processes, 20, None),
            Window::Headless { own_profile: false }
        );
        assert_eq!(window_in(processes, 30, None), Window::NotStarted);
        let person = Some(Path::new(
            "/Users/someone/Library/Application Support/Google/Chrome",
        ));
        assert_eq!(
            window_in(processes, 40, person),
            Window::Headless { own_profile: true }
        );
        // Without the person's folder to compare with, none counts as its own.
        assert_eq!(
            window_in(processes, 40, None),
            Window::Headless { own_profile: false }
        );
    }

    #[test]
    fn only_the_daemons_own_chrome_gives_a_devtools_profile() {
        let processes = "\
  10     1 /usr/local/bin/agent-browser daemon
  11    10 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --user-data-dir=/tmp/agent profile --headless=new
  12    11 /Applications/Google Chrome.app/Contents/Frameworks/Helper --type=renderer --user-data-dir=/tmp/helper
  20     1 /usr/local/bin/agent-browser daemon
  21     5 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=9222 --user-data-dir=/tmp/own
";
        assert_eq!(
            profile_in(processes, 10),
            Some(PathBuf::from("/tmp/agent profile"))
        );
        // A helper of that Chrome, and a daemon beside a Chrome it did not start.
        assert_eq!(profile_in(processes, 11), None);
        assert_eq!(profile_in(processes, 20), None);
    }

    #[test]
    fn a_headless_chrome_in_the_persons_profile_has_none_of_its_own() {
        let home = tempfile::tempdir().unwrap();
        let person = home.path().join(CHROME_PROFILE);
        fs::create_dir_all(&person).unwrap();
        let link = home.path().join("link");
        std::os::unix::fs::symlink(&person, &link).unwrap();
        for folder in [&person, &link] {
            let processes = format!(
                "  41     1 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --headless=new --user-data-dir={}\n",
                folder.display()
            );
            assert_eq!(
                window_in(&processes, 1, Some(&person)),
                Window::Headless { own_profile: false },
                "{}",
                folder.display()
            );
        }
        let other = format!(
            "  41     1 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=0 --headless=new --user-data-dir={}\n",
            home.path().join("agent").display()
        );
        assert_eq!(
            window_in(&other, 1, Some(&person)),
            Window::Headless { own_profile: true }
        );
    }

    #[test]
    fn the_devtools_port_is_read_from_the_profile() {
        let profile = tempfile::tempdir().unwrap();
        assert_eq!(port_in(profile.path()), None);
        fs::write(
            profile.path().join("DevToolsActivePort"),
            "53746\n/devtools/browser/abc\n",
        )
        .unwrap();
        assert_eq!(port_in(profile.path()), Some(53746));
    }

    #[test]
    fn running_sessions_are_read_from_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |file: &str, text: &str| fs::write(dir.path().join(file), text).unwrap();
        write("coder.pid", &format!("{}\n", std::process::id()));
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
                target: Some("T".into()),
                address: Some("https://example.com/".into()),
            }]
        );
        assert!(sessions(&dir.path().join("missing")).is_empty());
    }
}
