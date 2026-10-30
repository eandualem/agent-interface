# Use Agent Interface

Start with the [installation guide](INSTALL.md), which covers source builds
and native packages. Terminal sessions require tmux 3.3 or newer. Agent views
connect to a running Agent Backbone.

If no library exists, the first launch creates one empty workspace. Add a tab
with **+**, Command-T on macOS, or Ctrl+Shift+T on Linux, then choose **Open
terminal** or **Attach a session**. Opening an empty workspace does not start a
shell. To open another saved library, open Settings, choose **Open another**,
and select its folder. The shortcut is Command-O on macOS or Ctrl+Shift+O on Linux.

Sessions run on the same computer as the app. Connecting to another host or
synchronizing live sessions between computers is not supported.

## Navigate and use terminals

A **workspace** is a collection of tabs. A **tab** contains one or more
**panes**. Each pane holds a terminal session, a native view such as Skills,
or an experimental HTML artifact.
A session is the running work shown inside a terminal pane; closing its pane
does not send a session-termination command.

Use the workspace heading to choose a workspace, then choose a tab. The saved
split layout contains actual sessions; click a terminal before typing. Command-1
through Command-9 select tabs, and Command-Option-1 through Command-Option-9 select
workspaces. Command-[ and Command-] cycle panes; add Shift to cycle tabs.
Command-Option-Left/Right cycle workspaces, Command-Shift-Enter expands or
restores the selected pane, and Command-B focuses the sidebar. Navigation focuses
an already connected destination; if it is still connecting, click it when ready.
Command-V pastes clipboard text or copied file paths (Control-Shift-V on Linux),
using bracketed paste when the pane requests it. Drag files onto a terminal to
insert their full paths; spaces and quotes in names are preserved. On macOS,
you can also drag the floating screenshot thumbnail into a terminal. Temporary
screenshots and image-only drops are received as private temporary PNG files,
so the program in that terminal can read them. Keys follow the destination pane's
application cursor and extended-key modes. Paste is limited to 4 MiB per operation.
Double-click a pane header to expand it, then double-click again or press Escape
to restore the layout. **Focus one pane** in tab options does the same. Open
menus and dialogs handle Escape first. Scroll over a terminal to read its retained history. Shift-Page Up/Down moves a page;
Shift-Home/End goes to the oldest output or the live screen. Typing or pasting
returns to live output. Drag to select text; double-click selects a word
and triple-click selects a row. Command-C copies it (Control-Shift-C on Linux).
A selection stays on its output while the pane scrolls or new output arrives.
Drag past the top or bottom of a pane to scroll it while selecting, or scroll
and Shift-click to extend the selection to another point. Escape, typing or a
new click outside the selection clears it. Copies from recovered screens
currently retain their displayed row breaks, including wrapped lines.
Discarding old history, recovering the terminal or switching screen buffers clears the selection.
For programs with mouse reporting enabled, hold Shift to select text locally.
Ordinary clicks, drags, requested pointer motion and wheel gestures reach the
program. Each drag stays with its original pane; only one held-button gesture is
forwarded at a time. On macOS, either left/right release ends an active left/right gesture;
simultaneous left/right button chords are unsupported.
Hold Shift to keep scrolling local to the native viewer.
Command-N or **New Window** opens another native window on the current library,
with independent navigation and terminal connections. Closing one leaves the
other windows open. Command-O (Ctrl+Shift+O on Linux), or **Open another** in
Settings, chooses another folder containing `layouts.db`.

Text in Skills, policies, agent briefs and report readers supports drag selection,
double-click word selection, and Command-C to copy (Control-C on Linux).
Command-A selects the document text (Control-A on Linux). **Copy Markdown** in
a skill or policy copies the whole file when available; oversized skills offer
**Copy Markdown preview** for the portion shown. The Edit menu offers Copy and Select All; on macOS
these also work with selected text inside HTML Documents.

## Find work and change appearance

The window follows the Quiet Instrument design ([design language](DESIGN.md)).
The workspace icons at the bottom scroll horizontally when they do not fit.
The agents list shows at most nine rows at once, with working and starting agents
first; scroll to reach the remaining agents. Each row shows the name and its
status symbol.

