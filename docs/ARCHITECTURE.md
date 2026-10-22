# Architecture

tmux-workspaces is a native desktop window for working in tmux. A sidebar holds
workspaces, tabs and an optional agents roster; the rest of the window shows
terminal panes that the app draws itself. tmux runs every shell, and the app is
a tmux client.

This page describes the process model, crate boundaries and data flow. So far the code is the Cargo
workspace, its build, its test fixtures, the first parts of the terminal path
and the benchmark harness. Features come after the milestone-0
measurements.

## The design at a glance

**Processes.** tmux servers own every shell, terminal and session, so shells
keep running when the app closes. The app runs no daemon. It starts tmux
control-mode clients (`tmux -C`) and one-off tmux commands, and may start the
library's shell server, which then belongs to tmux.

**Connections.** Each leaf of a workspace layout refers to a tmux session: an
ordinary shell on the library's shell server, or an attached session. The app
keeps one control-mode connection per attachment and shows the session's
selected window with that window's own panes.

**Crates.** Six crates with a one-way dependency direction:

| Crate | Responsibility |
| --- | --- |
| `core` | Workspace, tab and layout model, validation, merge rules and read-only theme data |
| `store` | Reading `layouts.db`, the library shared with the Python tool |
| `tmux` | Control-mode protocol and connections |
| `term` | Terminal state for each pane, around the terminal engine |
| `roster` | Optional, read-only agents roster |
| `app` | The window, workers and the wiring between the other crates |

**Threads.** Inside the one app process, the user-interface thread runs the
window and draws. A terminal worker owns every terminal engine handle and
publishes owned display data, a storage worker owns the database connection,
and background input and output drives the tmux processes. The library crates
stay synchronous; `app` owns the threads, scheduling and shutdown.

**From pane output to a frame.** tmux reports pane output on a control
connection. The `tmux` crate decodes it, and a terminal worker feeds the bytes
to that pane's terminal state in `term`. The worker publishes owned display
data, which the user-interface thread draws.

**From a key to a pane.** The app reads every key. Its own shortcuts run
natively; other input is encoded for the pane and sent to tmux as literal bytes
for an exact pane. After a navigation, input waits until the destination is
ready.

**The shared library.** A storage worker reads `layouts.db` read-only and
watches it for changes made by the Python tool. Writing waits until the Python
tool's merge and conflict rules are ported and tested.

## The code today

The repository is a Cargo workspace. Each crate above is a package under
`crates/`, named `tmux-workspaces-<crate>`.

| Path | What is there now |
| --- | --- |
| `Cargo.toml` | The workspace: its members, and the metadata, lints and dependency versions they share. Every public item must be documented. |
| `rust-toolchain.toml` | The pinned Rust toolchain. |
| `deny.toml` | The allowed licences, the advisory and source checks, and the dependency direction between the crates. |
| `crates/core` | Workspace, tab, split, leaf and attachment types; in-memory create/rename/split/attach/close operations, workspace/tab navigation and tab moves, local navigation retention, cell rectangles with focused-pane fallback, and shared-document three-way merging, independent of storage and rendering. `keymap` holds immutable validated shortcut configuration and print-only help/TOML output. |
| `crates/store` | Read-only schema-v2 SQLite access, bounded JSON validation, original-document preservation and pure model-to-document projection that retains extension fields, and shared-edit reconciliation with local navigation. No database writes, migration or shell creation yet. |
| `crates/roster` | Validated roster snapshots, stale-state retention, attention ordering, bounded loopback HTTP reads, and lazy read-only settings and credential lookup. |
| `crates/tmux` | Read-only executable preflight and server version validation (`preflight`). `control`, the decoder for a control-mode client's output: pane output with tmux's escaping removed, command replies and notifications. `layout`, which reads visible pane rectangles (including zoom) from layout notifications. `capture`, which reads a pane's rows and pending parser bytes from `capture-pane`, and `recovery`, the commands and reply handling that rebuild a pane's state. |
| `crates/term` | `Terminal`, the wrapper around the terminal engine: it creates a terminal with grapheme clustering on, feeds it bytes, and copies the visible screen into a `Screen`, owned display data that any thread can hold. `restore`, which builds a terminal from a pane's state as tmux holds it. |
| `crates/app` | The `tmux-workspaces` binary, which can print the effective keymap, and a library with read-only keymap loading (`keymap`), library path and shell identity lookup plus a read-only storage worker and per-window layout reconciliation and viewing history (`library`), independent session discovery (`sessions`) and optional-provider resolution and explicit roster polling (`roster`), and the terminal path: `panes` starts from captured pane state, keeps each terminal on a worker thread and hands over owned display data, and `view` draws it with GPUI. The `bench-window` example draws one tmux window with them for the benchmarks; there is no app window yet. Connected-server version validation, initial capture, right-margin recovery and recovery after visible layout changes are integrated. Driving tmux geometry from native window resizing, changing windows and retrying failed attachments still belong to the full connection lifecycle. |
| `crates/test-support` | Development-only fixtures. `TmuxServer` starts the private tmux servers that tests use, and control-mode clients on them. |
| `crates/bench`, `crates/bench-rio` | The development-only benchmark harness and its separate `rio-vt` program. |
| `docs/` | Architecture, design and contributor documentation. |
| `.github/workflows/ci.yml` | Continuous integration on macOS and Linux. |

**Dependency direction.** A crate can use only the crates its manifest lists,
and `cargo deny check` fails when a project crate depends on one that `deny.toml` does not allow. The
`bans.deny` table in `deny.toml` names, for each crate, the only crates that
may depend on it.

**Native build.** `term` depends on `libghostty-vt` 0.2.2, whose build script
compiles Ghostty's terminal core with Zig 0.15.2. The
[contributor guide](../CONTRIBUTING.md#the-terminal-engines-native-build)
covers its build modes, CPU targets and offline builds.

**Where to start reading.** Read `Cargo.toml` and `deny.toml` for the shape
of the workspace, then `crates/term/src/lib.rs` for the boundary around the
terminal engine and `crates/test-support/src/lib.rs` for how tests isolate
tmux. `.github/workflows/ci.yml` shows every check a change must pass.

## Further reading

- [Contributor guide](../CONTRIBUTING.md): setting up, building, testing and
  proposing changes.

### Read-only theme configuration

`core::theme` validates the shared theme format and retains all six presets,
ordered fallbacks and ground inheritance. `app::theme::load` performs bounded
read-only loading and returns the chosen path and any diagnostic alongside the
valid or retained working theme. Native colour resolution and window activation
remain separate.
