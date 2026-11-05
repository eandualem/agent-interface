# Architecture

Agent Interface is a native desktop client for tmux. The app owns the window
and saved arrangement; tmux owns sessions and their processes. Terminals are
drawn with GPUI, using Ghostty's terminal core to interpret output. Terminals never use a web view. HTML Documents use an isolated system
webview; the app runs no daemon.

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
| `core` | Workspace, tab and split models; validation; local navigation; three-way merge rules; compatibility keymap data |
| `store` | Initializing, reading and transactionally saving schema-v2 `layouts.db` files |
| `tmux` | Server discovery and validation, control-mode protocol, pane capture, recovery and input commands |
| `term` | Terminal engine ownership, input encoding and owned display snapshots |
| `roster` | Agent Backbone roster and pane-view reads, plus explicit user feedback through bounded local requests |
| `native-fs` | Safe descriptor-based native filesystem checks; narrowly scoped Darwin ACL FFI, no paths or application state |
| `webview-host` | Borrowed XCB-to-Xlib window-handle adaptation for Linux document embedding; no application state or process ownership |
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

tmux writes a capture's text into the control connection unchanged, so a
captured line could match the line that ends tmux's reply and make the reader
take the rest of the capture for messages from tmux. The app requires tmux 3.4
or newer and writes each capture into a named buffer
instead of its reply, with a name unique to that line and process. After the
line's replies, a separate tmux client on the same socket reads each buffer
by its listed size using `show-buffer` and deletes it. This unattached client
receives exact bytes without invoking the user's `after-save-buffer` hook.
A rebuild that does not finish tries to
delete the buffers its line named, on a separate thread; this is best effort,
and a process exit or a failing tmux can leave them. No other buffer is read
or deleted. Version checks cover both the executable and the connected server
before capture or input is enabled. tmux 3.4 is the minimum because it excludes
named buffers from an ordinary paste; older servers are rejected rather than
falling back to captures in the reply stream.

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
global logging. The saved `logging` preference selects Off, Warnings (the default),
Debug or Trace and reloads the filter immediately. Off disables new events; queued
records may drain, and existing files are not erased. The writer defers filesystem
initialization on an Off launch until logging is enabled and a record arrives. Neither development builds
nor `RUST_LOG` enables verbose output. Only the explicitly instrumented app,
tmux and roster targets are admitted; dependency HTTP logging is not enabled. Events cover lifecycle boundaries and operation
timings; terminal byte parsing, painting and input carry no per-character logs.

The JSON formatter feeds a lossy 1,024-record background queue. Queue saturation
does not block the UI or terminal workers; the drop count and write failures are
visible in Settings. The writer rotates four 4 MiB files, rejects records larger
than 16 KiB and uses a private owned directory and regular files. A separate
advisory lock serializes rotation across desktop processes; lock contention is
reported as a failed write, rather than blocking producers. The worker guard
drains queued records during normal shutdown. Forced termination may lose queued
records. Instrumentation records pane/process/session IDs, classifications and
timings rather than terminal bytes, input or HTTP payloads. Error context and
identifiers may still be sensitive; this is not a universal secret-scrubbing
boundary. Users explicitly select detailed logging and manage its local retention.
Off does not disable functional storage (layouts, settings and resumable setup
state), tmux history, or user-visible operation errors. Failure logging leaves
existing lifecycle decisions and errors unchanged.

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

Until setup is explicitly completed, an ordinary launch opens a setup window
instead of the workspace window. Existing libraries and preferences, including
pre-rename data, are preserved and do not count as completed setup. An explicit
`--data-dir` launch opens that library directly. Setup has an eight-second opening scene, played once, then steps
that check tmux, Agent Backbone, Git and the agent tools, create a first agent,
and show the main shortcuts. tmux uses the terminal pane's `tmux_binary` resolver. Git, GitHub CLI,
Backbone and agent tools are looked up after the user's interactive login shell
runs its startup files, because an app opened from the Dock has a short PATH.
The probe itself runs in `/bin/sh` so non-POSIX login shells can supply PATH too.
Standard installation directories, including OpenCode's `~/.opencode/bin`, are
appended without overriding the shell's PATH order. Refresh checks those paths
again and passes the refreshed PATH to both runtime discovery and first-agent
setup. Submitted requests retain their original environment for replay.
The shell has an eight-second deadline; its output goes to a temporary file
instead of a pipe, with positional reads limited to 256 KiB. The lookup falls
back to PATH and the usual install folders. The
Backbone check is a single read-only roster request. These checks install nothing.
The welcome window offers the same persistent interface-size setting as the
workspace, including during the opening scene, so display scaling can be adjusted
before setup is complete. On Wayland, increasing this size asks the compositor
to maximize setup, or uses the display extent alone if maximizing is unsupported;
global window positions are unavailable, so app-calculated position limits cannot
safely place it. macOS and X11 instead bound resizing
against the display, respecting the platform resize anchor.

The first-agent step calls the selected engine's `runtimes --json --check-setup`
(falling back to `runtimes --json` when an older CLI rejects the option), then submits
`setup --file REQUEST --jsonl` only after an explicit Start action. It offers the
`orchestrator-v1` brief or custom instructions, a name and a folder. Discovery
reports installed tools and opt-in setup compatibility, never verified login;
discovery never chooses a tool automatically. Refresh preserves an explicit choice
when the tool remains in the catalog. Selecting any listed agent tool shows only
its own diagnostic and executable; an incompatible choice cannot start setup.
The same selected row controls the diagnostic, Start label and submitted runtime
and hooks, so another installed tool cannot block it. Permission and login prompts stay
in the actual agent terminal. First-agent selection also requires an explicit
`setup_supported: true`; older providers and unsupported tools remain unavailable
for this setup path. Start also requests an initial greeting, which may
use the selected account. A submitted greeting is not reported as a received reply.

The app saves the exact request, launcher and effective login-shell PATH in a
private `onboarding-agent-<installation-digest>.json` beside settings before
executing it. The resolved provider directory scopes each saved operation.
Recovery loads that record even when fresh tool discovery cannot find an engine. An interrupted
attempt reuses that request ID. Change agent tool first durably records cancellation
intent, then submits the same fields and ID with `action=cancel_first_agent`.
Cancellation uses the active or discovered engine and refreshed PATH when
available, falling back to the saved executable and PATH. Normal setup replay
always uses the saved launcher and PATH.
Only a matching `request_cancelled` completion with `can_start_new_request=true`
retires the local request and unlocks the form. The name, folder and brief survive;
tool discovery and the login-shell PATH refresh before a new request is created.
After interruption, cancellation intent resumes cancellation rather than starting
the previous tool. The provider refuses cancellation after setup effects begin;
the app retains the original record on refusal or an uncertain result. Saving,
execution, cancellation and archiving serialize through a sibling file lock, and
each worker checks that its request is still current before invoking the engine. The engine
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

The legacy keymap inspection commands do not replace native Settings
shortcuts.

The active Agent Backbone adapter resolves configuration lazily and uses
bounded, read-only requests to a local endpoint. Credential-bearing requests
require the Unix socket at `run/api.sock` in the provider's data directory, or an
explicit endpoint. A bound profile takes its endpoint from `profiles show` instead.
The socket must
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

