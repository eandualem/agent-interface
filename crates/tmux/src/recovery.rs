//! Rebuilding a pane's terminal state from tmux when its earlier output is
//! not available: on attach, after a pause and after a resize
//! (`docs/decisions/0021-pane-state-recovery.md`).
//!
//! The types here do no I/O. The caller writes the commands they return to
//! the pane's control connection, hands them the replies to those commands
//! and the pane's output, and gets back a [`Snapshot`] of the pane together
//! with the output that follows it.
//!
//! - [`Recovery`] rebuilds one pane on a connection that is receiving its
//!   output: it pauses the pane, waits for the pause command's reply, then
//!   sends one line that captures the pane and continues it.
//! - [`Attach`] rebuilds the panes of a connection attached with
//!   `-f no-output,ignore-size`, which has received no output yet.

use crate::capture::{self, Row};
use crate::control::PaneId;

/// An error in a reply to a recovery command.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A capture that could not be read.
    #[error(transparent)]
    Capture(#[from] capture::Error),
    /// A format whose value is not what tmux reports for it.
    #[error("malformed value of format {0}")]
    Format(&'static str),
    /// More replies than the commands sent.
    #[error("a reply to a command that recovery did not send")]
    UnexpectedReply,
}

/// The reply to one command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// Whether the command succeeded (`%end`) or failed (`%error`).
    pub ok: bool,
    /// The reply's lines, without their line ends.
    pub lines: Vec<Vec<u8>>,
}

/// A pane's state as tmux holds it, from which a terminal can be rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The pane.
    pub pane: PaneId,
    /// The rows of the screen the pane shows, after all the history tmux
    /// keeps for it: [`PaneState::history`] rows of history, then
    /// [`PaneState::height`] rows of screen.
    pub rows: Vec<Row>,
    /// With the alternate screen on, the rows of the primary screen that
    /// tmux saved when the program switched; otherwise empty.
    pub saved: Vec<Row>,
    /// The cursor, modes and other state.
    pub state: PaneState,
    /// Bytes the pane's parser holds without having acted on them yet,
    /// ready to feed a terminal after the state above
    /// ([`capture::pending`]).
    pub pending: Vec<u8>,
}

/// What tmux's formats report about a pane.
///
/// A mode that a tmux version does not report is `None`, never a false
/// value (`docs/decisions/0014-supported-tmux-versions.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneState {
    /// Content column within the tmux window, excluding borders and scrollbars.
    pub left: u16,
    /// Content row within the tmux window, excluding pane border status.
    pub top: u16,
    /// The pane's width in cells.
    pub width: u16,
    /// The pane's height in cells.
    pub height: u16,
    /// The number of lines of history tmux keeps for the pane.
    pub history: usize,
    /// With the alternate screen on, the cursor's column and row that tmux
    /// saved when the program switched to it; otherwise `None`.
    pub alternate_saved_cursor: Option<(u16, u16)>,
    /// The cursor's column and row, from zero at the top left of the screen.
    /// A column equal to the width is a cursor waiting to wrap. Resizing
    /// the alternate screen can leave it beyond the width; retain that
    /// value rather than clamping it.
    pub cursor: (u16, u16),
    /// Whether the program shows the cursor.
    pub cursor_visible: bool,
    /// The top and bottom rows of the scrolling region, from zero.
    pub scroll_region: (u16, u16),
    /// The columns with a tab stop, from zero.
    pub tab_stops: Vec<u16>,
    /// The terminal modes the program has set.
    pub modes: Modes,
}

