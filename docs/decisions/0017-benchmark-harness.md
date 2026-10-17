# 0017. Benchmark harness

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

The milestone-0 gates ([0013](0013-milestone-0-benchmark-method.md)) need a
harness in the repository. [0006](0006-workspace-and-crate-layout.md) allows
development-only packages under `crates/`, and the
[architecture page](../ARCHITECTURE.md) names `crates/bench` for the harness.

Gate 1 compares `libghostty-vt` with the engines that
[0004](0004-terminal-emulation-core.md) names as alternatives, `rio-vt` and
`alacritty_terminal`, on the same workload and the same read-out work. Two
facts observed while building it shape the harness:

- **`rio-vt` and `libghostty-vt` cannot be linked into one program.** Both
  contain the simdutf C++ library: `rio-vt` through the `simdutf` 0.7.0 crate,
  which it requires on every target except WebAssembly, and `libghostty-vt-sys`
  inside Ghostty's core. The macOS linker reports duplicate simdutf symbols as
  a warning, links anyway, and the program crashes at startup (`SIGBUS`).
- **The `term` crate links `libghostty-vt`.** A program that measures `rio-vt`
  cannot depend on it, yet must fill the same owned display data, the `term`
  crate's `Screen`.

## Options considered

| Option | Trade-offs |
| --- | --- |
| One benchmark program with every engine | Simplest, but crashes, as above. |
| Drop `rio-vt` from the comparison | Simple, but leaves decision 3 of 0004 unmeasured. |
| Move `Screen` into a new engine-free crate | A seventh product crate, and a change to 0006, for the benchmark's sake. |
| A `term` feature that leaves the engine out | Cargo unifies features across a workspace build, so `cargo build --workspace` would still link both engines into the `rio-vt` program. |
| **A second development-only program for `rio-vt` that includes the harness's engine-free source files** | Both programs run exactly the same replay code on the same `Screen` type, with no copies to keep in step. Including files by path is unusual, so each included file says so. |

## Decision

- **`crates/bench`** (`tmux-workspaces-bench`, binary `tmux-workspaces-bench`)
  holds the gates: the payload generator, the recorder that runs tmux on
  private servers through `TmuxServer`, the replay, the comparison of final
  screens and the reports. It measures `libghostty-vt` through the `term`
  crate, the path the app uses, and `alacritty_terminal` through an adapter.
- **`crates/bench-rio`** (`tmux-workspaces-bench-rio`) is the replay program for
  `rio-vt`. It includes, with `#[path]`, the harness files that do not touch
  `term` (`engine.rs`, `replay.rs`, `child.rs`, `dump.rs`, `options.rs`,
  `memory.rs`) and the `term` crate's `screen.rs`, which therefore uses only
  the standard library.
- Each engine replays in a child process of its own, so that its timings and
  peak memory are its own, and `rio-vt` never shares a process with
  `libghostty-vt`.
- Both packages set `publish = false` and are never dependencies of the six
  product crates. `deny.toml` lets them depend on `tmux` and `term`, and lets
  `bench` depend on the test fixtures.
- The harness's only `unsafe` code reads the peak resident memory with
  `getrusage`, in `memory.rs`.

Dependencies:

| Dependency | Version | Licence | Used by | Reason |
| --- | --- | --- | --- | --- |
| `rio-vt` | 0.5.28, exact, default features off | MIT | `bench-rio` | The engine 0004 measures against `libghostty-vt`. Its default feature adds a pseudo-terminal driver the benchmark does not use. |
| `alacritty_terminal` | 0.26.0, exact | Apache-2.0 | `bench`, and `bench-rio` in tests | 0004's fallback engine, and the reference for `rio-vt`'s tests. |
| `libc` | 0.2.191 | MIT OR Apache-2.0 | both | `getrusage`, for peak memory. |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | `bench` | Private directories for payloads and recordings, as in 0012. |

Their own dependencies pass `cargo deny check` with the licences
`deny.toml` already allows.

## Consequences

- Running gate 1 builds both programs with the same profile; the gate finds
  `tmux-workspaces-bench-rio` next to its own program
  ([benchmark guide](../benchmarks/README.md)).
- A change to a file `bench-rio` includes must keep it free of `term`, and
  `term`'s `screen.rs` must keep using only the standard library.
- Workspace builds also compile `rio-vt`, its C++ simdutf build and
  `alacritty_terminal`, which adds to build time.

## Revisit when

`rio-vt` or `libghostty-vt` stops embedding simdutf, or the comparison
engines are no longer needed.

## Evidence

- Observed here: the crash of a test program linking both engines, and the
  linker's duplicate-symbol warnings, on macOS 27.0.1 with Rust 1.99.1.
- [`rio-vt` 0.5.28 manifest](https://docs.rs/crate/rio-vt/0.5.28/source/Cargo.toml.orig)
  for its `simdutf` dependency and default features.
