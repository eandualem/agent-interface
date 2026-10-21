//! The terminal path from a tmux control-mode client to a drawn grid, and
//! read-only library polling on a storage worker.
//!
//! The native desktop window browses saved layouts read-only. The separate
//! milestone-0 benchmark window measures the terminal path.

#![forbid(unsafe_code)]

pub mod desktop;
pub mod library;
pub mod panes;
mod restore;
mod seed;
pub mod sessions;
mod stream;
pub mod view;