The navigation panel on the left shows the current workspace (click it for the
workspace menu), a **Jump** field (Command-K) that finds tabs, panes, agents and
commands across every workspace, the tabs with a thumbnail of each layout, and,
with Agent Backbone installed, the **AGENTS** section: agents needing you first,
each with a state symbol. Click an agent to show the tab holding its session.
The bottom row switches workspaces with one click (right-click for options,
including **Choose icon…**) and opens Settings. Drag the panel's right edge to
resize it. Panes are cards with a header naming what runs there; the focused
pane has an accent ring and the others are slightly dimmed. The status row
names the workspace, tab and pane, shows the keys for the current mode and the
saved state, and **agents shown** hides the agents section.

Double-click a pane header to expand it, then double-click again or press Escape
to restore the split layout. Expansion belongs to that tab: switching tabs leaves
their own layouts unchanged, and returning restores the tab's previous view.

Settings (Command-comma) offers a dark or light theme, the interface size,
the terminal text size, whether panes show headers (always, on hover or never) and editable shortcuts;
changes apply to every window at once and are saved in
`$XDG_CONFIG_HOME/agent-interface/settings.toml`, or
`~/.config/agent-interface/settings.toml` when that variable is unset.
**Interface size** (75% to 200%) scales the whole window, terminals included,
on top of the display's own scale. Use it when a screen draws the app too
small or too large, such as a 4K display under WSL.
In **Settings → Shortcuts**, commands are grouped by Workspace, Tab and Pane,
with separate Navigation, Terminal and Application groups. Click **Change**
beside a shortcut and press the new keys. Changes apply and save immediately;
Escape or **Cancel** stops editing. A conflict is shown beside the shortcut.
Rows marked **Fixed** cannot be changed in this editor.
If a new Close or Detach default conflicts with a saved shortcut after an
upgrade, your saved shortcut wins; the new command shows **Not assigned** until
you choose keys for it.

To investigate a problem, open **Settings → General → Diagnostic logging**.
**Warnings** is the quiet default. **Debug** records startup, terminal lifecycle,
layout and settings saves, and provider availability. **Trace** adds timings and
outcomes for individual connection and refresh operations. The selection applies
immediately and is saved for later launches, including opening the app normally
from the desktop. Development builds also default to Warnings; verbosity is
always an explicit choice. You can set `logging = "debug"` or `logging = "trace"`
in `settings.toml` before launching; `logging = "warn"` restores the default.

Logs stay on this computer: `~/Library/Logs/Agent Interface` on macOS, or
`$XDG_STATE_HOME/agent-interface/logs` on Linux (default
`~/.local/state/agent-interface/logs`). Settings shows the actual location.
`events.jsonl` holds the newest newline-delimited JSON records; numbered files
hold older records. Four files of at most 4 MiB each bound disk use. Copy these
files when reporting a problem, along with what happened and when. Nothing is
uploaded automatically. Logs contain diagnostic identities, event names, status
codes and timings, not terminal contents, typed commands, feedback text or
credentials. Settings reports unavailable logging, failed writes and dropped
records. A forced process termination can lose queued records.


**Close Pane** (Command-W on macOS, Ctrl+Shift+W on Linux) removes only the focused
pane after saving. Closing the last pane closes its tab. **Close Tab**
(Command-Shift-W on macOS, Ctrl+Alt+Shift+W on Linux) removes the whole selected
tab and detaches its clients. Closing the last tab leaves the
window open. Use the window close control or **Close Window** in the application
menu to close the whole window. Ordinary shells keep running; external sessions
retain their own tmux lifetime policy.

The agent roster refreshes immediately, then two seconds after each completed
request. Team and agent views refresh three seconds after each read. A rotating
ring marks working or starting agents; labels distinguish starting, working,
blocked, needs you, idle and unknown. Failed requests show unavailable or stale
data, rather than treating the last successful state as current. These are
polled observations, so changes appear after the next successful response.

## Organize workspaces and tabs