/// The terminal modes tmux reports for a pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    /// Cursor keys send application sequences (DECCKM).
    pub cursor_keys: bool,
    /// The keypad sends application sequences (DECKPAM).
    pub keypad: bool,
    /// Insert mode (IRM).
    pub insert: bool,
    /// Text wraps at the right margin (DECAWM).
    pub wrap: bool,
    /// Cursor positions are relative to the scrolling region (DECOM).
    pub origin: bool,
    /// Mouse button presses are reported (1000).
    pub mouse_standard: bool,
    /// Mouse motion with a button down is reported (1002).
    pub mouse_button: bool,
    /// All mouse motion is reported (1003).
    pub mouse_all: bool,
    /// Mouse reports use UTF-8 coordinates (1005).
    pub mouse_utf8: bool,
    /// Mouse reports use the SGR format (1006).
    pub mouse_sgr: bool,
    /// Bracketed paste (2004); reported from tmux 3.7.
    pub bracketed_paste: Option<bool>,
    /// The extended-keys mode; reported from tmux 3.5.
    pub extended_keys: Option<ExtendedKeys>,
    /// Source server extended-key format: true for CSI-u, false for xterm.
    pub extended_keys_csi_u: Option<bool>,
    /// Focus reporting (1004); reported from tmux 3.8.
    pub focus_events: Option<bool>,
}

/// How a program has asked for keys to be encoded, as tmux's
/// `pane_key_mode` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendedKeys {
    /// Ordinary keys (`VT10x`).
    Off,
    /// modifyOtherKeys level 1 (`Ext 1`).
    Level1,
    /// modifyOtherKeys level 2 (`Ext 2`).
    Level2,
}

/// Rebuilds one pane on a control connection that receives its output.
///
/// [`Recovery::start`] returns the pause command. Hand every reply to the
/// commands it returns to [`Recovery::reply`], in order, and every piece of
/// the pane's output to [`Recovery::output`]. tmux sends nothing for a paused
/// pane, so all of the pane's output after the pause command's reply belongs
/// after the rebuilt state; the line's commands do not wait, so tmux reads
/// no pane output while it runs them, and the capture is exactly the state
/// at which output resumes.
#[derive(Debug)]
pub struct Recovery {
    pane: PaneId,
    /// The commands whose replies have not all arrived: the pause, or the
    /// line.
    waiting: Waiting,
    replies: Vec<Reply>,
    /// The pane's output after the boundary.
    held: Option<Vec<u8>>,
}

#[derive(Debug)]
enum Waiting {
    Pause,
    Line,
    /// The rebuild is done; no more replies belong to it.
    Nothing,
}

/// What to do after a reply.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<T> {
    /// Write this command line to the connection.
    Send(String),
    /// Wait for more replies.
    Wait,
    /// The rebuild is ready.
    Done(T),
    /// A command failed, for example because the pane exited. The replies
    /// are not a snapshot and must be discarded.
    Failed,
}

/// A rebuilt pane and the output that follows it.
#[derive(Debug, PartialEq, Eq)]
pub struct Rebuilt {
    /// The pane's state at the boundary.
    pub snapshot: Snapshot,
    /// The pane's output after the boundary, to apply after the snapshot.
    pub output: Vec<u8>,
}

impl Recovery {
    /// Starts rebuilding `pane`, returning the command line that pauses it.
    pub fn start(pane: PaneId) -> (Self, String) {
        let recovery = Self {
            pane,
            waiting: Waiting::Pause,
            replies: Vec::new(),
            held: None,
        };
        (
            recovery,
            format!("refresh-client -A '{}:pause'\n", target(pane)),
        )
    }

    /// Starts rebuilding `pane` after tmux paused it itself (`pause-after`),
    /// returning the line to send. All of the pane's output after the
    /// `%pause` notification belongs after the rebuilt state.
    pub fn paused(pane: PaneId) -> (Self, String) {
        let recovery = Self {
            pane,
            waiting: Waiting::Line,
            replies: Vec::new(),
            held: Some(Vec::new()),
        };
        (recovery, line(&[pane], &continue_command(pane)))
    }

    /// The pane being rebuilt.
    pub fn pane(&self) -> PaneId {
        self.pane
    }

    /// Takes the pane's output, in order.
    pub fn output(&mut self, data: &[u8]) {
        // Output before the boundary is part of the capture.
        if let Some(held) = &mut self.held {
            held.extend_from_slice(data);
        }
    }

