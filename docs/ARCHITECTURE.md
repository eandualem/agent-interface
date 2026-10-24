# Architecture

Agent Interface is a native desktop client for tmux. The app owns the window
and saved arrangement; tmux owns sessions and their processes. Terminals are
drawn with GPUI, using Ghostty's terminal core to interpret output. There is no
web view and no app daemon.

## Processes and connections

A **library** is a directory containing `layouts.db`. It stores workspaces,
tabs and split layouts. Each leaf in a layout either opens an ordinary shell
on the library's persistent tmux server or attaches to an existing tmux session.
An empty leaf starts no shell.

Leaves connect on first display and retain their control-mode clients (`tmux -C`)
when tabs or workspaces change. The owning native window keeps each connection
until its target is removed or replaced, the library changes, or the window
closes. Hidden terminals continue consuming output, preserving their state at
the cost of retaining their workers and scrollback. A saved Detach action clears
the attachment and returns the same pane to its chooser. Detach, Close Pane and
Close Tab never send a session-termination command. Closing the final pane closes
its tab. An attached session retains its own lifetime policy, including tmux's
`destroy-unattached` option.

Each displayed native viewport participates in tmux's window sizing. Hidden
clients keep their connections with `ignore-size`; their next paint restores
sizing participation at the current viewport dimensions. Resizing a viewport can
change the geometry seen by other clients of the same window. Choosing a tmux
window within an attachment is local to that native leaf; it does not switch
another client's selected window.

## Crate boundaries

| Crate | Responsibility |
| --- | --- |
| `core` | Workspace, tab and split models; validation; local navigation; three-way merge rules; compatibility keymap and theme data |
| `store` | Initializing, reading and transactionally saving schema-v2 `layouts.db` files |
| `tmux` | Server discovery and validation, control-mode protocol, pane capture, recovery and input commands |
| `term` | Terminal engine ownership, input encoding and owned display snapshots |
| `roster` | Read-only Agent Backbone roster and pane-view data, with bounded loopback requests and stale-state handling |
| `app` | Native windows, input routing, storage and terminal workers, and integration between the other crates |

`app` connects the product crates. `store` also depends on `core`; the other
product libraries do not depend on one another. GPUI stays inside `app` and
terminal-engine types stay inside `term`. The dependency constraints are encoded
in [`deny.toml`](../deny.toml).

Development-only crates are separate: `test-support` provides disposable tmux
fixtures, `bench` runs measurements, and `bench-rio` provides an isolated
comparison-engine executable.

Pane rearrangement lives in `app::desktop::pane_drag`; `core` moves an existing
leaf within its tab and collapses its previous parent. Dragging paints only a
destination highlight, so pointer motion does not resize terminal sessions.
On release, the UI previews the detached tree while the existing save worker
commits it. Leaf identities retain their terminal/view entities. A changed
tab, tree or pane geometry cancels an in-progress drag; persistence keeps the
existing schema and conflict rules.

## Output, drawing and input

A background reader decodes tmux control messages into pane output, command
replies and layout events. A terminal worker owns the engine handles and feeds
output into each pane's terminal state. It publishes owned `Screen` data for the
UI thread to draw, so drawing does not borrow a live engine or wait for I/O.
If copying a screen fails, the worker reports a connection failure and stops
publishing updates. Partially copied frames never replace the last complete
display; embedded terminals offer the existing reconnect action.

On connection or recovery, the worker captures the pane's current state and
only the history its local scrollback retains, then continues with subsequent
output. Initial snapshots are released after restoration. The reader queues
output only for panes in its selected window. Recovery uses the state tmux
can report; it cannot reconstruct every historical parser or input mode.

The UI handles application shortcuts first. Other keys and paste operations
are encoded for the destination pane's reported input modes and sent as literal
bytes to that exact pane. Navigation does not redirect queued input to another
pane. Input waits until a newly selected destination is ready.
Each view admits at most 64 outstanding input operations and 8 MiB of payload,
including input retained by an encoding request after its caller times out,
with a reserved slot and 64 bytes for a
mouse release. A single paste remains limited to 4 MiB. Excess input is rejected
visibly without blocking the UI.

