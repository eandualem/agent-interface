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
//! private configuration, data, state and cache directories, so they never
//! read or change the user's own Ghostty settings, and are found again by
//! that file's path. Only a process found that way is ever signalled: the
//! user's own Ghostty may be running live sessions.
//!
//! A window counts as shown only while the very window found at the start is
//! on screen where it was, entirely on the main display and uncovered, with
//! that display awake and the screen unlocked. The benchmark window must also
//! have been built from the same revision as this program, with the same
//! engine CPU target and optimisation, and not as a debug build when this
//! program is a release build.

use std::ffi::OsString;
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

use super::clock::HostTime;
use super::displays::{self, DisplayState};
use super::keys::{self, Modifiers};
use super::windows::{self, WindowInfo, windows_of};
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
/// How often a run that is waiting checks that its window is still shown.
pub const CHECK_EVERY: Duration = Duration::from_secs(1);
/// How far, in points, a measured window may move or change size during a
/// run before the run fails.
const MOVE_TOLERANCE: f64 = 1.0;

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
    build: Option<Build>,
    display: DisplayState,
}

/// How a program was built: the benchmark window, as its `ready` line
/// states it, or this program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    /// The revision it was built from, in the form this program's own
    /// revision takes.
    pub revision: String,
    /// `debug` or `release`: whether it was built with debug assertions.
    pub profile: String,
    /// The CPU the terminal engine was built for: `LIBGHOSTTY_VT_SYS_CPU`
    /// when it was built, or `baseline`.
    pub cpu: String,
    /// The terminal engine's optimisation mode, as the engine reports it.
    pub optimize: String,
    /// The terminal engine's version, optimisation mode and SIMD support.
    pub engine: String,
}