The separate `roster::profiles` adapter implements the initial profile connection
contract. It validates registry list/show records, rejects non-ready bindings,
reads only the exact private dotenv credential reference, and verifies
`/api/profile` against the expected profile, installation and lifecycle IDs and
tmux socket. Its connection retains that credential and identity; every request
carries all three identity headers, including the handshake and subsequent
requests. Replacement or an unsupported provider fails explicitly without legacy
fallback. `Connection::roster` reads the agents endpoint on that same binding.
The explicit `app::roster::BoundRosterSource` owns one connection for its entire
worker lifetime, with a private coalescing cache. Every update carries its
verified identity and typed failure; a failed read retains only that worker's
previous metadata and observation time, marked stale. A running worker cannot
be rebound. Dropping it interrupts the polling wait and joins its bounded
in-flight request, so UI owners must dispose of it off the UI thread.
Desktop status observations are owned by explicit `LiveAgents` handles passed
from each window to its views and child readers. A weak source registry shares
one live poller per full profile/installation/lifecycle identity; explicit legacy
sources are keyed separately by provider directory. Closing the last owner
releases its cache and disposes of the poller off the UI thread. Deferred wakes
refer weakly to their original source, and bound updates must match that source's
identity. Failed reads hide its active rows and show unavailable status without
changing another source's attention state. No global selected source supplies
status to readers. Authored reports and historical events never update this model.
`roster::views::content` reads only retained Reports, Responses and Documents
through a captured connection, including pagination and roster enrichment.
Document fetches keep their larger wire limit and digest validation. Transport
failures remain typed and invalidate the whole read even during optional
metadata enrichment. Each observation retains its full identity and request;
legacy provider URL fields stay empty and do not authorize actions. Bound reads
use the credential and socket captured by their connection; they do not re-read
configuration or credentials per request.
`app::roster::BoundContentSource` owns that connection for its lifetime. Selection
and visibility changes invalidate pending results and in-flight generations;
Documents wait for explicit refresh. Its wake callback belongs to the original
owner. Dropping the source waits for its bounded in-flight request, so UI owners
must drop it off the UI thread.
They do not start services, migrate Default or configure runtime accounts.

`roster::library_snapshots` reads shared Skill and Policy catalogs using one
verified connection. Each selectable artifact retains its catalog identity and
credential snapshot; opening it selects a version and fetches that exact revision,
checking the revision ID and SHA-256 digest before exposing text. Source paths are
display metadata and never authorize local file reads. Cancellation and binding
refusals stop the chain. Repository and runtime-local skills are excluded, and
native library navigation remains separate from this reader.

The JSON fixture in `crates/roster/tests/fixtures/profiles-v1.json` comes from
Agent Backbone's [profile contract](https://github.com/eandualem/agent-backbone/blob/4848c6dc3b247781ea4ab4beb19f4a5b7922c2b4/docs/contracts/profiles-v1.json).
Contract revision 2 uses wire schema 1. Disposable Unix-socket checks cover the
handshake, pinned credentials, follow-up request headers, independent roster
workers with delayed responses, stale-cache ownership and replacement refusal;
these checks do not establish complete runtime or application profile isolation.

Native document readers share selection state across styled text blocks and source
text. GPUI text layouts map pointer positions to UTF-8 offsets, snapped to grapheme
boundaries. Copy uses the displayed text; selection resets when that text changes.
Native Copy and Select All menu actions follow the focused field or reader.
Native Markdown copying preserves authored link labels and restores abbreviated
bare addresses to their full URLs. Native links open only HTTP and HTTPS destinations; other schemes and
document anchors remain plain text.
Inline highlights use a sorted boundary sweep with active counts, preserving
nested formatting without rescanning every mark for each span.
Markdown nesting beyond 32 containers falls back to the complete source text,
bounding parsing, element-tree rendering and destruction. URL recognition scans
forward and closes each link by replacing only entries within its own label.
WebKit handles those selectors through the macOS responder chain, without a
clipboard JavaScript bridge.

For new agent launches, the app captures its foreground and background and sends
`terminal_defaults` only when the saved runtime advertises support through
Backbone's runtime catalog. First-agent setup saves these values with the request;
retries keep the original colours, and legacy requests still omit the field.
Backbone seeds the new pane before executing the CLI. This closes the interval
before Interface attaches, including on tmux versions without control-client
colour reporting. Attached clients continue reporting their current defaults.
A CLI that caches colours at startup may need a user-requested restart to adapt
to a later appearance change; Interface does not restart it or reinterpret its
explicit RGB output.

File pastes and drops share the terminal's bounded input queue. Copied file paths
take precedence over Finder display names and are quoted for terminal input.
The macOS adapter accepts file URLs and raw PNG, TIFF and JPEG drags. Raw images
are checked against those formats before constructing a decoder and staged as
PNG in a private temporary directory. Image file URLs from any folder are also
received as PNG at drop time, because access granted to the app need not extend
to an already-running terminal process. Mixed drops preserve file order and
ordinary paths. Drops are bounded to 64 files and 64 MiB of encoded images; each
decoder has dimension and allocation limits and applies EXIF orientation. Staged files are deleted on
cancellation and retained after an accepted drop for the receiving program to
read. This is invoked only by a user drag, with no screenshot monitoring.

Agent views cache unchanged render trees; data, size, focus and appearance
changes invalidate them. Visible working and starting indicators animate using GPUI; other states are
static. State labels retain the distinction between starting and busy, and
between blocked and waiting for a person. Each connection-owned roster worker reads
immediately and then two seconds after each completed request. Its live model
constructs each agent's status, label and ordering once per observation; windows,
navigation badges, choosers and information views retain that connection's model.
Shared UI components render its symbol, colour and label. Roster changes
invalidate retained view painting even when an authored record or plan is held
open. Offline stays distinct from idle; an unavailable roster suppresses working
indicators rather than presenting cached observations as current. Requests have
a three-second deadline. Team and agent-card metadata still poll every three
seconds; these reads do not supply live status. Activity's historical spans
remain separate from its current-state labels.
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
Report tags explicitly identify the authored report status; the adjacent agent
symbol comes from the shared live model. Authored status and blocker text do not
change runtime state or grant approval.

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
entry. Opening the panel reveals the newest messages; later arrivals follow only
while the reader stays at the bottom. A painted frame at the bottom advances
the owner's read marker through that frame's fetched entries. The "new" divider
stays at the marker from when the panel opened. Read acknowledgements retry at
most once per polling interval, including after failure. A plain 404 or an unknown
artifact switches the conversation back to one-way feedback. A shared skill or policy has
no record, so its conversation finds its identity on the first read: the
current provider, then the `artifact_id` that `/api/skills` or `/api/policies`
lists for its source path. Without a maintainer, a message names the agent the
person chose.

Profile-bound action adapters are separate from these legacy desktop callers.
`roster::conversations::Client` owns a verified connection and one artifact's
kind and ID; `roster::feedback::Client` owns a connection and the original
record. Both compare the record's full profile, installation and lifecycle
identity before construction. Clones retain the original credential snapshot;
reads, pagination, read markers, sends and redelivery never resolve the current
selection. Callers retain the message's request ID, recipient and anchor when
retrying an uncertain conversation send. Feedback verifies the author and
submits on that same binding, with no automatic retry after an unknown result.
Binding refusals remain explicit. Native retained-content readers can use these adapters while keeping their
connection for the pane's lifetime. Deferred operations capture that pane's
client; they never look up the selected profile. The record's conversation
entity keeps its draft, pending sends and retry identities across hiding and
navigation. Bound read workers check origin and selection generation before
applying an update. Document revision navigation retains the same pane origin.

