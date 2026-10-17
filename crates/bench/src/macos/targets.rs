//! The programs gates 2 to 4 measure, started for one run and stopped after
//! it (`docs/decisions/0018-milestone-0-benchmark-method-revised.md`).
//!
//! - `app`: the benchmark window of decision record 0019, attached as a
//!   control-mode client to a session on a private tmux server.
//! - `ghostty`: a separate Ghostty instance running a command directly.
//! - `ghostty-tmux`: a separate Ghostty instance running an ordinary tmux
//!   client attached to the same kind of session.
//!
//! Ghostty instances are started with a configuration file of their own and
//! found again by that file's path. Only a process found that way is ever
//! signalled: the user's own Ghostty may be running live sessions.

use std::fs;
use std::io::{self, BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tmux_workspaces_test_support::TmuxServer;

use super::windows::{WindowInfo, windows_of};
use crate::options::Options;

/// The session every run measures.
pub const SESSION: &str = "bench";

/// The font, colours and palette both the benchmark window and Ghostty read,
/// in Ghostty's configuration format. Palette entry 13 is the marker colour
/// gate 2 fills the screen with.
pub const APPEARANCE: &str = "\
font-family = Menlo
font-size = 13
foreground = #e5e5e5
background = #000000
cursor-color = #ffffff
palette = 0=#000000
palette = 1=#cd0000
palette = 2=#00cd00
palette = 3=#cdcd00
palette = 4=#0000ee
palette = 5=#cd00cd
palette = 6=#00cdcd
palette = 7=#e5e5e5
palette = 8=#7f7f7f
palette = 9=#ff0000
palette = 10=#00ff00
palette = 11=#ffff00
palette = 12=#5c5cff
palette = 13=#ff00ff
palette = 14=#00ffff
palette = 15=#ffffff
";

/// How long a program may take to show its window.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// A program a display gate measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The benchmark window, through tmux control mode.
    App,
    /// Ghostty running the command directly.
    Ghostty,
    /// Ghostty running an ordinary tmux client.
    GhosttyTmux,
}

impl Target {
    /// Returns the target's name, as the command line and reports use it.
    pub fn name(self) -> &'static str {
        match self {
            Target::App => "app",
            Target::Ghostty => "ghostty",
            Target::GhosttyTmux => "ghostty-tmux",
        }
    }

    /// Returns the target called `name`.
    pub fn from_name(name: &str) -> Result<Self, String> {
        [Target::App, Target::Ghostty, Target::GhosttyTmux]
            .into_iter()
            .find(|target| target.name() == name)
            .ok_or_else(|| format!("unknown target {name}"))
    }

    /// Returns whether the target draws a tmux session.
    pub fn uses_tmux(self) -> bool {
        self != Target::Ghostty
    }

    /// Parses `--targets`, a comma-separated list, or returns `default`.
    pub fn list(options: &Options, default: &[Target]) -> Result<Vec<Target>, String> {
        match options.required("targets") {
            Ok(list) => list.split(',').map(Self::from_name).collect(),
            Err(_) => Ok(default.to_vec()),
        }
    }
}

/// Where the measured programs are and where their windows go.
#[derive(Clone, Debug)]
pub struct Programs {
    /// The benchmark window program.
    pub window: PathBuf,
    /// Ghostty's application bundle.
    pub ghostty: PathBuf,
    /// The top left corner of every measured window, in points from the top
    /// left of the main display.
    pub origin: (i32, i32),
}

impl Programs {
    /// Reads `--window` (the `bench-window` example built next to this
    /// program), `--ghostty` (`/Applications/Ghostty.app`) and `--origin`
    /// (`40,60`).
    pub fn from_options(options: &Options) -> Result<Self, String> {
        let window = match options.required("window") {
            Ok(path) => PathBuf::from(path),
            Err(_) => std::env::current_exe()
                .map_err(|error| error.to_string())?
                .with_file_name("examples")
                .join("bench-window"),
        };
        let origin = options.get("origin", "40,60".to_owned())?;
        let (x, y) = origin
            .split_once(',')
            .and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))
            .ok_or("--origin needs X,Y")?;
        Ok(Self {
            window,
            ghostty: PathBuf::from(options.get("ghostty", "/Applications/Ghostty.app".to_owned())?),
            origin: (x, y),
        })
    }

    /// Returns Ghostty's version line, such as `Ghostty 1.3.1`.
    pub fn ghostty_version(&self) -> String {
        Command::new(self.ghostty.join("Contents/MacOS/ghostty"))
            .arg("+version")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| text.lines().next().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned())
    }
}

