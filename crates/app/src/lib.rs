//! The terminal path from a tmux control-mode client to a drawn grid, and
//! read-only library polling on a storage worker.
//!
//! The native desktop window manages saved layouts, terminal connections and
//! optional information views. A separate benchmark window measures the terminal path.

#![forbid(unsafe_code)]

pub mod artifacts;
pub mod desktop;
pub mod diagnostics;
pub mod keymap;
pub mod library;
pub mod panes;
mod restore;
pub mod roster;
mod seed;
pub mod sessions;
mod stream;
pub mod theme;
pub mod view;
