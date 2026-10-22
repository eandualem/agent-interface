# Architecture

tmux-workspaces is a native desktop window for working in tmux. A sidebar holds
workspaces, tabs and an optional agents roster; the rest of the window shows
terminal panes that the app draws itself. tmux runs every shell, and the app is
a tmux client.

This page describes the design decided so far, with the decision record behind
each part, and then the code that exists today. So far the code is the Cargo
workspace, its build, its test fixtures, the first parts of the terminal path
and the benchmark harness. Features come after the milestone-0
measurements ([0018](decisions/0018-milestone-0-benchmark-method-revised.md)).

## The design at a glance

**Processes.** tmux servers own every shell, terminal and session, so shells
keep running when the app closes. The app runs no daemon. It starts tmux
control-mode clients (`tmux -C`) and one-off tmux commands, and may start the
library's shell server, which then belongs to tmux
([0002](decisions/0002-tmux-is-the-only-process-layer.md)).

**Connections.** Each leaf of a workspace layout refers to a tmux session: an
ordinary shell on the library's shell server, or an attached session. The app
keeps one control-mode connection per attachment and shows the session's
selected window with that window's own panes
([0003](decisions/0003-control-mode-connection-and-attachment-model.md)).

**Crates.** Six crates with a one-way dependency direction
([0006](decisions/0006-workspace-and-crate-layout.md)):

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
stay synchronous; `app` owns the threads, scheduling and shutdown
([0007](decisions/0007-threads-ownership-and-event-ordering.md),
[0008](decisions/0008-background-io-runtime.md)).

**From pane output to a frame.** tmux reports pane output on a control
connection. The `tmux` crate decodes it, and a terminal worker feeds the bytes
to that pane's terminal state in `term`. The worker publishes owned display
data, which the user-interface thread draws
([0007](decisions/0007-threads-ownership-and-event-ordering.md)).

**From a key to a pane.** The app reads every key. Its own shortcuts run
natively; other input is encoded for the pane and sent to tmux as literal bytes
for an exact pane. After a navigation, input waits until the destination is
ready ([0007](decisions/0007-threads-ownership-and-event-ordering.md),
[0015](decisions/0015-input-routing.md)).

**The shared library.** A storage worker reads `layouts.db` read-only and
watches it for changes made by the Python tool. Writing waits until the Python
tool's merge and conflict rules are ported and tested
([0009](decisions/0009-python-library-and-shell-server-compatibility.md)).

## The code today

The repository is a Cargo workspace. Each crate above is a package under
`crates/`, named `tmux-workspaces-<crate>`.

| Path | What is there now |
| --- | --- |
| `Cargo.toml` | The workspace: its members, and the metadata, lints and dependency versions they share. Every public item must be documented. |
| `rust-toolchain.toml` | The pinned Rust toolchain ([0011](decisions/0011-toolchain-and-dependency-policy.md)). |
| `deny.toml` | The allowed licences, the advisory and source checks, and the dependency direction between the crates. |
| `crates/core` | Workspace, tab, split, leaf and attachment types; in-memory create/rename/split/attach/close operations, workspace/tab navigation and tab moves, local navigation retention, cell rectangles with focused-pane fallback, and shared-document three-way merging, independent of storage and rendering. `keymap` holds immutable validated shortcut configuration and print-only help/TOML output. |
| `crates/store` | Read-only schema-v2 SQLite access, bounded JSON validation, original-document preservation and pure model-to-document projection that retains extension fields, and shared-edit reconciliation with local navigation. No database writes, migration or shell creation yet. |
| `crates/roster` | Validated roster snapshots, stale-state retention, attention ordering, bounded loopback HTTP reads, and lazy read-only settings and credential lookup. |
| `crates/tmux` | Read-only executable preflight and server version validation (`preflight`). `control`, the decoder for a control-mode client's output: pane output with tmux's escaping removed, command replies and notifications. `layout`, which reads visible pane rectangles (including zoom) from layout notifications. `capture`, which reads a pane's rows and pending parser bytes from `capture-pane`, and `recovery`, the commands and reply handling that rebuild a pane's state ([0021](decisions/0021-pane-state-recovery.md)). |
| `crates/term` | `Terminal`, the wrapper around the terminal engine: it creates a terminal with grapheme clustering on, feeds it bytes, and copies the visible screen into a `Screen`, owned display data that any thread can hold. `restore`, which builds a terminal from a pane's state as tmux holds it ([0021](decisions/0021-pane-state-recovery.md)). |
| `crates/app` | The `tmux-workspaces` binary, which can print the effective keymap, and a library with read-only keymap loading (`keymap`), library path and shell identity lookup plus a read-only storage worker and per-window layout reconciliation and viewing history (`library`), independent session discovery (`sessions`) and optional-provider resolution and explicit roster polling (`roster`), and the terminal path: `panes` starts from captured pane state, keeps each terminal on a worker thread and hands over owned display data, and `view` draws it with GPUI. The `bench-window` example draws one tmux window with them for the benchmarks ([0019](decisions/0019-benchmark-window.md)); there is no app window yet. Connected-server version validation, initial capture, right-margin recovery and recovery after visible layout changes are integrated. Driving tmux geometry from native window resizing, changing windows and retrying failed attachments still belong to the full connection lifecycle. |
| `crates/test-support` | Development-only fixtures. `TmuxServer` starts the private tmux servers that tests use, and control-mode clients on them ([0012](decisions/0012-testing-strategy-and-tmux-isolation.md)). |
| `crates/bench`, `crates/bench-rio` | The development-only milestone-0 benchmark harness and its separate `rio-vt` program ([0017](decisions/0017-benchmark-harness.md)). |
| `docs/` | This page, the decision records, the research, the parity checklist and the [benchmark results](benchmarks/README.md). |
| `.github/workflows/ci.yml` | Continuous integration on macOS and Linux. |