/// Starts a private tmux server whose session [`SESSION`] is 200×50, has no
/// status line and runs `command` with `sh`.
pub fn session(command: &str) -> io::Result<TmuxServer> {
    let server = TmuxServer::start()?;
    server.run(&[
        "new-session",
        "-d",
        "-s",
        SESSION,
        "-x",
        "200",
        "-y",
        "50",
        "sh",
        "-c",
        command,
    ])?;
    // An ordinary client would draw a status line with a clock, which
    // changes the screen; a control client draws none.
    server.run(&["set-option", "-t", SESSION, "status", "off"])?;
    Ok(server)
}

/// A measured program, running with its window on screen.
pub struct Running {
    pid: i32,
    process: Process,
    window: WindowInfo,
}

enum Process {
    /// The benchmark window, a child of this process.
    Child(Child),
    /// A Ghostty instance, found by the path of its own configuration file.
    Ghostty { config: PathBuf },
}

/// Starts `target` and waits for its window to be on screen.
///
/// `server` holds the session the `app` and `ghostty-tmux` targets attach
/// to; `command` is the shell command plain Ghostty runs. `work` is a private
/// directory for configuration files and scripts.
pub fn start(
    target: Target,
    programs: &Programs,
    server: Option<&TmuxServer>,
    command: &str,
    work: &Path,
) -> Result<Running, String> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let config = work.join(format!("terminal-{id}.conf"));
    let session = || server.ok_or_else(|| format!("{} needs a tmux session", target.name()));
    let mut process = match target {
        Target::App => {
            fs::write(&config, APPEARANCE).map_err(|error| error.to_string())?;
            let (tmpdir, socket) = session()?.address();
            let mut child = Command::new(&programs.window)
                .arg("--tmux-tmpdir")
                .arg(tmpdir)
                .args(["--tmux-socket", socket, "--session", SESSION, "--config"])
                .arg(&config)
                .arg("--origin")
                .arg(format!("{},{}", programs.origin.0, programs.origin.1))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|error| format!("{}: {error}", programs.window.display()))?;
            wait_until_ready(&mut child)?;
            Process::Child(child)
        }
        Target::Ghostty | Target::GhosttyTmux => {
            let command = match target {
                Target::GhosttyTmux => session()?.shell_command(&["attach-session", "-t", SESSION]),
                _ => command.to_owned(),
            };
            let script = work.join(format!("terminal-{id}.sh"));
            fs::write(&script, format!("#!/bin/sh\n{command}\n"))
                .and_then(|()| fs::set_permissions(&script, fs::Permissions::from_mode(0o755)))
                .map_err(|error| error.to_string())?;
            fs::write(&config, ghostty_config(programs.origin, &script))
                .map_err(|error| error.to_string())?;
            launch_ghostty(&programs.ghostty, &config)?;
            Process::Ghostty { config }
        }
    };
    let pid = match &process {
        Process::Child(child) => child.id() as i32,
        Process::Ghostty { config } => find_ghostty(config)?,
    };
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match visible_window(pid) {
            Ok(window) => {
                return Ok(Running {
                    pid,
                    process,
                    window,
                });
            }
            Err(error) if Instant::now() >= deadline => {
                terminate(pid, &mut process);
                return Err(error);
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

impl Running {
    /// The program's process ID.
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// The window being measured, as it was when the program started.
    pub fn window(&self) -> &WindowInfo {
        &self.window
    }

    /// Checks that the program's window is still on screen. A window that is
    /// covered, minimised or on another Space does not draw, so a run with
    /// one is a failed run, not a slow one.
    pub fn check_visible(&self) -> Result<(), String> {
        visible_window(self.pid).map(|_| ())
    }

    /// Stops the program and waits for it to exit.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        terminate(self.pid, &mut self.process);
    }
}

/// Returns the ordinary window of `pid` that is on screen.
fn visible_window(pid: i32) -> Result<WindowInfo, String> {
    let windows = windows_of(pid).map_err(|error| error.to_string())?;
    windows
        .into_iter()
        .find(|window| window.layer == 0 && window.on_screen)
        .ok_or_else(|| {
            format!(
                "window not visible: process {pid} has no ordinary window on screen; \
                 leave a normal desktop Space showing where the window opens"
            )
        })
}

/// Stops a measured program and waits for it to exit.
fn terminate(pid: i32, process: &mut Process) {
    match process {
        Process::Child(child) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Process::Ghostty { config } => stop_ghostty(pid, config),
    }
}

/// Returns Ghostty's configuration: the shared appearance, settings that keep
/// the screen still apart from the output measured, the window's size and
/// place, and the script to run.
pub fn ghostty_config(origin: (i32, i32), script: &Path) -> String {
    format!(
        "{APPEARANCE}\
window-width = 200
window-height = 50
window-padding-x = 0
window-padding-y = 0
window-position-x = {}
window-position-y = {}
cursor-style-blink = false
shell-integration = none
window-save-state = never
confirm-close-surface = false
quit-after-last-window-closed = true
auto-update = off
command = {}
",
        origin.0,
        origin.1,
        script.display()
    )
}

/// Waits for the benchmark window's `ready` line.
fn wait_until_ready(child: &mut Child) -> Result<(), String> {
    let stdout = child
        .stdout
        .take()
        .ok_or("the window's output is not piped")?;
    let (lines, received) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match received.recv_timeout(left) {
            Ok(line) if line.starts_with("ready") => return Ok(()),
            Ok(_) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the benchmark window did not become ready".to_owned());
            }
        }
    }
}