Bound seen-state and discussion keys include profile, installation and lifecycle.
Legacy URL keys remain separate, and the legacy installation timestamp does not
mark another profile's older records as seen. A bound reader has no legacy data
directory, so general views, Plans, Interactions and Sheets cannot fall back to
ambient configuration. The explicit constructor currently supports Reports,
Responses, Documents, Usage, Activity and Skills and requires matching live-status
ownership. Usage range changes and Activity refreshes share the bound content
worker, including generation checks, hidden-pane cancellation and original
credential retention. General Agent, Policy and terminal navigation remain
unavailable through this constructor.

Native bound Skills retain the rendered catalog row, stable artifact ID and
verified connection when opened. Preview reads select an immutable provider
revision and verify its digest; provider paths never become local file reads.
Closing or reopening uses a new completion ticket, so an obsolete response cannot
populate the next opening. The reader keeps its original metadata through catalog
refreshes. Discussions are keyed by full origin, artifact and displayed revision,
and retain that revision in outgoing message anchors and retries. Missing IDs and
repository/runtime-local previews show unavailable; they do not fall back to local
SKILL.md files. Profile selection and the Policy library remain separate work.

The general read-only adapter in `roster::views::bound` captures the same
verified connection for Team, Usage, Activity, Skills, Policies and Tags. Its
closed request set excludes Agent inspection and local runtime or skill-file
reads. Optional history or per-agent enrichment cannot hide a connection
refusal; cancellation discards the complete observation. Native navigation and
actions are wired separately, so this adapter does not enable ambient fallback
or permission inspection.

The app's `profiles` discovery client invokes only the provider's local
`profiles list/show` commands using an explicit executable and captured operator
environment. It preserves managed-agent identity so the provider's operator
boundary still applies. A nonblocking pipe bounds retained output and supplies backpressure; duration
and cancellation are bounded. Each command owns a process group, cleaned before
reaping its leader so wrappers cannot leave background writers behind.
Nonzero exits stay errors, and `show` must return the requested profile ID.
Discovery does not connect, start or repair a provider. Returned records require the normal authenticated
handshake before content use.

`roster::profiles::operations` prepares immutable create/start/stop bodies from
the existing lifecycle contract and validates receipts against the original
request ID, action and full tuple. Create pins the first reserved identity;
terminal observations cannot regress or change. Provider error prose is dropped.
These models execute no command and grant no content authority: a successful
start still needs the authenticated identity handshake. Bound first-agent setup
remains a separate contract.

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

`desktop::views::artifact_web` is the system-webview boundary. On macOS and Linux it
creates a Wry/WebKit child view only for an opened document, positions it in
the pane's content rectangle, and updates its host theme stylesheet. Text,
backgrounds and controls follow the active app theme; layout and media retain
their authored presentation. Stored HTML is not rewritten. The view is
hidden while native overlays or pane dragging need that space. Closing the
document or hiding its pane releases the web view, so reopening starts a fresh
document runtime. Native terminals, menus and report views do not use WebKit.
WebKit's platform processes belong to the renderer; the app adds no daemon.
Linux uses WebKitGTK on X11, with XWayland on Wayland desktops and WSLg;
the integration and event loop are described below.

Artifacts use a private in-memory URL, an ephemeral browser session and a
content policy that permits inline HTML/CSS/JavaScript, SVG, canvas and data
assets but blocks external resources, network requests, frames and form
submission. Navigation, downloads, new windows and permission requests are
denied. WebRTC peer connections are disabled in native WebKit preferences before
loading content, because CSP does not govern their sockets. On macOS this uses WebKit's
private preference SPI; Linux uses its public settings API. Both are checked at runtime; if unavailable or ineffective, HTML
rendering fails closed. The supplied configuration explicitly keeps an ephemeral
data store. The sole JavaScript bridge reports pointer focus to select the owning
pane; it exposes no terminal commands, filesystem access or Backbone API.
This isolates the integration, but does not make arbitrary animations free:
visible documents still consume WebKit CPU and memory according to their code.

## Dependencies and licences

The project is MIT-licensed. [`Cargo.toml`](../Cargo.toml) declares dependency
versions, `Cargo.lock` records the resolved graph, and `deny.toml` checks licences,
advisories and permitted sources.
The audit runs separately for both architectures on macOS and Linux. Wry uses
system WebKit on macOS and GTK/WebKitGTK on Linux; unused Android dependencies
are not part of a supported build. The GTK compatibility patch and narrow
build-time advisory exception are described below.

- **`async-executor` 1.14.1:** MIT OR Apache-2.0, locked for the Linux
  accessibility and asynchronous service stack. This replaces yanked 1.14.0
  and fixes its executor-teardown use-after-free regression.
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
  and roster, the existing process integration, and resident-memory readings in
  the terminal tests. MIT OR Apache-2.0.
- **`wry` 0.57.1:** system WebKit child views for HTML Documents and legacy
  artifacts on macOS and Linux, pinned exactly. MIT OR Apache-2.0.
  The local `native/wry` patch enables the Linux process sandbox before view
  construction. It does not render terminals or supply a bundled browser engine.
- **`gtk` 0.18.2, `gdkx11` 0.18.2 and `webkit2gtk` 2.0.2:** MIT-licensed Rust bindings, pinned
  exactly, for the Linux system WebKitGTK 4.1 runtime, native security settings
  and GTK event dispatch. Their MIT GLib 0.18.5 dependency uses the local
  `native/glib` backport of upstream gtk-rs-core#1343, fixing the mutable
  output-pointer bug in `VariantStrIter` (RUSTSEC-2024-0429); an optimized Linux
  regression test exercises both iteration directions. GTK and WebKitGTK remain dynamically linked system
  libraries, installed and updated through the distribution's package manager.
- **`target-lexicon` 0.12.16:** Apache-2.0 WITH LLVM-exception; build-time
  target detection through GTK's `system-deps`. GTK's build-time
  `proc-macro-error` 1.0.4 is unmaintained (RUSTSEC-2024-0370), with no vulnerability
  in that advisory; the narrowly documented audit exception remains until the
  GTK3 binding graph can be replaced.
- **`gpui-pre-linux` 0.3.9:** existing Apache-2.0 platform implementation,
  now selected explicitly for the X11 host needed by embedded WebKitGTK.
- **Hyper 1.12.0, hyper-util 0.1.21, http-body-util 0.1.5 and Tokio 1.53.3:**
  MIT HTTP/runtime dependencies, pinned exactly, for HTTP over a connected
  Unix socket after the peer identity check.
