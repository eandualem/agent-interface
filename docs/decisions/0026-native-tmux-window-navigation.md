# 0026. Local navigation between existing tmux windows

- **Status:** Accepted for development; native desktop acceptance pending.
- **Date:** 2026-10-21

## Decision

Show the connected session's existing window indices and names above its native
terminal. Selecting one replaces only that leaf's control connection and captured
window. During a switch, the old client remains attached with input disabled
and its view hidden until the replacement is ready. This avoids an unattached
interval for sources with automatic destruction policies. A failed switch retains
the old view, requiring a ready-pane click before typing resumes. It never invokes `select-window` on the source session. The list follows
tmux by polling (see [0032](0032-source-equivalent-chrome-and-sizing.md)). An unavailable or changed target
shows an error and offers reconnect to the source's current window.

A listed window carries its server PID, session creation time and numeric
session identity. A later selection must match that source and still belong to
the session. After spawning, the app verifies the actual control client’s
server/session identity before allowing capture or input; a mismatch or attach
timeout stops only that owned client. Capture then uses a session-qualified
window target on that client. Input also qualifies every pane by its source
session and window, so tmux rejects it immediately if that membership changes.
An unlink notification or source-membership subscription marks the view unavailable
when the window leaves the source; subscription updates can take up to one second.
The subscription covers tmux 3.8, which uses a different close notification.
Exact numeric window/pane targets are separate from display names.
Window-list records decode tmux's shell quoting before interpreting names, so
spaces, backslashes and escaped newlines cannot inject another record. Invalid
UTF-8 in a display name is replaced for display. Each query is bounded by the
existing three-second discovery timeout and output limit and runs off the UI
thread. A new terminal requires explicit input focus after connection.

The first connection uses the source's current window. Choice remains local to
each saved leaf through resizing and workspace/tab switches. Hidden leaves release
their clients and renderers; returning reconnects the last successfully selected
window, with the same source-identity and membership validation. Changed or removed
leaves discard their remembered choice. Opening a library clears all remembered
choices. No choice is written to the database. Lists refresh on connection, selection and every two seconds while connected. Missing sessions/windows are never created. Saved layouts,
source selection, environment and geometry remain unchanged.

## Alternatives and evidence

The grouped-helper proposal in [0003](0003-control-mode-connection-and-attachment-model.md)
provides an independent tmux current-window setting but introduces session
creation, destruction-policy handling and cleanup. Native selection needs no
such setting: the existing control client receives output from other windows,
and capture/layout commands already target numeric window identities. Keeping
selection entirely native is smaller and avoids adding a second session
lifecycle. This supersedes the helper requirement for this native-only path.
tmux key-table compatibility remains outside this decision.

Selecting the source's window would move other clients and is rejected. Showing
only its current window prevents useful navigation and is replaced by the native
controls. No new dependencies are introduced; tmux (ISC), GPUI (Apache-2.0) and
the terminal engine (MIT) retain their existing pins and licences.

A private-server integration test captures a non-current window, observes its
continued output and sends input to its exact pane while the source remains on
its original window. Moving the chosen window out of the source stops existing input, rejects an
already attached but not yet captured connection, and makes subsequent selection
fail.
The same handoff is tested with `destroy-unattached=on`.
The name-framing unit test rejects forged records and mismatched source identity.
Protocol probes on tmux 3.3a, 3.4, 3.5a, 3.6b, 3.7c and 3.8 confirm the source
identity fields and quoted-name decoding. The older four versions accept newline
window names; the newer two reject them. Desktop event acceptance remains pending.
