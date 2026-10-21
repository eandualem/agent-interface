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
explicit terminal click or navigation to a ready screen, and go to an exact visible pane.
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
Command-number tab and Command-Option-number workspace navigation run before
terminal key handling. Command-brackets cycle panes; adding Shift cycles tabs.
Command-Option-arrows cycle workspaces, Command-Shift-Enter toggles pane expansion,
and Command-B focuses navigation. A selected connected leaf receives input focus
on its last selected ready inner pane, or its initial/first ready pane. Unavailable
or newly connecting destinations leave focus in navigation; no input waits for a
future connection. Structural library changes still return focus to navigation.

Selection owns a local copy of the selected pane's display. Live output continues
to drain into its latest screen while the selected copy stays visible. Dragging
selects within that pane; double-click selects a word and triple-click selects
a row. Native copy writes the selected text to the clipboard and keeps the
highlight. Escape, typing, explicit scrolling or a new outside click clears it;
layout changes may clear it too. Selection works on a retained disconnected screen. Completed primary, middle and
secondary clicks and wheel gestures use the destination's mouse encoder when
reporting is enabled. In a mouse-enabled live program, Shift starts local selection;
unmodified multi-clicks belong to the program. Local selections never become
application clicks, even if Shift is released or a drag returns to its anchor.
Shift, or an already scrolled
history viewport, keeps wheel navigation local; mouse clicks in that viewport
are not sent to the running program. Continuous pixel-wheel gestures retain local
ownership through their remaining events and momentum, including capture recovery;
a new platform start event begins a new gesture. Discrete wheel ticks decide
individually. A geometry change rejects stale mouse
coordinates. Pointer motion reports remain pending.

## Alternatives and limits

Keeping placeholders cannot support terminal work. A separate demonstration
window would exercise a different path from saved workspaces. Reuse the chosen
renderer in the product itself. No new dependencies or licences are introduced:
GPUI (Apache-2.0), libghostty-vt (MIT) and tmux (ISC) retain their recorded pins.

Grouped helper sessions, window switching, selection across history pages, input
methods, key-release events, mouse motion reporting and tmux prefix compatibility
remain follow-up work under 0003 and 0015. The first connection observes the
window selected at connection time; it does not promise independent tmux window
navigation. Scaling preserves source geometry but is not final sizing parity. Selection
retains graphemes and joins soft wraps known to the engine, but recovery captures
currently omit soft-wrap flags (0021): copied recovered rows keep visual line
breaks. Authoritative captured wrap metadata remains follow-up work.

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
