# Use tmux-workspaces

Install a native package using the [installation guide](INSTALL.md), then open
the app. Packages need no Rust, Zig or Python. Terminal sessions require tmux
3.3 or newer; Agent Backbone is optional. To build from source, follow
[Contributing](../CONTRIBUTING.md#set-up).

If no library exists, the first launch creates one empty workspace. Add a tab
with **+**, Command-T on macOS, or Ctrl+Shift+T on Linux, then choose **Open
terminal** or **Attach a session**. Opening an empty workspace does not start a
shell. To open another saved library, open Settings, choose **Open another**,
and select its folder. The shortcut is Command-O on macOS or Ctrl+Shift+O on Linux.

Sessions run on the same computer as the app. Connecting to another host or
synchronizing live sessions between computers is not supported.

## Navigate and use terminals

Use the workspace heading to choose a workspace, then choose a tab. The saved
split layout contains actual sessions; click a terminal before typing. Command-1
through Command-9 select tabs, and Command-Option-1 through Command-Option-9 select
workspaces. Command-[ and Command-] cycle panes; add Shift to cycle tabs.
Command-Option-Left/Right cycle workspaces, Command-Shift-Enter expands or
restores the selected pane, and Command-B focuses the sidebar. Navigation focuses
an already connected destination; if it is still connecting, click it when ready.
Command-V pastes clipboard text (Control-Shift-V on Linux), using
bracketed paste when the pane requests it. Keys follow the destination pane's
application cursor and extended-key modes. Paste is limited to 4 MiB per operation.
**Focus one pane** in tab options toggles the selected pane's view. Scroll over a
terminal to read its retained history. Shift-Page Up/Down moves a page;
Shift-Home/End goes to the oldest output or the live screen. Typing or pasting
returns to live output. Drag to select visible text; double-click selects a word
and triple-click selects a row. Command-C copies it (Control-Shift-C on Linux).
Selection keeps its displayed text while new output arrives. Escape, typing or
a new click outside the selection resumes the updated view. Copies from recovered
screens currently retain their displayed row breaks, including wrapped lines.
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

## Find work and change appearance

The window follows the Quiet Instrument design ([design language](DESIGN.md)).
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

Settings (Command-comma) offers a dark or light theme, the terminal text size,
whether panes show headers (always, on hover or never) and editable shortcuts;
changes apply to every window at once and are saved in
`$XDG_CONFIG_HOME/tmux-workspaces/settings.toml`, or
`~/.config/tmux-workspaces/settings.toml` when that variable is unset.
**Close Window** (Command-W) detaches that window's clients. Ordinary shells
keep running; external sessions retain their own tmux lifetime policy.

## Organize workspaces and tabs

Open the workspace menu by clicking or right-clicking the workspace block.
Choose **New workspace**, or press Command-Shift-N, and enter a name. Enter
saves it; Escape cancels. New workspaces have no tabs or shells.
Choose **Delete workspace** to remove an empty workspace; the last workspace and
workspaces with tabs cannot be deleted. Deleting selects the first remaining
workspace and leaves its tabs and sessions intact.

Click **+** beside TABS or press Command-T to add an empty tab. Command-D splits
right; Command-Shift-D splits below. Both split actions are also in tab options.
The existing pane remains intact and the new pane asks what it should run: Open
terminal, or Attach a session. New tabs
use the launch directory; splits inherit the ordinary shell’s remembered directory.
The new arrangement appears after saving. Navigation made while saving stays local;
otherwise the new tab or pane is selected.

Drag the divider between native panes to adjust their relative sizes. Release
saves the ratio; Escape or switching to another window during a drag cancels it.
This declares the native viewport's size to tmux. Its `window-size` policy
can change the shared window geometry seen by other clients. A changed tab or
split tree cancels the gesture. Save errors offer the usual Retry or Discard.

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

Missing sessions show an error with **Reconnect** instead of starting a shell.
Changing tabs or closing the app detaches its clients; the source’s existing
session-lifetime policy still applies. Connections preserve
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
prefix bindings, selection across history pages, terminal input methods
and key-release events are not yet wired into the product UI.

## Choose and recover a library

Omit `--data-dir` to use `TMUX_WORKSPACES_DATA_DIR`, then
`$XDG_DATA_HOME/tmux-workspaces`, or `~/.local/share/tmux-workspaces` by default.
On first launch, an absent library is created with one empty workspace. Add a
tab and choose **Open terminal** or **Attach a session** to start working; neither
Python nor Agent Backbone is required. Existing libraries are opened without
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
| New / close window | Ctrl+Shift+N / Ctrl+Shift+W |
| Open library / Settings / Quit | Ctrl+Shift+O / Ctrl+, / Ctrl+Shift+Q |

The defaults avoid Super keys and Ctrl+Alt+arrow keys commonly used by the desktop.
A customized desktop may reserve other combinations; use Settings to change the
configurable commands. Saved shortcut strings keep their literal meaning:
`cmd` means Command on macOS and Super on Linux. macOS defaults are unchanged.

## Inspect a legacy keymap

After the [source build setup](../CONTRIBUTING.md#build-and-test), print the effective
legacy keymap or a complete compatible TOML configuration:

```sh
cargo run -p tmux-workspaces-app -- --print-keymap help
cargo run -p tmux-workspaces-app -- --keymap ./keymap.toml --print-keymap toml
cargo run -p tmux-workspaces-app -- --no-keymap --print-keymap toml
```

Selection follows `--keymap`, then `TMUX_WORKSPACES_KEYMAP`, then
`$XDG_CONFIG_HOME/tmux-workspaces/keymap.toml` (or
`~/.config/tmux-workspaces/keymap.toml`). `--no-keymap` uses shipped defaults.
Only a missing implicit default is ignored; invalid or explicitly missing files
report an error. These commands create no library, start no terminal and write
no configuration. These options inspect the Python-compatible `keymap.toml`;
they do not configure the native desktop. Use **Settings → Shortcuts** for native
bindings, which are saved separately in `settings.toml`. The legacy prefix is not
dispatched by the desktop.