The focused terminal exposes its active pane and input caret through GPUI's
native accessibility bridge. Text ranges preserve terminal grapheme clusters
and cell positions. The caret follows an in-viewport program cursor even when
the program hides its drawing. An off-viewport cursor or capture-only recovery
placeholder is not exposed. When that position is unavailable, the node exposes
output as a plain value without text-run children, disabling native text-range
attributes rather than letting the macOS adapter invent a selection at zero.
Embedded terminals bypass GPUI's drawing cache while accessibility is active,
because that cache does not replay accessibility nodes or focus. On macOS accessibility
is active from window creation, so embedded terminals always bypass this cache.
The terminal role distinguishes inspectable program output from an editable
document: dictation tools can locate the input and send a normal
paste, while replacing the output buffer through a value setter is unsupported.

Closing a pane allows queued input, including a final mouse release, up to one
second to drain, then cancels the connection. A control client that cannot
detach within another second is terminated; its tmux session is not terminated.
An input write that times out terminates its connection before returning, so
queued input cannot arrive later. Disconnect diagnostics retain tmux exit
reasons and transport failures.

The app uses background workers for storage and terminal work. The library
crates expose synchronous operations; `app` owns scheduling, cancellation and
shutdown. Workers pass owned data rather than engine or database handles to the
UI thread.

## Saving and shared libraries

An absent library is initialized with one empty workspace. Existing databases
are opened without automatic repair or replacement. Invalid or unreadable files
remain intact and produce an error in the UI.

A storage worker polls for changes made by other windows or compatible library
writers. Edits are reconciled against the document they started from using the
shared merge rules. Structural changes are saved before replacing the visible
arrangement. Conflicting edits refresh the shared arrangement and ask the user
to repeat the edit; failed saves offer Retry or Discard.

Workspace and tab selection, pane expansion, and attachment-window selection
are local to each native window. Ordinary-shell identities belong to the saved
library. Closing a native window detaches its clients without issuing
session-termination commands; navigation keeps them attached.

## Appearance and integrations

Native appearance and shortcuts are saved separately in `settings.toml` under
`$XDG_CONFIG_HOME/tmux-workspaces`, or `~/.config/tmux-workspaces`. Shared colors,
spacing and components are described in [Design](DESIGN.md). The settings reader
accepts regular files up to 64 KiB, including symlinks to them; unreadable,
oversized or invalid settings keep the defaults without waiting on a pipe.
Saves replace the destination atomically and clean up only temporaries they
created. Failure to start a save worker follows the normal save-error callback
and releases its pending-write accounting.
Application shutdown waits up to ten seconds for accepted layout writes and
two seconds for settings writes, including writes from already-closed windows.
The wait tracks file workers, independently of their UI result handlers. A
write still blocked beyond that shutdown allowance may remain unfinished.

The compatibility theme reader validates the legacy theme format but does not
control native appearance. Likewise, the legacy keymap inspection commands do
not replace native Settings shortcuts.

The Agent Backbone adapter resolves configuration lazily and uses
bounded, read-only requests to an approved loopback endpoint. A failed refresh
must not present an old agent state as current. The core does not depend on
Backbone or the Python implementation. The `roster::views` adapter reads and
decodes view data; `app::desktop::views` renders it in native panes. Each
polling worker reuses its HTTP client while validating the endpoint and
credentials for every request. Selection changes coalesce and cancel obsolete
request chains. Views and empty-pane choosers hidden by an expanded pane pause
their polling. Revealing a chooser refreshes session availability and Team data,
preserving its filter and selection while those reads finish. Agent Usage
refreshes every 15 seconds, like the team-wide Usage view.
Local provider files must resolve to regular files, so pipes cannot leave a
reader waiting for a writer. The `.env` read is limited to 1 MiB;
permission files reject input beyond 1 MiB and skill previews retain their
256 KiB limit. Permission inspection distinguishes missing optional files from
read, size and parse failures, showing incomplete results explicitly. Rule
deduplication preserves file order and first occurrence with set membership.

