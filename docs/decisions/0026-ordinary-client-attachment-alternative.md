# 0026. Ordinary-client attachment alternative

- **Status:** Proposed alternative; no product transport is changed.
- **Date:** 2026-10-21

## Problem

[0015](0015-input-routing.md) proposes sending literal bytes to panes, including
attached sessions. That bypasses the attached session's prefix and custom tmux
bindings. The proposal explicitly requires acceptance of this difference. It
cannot establish full Python parity for CH-32 by itself.

A control client can route keys through tmux's key tables from version 3.4, but
copy mode, prompts and menus are not supplied as ordinary pane output. The
existing [input research](../research/tmux-control-mode.md#input-prefix-keys-mouse-and-history-ownership)
records that distinction. Supporting only bindings whose effects are easy to
recreate would still leave a compatibility gap.

## Alternative to validate

Run an ordinary `tmux attach-session -E` client in a pseudo-terminal for an
attached session. Feed its composed terminal stream into the native terminal
engine and renderer, and send terminal input back through the PTY. tmux handles
its own prefix, configured bindings and interactive displays.

This keeps tmux as the only process layer. It needs no application daemon,
private display server or web view. A workspace leaf would contain tmux's
composed screen, including the selected window's panes, instead of a separate
native terminal instance for each inner tmux pane. Workspace tabs and the
sidebar can remain native controls.

Grouping and ownership rules from [0003](0003-control-mode-connection-and-attachment-model.md)
still apply. A helper may separate window selection but shares window geometry;
its existence grants no right to change the source's options or resize policy.
This alternative does not resolve the known helper-cleanup failure.

## Fixture evidence

The test in
[`attachment_pty.rs`](../../crates/bench/src/gate5/attachment_pty.rs) uses the
existing ordinary-client PTY fixture and the native `Terminal` decoder. Every
server, session, binding and program belongs to the disposable fixture. Input
is written to that client's PTY, never injected into a desktop or existing
session.

The fixture checks:

- A custom prefix binding runs, and doubling the prefix delivers one literal
  prefix byte to the application.
- A command prompt and menu appear in the native cell grid, accept input, and
  execute their fixture commands.
- Copy mode is active and its position indicator appears in the native grid;
  exiting it restores application input.
- Prefix, prompt, menu and copy-mode keys do not leak into the application.
  Raw application markers arrive once and remain exact through reconnect.
- A grouped client's window selection changes independently of the source.
  Detach, reconnect and explicit helper removal preserve source session/window
  identity and pane PID.
- `-E` preserves a pre-existing helper session environment value.

The prefix/modes check passed on macOS with tmux 3.3a, 3.4, 3.5a, 3.6b, 3.7c
and 3.8. These results cover the assertions above, not the remaining acceptance
items below.

Run the focused check with the desired tmux binary first on `PATH`:

```sh
cargo test -p tmux-workspaces-bench attachment_pty:: -- --nocapture
```

This is decoded-cell and private-process evidence, not a visual inspection,
performance result or proof of arbitrary custom-binding behavior. The fixture
keeps automatic helper destruction off and removes its own helper explicitly;
it does not bypass or clear the separate cleanup regression. It disables
`assume-paste-time` in its own helper because synthetic bursts can otherwise be
classified as pasted text and bypass bindings. This is fixture setup, not a
proposal to override user options in the product.

### Outer-terminal protocol prototype

A second fixture uses the pinned native engine directly, with in-memory reply
and clipboard callbacks. The existing product `Terminal` wrapper suppresses
replies intentionally for control-mode pane streams; it is not an ordinary
PTY host as it stands.

The prototype returns the engine's replies to tmux through the owned PTY. A raw
fixture application asks for its cursor position and receives exactly one
response from tmux; outer-terminal negotiation does not leak extra bytes into
that application. The test also checks bracketed multiline paste, a cursor key
encoded from the outer terminal's modes, an encoded mouse click with known cell
geometry, and a tmux clipboard write decoded into a memory buffer. An application
marker after these operations verifies the preceding input remained exact.
No operating-system mouse, keyboard or clipboard service is called.

This protocol probe passed on macOS with tmux 3.4, 3.5a, 3.6b, 3.7c and 3.8.
The locally built tmux 3.3a linked to ncurses 6.6 exited unexpectedly at
`set-buffer -w`, after the reply, paste, mouse and key assertions passed. A
corresponding crash trace points to `tty_set_selection`: the generated code
accesses a null expansion returned through `tty_term_ptr2`. This is a bounded
build/platform failure, not proof that all tmux 3.3a installations fail.
A separately built tmux 3.3a linked to macOS system ncurses 5.4 passed the
same probe. A small expansion-only diagnostic also distinguished the libraries:
the older `tparm` call returned null with ncurses 6.6 and a string with system
ncurses; `tiparm_s`, used by newer tmux builds, returned a string with both.
The original failing build remains unchanged. Clipboard compatibility therefore
cannot be inferred from the tmux version number alone.

The probe is opt-in so ordinary tests do not trigger that known server crash:

```sh
cargo test -p tmux-workspaces-bench ordinary_host_routes -- --ignored --nocapture
```

It uses disposable servers even when explicitly requested. No supported-version
floor is raised and no clipboard success is inferred from an ignored test.

These assertions exercise the outer protocol, not the product input adapter.
They do not establish extended-key compatibility, clipboard read support,
selection policy, every mouse mode or real desktop-event mapping. Device
attributes are a fixture profile, not a production capability declaration.
The engine's reply callback must remain disabled for control-mode pane decoding:
forwarding those replies too would duplicate responses already owned by tmux.

## Tradeoffs and remaining acceptance

An ordinary client retains tmux's own presentation and input semantics rather
than requiring the app to reproduce every interactive binding. In exchange,
inner-pane geometry and overlays arrive as a composed terminal grid. Native
per-inner-pane selection, history and controls would need a different design;
tmux's copy mode and history remain distinct from the engine's local history.

Before adopting this alternative:

1. Complete the PTY-host contract beyond the bounded fixture: extended keys,
   terminal capability reporting, other mouse modes and clipboard interactions.
   The control-mode rule that discards engine replies cannot be reused blindly:
   an ordinary client can ask its outer terminal questions.
2. Validate resizing, source settings, helper lifetime and reconnect on supported
   versions, including the existing tmux 3.7c cleanup failure.
3. Measure the actual ordinary-client/native-renderer path against all applicable
   milestone-0 latency, throughput, frame-pacing and correctness gates. The
   control-mode path's results do not prove this alternative meets them.
4. Specify how native workspace actions and attached-session input interact,
   including the composed screen's selection/history boundary, and supersede
   the affected parts of 0003 and 0015 explicitly before product integration.

Continue evaluating this path before treating lost prefix/custom-binding
behavior as an acceptable parity result. No benchmark criterion, supported
version, persisted schema or product activation gate changes in this record.

## Dependencies and licences

The fixture reuses the existing PTY helper, `libc` (MIT OR Apache-2.0), `tempfile`
(MIT OR Apache-2.0), the MIT-licensed native terminal wrapper, and the installed
tmux executable (ISC). No package or native dependency is added or upgraded.
The protocol fixture adds the already pinned `libghostty-vt = 0.2.2`
(MIT OR Apache-2.0) as a direct benchmark development dependency so it can test
callbacks without widening the product wrapper API. No package version changes.
The alternative keeps the OS PTY boundary already used by the benchmark; a
production transport and its ownership rules remain separate work.
