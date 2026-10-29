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
the pane’s attachment or view and returns the same pane to its chooser, including
ordinary terminals. The ordinary shell remains available to reopen. Detach, Close
Pane and Close Tab never send a session-termination command. Closing the final pane closes
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
| `roster` | Agent Backbone roster and pane-view reads, plus explicit user feedback through bounded local requests |
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
display. Embedded attachments retry the saved socket and session on the existing
two-second window-refresh loop; retries never create a session. A stopped
Backbone agent's pane can ask Backbone to start or resume it (one authenticated
local request, never retried), after which the same loop connects. An ordinary
shell that cannot be reached is started once through the same guarded creation
path as Open terminal, which refuses when the owning server cannot be verified.

On connection or recovery, the worker captures the pane's current state and
only the history its local scrollback retains, then continues with subsequent
output. Capture and restore segments share immutable hyperlink strings, avoiding
per-segment copies of long targets. Initial snapshots are released after restoration. The reader queues
output only for panes in its selected window. Recovery uses the state tmux
can report; it cannot reconstruct every historical parser or input mode.

The UI handles application shortcuts first. Other keys and paste operations
are encoded for the destination pane's reported input modes and sent as literal
bytes to that exact pane. Navigation does not redirect queued input to another
pane. Input waits until a newly selected destination is ready. If an older tmux
server cannot report bracketed-paste state, multiline paste is refused until the
shell enables that mode in observed output; single-line paste remains available.
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

`app::diagnostics` owns the desktop process's `tracing` subscriber. Product
libraries emit structured events without choosing a destination or initializing
global logging. The saved `logging` preference selects Warnings (the default),
Debug or Trace and reloads the filter immediately. Neither development builds
nor `RUST_LOG` enables verbose output. Only the explicitly instrumented app,
tmux and roster targets are admitted, so dependency HTTP logs cannot expose
request bodies or credentials. Events cover lifecycle boundaries and operation
timings; terminal byte parsing, painting and input carry no per-character logs.

The JSON formatter feeds a lossy 1,024-record background queue. Queue saturation
does not block the UI or terminal workers; the drop count and write failures are
visible in Settings. The writer rotates four 4 MiB files, rejects records larger
than 16 KiB and uses a private owned directory and regular files. A separate
advisory lock serializes rotation across desktop processes; lock contention is
reported as a failed write, rather than blocking producers. The worker guard
drains queued records during normal shutdown. Forced termination may lose queued
records. Logs contain stable pane IDs, process/session IDs, classifications and
timings, never terminal bytes, input, feedback prose, HTTP payloads or credentials.
Failure logging leaves existing lifecycle decisions and errors unchanged.

## Saving and shared libraries

An absent library is initialized with one empty workspace. SQLite builds the
initial database in memory; its serialized bytes are written through the securely
created staging file handle, so SQLite never reopens a replaceable staging path.
Reads validate ordinary stored columns and the record in one database snapshot,
using dedicated PRAGMA statements that cannot resolve database-defined views.
SQLite values are bounded to the 16 MiB record budget plus 1 KiB for row metadata.
Writes require the canonical table declaration without added constraints, triggers
or indexes; object association checks follow SQLite's case-insensitive identifiers.
Existing databases are opened without automatic repair or replacement. Invalid or unreadable files
remain intact and produce an error in the UI.

A storage worker polls for changes made by other windows or compatible library
writers. Edits are reconciled against the document they started from using the
shared merge rules. Structural changes are saved before replacing the visible
arrangement. Conflicting edits refresh the shared arrangement and ask the user
to repeat the edit; failed saves offer Retry or Discard.

Workspace and tab selection, pane expansion, and attachment-window selection
are local to each native window. Header double-click and the pane shortcut toggle
expansion for the selected tab without changing the saved split tree; Escape
restores that tab. Each tab retains its own expansion during navigation and shared
library refreshes. The schema-v2 root `focus` flag still seeds only the initially
selected tab; other tabs start unexpanded in a new window. Ordinary-shell identities belong to the saved
library. Closing a native window detaches its clients without issuing
session-termination commands; navigation keeps them attached.

## First-run setup

