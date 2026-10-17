//! Replaying a recording through the path the app takes from control-mode
//! output to display data: decoding, the terminal engine and the read-out.
//!
//! `tmux-workspaces-bench-rio` includes this file.

use std::time::{Duration, Instant};

use tmux_workspaces_tmux::control::{self, Decoder, Message, PaneId};

use crate::display::Screen;
use crate::engine::{Engine, Grid};

/// How a replay feeds the recording and reads display data out.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    /// The control-wire bytes handed to the decoder at a time, as one read
    /// from the client's pipe would.
    pub chunk: usize,
    /// The pane bytes fed to the engine between two read-outs.
    pub read_every: usize,
}

/// The result of one replay.
#[derive(Clone, Copy, Debug)]
pub struct Replay {
    /// The time from the first wire byte to the end of the final read-out.
    pub elapsed: Duration,
    /// The number of read-outs, the final one included.
    pub reads: usize,
    /// The bytes of display data all read-outs copied.
    pub copied: usize,
    /// The pane bytes fed to the engine.
    pub pane_bytes: usize,
    /// The lines of history the engine held at the end.
    pub history: usize,
}

/// Replays `wire` into a new `E` engine, reading the screen out on
/// `schedule` and once more after the last chunk, so the final output is
/// always read out. `screen` holds the final read-out.
pub fn replay<E: Engine>(
    wire: &[u8],
    pane: PaneId,
    grid: Grid,
    schedule: Schedule,
    screen: &mut Screen,
) -> Result<Replay, control::Error> {
    let mut engine = E::new(grid.cols, grid.rows, grid.scrollback);
    let mut decoder = Decoder::new();
    let mut since_read = 0;
    let mut reads = 0;
    let mut copied = 0;
    let mut pane_bytes = 0;
    let start = Instant::now();
    for piece in wire.chunks(schedule.chunk) {
        decoder.decode(piece, |message| {
            if let Message::Output { pane: from, data } = message
                && from == pane
            {
                engine.write(data);
                pane_bytes += data.len();
                since_read += data.len();
                if since_read >= schedule.read_every {
                    engine.read_screen(screen);
                    reads += 1;
                    copied += screen.size_in_bytes();
                    since_read = 0;
                }
            }
        })?;
    }
    engine.read_screen(screen);
    reads += 1;
    copied += screen.size_in_bytes();
    let elapsed = start.elapsed();
    let history = engine.history();
    // Freeing the engine is not part of the measured path.
    drop(engine);
    Ok(Replay {
        elapsed,
        reads,
        copied,
        pane_bytes,
        history,
    })
}
