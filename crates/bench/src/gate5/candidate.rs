//! Gate 5's candidate: a pane rebuilt by the app's recovery code on a control
//! connection, then fed the pane's output that follows the rebuild.
//!
//! The conversion from the `tmux` crate's snapshot to the `term` crate's
//! restore lives here, as the app's will (decision record 0006: neither crate
//! depends on the other).

use tmux_workspaces_term::restore::{self, Alternate, Restore};
use tmux_workspaces_term::{Attributes, Color, Style, Terminal, Underline};
use tmux_workspaces_tmux::capture;
use tmux_workspaces_tmux::control::PaneId;
use tmux_workspaces_tmux::recovery::{self, Attach, Rebuilt, Recovery, Snapshot, Step};

use super::connection::{Connection, Event, TIMEOUT};

/// The lines of history a rebuilt terminal keeps: tmux's default limit.
const SCROLLBACK: usize = 2_000;

/// A pane rebuilt from a snapshot, kept up to date with its output.
pub struct Candidate {
    /// The pane's terminal.
    pub terminal: Terminal,
    /// The size the pane was rebuilt at.
    pub size: (u16, u16),
    pane: PaneId,
    /// The index of the connection's next event to apply.
    next: usize,
    /// Every byte written to the terminal after the rebuild, in order.
    pub applied: Vec<u8>,
}

impl Candidate {
    /// Rebuilds a pane from `snapshot`, applies `output`, the pane's output
    /// after the snapshot, and takes later output from the connection's
    /// event `next` on.
    pub fn rebuilt(snapshot: &Snapshot, output: &[u8], next: usize) -> Result<Self, String> {
        let restore = restore(snapshot);
        let mut terminal =
            Terminal::restored(&restore, SCROLLBACK).map_err(|error| error.to_string())?;
        terminal.write(output);
        Ok(Self {
            terminal,
            size: (restore.width, restore.height),
            pane: snapshot.pane,
            next,
            applied: output.to_vec(),
        })
    }

    /// Returns the pane.
    pub fn pane(&self) -> PaneId {
        self.pane
    }

    /// Applies the pane's output that has arrived since the last call.
    pub fn catch_up(&mut self, connection: &Connection) {
        let events = connection.events_from(self.next);
        self.next += events.len();
        for event in events {
            if let Event::Output(pane, data) = event
                && pane == self.pane
            {
                self.terminal.write(&data);
                self.applied.extend_from_slice(&data);
            }
        }
    }
}

/// Runs `recovery` to its end. Its first command was sent when the
/// connection's next event was `replies_from`, and the pane's output from
/// event `output_from` on belongs to it. Returns the rebuilt pane and the
/// index of the event after the last reply, or `None` if a command failed.
pub fn recover(
    connection: &Connection,
    output_from: usize,
    replies_from: usize,
    mut recovery: Recovery,
) -> Result<Option<(Rebuilt, usize)>, String> {
    let pane = recovery.pane();
    let mut index = output_from.min(replies_from);
    loop {
        let event = connection.next(&mut index, TIMEOUT)?;
        // `index` is now the index of the event after this one.
        match event {
            Event::Output(from, data) if from == pane && index > output_from => {
                recovery.output(&data);
            }
            Event::Reply(reply) if index > replies_from => {
                match recovery.reply(reply).map_err(|error| error.to_string())? {
                    Step::Send(line) => connection.send(&line)?,
                    Step::Wait => {}
                    Step::Done(rebuilt) => return Ok(Some((rebuilt, index))),
                    Step::Failed => return Ok(None),
                }
            }
            Event::Output(..) | Event::Reply(_) | Event::Notification(_) => {}
        }
    }
}