Open the workspace menu by clicking or right-clicking the workspace block.
Choose **New workspace**, or press Command-Shift-N, and enter a name. Enter
saves it; Escape cancels. New workspaces have no tabs or shells.
Choose **Delete workspace** to remove an empty workspace; the last workspace and
workspaces with tabs cannot be deleted. Deleting selects the first remaining
workspace and leaves its tabs and sessions intact.

Click **+** beside TABS or press Command-T to add an empty tab. Command-D splits
right; Command-Shift-D splits below. Both split actions are also in tab options.
Drag a tab onto another tab in the sidebar to move it to that position;
the order is saved while the selected tab stays selected.
The existing pane remains intact and the new pane asks what it should run: Open
terminal, or Attach a session. New tabs
use the launch directory, or your home folder when the app was opened from the
Dock or Finder; splits inherit the ordinary shell’s remembered directory.
The new arrangement appears after saving. Navigation made while saving stays local;
otherwise the new tab or pane is selected.

Drag the divider between native panes to adjust their relative sizes. Release
saves the ratio; Escape or switching to another window during a drag cancels it.
This declares the native viewport's size to tmux. Its `window-size` policy
can change the shared window geometry seen by other clients. A changed tab or
split tree cancels the gesture. Save errors offer the usual Retry or Discard.

To rearrange panes within a tab, drag a pane's header onto the left, right,
top or bottom edge of another pane. The highlight shows its destination.
Release to move it into an equal split; its session or view stays intact.
Drop in the center, outside the panes, or press Escape to cancel. Restore an
expanded pane before rearranging it. Terminal headers must be set to Always
or On hover in Settings. Saving uses the same Retry and Discard controls.

## Open a shell or attach a session

Choose **Open terminal** in an empty pane or tab options to start its
login shell, or reuse it if already running. The pane is saved before its shell
starts; a save error offers Retry or Discard. The new shell uses its saved directory
and the creating window's allowed environment. Existing shells retain their process
and environment. Navigating away before saving finishes cancels pending startup;
choose Open terminal again when returning. Exited shells retain their output and
are not restarted by this action.

Ordinary shells use the library's persistent tmux shell server; attachments use
their saved source socket, or the source resolved from the launch environment.
Use **Attach a session** in an empty pane, **Attach session** in tab options, or
Command-Shift-A, to
choose an existing source session. Type to filter names, use Up/Down and Enter,
or click a row. Escape or Cancel closes the chooser. It retains its destination
when focus moves within the tab; a changed or hidden empty destination must be
chosen again. Confirmation saves before replacing the terminal. Empty panes keep
their identity and saved directory when filled. Replacing an ordinary terminal
leaves its shell on the library server. Saves refresh the current tab’s nonempty
ordinary-shell directories, including parked shells, without copying external
session directories. Failed reads retain saved paths. Return to shell in tab options restores the existing ordinary shell
after saving; choose Open terminal if that shell is absent. The chooser lists tmux sessions
without optional agent metadata.

Attached sessions reconnect automatically when the saved session returns on its
source server. Attempts run at the two-second window-refresh interval and never
start an agent or create a session on their own. When the pane shows a stopped
Agent Backbone agent, **Resume** asks Backbone to continue its saved conversation
and **Start new** starts a new one, both with the agent's saved runtime and model;
the pane connects once the agent's session appears. **Detach** returns the pane
to its chooser. An ordinary terminal whose saved shell is no longer running, such
as after a restart, starts a new shell in its saved directory once, automatically.
If that fails, **Open terminal** tries again. Starting a new shell does not
restore a lost shell's process state.
**Detach** in tab options, or Command-Shift-X
(Ctrl+Shift+X on Linux), works with attached sessions, ordinary terminals and
native views such as skills and reports. Detaching keeps the pane and its saved
directory, so you can choose new content or reopen the existing terminal. The
change takes effect after saving; it sends no session-termination command.

Switching tabs, workspaces or pane focus keeps existing connections alive.
A connection ends when its session disconnects, you detach or replace its pane,
remove its tab, open another library, or close its native window. The source’s
session-lifetime policy still applies to an explicit detach. Connections preserve
the session environment. A displayed
terminal sizes its session to the pane under tmux's `window-size` option; text
scales down only while another client keeps the window larger.

## Rename and save changes

