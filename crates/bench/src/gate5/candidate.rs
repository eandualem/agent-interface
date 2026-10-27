//! Gate 5's candidate: a pane rebuilt by the app's recovery code on a control
//! connection, then fed the pane's output that follows the rebuild.
//!
//! The snapshot conversion is shared with the app. Neither the tmux nor
//! terminal crate depends on the other.

use std::time::Instant;

use agent_interface_term::Terminal;
use agent_interface_tmux::control::PaneId;
use agent_interface_tmux::recovery::{Attach, Rebuilt, Recovery, Snapshot, Step};

#[path = "../../../app/src/restore.rs"]
mod restore;
use restore::restore;
#[path = "../../../app/src/stream.rs"]
mod stream;

use super::connection::{Connection, Event, TIMEOUT};

/// The lines of history a rebuilt terminal keeps: tmux's default limit.
pub(super) const SCROLLBACK: usize = 2_000;

/// A pane rebuilt from a snapshot, kept up to date with its output.
pub struct Candidate {
    /// The pane's terminal.
    pub terminal: Terminal,
    /// The size the pane was rebuilt at.
    pub size: (u16, u16),
    pane: PaneId,
    /// The index of the connection's next event to apply.
    next: usize,
    /// Every delivered byte after the original rebuild, in order.
    pub applied: Vec<u8>,
    /// The latest snapshot, retained for resize diagnostics.
    pub seed: Snapshot,
    /// Bytes fed after that snapshot, retained for exact replay.
    pub replayed: Vec<u8>,
    // The engine cannot express an off-grid column, and its pending-wrap
    // transitions differ from tmux's. Keep a right-margin snapshot as the
    // authoritative state until a new capture permits streaming again.
    captured: bool,
    needs_capture: bool,
}

impl Candidate {
    /// Rebuilds a pane from `snapshot`, applies `output`, the pane's output
    /// after the snapshot, and takes later output from the connection's
    /// event `next` on.
    pub fn rebuilt(snapshot: &Snapshot, output: &[u8], next: usize) -> Result<Self, String> {
        let mut restore = restore(snapshot);
        let captured = restore.cursor.0 >= restore.width;
        if captured {
            restore.cursor = (0, 0);
            restore.cursor_visible = false;
            restore.pending.clear();
        }
        let mut terminal =
            Terminal::restored(&restore, SCROLLBACK).map_err(|error| error.to_string())?;
        let (fed, needs_capture) = if captured {
            (0, !output.is_empty())
        } else {
            stream::write_until_wrap(&mut terminal, output).map_err(|error| error.to_string())?
        };
        Ok(Self {
            terminal,
            size: (restore.width, restore.height),
            pane: snapshot.pane,
            next,
            applied: output.to_vec(),
            seed: snapshot.clone(),
            replayed: output[..fed].to_vec(),
            captured,
            needs_capture,
        })
    }

    /// Returns the pane.
    pub fn pane(&self) -> PaneId {
        self.pane
    }

    /// Applies the pane's output that has arrived since the last call.
    pub fn catch_up(&mut self, connection: &Connection) -> Result<(), String> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let events = connection.events_from(self.next);
            self.next += events.len();
            for event in events {
                if let Event::Output(pane, data) = event
                    && pane == self.pane
                {
                    if self.captured || self.needs_capture {
                        self.needs_capture |= !data.is_empty();
                    } else {
                        let (fed, needs_capture) =
                            stream::write_until_wrap(&mut self.terminal, &data)
                                .map_err(|error| error.to_string())?;
                        self.replayed.extend_from_slice(&data[..fed]);
                        self.needs_capture = needs_capture;
                    }
                    self.applied.extend_from_slice(&data);
                }
            }
            if !self.needs_capture {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("output did not settle during margin cursor recovery".to_owned());
            }
            let from = connection.mark();
            let (recovery, pause) = Recovery::start(self.pane, SCROLLBACK);
            connection.send(&pause)?;
            let (rebuilt, next) = recover(connection, from, from, recovery, deadline)?
                .ok_or("margin cursor recovery failed")?;
            // Keep the delivered-output audit continuous across the new
            // snapshot. Its boundary decides which bytes the new terminal
            // receives; this ledger never feeds the engine itself.
            for event in connection
                .events_from(self.next)
                .into_iter()
                .take(next - self.next)
            {
                if let Event::Output(pane, data) = event
                    && pane == self.pane
                {
                    self.applied.extend_from_slice(&data);
                }
            }
            let mut candidate = Self::rebuilt(&rebuilt.snapshot, &rebuilt.output, next)?;
            candidate.applied = std::mem::take(&mut self.applied);
            *self = candidate;
        }
    }

    /// The pane's logical cursor, not the ordinary client's drawing position.
    pub fn cursor(&self) -> Result<(u16, u16), String> {
        if self.needs_capture {
            return Err("the margin cursor needs a new snapshot".to_owned());
        }
        if self.captured {
            Ok(self.seed.state.cursor)
        } else {
            self.terminal.cursor().map_err(|error| error.to_string())
        }
    }

    /// The pane's cursor mode, including while its cursor is off-grid.
    pub fn cursor_enabled(&self) -> Result<bool, String> {
        if self.captured {
            Ok(self.seed.state.cursor_visible)
        } else {
            self.terminal
                .cursor_enabled()
                .map_err(|error| error.to_string())
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
    deadline: Instant,
) -> Result<Option<(Rebuilt, usize)>, String> {
    let pane = recovery.pane();
    let mut index = output_from.min(replies_from);
    loop {
        let wait = deadline
            .checked_duration_since(Instant::now())
            .filter(|wait| !wait.is_zero())
            .ok_or("timed out recovering the pane")?;
        let event = connection.next(&mut index, wait)?;
        if Instant::now() >= deadline {
            return Err("timed out recovering the pane".to_owned());
        }
        // `index` is now the index of the event after this one.
        match event {
            Event::Output(from, data) if from == pane && index > output_from => {
                recovery.output(&data);
            }
            Event::Reply(reply) if index > replies_from => {
                match recovery.reply(reply).map_err(|error| error.to_string())? {
                    Step::Send(line) => connection.send(&line)?,
                    Step::Wait => {}
                    Step::Done(rebuilt) => {
                        if Instant::now() >= deadline {
                            return Err("timed out recovering the pane".to_owned());
                        }
                        return Ok(Some((rebuilt, index)));
                    }
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
