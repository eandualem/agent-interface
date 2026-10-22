//! Display measurements for gates 2 to 4, on macOS: capturing one window's
//! frames with their display times, and typing into one process.
//!
//! Everything here acts on a process the harness started and names by its
//! process ID: it never captures, or posts events to, any other window.

pub mod capture;
pub mod clock;
pub mod displays;
pub mod frames;
pub mod gate2;
pub mod gate3;
pub mod gate4;
pub mod keys;
pub mod pixels;
pub mod probe;
pub mod report;
pub mod targets;
pub mod vtebench;
pub mod windows;