- **`rustix` 1.1.5:** effective-UID access for that check. MIT OR Apache-2.0
  OR Apache-2.0 WITH LLVM-exception. The app also uses this pinned workspace
  dependency for safe nonblocking pipes and owned discovery-process cleanup.
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
- **`pulldown-cmark` 0.13.6:** MIT-licensed CommonMark parser, pinned exactly
  with default features off, behind the one Markdown reader that skills,
  policies, reports, answers and conversation messages use (`views/markdown.rs`).
  Authored messages treat a leading `---` as a rule rather than front matter.
  It adds tables,
  strikethrough and task lists; the app draws the parsed blocks natively.
  Its transitive `unicase` 2.10.0 dependency (MIT OR Apache-2.0) supplies
  case-insensitive matching for Markdown reference labels.
- **`alacritty_terminal` and `rio-vt` 0.5.29:** development-only comparison engines.
  Apache-2.0 and MIT respectively. The exact Rio pin includes its Kitty graphics
  file-path validation fix; the desktop terminal uses Ghostty. Its benchmark-only
  support crates are `rio-graphics` and `rio-grapheme-width` (MIT), plus
  `rio-unicode` (MIT OR Apache-2.0), all locked at 0.5.29 for graphics data,
  grapheme widths and Unicode classification.
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
[`native/ghostty.patch`](../native/ghostty.patch). Among its changes, the patch
discards an operating system command (OSC) longer than 1 MiB, tmux's default
input limit, so a sequence that never ends holds bounded memory. The preparation
script checks the source and patch digests and reuses immutable prepared directories. See
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
`install.sh` installs the latest stable published `agent-backbone>=0.4.5` with
uv, independently of the app archive. Backbone (MIT) owns its package, dependency
testing and publication. uv (MIT OR Apache-2.0) installs the Python environment;
when uv is absent, its 0.13.6 installer is version- and SHA-256-pinned. No Backbone
source checkout or portable payload is composed into Interface's packages.
After setup or recovery, the installer verifies this package minimum before
replacing the app. Recovery keeps its original provider; if that provider is
older, app installation stops until a subsequent permitted upgrade.
The helpers compare stable Python package versions, including post-releases
and permitted spelling variations in metadata, without assuming a version-parser
dependency in Backbone's environment. Pre-release and development versions do
not satisfy either gate.
The installer's check does not require a new helper command in older archives.

Linux archives also include `upgrade_backbone.py`. Before changing an existing
Linux provider package, this guard requires a completed installer-owned setup,
unchanged installation and service identities, and a healthy private endpoint
whose **running** version is at least 0.4.3. An installed CLI version alone is
insufficient. Affected 0.4.0–0.4.2 or unknown running services are deferred without
package mutation: earlier listeners may remain inherited by tmux children, and
installing fixed code cannot repair them. Recovery requires completing affected
work and explicitly coordinating session shutdown with Backbone; the app
installer never kills sessions or invokes a manager restart to bypass this gate.
The narrow first-setup exception requires an installed fixed provider, no data
directory, no service file or loaded service, and no prior setup outcome or
upgrade hold. It may resume an unchanged saved initialize request or start setup,
but skips uv package mutation. Partial installations with prior side effects
remain deferred until their recovery is coordinated.

For a verified fixed launch, the guard persists an operation UUID and obtains
Backbone's existing authenticated upgrade hold before uv runs. It uses provider
read-only operator credential loading, a private same-user Unix endpoint, and
checks kernel PID/start time plus the daemon's start timestamp and version.
After a successful package update it rechecks ownership and process identity,
validates the setup protocol, and releases only its own hold. Failed or ambiguous
updates retain the operation record and hold for coordinated recovery. Foreign
holds and disabled automatic reload settings remain in force. The final health
wait still requires the expected newly installed version. Successful automatic
same-process re-exec is the supported path; service-manager restarts and daemon
failure are not claimed to preserve service-owned tmux processes.

The setup helper uses Backbone's versioned setup command and retains its exact request
and progress in a private per-user state directory. Pending setup is resumed
before any package upgrade (after Linux safety preflight), with the provider version, distribution metadata and
launcher checked against the saved installation. For a fresh installation only,
it enables the private socket and restarts the service after checking the saved
installation identity, service definition and loaded service owner. Interrupted
socket writes are reconciled against the intended value; conflicting choices
are preserved. Existing installations and completed setup keep their settings
and service definitions. This is not a running-session survival guarantee.
The final socket probe sends only an unauthenticated health request;
the app's native transport performs peer verification before sending credentials.
For an unchanged, installer-owned completed setup, upgrades also observe the
installed version becoming healthy on the private socket for up to 120 seconds.
This read-only check validates directory/socket ownership and modes plus the
peer UID, and rechecks the saved installation, unit and socket choices before
accepting readiness. It lets Backbone's automatic upgrade watcher reload without
restarting the service or changing a disabled/held reload preference. Independently
managed or subsequently changed installations keep their own lifecycle.
Provider installation/setup finishes before replacing the app; provider state
is retained if a later app replacement fails.

Shell command integration through `uv tool update-shell` is best-effort. Its
nonzero status can mean startup files already contain the executable directory
but the current terminal has not loaded them. The installer adds the tool binary
directory to its own PATH and runs provider setup by absolute path, so this
optional integration cannot abort an otherwise usable app installation. Actual
provider installation and setup failures remain fatal. Archive transfers use
curl's progress bar; quiet package/setup commands emit elapsed-time updates from
a shell helper that is stopped and reaped when the command finishes. No extra
progress library is required. Each run resolves the latest release to a fixed tag
before fetching its checksum and archive, so retries cannot switch releases.
Downloads resume from a private cache beside the
installation, protected by the installer lock and keyed by the published SHA-256
checksum. Five attempts with exponential backoff tolerate interrupted connections;
a 60-second stalled-transfer timeout replaces a total download deadline. A later
installer run reuses partial or verified bytes for the same checksum. A changed
checksum invalidates the cache, and range rejection or checksum failure starts a
fresh transfer. Verification still precedes extraction and execution. WSL runtime
installation and its sudo prompt occur after the archive is verified, so a long
download does not consume the authorization window.

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
on another server. Settings
and keymaps fall back to the old configuration directory when the new file is absent; settings writes
use the new directory. Legacy environment overrides remain supported, with the new
`AGENT_INTERFACE_*` names taking precedence. No database or running server is moved.
The Linux launcher uses the packaged Agent Interface icon.
Under WSL, the installer installs missing Ubuntu runtime packages through apt
and registers its desktop entry in `/usr/share/applications`, which
WSLg scans for Windows Start integration. These steps require sudo; the app and
Backbone still run as the invoking user. The system entry carries the installer
marker and home directory, is checked before replacement, and participates in
launcher rollback. An ordinary Linux desktop keeps its per-user XDG launcher.

Linux windows request client decorations and draw a matching right-side caption
with minimize, maximize/restore and close controls, plus invisible resize edges.
The app sets Motif decoration hints on its own X11 windows through pinned
`x11rb` 0.13.2 (MIT OR Apache-2.0), so an opaque caption also works without
a compositor or GTK shadow extensions, including the WSLg X11 host. Theme
variant hints follow the selected appearance. A failed X11 connection retains
GPUI’s decoration fallback; window-manager ownership and functions are unchanged. macOS retains the native traffic lights. Linux uses
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