impl Build {
    /// Returns how this program was built, in the terms the benchmark window
    /// uses for itself.
    pub fn own() -> Self {
        let engine = tmux_workspaces_term::engine_build();
        Self {
            revision: env!("BENCH_REVISION").to_owned(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .to_owned(),
            cpu: env!("BENCH_GHOSTTY_CPU").to_owned(),
            optimize: engine
                .as_ref()
                .map_or_else(|_| "unknown".to_owned(), |engine| engine.optimize.clone()),
            engine: match engine {
                Ok(engine) => format!(
                    "libghostty-vt {}, {}, SIMD {}",
                    engine.version, engine.optimize, engine.simd
                ),
                Err(error) => format!("unknown ({error})"),
            },
        }
    }
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
    let (mut process, build) = match target {
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
            let build = wait_until_ready(&mut child)?;
            (Process::Child(child), Some(build))
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
            let home = work.join(format!("ghostty-{id}"));
            launch_ghostty(&programs.ghostty, &config, &home)?;
            (Process::Ghostty { config }, None)
        }
    };
    let pid = match &process {
        Process::Child(child) => child.id() as i32,
        Process::Ghostty { config } => find_ghostty(config)?,
    };
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        // A window may appear before it reaches its place, so the display is
        // checked until the deadline too.
        let shown = first_window(pid)
            .and_then(|window| check_shown(pid, &window).map(|display| (window, display)));
        match shown {
            Ok((window, display)) => {
                return Ok(Running {
                    pid,
                    process,
                    window,
                    build,
                    display,
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
    /// The window being measured, as it was when the program started.
    pub fn window(&self) -> &WindowInfo {
        &self.window
    }

    /// How the program was built, if it is the benchmark window.
    pub fn build(&self) -> Option<&Build> {
        self.build.as_ref()
    }

    /// The state of the window's display when the program started.
    pub fn display_before(&self) -> DisplayState {
        self.display
    }

    /// Checks that the window found at the start is still on screen where
    /// it was, entirely on the main display and uncovered, on an awake
    /// display with the screen unlocked, and returns the display's state. A
    /// window that is covered, minimised or on another Space, or on a
    /// display that is asleep or locked, does not draw, and one that moved
    /// is no longer the window captured, so a run with one is a failed run,
    /// not a slow one.
    pub fn check_visible(&self) -> Result<DisplayState, String> {
        check_shown(self.pid, &self.window)
    }

    /// Waits for `span`, checking every [`CHECK_EVERY`] and at the end that
    /// the window is still shown, and returns the display's state at the
    /// end.
    pub fn wait_visible(&self, span: Duration) -> Result<DisplayState, String> {
        let deadline = Instant::now() + span;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            thread::sleep(left.min(CHECK_EVERY));
            let display = self.check_visible()?;
            if left <= CHECK_EVERY {
                return Ok(display);
            }
        }
    }

    /// Types `text` into the program, if its window passes
    /// [`Running::check_visible`] and is frontmost just before, and returns
    /// the host time read after those checks, just before the first key was
    /// posted.
    pub fn post_text(&self, text: &str) -> Result<HostTime, String> {
        self.check_visible()?;
        keys::post_text(self.pid, &self.window, text).map_err(|error| error.to_string())
    }

    /// Presses one key with `modifiers` in the program, if its window passes
    /// [`Running::check_visible`] and is frontmost just before, and returns
    /// the host time read after those checks, just before it was posted.
    pub fn post_key(&self, code: u16, modifiers: Modifiers) -> Result<HostTime, String> {
        self.check_visible()?;
        keys::post_key(self.pid, &self.window, code, modifiers).map_err(|error| error.to_string())
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

/// Returns the ordinary window of `pid` that is on screen: the window a run
/// measures from then on.
fn first_window(pid: i32) -> Result<WindowInfo, String> {
    windows_of(pid)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|window| window.layer == 0 && window.on_screen)
        .ok_or_else(|| {
            format!(
                "window not visible: process {pid} has no ordinary window on screen; \
                 leave a normal desktop Space showing where the window opens"
            )
        })
}

/// Checks that `expected`, the window of `pid` a run measures, is still
/// listed and on screen within [`MOVE_TOLERANCE`] of where it was, entirely
/// on the main display and uncovered, on an awake display with the screen
/// unlocked, and returns the display's state.
fn check_shown(pid: i32, expected: &WindowInfo) -> Result<DisplayState, String> {
    let id = expected.id;
    let window = windows::window_of(pid, id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("window not visible: process {pid}'s window {id} is gone"))?;
    if !window.on_screen {
        return Err(format!(
            "window not visible: process {pid}'s window {id} is not on screen; leave a normal \
             desktop Space showing where the window opens"
        ));
    }
    let (now, then) = (window.bounds, expected.bounds);
    if !now.is_near(then, MOVE_TOLERANCE) {
        return Err(format!(
            "window moved: process {pid}'s window {id} is at {},{} sized {}×{}, not at {},{} \
             sized {}×{} as at the start",
            now.x, now.y, now.width, now.height, then.x, then.y, then.width, then.height
        ));
    }
    on_main_display(pid, &window)?;
    let display = DisplayState::read(window.bounds);
    display.check()?;
    // A window ordered on screen can still be covered by another one, even
    // one of its own process, and a covered window stops drawing.
    if windows::is_covered(pid, &window).map_err(|error| error.to_string())? {
        return Err(format!(
            "window not visible: another window covers part of process {pid}'s window {id}"
        ));
    }
    Ok(display)
}

/// Checks that `window`, of process `pid`, lies entirely on the main
/// display. Programs place a window's origin on different displays, so this
/// keeps every target on the same one.
fn on_main_display(pid: i32, window: &WindowInfo) -> Result<(), String> {
    let main = displays::main_bounds();
    let at = window.bounds;
    if main.contains(at) {
        Ok(())
    } else {
        Err(format!(
            "window not on the main display: process {pid}'s window at {},{} sized {}×{} \
             does not lie within the main display's {}×{} points at {},{}; \
             choose another --origin",
            at.x, at.y, at.width, at.height, main.width, main.height, main.x, main.y
        ))
    }
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

/// Waits for the benchmark window's `ready` line, and returns how the window
/// was built once that is checked against this program's own build.
fn wait_until_ready(child: &mut Child) -> Result<Build, String> {
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
    let result = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match received.recv_timeout(left) {
            Ok(line) if line.starts_with("ready") => {
                let build = build_of(&line);
                break check_build(&build, &Build::own()).map(|()| build);
            }
            Ok(_) => {}
            Err(_) => break Err("the benchmark window did not become ready".to_owned()),
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// Reads how the benchmark window was built from its `ready` line: after
/// the window's size, tab-separated fields, each a name, a space and a
/// value. A field the line lacks is left empty.
fn build_of(ready: &str) -> Build {
    let field = |name: &str| {
        ready
            .split('\t')
            .skip(1)
            .find_map(|field| field.strip_prefix(name)?.strip_prefix(' '))
            .unwrap_or_default()
            .to_owned()
    };
    Build {
        revision: field("revision"),
        profile: field("profile"),
        cpu: field("cpu"),
        optimize: field("optimize"),
        engine: field("engine"),
    }
}

/// Refuses a benchmark window built otherwise than `own`, this program: from
/// another revision, with another engine CPU target or optimisation, or as a
/// debug build when this program is not one. It would measure other code, or
/// code built another way.
fn check_build(build: &Build, own: &Build) -> Result<(), String> {
    if [&build.revision, &build.profile, &build.cpu, &build.optimize]
        .iter()
        .any(|field| field.is_empty())
    {
        return Err(
            "the benchmark window does not say how it was built; build both programs together"
                .to_owned(),
        );
    }
    if build.revision != own.revision {
        return Err(format!(
            "the benchmark window was built from {}, but this program from {}; \
             build both programs together",
            build.revision, own.revision
        ));
    }
    if build.profile == "debug" && own.profile != "debug" {
        return Err(
            "the benchmark window is a debug build, but this program a release build; \
             build the window with --release too"
                .to_owned(),
        );
    }
    if build.cpu != own.cpu {
        return Err(format!(
            "the benchmark window's terminal engine was built for CPU {}, but this program's \
             for {}; build both with the same LIBGHOSTTY_VT_SYS_CPU",
            build.cpu, own.cpu
        ));
    }
    if build.optimize != own.optimize {
        return Err(format!(
            "the benchmark window's terminal engine is optimised {}, but this program's {}; \
             build both with the same LIBGHOSTTY_VT_SYS_OPTIMIZE",
            build.optimize, own.optimize
        ));
    }
    Ok(())
}

/// The private directories a Ghostty instance gets in place of the user's
/// own, by environment variable and directory name.
const GHOSTTY_DIRECTORIES: [(&str, &str); 4] = [
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_CACHE_HOME", "cache"),
];

/// Starts a new Ghostty instance in the background, without bringing it to
/// the front, with `config` as its only configuration file and private
/// directories in `home`.
fn launch_ghostty(app: &Path, config: &Path, home: &Path) -> Result<(), String> {
    for (_, directory) in GHOSTTY_DIRECTORIES {
        fs::create_dir_all(home.join(directory)).map_err(|error| error.to_string())?;
    }
    let status = Command::new("open")
        .args(ghostty_arguments(app, config, home))
        .status()
        .map_err(|error| format!("open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open could not start {}", app.display()))
    }
}

/// Returns the arguments of `open` that start a new Ghostty instance in the
/// background, with `config` as its only configuration file and its
/// configuration, data, state and cache directories inside `home`.
fn ghostty_arguments(app: &Path, config: &Path, home: &Path) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec!["-g".into(), "-n".into(), "-a".into(), app.into()];
    for (variable, directory) in GHOSTTY_DIRECTORIES {
        let mut value = OsString::from(format!("{variable}="));
        value.push(home.join(directory));
        arguments.extend(["--env".into(), value]);
    }
    let mut config_file = OsString::from("--config-file=");
    config_file.push(config);
    arguments.extend([
        "--args".into(),
        "--config-default-files=false".into(),
        config_file,
    ]);
    arguments
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
    fn ghostty_starts_with_only_private_settings_and_directories() {
        let arguments = ghostty_arguments(
            Path::new("/Applications/Ghostty.app"),
            Path::new("/tmp/w/terminal-1.conf"),
            Path::new("/tmp/w/ghostty-1"),
        );
        let expected = [
            "-g",
            "-n",
            "-a",
            "/Applications/Ghostty.app",
            "--env",
            "XDG_CONFIG_HOME=/tmp/w/ghostty-1/config",
            "--env",
            "XDG_DATA_HOME=/tmp/w/ghostty-1/data",
            "--env",
            "XDG_STATE_HOME=/tmp/w/ghostty-1/state",
            "--env",
            "XDG_CACHE_HOME=/tmp/w/ghostty-1/cache",
            "--args",
            "--config-default-files=false",
            "--config-file=/tmp/w/terminal-1.conf",
        ];
        assert_eq!(arguments, expected.map(OsString::from));
        // The instance is found again by the same argument.
        let ps = "  202 /Applications/Ghostty.app/Contents/MacOS/ghostty \
                  --config-default-files=false --config-file=/tmp/w/terminal-1.conf\n";
        assert_eq!(
            pid_with_config(ps, Path::new("/tmp/w/terminal-1.conf")),
            Some(202)
        );
    }

    #[test]
    fn the_ready_line_states_how_the_window_was_built() {
        let ready = "ready 200x50 cell 7.8x16\trevision 0123abc with uncommitted changes\t\
                     profile release\tcpu baseline\toptimize ReleaseFast\t\
                     engine libghostty-vt 1.2.0, ReleaseFast, SIMD true";
        assert_eq!(build_of(ready), build("0123abc with uncommitted changes"));
        let unsaid = Build {
            revision: String::new(),
            profile: String::new(),
            cpu: String::new(),
            optimize: String::new(),
            engine: String::new(),
        };
        assert_eq!(build_of("ready 200x50 cell 7.8x16"), unsaid);
    }

    /// A release build from `revision`, for a baseline CPU and optimised
    /// ReleaseFast.
    fn build(revision: &str) -> Build {
        Build {
            revision: revision.to_owned(),
            profile: "release".to_owned(),
            cpu: "baseline".to_owned(),
            optimize: "ReleaseFast".to_owned(),
            engine: "libghostty-vt 1.2.0, ReleaseFast, SIMD true".to_owned(),
        }
    }

    #[test]
    fn only_a_window_built_like_this_program_is_measured() {
        let own = build("abc");
        let debug = |build: Build| Build {
            profile: "debug".to_owned(),
            ..build
        };
        assert_eq!(check_build(&build("abc"), &own), Ok(()));
        assert_eq!(
            check_build(&debug(build("abc")), &debug(own.clone())),
            Ok(())
        );
        assert_eq!(check_build(&build("abc"), &debug(own.clone())), Ok(()));
        // Other code, unoptimised code, or code built for another CPU or
        // with another engine optimisation.
        assert!(check_build(&build("def"), &own).is_err());
        assert!(check_build(&debug(build("abc")), &own).is_err());
        let native = Build {
            cpu: "native".to_owned(),
            ..build("abc")
        };
        assert!(check_build(&native, &own).is_err());
        let safe = Build {
            optimize: "ReleaseSafe".to_owned(),
            ..build("abc")
        };
        assert!(check_build(&safe, &own).is_err());
        // A window that does not say.
        let silent = Build {
            cpu: String::new(),
            ..build("abc")
        };
        assert!(check_build(&silent, &own).is_err());
    }

    #[test]
    fn this_program_states_its_own_build() {
        let own = Build::own();
        assert_eq!(own.revision, env!("BENCH_REVISION"));
        assert_eq!(own.cpu, env!("BENCH_GHOSTTY_CPU"));
        assert!(!own.optimize.is_empty());
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
