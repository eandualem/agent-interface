# Architecture

tmux-workspaces is a native desktop window for working in tmux. A sidebar holds
workspaces, tabs and an optional agents roster; the rest of the window shows
terminal panes that the app draws itself. tmux runs every shell, and the app is
a tmux client.

This page describes the process model, crate boundaries and data flow. So far the code is the Cargo
workspace, its build and its test fixtures. Features come after the milestone-0
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
| `core` | Workspace, tab and layout model, validation and merge rules |
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
| `crates/core`, `crates/store`, `crates/tmux`, `crates/roster` | Crate documentation stating each crate's responsibility; no code yet. |
| `crates/term` | `Terminal`, a minimal wrapper around the terminal engine that creates a terminal, feeds it bytes and reads the cursor. It proves the engine builds and runs on each platform. |
| `crates/app` | The `tmux-workspaces` binary. It only prints its version: there is no window yet. |
| `crates/test-support` | Development-only fixtures. `TmuxServer` starts the private tmux servers that tests use. |
| `docs/` | Architecture, design and contributor documentation. |
| `.github/workflows/ci.yml` | Continuous integration on macOS and Linux. |

The benchmark harness will be a development-only crate,
`crates/bench`.

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
