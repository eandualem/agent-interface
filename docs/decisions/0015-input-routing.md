# 0015. Input routing

- **Status:** Proposed. Waits on the input and terminal-ownership tests
  described under **Accept when**.
- **Date:** 2026-10-17

## Context

In the Python tool, the viewer's own prefix (`Ctrl-g`) travels through a tmux
prefix table, and an attached session's prefix (such as `Ctrl-b`) reaches a
nested tmux client inside the leaf. A control-mode client has no nested client
([0003](0003-control-mode-connection-and-attachment-model.md)), so the app must
decide where every key, paste and mouse event goes.

The control-mode research observed two delivery paths:

- `send-keys -H` delivers literal bytes to a pane and bypasses tmux's prefix
  handling. A test sent `Ctrl-b q` this way; the program received both bytes
  and a custom prefix binding did not run.
- `send-keys -K` (tmux 3.4 and later) routes a key through the client's key
  tables, so prefix and custom bindings run. Those bindings can change the
  session's selected window or pane, or open tmux modes, prompts and menus,
  whose screens are not sent as pane output.

tmux also answers terminal queries, such as device attributes and cursor
position reports, on the pane itself. The app's terminal engine sees the same
queries and generates its own replies.

Parity rows: [CH-32](../PARITY.md#choosers-and-attaching-existing-sessions)
(open), [IN-1, IN-2, IN-12, IN-19, IN-20, IN-23 and IN-25](../PARITY.md#mouse-and-keyboard-input-selection-and-copying).

## Options considered

| Option | Trade-offs |
| --- | --- |
| **Native bindings plus literal bytes** | The app reads every key. Its own actions run natively; everything else is encoded for the pane's current modes and sent as literal bytes to an exact pane. Predictable and fully under the app's control. Bindings defined in an attached session's own tmux configuration do not run. |
| Route keys through tmux key tables with `send-keys -K` | Keeps an attached session's prefix and custom bindings. Needs tmux 3.4 (3.8 for correct ordering), and the app would need native presentations for whatever those bindings open, such as copy mode or menus. |
| Imitate common tmux bindings natively | Familiar keys without tmux's modes, but guesses at each user's configuration and can disagree with it. |

## Decision

1. **The app reads every key first.** Its own prefix (`Ctrl-g` by default),
   prefix actions and direct shortcuts run natively (IN-1). Escape cancels a
   pending prefix, and pressing the prefix twice sends it literally (IN-2).
2. **Everything else is literal bytes.** Keys are encoded for the destination
   pane's negotiated modes using the terminal engine's key encoder, and sent
   with `send-keys -H` to an exact pane target, never to "the current pane".
   This applies to attached sessions too: their tmux prefix reaches the
   program as ordinary bytes. Switching an attached session's windows is
   offered by native controls, designed under CH-33.
3. **`send-keys -K` is not used in Phase 1.** A tmux key-table compatibility
   mode for attached sessions may be proposed later, in its own record, once
   its effects can be presented.
4. **Paste** uses bracketed paste when the pane's program has turned it on
   (IN-19, IN-20).
5. **Input methods.** Only committed text is sent. Composition (preedit) text
   is drawn locally and never sent to the shell (IN-23).
6. **Mouse.** Selection, local scrollback scrolling and other app gestures are
   handled by the app. When the pane's program has asked for mouse reports,
   the app encodes them for that program's mouse mode and sends them as literal
   bytes (IN-12, IN-25).
7. **Terminal replies.** Replies the terminal engine generates for terminal
   queries are not forwarded to tmux, because tmux already answers them for the
   pane. Any reply the app does need to send, such as colour reports through
   `refresh-client -r` on tmux 3.5 and later, is added to an explicit list in
   a later change, with its own test.
8. **Ordering** follows [0007](0007-threads-ownership-and-event-ordering.md):
   after a navigation, input waits for the destination to be ready and goes
   only to that pane.

## Consequences

- An attached session's own tmux bindings, copy mode and menus are not
  available inside the app in Phase 1. This is a visible difference from the
  Python tool that CH-32 must record when this record is accepted.
- Literal bytes work on every tmux version from the floor in
  [0014](0014-supported-tmux-versions.md).
- The terminal engine's key and mouse encoders become part of the input path,
  so their behaviour is covered by the fidelity corpus
  ([0004](0004-terminal-emulation-core.md)).

## Accept when

Run through private tmux servers ([0012](0012-testing-strategy-and-tmux-isolation.md)):

- Rapid switching followed by typing and paste delivers each marker once, to
  the intended pane only ([PF-5 and PF-12](../PARITY.md#performance-budgets)).
- Device-attribute and cursor-position queries get exactly one reply, from
  tmux.
- Extended keys, bracketed paste, application mouse modes and hyperlinks behave
  the same as in an ordinary tmux client, on first attach and after the pane's
  state is rebuilt.
- The maintainer accepts the CH-32 behaviour above for attached sessions.

## Evidence

- [Input, prefix keys, mouse and history ownership](../research/tmux-control-mode.md#input-prefix-keys-mouse-and-history-ownership)
  in the control-mode research.
- [Ownership and tmux effects](../research/terminal-core.md#ownership-and-tmux-effects)
  in the terminal-core research.
- [What the terminal element must own](../research/ui-framework.md#what-the-terminal-element-must-own)
  in the UI framework research, for input-method handling.
- [Parity checklist](../PARITY.md#mouse-and-keyboard-input-selection-and-copying),
  rows IN-1 to IN-25, and [CH-32](../PARITY.md#choosers-and-attaching-existing-sessions).