A fresh install (no settings file and no library) opens a setup window instead
of the workspace window: an eight-second opening scene, played once, then steps
that check tmux, Agent Backbone, Git and the agent tools, create a first agent,
and show the main shortcuts. tmux uses the terminal pane's `tmux_binary` resolver. Git, GitHub CLI,
Backbone and agent tools are looked up after the user's interactive login shell
runs its startup files, because an app opened from the Dock has a short PATH.
The probe itself runs in `/bin/sh` so non-POSIX login shells can supply PATH too.
The shell has an eight-second deadline; its output goes to a temporary file
instead of a pipe, with positional reads limited to 256 KiB. The lookup falls
back to PATH and the usual install folders. The
Backbone check is a single read-only roster request. These checks install nothing.

The first-agent step calls the selected engine's `runtimes --json`, then submits
`setup --file REQUEST --jsonl` only after an explicit Start action. It offers the
`orchestrator-v1` brief or custom instructions, a name and a folder. Discovery
reports installed tools, never verified login; permission and login prompts stay
in the actual agent terminal. First-agent selection also requires an explicit
`setup_supported: true`; older providers and unsupported tools remain unavailable
for this setup path. Start also requests an initial greeting, which may
use the selected account. A submitted greeting is not reported as a received reply.

The app saves the exact request, launcher and effective login-shell PATH in a
private `onboarding-agent-<installation-digest>.json` beside settings before
executing it. The resolved provider directory scopes each saved operation.
Recovery loads that record even when fresh tool discovery cannot find an engine. An interrupted
attempt reuses that request ID; only a confirmed preflight failure permits editing
choices. Saving and archiving serialize through a sibling file lock. The engine
owns durable operation receipts and mutation deduplication. Requests are capped at
32 KiB; stdout uses positional reads with a 1 MiB total limit, 64 KiB lines,
1,024 events and a five-minute deadline. Request identity, sequence, final state
and exit status must agree. Missing or contradictory results preserve the saved
request and show an unknown outcome. Discovery has a fifteen-second deadline.
Inherited authority remains intact; the app never substitutes an operator key.

An installer activation record under the private engine root may select an
immutable payload launcher only when its data directory matches the current
connection and its canonical entrypoint remains inside that payload. Otherwise
setup uses the detected Backbone CLI. Existing agents can be used without creating
another agent; fresh installations may choose terminals for now and reopen setup
later. A saved request remains intact when the user chooses terminals after an
attempt finishes, even if that attempt needs recovery. Rechecking the computer
refreshes unsubmitted discovery inputs while
retaining any saved operation unchanged. Opening a created agent adds a saved terminal attachment through
the normal workspace save path.
Progress is saved as `[setup]` in `settings.toml`: an interrupted setup resumes
without the scene, and **Set Up Agent Interface…** in the application menu
reopens it. An explicit library directory skips setup.

## Appearance and integrations

Native appearance and shortcuts are saved separately in `settings.toml` under
`$XDG_CONFIG_HOME/agent-interface`, or `~/.config/agent-interface`. Shared colors,
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
bounded, read-only requests to a local endpoint. Credential-bearing requests
require an absolute Unix socket path from `BACKBONE_SOCKET_PATH` or the provider's
saved `backbone.socket_path` setting. An explicit endpoint overrides the saved
setting, which is re-read on each request. The socket must
be owned by the app's effective user with mode 0600, inside a directory owned
by that user with mode 0700. The connected peer's UID must match before HTTP
or credentials are sent. This authenticates the OS account, not an individual
process within that account. HTTP paths and bearer authentication are unchanged.
There is no TCP fallback when the socket is missing or invalid; loopback TCP
is supported only for anonymous requests. A failed refresh
must not present an old agent state as current. The core does not depend on
Backbone or the Python implementation. The `roster::views` adapter reads and
decodes view data; `app::desktop::views` renders it in native panes. Each
Unix request owns a short-lived runtime and connection so timeouts cancel all
associated work. Anonymous TCP reads reuse a client per polling worker.
Configuration and credentials are re-read for every request. Selection changes coalesce and cancel obsolete
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

