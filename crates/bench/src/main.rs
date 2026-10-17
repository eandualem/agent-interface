//! The milestone-0 benchmark harness
//! (`docs/decisions/0013-milestone-0-benchmark-method.md`).
//!
//! A development tool, never shipped. Every tmux process it starts runs on a
//! private server through `tmux-workspaces-test-support`. See
//! `docs/benchmarks/README.md` for how to run it.

#![deny(unsafe_code)]

mod child;
mod compare;
mod dump;
mod engine;
mod engines;
mod environment;
mod gate1;
mod memory;
mod options;
mod payload;
mod record;
mod replay;
mod stats;

/// The owned display data the engines fill, from the `term` crate.
mod display {
    pub use tmux_workspaces_term::{Attributes, Color, Cursor, Screen, Style, Underline, Width};
}

use std::process::ExitCode;

use options::Options;

const USAGE: &str = "usage: tmux-workspaces-bench gate1 --out DIR [options]
  --size BYTES        bytes of each payload (20000000)
  --seed N            payload seed (1)
  --payloads LIST     comma-separated: ascii,unicode,color,scroll (all)
  --engines LIST      comma-separated: libghostty-vt,rio-vt,alacritty_terminal (all)
  --tmux-warmup N     tmux runs before timing (1)
  --tmux-runs N       timed tmux runs per payload (10)
  --engine-warmup N   replays before timing (2)
  --engine-runs N     timed replays per engine and payload (10)
  --chunk BYTES       control-wire bytes per decode call (65536)
  --frame-rate HZ     display refresh the read-out schedule models (120)
  --keep DIR          keep the payloads, recordings and final screens in DIR";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("gate1") => Options::parse(&args[1..]).and_then(|options| gate1::run(&options)),
        Some("replay") => {
            Options::parse(&args[1..]).and_then(|options| gate1::replay_child(&options))
        }
        _ => Err(String::new()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("{message}");
            }
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}
