# 0007. Threads, ownership and event ordering

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

The window's event loop must run on the main thread. Pane output can arrive
continuously from many panes, and parsing it must not stall typing or drawing.
The terminal engine's Rust handles hold raw pointers and are not `Send` or
`Sync`, and a render snapshot borrows its render state, so the handles cannot
simply be passed between threads
([0004](0004-terminal-emulation-core.md)). The single SQLite connection must
stay open between polls ([0009](0009-python-library-and-shell-server-compatibility.md)).

The Python tool guarantees that text typed right after a navigation action
reaches the newly selected shell exactly once, even during rapid switching
([IN-10 and IN-11](../PARITY.md#mouse-and-keyboard-input-selection-and-copying),
[PE-16](../PARITY.md#persistence-concurrent-windows-and-conflict-handling)).
A native app must keep that guarantee without the Python tool's private action
socket.

## Options considered

| Option | Trade-offs |
| --- | --- |
| Everything on the user-interface thread | Simple, but blocking reads and large parse batches would delay input handling and frames. Rejected. |
| A thread per pane | Easy to reason about per pane, but thread count grows with every attached leaf, and each needs its own drain and cancellation. Not needed. |
| **Dedicated owner threads, with owned data crossing between them** | Terminal handles stay on the thread that created them; the user interface receives copies it owns. Costs a copy of display data per update, which must be measured. |
| Share engine handles across threads behind a lock, or assert `Send` with `unsafe` | Avoids copies, but the binding does not support it and a borrowed snapshot cannot cross threads safely. Rejected. |

## Decision

**Threads.** Inside the single app process:

- The **user-interface thread** runs the window event loop and draws.
- A **terminal worker** constructs, feeds, resizes and destroys terminal
  engine handles and their render state, and publishes owned display data:
  rows, cursor and mode information, each update tagged with a revision. One
  worker owns all terminals to start with; more are added only if fairness
  measurements require it.
- A **storage worker** owns the one retained SQLite connection.
- **Background input and output** drives tmux child processes and timers. The
  runtime used for this is decided in [0008](0008-background-io-runtime.md).

These are threads in one process, not extra process layers. The `core`,
`store` and `term` APIs stay synchronous; `app` owns scheduling and shutdown.

**Data flow rules.**

- Keep each pane's bytes in order, with a persistent parser state per pane.
- Bound queues by bytes, not only by message count. Drain a child's standard
  output and standard error independently.
- Coalesce repaint requests and replace superseded complete display snapshots,
  but never discard undecoded protocol bytes or ordered terminal events to save
  work. If updates are coalesced, the consumed update must still contain every
  row change made since the last one consumed.
- Give input, resizes and disconnects a chance to run during sustained output by
  bounding the size of each parse batch and snapshot. Queue limits and batch
  budgets are set by measurement.
- Treat a tmux pause or a transport gap as a recovery state, not as data loss
  to hide ([0003](0003-control-mode-connection-and-attachment-model.md)).
- Tag asynchronous results with their connection generation, so a result from
  an old connection cannot change a newly connected pane.

**Input ordering.** The `app` controller owns the current input target: a
connection generation plus an exact session and pane. User-interface navigation
and terminal input enter one ordered event stream.

1. A navigation action immediately holds back the key and paste events that
   follow it, in a bounded queue tagged with that navigation.
2. Held events are encoded for the destination only after its connection
   confirms the attachment is ready.
3. The controller then commits the target, updates focus and the displayed
   view, acknowledges the navigation and releases the held input to that exact
   pane. Input accepted earlier keeps its original target.
4. Navigations are applied one at a time, in the order they were made. A
   navigation made while an earlier one is still waiting is queued behind it;
   the earlier one is not abandoned, so input typed for it still reaches its
   destination exactly once (IN-10, [PF-12](../PARITY.md#performance-budgets)).
   This keeps the Python tool's ordering. The research's draft rule, which
   rejected input held for a navigation that a later one superseded, would
   drop keystrokes during burst navigation.
5. A navigation that fails, or whose connection drops, rejects its held input
   with a visible status. That input is never replayed into the old pane or a
   later selection. A full queue is also reported as a rejection. After a
   failed switch, input stays held until a selection succeeds (IN-11).
6. A late attachment confirmation cannot acknowledge a different navigation.

**Shutdown.** Stop accepting new requests, end and reap only the tmux client
processes the app started, then join the workers. Sessions belong to tmux and
keep running ([0002](0002-tmux-is-the-only-process-layer.md)).

## Consequences

- Copying display data to the user-interface thread has a cost that the
  milestone-0 benchmark measures ([0013](0013-milestone-0-benchmark-method.md)).
- Tests must cover rapid and burst switching with typing or paste between
  switches, delayed attachment confirmation, failures and disconnects, and
  check that input reaches only its intended ready pane, exactly once
  ([0012](0012-testing-strategy-and-tmux-isolation.md)).
- Generation tags and per-pane parser state make reconnect handling explicit
  rather than accidental.

## Revisit when

The chosen engine's handles become safe to share and measurements show the
display-data copy is a bottleneck, or fairness measurements show one terminal
worker cannot keep every pane responsive.

## Evidence

- [Concurrency and terminal ownership](../research/rust-architecture.md#concurrency-and-terminal-ownership)
  in the Rust architecture research, including the Python tool's input-ordering
  contract it cites.
- [Ownership and tmux effects](../research/terminal-core.md#ownership-and-tmux-effects)
  and [render state is not a renderer](../research/terminal-core.md#render-state-is-not-a-renderer)
  in the terminal-core research.
- [Parity checklist](../PARITY.md#mouse-and-keyboard-input-selection-and-copying),
  rows IN-10 and IN-11, and [PE-16](../PARITY.md#persistence-concurrent-windows-and-conflict-handling).