Native document readers share selection state across styled text blocks and source
text. GPUI text layouts map pointer positions to UTF-8 offsets, snapped to grapheme
boundaries. Copy uses the displayed text; selection resets when that text changes.
Native Copy and Select All menu actions follow the focused field or reader.
WebKit handles those selectors through the macOS responder chain, without a
clipboard JavaScript bridge.

File pastes and drops share the terminal's bounded input queue. Copied file paths
take precedence over Finder display names and are quoted for terminal input.
The macOS adapter accepts file URLs and raw PNG, TIFF and JPEG drags. Raw images
are checked against those formats before constructing a decoder and staged as
PNG in a private temporary directory. A single temporary image URL
is also received as PNG at drop time, because access granted to the app need not
extend to an already-running terminal process. Staged files are deleted on
cancellation and retained after an accepted drop for the receiving program to
read. This is invoked only by a user drag, with no screenshot monitoring.

Agent views cache unchanged render trees; data, size, focus and appearance
changes invalidate them. Visible working and starting indicators animate using GPUI; other states are
static. State labels retain the distinction between starting and busy, and
between blocked and waiting for a person. The roster reads immediately and then
two seconds after each completed request; Team and agent-card views poll every
three seconds after a read. Requests have a three-second deadline, and failures
mark cached observations stale.
Team-wide libraries (`roster::views::library`) are gathered from per-agent
reads, the store, each agent's skill preview and its brief, for at most 32
agents per refresh, merged by file path so no row names an agent. Skill paths and
tag membership use indexed lookup. Policy text and metadata share immutable
allocations between groups. Expanded skill and policy groups are limited to 4,096
rows and 16 MiB of display work; policy sources are bounded while decoding for
both team and agent views, and each collected brief must fit the team's remaining
budget. Excessive responses leave the last view stale. They poll
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
newest page. An optional agent filter is sent to both feeds and retained across
view changes and pagination; choosing another agent restarts at the newest page.
The picker reads configured agents, with page authors as a fallback when the
roster is unavailable. Each row opens the complete authored record, including
its sections and links. An open record keeps its contents through background refreshes.
An open Response is polled by ID so its correction metadata stays current even
after newer publications move it off its former list page.
Authored status and blocker text do not change runtime state or grant approval.

Every reader places one native conversation (`views/conversation.rs`) beside
the authored content: a rail while closed, a thread and composer while open.
No authored HTML can reach it. It retains its thread and draft by provider,
author and record identity while its saved pane exists, including hidden tabs.
Hidden readers pause polling and release their web surface; delivery workers
can still finish and retain their result. On explicit send,
`roster::feedback` re-reads the exact record, checks the original stable author ID
and current registered name, then uses the existing authenticated `/api/messages`
POST as `agent-interface-app`. The message carries
the record kind and ID plus the user's text, not the original content. This is
ordinary feedback, without signed owner-confirmation provenance. The existing
name-based API cannot make source lookup and recipient routing atomic. Each
message is sent once; a refused message can be retried, and an unknown delivery
is marked without automatic resubmission. That is the path for a provider
without conversations.

Where Backbone keeps per-artifact conversations, `roster::conversations` reads
and writes them instead (`/api/conversations/{kind}/{id}`). A record names its
conversation: a document by its stable document ID, a report or answer by the
root of its correction chain. Requests go only to the provider the record was
read from; after a connection change they are refused until it is reopened.
Only the conversation shown in a visible pane polls, every 5 seconds while
open and every 25 while closed, and re-reads the newest page; entries that page
no longer holds stay with the older ones, which load on request, and older
owner messages still pending or held are re-read by ID with each poll. Until
Backbone first answers, a message waits rather than choosing a path, and a
retry always takes the path the message first took. Each message carries a
client request ID, so a send whose outcome is unknown is retried with the same
ID and stored at most once; a stored message whose delivery failed is redelivered without a new
entry. Opening the panel moves the owner's read marker to the newest entry and
fixes the "new" divider where the marker was. A plain 404 or an unknown
artifact switches the conversation back to one-way feedback. A shared skill or policy has
no record, so its conversation finds its identity on the first read: the
current provider, then the `artifact_id` that `/api/skills` or `/api/policies`
lists for its source path. Without a maintainer, a message names the agent the
person chose.

