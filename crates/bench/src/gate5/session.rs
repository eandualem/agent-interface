//! One scenario's private server, its fixture script and the comparisons
//! made on it.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tmux_workspaces_term::Terminal;
use tmux_workspaces_term::restore::Modes;
use tmux_workspaces_test_support::TmuxServer;
use tmux_workspaces_tmux::control::PaneId;

use super::Comparison;
use super::candidate::Candidate;
use super::compare::{self, Rules};
use super::connection::{Connection, TIMEOUT};
use super::pty::Reference;

/// The session every scenario runs in.
pub const SESSION: &str = "gate5";

/// What the reference's terminal is told it supports, so that tmux sends it
/// every style a pane can use.
const FEATURES: &str = "xterm-256color:RGB:usstyle:overline:strikethrough:hyperlinks:sync";

/// A private server for one scenario, with a directory for its scripts.
pub struct Session {
    /// The server.
    pub server: TmuxServer,
    work: TempDir,
    /// The server's tmux version, as (major, minor).
    pub version: (u32, u32),
}

/// How the fixture's window is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One pane of this many columns and rows.
    One(u16, u16),
    /// A 81x19 window of three panes, the fixture's 40x9 at the bottom right.
    Split,
}

impl Layout {
    /// The window's size, which is also the reference's terminal's.
    pub fn window(self) -> (u16, u16) {
        match self {
            Self::One(cols, rows) => (cols, rows),
            Self::Split => (81, 19),
        }
    }
}

/// What tmux reports about a pane for a comparison.
struct PaneInfo {
    origin: (u16, u16),
    size: (u16, u16),
    cursor: (u16, u16),
    cursor_visible: bool,
    modes: Modes,
}

impl Session {
    /// Starts a private server whose clients get no status line.
    pub fn start(version: (u32, u32)) -> Result<Self, String> {
        let server = TmuxServer::start().map_err(|error| format!("starting tmux: {error}"))?;
        let work = tempfile::Builder::new()
            .prefix("tw-gate5-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let session = Self {
            server,
            work,
            version,
        };
        session.run(&["set-option", "-g", "status", "off"])?;
        session.run(&["set-option", "-as", "terminal-features", FEATURES])?;
        Ok(session)
    }

