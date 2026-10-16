# 0002. tmux is the only process layer

- **Status:** Accepted
- **Date:** 2026-10-17

## Context

The Python tool keeps every shell in tmux, so shells and programs survive when a
window closes, and existing tmux sessions can be attached. The product contract
for this app keeps that model: tmux is the only process layer, and the app is a
client that runs no daemon of its own.

The [control-mode research](../research/tmux-control-mode.md) confirmed this
model, with two qualifications: tmux servers own shells, terminals and
sessions, but options such as `destroy-unattached` or `exit-unattached`, kill
commands, a shell exiting and a host shutdown can still end them. A client must
respect the policy a server already has rather than promise unconditional
persistence.

Parity rows that depend on this: [SH-11 to SH-17](../PARITY.md#shell-environment-and-process-lifetime),
[PE-14](../PARITY.md#persistence-concurrent-windows-and-conflict-handling) and
[LP-18](../PARITY.md#launchers-tpm-and-packaging).

## Options considered

| Option | Trade-offs |
| --- | --- |
| **tmux owns every shell; the app is a tmux client** | Shells outlive the app and can be shared with other tmux clients and with the Python tool. tmux's own processing sits on the output path and caps bulk throughput, and the app must rebuild pane state after a reconnect because tmux keeps no replay log. |
| The app owns the terminals (pseudo-terminals) itself, like an ordinary terminal emulator | Shortest output path. Shells die with the app, and attaching existing tmux sessions would need a second mechanism. Rejected by the research: it loses the lifetime and attachment model the product needs. |
| A new persistence daemon owned by the app | Could own sessions and speak a custom protocol. Duplicates what tmux already does and adds a long-lived process to install, upgrade and debug. Rejected by the product contract. |

## Decision

tmux servers own every shell, terminal and session the app shows. The app owns
only these:

- its windows and the threads inside its own process;
- the tmux client processes it starts, such as control-mode connections and
  one-off commands, which it reaps when they end;
- the grouped helper sessions it creates for attachments, as described in
  [0003](0003-control-mode-connection-and-attachment-model.md), which it
  removes when it no longer needs them.

The app starts no daemon, background service or helper server. The only tmux
servers it may start are the library's shell server, the same server the
Python tool uses ([0009](0009-python-library-and-shell-server-compatibility.md)),
and the disposable fixture server of demo mode
([DM-1](../PARITY.md#demo-and-preview-modes)). The shell server belongs to
tmux once started; closing the app never stops it. The demo fixture server is
the one exception: like the Python tool, the app stops the fixture server it
started when its demo window closes, and a new demo launch starts a fresh one
(DM-3). Ordinary shells in the demo library persist like any other library's
(DM-2).

The app never runs `kill-server` except on a demo fixture server it started
itself, and never changes options on a server or session it did not create.
Closing a window or quitting detaches. An ordinary
shell ends only when the user explicitly closes its pane or tab (SH-12), and
external sessions are never ended by the app (CH-13).

## Consequences

- Quitting or crashing the app leaves shells and programs running; reopening
  reattaches to them.
- Bulk output can be no faster than tmux itself. The performance target is
  therefore set against Ghostty running tmux, not against a terminal without
  tmux ([0013](0013-milestone-0-benchmark-method.md)).
- Every connection can be lost and must be recoverable: the app rebuilds pane
  state from tmux on reconnect, within the limits described in
  [0003](0003-control-mode-connection-and-attachment-model.md).
- Cleanup code must only ever target processes and sessions the app created,
  and must say so in its tests.

## Revisit when

The product contract changes. Performance alone is not a reason: the bulk
output target already includes tmux.

## Evidence

- [tmux control mode and process model](../research/tmux-control-mode.md):
  hypothesis 1 verdict, [process lifetime](../research/tmux-control-mode.md#process-lifetime-transport-and-protocol)
  and the [comparison with app-owned terminals and a new daemon](../research/tmux-control-mode.md#existing-implementations-and-licences).
- [Parity checklist](../PARITY.md#shell-environment-and-process-lifetime),
  rows SH-11 to SH-17.