Changes saved by the Python app refresh automatically. Double-click the workspace
name or the active tab's name to rename it. An inactive tab must first be selected.
Command-R renames the active tab; Command-Shift-R renames the workspace. The tab's
**⋯**, or right-clicking any tab, opens options to rename it, move it up/down or
move its complete layout to another workspace. Workspace options also offer
**Rename workspace**. A tab changed by another window must have its options
opened again before editing it.

The original name starts selected; Enter saves it, Escape or
clicking elsewhere cancels, and Ctrl-U clears the field. Names are limited to 80
characters. A failed save retains the edit and offers **Retry saving changes** or
**Discard unsaved changes**. Discard restores the last shared arrangement and
resumes polling with surviving local navigation. Panes that remain displayed keep
their connections.
A conflicting peer edit refreshes the saved arrangement and asks you to repeat
the edit.
Renaming does not change tmux sessions.
Navigation and pane expansion stay local. Attached
sessions start on the source’s current window. An attached session has one row
above its terminal naming it on the right. When it has several windows, their
names share that row: click one to view it without switching other clients.
The chosen window is remembered locally when switching workspace tabs; reopening
the library starts from the source's current window again.
The list follows tmux, checking every two seconds. Custom tmux
prefix bindings, terminal input methods
and key-release events are not yet wired into the product UI.

## Read reports and interactive artifacts

In an empty pane, choose **Reports & updates** to read Agent Backbone's
authored reports and retained answers. **Recent updates** shows report history;
**Responses** shows questions, answers and their next actions. **Waiting on
you** shows owner blockers from each agent's latest report. A report's age does
not resolve its blocker, and an answer's action text is not a pending approval.
Choose a row to read it; use Back or Escape to return. Previous/Next page browse
older records, and Refresh returns to the newest page. Authored records cannot be
edited in the reader.

**Documents** shows lasting rich documents stored by Agent Backbone. Each row
is the newest revision of one document. Use the same agent filter, or open a
document and choose **More from this author** to follow its stable author even
after an agent is renamed or removed. **Back** restores the selected page and
filter. **Refresh** returns to the newest documents.

An open report, Response or Document has a conversation with its author on
the right edge. Choose its button, or press **C**, to open it; press **C** or
Esc in the message field to close it again, keeping the draft. Enter adds a
paragraph and **⌘↵** sends up to 4,000 characters. Each message includes a
reference to the exact record being read, and the app checks that the author
is still registered before sending. A message shows **Sending…** until Agent
Backbone answers, then the time it was sent; **Not sent** offers **Retry**,
and **Edit** to move the text back into an empty message field.
When Agent Backbone keeps conversations, the thread is the record's stored
conversation: your messages, the author's replies and the record's new
revisions, in order, with a **new** divider before what arrived since you last
looked. It is read every few seconds while open, and the rail shows a mark when
something new arrives while it is closed. **Earlier messages** loads older
ones. Retrying a message Backbone did not confirm is safe: it is stored once.
A message whose delivery failed after Backbone stored it offers **Retry**,
which delivers it again without adding it twice.

With an Agent Backbone that does not keep conversations, the thread shows only
the messages sent from this pane, and if delivery is uncertain the message
says so: check with the author before sending it again. Delivered or queued
does not mean the author has read it. Threads and drafts remain with their
record when switching tabs or workspaces; detaching or closing the pane or app
discards the ones Backbone does not keep. A message never changes the authored
record.

A shared skill or policy opened in its reader has the same conversation, so
you can work on it with an agent. Backbone names a maintainer only when one has
been set; until then, choose who to ask from the agents the skill reaches, the
conversation's participants or the team. Skills kept in a repository or a CLI's
own folder have no conversation.

Opening a document loads that exact revision. **Check for update** refreshes its
correction information; **Open newer revision** opens the linked correction.
An update never silently replaces the revision being read. Documents remain
stored across agent restarts and ordinary report retention. Inline controls
reset when the document is closed or its pane is hidden.

Publish from an agent's configured Backbone environment:

```sh
backbone documents publish --file explanation.html --title "Explanation"
backbone documents publish --file revised.html --title "Explanation" --supersedes 7
```

