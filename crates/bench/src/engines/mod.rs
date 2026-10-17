//! The engines this program measures: `libghostty-vt` through the `term`
//! crate, the path the app uses, and `alacritty_terminal` from decision
//! record 0004, adapted to the same owned display data.
//!
//! `rio-vt` runs in `tmux-workspaces-bench-rio` instead: it and
//! `libghostty-vt` both contain the simdutf C++ library, and a program that
//! links both crashes.

pub mod alacritty;
#[cfg(test)]
pub mod common;

use tmux_workspaces_term::{Screen, Terminal};

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::alacritty::Alacritty;

    #[test]
    fn alacritty_agrees_with_libghostty_vt_on_common_output() {
        for (input, bytes) in common::INPUTS {
            common::assert_same_screen(
                input,
                &common::read::<Ghostty>(bytes),
                &common::read::<Alacritty>(bytes),
            );
        }
    }
}