Agent views cache unchanged render trees; data, size, focus and appearance
changes invalidate them. Visible working and starting indicators animate using GPUI; other states are
static. State labels retain the distinction between starting and busy, and
between blocked and waiting for a person. The roster reads immediately and then
two seconds after each completed request; Team and agent-card views poll every
three seconds after a read. Requests have a three-second deadline, and failures
mark cached observations stale.
Team-wide libraries (`roster::views::library`) are gathered from per-agent
reads, the store, each agent's skill preview and its brief, for at most 32
agents per refresh, merged by file path so no row names an agent. They poll
every 30 seconds. An agent view reads only the part its shown section needs.
The Activity timeline draws the last hour from `/api/agents/states`, the
states Backbone observed. It reads from six hours before the hour, so a state
that began within those six hours is drawn from the hour's start; an older
one is not known. It follows the `before_id` cursor for up to four pages of
500, says so when that limit leaves the oldest observations unread, and stops
when the pane closes. Deliveries are ticks and
each agent's state now sits beside its lane. A Backbone that answers 404 or
405 keeps no history and the timeline shows deliveries and current states;
when history fails to load, the legend says so.
A saved view records the agent section it opens on (`view_section`); a pane
saved by an earlier version as one agent's policies opens on that agent's
Policies section and is written in the new form with the next change.

Reports use Backbone's existing read-only `/api/reports` and `/api/responses`
feeds. The native pane keeps report history, latest owner blockers and retained
answers distinct. Pagination follows each feed's cursor; Refresh starts at the
newest page. An open record keeps its contents through background refreshes.
An open Response is polled by ID so its correction metadata stays current even
after newer publications move it off its former list page.
Authored status and blocker text do not change runtime state or grant approval.

The experimental Artifacts pane is independent of Backbone. `app::artifacts`
copies a UTF-8 HTML document and small metadata record into an immutable UUID
directory under the selected library's `artifacts/`, publishing by atomic
rename. The CLI does this without starting a window, tmux client or daemon.
Discovery reads metadata off the UI thread every two seconds while visible;
the catalog retains the newest 200 entries. HTML is read only on opening a
document and is limited to 4 MiB, including inline assets.

`desktop::views::artifact_web` is the system-webview boundary. On macOS it
creates a Wry/WebKit child view only for an opened document, positions it in
the pane's content rectangle, and updates its theme variables. The view is
hidden while native overlays or pane dragging need that space. Closing the
document or hiding its pane releases the web view, so reopening starts a fresh
document runtime. Native terminals, menus and report views do not use WebKit.
WebKit's platform processes belong to the renderer; the app adds no daemon.
Linux currently shows an explicit unsupported-renderer message: embedding
WebKitGTK in GPUI's Wayland/X11 windows needs a separate integration.

Artifacts use a private in-memory URL, an ephemeral browser session and a
content policy that permits inline HTML/CSS/JavaScript, SVG, canvas and data
assets but blocks external resources, network requests, frames and form
submission. Navigation, downloads, new windows and permission requests are
denied. The sole JavaScript bridge reports pointer focus to select the owning
pane; it exposes no terminal commands, filesystem access or Backbone API.
This isolates the integration, but does not make arbitrary animations free:
visible documents still consume WebKit CPU and memory according to their code.

## Dependencies and licences

The project is MIT-licensed. [`Cargo.toml`](../Cargo.toml) declares dependency
versions, `Cargo.lock` records the resolved graph, and `deny.toml` checks licences,
advisories and permitted sources.
The audit runs separately for both architectures on macOS and Linux. Wry is
reachable only in macOS graphs, so its unused GTK and Android dependencies are
not part of any supported build. No advisory or licence exception is added for
those dependencies.

