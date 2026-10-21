# 0025. Existing-session terminals in the native workspace window

- **Status:** Accepted for development; native visual and performance acceptance pending.
- **Date:** 2026-10-21

## Decision

Replace the saved pane cards with the existing native `TerminalView` and `Panes`
transport. Each visible nonempty leaf connects to its actual session: an ordinary
shell uses the library-compatible socket and `terminal-<leaf id>` identity; an
attachment uses its saved source socket, falling back to the window's captured
source. Query the exact session name, then attach to its numeric identity using
`-E` and `ignore-size`. Do not start a server, create a shell, change source
options or resize other clients. The initial window's visible panes render at
reported geometry, scaled down if needed. A pane can be expanded locally.

Connection discovery and capture run in the background. Each leaf entity owns
its connection generation; a removed entity cannot receive a stale result.
Failed connections show an error and a reconnect action. Disconnect retains the
last screen and offers reconnect without closing the app. Keys require an
explicit terminal click and a ready screen, and go to an exact visible pane.
A bounded input queue keeps pipe writes off the UI thread. The terminal worker
encodes keys and paste using each destination's current modes after preceding
queued output. It retains legacy shell control bytes when extended keys are off
and reads the source server's `extended-keys-format` for modified keys. A source
option subscription refreshes this setting after changes. During capture recovery
it waits for authoritative modes, with the capture timeout. Clipboard text is sanitized by the pinned engine and uses
bracketed paste when enabled. Each operation is limited to 4 MiB and the UI queue
holds at most 64 operations; rejected input is reported in the terminal view.
Queue and size rejection messages remain visible until a ready terminal is
clicked, independently of earlier successful operations. Navigation moves
focus back to the sidebar, as do structural changes from the shared library.

Use the Python panel organization: workspace chooser in the heading, numbered
tabs with pane counts, saved split proportions and a compact bottom status row.
The library reader and conflict handling remain unchanged; no database writer
or editing controls are enabled. History navigation uses the terminal engine's
local viewport: wheel/trackpad and Shift-Page Up/Down/Home/End operate on the
pointed or focused pane. It leaves the shared tmux copy mode unchanged. Typing
returns to live output. Capture recovery retains the local history row, clamped
to the retained history; pruning or geometry changes can change its contents.
Command-number tab and Command-Option-number workspace navigation run before terminal key handling.

## Alternatives and limits

Keeping placeholders cannot support terminal work. A separate demonstration
window would exercise a different path from saved workspaces. Reuse the chosen
renderer in the product itself. No new dependencies or licences are introduced:
GPUI (Apache-2.0), libghostty-vt (MIT) and tmux (ISC) retain their recorded pins.

Grouped helper sessions, window switching, native text selection, input
methods, key-release events, mouse reporting and tmux prefix compatibility
remain follow-up work under 0003 and 0015. The first connection observes the
window selected at connection time; it does not promise independent tmux window
navigation. Scaling preserves source geometry but is not final sizing parity.

## Verification

A disposable private server test connects from a saved attachment, recovers two
existing pane screens, sends a marker to exactly the selected pane, preserves
geometry and environment, and detaches without stopping the source. Exact-name
lookup and absent sockets are checked without creating a server. The existing
terminal recovery and transport tests remain applicable. This does not establish
visual, keyboard-event or benchmark acceptance on a native desktop. A private
raw-terminal fixture also checks actual application-cursor key and bracketed-paste
bytes delivered through the worker. Engine tests cover mode changes, extended
keys, ordinary control keys and sanitization of pasted escape/control bytes.
A private-session history fixture checks scrolling through existing output,
retaining that view during wrap recovery and returning to live output on input.
