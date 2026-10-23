# Run benchmarks

Use the benchmark harness to measure terminal processing, output presentation,
typing latency, frame pacing and tmux state recovery. It creates disposable tmux
servers and never attaches to existing sessions. These measurements are for
contributors investigating performance; a result from one machine is not a
product-wide performance guarantee.

Complete the [build setup](../CONTRIBUTING.md#set-up) first. Keep other builds
stopped while measuring, use the same build configuration for every target,
and record the revision, OS, hardware, tmux version and display refresh rate.

## Build the harness

```sh
python3 scripts/prepare-native.py
LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast cargo build --release --locked -p tmux-workspaces-bench -p tmux-workspaces-bench-rio
```

The native engine uses `ReleaseFast` for measurements. Keep the default
`baseline` CPU target when comparing portable builds. The two benchmark
executables must come from the same revision.

## Headless measurements

```sh
./target/release/tmux-workspaces-bench gate1 --out results/headless
./target/release/tmux-workspaces-bench gate5 --out results/recovery
```

The command names identify these measurements:

| Command | What it measures |
| --- | --- |
| `gate1` | tmux output delivery and terminal processing of deterministic ASCII, Unicode, color and scrolling payloads, with final-screen comparisons |
| `gate5` | State capture and recovery compared with an ordinary tmux client, including attach, pause, resize and exit scenarios |

Run recovery measurements separately for each tmux version you want to check,
putting that version first on `PATH` and using a different output directory.
Partial scenario or fixture selections do not establish full recovery coverage.

## Measurements with a display

The display measurements run on macOS. They need Ghostty installed at
`/Applications/Ghostty.app`, Screen Recording and Accessibility permission for
the harness's launcher, and an awake, unlocked display. The harness opens its
own measured windows and stops if they are covered or lose the required focus.
Do not type or click while a run is active.

Build the benchmark window with the same configuration as the harness:

```sh
LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast cargo build --release --locked -p tmux-workspaces-app --example bench-window
```

The bulk-output measurement also uses a pinned vtebench checkout. Prepare it
outside this repository:

```sh
git clone https://github.com/alacritty/vtebench
git -C vtebench checkout ead80032e57dee2e75f0b51f2ea67528647d9944
cargo build --release --locked --manifest-path vtebench/Cargo.toml
```

Then run:

```sh
./target/release/tmux-workspaces-bench gate2 --vtebench /path/to/vtebench --out results/output
./target/release/tmux-workspaces-bench gate3 --out results/typing
./target/release/tmux-workspaces-bench gate4 --out results/frames
```

`gate2` measures bulk output and vtebench write throughput. `gate3` measures the
time from a posted key to the first changed frame, with idle and busy panes.
`gate4` measures changed frames and their intervals during animation and
scrolling. The harness launches a separate Ghostty instance with isolated
configuration for comparison.

## Interpret and share results

Each run writes a summary and raw samples to the selected directory. Retain
both, along with the full command and environment. Repeat measurements under
comparable load rather than relying on one run. Investigate screen differences
before treating a faster result as an improvement.

Write throughput, terminal processing and displayed-frame timing measure
different parts of the system. tmux's behavior differs between ordinary and
control-mode clients, so write throughput alone does not establish what reached
the screen. Display capture timestamps also exclude the panel's scan-out and
pixel response. Close latency comparisons need an independent display-level
measurement, such as a camera.

Run `tmux-workspaces-bench` without arguments for the available options. Keep
raw run output outside tracked documentation; include the relevant measurements
and their limitations with a performance-related pull request.