/// Runs `attach`, whose line was sent when the connection's next event was
/// `from`, to its end. Returns the panes' snapshots and the index of the
/// event after the last reply, or `None` if a command failed.
pub fn attach(
    connection: &Connection,
    from: usize,
    mut attach: Attach,
) -> Result<Option<(Vec<Snapshot>, usize)>, String> {
    let mut index = from;
    loop {
        match connection.next(&mut index, TIMEOUT)? {
            Event::Output(pane, _) => {
                return Err(format!(
                    "output from %{} before the attach line turned output on",
                    pane.0
                ));
            }
            Event::Reply(reply) => match attach.reply(reply).map_err(|error| error.to_string())? {
                Step::Send(line) => connection.send(&line)?,
                Step::Wait => {}
                Step::Done(snapshots) => return Ok(Some((snapshots, index))),
                Step::Failed => return Ok(None),
            },
            Event::Notification(_) => {}
        }
    }
}

/// Converts a snapshot into the terminal state it describes.
pub fn restore(snapshot: &Snapshot) -> Restore {
    let state = &snapshot.state;
    let modes = &state.modes;
    Restore {
        width: state.width,
        height: state.height,
        rows: snapshot.rows.iter().map(row).collect(),
        history: state.history,
        alternate: state.alternate_saved_cursor.map(|saved_cursor| Alternate {
            saved_rows: snapshot.saved.iter().map(row).collect(),
            saved_cursor,
        }),
        cursor: state.cursor,
        cursor_visible: state.cursor_visible,
        scroll_region: state.scroll_region,
        tab_stops: state.tab_stops.clone(),
        modes: restore::Modes {
            cursor_keys: modes.cursor_keys,
            keypad: modes.keypad,
            insert: modes.insert,
            wrap: modes.wrap,
            origin: modes.origin,
            mouse_standard: modes.mouse_standard,
            mouse_button: modes.mouse_button,
            mouse_all: modes.mouse_all,
            mouse_utf8: modes.mouse_utf8,
            mouse_sgr: modes.mouse_sgr,
            bracketed_paste: modes.bracketed_paste,
            extended_keys: modes.extended_keys.map(|keys| match keys {
                recovery::ExtendedKeys::Off => restore::ExtendedKeys::Off,
                recovery::ExtendedKeys::Level1 => restore::ExtendedKeys::Level1,
                recovery::ExtendedKeys::Level2 => restore::ExtendedKeys::Level2,
            }),
            focus_events: modes.focus_events,
        },
        pending: snapshot.pending.clone(),
    }
}

fn row(row: &capture::Row) -> restore::Row {
    restore::Row {
        segments: row
            .segments
            .iter()
            .map(|segment| restore::Segment {
                style: style(&segment.style),
                underline_color: color(segment.style.underline_color),
                link: segment.link.as_ref().map(|link| restore::Link {
                    params: link.params.clone(),
                    uri: link.uri.clone(),
                }),
                line_drawing: segment.line_drawing,
                text: segment.text.clone(),
            })
            .collect(),
    }
}

fn style(style: &capture::Style) -> Style {
    let mut attributes = Attributes::default();
    for (from, to) in [
        (capture::Attributes::BOLD, Attributes::BOLD),
        (capture::Attributes::FAINT, Attributes::FAINT),
        (capture::Attributes::ITALIC, Attributes::ITALIC),
        (capture::Attributes::BLINK, Attributes::BLINK),
        (capture::Attributes::INVERSE, Attributes::INVERSE),
        (capture::Attributes::INVISIBLE, Attributes::INVISIBLE),
        (
            capture::Attributes::STRIKETHROUGH,
            Attributes::STRIKETHROUGH,
        ),
        (capture::Attributes::OVERLINE, Attributes::OVERLINE),
    ] {
        if style.attributes.contains(from) {
            attributes.insert(to);
        }
    }
    Style {
        foreground: color(style.foreground),
        background: color(style.background),
        underline: match style.underline {
            capture::Underline::None => Underline::None,
            capture::Underline::Single => Underline::Single,
            capture::Underline::Double => Underline::Double,
            capture::Underline::Curly => Underline::Curly,
            capture::Underline::Dotted => Underline::Dotted,
            capture::Underline::Dashed => Underline::Dashed,
        },
        attributes,
    }
}

fn color(color: capture::Color) -> Color {
    match color {
        capture::Color::Default => Color::Default,
        capture::Color::Palette(index) => Color::Palette(index),
        capture::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
