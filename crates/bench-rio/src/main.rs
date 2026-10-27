//! The milestone-0 benchmark's replay child for `rio-vt`, a comparison engine
//! measured against `libghostty-vt`.
//!
//! It is its own program because `rio-vt` and `libghostty-vt` both contain
//! the simdutf C++ library, and a program that links both crashes. So that
//! `rio-vt` runs exactly the replay the other engines run, this program
//! includes the harness's own source files, and the `term` crate's display
//! data, rather than copying them. `agent-interface-bench gate1` starts it;
//! the protocol is described in `crates/bench/src/child.rs`.

#![deny(unsafe_code)]

#[path = "../../bench/src/child.rs"]
mod child;
#[path = "../../term/src/screen.rs"]
#[allow(dead_code)] // This renderer benchmark does not use native selection metadata.
mod display;
#[path = "../../bench/src/dump.rs"]
mod dump;
#[path = "../../bench/src/engine.rs"]
mod engine;
#[path = "../../bench/src/memory.rs"]
mod memory;
#[path = "../../bench/src/options.rs"]
mod options;
#[path = "../../bench/src/replay.rs"]
mod replay;
mod rio;

#[cfg(test)]
#[path = "../../bench/src/engines/alacritty.rs"]
mod alacritty;
#[cfg(test)]
#[path = "../../bench/src/engines/common.rs"]
mod common;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match options::Options::parse(&args).and_then(|options| child::run::<rio::Rio>(&options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
