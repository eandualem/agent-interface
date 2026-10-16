//! Terminal state for one tmux pane, kept by the terminal engine behind this
//! crate's own types.
//!
//! The engine is `libghostty-vt`
//! (`docs/decisions/0004-terminal-emulation-core.md`). No engine type appears
//! in this crate's public API, so the engine can be replaced without changing
//! other crates. This crate depends on no other project crate.
//!
//! Engine handles are not `Send`: construct, use and drop a [`Terminal`] on
//! the thread that owns it
//! (`docs/decisions/0007-threads-ownership-and-event-ordering.md`).

#![forbid(unsafe_code)]

use libghostty_vt::terminal::{Options, Terminal as Engine};

/// An error reported by the terminal engine.
#[derive(Debug, thiserror::Error)]
#[error("terminal engine error: {0}")]
pub struct Error(libghostty_vt::Error);

/// The terminal state of one pane.
pub struct Terminal {
    engine: Engine<'static, 'static>,
}

impl Terminal {
    /// Creates a terminal of `cols` by `rows` cells that keeps up to
    /// `scrollback` lines of history.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Result<Self, Error> {
        let engine = Engine::new(Options {
            cols,
            rows,
            max_scrollback: scrollback,
        })
        .map_err(Error)?;
        Ok(Self { engine })
    }

    /// Feeds pane output to the terminal, in the order the pane produced it.
    pub fn write(&mut self, bytes: &[u8]) {
        self.engine.vt_write(bytes);
    }

    /// Returns the cursor's column and row, counted from zero at the top left
    /// of the screen.
    pub fn cursor(&self) -> Result<(u16, u16), Error> {
        let column = self.engine.cursor_x().map_err(Error)?;
        let row = self.engine.cursor_y().map_err(Error)?;
        Ok((column, row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_text_moves_the_cursor() {
        let mut terminal = Terminal::new(80, 24, 100).unwrap();
        terminal.write(b"hello");
        assert_eq!(terminal.cursor().unwrap(), (5, 0));
        terminal.write(b"\r\nab");
        assert_eq!(terminal.cursor().unwrap(), (2, 1));
    }
}