    /// Takes the reply to the next command this recovery sent.
    pub fn reply(&mut self, reply: Reply) -> Result<Step<Rebuilt>, Error> {
        if matches!(self.waiting, Waiting::Nothing) {
            return Err(Error::UnexpectedReply);
        }
        if !reply.ok {
            // tmux skips the rest of a line after a failed command.
            return Ok(Step::Failed);
        }
        match self.waiting {
            Waiting::Nothing => Err(Error::UnexpectedReply),
            Waiting::Pause => {
                self.waiting = Waiting::Line;
                self.held = Some(Vec::new());
                Ok(Step::Send(line(&[self.pane], &continue_command(self.pane))))
            }
            Waiting::Line => {
                self.replies.push(reply);
                if self.replies.len() < replies_per_pane() + 1 {
                    return Ok(Step::Wait);
                }
                self.waiting = Waiting::Nothing;
                let snapshot = snapshot(self.pane, &self.replies[..replies_per_pane()])?;
                Ok(Step::Done(Rebuilt {
                    snapshot,
                    output: self.held.take().unwrap_or_default(),
                }))
            }
        }
    }
}

/// Rebuilds every pane of a connection attached with
/// `-f no-output,ignore-size`, which receives no output until the line
/// [`Attach::new`] returns turns it on. All of the connection's output after
/// that belongs after the rebuilt panes.
///
/// If a command fails, for example because a pane exited, tmux skips the rest
/// of the line and output stays off. Send a new [`Attach`] line on the same
/// connection for the panes that remain: closing the connection could
/// destroy a session that has `destroy-unattached` on.
#[derive(Debug)]
pub struct Attach {
    panes: Vec<PaneId>,
    replies: Vec<Reply>,
}

impl Attach {
    /// Starts rebuilding `panes`, returning the line to send.
    pub fn new(panes: Vec<PaneId>) -> (Self, String) {
        let command = line(&panes, "refresh-client -f '!no-output'");
        let attach = Self {
            panes,
            replies: Vec::new(),
        };
        (attach, command)
    }

    /// Takes the reply to the next command of the line.
    pub fn reply(&mut self, reply: Reply) -> Result<Step<Vec<Snapshot>>, Error> {
        if !reply.ok {
            return Ok(Step::Failed);
        }
        let expected = self.panes.len() * replies_per_pane() + 1;
        if self.replies.len() == expected {
            return Err(Error::UnexpectedReply);
        }
        self.replies.push(reply);
        if self.replies.len() < expected {
            return Ok(Step::Wait);
        }
        self.panes
            .iter()
            .zip(self.replies.chunks(replies_per_pane()))
            .map(|(&pane, replies)| snapshot(pane, replies))
            .collect::<Result<_, _>>()
            .map(Step::Done)
    }
}

/// Returns the commands that ask whether hooks are set on the commands
/// recovery sends, globally and for `session`. Their replies go to
/// [`hooks_break_recovery`].
///
/// tmux runs a command's hooks before the next command of the line. A hook
/// that waits lets pane output in between, and any hook can change the pane.
pub fn hooks_command(session: &str) -> String {
    format!(
        "show-hooks -g ; show-hooks -t '{}'\n",
        session.replace('\'', r"'\''")
    )
}

/// Returns whether the lines of `show-hooks` replies show a hook on a
/// command that recovery sends, so the pane cannot be rebuilt exactly.
pub fn hooks_break_recovery<'a>(lines: impl IntoIterator<Item = &'a [u8]>) -> bool {
    // A hook that is set is listed with an index, as in
    // `after-capture-pane[0] run-shell ...`; one that is not, by its name.
    lines.into_iter().any(|line| {
        [
            "after-capture-pane[",
            "after-display-message[",
            "after-refresh-client[",
        ]
        .iter()
        .any(|hook| line.starts_with(hook.as_bytes()))
    })
}