Documents need an Agent Backbone version with the Documents API. Older or
unavailable providers show an explicit error. Rich HTML renders inside the app
on macOS and Linux, including Windows through WSLg. Linux uses WebKitGTK 4.1
and X11 (XWayland on Wayland desktops and WSLg).
Self-contained HTML is limited to 4 MiB, including inline assets. Network
requests, external resources, new windows, downloads and host-command access
are disabled. Native terminal rendering is unchanged.

Existing **Artifacts** panes hold agent-created HTML documents in the current
library and work without Backbone. This legacy view uses the same system renderer as Documents but is no longer
offered in the new-pane chooser.
To publish a document from the source build:

```sh
cargo run -p agent-interface-app -- artifact publish report.html \
  --title "Architecture report" --author "worker"
```

The installed binary accepts the same `artifact publish` arguments. Add
`--data-dir PATH` to select another library, using the same path as its window.
Publication prints JSON containing the new document's ID, title, author and
time. It copies the file, so the source can subsequently move or change.
The author label defaults to `BACKBONE_AGENT`, then `local`; it is attribution,
not a verified identity. The open Artifacts list discovers publications within
its next two-second refresh. Choose a document to open it in that pane.

Write one self-contained UTF-8 HTML file, up to 4 MiB including its assets.
Inline CSS and JavaScript, SVG, canvas, animations and data URLs are supported.
External scripts, styles, images and network requests do not load. Links cannot
leave the document; downloads, popups, frames and form submissions are disabled.
The document cannot call terminal commands or the Backbone API. Store any
results that must survive as part of the published HTML: document state resets
when it is reloaded, closed or hidden by switching tabs or expanding another
pane. Publication persists until its local artifact directory is removed.

Documents follow the app’s active light or dark theme. The reader supplies text,
surface, link and control colours. Authored layout, images, SVG and canvas remain
intact; graphics should provide readable contrast in both themes. Stored HTML is unchanged. For themed graphics and standalone exports,
consume these host CSS variables with fallback colours:
`--ai-background`, `--ai-surface`, `--ai-text`, `--ai-muted`, `--ai-accent` and
`--ai-line`. Do not redefine the host variables or force a fixed theme with
important colour overrides. Keep text selectable. For example:

```html
<!doctype html>
<style>
  body { padding: 24px; }
  body { color: var(--ai-text, #17202a); background: var(--ai-background, #fff); }
  button { color: var(--ai-text, #17202a); background: var(--ai-surface, #f4f6f8); }
</style>
<h1>Architecture report</h1>
<button onclick="this.textContent = 'Details opened'">Show details</button>
```

## Choose and recover a library

Omit `--data-dir` to use `AGENT_INTERFACE_DATA_DIR`, then
`$XDG_DATA_HOME/agent-interface`, or `~/.local/share/agent-interface` by default.
On first launch, an absent library is created with one empty workspace. Add a
tab and choose **Open terminal** or **Attach a session** to start working; no other
setup is required. Existing libraries are opened without
initialization or repair. An unreadable or invalid library shows an error and
can be replaced through **Open Library…**. Keep its database and journal files
for recovery; the app never overwrites them with an empty arrangement.
`--help` and `--version` work without a display or tmux.

## Linux keyboard shortcuts

Linux uses the same commands and shortcut editor, with these defaults. Ordinary
Control keys such as Ctrl-C, Ctrl-D and Ctrl-R still reach the terminal.

| Command | Linux default |
|---|---|
| New tab | Ctrl+Shift+T |
| Split right / below | Ctrl+Shift+D / Ctrl+Shift+E |
| Attach session | Ctrl+Shift+A |
| Rename tab | Ctrl+Shift+R |
| Previous / next pane | Ctrl+Shift+[ / Ctrl+Shift+] |
| Previous / next tab | Ctrl+Page Up / Ctrl+Page Down |
| Expand or restore pane | Ctrl+Shift+Enter |
| New / rename workspace | Ctrl+Alt+Shift+N / Ctrl+Alt+Shift+R |
| Previous / next workspace | Ctrl+Shift+Page Up / Ctrl+Shift+Page Down |
| Focus panel / Jump | Ctrl+Shift+B / Ctrl+Shift+K |
| Select tab / workspace 1–9 | Alt+1–9 / Ctrl+Alt+1–9 |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V |
| Close pane / close tab | Ctrl+Shift+W / Ctrl+Alt+Shift+W |
| Detach pane contents | Ctrl+Shift+X |
| New window | Ctrl+Shift+N |
| Open library / Settings / Quit | Ctrl+Shift+O / Ctrl+, / Ctrl+Shift+Q |

