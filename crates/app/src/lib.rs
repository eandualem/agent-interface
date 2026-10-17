//! The parts of the tmux-workspaces app that exist so far: the terminal path
//! from a tmux control-mode client to a drawn grid.
//!
//! The milestone-0 benchmark window (`examples/bench-window.rs`) measures this
//! path before any product interface is built
//! (`docs/decisions/0018-benchmark-window-and-display-measurement.md`).

#![forbid(unsafe_code)]

pub mod panes;
pub mod view;
