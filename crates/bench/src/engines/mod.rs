//! The engine this program measures: `libghostty-vt` through the `term`
//! crate, the path the app uses.

use agent_interface_term::{Screen, Terminal};

/// `libghostty-vt`, through the `term` crate.
pub struct Ghostty(Terminal);

impl Ghostty {
    /// The name the benchmark reports the engine by.
    pub const NAME: &'static str = "libghostty-vt";

    /// Creates a terminal of `cols` by `rows` cells that keeps up to
    /// `scrollback` lines of history.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        Self(Terminal::new(cols, rows, scrollback).expect("create a libghostty-vt terminal"))
    }

    /// Feeds pane output to the terminal.
    pub fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    /// Copies the visible screen into `screen`, replacing what it held.
    pub fn read_screen(&mut self, screen: &mut Screen) {
        self.0
            .read_screen(screen)
            .expect("read the libghostty-vt screen");
    }

    /// Returns the number of lines of history the terminal holds.
    pub fn history(&self) -> usize {
        self.0.history().expect("read the libghostty-vt history")
    }
}