An explicitly bound Sheets client retains its verified profile identity and
credential snapshot through pagination, range reads, exports and mutations.
Prepared mutations retain that client for exact retries, even after credential
rotation; another profile or lifecycle cannot submit them. Binding refusals
preserve drafts without legacy fallback. Native bound Sheets wiring is separate.

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
No spreadsheet widget or formula engine is added. An explicitly bound Reports
pane opens Sheets with the same verified connection. Shelf refreshes, navigation,
exports and prepared feedback keep that original source and credential snapshot.
Read markers and retained workbook conversations include the full profile,
installation and lifecycle identity. Bound workbooks do not inherit the legacy
installation's initial read timestamp. Workspace routing supplies the verified connection when a profile workspace opens.

`roster::plans::Client` and `roster::interactions::Client` retain a verified
profile connection for all list, history and detail pages. Bound interaction
snapshots retain that client for later detail reads, including its original
credentials; their legacy provider URL is empty. Every bound request carries the
full identity. Binding refusal from optional state-history enrichment invalidates
the whole snapshot, while unsupported state history remains an optional error.
Legacy plan pagination also captures its connection once per complete read.
Native profile panes consume these clients through their retained connection.

`roster::plans` reads Backbone's `/api/current-plans` list over the same
authenticated local transport as Sheets, on a worker, every 20 seconds while
the Current plans section is open. It is read-only: it publishes, approves and
changes nothing. A missing endpoint is reported as unsupported, and a failed
reread keeps the plans last read. Each refresh also reads the agents' runtime
metadata; failure of either read preserves the last complete snapshot and shows
the read-error notice in both the list and opened plan. Working states
come from the shared live roster, independently of the plan refresh.
Explicitly bound panes read plans and runtime metadata through the same captured
connection. Plan frames reject another profile's deferred result, and discussions
retain the connection, author, observed revision and step. Their draft keys include
the full profile, installation and lifecycle identity; refreshing or switching
sections cannot redirect an existing discussion. Workspace selection supplies the connection without rewriting an existing pane.
The section draws through the pane, like the record readers, so
an opened plan's text can be selected and copied. Its selected step uses the shared
conversation panel, addressed by the plan author's stable identity. Drafts are
kept by provider, author, observed revision and step; changing steps or refreshing
a plan cannot retarget them. Messages and retries retain that revision and item
anchor. Plan refresh pauses while a draft or unresolved delivery exists, keeping
its step reachable until the person finishes it. The panel requires stored conversation support and never falls back to
unanchored feedback for plans.

`roster::interactions` reads the versioned interaction summary, logical-message
history, standalone originals and paged delivery evidence over the authenticated
private socket. It never claims the inbox or scrapes terminal transcripts. Reads
are bounded by response size, nesting, page count and record count; malformed
timestamps, missing full bodies and unrelated conversation recipients are rejected.
Summary and history share the provider's snapshot token. Stable registration IDs
identify participants, including retired registrations; an unknown sender remains
unknown. A reverse relation keeps its recorded source ID. Full-body retention is
separate from the start of retained history.

Explicitly bound interaction panes retain one verified connection for summary,
state/history reads, detail navigation and linked reports or responses. Deferred
snapshots and link requests check the full origin. Interaction and referenced
artifact discussions retain the same connection and profile-scoped draft keys;
unsupported or refused bound reads never use a legacy directory or open the
legacy Activity fallback.

`desktop::views::interactions` owns the visible window, filters, selected original,
peer-navigation stack and discussions. Reads run on a worker every 20 seconds
while visible; numbered requests prevent an older selection's read from replacing
newer state. New logical messages wait behind an explicit insertion control.
Selection and discussion survive filtering and changes to the time window; a
missing or failed reread leaves retained data visible. `ledger` builds fixed-height
rows and `layout` maps their shared time edges to the rail. Long gaps are
compressed, labels hold time still, and same-second messages keep separate marks.
Only rows near the viewport become native elements. Explicitly anchored
conversation entries group into contiguous runs, so an intervening unrelated
message never produces a backwards time span. Counts remain logical messages,
not delivery attempts or collapsed groups.

Consecutive exchanges with the same recorded counterparty collapse from two
messages, including both directions, when adjacent messages are at most fifteen
minutes apart. A different counterparty or an anchored discussion ends the run;
unknown actors are never grouped by a guessed identity. Expanding a group shows
its individual messages and preserves the original selected message.

State spans have a separate snapshot and stable registration identity. They are
sampled evidence: gaps and conflicting overlaps stay unknown. Message coverage
comes from finite provider-verified capture intervals, independently of state
coverage. Quiet rows require the complete unfiltered message population and
subtract every overlapping logical-message lifecycle, including pending spans.
A failed state refresh retains earlier observations with a stale notice.

The original is selectable plain text. Follow-up questions use the existing
stored conversation component with kind `interaction`, pinned to the provider
and immutable message ID. A collapsed discussion rail is visible from the first supported selection,
before the thread is opened. Its width participates in the responsive layout.
The reader keeps direction, participants and navigation in one header, with the
timestamp beneath; delivery evidence and technical attempts have separate sections,
and the primary Ask action also opens the same retained discussion. Eligible
recipients come from the recorded participants. The focused agent is selected by
default for both sent and received messages when eligible; the owner can switch
the recipient before sending. With no eligible focused agent, a sole participant
is selected, otherwise an explicit choice is required. A retry retains the chosen
registration ID. A changed connection cannot silently redirect it. These discussions never fall back to unanchored feedback,
edit the original, or infer an explanation for its send. No additional process,
web renderer or third-party dependency is introduced.

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

### Linux HTML document host

Linux uses WebKitGTK 4.1 inside the same native window. GPUI exposes an XCB
window; `webview-host` borrows that window identity as an Xlib handle for Wry.
The adapter changes only the representation of the XID and retains the source
handle's lifetime. It does not create a second connection or own the window.
The application selects X11 on Linux; Wayland desktops and Windows WSLg use
XWayland. A Wayland session without XWayland cannot host these child views.
Terminals and all navigation remain native GPUI rendering.

GTK initializes on the main thread with the X11 backend. A foreground task
services its events beside GPUI's loop, bounded to 64 iterations or 2 ms per
turn. It wakes every 16 ms with live document views, otherwise every 250 ms.
Native GTK key events forward configured application shortcuts and the fixed
window, library, settings and quit shortcuts to GPUI, while
ordinary typing and text editing stay in WebKit; authored IPC cannot invoke
those shortcuts. The child follows pane bounds, interface zoom, theme and overlay visibility;
closing or hiding a document destroys its ephemeral view. Child placement uses
physical pixels on Linux, and page zoom compensates for GPUI's fractional display
scale versus GTK's integer scale. Teardown and queued GTK event dispatch trap
GDK X errors: closing the GPUI parent can destroy its child XID before a queued
property update runs. Expected BadWindow errors are logged at debug level; other
X errors are warnings. The trap is scoped to the document bridge, so a late child
update cannot terminate other native windows.
Native document clicks focus the GTK child so moving the pointer does not redirect
subsequent typing. Late page focus notifications are ignored while native overlays
or geometry gestures occlude the page, preserving the active native input field.
Native interface clicks restore X input focus to the original GPUI parent, not GTK's
intermediary window. GTK retains its registered GdkWindow, reparented beneath the
X11 container; replacing that window with a foreign one breaks GTK teardown.
The Wry patch destroys the GTK owner before the owned container synchronously,
including without a window manager, and uses a weak view reference in the IPC handler so
closing a document or switching revisions does not retain its ephemeral context.

