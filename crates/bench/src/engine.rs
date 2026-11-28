//! The grid every run uses.

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
