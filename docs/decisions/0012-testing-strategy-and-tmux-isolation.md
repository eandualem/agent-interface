# 0012. Testing strategy and tmux isolation

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

Most of the app's risk sits at its boundaries: the control-mode protocol, the
terminal engine, the store shared with the Python tool, input ordering, and
tmux itself. Tests must exercise those boundaries without touching anyone's
real tmux sessions. Developers and agents run their own sessions on the default
tmux server of the same machine, and any attached client, control mode
included, takes part in window sizing.

## Options considered

| Option | Trade-offs |
| --- | --- |
| **Rust's built-in test harness, plus a few focused tools, with one shared helper for every tmux process** | Familiar to every Rust contributor; isolation is enforced in one place instead of in each test. |
| Each test starts tmux its own way | Flexible, but one forgotten flag reaches the default server. Rejected. |
| A heavier runner such as cargo-nextest (0.9.148, MIT OR Apache-2.0) from the start | Better process isolation and reporting, but not needed at the current suite size. Deferred; `cargo test` stays the baseline either way. |

## Decision

**Test layers.** Build the smallest layer that exercises each boundary:

| Layer | What it checks |
| --- | --- |
| Model (`core`) | Layout operations; property tests for valid identities and structure; differential tests against the Python merge, including conflicts. Merge order is not assumed to be symmetric. |
| Protocol and terminal (`tmux`, `term`) | Input split at every byte boundary, including escapes, UTF-8 and terminal sequences; malformed records; notifications inside replies for each supported tmux version; disconnect and reconnect; display-data snapshots at a fixed size. |
| Input ordering (`app`) | Rapid switch then typing or paste; delayed attachment confirmation; failures, disconnects and superseded switches. Input reaches only its intended ready pane, and rejected input is visible and never sent elsewhere ([0007](0007-threads-ownership-and-event-ordering.md)). |
| Store (`store`) | Synthetic schema-v2 databases, optional and default fields, duplicate, oversized and deeply nested records, missing table versus missing row, read-only enforcement, short reads, hot-journal retry, refresh on a retained connection and two-writer conflicts ([0009](0009-python-library-and-shell-server-compatibility.md)). |
| Real tmux | Disposable sessions on private servers: process exit, resize, several windows and panes, output pressure and recovery. |
| Roster (`roster`) | Fixture responses and a local fake endpoint: absent provider, timeouts, invalid responses, rejected URLs, refused redirects, ignored proxy settings, and a synthetic key that never leaves the intended endpoint ([0010](0010-agents-roster-adapter.md)). |
| User interface | Build and headless tests first. Native input, input-method and rendering checks on macOS and on Linux Wayland and X11 start after the milestone-0 gate. A green hosted CI build is not evidence of display latency. |

Differential fixtures for the store and merge are generated from a pinned
commit of the Python tool, without modifying its repository, and are checked
in together with the script that generates them. Ordinary test runs do not need
Python.

**Test tools.**

| Tool | Version examined | Licence | Use |
| --- | --- | --- | --- |
| `proptest` | 1.12.0 | MIT OR Apache-2.0 | Property tests for trees, edit sequences and protocol chunking, with failing seeds kept |
| `insta` | 1.49.0 | Apache-2.0 | Reviewable snapshots of decoded output or display data, where a snapshot is clearer than an assertion; updates disabled in CI |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | Isolated fixture directories |

**tmux isolation.** Every tmux process in tests, benchmarks and spikes is
started through one fixture helper, which:

- creates a private `TMUX_TMPDIR` before launching anything, and passes it to
  every tmux command;
- removes `TMUX` and `TMUX_PANE` from every command's environment, because an
  unset `TMUX` alone falls back to the default server;
- always passes `-L` with a unique name or `-S` with a private path, keeping
  socket paths short enough for macOS;
- starts tmux with no user configuration (`-f /dev/null`);
- creates only disposable sessions, and never attaches to a session it did not
  create;
- on success or failure, stops only the server it created, and never runs a
  default-server command while cleaning up.

A check against a live session is read-only (`-f read-only,ignore-size`) and is
reported to the maintainer before it runs. Tests never run `kill-server` on, or
change options of, a server they did not create.

**Continuous integration.**

- Runners: `macos-15` (arm64) and `ubuntu-24.04` (x86-64), using the same
  lockfile and the pinned Rust and Zig versions. Intel macOS and Linux arm64
  are added when they become supported targets.
- Jobs: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked` (doctests included),
  `cargo doc --workspace --no-deps --locked` with rustdoc warnings denied, and
  `cargo deny check` ([0011](0011-toolchain-and-dependency-policy.md)).
- Pin every Action by full commit SHA and give the workflow read-only
  repository access. Cache keys include the OS, architecture, toolchain,
  lockfile and native build settings. Print the versions of tmux and native
  tools installed.
- CI builds and tests only. Publishing and signing belong to a separately
  approved release workflow.

## Consequences

- One helper is the only place that starts tmux in tests, so isolation can be
  reviewed in one file.
- Tests that need tmux need it installed on the runner and on contributors'
  machines.
- The tmux version each runner provides affects which version fixtures run in
  CI; recording and extending that coverage is part of
  [0014](0014-supported-tmux-versions.md).

## Revisit when

Suite size or flaky process interactions justify cargo-nextest, or a new
supported platform needs its own runner.

## Evidence

- [Testing and continuous integration](../research/rust-architecture.md#testing-and-continuous-integration)
  in the Rust architecture research, including the
  [runner inventory at `ee1ff45`](https://github.com/actions/runner-images/blob/ee1ff4569e381a9649b400338bdc8ceb7e18a80e/README.md).
- Reproduction rules in the [control-mode research](../research/tmux-control-mode.md#local-throughput-evidence).
- [Parity checklist](../PARITY.md#performance-budgets), row PF-2.
