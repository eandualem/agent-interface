# Benchmarks

The milestone-0 gates check, by measurement, that the chosen terminal path is
fast enough before any user interface is built. The method is decision record
[0018](../decisions/0018-milestone-0-benchmark-method-revised.md), which
restates [0013](../decisions/0013-milestone-0-benchmark-method.md) with
instruments for the display gates; gate 1 was measured under 0013, whose
gate-1 method 0018 keeps unchanged. The harness is described in
[0017](../decisions/0017-benchmark-harness.md), and the benchmark window of
gates 2 to 4 in [0019](../decisions/0019-benchmark-window.md).

| Gate | Results |
| --- | --- |
| 1. Headless headroom | Failed on escape-heavy coloured output: [gate-1.md](gate-1.md) |
| 2. End-to-end bulk output | Harness ready, not measured yet |
| 3. Typing latency | Harness ready, not measured yet |
| 4. Frame pacing | Harness ready, not measured yet |
| 5. Control-mode fidelity | Not measured yet |

## Running gate 1

Gate 1 needs tmux, the [build prerequisites](../../CONTRIBUTING.md#set-up) and
about 200 MB of temporary disk space. Build both benchmark programs in the
distributable configuration: release profile, the engine's core in Zig's
`ReleaseFast` mode and the portable `baseline` CPU target, which is the
default.

```sh
LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast \
  cargo build --release --locked -p tmux-workspaces-bench -p tmux-workspaces-bench-rio
./target/release/tmux-workspaces-bench gate1 --out results
```

A run takes a minute or two. It writes `results/summary.md`, the report, and
`results/raw.tsv`, every timed run. `--keep DIR` keeps the payloads, the
recordings and the final screens in `DIR`; `tmux-workspaces-bench` without
arguments lists the other options. The defaults are the gate's settings.

Every tmux process runs on a new private server through the test fixtures'
`TmuxServer`: a private `TMUX_TMPDIR`, its own `-L` socket name, `TMUX` and
`TMUX_PANE` removed, and no user configuration. The harness never touches
another tmux server.

An acceptance result needs three runs of a clean build of a committed
revision, under comparable load, with no other builds on the machine. The
report records the revision both programs were built from, load averages
before and after, and any compilers that were running. The gate refuses to
run a `rio-vt` program built from a different revision.

## How gate 1 measures

**Payloads.** A checked-in generator (`crates/bench/src/payload.rs`) writes
four payloads of at least 20,000,000 bytes from seed 1, for a 200×50
terminal. A pinned hash in its tests catches any change to its output.

| Payload | Imitates |
| --- | --- |
| `ascii` | Log lines of printable ASCII, some empty and some longer than the screen is wide. |
| `unicode` | Mixed scripts: accents and combining marks, Greek, Cyrillic, Hebrew, Arabic, CJK, Hangul and kana, emoji, ZWJ sequences, skin tones, flags and VS16 sequences. |
| `color` | Escape-heavy coloured text: 16-colour, 256-colour and direct colours, attributes, partial and full resets, and erasing with a background colour. Nearly every coloured word gets a style of its own. |
| `scroll` | A full-screen program: scroll regions, reverse index, inserted and deleted lines, erases, and status lines redrawn with the cursor saved and restored. |

Lines end with LF only; the pane's terminal driver adds the CR. No payload
contains `~`, which the harness's markers use.

**tmux's rate.** For each payload, the harness starts a private server whose
pane runs a small script that waits on a FIFO. A control-mode client
(`tmux -C`) attaches, sets its size to 200×50 with `refresh-client -C`, and the
harness checks the window size. Opening the FIFO releases the script, which
prints a start marker, the payload and an end marker. The client reads 64 KiB
at a time, keeps the bytes and decodes them, as any client must. tmux's rate is
the payload's size divided by the time between the two markers reaching the
client. There is one warm-up run and ten timed runs, each on a new server.

Every run must deliver exactly what the script printed: the start marker,
the payload with each LF turned into CR LF, and the end marker. The macOS
terminal driver sometimes writes the CR of a line end twice; a repeated CR
before an LF changes nothing on the screen, so it is taken as one.

**History.** tmux, `rio-vt` and `alacritty_terminal` keep up to 10,000 lines
of history. 0013 asks every engine for the same history limit, but
`libghostty-vt` limits history by memory, not lines, so that can only be
approximated: the `term` crate gives it room for about 10,000 lines, which
held 10,914 to 14,513 lines of the payloads that fill it. Engines also differ
in which lines enter history at all. The report shows the lines each kept,
and the gate does not decide on them.

**The engines' rate.** The last tmux run's recording is replayed in one child
process per engine: two warm-up replays and ten timed ones. A replay feeds
the control-mode stream to the decoder 64 KiB at a time, writes each `%output`
of the pane to the engine, and copies the whole screen into owned display data
(the `term` crate's `Screen`) once every N pane bytes, then once more after
the last chunk. N is what a 120 Hz display would see while tmux delivers the
payload at its median rate. The time runs from the first byte to the end of
the final read-out.

- `libghostty-vt` runs through the `term` crate, the app's path, with grapheme
  clustering on.
- `alacritty_terminal` and `rio-vt` run through adapters to the same `Screen`;
  `rio-vt` runs in its own program, as 0017 explains.

**Pass or fail.** Rates divide the payload's bytes, as generated, by the time.
The gate decides on `libghostty-vt`, at the slow end of its runs: the rate of
its nearest-rank 95th-percentile time must be at least 200 MB/s for `ascii`,
and at least five times tmux's median rate for every payload. As 0013
requires, a timing counts only when `libghostty-vt`'s final screen matches
every other engine's.

**Final screens.** Each engine's last screen is written one line per cell.
The report compares it with `libghostty-vt`'s, and with tmux's own view of the
pane in two ways:

- the text of each row and the cursor, from `tmux capture-pane -p -e -N` with
  its escape sequences removed, and `display-message`;
- every cell, by having `libghostty-vt` read tmux's capture. This shows
  styles, but its widths are `libghostty-vt`'s own, so it mirrors tmux only
  where the two agree on widths.

**Other figures.** "Copied per replay" is the display data all the read-outs
of one replay produced. "Peak memory" is the child process's peak resident
memory, which includes the recording it holds.

## Running gates 2 to 4

Gates 2 to 4 measure what appears on screen, so they run on macOS with a
display, not in CI. Each run opens one measured window at a time: the app's
benchmark window ([0019](../decisions/0019-benchmark-window.md)) or a separate
Ghostty instance that the harness starts with its own configuration and stops
afterwards. The harness never captures, or sends keys to, any other window.

Before running them:

- Give the terminal you run the harness from **Screen Recording** and
  **Accessibility** access in System Settings, under Privacy & Security. The
  harness checks both and stops if either is missing; it never asks for them.
- Install Ghostty, by default at `/Applications/Ghostty.app`, for the
  baselines.
- Leave a display free where the measured windows stay fully visible, and keep
  the machine quiet. A covered or hidden window stops drawing, and the harness
  then records a failed run, not a slow one. The windows open at the top left
  of the main display (`--origin` moves them).
- Do not type or click while a run is going: the measured window must keep
  its place and contents.

Build the harness and the window in the distributable configuration, as two
commands, then run each gate:

```sh
LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast cargo build --release --locked -p tmux-workspaces-bench
LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast cargo build --release --locked -p tmux-workspaces-app --example bench-window
./target/release/tmux-workspaces-bench gate2 --out results/gate2
./target/release/tmux-workspaces-bench gate3 --out results/gate3
./target/release/tmux-workspaces-bench gate4 --out results/gate4
```

Each writes `summary.md`, with the environment, the windows and a verdict per
case, and `raw.tsv`, every sample. Running `tmux-workspaces-bench` without
arguments lists the options. An acceptance result needs three runs of each
gate, as for gate 1.

## How gates 2 to 4 measure

The method is [0018](../decisions/0018-milestone-0-benchmark-method-revised.md).

**The windows.** Every run starts a new private tmux server holding one
200×50 session with its status line off. The app's window attaches to it as a
control-mode client. Ghostty runs either the workload directly ("plain
Ghostty") or an ordinary `tmux attach` to the same kind of session. Both read
one configuration file written by the harness: Menlo 13, the same colours and
16-colour palette, no padding and no blinking cursor. Targets alternate run by
run.

**Timing.** ScreenCaptureKit captures the measured window, and each frame
carries the time the window server displayed it. Times the harness takes, such
as posting a key, use the same clock. Keys are posted to the measured
process only. This leaves out the display's own scan-out and pixel response,
which are the same for both programs.

**Gate 2, bulk output.** The pane runs a producer that waits on a FIFO, as in
gate 1, then prints one of gate 1's payloads (20 MB by default) and fills the
screen with a marker colour. The time runs from releasing the producer to the
first frame that the marker colour fills, and the rate divides the payload's
bytes by it. The app is compared with Ghostty running tmux. vtebench, which
0018 also asks for, is not run yet, so this gate cannot pass until it is.

**Gate 3, typing latency.** The pane runs `cat`. Lowercase letters are posted
one at a time, at seeded random gaps of 150 to 350 ms, after a few unmeasured
warm-up keys. A key's latency runs from just before it is posted to the first
frame in which the typing pane's region changed. In the busy condition a
second pane below streams output, and only the top 45% of the window is
compared. A key with no change within one second is reported as missed. The
app is compared with plain Ghostty, and Ghostty running tmux shows tmux's
share.

**Gate 4, frame pacing.** The pane runs a fire animation, which redraws the
whole pane as fast as the terminal accepts, or a scroll test, each for 30
seconds. ScreenCaptureKit also delivers frames whose pixels did not change, so
only frames that changed count. Leaving out the first second, the frame rate
is changed frames per second, and pacing is the share of intervals between
them no longer than 1.5 times the display's refresh interval. The app is
compared with plain Ghostty.
