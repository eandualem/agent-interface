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
#[cfg(target_os = "macos")]
mod macos;
mod memory;
mod options;
mod payload;
mod producers;
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
  --keep DIR          keep the payloads, recordings and final screens in DIR

usage: tmux-workspaces-bench gate2|gate3|gate4 --out DIR [options]   (macOS)
  display gates of decision record 0018; each measured window must stay fully
  visible, and the program running this needs Screen Recording and
  Accessibility (posting events) access
  --targets LIST      app,ghostty,ghostty-tmux (gate2: app,ghostty-tmux;
                      gate3: all three; gate4: app,ghostty)
  --runs N            timed runs per target (gate2: 5; gate3, gate4: 1)
  --window PATH       the bench-window example (examples/bench-window next to
                      this program)
  --ghostty PATH      Ghostty's app bundle (/Applications/Ghostty.app); each run
                      starts a separate instance with its own configuration
  --origin X,Y        top left of every measured window, in points (40,60);
                      the window must lie on the main display
  --refresh HZ        the display's refresh rate, when it reports none
  gate2: --payloads LIST (all four), --size BYTES (20000000), --seed N (1),
         --warmup N (1); vtebench is not run yet
  gate3: --keys N (40), --warmup-keys N (5), --conditions idle,busy, --seed N (1)
  gate4: --seconds S (30), --scenarios fire,scroll
  a run with fewer payloads, conditions, scenarios, keys, bytes or seconds
  than the gate needs is an exploratory subset: it can fail the gate but not
  pass it

usage: tmux-workspaces-bench fire|scroll-test [--seconds S] [--seed N] [--ready FILE]
  output the display gates run in the measured pane; --ready creates FILE
  once the first output is written; fire also takes --cols N --rows N (the
  terminal's size)

usage: tmux-workspaces-bench probe-display --pid PID [options]   (macOS)
  lists the windows of process PID and captures one, printing frame intervals
  --seconds N         how long to capture (5)
  --window ID         the window of PID to capture (its first one on screen)";

/// Stops the programs the display gates started that are not children of
/// this process, before an interrupt stops the tmux servers.
fn stop_started_programs() {
    #[cfg(target_os = "macos")]
    macos::targets::stop_all_opened();
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The gates start private tmux servers, which an interrupt must stop too.
    if args
        .first()
        .is_some_and(|command| command.starts_with("gate"))
        && let Err(error) =
            tmux_workspaces_test_support::stop_servers_when_interrupted(stop_started_programs)
    {
        eprintln!("cannot handle interrupts: {error}");
        return ExitCode::FAILURE;
    }
    let result = match args.first().map(String::as_str) {
        Some("gate1") => Options::parse(&args[1..]).and_then(|options| gate1::run(&options)),
        Some("replay") => {
            Options::parse(&args[1..]).and_then(|options| gate1::replay_child(&options))
        }
        Some("fire") => Options::parse(&args[1..]).and_then(|options| producers::fire(&options)),
        Some("scroll-test") => {
            Options::parse(&args[1..]).and_then(|options| producers::scroll_test(&options))
        }
        #[cfg(target_os = "macos")]
        Some("gate2") => Options::parse(&args[1..]).and_then(|options| macos::gate2::run(&options)),
        #[cfg(target_os = "macos")]
        Some("gate3") => Options::parse(&args[1..]).and_then(|options| macos::gate3::run(&options)),
        #[cfg(target_os = "macos")]
        Some("gate4") => Options::parse(&args[1..]).and_then(|options| macos::gate4::run(&options)),
        #[cfg(target_os = "macos")]
        Some("probe-display") => {
            Options::parse(&args[1..]).and_then(|options| macos::probe::run(&options))
        }
        _ => Err(String::new()),
    };
    // An interrupt may still be stopping what a gate started.
    tmux_workspaces_test_support::wait_if_interrupted();
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
