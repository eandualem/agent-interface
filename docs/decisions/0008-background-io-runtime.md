# 0008. Background I/O runtime

- **Status:** Proposed. Waits on the worker-architecture comparison described
  under **Accept when**.
- **Date:** 2026-10-17

## Context

The app reads and writes the pipes of tmux control-mode children, waits on
timeouts, and supervises child processes, possibly for many leaves at once
([0003](0003-control-mode-connection-and-attachment-model.md)). The thread
layout is fixed by [0007](0007-threads-ownership-and-event-ordering.md); this
record decides what drives the background input and output.

GPUI ([0005](0005-ui-framework-and-terminal-renderer.md)) has its own
foreground executor for the main thread and a background executor for `Send`
futures. It does not provide a Tokio reactor, and GPUI Component already uses
smol internally.

## Options considered

| Option | Licence and version examined | Trade-offs |
| --- | --- | --- |
| Standard library threads and channels | Rust standard library, MIT OR Apache-2.0 | No runtime to learn and easy to debug for a few sessions. Each child needs its own drain threads and cancellation, and thread count grows with attached leaves. |
| smol | 2.0.2, MIT OR Apache-2.0 | Small runtime with process and I/O support; already in the graph through GPUI Component, which could avoid a second runtime. |
| **Tokio** | 1.53.3, MIT | Mature process, timer, bounded-channel and shutdown APIs that can be tested headless without GPUI. Adds a runtime beside GPUI's executor. |

No throughput advantage is established for any of them; an async runtime does
not make parsing faster.

## Decision

Use **one application-owned Tokio runtime** on background threads for child
process I/O, timers and supervision, as the provisional choice. Its handles
stay inside `app`; no Tokio type appears in the APIs of `core`, `store`,
`tmux`, `term` or `roster` ([0006](0006-workspace-and-crate-layout.md)).

Whatever runtime is finally chosen, these rules apply:

- Persistent blocking work runs on dedicated threads, not on a runtime's
  blocking pool. Tokio's `spawn_blocking` tasks cannot be aborted once started.
- Every child process the app starts is explicitly waited for. Dropping a
  Tokio `Child` leaves the process running, and `kill_on_drop` does not replace
  an explicit wait.
- Protocol framing reads bytes into a persistent buffer with explicit bounds.
  String line readers that require UTF-8 or are not cancellation-safe are not
  used for pane data.
- A current-thread runtime, if one is used, is driven continuously, not only
  when the user interface has work.

## Consequences

- Process supervision and timeouts get tested, well-documented primitives.
- The dependency graph may hold both Tokio and smol. The comparison below
  measures whether that costs anything that matters.

## Accept when

The comparison proposed in the research is run: replay identical recorded tmux
traffic through a plain-threads reference and through the runtime-based
design, at increasing leaf counts, and measure allocations, byte-queue
high-water marks, idle thread count and memory, input responsiveness during
output, display-copy cost and clean shutdown. If both meet the milestone-0
gate, choose the simpler one. If smol matches Tokio, prefer smol to avoid a
second runtime. A result other than Tokio supersedes this record.

## Evidence

- [Background I/O row of the verdict table](../research/rust-architecture.md#verdict-and-alternatives)
  and [concurrency and terminal ownership](../research/rust-architecture.md#concurrency-and-terminal-ownership)
  in the Rust architecture research, with Tokio 1.53.3 sources for
  [GUI integration](https://tokio.rs/tokio/topics/bridging),
  [blocking tasks](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/src/task/blocking.rs)
  and [processes](https://github.com/tokio-rs/tokio/blob/tokio-1.53.3/tokio/src/process/mod.rs).
- [GPUI executor at `93d7456`](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/executor.rs)
  and the [UI framework research](../research/ui-framework.md#gpui-lineage-and-a-reproducible-candidate).
- [What would settle the remaining risks](../research/rust-architecture.md#what-would-settle-the-remaining-risks),
  item 2.