/// The formats each rebuild reads, one `display-message` each: some formats
/// can contain a line end.
const FORMATS: [&str; 28] = [
    "pane_width",
    "pane_height",
    "history_size",
    "alternate_on",
    "alternate_saved_x",
    "alternate_saved_y",
    "cursor_x",
    "cursor_y",
    "cursor_flag",
    "scroll_region_upper",
    "scroll_region_lower",
    "pane_tabs",
    "keypad_cursor_flag",
    "keypad_flag",
    "insert_flag",
    "wrap_flag",
    "origin_flag",
    "mouse_standard_flag",
    "mouse_button_flag",
    "mouse_all_flag",
    "mouse_utf8_flag",
    "mouse_sgr_flag",
    "bracket_paste_flag",
    "pane_key_mode",
    "extended-keys-format",
    "pane_private_modes",
    "pane_left",
    "pane_top",
];

/// The replies a line has for each pane: two captures, the formats and the
/// pending bytes.
const fn replies_per_pane() -> usize {
    FORMATS.len() + 3
}

/// Returns `pane` as a target, quoted: in a control command, `%` starts a
/// format.
fn target(pane: PaneId) -> String {
    format!("%{}", pane.0)
}

fn continue_command(pane: PaneId) -> String {
    format!("refresh-client -A '{}:continue'", target(pane))
}

/// Returns one line that reads every pane's state and then runs `last`.
/// None of its commands waits, so tmux reads no pane output while it runs
/// them.
fn line(panes: &[PaneId], last: &str) -> String {
    let mut commands = Vec::new();
    for &pane in panes {
        let target = target(pane);
        // The screen with all retained history, then the primary screen
        // saved while the alternate screen is on (empty otherwise).
        commands.push(format!("capture-pane -p -e -N -C -S - -E - -t '{target}'"));
        commands.push(format!(
            "capture-pane -p -e -N -C -a -q -S - -E - -t '{target}'"
        ));
        for format in FORMATS {
            commands.push(format!("display-message -p -t '{target}' '#{{{format}}}'"));
        }
        commands.push(format!("capture-pane -p -P -C -t '{target}'"));
    }
    commands.push(last.to_owned());
    let mut line = commands.join(" ; ");
    line.push('\n');
    line
}

/// Builds a snapshot from one pane's replies, in the order [`line`] sends
/// its commands.
fn snapshot(pane: PaneId, replies: &[Reply]) -> Result<Snapshot, Error> {
    let values: Vec<String> = replies[2..2 + FORMATS.len()]
        .iter()
        .map(|reply| {
            reply
                .lines
                .first()
                .map(|line| String::from_utf8_lossy(line).into_owned())
                .unwrap_or_default()
        })
        .collect();
    let state = pane_state(&values)?;
    // Without an alternate screen, the saved capture is one empty line.
    let saved = if state.alternate_saved_cursor.is_some() {
        capture::rows(lines(&replies[1]))?
    } else {
        Vec::new()
    };
    Ok(Snapshot {
        pane,
        rows: capture::rows(lines(&replies[0]))?,
        saved,
        state,
        pending: capture::pending(lines(&replies[2 + FORMATS.len()]))?,
    })
}

fn lines(reply: &Reply) -> impl Iterator<Item = &[u8]> {
    reply.lines.iter().map(Vec::as_slice)
}