Authored HTML loads only after native settings disable WebRTC (not governed
by CSP), DNS prefetch, hyperlink auditing and scripted clipboard/file access.
File chooser requests are cancelled. The shared content policy blocks network
requests, subframes, objects and forms; navigation stays within the document,
new windows and downloads are denied, and the only IPC message reports focus.
The local MIT/Apache-2.0 Wry patch enables the ephemeral WebKit context process
sandbox before constructing any view; the app verifies
that it is enabled before loading authored content. A bounded bubblewrap probe
runs before any web view; denied namespace creation disables Documents with an
explicit error while native views remain usable. Renderer load/process failures
also reach the native error view. The unsafe WebKit sandbox environment override
is refused. On restricted Ubuntu hosts the installer can enable the distribution's
`bwrap-userns-restrict` policy for the root-owned helper, preserving existing
policy and without changing global sysctls or granting userns to writable app
paths. Its child profile restricts capabilities. This optional system setup
requires sudo; refusal or failure does not block installing native views. `LIBGL_ALWAYS_SOFTWARE=1` selects
WebKitGTK's nonaccelerated drawing policy for headless/software environments.

### Bound native Policies

Bound native Policies retain shared catalog IDs with the displayed instruction layers. Shared bodies come from the exact immutable provider revision, and discussions retain that artifact, revision and connection. Base and swarm briefs remain provider-owned read-only previews without artifact discussion authority. Catalog binding refusals invalidate the whole library; missing IDs cannot fall back to preview text or local paths. Reader completion tickets cancel closed openings and reject results after reopening the same path.

### Bound native Team and Tags

`roster::views::content` owns Team and Tags reads, and `desktop::views` owns their native panes. Bound Team and Tags panes use the same captured content connection and generation checks as other observations, with live status from a matching `LiveAgents` origin. Tag membership drill-down stays within the pane. Bound Team rows can open registration-pinned Agent cards; name-only terminal navigation and ambient tag-mutation commands remain unavailable.

### Verified profile selection

Profile selection uses an explicit cancellable handoff in the app crate. Each attempt retains its discovery client and requested profile ID, reads one atomic registry record and authenticates its connection before exposing a selected result. Per-owner tickets reject superseded or cancelled completions, including repeated choices of the same ID. Registry labels/readiness confer no connection authority, and a failed choice supplies no replacement for existing workspace connections. This handoff performs no service or account changes; workspace activation consumes only that authenticated result.

The native `ProfilePicker` component consumes an explicit discovery client and the selection handoff. Catalog tickets invalidate stale refreshes and rows; only currently ready IDs can start verification. Selection emits an authenticated connection or a fixed error, never a registry-only binding. Closing cancels pending work, and committed/closed pickers reject further interaction. The workspace menu opens this picker to create a workspace bound to an existing ready profile.

Profile-bound session discovery retains a verified connection and queries only its authenticated tmux socket. It verifies the original provider binding before and after each read-only session listing; refusal clears session names and returns a typed profile error with the full origin. Visibility generations and cancellation discard obsolete reads. This path neither starts servers nor attaches clients, and does not fall back to legacy socket resolution. Workspace choosers supply their own verified connection.

Verified connections also carry an opaque, process-local cache key. Clones share
that key; each fresh authenticated handshake gets a new one even if its full
provider identity is unchanged. The desktop roster registry deduplicates by both
origin and this key, so retained panes keep their old credential worker while a
fresh connection can recover after credential rotation. The key is neither
serialized nor used as request authority; wire validation still uses the full
profile, installation and lifecycle identity. Legacy directory sharing is unchanged.

`roster::profiles::runtime_context` acquires an agent registration from a bound
inspection and retains that registration, runtime and verified connection for
refreshes and permission reads. List-derived navigation supplies its observed
registration immediately. Retained requests always include `expected_registration_id`;
replacement or removal invalidates the target. Permission responses must match
the full profile identity, name, runtime and registration. Typed provider-parsed
permissions expose only relative display labels and fixed unavailable/unsupported
messages; no local runtime files or home directories are read. Native Agent cards
consume this retained context. The shared profiles fixture includes contract
revision 2 extension 1 for setup schema 2 and registered runtime context.

Native pane and attachment choosers retain an explicit discovery origin. Bound
constructors use the verified connection for both session and team observations;
legacy constructors keep their explicit directory/socket behavior. Session replies
must match the retained full identity, and worker visibility generations discard
hidden reads. Event owners can retrieve the retained connection and must validate
it against their workspace before accepting a choice. Bound new-pane choosers
retain registrations for Agent cards. Desktop routing constructs them from the owning workspace connection.

Workspace records may carry an optional stable `profile_id`. Schema-v2 projection
and ordinary three-way conflict handling preserve it; endpoints, lifecycles and
credentials are never persisted in the layout. Missing or null IDs retain explicit
legacy/standalone meaning. A native picker can create a workspace for an existing
ready profile. Restored bindings resolve through read-only registry discovery and
an authenticated handshake, with bounded concurrent attempts and explicit retry.
Unavailable bindings display a connection state instead of opening legacy content.

Each workspace retains its verified connection and status source. Pane entities,
attachment choosers, attention markers and new native windows inherit that authority.
Changing a saved binding or explicitly reconnecting invalidates old pane owners;
delayed callbacks cannot apply to their replacements. Cross-workspace tab transfers
require the same connection instance, including its credential snapshot. Bound
terminal attachments use only the verified tmux socket, recheck the provider before
and after attachment, and release the owned client if that final check fails.
Bound panes never resume agents or create ordinary shells through legacy paths.
A bound shell comes from the provider: when a pane with no attachment connects,
`POST /api/profile/shells` ensures a session for the pane on the verified tmux socket,
and the app then attaches to it. A new pane sends no folder, so the shell starts in the
profile's home. The app
sends no environment and never starts a bound tmux server, so the app's launch
context cannot reach the shell. Signing a CLI account in works the same way:
`POST /api/profile/accounts/sign-in` returns a session on that socket, and the pane
attaches to it so the person completes the CLI's own sign-in there. A profile itself
has no sign-in; a workspace selects it. A Default profile keeps its existing CLI
sign-ins, so it does not offer this. An Agent card removes its agent after a
confirmation: the app stops the session, then deletes the registration, because the
provider refuses to remove a running agent. The Skills view and the Documents section
add a person's own folder or HTML file to the profile through the provider: a skill is
copied for every agent in the profile and the folder stays in place, and a document is
published under the reserved `operator` author. Native lifecycle and setup controls use the captured
operator client and durable recovery controllers. Real-account acceptance remains
separate; unavailable provider operations are shown explicitly. Existing standalone
workspaces retain their ordinary shell behavior.

Sidebar attention actions, retained view events and Jump results capture their
workspace, connection epoch and library origin before dispatch. Navigation validates
that origin and selects the original workspace; a removed, rebound or reconnected
workspace rejects the old callback. Jump activates the displayed target rather than
looking up its old list index again. Closed view entities and replaced popovers
cannot dispatch into their successors. Moving a retained view renews its event
subscription for the destination workspace; events from the previous subscription
generation expire while the view and its conversation state remain intact.

