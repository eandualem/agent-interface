# 0004. Terminal emulation core

- **Status:** Proposed. Waits on the milestone-0 engine benchmark and fidelity
  checks ([0013](0013-milestone-0-benchmark-method.md)) and a clean build on
  macOS and Linux.
- **Date:** 2026-10-17

## Context

The app keeps a terminal state for every tmux pane it shows: the screen grid,
scrollback, modes, cursor, Unicode grapheme handling, selection, and the
encoding of keys and mouse reports. It feeds that state the pane bytes decoded
from control mode ([0003](0003-control-mode-connection-and-attachment-model.md))
and hands owned display data to the renderer.

Writing a terminal emulator is a large, long-lived job. The
[terminal-core research](../research/terminal-core.md) compared existing
engines by their source and ran an exploratory throughput probe. That probe is
not a benchmark result: it had no control-mode decoding, no check that the
engines ended in the same state, different read-out work per engine and unequal
CPU targeting.

## Options considered

| Engine and licence | Strengths observed | Costs observed |
| --- | --- | --- |
| **`libghostty-vt` / `libghostty-vt-sys`**, MIT OR Apache-2.0, over Ghostty's core, MIT | Scrollback, reflow on resize, grapheme-aware cells, key and mouse encoders, selection, incremental render state. Highest rate in the exploratory probe. | Zig and C build; the C API is explicitly unstable; the published crate and the current Git source expose different APIs; handles are not `Send`. |
| `rio-vt` 0.5.28, MIT, with files that keep Apache-2.0 attribution | Separate core with reflow, selection, regex search, grapheme handling and damage tracking. | Recently split out of Rio; its default feature enables a pseudo-terminal driver that must be turned off. Not yet measured here. |
| `alacritty_terminal` 0.26.0, Apache-2.0 | Mature pure-Rust grid, scrollback, reflow, selection, regex search, damage tracking. | Width is computed per character, so it lacks general grapheme clustering; no complete key encoder in the crate. |
| `wezterm-term` (Git only), MIT, with `termwiz` 0.23.3, MIT | Grapheme handling, reflow, rich escape support, keyboard encoding. | Depends on many sibling crates from the WezTerm workspace and is not published to crates.io. |
| `vt100` 0.16.2, MIT | Small and simple. | No reflow, no hyperlinks, no modern keyboard or selection support. |
| Our own emulator on `vte` 0.15.0, MIT OR Apache-2.0 | Full control. | The project would own history, reflow, Unicode layout, modes and every edge case. Rejected for a parity-first project. |

## Decision

1. Use **`libghostty-vt`** as the leading engine, wrapped by the `term` crate
   ([0006](0006-workspace-and-crate-layout.md)). The `term` crate's public API
   exposes only the project's own types, so the engine can be replaced without
   touching other crates.
2. Start from the **published 0.2.2** crates. They pin Ghostty commit
   `a887df42c56f6de86c0fe6da9c4eeca37931e083` and build with Zig 0.15.2. The
   newer Git source (wrapper `d2036baeb605d31d09a9e2ba6d0c923e04c34d54` with
   Ghostty `3425025e585a3403340d4dc6d65132f42e05e605`, built with Zig 0.16)
   adds a search API that the parity checklist does not need yet. Moving to it
   later is a version change that keeps the wrapper, native library and Zig
   version together.
3. Measure **`rio-vt`**, with default features off, against it in the
   milestone-0 engine benchmark, using the same workload and the same
   read-out work.
4. Keep **`alacritty_terminal`** as the fallback if both fail and the grapheme
   gap is acceptable. Use **`vt100`** only as a test oracle where its narrower
   behaviour is what a test means to check. Do not build a new emulator.
5. Construct, use and destroy terminal handles on their owner thread, and send
   owned display data to the user interface
   ([0007](0007-threads-ownership-and-event-ordering.md)). Never add an unsafe
   `Send` implementation to move a handle.
6. Release and benchmark builds set `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast`.
   Without it, a Cargo profile that has debug information builds Ghostty's
   core in Zig's Debug mode ([0011](0011-toolchain-and-dependency-policy.md)).
7. Terminal replies the engine generates, such as answers to device queries,
   are not forwarded to tmux by default, because tmux already answers them for
   the pane ([0015](0015-input-routing.md)).

## Consequences

- Building the project needs Zig 0.15.2 and a C toolchain, and by default
  fetches Ghostty's source at build time. Offline builds use
  `GHOSTTY_SOURCE_DIR` with a pre-fetched copy.
- The native inputs (Ghostty's core; simdutf and Highway when SIMD is enabled;
  uucode and its Unicode data; Wuffs when graphics support is enabled) need
  their own licence notices, because `cargo-deny` does not see them. The Wuffs archive could not be inspected during research
  and remains an open check before any release.
- Capabilities must be verified through tmux, not only by writing bytes to the
  engine: tmux and the engine can disagree about character widths, modes and
  supported input protocols.

## Accept when

All of these hold, measured as [0013](0013-milestone-0-benchmark-method.md)
describes:

- The engine passes milestone-0 gate 1 (headless headroom) in the
  distributable build configuration.
- A fidelity corpus passes: complete and fragmented streams covering
  scrollback, alternate screen, reflow, CJK, combining marks, emoji sequences,
  hyperlinks, clipboard sequences, extended keys, mouse reports and bracketed
  paste, also replayed through a private tmux server.
- The project builds from a clean checkout on macOS arm64 and Linux x86-64 with
  the pinned Zig, and the native licence notices are collected.

If `rio-vt` matches it on these checks with a simpler build, prefer `rio-vt`
in a superseding record.

## Evidence

- [Terminal emulation core research](../research/terminal-core.md): the
  [comparison](../research/terminal-core.md#verdict-and-alternatives),
  [what is actually pinned](../research/terminal-core.md#what-is-actually-pinned),
  [build, portability and licence surface](../research/terminal-core.md#build-portability-and-licence-surface)
  and the [exploratory headless probe](../research/terminal-core.md#exploratory-headless-probe)
  with its limits.
- [Evidence still needed before adoption](../research/terminal-core.md#evidence-still-needed-before-adoption).
