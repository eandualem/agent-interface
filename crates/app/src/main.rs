//! The tmux-workspaces desktop app.
//!
//! There is no window yet: the terminal path is measured first, against the
//! milestone-0 gates in `docs/decisions/0013-milestone-0-benchmark-method.md`.

#![forbid(unsafe_code)]

fn main() {
    println!(
        "tmux-workspaces {}: no window yet, see docs/ARCHITECTURE.md",
        env!("CARGO_PKG_VERSION")
    );
}