- **GPUI (`gpui-pre` and platform packages):** native windowing and GPU drawing,
  pinned as one package family. Apache-2.0. The macOS 0.3.9 backend is
  [vendored with a narrow native focus correction](../native/gpui-macos/README.md):
  window focus queries reach the content-view accessibility adapter, and the
  adapter starts with the window's current key status. Accessibility is primed
  on window creation so the first external query sees the rendered text tree.
  This builds accessibility data during rendering even before a client connects;
  it trades some rendering work for reliable first-use dictation detection.
  The dependency graph remains unchanged.
- **Plasma Wayland protocols:** GPUI's Linux backend uses the MIT-licensed
  `wayland-protocols-plasma` wrapper. Its vendored protocol definitions also
  carry LGPL-2.1-or-later, MIT-CMU and BSD terms, preserved in the
  [Linux protocol notices](../packaging/wayland-notices.txt). Cargo licence
  metadata alone does not describe those generated-binding inputs.
- **`libghostty-vt`:** terminal emulation behind the `term` boundary, pinned with
  its native binding. MIT OR Apache-2.0; Ghostty's core is MIT-licensed.
- **`rusqlite`:** transactional access to the existing SQLite library format.
  MIT. Its bundled SQLite uses SQLite's public-domain dedication.
- **`libc`:** Unix constants for nonblocking configuration reads in the app
  and roster, and the existing process integration. MIT OR Apache-2.0.
- **`wry` 0.57.1:** macOS system WebKit child views for experimental HTML
  artifacts, pinned exactly and enabled only on macOS. MIT OR Apache-2.0.
  It does not render terminals or supply a bundled browser engine.
- **`serde`, `serde_json`, `uuid` and `tempfile`:** existing workspace
  dependencies also used by artifact metadata, publication IDs and atomic
  staging. Each is MIT OR Apache-2.0.
- **`alacritty_terminal` and `rio-vt`:** development-only comparison engines.
  Apache-2.0 and MIT respectively.

The native terminal source is prepared from a pinned Ghostty commit plus
[`native/ghostty.patch`](../native/ghostty.patch). The preparation script checks
the source and patch digests and reuses immutable prepared directories. See
[the native build instructions](../CONTRIBUTING.md#the-terminal-engines-native-build)
for Zig, build modes, CPU targets and offline builds.

## Reading the code

Start with `core::Layout` for the saved model and `app::library` for storage and
per-window reconciliation. `app::panes` owns terminal workers; `app::view` draws
their snapshots. `app::desktop` provides the product UI. Read `tmux::control` and
`tmux::recovery` for the wire protocol, then `term::Terminal` for the engine
boundary. Tests that start tmux use `test_support::TmuxServer` to isolate their
servers from real sessions.

## Native distribution

`scripts/package.py` builds the product binary in release mode for the host,
with Ghostty explicitly set to `ReleaseFast` and the baseline CPU target. It
packages embedded UI assets, the app icon from `packaging/icon/`, dependency
notices and source/build metadata.
macOS receives an ad-hoc-signed `Agent Interface.app` bundle; Linux receives a glibc
binary linked to the system desktop libraries. Neither package includes a
Python runtime, build toolchain, tmux server or application daemon.

The shell installer verifies the archive checksum and target, stages a
per-user replacement, and preserves the data directories. Linux desktop and
window identifiers use `io.github.eandualem.tmux-workspaces`; macOS uses the same
bundle identifier. The installer replaces its owned legacy `tmux-workspaces.app`
bundle with `Agent Interface.app`, restoring the old name if installation fails.
The Linux launcher uses the packaged Agent Interface icon.

Build-only `cargo-about` 0.9.2 (MIT OR Apache-2.0) generates Rust dependency
notices. Hash-pinned native notices supplement Cargo's graph for Ghostty,
uucode/Unicode, simdutf, Highway, X11 color data and compiler runtimes. The
package retains Lucide's ISC notice and SQLite's public-domain dedication.
The manual package workflow uses `actions/upload-artifact` v7.0.0 (MIT), pinned
to a full commit, to retain build artifacts; it does not publish releases.
