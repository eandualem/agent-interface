//! The interface every terminal engine under measurement implements, and the
//! grid every run uses.
//!
//! `agent-interface-bench-rio` includes this file.

use crate::display::Screen;

/// The terminal size and history limit of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Lines of history an engine keeps.
    pub scrollback: usize,
}

/// The common terminal size for benchmark runs.
pub const GRID: Grid = Grid {
    cols: 200,
    rows: 50,
    scrollback: 10_000,
};

/// A terminal engine under measurement.
pub trait Engine {
    /// The name the benchmark reports the engine by.
    const NAME: &'static str;

    /// Creates a terminal of `cols` by `rows` cells that keeps up to
    /// `scrollback` lines of history.
    fn new(cols: u16, rows: u16, scrollback: usize) -> Self;

    /// Feeds pane output to the terminal.
    fn write(&mut self, bytes: &[u8]);

    /// Copies the visible screen into `screen`, replacing what it held.
    fn read_screen(&mut self, screen: &mut Screen);

    /// Returns the number of lines of history the terminal holds.
    fn history(&self) -> usize;
}