New Tab and Split controls capture the displayed workspace, selection and connection
epoch. They refuse stale callbacks and recheck after consuming a pending library
update, before creating a save intent. Keyboard commands capture at dispatch;
rendered buttons, Jump commands and tab options retain their displayed destination.
Tab-menu listeners also retain the menu generation, so a retired listener cannot
consume a replacement menu or its creation target. Name editors and inline pane
choosers accept events only from their active entity. Tab operations, name commits
and icon changes recheck the selected workspace origin after pending library
updates, while focus changes within the same workspace remain valid. Attachment
and terminal openers also capture the pane origin before their own refresh; a
nested refresh cannot turn an earlier bound action into a legacy-source lookup.
Sidebar workspace/tab choices, menu openers, pane focus and tmux window controls
retain their rendered origin. Reorder and resize gestures reject binding or
connection changes even when saved IDs and tree contents remain unchanged;
retired terminal entities cannot receive window-selection callbacks. Workspace
menus also retain an opening generation, so an old action cannot consume or
replace a newer menu. The unavailable-profile retry button retains its displayed
origin and rechecks pending library updates before resetting connection state;
delayed retries cannot discard a different or already recovered connection.
Close Pane and Close Tab capture the selected origin before consuming library
updates and reject changed bindings or connection epochs even when tab and pane
IDs are unchanged. Fresh local closes still work for unavailable profiles.
The application-menu setup command opens first-run setup from any workspace.
Setup always adds its agent to the Default profile, never to the selected
workspace's profile, and Manage offers the same flow as Add agent on Default.

Save reconciliation treats a workspace's profile binding and its tab contents as
an ownership boundary: a concurrent binding change cannot merge with a pane edit,
tab addition or transfer in that workspace. The transaction rechecks this against
the latest stored layout, so a pre-save UI check alone cannot authorize a stale
edit. Connection-unavailable content also hides retained native web surfaces, and
background focus resets preserve an open profile picker's keyboard ownership.

Profile-bound Team cards retain a listed agent registration on Alt-click and save
it as `view_registration_id` in the schema-v2 pane. Bound agent overview and
permission reads require this registration and the owning workspace connection;
restoring a saved card cannot reacquire a same-name replacement. A separate native
card uses provider-parsed runtime context, never legacy host-file permission
inspection or default launch-policy claims. Refusals clear the card. Name-only
saved cards require reopening from Team, and removing a workspace binding does
not convert a registration-pinned card into a legacy read. The card's other
sections (Connections, Skills, Policies and Usage) read on the same connection
after that registration check; their skill and policy rows list without opening
the reader. Name-only terminal navigation is unavailable.

The new-pane Agent chooser carries the same observed registration as Team
navigation. Bound choices omit unregistered or malformed targets. Row keys include
registration identity, so a refreshed same-name replacement disarms selection and
cannot inherit a previously rendered number key or click target. The chooser
exposes only the supported bound overview and permission card; it never builds a
name-only bound Agent view.

Profile first-agent setup uses an independent schema-2 request model. Preparation
captures the full expected profile tuple and immutable agent/hook choices; the
cancellation body changes only the action and keeps the original request ID.
Bounded JSON-lines decoding checks every progress and final event against that
origin and action, enforces monotonic sequence numbers and refuses incomplete or
contradictory streams. Only a precise provider cancellation acknowledgement allows
new choices; unsafe cancellation preserves the original operation. These models
invoke no process and do not activate the legacy setup UI for a bound workspace.

Profile runtime catalogs use the captured discovery executable and caller context,
with all three expected identity fields sent explicitly. Only successful schema-2
responses for that tuple are retained; unsupported flags, command failures and
foreign results never fall back to legacy discovery. Catalogs keep authentication
observations separate from executable availability and setup compatibility.
Provider paths and unattended argument examples are discarded; these reads cannot
launch runtimes or authorize an account operation. Discovery remains bounded and
cancellable, and retained catalogs carry their full origin for deferred consumers.

Local lifecycle `Operation` owns a prepared request together with the discovery
client's captured executable and caller environment. Apply writes the exact bytes
to a private temporary file; receipt lookup uses the original request ID without
resubmitting. Bounded receipt reads and deadlines leave uncertain outcomes attached to
that request. Nonzero exits may yield validated durable failure receipts, never
success. Cleanup terminates only the direct command, since a provider may launch
long-lived service children; it does not kill the service process group. Callers
must persist original request state before exposing lifecycle actions in the UI.
No lifecycle control is automatically invoked by discovery or selection.
The receipt sink is an anonymous regular file, so inherited service writers do
not receive SIGPIPE when the consumer finishes or exits. Receipt reads reject an
observed file above the output limit during invocation; the provider owns the
logging destination and lifetime of any service that inherits the descriptor.

Profile setup execution owns immutable schema-2 requests and the captured provider
client. It emits only validated progress steps before exit; terminal readiness or
cancellation is returned only when the full-origin result matches the process exit
contract. Timeouts and loss of observation terminate and reap only the direct
command, leaving the same request with an unknown outcome for explicit recovery.
Neither cleanup nor closing confirms provider cancellation or rollback. The
consumer does not detach a setup command and allow a concurrent retry.
Private request files and anonymous regular output files preserve inherited service
writers, while observed receipt bytes and stream events remain bounded. The provider
owns long-lived service logging. No setup executor is invoked by discovery or
selection; native recovery controls persist the exact request before execution.

Lifecycle request checkpoints use a private consumer format, separate from the
provider protocol. They preserve exact generated request bytes, the first reserved
identity and terminal non-regression across restoration. Restoring validates the
original request and saved observation without discovery, new IDs or command
execution. Provider diagnostic
prose is discarded. A restored record supplies expectations only. Durable storage
and verification of the original operator/registry context are required before
any restored request may execute; these model APIs do not activate lifecycle UI.

Setup request checkpoints retain both generated schema-2 bodies byte for byte,
including cancellation's original ID, profile tuple, hooks, project and brief.
Restoration validates all choices and rejects changed or noncanonical bodies
without discovery or allocation. The private format records request intent only:
interrupted output streams and successful readiness/cancellation are not inferred
from it. Durable storage remains required before recovery can submit either body. No setup UI is activated by restoration.

Profile control-plane clients capture the working directory with their executable
and environment; all discovery, lifecycle and setup commands use that snapshot.
Private executor recovery records wrap model checkpoints with an operation kind.
Environment values are not persisted. A record reopens however the app was launched,
because launching from another terminal or after a reboot changes the environment
without changing the operation: the profile tuple and request ID pin it, and the
provider refuses a tuple that no longer matches. This is not authentication of an
executable or protection against malicious same-user record replacement. The durable
controllers and native recovery panes described below build on these records.

Private operation journals use an owned 0700 directory and 0600 records. Directory
components and record opens reject symbolic links; file descriptors pin the root
for subsequent access. A stable advisory lock serializes independent windows and
processes. The lock guard explicitly unlocks before closing its descriptor, so a
concurrently spawned child cannot prolong a completed operation's lock before exec.
Compare-and-swap revisions prevent stale observations from replacing
newer ones. Writes use exclusive temporary files, file sync, atomic rename and
directory sync; a write error may follow replacement, so callers reread before
retrying. Bounded binary records retain exact opaque capsules with identity,
revision and checksum validation. The existing `rustix` and `sha2` dependencies
provide descriptor-relative filesystem operations/locking and corruption checks.
The journal grants no operation authority and invokes no provider; controllers
must validate capsule context and state, persist before submission and reconcile
uncertain writes before native recovery actions can be enabled.
On macOS, the journal also refuses extended ACLs through the open descriptors:
mode bits alone cannot establish privacy there. This deliberately includes
otherwise restrictive ACLs. Opening an existing journal syncs its parent too,
so an interrupted directory initialization cannot bypass that durability step.

