//! The engine this program measures: `libghostty-vt` through the `term`
//! crate, the path the app uses.

use agent_interface_term::{Screen, Terminal};

use crate::engine::Engine;

/// `libghostty-vt`, through the `term` crate.
pub struct Ghostty(Terminal);

impl Engine for Ghostty {
    const NAME: &'static str = "libghostty-vt";

    fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        Self(Terminal::new(cols, rows, scrollback).expect("create a libghostty-vt terminal"))
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    fn read_screen(&mut self, screen: &mut Screen) {
        self.0
            .read_screen(screen)
            .expect("read the libghostty-vt screen");
    }

    fn history(&self) -> usize {
        self.0.history().expect("read the libghostty-vt history")
    }
}