    /// Runs one tmux command and returns its output.
    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        self.server.run(args).map_err(|error| error.to_string())
    }

    /// Writes the script a fixture pane runs and returns the command that
    /// runs it. The script waits for the reference, runs `draw`, then waits
    /// for the harness before `more`, then echoes a line of input. It
    /// signals the harness after each step.
    pub fn script(&self, draw: &str, more: &str) -> Result<String, String> {
        let wait = |channel| self.server.shell_command(&["wait-for", channel]);
        let signal = |channel| self.server.shell_command(&["wait-for", "-S", channel]);
        let path = self.work.path().join("fixture.sh");
        let text = format!(
            "stop={}\n{}\n{draw}\n{}\n{}\n{more}\n{}\nIFS= read -r line\nprintf '<%s>' \"$line\"\n{}\nexec sleep 1000\n",
            quote(&self.stop_file().display().to_string()),
            wait("ref"),
            signal("drawn"),
            wait("go"),
            signal("more"),
            signal("input"),
        );
        std::fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(format!("sh {}", quote(&path.display().to_string())))
    }

    /// The file whose creation ends a fixture that draws until told to stop.
    pub fn stop_file(&self) -> PathBuf {
        self.work.path().join("stop")
    }

    /// Creates the scenario's session, laid out as `layout`, with `program`
    /// in the fixture's pane, and returns that pane.
    pub fn open(&self, layout: Layout, program: &str) -> Result<PaneId, String> {
        let (cols, rows) = layout.window();
        let (cols, rows) = (cols.to_string(), rows.to_string());
        let size = ["-x", cols.as_str(), "-y", rows.as_str()];
        let created = match layout {
            Layout::One(..) => {
                let mut args = vec!["new-session", "-d", "-s", SESSION, "-P", "-F", "#{pane_id}"];
                args.extend(size);
                args.push(program);
                self.run(&args)?
            }
            Layout::Split => {
                let mut args = vec!["new-session", "-d", "-s", SESSION];
                args.extend(size);
                args.push("sleep 1000");
                self.run(&args)?;
                self.run(&["split-window", "-v", "-t", SESSION, "sleep 1000"])?;
                self.run(&[
                    "split-window",
                    "-h",
                    "-t",
                    SESSION,
                    "-P",
                    "-F",
                    "#{pane_id}",
                    program,
                ])?
            }
        };
        pane_id(&created)
    }

    /// Adds a pane running `program` beside `pane`, returning it.
    pub fn split(&self, pane: PaneId, program: &str) -> Result<PaneId, String> {
        let target = format!("%{}", pane.0);
        pane_id(&self.run(&[
            "split-window",
            "-h",
            "-t",
            &target,
            "-P",
            "-F",
            "#{pane_id}",
            program,
        ])?)
    }

    /// Adds a window, not shown, whose pane writes as fast as it can for
    /// `seconds` once the harness signals `flood`, then sleeps.
    ///
    /// The window is added before the reference attaches and never closed:
    /// closing a window makes tmux redraw every client of its session, and
    /// tmux 3.7 and 3.8 lose underline colours in a redraw, which would show
    /// as a difference the rebuild did not cause. The pane runs `yes`, which
    /// writes faster than the panes it competes with and so fills a stalled
    /// connection's buffer quickly.
    pub fn flood(&self, seconds: f64) -> Result<(), String> {
        let path = self.work.path().join("flood.sh");
        let text = format!(
            "{}
yes {} &
flood=$!
sleep {seconds}
kill $flood
exec sleep 1000
",
            self.server.shell_command(&["wait-for", "flood"]),
            "b".repeat(50),
        );
        std::fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
        let target = format!("{SESSION}:");
        let program = format!("sh {}", quote(&path.display().to_string()));
        self.run(&["new-window", "-d", "-t", &target, &program])
            .map(drop)
    }

    /// Signals `channel` to the fixture.
    pub fn signal(&self, channel: &str) -> Result<(), String> {
        self.run(&["wait-for", "-S", channel]).map(drop)
    }

    /// Waits up to `timeout` for the fixture to signal `channel`.
    pub fn wait(&self, channel: &str, timeout: Duration) -> Result<(), String> {
        if self.waiter(channel)?.done(timeout)? {
            Ok(())
        } else {
            Err(format!("the fixture did not signal {channel}"))
        }
    }

    /// Starts waiting for the fixture to signal `channel`.
    pub fn waiter(&self, channel: &str) -> Result<Waiter, String> {
        let line = self.server.shell_command(&["wait-for", channel]);
        let child = Command::new("/bin/sh")
            .args(["-c", &format!("exec {line}")])
            .stdin(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;
        Ok(Waiter {
            channel: channel.to_owned(),
            child,
        })
    }

    /// Waits until the reference has attached and drawn, and returns the
    /// comparison rules its terminal features call for.
    pub fn reference_ready(&self, reference: &Reference) -> Result<Rules, String> {
        let deadline = Instant::now() + TIMEOUT;
        let features = loop {
            let clients = self.run(&[
                "list-clients",
                "-t",
                SESSION,
                "-F",
                "#{client_control_mode} #{client_termfeatures}",
            ])?;
            if let Some(features) = clients.lines().find_map(|line| line.strip_prefix("0 ")) {
                break features.to_owned();
            }
            if Instant::now() >= deadline {
                return Err("the reference client did not attach".to_owned());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        quiet(reference, None)?;
        Ok(Rules {
            links: features.split(',').any(|feature| feature == "hyperlinks"),
            underline_color_by_value: self.version < (3, 4),
        })
    }

    /// Compares `candidate` with the reference's terminal fed the
    /// reference's output from byte `from` on, at `size`.
    pub fn compare(
        &self,
        reference: &Reference,
        from: usize,
        size: (u16, u16),
        candidate: &mut Candidate,
        connection: &Connection,
        rules: Rules,
    ) -> Result<Comparison, String> {
        candidate.catch_up(connection)?;
        let info = self.pane(candidate.pane())?;
        let mut terminal = Terminal::new(size.0, size.1, 0).map_err(|error| error.to_string())?;
        terminal.write(&reference.bytes_from(from));
        let theirs = compare::grid(&mut terminal)?;
        let ours = compare::grid(&mut candidate.terminal)?;
        let mut comparison = Comparison {
            width: ours.size().0,
            cells: compare::cells(&theirs, info.origin, &ours, rules),
            ..Comparison::default()
        };
        let logical = candidate.cursor()?;
        let enabled = candidate.cursor_enabled()?;
        let presented = presentation(logical, info.origin, size);
        let mut cursor_differences = Vec::new();
        if logical != info.cursor {
            cursor_differences.push(format!(
                "logical cursor at {logical:?}, tmux's at {:?}",
                info.cursor
            ));
        }
        if presented != (theirs.cursor.col, theirs.cursor.row) {
            cursor_differences.push(format!(
                "client cursor projected to ({}, {}), reference at ({}, {})",
                presented.0, presented.1, theirs.cursor.col, theirs.cursor.row
            ));
        }
        if !cursor_differences.is_empty() {
            comparison.cursor = Some(cursor_differences.join("; "));
        }
        if enabled != info.cursor_visible {
            comparison.notes.push(format!(
                "cursor visible {}, tmux's cursor_flag {}",
                enabled, info.cursor_visible
            ));
        }
        if candidate.size != info.size || ours.size() != info.size {
            comparison.notes.push(format!(
                "pane {:?} rebuilt at {:?}, tmux's pane {:?}",
                ours.size(),
                candidate.size,
                info.size
            ));
        }
        let our_modes = candidate
            .terminal
            .modes()
            .map_err(|error| error.to_string())?;
        for ((name, theirs), (_, ours)) in modes(&info.modes).into_iter().zip(modes(&our_modes)) {
            match theirs {
                None => comparison.unknown.push(name),
                Some(theirs) if Some(theirs) != ours => comparison
                    .notes
                    .push(format!("{name} {ours:?}, tmux's {theirs}")),
                Some(_) => {}
            }
        }
        Ok(comparison)
    }

    /// Reads what tmux reports about `pane` in one command, which cannot
    /// redraw any client.
    fn pane(&self, pane: PaneId) -> Result<PaneInfo, String> {
        const FORMAT: &str = "#{pane_left} #{pane_top} #{pane_width} #{pane_height} #{cursor_x} #{cursor_y} #{cursor_flag} #{keypad_cursor_flag} #{keypad_flag} #{insert_flag} #{wrap_flag} #{origin_flag} #{mouse_standard_flag} #{mouse_button_flag} #{mouse_all_flag} #{mouse_utf8_flag} #{mouse_sgr_flag}|#{bracket_paste_flag}|#{pane_private_modes}";
        let target = format!("%{}", pane.0);
        let reply = self.run(&["display-message", "-p", "-t", &target, FORMAT])?;
        let malformed = || format!("unexpected pane formats {reply:?}");
        let mut parts = reply.trim_end_matches('\n').split('|');
        let (Some(fixed), Some(paste), Some(private)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(malformed());
        };
        let numbers: Vec<u16> = fixed
            .split(' ')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| malformed())?;
        let [left, top, width, height, col, row, flags @ ..] = numbers.as_slice() else {
            return Err(malformed());
        };
        let [
            visible,
            cursor_keys,
            keypad,
            insert,
            wrap,
            origin,
            standard,
            button,
            all,
            utf8,
            sgr,
        ] = flags.iter().map(|&flag| flag == 1).collect::<Vec<_>>()[..]
        else {
            return Err(malformed());
        };
        // An empty value is a format this tmux version does not have. The
        // private modes list is empty before 3.8, and on 3.8 only when
        // every mode in it is off, the cursor included.
        let paste = match paste {
            "" => None,
            value => Some(value == "1"),
        };
        let private: Vec<&str> = private.split(',').filter(|mode| !mode.is_empty()).collect();
        let focus = (!private.is_empty()).then(|| private.contains(&"1004"));
        Ok(PaneInfo {
            origin: (*left, *top),
            size: (*width, *height),
            cursor: (*col, *row),
            cursor_visible: visible,
            modes: Modes {
                cursor_keys,
                keypad,
                insert,
                wrap,
                origin,
                mouse_standard: standard,
                mouse_button: button,
                mouse_all: all,
                mouse_utf8: utf8,
                mouse_sgr: sgr,
                bracketed_paste: paste,
                extended_keys: None,
                focus_events: focus,
            },
        })
    }
}

/// tmux's ordinary-client cursor in this harness's viewport (the full
/// window, with no status line or overlay). An off-viewport cursor is
/// parked at home; one at the right edge is clamped to its last column.
fn presentation(logical: (u16, u16), origin: (u16, u16), window: (u16, u16)) -> (u16, u16) {
    let col = u32::from(logical.0) + u32::from(origin.0);
    let row = u32::from(logical.1) + u32::from(origin.1);
    if col > u32::from(window.0) || row > u32::from(window.1) {
        return (0, 0);
    }
    (
        col.min(u32::from(window.0 - 1)) as u16,
        row.min(u32::from(window.1 - 1)) as u16,
    )
}

/// The modes a comparison checks, by name. The extended-keys mode is not
/// among them: the engine does not report it.
fn modes(modes: &Modes) -> [(&'static str, Option<bool>); 12] {
    [
        ("cursor keys", Some(modes.cursor_keys)),
        ("keypad", Some(modes.keypad)),
        ("insert", Some(modes.insert)),
        ("wrap", Some(modes.wrap)),
        ("origin", Some(modes.origin)),
        ("mouse 1000", Some(modes.mouse_standard)),
        ("mouse 1002", Some(modes.mouse_button)),
        ("mouse 1003", Some(modes.mouse_all)),
        ("mouse 1005", Some(modes.mouse_utf8)),
        ("mouse 1006", Some(modes.mouse_sgr)),
        ("bracketed paste", modes.bracketed_paste),
        ("focus reporting", modes.focus_events),
    ]
}

/// A wait for a signal from the fixture, running alongside the harness.
pub struct Waiter {
    channel: String,
    child: Child,
}

impl Waiter {
    /// Waits up to `within` for the signal, and returns whether it came.
    pub fn done(&mut self, within: Duration) -> Result<bool, String> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                return if status.success() {
                    Ok(true)
                } else {
                    Err(format!("waiting for {} failed", self.channel))
                };
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Waits until neither the reference nor `connection`'s watched panes have
/// had output for a while, so tmux has passed on everything the fixture
/// wrote.
pub fn quiet(reference: &Reference, connection: Option<&Connection>) -> Result<(), String> {
    const STILL: Duration = Duration::from_millis(150);
    let counts = || {
        (
            reference.len(),
            connection.map_or(0, Connection::output_bytes),
        )
    };
    let deadline = Instant::now() + TIMEOUT;
    let mut last = counts();
    let mut since = Instant::now();
    while since.elapsed() < STILL {
        if Instant::now() >= deadline {
            return Err("the pane's output did not stop".to_owned());
        }
        std::thread::sleep(Duration::from_millis(25));
        let now = counts();
        if now != last {
            last = now;
            since = Instant::now();
        }
    }
    Ok(())
}

/// Reads a pane ID as tmux prints it, such as `%3`.
pub fn pane_id(text: &str) -> Result<PaneId, String> {
    text.trim()
        .strip_prefix('%')
        .and_then(|number| number.parse().ok())
        .map(PaneId)
        .ok_or_else(|| format!("{text:?} is not a pane"))
}

/// Quotes `word` for a POSIX shell.
pub fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}
