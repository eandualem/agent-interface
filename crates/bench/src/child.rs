//! A replay child: a process that replays one recording through one engine,
//! so that each engine's timings and peak memory are its own.
//!
//! The child writes `engine <name>` and `revision <the revision it was built
//! from>`, then one `run <nanoseconds>` line per
//! timed replay, then `pane_bytes`, `reads`, `copied` and `history` (lines
//! kept at the end) for one replay and `peak_rss` in bytes, to its standard
//! output. It writes the last replay's
//! final screen, in the comparison format, to the `--screen` file.
//!
//! `agent-interface-bench-rio` includes this file.

use std::fs;

use agent_interface_tmux::control::PaneId;

use crate::display::Screen;
use crate::dump::dump;
use crate::engine::{Engine, GRID};
use crate::memory;
use crate::options::Options;
use crate::replay::{self, Schedule};

/// Runs a replay child for engine `E` with the parent's `options`: `--wire`,
/// `--pane`, `--chunk`, `--read-every`, `--warmup`, `--runs` and `--screen`.
pub fn run<E: Engine>(options: &Options) -> Result<(), String> {
    let wire_path = options.required("wire")?;
    let wire = fs::read(wire_path).map_err(|error| format!("{wire_path}: {error}"))?;
    let pane = PaneId(options.get("pane", 0)?);
    let schedule = Schedule {
        chunk: options.get("chunk", 64 * 1024)?,
        read_every: options.get("read-every", 0)?,
    };
    if schedule.chunk == 0 || schedule.read_every == 0 {
        return Err("--chunk and --read-every must be above zero".to_owned());
    }
    let warmup: usize = options.get("warmup", 2)?;
    let runs: usize = options.get("runs", 10)?;
    if runs == 0 {
        return Err("--runs must be above zero".to_owned());
    }

    println!("engine {}", E::NAME);
    println!("revision {}", env!("BENCH_REVISION"));
    let mut screen = Screen::default();
    let mut last = None;
    for run in 0..warmup + runs {
        let replay = replay::replay::<E>(&wire, pane, GRID, schedule, &mut screen)
            .map_err(|error| error.to_string())?;
        if run >= warmup {
            println!("run {}", replay.elapsed.as_nanos());
        }
        last = Some(replay);
    }
    let last = last.expect("at least one replay ran");
    println!("pane_bytes {}", last.pane_bytes);
    println!("reads {}", last.reads);
    println!("copied {}", last.copied);
    println!("history {}", last.history);
    println!("peak_rss {}", memory::peak_resident_bytes().unwrap_or(0));
    let screen_path = options.required("screen")?;
    fs::write(screen_path, dump(&screen)).map_err(|error| format!("{screen_path}: {error}"))
}
