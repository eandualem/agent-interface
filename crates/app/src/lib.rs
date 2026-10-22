//! The terminal path from a tmux control-mode client to a drawn grid, and
//! read-only library polling on a storage worker.
//!
//! The native desktop window browses saved layouts and saves workspace/tab renames. The separate
//! milestone-0 benchmark window measures the terminal path.

#![forbid(unsafe_code)]

pub mod desktop;
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
