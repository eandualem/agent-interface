//! The terminal path from a tmux control-mode client to a drawn grid, and
//! read-only library polling on a storage worker.
//!
//! The milestone-0 benchmark window (`examples/bench-window.rs`) measures this
//! path before any product interface is built
//! (`docs/decisions/0019-benchmark-window.md`).

#![forbid(unsafe_code)]

pub mod library;
pub mod panes;
mod restore;
pub mod roster;
mod seed;
pub mod sessions;
mod stream;
pub mod view;