The defaults avoid Super keys and Ctrl+Alt+arrow keys commonly used by the desktop.
A customized desktop may reserve other combinations; use Settings to change the
configurable commands. Saved shortcut strings keep their literal meaning:
`cmd` means Command on macOS and Super on Linux. macOS defaults are unchanged.

## Inspect a legacy keymap

After the [source build setup](../CONTRIBUTING.md#build-and-test), print the effective
legacy keymap or a complete compatible TOML configuration:

```sh
cargo run -p agent-interface-app -- --print-keymap help
cargo run -p agent-interface-app -- --keymap ./keymap.toml --print-keymap toml
cargo run -p agent-interface-app -- --no-keymap --print-keymap toml
```

Selection follows `--keymap`, then `AGENT_INTERFACE_KEYMAP`, then
`$XDG_CONFIG_HOME/agent-interface/keymap.toml` (or
`~/.config/agent-interface/keymap.toml`). `--no-keymap` uses shipped defaults.
Only a missing implicit default is ignored; invalid or explicitly missing files
report an error. These commands create no library, start no terminal and write
no configuration. These options inspect the Python-compatible `keymap.toml`;
they do not configure the native desktop. Use **Settings → Shortcuts** for native
bindings, which are saved separately in `settings.toml`. The legacy prefix is not
dispatched by the desktop.


## Current plans

**Reports & updates → Current plans** shows one card per agent: what its
current plan undertakes, each step as done, now, waiting or still ahead, and
what the waiting steps wait on. Agents with something waiting come first; an
agent that has not published a plan says so. Plans are what each agent
declares it is doing, kept current by the agent; the view only reads them and
rereads them every 20 seconds while open. A plan not updated for a day is
marked stale, which does not mean it is complete. The section needs an Agent
Backbone that offers current plans; an older one says it does not.

## Sheets

Use Sheets for research comparisons, budgets, opportunity trackers and test results.
Agents create and update the workbooks through Agent Backbone. In the app, readers
inspect the results and send feedback. Documents hold the longer explanation,
Reports record progress, and Responses answer specific questions; link to the
workbook from those artifacts so readers can inspect the underlying data.

Open **Reports & updates → Sheets**, choose a workbook, then a tab. Agent Backbone
must support Sheets and be configured through its normal private local connection.
The grid is read-only. Select a cell or use Shift-click or Shift-arrow to select a
range. Arrow keys and Tab move the selection. Row and column paging keep large
workbooks bounded; labels retain canonical A1 coordinates under saved views.

The workbook's conversation with its author sits on the right, as it does
beside Documents and Reports. Open it, write your message and press **⌘↵**.
The composer shows the cells it refers to, such as `Runs!C4`. Each message includes the
displayed revision and selected cells (up to 100); without a selection it
refers to the tab and whole workbook revision. Closing the conversation keeps
the draft. Send or clear the draft before changing the workbook, revision, tab
or page. After an uncertain submission, **Retry the same request** checks the
original operation without creating another feedback message. A message that
was not sent offers **Edit**, so you can resend it with the current selection.
Each workbook keeps its own conversation. Delivery status
does not mean the agent has
answered. **Copy feedback receipt** preserves the identifiers for checking delivery.

**History** opens earlier retained revisions. **Refresh** loads the latest revision.
**Copy values** copies selected cells as tab-separated text. Text values that could
be interpreted as formulas receive a leading apostrophe for pasting into a spreadsheet;
numeric values are preserved. **Export** saves the displayed revision. JSON/XLSX preserve supported workbook features, while CSV/TSV
export one tab as values only. Read any export warnings. This is not a claim of full
Excel or Google Sheets compatibility. Cell editing, workbook creation and import
are managed by agents, with no editing controls in this view.