The milestone-0 benchmark harness and how to run it are described in the
[benchmark guide](benchmarks/README.md).

**Dependency direction.** A crate can use only the crates its manifest lists,
and `cargo deny check` fails when a project crate depends on one that
[0006](decisions/0006-workspace-and-crate-layout.md) does not allow. The
`bans.deny` table in `deny.toml` names, for each crate, the only crates that
may depend on it.

**Native build.** `term` depends on `libghostty-vt` 0.2.2, whose build script
compiles Ghostty's terminal core with Zig 0.15.2 and the verified repository
patch ([0025](decisions/0025-patched-terminal-source.md)). The
[contributor guide](../CONTRIBUTING.md#the-terminal-engines-native-build)
covers its build modes, CPU targets and offline builds.

**Where to start reading.** Read `Cargo.toml` and `deny.toml` for the shape
of the workspace, then `crates/term/src/lib.rs` for the boundary around the
terminal engine and `crates/test-support/src/lib.rs` for how tests isolate
tmux. `.github/workflows/ci.yml` shows every check a change must pass.

## Decision records

Each record states what was chosen, the options considered and the evidence.
A proposed record names the measurement that settles it. How records work is
described in [0001](decisions/0001-record-architecture-decisions.md).

| Record | Status |
| --- | --- |
| [0001. Record architecture decisions](decisions/0001-record-architecture-decisions.md) | Accepted |
| [0002. tmux is the only process layer](decisions/0002-tmux-is-the-only-process-layer.md) | Accepted |
| [0003. Control-mode connection and attachment model](decisions/0003-control-mode-connection-and-attachment-model.md) | Proposed |
| [0004. Terminal emulation core](decisions/0004-terminal-emulation-core.md) | Proposed |
| [0005. UI framework and terminal renderer](decisions/0005-ui-framework-and-terminal-renderer.md) | Proposed |
| [0006. Workspace and crate layout](decisions/0006-workspace-and-crate-layout.md) | Accepted |
| [0007. Threads, ownership and event ordering](decisions/0007-threads-ownership-and-event-ordering.md) | Accepted |
| [0008. Background I/O runtime](decisions/0008-background-io-runtime.md) | Proposed |
| [0009. Compatibility with the Python tool's library and shell server](decisions/0009-python-library-and-shell-server-compatibility.md) | Accepted |
| [0010. Agents roster adapter](decisions/0010-agents-roster-adapter.md) | Accepted |
| [0011. Toolchain and dependency policy](decisions/0011-toolchain-and-dependency-policy.md) | Accepted |
| [0012. Testing strategy and tmux isolation](decisions/0012-testing-strategy-and-tmux-isolation.md) | Accepted |
| [0013. Milestone-0 benchmark method](decisions/0013-milestone-0-benchmark-method.md) | Superseded by 0018 |
| [0014. Supported tmux versions](decisions/0014-supported-tmux-versions.md) | Proposed |
| [0015. Input routing](decisions/0015-input-routing.md) | Proposed |
| [0016. Shell environment and launch context](decisions/0016-shell-environment-and-launch-context.md) | Proposed |
| [0017. Benchmark harness](decisions/0017-benchmark-harness.md) | Accepted |
| [0018. Milestone-0 benchmark method, revised](decisions/0018-milestone-0-benchmark-method-revised.md) | Accepted |
| [0019. Benchmark window](decisions/0019-benchmark-window.md) | Accepted |
| [0020. Benchmark launch context](decisions/0020-benchmark-launch-context.md) | Accepted |
| [0021. Pane state recovery and the control-mode fidelity harness](decisions/0021-pane-state-recovery.md) | Accepted |
| [0022. vtebench in gate 2](decisions/0022-vtebench-in-gate-2.md) | Proposed |

## Further reading

- [Architecture research](research/README.md): the evidence behind the
  records, with the alternatives and licences examined.
- [Parity checklist](PARITY.md): every behaviour of the Python tool and what
  this app does with it.
- [Contributor guide](../CONTRIBUTING.md): setting up, building, testing and
  proposing changes.