/// Starts a new Ghostty instance in the background, without bringing it to
/// the front.
fn launch_ghostty(app: &Path, config: &Path) -> Result<(), String> {
    let status = Command::new("open")
        .args(["-g", "-n", "-a"])
        .arg(app)
        .args(["--args", "--config-default-files=false"])
        .arg(format!("--config-file={}", config.display()))
        .status()
        .map_err(|error| format!("open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open could not start {}", app.display()))
    }
}

/// Finds the Ghostty instance started with `config`.
fn find_ghostty(config: &Path) -> Result<i32, String> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(pid) = pid_with_config(&processes(), config) {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            return Err("the Ghostty instance did not start".to_owned());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Stops the Ghostty instance `pid`, after checking it is still the one
/// started with `config`.
fn stop_ghostty(pid: i32, config: &Path) {
    let ours = || pid_with_config(&processes(), config) == Some(pid);
    if !ours() {
        return;
    }
    // The `kill` program, so this needs no unsafe code.
    let signal = |name: &str| {
        let _ = Command::new("kill")
            .args([&format!("-{name}"), &pid.to_string()])
            .status();
    };
    signal("TERM");
    let deadline = Instant::now() + Duration::from_secs(5);
    while ours() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    if ours() {
        signal("KILL");
    }
}

/// Lists every process with its arguments.
fn processes() -> String {
    Command::new("ps")
        .args(["-axo", "pid=,args="])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_default()
}

/// Returns the process in `ps` output whose arguments name `config` as its
/// configuration file.
fn pid_with_config(processes: &str, config: &Path) -> Option<i32> {
    let argument = format!("--config-file={}", config.display());
    processes.lines().find_map(|line| {
        let (pid, args) = line.trim_start().split_once(' ')?;
        args.split(' ')
            .any(|word| word == argument)
            .then(|| pid.parse().ok())?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_the_instance_started_with_the_config() {
        let ps = "  101 /Applications/Ghostty.app/Contents/MacOS/ghostty\n\
                  202 /Applications/Ghostty.app/Contents/MacOS/ghostty \
                  --config-default-files=false --config-file=/tmp/w/terminal-1.conf\n\
                  303 /Applications/Ghostty.app/Contents/MacOS/ghostty \
                  --config-default-files=false --config-file=/tmp/w/terminal-10.conf\n";
        let config = |path: &str| PathBuf::from(path);
        assert_eq!(
            pid_with_config(ps, &config("/tmp/w/terminal-1.conf")),
            Some(202)
        );
        assert_eq!(
            pid_with_config(ps, &config("/tmp/w/terminal-10.conf")),
            Some(303)
        );
        assert_eq!(pid_with_config(ps, &config("/tmp/w/terminal-2.conf")), None);
    }

    #[test]
    fn ghostty_reads_the_shared_appearance_and_runs_the_script() {
        let config = ghostty_config((40, 60), Path::new("/tmp/w/terminal-1.sh"));
        assert!(config.starts_with(APPEARANCE));
        for line in [
            "window-width = 200",
            "window-height = 50",
            "window-position-x = 40",
            "window-position-y = 60",
            "cursor-style-blink = false",
            "command = /tmp/w/terminal-1.sh",
        ] {
            assert!(config.lines().any(|l| l == line), "{line}");
        }
        assert!(APPEARANCE.contains("palette = 13=#ff00ff\n"));
    }

    #[test]
    fn targets_round_trip_their_names() {
        for target in [Target::App, Target::Ghostty, Target::GhosttyTmux] {
            assert_eq!(Target::from_name(target.name()), Ok(target));
        }
        assert!(Target::from_name("kitty").is_err());
    }

    #[test]
    fn a_session_is_200_by_50_without_a_status_line() {
        let server = session("exec cat").unwrap();
        let shown = server
            .run(&[
                "display-message",
                "-p",
                "-t",
                SESSION,
                "#{window_width}x#{window_height} #{status}",
            ])
            .unwrap();
        assert_eq!(shown.trim(), "200x50 off");
    }
}