The MIT `native-fs` workspace crate keeps Darwin ACL FFI outside the safe app
crate. It uses the existing MIT/Apache-2.0 `libc` dependency for system types; no
additional third-party dependency or service is introduced.

The app's lifecycle controller stores exact request intent before submission and
syncs a `Submitted` phase before invoking the provider. Recovery reads the
original request receipt by default; resending requires an explicit identical
retry. Journal keys derive from the original request ID, so duplicate enqueue
attempts cannot silently replace intent. Each invocation claims a revision and
may publish its validated receipt only after the updated capsule is durably
stored at that revision. Competing observations fail closed; the caller rereads
the same key after errors. Terminal receipts disable further controller commands.
These receipts remain lifecycle progress, never authenticated content access.
Native action controls invoke the controller off the UI thread. First-agent setup
uses its own controller and journal with the same durable recovery boundary.

Recovered controller state crosses a fresh directory-sync barrier before it is
returned or treated as terminal. A visible record alone cannot establish durable
completion after an earlier post-rename sync failure. The journal's ordinary
read remains a storage inspection; `read_durable` provides this recovery barrier.

Setup terminal observations have a separate private checkpoint from request
intent. They retain the original tuple, request, action, sequence and bounded
outcome/retry fields, including the actual launch ID for confirmed readiness.
Normalization removes provider prose and unneeded result details; restoration
revalidates the result through the existing schema-2 stream rules and requires
canonical encoding for the original prepared request and explicit action.
Progress is never reconstructed as terminal authority. The executor must still
verify the matching process exit before a controller durably saves an outcome;
these model checkpoints neither prove that exit nor grant content/account access.

The setup controller persists both exact request bodies before execution and
records cancellation intent before invoking it. An unknown cancellation outcome
permits only an explicit retry of that same cancellation; it never resumes
first-agent work. Confirmed unsafe cancellation preserves the original setup
request, while validated retry policies govern subsequent explicit attempts.
Readiness and successful cancellation close the request. Every claim and result
uses journal revision checks, and recovered state crosses the durability barrier.
Progress carries the record key and revision for deferred UI checks; terminal
results are returned only after matching process exit and durable storage.
Native recovery controls retain the original record key and revision while invoking
this controller off the UI thread.

Native profile service controls are an explicit branch of the workspace picker.
Opening them captures the selected library location once and opens a private
lifecycle journal; it does not invoke a lifecycle action. Start/Stop controls use
an atomic registry record loaded before the buttons appear. Submitting switches
the view to the original operation key before persistence or provider execution,
so ambiguous writes and replies retain recovery identity. Reopening enumerates
bounded record IDs and validates one page of durable, caller-bound snapshots at
a time. Receipt lookup and identical retry are distinct explicit actions. Closing
interrupts the local command; its outcome remains unknown and closing does not
claim provider cancellation or rollback.
Worker generations reject stale results and controls. Successful receipts remain
progress metadata: workspace selection still performs the authenticated handshake.
Creation uses a child form. Fresh first-agent setup choices
remain separate from saved-request recovery.

Native creation takes an explicit profile name and uses the one baseline the
provider can create today. The form emits one immutable request into the same
durable lifecycle controller and recovery view; later field edits cannot rewrite
it. Escape/back cancel form observation, and text composition is respected. The
form neither selects a workspace nor grants connection authority.

Native profile management exposes saved first-agent setup separately from the
current runtime catalog. It opens a private setup journal from the already
resolved library directory and renders validated records in bounded pages.
Retries and cancellation retain the original profile tuple and request bytes. Progress is checked against its journal key, request, tuple and
revision; superseded pane tasks cannot publish results. Stopping observation
reloads the same record and never reports safe cancellation. Confirmed setup
readiness does not authorize account access or open an ambient terminal.

Fresh setup admission validates existing durable setup records and inserts the
new intent under one journal lock. An open request blocks only the same agent name
in the same full profile instance, so a second agent can be added while another
needs attention. A request closes on confirmed readiness, safe cancellation, or a
result whose retry policy says this same request can go no further (a name already
in use answers `new_request`, for example). A result the provider says can be retried
(`launch_timeout`, `runtime_permission`) stays open, and the recovery view shows fixed
text for its code. Forgetting a request removes only the app's saved record; it runs
no command and keeps any agent the request created. Malformed records fail closed
until they are forgotten; unsafe storage always fails closed. A directory durability barrier precedes the
check, and the bounded scan has a read budget. Catalog-facing pending reads are
observations only: actual admission repeats the check atomically. This local
journal rule does not replace the provider's own request serialization.

First run stays with the installer's Default profile: the installer initializes it,
onboarding adds the first agent through schema-1 setup, and the app then opens that
agent in a workspace bound to Default, through the same handshake as any profile.
tmux-only use, with no Backbone, stays unbound.

A profile workspace separates connecting from a failed connection and shows the
reason. A profile's service does not restart itself after a reboot and its agents
never return with the service, so the failed state offers Start service (`profiles
apply` start, recorded in the same journal as Manage) and a stopped agent's pane offers
Resume and Start new (`POST /api/agents/{session}/start`, which takes the name only).
A failed roster read loses status, not access: tmux keeps the sessions and panes stay
attached, with a status note, while the read retries.

Native first-agent setup captures one atomic registry profile and the operator
client before loading choices. It checks the independent private setup journal
for open requests, which it points to without blocking other agent names, and then
reads that instance's schema-2 runtime catalog.
An explicit project folder, name, brief, runtime and optional catalog model are
frozen with hook and greeting choices into one original request. Compatibility
and authentication observations remain separate; neither opens a terminal or
connects a workspace. Confirmation hands the retained controller and prepared
request to recovery with its original key established before admission or I/O.
Admission repeats the pending check atomically. Failed or interrupted submission
keeps recovery on the original key; queued form callbacks cannot create a second
request after the form closes.

Provider request files are created inside fresh private directories only after
checking the temporary path's ancestors, ownership, mode and macOS ACLs. Open
directory and file descriptors must pass the same privacy checks as the journal
before any request bytes are written. Receipt sinks use this preparation too,
then become anonymous regular files so inherited service writers can survive
consumer cleanup. Unsafe temporary locations fail before provider invocation.
The journal capacity includes the worst-case JSON escaping of a complete
client recovery capsule and its envelope, including terminal observations; an
accepted large request cannot exhaust that capacity merely by gaining a receipt.

Consumer-generated lifecycle request IDs are limited to 1 KiB before intent is
accepted or restored. Submission uses a file, but receipt lookup passes the ID
as a command argument; the bound keeps accepted IDs well below native argument
limits. Large request content and terminal metadata retain their separate wire
and journal bounds, and lost-reply recovery still looks up the original ID.