Durable Documents use Backbone's metadata-only `/api/documents` list and exact
`/api/documents/{revision_id}` JSON reads. Backbone owns immutable revisions,
stable document and author identities, publication authorization and retention;
Interface owns native selection, filtering and rendering. The list is latest
per document, not per author. Agent-name and stable-author filters persist across
paging. The reader keeps the selected revision until an explicit navigation;
checking for a correction does not replace its contents. Document lists and
bodies refresh on explicit requests or reveal, rather than the report poll.
An open HTML reader remains uncached so native child creation is replayed when
overlays close; native report lists retain their usual rendering cache.

Only exact-document reads allow a response of `6 × 4 MiB + 8192` bytes, covering
JSON escaping of the bounded UTF-8 HTML. The native adapter validates the decoded
body size, metadata envelope, revision ID, byte count and SHA-256 before passing
HTML to the same isolated system-webview surface. Credentials remain in the
native transport. Missing API support is distinct from an empty catalog. No
local artifact is automatically imported under an authenticated author name.

The experimental Artifacts pane is independent of Backbone. `app::artifacts`
copies a UTF-8 HTML document and small metadata record into an immutable UUID
directory under the selected library's `artifacts/`, publishing by atomic
rename. The CLI does this without starting a window, tmux client or daemon.
Discovery reads metadata off the UI thread every two seconds while visible;
the catalog retains the newest 200 entries. HTML is read only on opening a
document and is limited to 4 MiB, including inline assets.

`desktop::views::artifact_web` is the system-webview boundary. On macOS it
creates a Wry/WebKit child view only for an opened document, positions it in
the pane's content rectangle, and updates its host theme stylesheet. Text,
backgrounds and controls follow the active app theme; layout and media retain
their authored presentation. Stored HTML is not rewritten. The view is
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
denied. WebRTC peer connections are disabled in native WebKit preferences before
loading content, because CSP does not govern their sockets. This uses WebKit's
private preference SPI, checked at runtime; if unavailable or ineffective, HTML
rendering fails closed. The supplied configuration explicitly keeps an ephemeral
data store. The sole JavaScript bridge reports pointer focus to select the owning
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
  The app wraps GPUI's platform to provide the interface size setting: the
  wrapper multiplies each window's display scale by the chosen size and
  converts logical coordinates at the boundary, so views stay unaware of it.
  Text input handlers are registered through the same module, so input-method
  candidate windows follow the caret. A window keeps the minimum size it opened with until reopened.
  The macOS artifact web view sits outside GPUI and is positioned and zoomed
  separately.
- **`raw-window-handle` 0.6.2:** the window handle traits that wrapper
  forwards. MIT OR Apache-2.0 OR Zlib, pinned exactly; GPUI already builds
  this version, so it adds no package.
  The dependency graph remains unchanged.
- **Plasma Wayland protocols:** GPUI's Linux backend uses the MIT-licensed
  `wayland-protocols-plasma` wrapper. Its vendored protocol definitions also
  carry LGPL-2.1-or-later, MIT-CMU and BSD terms, preserved in the
  [Linux protocol notices](../packaging/wayland-notices.txt). Cargo licence
  metadata alone does not describe those generated-binding inputs.
- **`libghostty-vt`:** terminal emulation behind the `term` boundary, pinned with
  its native binding. MIT OR Apache-2.0; Ghostty's core is MIT-licensed.
- **`rusqlite`:** transactional access to the existing SQLite library format.
  MIT. Its bundled SQLite uses SQLite's public-domain dedication. Runtime limits
  bound values; serialization writes the initial database through an open file handle.
- **`libc`:** Unix constants for nonblocking configuration reads in the app
  and roster, and the existing process integration. MIT OR Apache-2.0.
- **`wry` 0.57.1:** macOS system WebKit child views for experimental HTML
  artifacts, pinned exactly and enabled only on macOS. MIT OR Apache-2.0.
  It does not render terminals or supply a bundled browser engine.
- **Hyper 1.12.0, hyper-util 0.1.21, http-body-util 0.1.5 and Tokio 1.53.3:**
  MIT HTTP/runtime dependencies, pinned exactly, for HTTP over a connected
  Unix socket after the peer identity check.
