# 0006. Workspace and crate layout

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

The app has a few distinct concerns: the workspace model and its merge rules,
storage compatible with the Python tool, the tmux protocol, terminal emulation,
the optional agents roster, and the user interface. Each changes for different
reasons and should be testable without the others. A contributor should be able
to tell from the manifests which part may depend on which.

The [Rust architecture research](../research/rust-architecture.md#crates-and-dependency-direction)
revised the starting five-crate idea by moving SQLite storage out of the model
crate.

## Options considered

| Option | Trade-offs |
| --- | --- |
| One application crate | Simplest to start. Nothing enforces dependency direction, so the model, protocol and user interface tend to entangle. |
| Five crates, with storage inside the model crate | Clear adapters, but pure model and merge tests would link SQLite. |
| **Six crates** | Each boundary is enforced by Cargo. Slightly more manifest upkeep. |
| More crates, or a plugin or service framework | No current need; adds structure to understand without a second implementation to justify it. |

## Decision

Use a virtual Cargo workspace with a flat `crates/` directory and six crates.
Package names start with `tmux-workspaces-`; the application package builds
the binary `tmux-workspaces`.

| Directory | Package | Responsibility | May depend on |
| --- | --- | --- | --- |
| `crates/core` | `tmux-workspaces-core` | Workspace, tab and layout model; stable identities; validation; the shared-layout merge; local navigation | No project crate |
| `crates/store` | `tmux-workspaces-store` | SQLite access; schema-v2 decoding and encoding; persistence transactions | `core` |
| `crates/tmux` | `tmux-workspaces-tmux` | Control-mode decoder, commands, session and pane identity, connection lifecycle | No project crate |
| `crates/term` | `tmux-workspaces-term` | Safe boundary around the terminal engine; input encoding; terminal state; owned display data | No project crate |
| `crates/roster` | `tmux-workspaces-roster` | Read-only roster transport and response types | No project crate |
| `crates/app` | `tmux-workspaces-app` | User interface, configuration loading, ownership of workers and connections, mapping adapter data into the model | All five |

Rules that keep the boundaries useful:

- `tmux` and `term` do not depend on each other. `tmux` emits pane bytes,
  `term` consumes them, and `app` connects the two.
- `tmux` and `roster` do not depend on `core` just to share an identifier;
  `app` maps their identifiers into the model.
- `core` contains no UI framework types, async runtime handles, SQLite
  connections or terminal engine pointers.
- Within `tmux`, protocol parsing is separate from the transport that starts
  and reads processes. Within `roster`, response decoding is separate from
  polling.
- Library APIs are synchronous. `app` owns threads, scheduling and shutdown
  ([0007](0007-threads-ownership-and-event-ordering.md)).
- Paths are resolved once in `app` and passed into libraries.
- Libraries return typed errors where callers need to respond differently,
  such as a malformed protocol message, a lost connection, an unsupported
  schema, an invalid layout, a busy database or a merge conflict. `app` may
  wrap errors for reporting. Libraries emit `tracing` events but never install
  a global subscriber; `app` configures logging. By default, logs record
  counts, durations, queue depths and lifecycle changes, never pane contents,
  paths or credentials.
- Project-written `unsafe` code lives only at the terminal engine and platform
  boundaries, with its invariants documented. Crates that need none declare
  `#![forbid(unsafe_code)]`.
- Add a trait when there is a second implementation or a test seam that needs
  one, not in advance.

Workspace settings: edition 2024, resolver 3, an explicit member list, and
shared package metadata, lints and dependency versions inherited from the
workspace. Every package sets `publish = false` while it is an application
internal. Development-only packages, such as shared test fixtures or the
benchmark harness, may be added under `crates/` with `publish = false` and are
used only as development dependencies of the six crates above.

Shared dependencies chosen here:

| Dependency | Version examined | Licence | Reason |
| --- | --- | --- | --- |
| `uuid` with `v4` | 1.28.0 | MIT OR Apache-2.0 | Generate the same 12-character lowercase hexadecimal identities used by the Python model; already present through the UI dependency graph. |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | Deriving typed library errors |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | Error reporting at the application boundary only |
| `tracing` | 0.1.44 | MIT | Structured events and timing spans in every crate |
| `tracing-subscriber` | 0.3.23 | MIT | Log output, configured only by `app` |

## Consequences

- Model and merge tests run without SQLite, tmux or a display. Protocol and
  terminal tests run without a user interface.
- `term` still needs Zig and a C toolchain, and tmux tests still need tmux:
  headless does not mean free of native dependencies.
- Launching the app from source is `cargo run -p tmux-workspaces-app`.
- A dependency that crosses these lines is a review finding, not a style
  preference.

## Revisit when

A real second implementation or boundary appears, for example a remote
transport that needs its own crate, or a crate grows large enough that its
internal modules change for unrelated reasons.

## Evidence

- [Crates and dependency direction](../research/rust-architecture.md#crates-and-dependency-direction)
  and [configuration, errors and observability](../research/rust-architecture.md#configuration-errors-and-observability)
  in the Rust architecture research.
- [Cargo workspace reference at `d419f52`](https://github.com/rust-lang/cargo/blob/d419f522507a7f981d19bd80a7ae8566e744a487/doc/book/src/reference/workspaces.md).

The in-memory `Layout` operations mirror the Python model's create, split,
attach, close and pane-cycle behavior. They create or remove records only;
the application owns any eventual process effects. Callers supply proposed
names, directories and attachment references. As in the Python model, these
operations do not replace persisted-schema validation or the write-conflict
gate in [0009](0009-python-library-and-shell-server-compatibility.md).
