# 0003. Control-mode connection and attachment model

- **Status:** Proposed. Waits on milestone-0 gate 5 and the coexistence tests
  described under **Accept when**.
- **Date:** 2026-10-17

## Context

The Python tool shows each pane through a nested `tmux attach` client inside a
private display server. A native app that draws panes itself needs each pane's
output as data, plus the session's window and pane layout.

tmux control mode (`tmux -C`) provides that: a text protocol in which tmux
reports pane output and session events to a program instead of drawing a
screen. The [control-mode research](../research/tmux-control-mode.md#decisions-that-materially-change-the-architecture)
revised the starting idea of "one control client per leaf showing one pane":

- A leaf refers to a tmux session, and a session can have many windows, each
  with several panes.
- A control connection receives output from the panes in all windows of its
  session, not only the selected pane.
- A capture of a pane is not a full terminal checkpoint, and `pause`/`continue`
  flow control discards output rather than replaying it.

Related parity rows: [CH-14 to CH-21 and CH-33](../PARITY.md#choosers-and-attaching-existing-sessions),
[SH-9](../PARITY.md#shell-environment-and-process-lifetime) and
[SH-13](../PARITY.md#shell-environment-and-process-lifetime).

## Options considered

| Option | Trade-offs |
| --- | --- |
| **Control-mode client per attachment, mirroring the session's selected window and its panes** | Keeps split panes, zoom and pane targeting visible, and follows the viewer model of iTerm2 and Ghostty. The app must parse window and layout events, keep terminal state per pane, and keep the workspace split tree separate from tmux's window split tree. |
| Show only the session's active pane | Smallest renderer integration. Hides other running panes, and a selection made elsewhere can swap the visible terminal. Rejected as the parity design. |
| Ordinary tmux client inside a pseudo-terminal, as the Python tool does | Keeps tmux's key tables, status line, copy mode and menus. The app would draw tmux's composed screen, losing native per-pane scrollback and selection and keeping nested-tmux sizing and scrolling problems. Kept only as a behaviour baseline and a fallback. |
| One control client per pane | Not needed: one connection already carries every pane of its session. More processes for no benefit. |
| One synthetic session holding every window of a server | Fewer connections, but it changes which sessions own which windows and complicates isolation and cleanup. Rejected. |

## Decision

**Transport.** Run `tmux -C` as a child process and talk to it over its standard
input and output pipes, with no pseudo-terminal. (`-CC` is meant for a client
hosted inside a terminal and is not used.) Keep protocol parsing separate from
the code that starts and reads the child, so another transport, such as
`ssh -T` to a remote host, can be added later. Remote hosts are not part of
Phase 1; that question stays open as [RM-1](../PARITY.md#remote-use-over-ssh).

**Leaves and views.** A leaf refers to a tmux session: either an ordinary shell,
`terminal-<leaf id>` on the library's shell server, or an attached session on a
source server. The leaf shows that session's selected window, with the window's
own panes laid out natively inside the leaf. The workspace split tree stored in
`layouts.db` and tmux's per-window pane layout are separate models and are
never merged.

**Connections.** Open one control connection per independently navigable
attachment, not one per pane. Keep terminal state for each pane the connection
reports. A connection's identity is the host, the server socket, the source
session and the attachment session. Pane IDs are unique only within one running
server, so after a reconnect the app rediscovers panes instead of feeding new
output into an old pane's state. Each connection carries a generation number,
and results tagged with an older generation are discarded
([0007](0007-threads-ownership-and-event-ordering.md)).

**Attaching existing sessions.** Follow the Python tool's policy (CH-14, CH-20,
CH-21):

- If the source session's `destroy-unattached` option is `off`, attach through
  a grouped helper session that the app owns, so the leaf can select a window
  without moving other clients (CH-19).
- Otherwise attach directly, because adding a group could change when the
  source is destroyed.
- Attach with `-E`, so the session keeps its own environment (SH-9).
- A helper uses the app's own name prefix, sets the session option
  `@tmux_workspaces_attachment` to `1` so neither tool offers it as an
  attachment target, and sets `destroy-unattached` to `on`. The app removes it
  when the leaf lets go or the source session ends (CH-18).
- The app never renames, kills or changes options on the source session.

**Protocol handling.** Match each command's reply by its `%begin` and
`%end`/`%error` guards and command number. A line inside a reply is reply data
even when it starts with `%`, because captured terminal text can. The one
exception is the notifications that tmux releases before 3.8 can emit inside a
reply; they are recognised only as [0014](0014-supported-tmux-versions.md)
describes, from recorded per-version fixtures. Handle notifications without
assuming they complete an outstanding command. Decode `%output` and `%extended-output`
escaping first, then feed the bytes to a persistent parser for that pane;
escape sequences and UTF-8 characters can span notifications, and pane output
need not be UTF-8. Which tmux versions interleave notifications inside replies
is covered in [0014](0014-supported-tmux-versions.md).

**Sizing.** Draw each pane at the size tmux reports, with padding when the leaf
is larger. Never detach other clients or rewrite their window options to make
a pane fit. The app declares its size for ordinary shells on the library's
shell server, as the Python tool's clients do. A grouped helper shares the
source session's windows, so owning the helper gives the app no right to
resize them. When the app may resize an external session's windows, directly
or through a helper, is a follow-up decision, settled with the coexistence
tests listed below.

**State recovery.** On attach, reconnect or after a pause, rebuild the pane's
state from tmux: captures of the visible and saved screens, pane formats and
any pending parser bytes, installed at a defined reply boundary before later
output is applied. This is the research's candidate and has not been
validated. tmux does not export every piece of terminal state, such as the
current text style or a general saved cursor, so an exact copy of an arbitrary
running program's screen cannot be promised yet. The exact command sequence
and its limits are a follow-up decision, settled by the recovery-fidelity
tests listed below.

## Consequences

- No nested display server: switching tabs starts no process, and native
  scrollback, selection and mouse handling become possible
  ([PARITY known design differences](../PARITY.md#known-design-differences)).
- The app keeps its own scrollback per pane. It is separate from tmux's
  history: on reconnect it is rebuilt from what tmux still retains, so output
  tmux has already dropped cannot come back.
- tmux draws no status line for a control client, so the app draws its own pane
  header (CH-16). tmux's mouse handling does not apply either, so the app
  forwards mouse events itself (CH-15).
- tmux's own modes, such as copy mode, are not sent as pane output. How keys
  reach tmux is decided in [0015](0015-input-routing.md).
- How inner panes look and how the user switches an attached session's windows
  remain user-interface questions under CH-33.

Follow-up decisions this record leaves open, each to be recorded when its tests
exist:

1. The recovery command sequence and its fidelity limits: settled by
   [0021](0021-pane-state-recovery.md), with
   [gate 5 passing](../benchmarks/gate-5-cursor-recovery.md).
2. When the app may resize an external session's windows.
3. Whether hidden windows keep live terminal state or are rebuilt when shown.
4. How floating panes and tmux 3.8's newer layout format are parsed.

## Accept when

The `tmux` crate is built on this design in the meantime. It is accepted when
both of these pass on disposable fixtures:

- Milestone-0 gate 5, control-mode fidelity, measured as
  [0013](0013-milestone-0-benchmark-method.md) describes, on the tmux versions
  of [0014](0014-supported-tmux-versions.md).
- The coexistence tests in the research's
  [spike list](../research/tmux-control-mode.md#spikes-needed-before-selecting-the-final-contract):
  ordinary and control clients on one session, grouped helpers, every sizing
  policy, detach, helper cleanup and reconnect, without killing source
  sessions.

If those tests show that control mode cannot reproduce pane state well enough
for parity, compare an ordinary tmux client for attached sessions in a
superseding record.

## Evidence

- [Decisions that materially change the architecture](../research/tmux-control-mode.md#decisions-that-materially-change-the-architecture)
  and [existing shell-server compatibility](../research/tmux-control-mode.md#existing-shell-server-compatibility).
- [Process lifetime, transport and protocol](../research/tmux-control-mode.md#process-lifetime-transport-and-protocol),
  [seeding and pause recovery](../research/tmux-control-mode.md#seeding-and-pause-recovery)
  and [sizing and coexistence](../research/tmux-control-mode.md#sizing-and-coexistence-with-other-clients).
- [Existing implementations and licences](../research/tmux-control-mode.md#existing-implementations-and-licences):
  iTerm2 (GPL, read for behaviour only), WezTerm (MIT) and Ghostty (MIT).
- [Parity checklist](../PARITY.md#choosers-and-attaching-existing-sessions),
  rows CH-14 to CH-21 and CH-33.