- **`rustix` 1.1.5:** effective-UID access for that check. MIT OR Apache-2.0
  OR Apache-2.0 WITH LLVM-exception.
- **`objc2-web-kit` 0.3.2:** MIT-licensed bindings, pinned exactly, used by the
  macOS bridge to configure WebKit peer connections and its
  ephemeral data store before document execution. The app calls a safe bridge
  function through the existing GPUI macOS package, pinned to 0.3.9.
- **`sha2`:** existing MIT OR Apache-2.0 workspace dependency, also used by
  the document adapter to verify the server-provided HTML digest.
- **`serde`, `serde_json`, `uuid` and `tempfile`:** existing workspace
  dependencies also used by artifact metadata, publication IDs and atomic
  staging. The macOS adapter also uses `tempfile` 3.27.0 (exact pin) for private
  screenshot-drop staging. Each is MIT OR Apache-2.0.
- **`alacritty_terminal` and `rio-vt`:** development-only comparison engines.
  Apache-2.0 and MIT respectively.
- **`tracing` 0.1.44:** MIT-licensed structured diagnostic events in app, tmux
  and roster; pinned exactly. **`tracing-subscriber` 0.3.23** and
  **`tracing-appender` 0.2.5** (MIT, exact pins) provide the app-owned JSON
  formatter, reloadable filter and nonblocking background writer. The small
  app-owned file writer adds size-based rotation and explicit write-health
  reporting; it does not implement another logging facade.
- **`chrono` 0.4.45:** MIT OR Apache-2.0, pinned exactly with only its `clock`
  feature. It names local days and times for records and messages without
  `unsafe` code in the app. GPUI already builds the same version and feature,
  so it adds no package to the graph.

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

The archive includes `setup_backbone.py`, a standard-library installer helper.
`install.sh` installs the latest stable published `agent-backbone>=0.4.0` with
uv, independently of the app archive. Backbone (MIT) owns its package, dependency
testing and publication. uv (MIT OR Apache-2.0) installs the Python environment;
when uv is absent, its 0.13.6 installer is version- and SHA-256-pinned. No Backbone
source checkout or portable payload is composed into Interface's packages.

The helper uses Backbone's versioned setup command and retains its exact request
and progress in a private per-user state directory. Pending setup is resumed
before any package upgrade, with the provider version, distribution metadata and
launcher checked against the saved installation. For a fresh installation only,
it enables the private socket and restarts the service after checking the saved
installation identity, service definition and loaded service owner. Interrupted
socket writes are reconciled against the intended value; conflicting choices
are preserved. Existing installations and completed setup keep their settings
and services. The final socket probe sends only an unauthenticated health request;
the app's native transport performs peer verification before sending credentials.
Provider installation/setup finishes before replacing the app; provider state
is retained if a later app replacement fails.

The shell installer verifies the archive checksum and target, stages a
per-user replacement, and preserves the data directories. Linux desktop and
window identifiers use `io.github.eandualem.agent-interface`; macOS uses the same
bundle identifier. The executable, workspace crates, package archives and new
install, configuration and data directories use `agent-interface`. The installer
accepts its previous ownership marker when updating, removes owned legacy Linux launchers,
and rolls back the app and launchers on failure. Existing library directories stay
in place: their absolute path determines the ordinary-shell socket hash. Existing
private legacy shell sockets remain usable. Reconnection, explicit shell opening
and saved working-directory refresh select the server containing the exact
ordinary session across the current and legacy sockets. A verified current owner
is used without inspecting unused legacy metadata. When no owner is verified,
a metadata or discovery failure does not authorize creating a replacement shell
on another server. Settings,
themes and keymaps fall back to the old configuration directory when the new file is absent; settings writes
use the new directory. Legacy environment overrides remain supported, with the new
`AGENT_INTERFACE_*` names taking precedence. No database or running server is moved.
The Linux launcher uses the packaged Agent Interface icon.

Linux windows request client decorations and draw a matching right-side caption
with minimize, maximize/restore and close controls, plus resize edges. If the X11
window manager requires server decorations, the native frame supplies its controls
and the app omits its caption. macOS retains the native traffic lights. Linux uses
DejaVu Sans Mono (the documented runtime font) for terminal cells, avoiding a
proportional fallback for the macOS-only Menlo family. Graphical Ubuntu CI checks
ASCII font metrics and exercises the actual window controls on a private X11
desktop, retaining screenshots; this does not establish WSLg hardware compatibility.

