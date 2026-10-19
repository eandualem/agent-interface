# 0022. vtebench in gate 2

- **Status:** Proposed. Waits on the agreement and the run described under
  **Accept when**.
- **Date:** 2026-10-19

## Context

Gate 2 of [0018](0018-milestone-0-benchmark-method-revised.md) has two
measurements in the app and in Ghostty running tmux: the marker probe, and
"vtebench, built from a pinned commit, runs inside the tmux pane and reports
its own timings". Its pass rule is "at least 0.95 times Ghostty with tmux on
every case, with the final frame no more than one refresh interval later".
0018 names no commit, no settings and no way to read vtebench's timings, and
until now the harness ran only the marker probe, so gate 2 could not pass.

Observed in [vtebench](https://github.com/alacritty/vtebench) at commit
`ead80032e57dee2e75f0b51f2ea67528647d9944`:

- For each sample, vtebench writes a terminal reset (`ESC c`) and the
  benchmark's setup, then times the write and flush of the benchmark's output
  to its standard output: how long those writes block. Samples are whole
  milliseconds, truncated.
- It repeats a benchmark's output to at least `--min-bytes` bytes per sample
  (1 MiB by default), and takes samples until `--max-secs` seconds have
  passed (10 by default), after `--warmup` samples (1 by default).
- `--dat FILE` writes the samples: a line of benchmark names, then one line
  per sample, with `_` where a benchmark has no more samples. The file does
  not record the bytes per sample. `--silent` leaves out the summary it
  would otherwise print to the terminal being measured.
- A benchmark whose script prints nothing is left out of the results without
  a message, and a `.dat` file that cannot be written is reported on standard
  error while vtebench still exits successfully.
- Its twelve default benchmarks size their output to the terminal with
  `tput cols` and `tput lines`, so both targets write the same bytes in the
  200×50 session.
- Its newest tag, `v0.3.1`, is from September 2020. The 19 commits since add
  the `medium_cells` and `sync_medium_cells` benchmarks and fix how the
  scripts read the terminal's size.

Observed in tmux's source, in releases 3.3a, 3.4, 3.5a, 3.6b and 3.8:

- `server_client_check_pane_buffer` in `server-client.c` stops reading a
  pane only when it has attached clients and every one of them is a control
  client that cannot take more: one with at least `CONTROL_BUFFER_LOW` (512
  bytes) of output pending, or, from 3.4, one that has turned the pane's
  output off. Any ordinary client keeps the pane being read.
- `tty_block_maybe` in `tty.c`: once an ordinary client has more than 8
  bytes per cell of its terminal pending, tmux discards that output, stops
  sending to the client, and redraws the whole client once less than an
  eighth of a byte per cell is discarded in 100 ms.

So vtebench's times inside tmux depend on which client is attached. With
Ghostty running tmux, tmux keeps reading the pane however far Ghostty falls
behind, and the times are mostly tmux's own. With the app, whose control
client stops tmux reading the pane when it falls behind, they are the slower
of tmux and the app.

## Options considered

Which commit:

| Option | Trade-offs |
| --- | --- |
| The newest tag, `v0.3.1` | A release, but from 2020: it lacks two of today's benchmarks and the fix to their terminal size. |
| **The newest commit on the default branch, `ead8003`** (9 January 2025, "Bump dependencies") | Every current benchmark and fix. Not a release, so the commit is pinned by its full hash. |

How vtebench reaches the harness:

| Option | Trade-offs |
| --- | --- |
| The harness clones and builds it | Nothing to prepare, but the gate needs a network and builds during the measurement, which 0018 rules out for acceptance runs. |
| Copy its source or scripts into this repository | Fixed with the harness, but it is another project's code to keep and license here. |
| **The contributor builds a checkout once; the harness checks it** | One clone and build, documented in the [benchmark guide](../benchmarks/README.md). The harness checks the commit, the working tree and the build, but cannot check that the build came from the tree as it is now. |

How its times are read and judged:

| Option | Trade-offs |
| --- | --- |
| Read the summary vtebench prints | It prints into the terminal being measured, which would then have to be read back from the screen. |
| **Read the `.dat` file** | Every sample, machine-readable. It has no byte counts, but both targets write the same bytes per sample, so a ratio of times is the ratio of rates. |
| Judge each case like the marker probe, with its camera rule and final-frame limit | vtebench's times are write-blocking times, not display times, and it ends on no frame of its own. |
| **Compare median times plainly against 0.95** | Matches what vtebench measures. Says nothing about frames, which the marker probe covers. |

## Decision

**The pin and its licence.**

| Program | Commit | Licence | Reason |
| --- | --- | --- | --- |
| vtebench | `ead80032e57dee2e75f0b51f2ea67528647d9944` (version 0.3.1) | MIT OR Apache-2.0, dual (`MIT/Apache-2.0` in its manifest, with both licence files) | The benchmark 0018 names for gate 2. It runs as a separate program from its own checkout, and is never linked into, or copied into, this project. |

Its own dependencies (`clap` 4.5.24 and `walkdir` 2.5.0 in its lock file)
belong to its build, not to this workspace's dependency graph.

**Building and checking it.** The benchmark guide clones vtebench, checks out
the pinned commit and builds it with `cargo build --release --locked`. Gate 2
takes the checkout with `--vtebench DIR`. Before any window opens, the harness
checks that `git rev-parse HEAD` in it is the pinned commit, that
`git status --porcelain` prints nothing, and that `target/release/vtebench`
exists, and otherwise stops with how to build it. Without `--vtebench`, gate
2 runs the marker probe alone and its report is an exploratory subset.

**Running it.** After the marker probe, each run of each target gets a new
private server and a new window, as the marker probe does. The pane runs a
script that waits on a FIFO, which the harness opens once the window is
shown, then runs
`vtebench -b benchmarks --dat FILE --silent --min-bytes 8388608 --max-secs 10`
in the checkout, with its standard error kept in a file, writes vtebench's
exit status and renames the samples to their final name. The benchmark path
is relative because some benchmark scripts find their data with an unquoted
`dirname $0`, which a checkout path with spaces would break. While it waits
for that file, the harness checks once a second that the window is still
shown, since a hidden window stops drawing; the window is not captured. A
run that exits unsuccessfully, writes anything to standard error, or leaves a
`.dat` file that does not parse stops the gate, since vtebench exits
successfully even when it cannot write its samples.

**Settings.** Each one below its value makes the run an exploratory subset:

- `--min-bytes 8388608` (`--vtebench-bytes`). Samples are truncated to whole
  milliseconds; a sample of 8 MiB takes at least 50 ms at any rate below
  167 MB/s, which keeps that error under 2%. tmux's own rate on the
  benchmark machine was 10 to 47 MB/s in [gate 1](../benchmarks/gate-1.md).
  vtebench's default of 1 MiB would leave a fast case with only a few
  milliseconds per sample.
- `--max-secs 10` (`--vtebench-secs`), vtebench's default, and its default
  warm-up sample. A run of the twelve cases then takes a few minutes.
- Two runs per target (`--vtebench-runs`), alternating which target goes
  first, so each runs both first and second.

Every one of the twelve cases must be present in every run's `.dat` file;
a case missing from one also makes the run an exploratory subset, because
vtebench leaves out a failing benchmark silently.

**Judging.** For each case, each target's samples are pooled over its runs.
The case passes when the app's median rate is at least 0.95 times Ghostty
with tmux's, that is, when Ghostty with tmux's median time divided by the
app's is at least 0.95, and fails otherwise. vtebench's times are not display
times, so the camera rule of 0018 does not apply to them. A median of 0 ms is
below vtebench's resolution and leaves the case undecided. Gate 2 passes only
when every marker probe case and every vtebench case passes, with full
coverage, in a distributable build.

**The final-frame limit, as proposed here.** 0018's pass rule ends "with the
final frame no more than one refresh interval later". vtebench has no final
frame: it stops timing when its writes return, and resets the terminal after
each case. This record proposes reading that limit as checked by the marker
probe, which ends on a frame filled with its marker colour, and judging
vtebench on its rates alone. This is a reading of 0018, not a change to it;
if the maintainer reads 0018 otherwise, vtebench needs a different method in
a record that supersedes 0018.

## Consequences

- Gate 2 can pass. A full run adds four vtebench runs of a few minutes each
  to the marker probe.
- Running gate 2 needs a vtebench checkout, built once with a network.
- The vtebench case compares the app with mostly tmux's own rate, so it is a
  stricter test for the app than for Ghostty with tmux. A failure can come
  from how fast the app's control client reads, as well as from drawing.
- Two benchmark setup scripts use brace expansion under `#!/bin/sh`, which
  works where `sh` is bash, as on macOS by default. Display gates on Linux
  would need to check them with that system's `sh`.

## Accept when

The maintainer agrees with the reading of 0018's final-frame limit above, and
a gate-2 run in a display slot, a quiet period on the benchmark machine set
aside for display measurements, has run every vtebench case in both targets.

## Evidence

- vtebench at
  [`ead8003`](https://github.com/alacritty/vtebench/tree/ead80032e57dee2e75f0b51f2ea67528647d9944):
  `Benchmark::run` and `run_sample` in `src/bench.rs` for what a sample
  times, `BenchmarkLoader::load` and the `filter_map` in `src/main.rs` for
  benchmarks left out, `src/format/dat.rs` for the `.dat` format,
  `src/cli.rs` for the options and their defaults, and `Cargo.toml`,
  `LICENSE-MIT` and `LICENSE-APACHE` for the licence.
- tmux 3.3a, 3.4, 3.5a, 3.6b and 3.8 release sources:
  `server_client_check_pane_buffer` in `server-client.c`,
  `control_pane_offset` and `CONTROL_BUFFER_LOW` in `control.c`, and
  `tty_block_maybe`, `tty_timer_callback` and `TTY_BLOCK_START` in `tty.c`.
- tmux's rates on the benchmark machine: [gate-1 results](../benchmarks/gate-1.md).