/// Reads the values of [`FORMATS`], in order.
fn pane_state(values: &[String]) -> Result<PaneState, Error> {
    let value = |name: &'static str| -> &str {
        let index = FORMATS.iter().position(|format| *format == name);
        // Every name asked for here is in FORMATS.
        values[index.unwrap_or_default()].as_str()
    };
    let number = |name: &'static str| -> Result<u16, Error> {
        value(name).parse().map_err(|_| Error::Format(name))
    };
    let flag = |name: &'static str| -> Result<bool, Error> {
        match value(name) {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(Error::Format(name)),
        }
    };
    // A format this tmux does not have reads as empty.
    let optional_flag = |name: &'static str| -> Result<Option<bool>, Error> {
        if value(name).is_empty() {
            Ok(None)
        } else {
            flag(name).map(Some)
        }
    };
    let tab_stops = value("pane_tabs")
        .split(',')
        .filter(|stop| !stop.is_empty())
        .map(|stop| stop.parse().map_err(|_| Error::Format("pane_tabs")))
        .collect::<Result<_, _>>()?;
    let extended_keys = match value("pane_key_mode") {
        "" => None,
        "VT10x" => Some(ExtendedKeys::Off),
        "Ext 1" => Some(ExtendedKeys::Level1),
        "Ext 2" => Some(ExtendedKeys::Level2),
        _ => return Err(Error::Format("pane_key_mode")),
    };
    let private_modes = value("pane_private_modes")
        .split(',')
        .filter(|mode| !mode.is_empty())
        .map(|mode| {
            mode.parse::<u16>()
                .map_err(|_| Error::Format("pane_private_modes"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // The list is empty on a tmux before 3.8, which has no such format, and
    // on 3.8 when every mode it lists is off, the cursor and wrapping
    // included. Either way, focus reporting is then unknown.
    let focus_events = (!private_modes.is_empty()).then(|| private_modes.contains(&1004));
    Ok(PaneState {
        left: number("pane_left")?,
        top: number("pane_top")?,
        width: number("pane_width")?,
        height: number("pane_height")?,
        history: value("history_size")
            .parse()
            .map_err(|_| Error::Format("history_size"))?,
        alternate_saved_cursor: if flag("alternate_on")? {
            Some((number("alternate_saved_x")?, number("alternate_saved_y")?))
        } else {
            None
        },
        cursor: (number("cursor_x")?, number("cursor_y")?),
        cursor_visible: flag("cursor_flag")?,
        scroll_region: (
            number("scroll_region_upper")?,
            number("scroll_region_lower")?,
        ),
        tab_stops,
        modes: Modes {
            cursor_keys: flag("keypad_cursor_flag")?,
            keypad: flag("keypad_flag")?,
            insert: flag("insert_flag")?,
            wrap: flag("wrap_flag")?,
            origin: flag("origin_flag")?,
            mouse_standard: flag("mouse_standard_flag")?,
            mouse_button: flag("mouse_button_flag")?,
            mouse_all: flag("mouse_all_flag")?,
            mouse_utf8: flag("mouse_utf8_flag")?,
            mouse_sgr: flag("mouse_sgr_flag")?,
            bracketed_paste: optional_flag("bracket_paste_flag")?,
            extended_keys,
            extended_keys_csi_u: match value("extended-keys-format") {
                "" => None,
                "xterm" => Some(false),
                "csi-u" => Some(true),
                _ => return Err(Error::Format("extended-keys-format")),
            },
            focus_events,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(lines: &[&str]) -> Reply {
        Reply {
            ok: true,
            lines: lines.iter().map(|line| line.as_bytes().to_vec()).collect(),
        }
    }

    /// Format values for an 80x24 pane with nothing set, in FORMATS order.
    fn values() -> Vec<(&'static str, &'static str)> {
        FORMATS
            .iter()
            .map(|&name| {
                let value = match name {
                    "pane_width" => "80",
                    "pane_height" => "24",
                    "history_size" | "alternate_on" | "scroll_region_upper" => "0",
                    "scroll_region_lower" => "23",
                    "pane_tabs" => "8,16",
                    "cursor_flag" | "wrap_flag" => "1",
                    "bracket_paste_flag"
                    | "pane_key_mode"
                    | "pane_private_modes"
                    | "extended-keys-format" => "",
                    _ => "0",
                };
                (name, value)
            })
            .collect()
    }

    fn replies(values: &[(&str, &str)]) -> Vec<Reply> {
        // Without an alternate screen, tmux replies to the saved capture
        // with one empty line.
        let mut replies = vec![ok(&["\\033[1mhi"]), ok(&[""])];
        replies.extend(values.iter().map(|(_, value)| ok(&[value])));
        replies.push(ok(&["\\033[3"]));
        replies
    }

    /// The values of [`values`] with `changes` made.
    fn with_values<'a>(changes: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        values()
            .into_iter()
            .map(|(name, value)| {
                let changed = changes.iter().find(|(changed, _)| *changed == name);
                (name, changed.map_or(value, |(_, value)| *value))
            })
            .collect()
    }

    fn with(changes: &[(&str, &str)]) -> Result<PaneState, Error> {
        let values: Vec<String> = with_values(changes)
            .into_iter()
            .map(|(_, value)| value.to_owned())
            .collect();
        pane_state(&values)
    }

    #[test]
    fn a_recovery_pauses_then_captures_and_continues_in_one_line() {
        let (mut recovery, pause) = Recovery::start(PaneId(7));
        assert_eq!(pause, "refresh-client -A '%7:pause'\n");
        // Output before the pause's reply is in the capture.
        recovery.output(b"before");
        let Step::Send(line) = recovery.reply(ok(&[])).unwrap() else {
            panic!("the line follows the pause's reply");
        };
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.starts_with("capture-pane -p -e -N -C -S - -E - -t '%7' ; "));
        assert!(line.contains(" ; display-message -p -t '%7' '#{cursor_x}' ; "));
        assert!(
            line.ends_with(" ; capture-pane -p -P -C -t '%7' ; refresh-client -A '%7:continue'\n")
        );
        assert_eq!(line.matches(" ; ").count(), replies_per_pane());

        recovery.output(b"after");
        let mut replies = replies(&values());
        replies.push(ok(&[]));
        let last = replies.pop().unwrap();
        for reply in replies {
            assert_eq!(recovery.reply(reply).unwrap(), Step::Wait);
        }
        let Step::Done(rebuilt) = recovery.reply(last).unwrap() else {
            panic!("done after the continue's reply");
        };
        assert_eq!(rebuilt.output, b"after");
        assert_eq!(recovery.reply(ok(&[])), Err(Error::UnexpectedReply));
        let snapshot = rebuilt.snapshot;
        assert_eq!(snapshot.pane, PaneId(7));
        assert_eq!(snapshot.rows.len(), 1);
        assert!(snapshot.saved.is_empty());
        assert_eq!(snapshot.pending, b"\x1b[3");
        assert_eq!((snapshot.state.width, snapshot.state.height), (80, 24));
        assert_eq!(snapshot.state.tab_stops, [8, 16]);
    }

    #[test]
    fn reads_the_saved_screen_only_with_the_alternate_screen_on() {
        let mut replies = replies(&values());
        replies[1] = ok(&["P1", ""]);
        assert!(snapshot(PaneId(1), &replies).unwrap().saved.is_empty());
        let on = with_values(&[
            ("alternate_on", "1"),
            ("alternate_saved_x", "0"),
            ("alternate_saved_y", "1"),
        ]);
        let mut replies = self::replies(&on);
        replies[1] = ok(&["P1", ""]);
        assert_eq!(snapshot(PaneId(1), &replies).unwrap().saved.len(), 2);
    }

    #[test]
    fn after_tmux_pauses_a_pane_its_output_from_then_on_is_held() {
        let (mut recovery, line) = Recovery::paused(PaneId(2));
        assert!(line.ends_with("refresh-client -A '%2:continue'\n"));
        recovery.output(b"held");
        for reply in replies(&values()) {
            assert_eq!(recovery.reply(reply).unwrap(), Step::Wait);
        }
        let Step::Done(rebuilt) = recovery.reply(ok(&[])).unwrap() else {
            panic!("done");
        };
        assert_eq!(rebuilt.output, b"held");
    }

    #[test]
    fn a_failed_command_fails_the_recovery() {
        let failed = Reply {
            ok: false,
            lines: vec![b"can't find pane: %7".to_vec()],
        };
        let (mut recovery, _) = Recovery::start(PaneId(7));
        assert_eq!(recovery.reply(failed.clone()).unwrap(), Step::Failed);
        let (mut recovery, _) = Recovery::paused(PaneId(7));
        assert_eq!(recovery.reply(ok(&[])).unwrap(), Step::Wait);
        assert_eq!(recovery.reply(failed).unwrap(), Step::Failed);
    }

    #[test]
    fn an_attach_reads_every_pane_then_turns_output_on() {
        let (mut attach, line) = Attach::new(vec![PaneId(1), PaneId(4)]);
        assert!(line.contains("-t '%1'") && line.contains("-t '%4'"));
        assert!(line.ends_with(" ; refresh-client -f '!no-output'\n"));
        let mut replies = replies(&values());
        replies.extend(self::replies(&values()));
        for reply in replies {
            assert_eq!(attach.reply(reply).unwrap(), Step::Wait);
        }
        let Step::Done(snapshots) = attach.reply(ok(&[])).unwrap() else {
            panic!("done");
        };
        let panes: Vec<_> = snapshots.iter().map(|snapshot| snapshot.pane).collect();
        assert_eq!(panes, [PaneId(1), PaneId(4)]);
        assert_eq!(attach.reply(ok(&[])), Err(Error::UnexpectedReply));
        let (mut attach, _) = Attach::new(vec![PaneId(1)]);
        let failed = Reply::default();
        assert_eq!(attach.reply(failed).unwrap(), Step::Failed);
    }

    #[test]
    fn modes_a_tmux_does_not_report_are_unknown() {
        let modes = with(&[]).unwrap().modes;
        assert_eq!(modes.extended_keys_csi_u, None);
        assert_eq!(
            with(&[("extended-keys-format", "xterm")])
                .unwrap()
                .modes
                .extended_keys_csi_u,
            Some(false)
        );
        assert_eq!(
            with(&[("extended-keys-format", "csi-u")])
                .unwrap()
                .modes
                .extended_keys_csi_u,
            Some(true)
        );
        assert_eq!(
            (
                modes.bracketed_paste,
                modes.extended_keys,
                modes.focus_events
            ),
            (None, None, None)
        );
        let modes = with(&[
            ("bracket_paste_flag", "1"),
            ("pane_key_mode", "Ext 2"),
            ("pane_private_modes", "7,25,1004"),
        ])
        .unwrap()
        .modes;
        assert_eq!(
            (
                modes.bracketed_paste,
                modes.extended_keys,
                modes.focus_events
            ),
            (Some(true), Some(ExtendedKeys::Level2), Some(true))
        );
        let modes = with(&[("pane_key_mode", "VT10x"), ("pane_private_modes", "7,25")])
            .unwrap()
            .modes;
        assert_eq!(
            (modes.extended_keys, modes.focus_events),
            (Some(ExtendedKeys::Off), Some(false))
        );
    }

    #[test]
    fn reads_the_alternate_screen_cursor_and_rejects_bad_values() {
        let state = with(&[
            ("alternate_on", "1"),
            ("alternate_saved_x", "3"),
            ("alternate_saved_y", "2"),
            ("cursor_x", "80"),
        ])
        .unwrap();
        assert_eq!(state.alternate_saved_cursor, Some((3, 2)));
        assert_eq!(state.cursor, (80, 0));
        for (name, bad) in [
            ("pane_width", ""),
            ("cursor_flag", "yes"),
            ("pane_tabs", "8,x"),
            ("pane_key_mode", "Ext 3"),
            ("pane_private_modes", "25,?"),
            ("bracket_paste_flag", "2"),
        ] {
            assert!(
                matches!(with(&[(name, bad)]), Err(Error::Format(_))),
                "{name}={bad}"
            );
        }
    }

    #[test]
    fn a_hook_on_a_recovery_command_breaks_recovery() {
        assert_eq!(
            hooks_command("work"),
            "show-hooks -g ; show-hooks -t 'work'\n"
        );
        let unset = [
            &b"after-capture-pane"[..],
            b"after-display-message",
            b"after-display-panes",
        ];
        assert!(!hooks_break_recovery(unset));
        assert!(!hooks_break_recovery([
            &b"after-display-panes[0] run-shell true"[..]
        ]));
        for set in [
            &b"after-capture-pane[0] run-shell true"[..],
            b"after-display-message[3] set -g @x 1",
            b"after-refresh-client[0] run-shell true",
        ] {
            assert!(
                hooks_break_recovery([set]),
                "{}",
                String::from_utf8_lossy(set)
            );
        }
    }
}