Build-only `cargo-about` 0.9.2 (MIT OR Apache-2.0) generates Rust dependency
notices. Hash-pinned native notices supplement Cargo's graph for Ghostty,
uucode/Unicode, simdutf, Highway, X11 color data and compiler runtimes. The
package retains Lucide's ISC notice and SQLite's public-domain dedication.
The manual package workflow uses `actions/upload-artifact` v7.0.0 (MIT), pinned
to a full commit, to retain build artifacts; it does not publish releases.


### Retained Sheets

`agent-interface-roster::sheets` owns the typed workbook protocol and bounded local
transport through the existing authenticated private Unix socket. Stable workbook,
revision, tab, row and column identifiers are validated before constructing URLs.
Reads pin one immutable revision and never combine pages from different revisions.
Range and metadata response-size refusals reduce page size without changing the
revision or discovery/history cursor. Other failures do not trigger automatic retries.
Agent Backbone owns canonical data, calculated results, engine versions, storage
admission and atomic updates. The app does not calculate formulas.

`desktop::views::sheets` supplies a read-only native GPUI grid beside Documents and
Responses in Reports & updates. Agents manage workbook creation, cell and structural
edits, saved views and imports through Backbone. The native UI exposes inspection,
selection, history, copying values, export and contextual feedback. Virtualized rows
and bounded column pages limit rendering work. Labels retain canonical A1 coordinates
under saved sort/filter views. The shelf reads up to 20 metadata pages, rejects
cursor cycles, and orders unread revisions first, newest updates first within each
group. It has no separate page controls. Grid navigation appears only when more
rows or columns exist. Copy uses selected cells or the displayed range; the label
states its scope. Export uses an occluding deferred menu above the grid.
No spreadsheet widget or formula engine is added.

`roster::plans` reads Backbone's `/api/current-plans` list over the same
authenticated local transport as Sheets, on a worker, every 20 seconds while
the Current plans section is open. It is read-only: it publishes, approves and
changes nothing. A missing endpoint is reported as unsupported, and a failed
reread keeps the plans last read. Each read also takes the agents' runtimes for
their badges. The section draws through the pane, like the record readers, so
an opened plan's text can be selected and copied. Its selected step uses the shared
conversation panel, addressed by the plan author's stable identity. Drafts are
kept by provider, author, observed revision and step; changing steps or refreshing
a plan cannot retarget them. Messages and retries retain that revision and item
anchor. Plan refresh pauses while a draft or unresolved delivery exists, keeping
its step reachable until the person finishes it. The panel requires stored conversation support and never falls back to
unanchored feedback for plans.

Sheets shows the same `desktop::views::conversation::Conversation` entity as the
record readers, beside the grid, with its native text field, draft preservation,
focus restoration, delivery states and thread. A record's conversation sends
through `roster::feedback` and restores focus to its web surface on close. Sheets
creates the conversation in owner-delivered mode: it supplies the workbook's
author and the selected cells as the anchor, receives each send and retry as an
event, handles the provider's feedback transaction and reports the outcome back.
Feedback carries the immutable observed revision and stable
selected cell identities. Navigation cannot move a draft to another revision.

A prepared feedback request retains exact bytes and its retry key after an uncertain
response. The composer freezes while that request is unresolved and offers a retry of
the same operation. Accepted feedback retains its receipt and distinguishes delivery
states without claiming an agent answer. No agent workflow or service is started by
the view. The typed adapter also represents provider mutation APIs, but the native
Sheets view calls only reads, export and feedback submission.

JSON/XLSX exports preserve supported workbook features; CSV/TSV export one tab as
values. Export warnings are bounded and displayed; malformed diagnostics fail
visibly. The first integration does not claim full Excel formula compatibility,
Google account integration, macros, network formulas, charts, pivots or live keystroke
coediting. The provider isolates calculation in a bounded worker; macOS resident
memory monitoring can overshoot between samples and is not a hard allocation sandbox.
