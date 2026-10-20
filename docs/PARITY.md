# Parity checklist

This file lists the user-visible behaviour of the Python tool
[tmux-workspaces](https://github.com/eandualem/tmux-workspaces) and says, for each
item, what this Rust app does with it. Parity with the Python tool comes first;
a better UX comes after. This file is the parity checklist.

The rows come from the Python tool's documentation at commit
[`51de675`](https://github.com/eandualem/tmux-workspaces/tree/51de6757cd0aeda3c19fe68fa4ec5b93ab206804):
the README, the files under `docs/`, the CHANGELOG and `integrations/keymap.toml`.
Every row links to the section it comes from. When the Python tool changes,
check the rows against a newer commit and move the pin.

## How the Rust app differs, in short

The Python tool is a curses program that runs inside a terminal. Each of its
windows starts a private tmux server (the "display server") and shows every
pane through a nested `tmux attach` client. Its own keys travel through a tmux
prefix table and a private socket.

The Rust app is a native desktop window. It is a tmux client and runs no daemon.
It talks to tmux in control mode (`tmux -C`), a text protocol in which tmux
reports pane output and events to a program instead of drawing a screen. The app
draws every terminal pane itself on the GPU, handles keys and mouse natively,
and keeps each pane's terminal state in memory. It reads the same library
(`layouts.db`, schema version 2) and attaches to the same shell server as the
Python tool, so one library opens in both. It writes that library only after it
implements the same merge and conflict rules.

## Words used here

- **Library**: the directory that holds `layouts.db`, the saved arrangement of
  workspaces, tabs and splits.
- **Leaf** or **pane**: one terminal area in a tab's split layout. Each leaf has a
  stable 12-character hexadecimal identity.
- **Ordinary shell**: a login shell the tool owns. It runs as a tmux session on
  the library's own tmux server, called the **shell server**.
- **Source server**: the tmux server whose sessions the attachment chooser lists.
- **Attachment**: a leaf that shows an existing tmux session instead of its
  ordinary shell. The shell keeps running in the background; it is **parked**.
- **Viewer**: the Python tool's name for one of its running windows.
- **Prefix**: a key you press and release before an action key, like tmux's
  `Ctrl-b`. The Python tool's default prefix is `Ctrl-g`.
- **Direct shortcut**: a one-press key such as `Command-T`.

## How to read the tables

Each table has these columns:

- **#**: an area prefix and a number, such as `WS-1`. Numbers are stable; add new
  rows at the end of a table.
- **Behaviour (Python tool)**: one concrete, testable sentence.
- **Source**: the Python documentation section it comes from.
- **Decision**: one of the four values below.
- **Notes**: why and how, for anything other than a plain Keep.
- **Status**: `Not started`, `In progress`, `Done` (with the pull request) or
  `Won't do`.

Decision values:

- **Keep**: the Rust app behaves the same way.
- **Change**: same user goal, different mechanism or UX. The Notes say how.
- **Drop**: the behaviour exists only because of the nested-tmux and curses
  design, or it is out of scope. The Notes say why.
- **Open**: needs a decision. The Notes say what. Every Open row is repeated in
  [Open questions](#open-questions).

## Workspaces and tabs

The core model supports in-memory creation, splitting, attachment changes and
closing records. Product controls, persistence and shell lifecycle integration
remain pending; model operations alone do not complete these workflows.

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| WS-1 | A new library starts with one workspace, **Workspace 1**, holding one tab with a random name such as **Tab a82f** and one ordinary shell. | [README][rd-first], [GUIDE][g-organize] | Keep | | In progress |
| WS-2 | Tabs are only created by the user. Nothing is populated from the agent roster, and the session list is only an attachment chooser, never a source of tabs. | [GUIDE][g-organize], [GUIDE][g-attach] | Keep | | In progress |
| WS-3 | A new workspace starts empty, with no tabs; the user adds its first tab with new tab. | [GUIDE][g-organize], [SHORTCUTS][sc-ws] | Keep | | In progress |
| WS-4 | Upgrading keeps existing saved names, tabs and attachments; the tool never deletes earlier arrangements. | [GUIDE][g-organize] | Keep | Also applies between the two tools: opening a library in the Rust app must not drop anything the Python tool saved. | Not started |
| WS-5 | The **+** on the TABS row and the new-tab shortcut create a tab whose only pane starts as the new-pane chooser (see CH-1). | [GUIDE][g-organize], [README][rd-first] | Keep | | In progress |
| WS-6 | Names belong to the user: attaching a session never renames a tab, and renaming a tab or workspace never renames an attached session or ends a shell. | [GUIDE][g-organize], [INLINE_RENAME][ir-use] | Keep | | In progress: model renaming and name normalization implemented; native editing and persistence pending. |
| WS-7 | **Rename tab** is the first item in the tab menu. The rename form saves with **Save name** or Enter, and `Ctrl-u` clears the field. | [GUIDE][g-organize], [SHORTCUTS][sc-right] | Keep | | Not started |
| WS-8 | Rename fields open with the existing name selected, so typing replaces it. | [GUIDE][g-keys], [SHORTCUTS][sc-right] | Keep | | Not started |
| WS-9 | Double-clicking the active tab's name, or the workspace name at the top, edits it in place. Enter saves; Escape or a click elsewhere cancels. | [INLINE_RENAME][ir-use], [GUIDE][g-organize] | Keep | | In progress: model inline-rename guard implemented; native editor and gestures pending. |
| WS-10 | A double-click counts only when both clicks land within 450 ms and near the same character. Double-clicking an inactive tab only selects it; a second double-click on its name edits it. | [INLINE_RENAME][ir-use] | Change | Use the operating system's double-click interval instead of a fixed 450 ms. Keep the select-first rule for inactive tabs. | Not started |
| WS-11 | Counts and the workspace icon or number slots keep their normal click behaviour; only names start an inline edit. | [INLINE_RENAME][ir-use] | Keep | | Not started |
| WS-12 | The inline editor supports Left/Right, Home/End, Backspace, Delete and click-to-place-cursor; long names scroll horizontally. | [INLINE_RENAME][ir-use] | Keep | A native text field. | Not started |
| WS-13 | Tab and workspace names are limited to 80 characters. | [INLINE_RENAME][ir-use] | Keep | Keeps names valid in both tools. | In progress: model renaming and name normalization implemented; native editing and persistence pending. |
| WS-14 | An empty name cannot be saved; the editor stays open until a name is entered or the edit is cancelled. | [INLINE_RENAME][ir-use] | Keep | | In progress: model renaming and name normalization implemented; native editing and persistence pending. |
| WS-15 | A workspace can be renamed even when it has no tabs. | [INLINE_RENAME][ir-use] | Keep | | In progress: model renaming and name normalization implemented; native editing and persistence pending. |
| WS-16 | The **▾** beside the workspace name, or a right-click on the name, opens the workspace chooser and options: switch, create, rename, set an icon, or delete an empty workspace. | [GUIDE][g-ws], [SHORTCUTS][sc-right] | Keep | How the menu is presented is VL-10. | Not started |
| WS-17 | A workspace that still has tabs cannot be deleted; its **Delete** row says so. | [SHORTCUTS][sc-cover] | Keep | | In progress: model navigation, ordering, transfer and deletion rules implemented; product controls pending. |
| WS-18 | The icon row at the bottom of the panel switches workspace with one click. Each workspace has a three-cell slot showing its icon, or its number when it has none; the current one is filled. | [GUIDE][g-ws], [UI_PREVIEW][up-org] | Keep | Slots are sized natively instead of in terminal cells. | Not started |
| WS-19 | Right-clicking a workspace slot opens that workspace's options. When the row is full, its last slot, **…**, opens the full list. | [GUIDE][g-ws], [SHORTCUTS][sc-right] | Keep | | Not started |
| WS-20 | **Set icon…** offers a small set of glyphs that render one cell wide in the usual terminal fonts; **Number** removes the icon; the choice is saved with the workspace. | [GUIDE][g-ws] | Keep | Offer the same glyph set, so an icon chosen in one tool draws correctly in the other. | Not started |
| WS-21 | Previous and next workspace shortcuts cycle workspaces; numbered shortcuts select workspace 1 to 9. | [GUIDE][g-ws], [SHORTCUTS][sc-ws] | Keep | Keys are KB-17, KB-18 and KB-27. | In progress: model navigation, ordering, transfer and deletion rules implemented; product controls pending. |
| WS-22 | Next and previous tab shortcuts switch complete layouts; numbered shortcuts select tab 1 to 9 in the current workspace. | [GUIDE][g-split], [SHORTCUTS][sc-tabs] | Keep | Keys are KB-8, KB-9 and KB-26. | In progress: model navigation, ordering, transfer and deletion rules implemented; product controls pending. |
| WS-23 | Out-of-range numbered shortcuts do nothing. | [SHORTCUTS][sc-ws] | Keep | | Not started |
| WS-24 | The tab menu opens from the **⋯** at the right end of the selected tab, from a right-click on any tab (even an inactive one), or from the tab-options shortcut. | [GUIDE][g-split], [SHORTCUTS][sc-right] | Keep | | Not started |
| WS-25 | The tab menu holds Rename tab, Split right, Split below, Focus one pane or Restore layout, pane cycling, Attach session, Return to shell, reordering, moving to another workspace, and closing panes and tabs. | [SHORTCUTS][sc-right], [GUIDE][g-split], [GUIDE][g-attach] | Keep | | Not started |
| WS-26 | The tab menu reorders tabs and moves a tab to another workspace; these need no shortcut of their own. | [GUIDE][g-ws], [SHORTCUTS][sc-cover] | Keep | The menu path stays even if drag and drop is added later. | In progress: model navigation, ordering, transfer and deletion rules implemented; product controls pending. |
| WS-27 | **Close tab** closes the whole tab and ends its ordinary shells; attached external sessions keep running. | [GUIDE][g-settings], [SHORTCUTS][sc-tabs] | Keep | Keys are KB-20. Process rules are SH-12. | In progress |
| WS-28 | Every new tab gets a neutral random name, **Tab** followed by four hexadecimal characters, unless the user supplies one. | [INITIAL_TASK][it-req], [ACCEPTANCE][ac-checks] | Keep | | In progress |

## Splits, focus and layout

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| SP-1 | **Split right** adds a pane beside the selected one, with a vertical divider. **Split below** adds one under it, with a horizontal divider. | [GUIDE][g-split], [SHORTCUTS][sc-tabs] | Keep | Keys are KB-3 and KB-4. | In progress |
| SP-2 | Both splits are in the tab menu and on shortcuts. | [GUIDE][g-split] | Keep | | Not started |
| SP-3 | A new split opens as the new-pane chooser. A terminal chosen there starts in the working directory of the shell it was split from. | [GUIDE][g-split], [README][rd-first], [ARCHITECTURE][ar-modules] | Keep | Read the source pane's current directory from tmux on the shell server. | Not started |
| SP-4 | One tab can hold several panes; a four-pane tab is a normal, tested case. | [GUIDE][g-split], [ACCEPTANCE][ac-checks] | Keep | | In progress: pure cell geometry and focused-pane fallback implemented; product rendering pending. |
| SP-5 | Clicking a pane selects it, and typing goes there. | [GUIDE][g-split] | Keep | | In progress: model pane selection and measured split ratios implemented; native controls and input handoff pending. |
| SP-6 | Dragging the separator between two panes resizes the split; the navigation panel keeps its width. | [GUIDE][g-split], [GUIDE][g-resize] | Keep | | In progress: model pane selection and measured split ratios implemented; native controls and input handoff pending. |
| SP-7 | Next and previous pane shortcuts move input focus between the panes of the selected tab. | [GUIDE][g-split] | Keep | Keys are KB-10 and KB-11. | Not started |
| SP-8 | **Focus one pane** temporarily shows only the selected pane; **Restore layout** shows the splits again. The saved arrangement does not change. | [GUIDE][g-split], [SHORTCUTS][sc-tabs], [UI_PREVIEW][up-try2] | Keep | Key is KB-12. Focus mode is per window (PE-19). | In progress: pure cell geometry and focused-pane fallback implemented; product rendering pending. |
| SP-9 | When the complete layout does not fit the window, only the focused pane is shown for the moment, and the saved split tree is kept. | [GUIDE][g-split], [GUIDE][g-resize], [ACCEPTANCE][ac-limits] | Keep | | In progress: pure cell geometry and focused-pane fallback implemented; product rendering pending. |
| SP-10 | Saved split proportions are limited by the space each nested layout needs. | [GUIDE][g-resize] | Keep | | Not started |
| SP-11 | Split proportions survive closing and reopening. | [ACCEPTANCE][ac-checks] | Keep | | Not started |
| SP-12 | **Close pane** closes the selected pane and ends its ordinary shell; closing a tab's last pane closes the tab. | [SHORTCUTS][sc-tabs], [GUIDE][g-settings] | Keep | Keys are KB-19. | Not started |
| SP-13 | A layout change recreates attachment clients and can cause a brief redraw. | [GUIDE][g-limits], [ACCEPTANCE][ac-limits] | Drop | A limit of nested tmux clients. The app keeps each leaf's terminal state in memory, so a layout change only re-lays out and redraws. | Not started |
| SP-14 | Switching tabs restarts each visible pane's attachment client, reusing pane containers when the geometry still matches. | [ARCHITECTURE][ar-nav] | Drop | Mechanics of the private display server. Switching tabs in the app changes what is drawn and starts no process. | Not started |
| SP-15 | A second tmux layer affects sizing, scrolling and repaint. | [ACCEPTANCE][ac-limits], [GUIDE][g-limits] | Drop | There is no second tmux layer. | Not started |
| SP-16 | A writable attachment takes part in tmux window sizing, so another window showing the same terminal can change its size. | [GUIDE][g-limits], [ACCEPTANCE][ac-limits] | Keep | This is how tmux works: a control-mode client also takes part in sizing. Document it. | Not started |

## Choosers and attaching existing sessions

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| CH-1 | An empty pane asks what it should run: **Open terminal** for a login shell, or any tmux session from the attachment roster, with its state beside it. | [GUIDE][g-organize], [CHANGELOG][cl] | Keep | The chooser is drawn natively inside the pane. | Not started |
| CH-2 | Nothing is created until the user chooses. An empty pane stays empty across exit and reopen until it is filled. | [GUIDE][g-organize] | Keep | Store an empty leaf exactly as the Python tool does. | In progress: read-only empty-leaf parsing and guarded model fill implemented; product chooser, persistence and shell creation pending. |
| CH-3 | **Open terminal** starts, at first, in the directory where the viewer was launched. | [GUIDE][g-organize] | Open | An app started from the Dock or a desktop launcher has no useful working directory. Decide the default (for example the home directory) and whether a command-line launch keeps the invoking directory. | Not started |
| CH-4 | In a chooser, the arrow keys and Enter, or a click on a row, pick an entry. | [GUIDE][g-organize] | Keep | Full menu keys are IN-4. | Not started |
| CH-5 | Typing filters the attachment chooser by session name, ignoring case. Editing the filter selects the first match; with no match, Enter does nothing; `Ctrl-u` clears the filter. | [SHORTCUTS][sc-cover], [GUIDE][g-keys] | Keep | | Not started |
| CH-6 | The chooser follows the theme: title in `accent`, hints in `muted`, the selected row in `active`, agent states in the accent, danger and muted colours, all on the `surface`. | [THEMES][th-beyond] | Keep | | Not started |
| CH-7 | **Attach session** attaches a session to a pane that already has a shell. The pane's shell keeps running in the background (parked). | [GUIDE][g-attach], [SHORTCUTS][sc-ws] | Keep | Keys are KB-5. | Not started |
| CH-8 | **Return to shell** in the tab menu brings the parked shell back. | [GUIDE][g-attach], [SHORTCUTS][sc-ws] | Keep | | Not started |
| CH-9 | Each split can attach a different session or stay an ordinary terminal. | [GUIDE][g-attach] | Keep | | Not started |
| CH-10 | Once open, the attachment chooser keeps its destination pane even if focus moves to another pane. | [GUIDE][g-resize] | Keep | | In progress: model destination guard implemented; native chooser, shared refresh and persistence integration pending. |
| CH-11 | If another window removes or changes the chooser's destination, the user attaches again from the updated pane; the tool does not overwrite the other window's change. | [GUIDE][g-resize] | Keep | | In progress: model destination guard implemented; native chooser, shared refresh and persistence integration pending. |
| CH-12 | An offline attachment stays with its pane and reconnects when the session returns. The tool never starts an external session. | [GUIDE][g-attach], [ACCEPTANCE][ac-checks] | Keep | | Not started |
| CH-13 | The tool never moves, renames, restarts or kills external sessions, and never sends them programmatic messages. | [GUIDE][g-iso] | Keep | The app forwards only what the user types or clicks. | Not started |
| CH-14 | An attached session is joined through a grouped tmux session owned by the tool. The group shares the session's windows but has its own options and its own current window. | [GUIDE][g-attach] | Keep | Control mode still needs the group, so the app can pick a window without moving other clients. | Not started |
| CH-15 | The grouped session starts with the attached session's settings, mouse mode included. | [GUIDE][g-attach] | Change | tmux's mouse handling does not apply to a control-mode client. The app tracks each pane's mouse-reporting mode and forwards mouse events itself. | Not started |
| CH-16 | The grouped session replaces its status line with one row along the top of the pane, showing only the session's name, on the right, in the theme's panel and accent colours. | [GUIDE][g-attach] | Change | A control-mode client draws no tmux status line. The app draws its own pane header with the session name in the same colours. | Not started |
| CH-17 | The attached session keeps its own status line and every other setting; closing a viewer preserves external sessions and their options. | [GUIDE][g-attach], [ACCEPTANCE][ac-checks] | Keep | | Not started |
| CH-18 | The grouped session is removed when the pane lets go of it, or as soon as the session it joined ends, so an ended session shows as offline instead of living on. | [GUIDE][g-attach] | Keep | A tmux group keeps shared windows alive while any member lives, so the app must remove its helper when the source ends. | Not started |
| CH-19 | An attachment starts on the source session's current window. A window selected inside it stays selected through resizing and tab switches, independent of the source. Reopening starts from the source's current window again. | [GUIDE][g-attach] | Keep | | Not started |
| CH-20 | A source with an automatic `destroy-unattached` policy is attached directly instead, because adding a group could delete it. A direct attachment shares the source's current window and status line, and the source's cleanup policy still applies when its last client detaches. | [GUIDE][g-attach], [ACCEPTANCE][ac-checks] | Keep | A direct control-mode client counts as an attached client for that policy. No status line is drawn (CH-16). | Not started |
| CH-21 | Grouped helper sessions carry the tmux session option `@tmux_workspaces_attachment=1` and are hidden from the attachment chooser. | [GUIDE][g-helper] | Keep | Use the same option name and value, so each tool hides the other's helpers. | Not started |
| CH-22 | Ordinary sessions are never hidden only because their names start with `tw-`. Unmarked helpers from older versions stay visible until their attachment closes. Discovery never migrates or stops sessions. | [GUIDE][g-helper] | Keep | | Not started |
| CH-23 | The panel never names the session attached to the focused pane; the user names the tab for that. | [GUIDE][g-bb] | Keep | The pane header from CH-16 shows the session name. | Not started |
| CH-24 | Session names may contain spaces, Unicode and punctuation. Display text is kept separate from exact tmux targets. | [GUIDE][g-iso], [ACCEPTANCE][ac-checks] | Keep | Always target sessions by exact name, never by prefix match. | Not started |
| CH-25 | A normal launch lists sessions from one source server: the default socket outside tmux, or the invoking server when started inside tmux. | [GUIDE][g-iso] | Keep | A Dock launch has no invoking server and uses the default socket. | Not started |
| CH-26 | `--source-socket PATH` selects another source server. | [GUIDE][g-iso] | Keep | | Not started |
| CH-27 | Each new attachment saves its source socket. If a later launch uses a different source server, the pane still connects to its saved server, and the panel labels it "saved server". | [GUIDE][g-iso] | Keep | | Not started |
| CH-28 | Older version-2 references without a saved socket use the launcher's source socket. | [GUIDE][g-iso] | Keep | | Not started |
| CH-29 | A missing source server counts as an empty, successful session list, so a fresh install needs no external sessions. A failed tmux listing marks sessions unavailable instead of keeping stale "online" claims. | [ARCHITECTURE][ar-provider] | Keep | Control-mode notifications can replace some periodic listing. | Not started |
| CH-30 | When the viewer runs inside tmux, the session hosting it cannot be attached inside itself; the pane explains this and offers to return to its shell. | [GUIDE][g-limits], [GUIDE][g-tpm], [ACCEPTANCE][ac-checks] | Drop | The app draws in its own native window and is never hosted inside a tmux pane, so this loop cannot happen. | Not started |
| CH-31 | **Attach session** and the attach shortcut work on every supported tmux version. | [GUIDE][g-resize], [SHORTCUTS][sc-right] | Keep | | Not started |
| CH-32 | Inside an attachment, the usual `Ctrl-b` prefix reaches the inner tmux client. | [GUIDE][g-keys] | Open | In control mode there is no inner tmux client to read the attached session's prefix keys. Decide whether the app sends such keys literally, imitates common tmux bindings, or offers its own window controls for attached sessions. Evidence: [tmux control mode research](research/tmux-control-mode.md#input-prefix-keys-mouse-and-history-ownership). | Not started |
| CH-33 | An attached session is shown through a full nested tmux client, so tmux draws that session's windows, panes and status line inside the leaf. | [GUIDE][g-iso], [GUIDE][g-attach] | Open | The app must draw the attached session's current window, including that window's own panes, inside one leaf. Decide how those inner panes look and how the user switches the session's windows. Evidence: [tmux control mode research](research/tmux-control-mode.md#decisions-that-materially-change-the-architecture). | Not started |
| CH-34 | Scrolling and rendering inside an attachment depend on the outer terminal and the inner session's mouse settings. | [GUIDE][g-limits] | Change | The app owns rendering and scrollback for every pane (IN-25). | Not started |

## Persistence, concurrent windows and conflict handling

The read-only model/store implements the schema checks below and exposes a
change token. App integration, local navigation and database writes remain
pending; these rows do not yet describe a usable app workflow.

Data compatibility lives here. The Rust app reads the same library as the Python
tool. It writes that library only after PE-15 is in place.

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| PE-1 | The library directory is chosen in this order: `--data-dir`, then `$TMUX_WORKSPACES_DATA_DIR`, then `$XDG_DATA_HOME/tmux-workspaces`, then `~/.local/share/tmux-workspaces`. | [GUIDE][g-iso], [NAMING][nm-compat], [PACKAGING][pk-layout] | Keep | Use the same order on macOS too (not `~/Library/Application Support`), so both tools open the same library. | In progress: read-only path selection and expansion implemented; product CLI/window integration pending. |
| PE-2 | Workspaces, ordered tabs, terminal identities and split layouts live in `layouts.db`, a SQLite database with schema version 2. | [GUIDE][g-iso], [ARCHITECTURE][ar-modules] | Keep | Read support first. Write support waits for PE-15. | In progress |
| PE-3 | The current arrangement is in the `terminal_layout` table. The earlier `layout` and `shared_layout` tables are kept with their exact JSON text and are never used as a silent fallback. | [RECOVERY][rc-copy], [GUIDE][g-iso] | Keep | | In progress |
| PE-4 | Older layouts are imported once into the terminal library in the same database. Old viewers cannot overwrite the new library, and later edits made in an old version are not synchronized. | [GUIDE][g-iso] | Open | Decide whether the Rust app performs this one-time import itself, or asks the user to open the library once with the Python tool first. | Not started |
| PE-5 | A library written by an unsupported future version is refused. The version number must not be edited to get past validation. | [RECOVERY][rc-copy] | Keep | | In progress |
| PE-6 | The version-2 schema and the historical `agent` attachment field stay unchanged. Updating the code does not migrate libraries or restart running terminals. | [ARCHITECTURE][ar-life], [ARCHITECTURE][ar-provider] | Keep | | In progress |
| PE-7 | Unknown extension fields are preserved while they stay within the limits in PE-11. | [RECOVERY][rc-valid] | Keep | Lets each tool round-trip fields the other adds. Any field the Rust app adds must respect the same limits. | In progress: model edits project into validated documents while retaining extensions; database writes remain pending. |
| PE-8 | Validation checks required workspace, tab and leaf fields; globally unique 12-character hexadecimal identities; names; selected identities; exact session references; and absolute cwd and socket paths. | [RECOVERY][rc-valid], [ARCHITECTURE][ar-modules] | Keep | | In progress |
| PE-9 | A missing cwd or source socket is accepted for older migrated leaves. Paths may contain spaces or newlines. NUL and paths the filesystem cannot represent are rejected. A socket reference requires an attachment and does not prove the session is online. | [RECOVERY][rc-valid] | Keep | | In progress |
| PE-10 | A split needs both children and a `right` or `below` direction. An omitted ratio means an equal split. An explicit ratio must be finite and between 0.01 and 0.99. | [RECOVERY][rc-valid] | Keep | | In progress |
| PE-11 | Limits: tree depth 64, data depth 128, 10000 containers or identified objects, 100000 data values and 16 MiB of JSON text. The size limit is also checked before saving or migrating, so a write cannot create a record the next read refuses. | [RECOVERY][rc-valid] | Keep | | In progress |
| PE-12 | Duplicate JSON keys, non-finite numbers, invalid JSON and malformed SQLite schemas are rejected without rewriting any record. | [RECOVERY][rc-valid] | Keep | | In progress |
| PE-13 | `layouts.db-wal` and `layouts.db-shm` may sit beside the database and hold committed data that has not reached the main file yet. | [RECOVERY][rc-keep] | Keep | The Python tool does not enable write-ahead logging; it uses SQLite's default rollback journal. If these files exist, let SQLite use them, never change the journal mode and never delete them. Keep read transactions short so the Python tool's five-second busy timeout is not exhausted ([research](research/rust-architecture.md#sqlite-and-python-compatibility)). | In progress |
| PE-14 | Ordinary shells run on a persistent shell server that belongs to the library. Leaves have stable identities, and opening the same terminal in two windows attaches to the same running shell. | [GUIDE][g-iso], [NAMING][nm-compat] | Keep | From the Python source, not its docs: the shell server socket is `/tmp/tmux-workspaces-<uid>/<first 16 hex digits of the SHA-256 of the resolved library path>-terminals.sock`, and each ordinary shell is the session `terminal-<leaf id>` on it. Derive both the same way. | In progress: read-only shell socket and session identity lookup implemented; connection/lifecycle integration pending. |
| PE-15 | Concurrent edits to different items are merged. A conflicting edit to the same item is refreshed, and the user is asked to retry. | [GUIDE][g-iso], [ACCEPTANCE][ac-checks] | Keep | This three-way merge is the gate for writing `layouts.db`. | In progress: document reconciliation merges local edits or refreshes conflicts while retaining navigation; persistence and UI refresh pending. |
| PE-16 | If another window changes the active pane's layout or attachment, focus moves to the navigation panel, so typing cannot land in a different terminal. | [GUIDE][g-iso] | Keep | | In progress: model reconciliation reports active-tree changes; controller focus handoff pending. |
| PE-17 | A rename that conflicts with another window's rename or deletion is cancelled instead of overwriting it. | [INLINE_RENAME][ir-use] | Keep | | In progress: model inline-rename guards and document reconciliation implemented; native editor and persistence pending. |
| PE-18 | Shared names, tabs, arrangements and attachment availability refresh during normal use, without a restart. | [REFRESH][rf-reopen] | Keep | The Python docs do not give an interval. Watching the database file or polling are both possible. | In progress: read-only storage worker polls and publishes validated updates; controller/UI integration pending. |
| PE-19 | Each window keeps its own selected workspace, tab, pane, focus mode and dimensions. The library is shared, and closing one window leaves the others running. | [GUIDE][g-iso], [SHORTCUTS][sc-top] | Keep | Windows of one app process behave like separate viewers. | In progress: model retains local selection; app windows pending. |
| PE-20 | The last-used navigation seeds new windows. | [GUIDE][g-iso] | Keep | | In progress |
| PE-21 | The store holds one shared arrangement. Each running window keeps its own selected workspace, tab, pane and focus mode in memory, and the last-used navigation seeds new windows. The agents toggle is saved with the layout. | [ARCHITECTURE][ar-modules], [GUIDE][g-iso], [GUIDE][g-bb] | Open | Until write support lands, decide whether the app keeps navigation and edits in memory only, or disables editing. | Not started |
| PE-22 | The tool stores references, not transcripts or credentials. | [GUIDE][g-iso] | Keep | | Not started |
| PE-23 | No other checkout's layouts are imported by default; `--data-dir` selects a separate library. | [GUIDE][g-iso] | Keep | | Not started |
| PE-24 | Runtime manifests under `windows/<instance>/` in the library name each window's sockets. Closing a window cleans up only its own display server and action socket. | [GUIDE][g-iso] | Drop | The app has no display server or action socket. It must leave the Python tool's manifests alone. | Not started |
| PE-25 | There is no per-display arrangement storage. | [ACCEPTANCE][ac-limits] | Keep | | Not started |

## Shell environment and process lifetime

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| SH-1 | **Open terminal** starts a normal interactive login shell. The user's shell startup files run as in any terminal. | [SHELL_ENVIRONMENT][se-top], [GUIDE][g-iso] | Keep | | Not started |
| SH-2 | The shell gets only an explicit set of variables from the launcher: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `LANG`, `TMPDIR`, `COLORTERM`, `TERMINFO`, `TERMINFO_DIRS` and `LC_*`. tmux supplies the pane's terminal settings. | [SHELL_ENVIRONMENT][se-vars] | Open | An app started from the Dock gets a minimal environment, not the user's terminal environment. Decide how to obtain these values (for example from a login shell at startup) and what `TERM` and `COLORTERM` mean when there is no outer terminal. | Not started |
| SH-3 | Ordinary shells drop the private `TMUX` and `TMUX_PANE` values and any agent launch metadata, so tools such as Backbone's CLI use the normal tmux server. | [SHELL_ENVIRONMENT][se-vars], [GUIDE][g-iso] | Keep | | Not started |
| SH-4 | Each new shell receives `SSH_AUTH_SOCK`, `XDG_CONFIG_HOME`, `XDG_CONFIG_DIRS`, `XDG_DATA_HOME`, `XDG_DATA_DIRS`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` and `XDG_RUNTIME_DIR` from the window that creates it. Missing ones are explicitly unset, so the server cannot hand out an old agent socket. | [SHELL_ENVIRONMENT][se-vars], [SHELL_ENVIRONMENT][se-ssh] | Keep | | Not started |
| SH-5 | `SSH_AGENT_PID`, `SSH_CONNECTION`, desktop-session variables, other `XDG_*` names, unrelated launcher variables and API keys are never forwarded. | [SHELL_ENVIRONMENT][se-vars], [GUIDE][g-iso] | Keep | | Not started |
| SH-6 | Paths are passed as supplied, spaces included. No directories or agent sockets are created or migrated. | [SHELL_ENVIRONMENT][se-vars] | Keep | | Not started |
| SH-7 | Two windows can create shells with different SSH and XDG contexts without changing each other's shells. | [SHELL_ENVIRONMENT][se-ssh] | Keep | | Not started |
| SH-8 | A running shell keeps its environment, PID, working directory and foreground programs. The tool never rewrites shell profiles, injects exports, redirects sockets or reconnects an SSH agent. | [SHELL_ENVIRONMENT][se-ssh] | Keep | | Not started |
| SH-9 | Attaching an external session uses `tmux attach-session -E`, so the session keeps its own environment. | [SHELL_ENVIRONMENT][se-vars], [ARCHITECTURE][ar-modules] | Keep | Pass `-E` on the control-mode attach as well. | Not started |
| SH-10 | Control commands and session discovery run with the narrower environment, without the extra SSH and XDG paths. | [SHELL_ENVIRONMENT][se-vars] | Keep | | Not started |
| SH-11 | **Detach**, or closing the window, leaves saved tabs, ordinary shells, running programs and agents available for reopening. | [GUIDE][g-settings], [README][rd-first] | Keep | Detach closes this window. Quitting the app detaches every window. Keys are KB-25. | Not started |
| SH-12 | Explicitly closing a pane or tab ends its ordinary shells and their programs. Attached external sessions keep running. This holds even when another window shows the same tab. | [GUIDE][g-settings], [README][rd-get] | Keep | | Not started |
| SH-13 | Switching tabs, resizing and exiting never end a shell. Working directory, variables, foreground programs and scrollback survive reopening. | [GUIDE][g-iso], [ACCEPTANCE][ac-checks] | Keep | When the app reconnects to a pane, it rebuilds its scrollback from tmux's pane history. | Not started |
| SH-14 | Processes survive only while their tmux server is alive. After a reboot or the loss of the shell server, layouts and attachment references remain, and ordinary shells restart in their last saved directories. | [GUIDE][g-iso], [ACCEPTANCE][ac-limits], [REFRESH][rf-preserve] | Keep | | Not started |
| SH-15 | Each viewer window owns a private tmux display server, a runtime directory and an action socket, all destroyed when it closes. | [GUIDE][g-iso], [ARCHITECTURE][ar-life] | Drop | The app is a control-mode client. Apart from the library's shell server, it starts no tmux server of its own. | Not started |
| SH-16 | Closing a window never stops the library's shell server or any external tmux server; shared shell servers are never cleanup targets. | [ARCHITECTURE][ar-life], [STARTUP][su-term] | Keep | | Not started |
| SH-17 | Secrets in the launcher's environment are not exported into the tool's tmux servers. | [GUIDE][g-iso], [GUIDE][g-bb] | Keep | Applies to the shell server the app starts. | Not started |

## Agents roster

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| AG-1 | The roster is optional. Workspaces work fully without it, and it adds no notifications. | [README][rd-top], [GUIDE][g-top] | Keep | | Not started |
| AG-2 | The roster appears only when a state-reporting source is connected: Backbone, found on its own, or the demo. | [GUIDE][g-bb] | Keep | | Not started |
| AG-3 | The Backbone adapter turns on by itself when Backbone's data directory (`~/.local/share/agent-backbone`, or `$BACKBONE_DATA_DIR`) holds its database. `--no-backbone` keeps it off; `--backbone` asks for it. | [GUIDE][g-bb], [ARCHITECTURE][ar-modules] | Keep | | Not started |
| AG-4 | `--backbone-data-dir PATH` selects another installation. `--url http://127.0.0.1:PORT` sets the address and is required when it cannot be read from SQLite, including PostgreSQL setups. | [GUIDE][g-bb], [SHORTCUTS][sc-top] | Keep | | Not started |
| AG-5 | The adapter reads the API address from Backbone's settings database in read-only mode and the API key from Backbone's `.env`. It never changes those files. | [GUIDE][g-bb], [ARCHITECTURE][ar-provider] | Keep | | Not started |
| AG-6 | It polls the existing `/api/agents` endpoint every five seconds with loopback HTTP GET requests, without proxies or redirects, and uses Backbone's own state decisions. | [GUIDE][g-bb], [ARCHITECTURE][ar-provider] | Keep | | Not started |
| AG-7 | Unless Backbone is requested with `--backbone`, `--backbone-data-dir` or `--url`, a launch without Backbone's database (and not in demo mode) loads no Backbone provider, reads no Backbone configuration and calls no API. | [ARCHITECTURE][ar-provider], [ARCHITECTURE][ar-life], [ACCEPTANCE][ac-checks] | Keep | | Not started |
| AG-8 | One row per active agent: a state symbol in a fixed slot, then the name. Offline agents take no row. | [GUIDE][g-bb], [UI_PREVIEW][up-org] | Keep | | Not started |
| AG-9 | Symbols: `▶` working, `!` needs you (waiting for input, or blocked), `○` idle, `?` unknown. | [GUIDE][g-bb], [README][rd-top] | Keep | Keep the symbols with any colour, so a state never depends on colour alone. | Not started |
| AG-10 | Working agents use the `accent` colour; agents that need you use the `danger` colour. | [THEMES][th-roles] | Keep | | Not started |
| AG-11 | States come only from Backbone's reports, never from terminal text or a running process. | [GUIDE][g-bb] | Keep | | Not started |
| AG-12 | Order: agents that need you, then working agents, then the rest, each group by name. A row moves when its state changes. | [GUIDE][g-bb], [CHANGELOG][cl] | Keep | | Not started |
| AG-13 | The roster uses the height the tab list leaves, so a tall window shows every active agent. When not all fit, the label row shows the total and scroll arrows. | [GUIDE][g-bb], [CHANGELOG][cl], [UI_PREVIEW][up-org] | Keep | | Not started |
| AG-14 | The tab list keeps its rows first, but agents that need you or are working keep their rows while the tab list keeps four rows, or one per tab when there are fewer. | [GUIDE][g-bb] | Keep | Translate rows into native heights. | Not started |
| AG-15 | With no active agents the section shows one quiet row, *No active agents*. When the connection fails it shows *Roster unavailable* instead of old states. | [GUIDE][g-bb], [UI_PREVIEW][up-org] | Keep | | Not started |
| AG-16 | After a failed read, cached states stay but are marked stale, with a fixed error that never includes credentials, response bodies or exception text. A later successful read clears it. | [GUIDE][g-bb], [ARCHITECTURE][ar-provider] | Keep | | Not started |
| AG-17 | One provider's failure cannot stop polling or remove live sessions found by tmux discovery. | [ARCHITECTURE][ar-provider] | Keep | | Not started |
| AG-18 | Agent names from the API may stay as offline references, but an API state can never make a missing tmux session attachable. | [ARCHITECTURE][ar-provider] | Keep | | Not started |
| AG-19 | Generic tmux sessions stay available beside configured agents, including offline entries. | [GUIDE][g-bb] | Keep | | Not started |
| AG-20 | The roster does not follow the selected tab or pane. | [GUIDE][g-bb] | Keep | | Not started |
| AG-21 | Clicking an agent's row goes to a tab already showing its session, switching workspace if needed. If several tabs do, it picks the one this window showed most recently. It never attaches or starts anything. | [GUIDE][g-bb] | Keep | | Not started |
| AG-22 | If no tab shows that agent's session, the click opens **Agent status…**, which says so. | [GUIDE][g-bb] | Keep | | Not started |
| AG-23 | **Agent status…** in Configure… spells out each state and ends with the symbol legend. | [GUIDE][g-bb], [UI_PREVIEW][up-org] | Keep | | Not started |
| AG-24 | The roster can be hidden with **Show agents** in Configure…, with `Ctrl-g A`, or by clicking **agents shown** in the status row. The choice is saved with the layout, applies before the first frame and hides the whole section. | [GUIDE][g-bb], [README][rd-keys] | Keep | Saving the choice depends on PE-21. | Not started |
| AG-25 | Ordinary terminals stay usable when Backbone's configuration or API is unavailable. | [GUIDE][g-bb] | Keep | | Not started |

## Settings: themes, keymaps, the JSON editor, refresh

This area has five tables: themes (TH), keymap files (KM), the settings editor
(ED), the shortcut reference (SR) and refresh (RF). The default key bindings
themselves are listed under [Default key bindings](#default-key-bindings).

### Themes

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| TH-1 | The theme file is the first of: `--theme PATH`, `$TMUX_WORKSPACES_THEME`, `$XDG_CONFIG_HOME/tmux-workspaces/theme.toml`, `$HOME/.config/tmux-workspaces/theme.toml`. | [THEMES][th-file] | Keep | Same order on macOS, so both tools share one file. | Not started |
| TH-2 | The path is resolved once, at launch. A missing file is not an error: the shipped colours are used, and the editor saves to that path. | [THEMES][th-file] | Keep | | Not started |
| TH-3 | The theme file may be at most 16 KiB. | [THEMES][th-file], [JSON_SETTINGS][js-save] | Keep | | Not started |
| TH-4 | Reading follows symbolic links. Saving through a link is refused rather than replacing the link with a plain file. | [THEMES][th-file] | Keep | | Not started |
| TH-5 | The file is read at startup and again when **Edit theme…** opens. Edits made in another editor do not change a running window until **Refresh viewer…**. | [THEMES][th-file] | Change | The app reloads the theme in-process (RF-1); no restart. | Not started |
| TH-6 | Presets are `default`, `plain`, `forest`, `paper`, `mono` and `debug`. A one-line file `preset = "<name>"` selects one. Without a `preset` line everything starts from `default`. | [THEMES][th-presets], [THEMES][th-edit] | Keep | | Not started |
| TH-7 | `default`: a `#15171c` panel beside a `#1b1e24` surface, `#2a2e36` lines, `#d4d6db` text, `#7d828c` secondary text, a `#e0a458` accent, a `#e06c75` danger colour, a `#1e2128` header bar and a `#262a33` selection. | [THEMES][th-presets] | Keep | | Not started |
| TH-8 | `paper`: a `#f8f8f8` panel on white, `#3b3b3b` text, a `#005fb8` accent and an `#e8e8e8` selection. `debug`: purple panel, blue surface, amber header bar, red lines, green selection. | [THEMES][th-presets] | Keep | | Not started |
| TH-9 | `plain` (the default colours on the terminal's own background), `forest` (the terminal's background with a sage-green accent) and `mono` (the terminal's two colours, with bold, dim and reverse) use `default` for both grounds. | [THEMES][th-presets] | Open | A native app has no "terminal's own background". Decide what `default` resolves to, so these presets still look as intended. | Not started |
| TH-10 | A `panel` or `surface` value, or a role table, after the `preset` line overrides that part of the preset. | [THEMES][th-presets] | Keep | | Not started |
| TH-11 | Inherited `normal`, `accent` and `muted` backgrounds follow `panel` (and `active` too, in `mono`). The `outline` background follows `surface`. An explicit role `background` wins and stays fixed. | [THEMES][th-presets] | Keep | | Not started |
| TH-12 | Bright ground colours (`panel` and `surface`) accept both `bright-blue` and tmux's `brightblue` spelling. Role colours accept `bright-blue` only. | [THEMES][th-presets] | Keep | | Not started |
| TH-13 | The editor keeps the preset and inherited backgrounds in the saved file instead of turning them into explicit overrides. | [THEMES][th-presets] | Keep | | Not started |
| TH-14 | Two grounds: `panel` (sidebar, status row, popups) and `surface` (every content pane, the new-pane chooser and the separators). Each is `default`, a colour name, a number from 0 to 255, or `#rrggbb`. | [THEMES][th-grounds] | Keep | | Not started |
| TH-15 | Seven roles, `normal`, `active`, `accent`, `muted`, `outline`, `header` and `danger`, each drawn in the places THEMES lists. Each role takes optional `foreground`, `background` and `attributes`. | [THEMES][th-roles] | Keep | Map each role to the matching native element. | Not started |
| TH-16 | Text of one role on the ground of another (the accent marker on the selected row, the muted chevron on the header bar) keeps both colours. | [THEMES][th-roles] | Keep | Natively this is plain painting; no colour pairs are needed. | Not started |
| TH-17 | A colour is `#rrggbb`, `default`, one of the eight basic names, a `bright-` name or a number from 0 to 255. A list is an ordered set of fallbacks, and the first value the terminal can show wins. | [THEMES][th-roles] | Change | Parse the same forms. The app can always show `#rrggbb`, so the first valid entry wins; numbers resolve through the app's 256-colour palette. | Not started |
| TH-18 | `attributes` accepts `bold`, `dim`, `reverse`, `standout` and `underline`. | [THEMES][th-roles] | Keep | Map to native weight, opacity and decoration. | Not started |
| TH-19 | The `▶` marker on the selected row is never removed, so selection never depends on colour alone. | [THEMES][th-roles] | Keep | | Not started |
| TH-20 | Failures are labelled `Error:` and drawn bold. Progress messages such as `Colors saved` or `Defaults shown` are neither. A failure message leads with its reason and puts the file path last. | [THEMES][th-roles] | Keep | | Not started |
| TH-21 | If a role's foreground and background resolve to the same colour, that role falls back to its shipped colours. | [THEMES][th-palettes] | Keep | | Not started |
| TH-22 | On a terminal that refuses more colour pairs, `header` draws as `normal` and `danger` as `accent` with bold. | [THEMES][th-roles] | Drop | A curses colour-pair limit. | Not started |
| TH-23 | RGB role colours are installed into the viewer pane's palette slots 16 to 47 through tmux, and tmux paints the panel. | [THEMES][th-keeps] | Drop | curses cannot name an RGB colour; native drawing can. | Not started |
| TH-24 | The palette size is read from the outer terminal's terminfo, and colours outside it fall back the same way every time (`bright-` to its base colour plus bold, anything else to the nearest xterm colour). | [THEMES][th-palettes] | Drop | The app draws in full colour and does not run inside another terminal. | Not started |
| TH-25 | At least eight colours and six colour pairs are required; there is no monochrome mode. | [THEMES][th-palettes], [STARTUP][su-term] | Drop | A check on the hosting terminal. | Not started |
| TH-26 | An invalid or unreadable startup theme keeps the shipped colours and shows a diagnostic. If the terminal refuses theme installation, startup falls back to the shipped theme, then to the terminal's own colours. | [THEMES][th-edit] | Change | Keep the invalid-file fallback and the diagnostic. The terminal-refusal fallback does not apply. | Not started |
| TH-27 | The font, its size, glyph availability and the colours programs print belong to the terminal. The viewer offers no font setting and never edits terminal or operating-system settings. | [THEMES][th-keeps] | Open | The app draws text itself, so it must own font family, size, fallback glyphs and the terminal colour palette. Decide where these settings live, given that `theme.toml` must stay readable by the Python tool. | Not started |
| TH-28 | Saved colours apply to the window that saved them when the editor closes. Other windows keep their colours until they reload. | [JSON_SETTINGS][js-save], [THEMES][th-file] | Open | Decide whether a save applies to every window of the app, or only to the saving window as in the Python tool. | Not started |
| TH-29 | A saved theme reaches every open part of the window at once, including separators, choosers and popups. | [THEMES][th-beyond], [THEMES][th-notest], [CHANGELOG][cl] | Keep | | Not started |
| TH-30 | Popups use a rounded frame on the panel with a dim border, a title bar shaded from warm dark amber to the header colour, and a footer on the `header` ground. JSON keys and strings in the editor have fixed colours. | [THEMES][th-beyond] | Keep | | Not started |
| TH-31 | Pressing `t` while the sidebar has focus and no menu is open opens **Edit theme…**. | [THEMES][th-edit] | Keep | | Not started |
| TH-32 | Light and dark appearance is not verified by any automated test; contrast is a manual check on a disposable library. | [THEMES][th-notest] | Keep | Record a visual check of each preset on macOS and Linux. | Not started |

### Keymap files

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| KM-1 | One keymap file is used, chosen in this order: `--no-keymap` (shipped map, no files), `--keymap PATH`, `$TMUX_WORKSPACES_KEYMAP`, `$XDG_CONFIG_HOME/tmux-workspaces/keymap.toml`, else `~/.config/tmux-workspaces/keymap.toml`. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-2 | `--keymap` and `--no-keymap` cannot be combined. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-3 | A missing implicit default file is fine. An explicit missing file, an unreadable file, malformed TOML, an unknown action, or an unsupported or duplicate key is a startup error before any terminal is created. | [SHORTCUTS][sc-keymaps] | Keep | Show the error in a native dialog as well as on standard error. | Not started |
| KM-4 | Keymap files are limited to 64 KiB. Relative paths resolve against the invoking directory; `~/` uses the invoking home directory. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-5 | Format: a top-level `prefix`, a `[bindings]` table of prefix keys and a `[direct]` table of one-press shortcuts. A listed action replaces that action's defaults in that table; `[]` disables it; omitted actions keep their defaults. | [SHORTCUTS][sc-keymaps], [keymap.toml][km] | Keep | | Not started |
| KM-6 | Action names are stable identifiers, listed in `integrations/keymap.toml`. Both tables accept every action, with up to 32 keys per action per table. Keys are data, never shell commands. | [SHORTCUTS][sc-keymaps], [ARCHITECTURE][ar-input] | Keep | | Not started |
| KM-7 | A key used twice, including aliases such as `C-i` and `Tab`, is an error. Moving a key means removing it from the other action too; conflicts are named, never silently resolved. | [SHORTCUTS][sc-keymaps], [JSON_SETTINGS][js-ex] | Keep | | Not started |
| KM-8 | The default `m` and `M` menu keys yield when an existing file gives either key to another action, or uses it as the prefix, and leaves out the menu action. Explicit entries follow the normal collision rules. User files are never rewritten to resolve a conflict. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-9 | `bindings` keys: printable ASCII except `;` and `\`; the named keys `Space`, `Enter`, `Tab`, `BSpace`, `Escape`, `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`, `Delete`, `BTab` and `F1` to `F12`; and the `C-` and `M-` modifiers. Uppercase means shifted. Indistinguishable control aliases are treated as one key. | [SHORTCUTS][sc-keymaps] | Keep | Parse the same notation, even though native key events can tell more keys apart. | Not started |
| KM-10 | Escape is reserved for cancelling a prefix, and pressing the prefix twice sends the literal prefix to the terminal. Neither can be bound to an action in the prefix table. | [SHORTCUTS][sc-keymaps], [ARCHITECTURE][ar-input] | Keep | | Not started |
| KM-11 | `direct` keys: lowercase letters and digits; `f1` to `f25`; the punctuation names `bracket_left`, `bracket_right`, `apostrophe`, `backslash`, `comma`, `equal`, `grave_accent`, `minus`, `period`, `semicolon` and `slash`; and the navigation names `space`, `enter`, `tab`, `backspace`, `escape`, `insert`, `delete`, `left`, `right`, `up`, `down`, `page_up`, `page_down`, `home` and `end`. They combine with `super`, `ctrl`, `alt` and `shift`, with at least one of `super`, `ctrl` or `alt`. | [SHORTCUTS][sc-keymaps] | Change | Same notation. The app registers these as its own native shortcuts instead of generating Ghostty triggers. | Not started |
| KM-12 | Direct `ctrl` or `alt` keys that shadow the prefix, the cancel key or a prefix-table key are rejected. Global bindings, key sequences and arbitrary Ghostty actions are outside the format. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-13 | The effective map is validated once and snapshotted per window. On-screen hints, the shortcut reference and CLI help are generated from it, and no config file is reread during navigation. | [SHORTCUTS][sc-keymaps], [ARCHITECTURE][ar-input] | Keep | | Not started |
| KM-14 | `--print-keymap help` and `--print-keymap toml` print the effective map without a terminal or tmux server, and create no library. | [SHORTCUTS][sc-keymaps], [STARTUP][su-diag] | Keep | | Not started |
| KM-15 | `--print-keymap ghostty` prints exact Ghostty triggers and CSI codes, for other terminals to copy into their own settings. | [SHORTCUTS][sc-keymaps] | Drop | The app owns its shortcuts; there is no CSI path to configure. | Not started |
| KM-16 | The dedicated Ghostty launcher applies generated bindings as per-launch options and consumes disabled shipped triggers with Ghostty's `ignore`; its profile is fixed until a fresh instance opens. | [SHORTCUTS][sc-keymaps], [REFRESH][rf-reopen] | Drop | No Ghostty profile is needed. | Not started |
| KM-17 | Prefix bindings change only the viewer's private tmux server. Its prefix table is rebuilt from the map, so disabling a key cannot expose a native tmux command. | [SHORTCUTS][sc-keymaps], [ARCHITECTURE][ar-input] | Change | There is no tmux prefix table. The app reads the prefix itself; an unbound key after the prefix never runs a tmux command. | Not started |
| KM-18 | There is no automatic hot reload; keymap changes apply after **Refresh viewer** or a new launch. | [SHORTCUTS][sc-keymaps] | Change | A saved keymap applies in-process (RF-1). | Not started |
| KM-19 | Disabling an action's keys leaves its mouse path unchanged. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-20 | Menu movement and filtering keys are fixed and cannot be customised. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-21 | Keymaps never affect existing shells or saved arrangements. | [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| KM-22 | Users should pick a prefix that the hosting tmux and terminal do not consume first; validation cannot find every operating-system, window-manager or hosting-tmux conflict. | [SHORTCUTS][sc-keymaps] | Change | The app sees keys before any terminal or tmux. Only operating-system and window-manager shortcuts can still take a key first. | Not started |

### Settings editor

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| ED-1 | **Edit theme…** and **Edit shortcuts…** open a built-in JSON editor in a popup 88 columns wide, centred over the panes, with the workspace visible around it. No external editor is needed, and shells keep running. | [JSON_SETTINGS][js-top], [README][rd-get], [GUIDE][g-settings] | Keep | Size the popup natively. | Not started |
| ED-2 | The editor has clickable **Save**, **Cancel** and **Help** buttons. | [JSON_SETTINGS][js-top] | Keep | | Not started |
| ED-3 | `F2` or `Ctrl-S` saves. Syntax and setting errors keep the editor open. | [JSON_SETTINGS][js-top], [THEMES][th-file] | Keep | Also accept `Command-S` on macOS. | Not started |
| ED-4 | `F10`, `Escape` or `Ctrl-C` cancels. A changed draft needs a second Cancel; editing again clears that confirmation. | [JSON_SETTINGS][js-top] | Keep | | Not started |
| ED-5 | `F1` toggles help; `F4` formats valid JSON. | [JSON_SETTINGS][js-top] | Keep | | Not started |
| ED-6 | `Ctrl-A` selects all; `Ctrl-Z` and `Ctrl-Y` undo and redo. | [JSON_SETTINGS][js-top] | Keep | Also accept the usual Command equivalents on macOS. | Not started |
| ED-7 | Arrow keys, Home/End, PageUp/PageDown, mouse clicks and the wheel move through the text. Enter starts an indented line; Tab inserts spaces. | [JSON_SETTINGS][js-top] | Keep | | Not started |
| ED-8 | Pasted text is inserted as text, never as viewer actions. A bracketed paste is one undo step, and unsupported terminal shortcut sequences are ignored. | [JSON_SETTINGS][js-top] | Change | A native clipboard paste is one undo step. There are no terminal sequences to filter. | Not started |
| ED-9 | A paste interrupted for two seconds is discarded, and the draft stays unchanged. | [JSON_SETTINGS][js-top], [JSON_SETTINGS][js-view] | Drop | An artefact of terminal input; a native paste arrives whole. | Not started |
| ED-10 | While the editor opens, viewer actions are suppressed so early typing never reaches a shell. | [JSON_SETTINGS][js-top] | Keep | Same guarantee: keys typed while the popup opens go to the popup or nowhere. | Not started |
| ED-11 | The draft survives resizing. Below 40 columns or 12 rows the editor asks for more room and disables Save until its controls are visible. | [JSON_SETTINGS][js-top] | Change | Use a native minimum size. Keep the rule that Save is disabled while its controls are hidden. | Not started |
| ED-12 | The editor shows JSON but saves TOML to the selected file, named in the footer. No JSON file is looked up, nothing is migrated, and a missing file is created only on a successful Save. | [JSON_SETTINGS][js-save] | Keep | | Not started |
| ED-13 | The default view of a new theme file includes the default colour fields, so they can be edited directly. | [JSON_SETTINGS][js-ex] | Keep | | Not started |
| ED-14 | If saving would drop comments or hand formatting, the first Save explains this and a second Save confirms that specific draft. | [JSON_SETTINGS][js-save], [THEMES][th-edit], [SHORTCUTS][sc-view] | Keep | | Not started |
| ED-15 | Unreadable files, unsafe write targets, read-only files and concurrent changes are refused with a reason. A draft with invalid JSON, duplicate properties, conflicting keys or unsupported colours cannot replace the working file. | [JSON_SETTINGS][js-save], [THEMES][th-edit], [SHORTCUTS][sc-view] | Keep | | Not started |
| ED-16 | Saving uses a conflict-checked atomic writer. A file that could not be read is never replaced. | [THEMES][th-edit] | Keep | | Not started |
| ED-17 | Text limits are 64 KiB for shortcuts and 16 KiB for colours. | [JSON_SETTINGS][js-save] | Keep | | Not started |
| ED-18 | `--no-keymap` disables editing the shortcut file. | [JSON_SETTINGS][js-save], [SHORTCUTS][sc-view] | Keep | | Not started |
| ED-19 | Opening the theme editor does not restyle the window, and editing a draft does not preview it. Cancel leaves the file and the appearance unchanged. | [THEMES][th-file], [THEMES][th-edit] | Keep | | Not started |
| ED-20 | Saving shortcuts opens the refresh flow; the user may refresh now or keep the current bindings. | [JSON_SETTINGS][js-save], [SHORTCUTS][sc-view] | Change | Save applies the new bindings in-process; there is no refresh step. | Not started |
| ED-21 | Long file paths in popup footers keep the file name visible. | [CHANGELOG][cl] | Keep | | Not started |

### Shortcut reference

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| SR-1 | **Configure… → View shortcuts…** opens a read-only reference in the same popup frame: one row per action, grouped by tabs, panes, workspaces and viewer actions, with the prefix key in one column and the terminal-profile key in another. Custom aliases are included. | [SHORTCUTS][sc-view], [JSON_SETTINGS][js-view], [README][rd-get] | Change | The second column shows the app's native shortcut. | Not started |
| SR-2 | Wide windows show action, prefix key and terminal key in columns; narrow windows stack and wrap them. | [JSON_SETTINGS][js-view] | Keep | | Not started |
| SR-3 | The reference scrolls with arrows, PageUp/PageDown, Home/End or the wheel; the title counts pages. | [SHORTCUTS][sc-view], [JSON_SETTINGS][js-view] | Keep | | Not started |
| SR-4 | **Close**, Escape or F10 returns to the workspace. Typing, pasting, Enter and clicks on rows never run an action or change a setting. | [SHORTCUTS][sc-view], [JSON_SETTINGS][js-view] | Keep | | Not started |
| SR-5 | The reference shows the bindings this window is using, even if the file changed since launch, and it also works with `--no-keymap`. | [JSON_SETTINGS][js-view], [SHORTCUTS][sc-view] | Keep | | Not started |
| SR-6 | Terminal keys need a matching terminal profile; the reference does not imply the terminal has installed them. | [JSON_SETTINGS][js-view] | Drop | Native shortcuts are always active. | Not started |
| SR-7 | The per-row shortcut editor is retired; shortcuts are edited as JSON. | [SHORTCUTS][sc-view] | Keep | | Not started |

### Refresh

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| RF-1 | `Ctrl-g f` or **Configure… → Refresh viewer…** opens a confirmation. Confirming reloads the viewer's code, theme and keymap (prefix bindings and help labels included) while shells keep running; cancelling keeps the current viewer. | [REFRESH][rf-top], [REFRESH][rf-reopen] | Change | Becomes an in-process reload of the theme and keymap. New code needs an app restart, which is cheap because shells live in tmux. Keep the `refresh-viewer` action name so existing keymap files stay valid. | Not started |
| RF-2 | An open name or colour edit must be saved or cancelled before refreshing. | [REFRESH][rf-top], [REFRESH][rf-preserve] | Keep | | Not started |
| RF-3 | A refresh saves the current layout first. A save conflict or an invalid edited keymap leaves the viewer open with an error. | [REFRESH][rf-preserve] | Keep | | Not started |
| RF-4 | The library, attachment source, adapter selection, working directory and theme and keymap selections belong to the launch and carry over. A refreshed viewer never reuses a stale keymap snapshot. | [REFRESH][rf-preserve] | Keep | | Not started |
| RF-5 | A refresh starts a fresh interpreter, watched by a small parent process. Each replacement gets fresh sockets and a fresh identity; repeated requests create at most one; queued actions for the old viewer, Detach included, are ignored. | [REFRESH][rf-reopen], [REFRESH][rf-preserve] | Drop | Interpreter and private-server mechanics. | Not started |
| RF-6 | Changes to the Python interpreter, tmux, the install location, launch options or the terminal itself need a reopen with the intended launcher. | [REFRESH][rf-reopen] | Change | App updates and tmux changes need an app restart. | Not started |
| RF-7 | If a replacement fails, the launcher prints the reason and an exact, quoted reopen command, then becomes an interactive recovery shell; a non-interactive failure returns a failing status. | [REFRESH][rf-preserve] | Drop | There is no interpreter replacement. | Not started |
| RF-8 | A viewer that ignores a stop signal gets ten seconds to clean up before it is ended. | [REFRESH][rf-preserve] | Drop | There is no child viewer process. | Not started |
| RF-9 | Refresh and reopen never install updates, restart a shell server, close a tab or pane, stop an external session, download a package or rewrite user configuration. | [REFRESH][rf-reopen], [REFRESH][rf-preserve], [SHORTCUTS][sc-top] | Keep | | Not started |
| RF-10 | If an existing keymap uses `f` for another action or as its prefix, that wins over the refresh default. **Refresh viewer…** stays in Configure… even without a key. | [SHORTCUTS][sc-other] | Keep | | Not started |
| RF-11 | A dedicated Ghostty instance needs reopening for native-key changes; the refresh screen gives the exact command. | [SHORTCUTS][sc-top], [REFRESH][rf-reopen], [JSON_SETTINGS][js-save] | Drop | No Ghostty profile. | Not started |
| RF-12 | Refresh picks up Python dependency changes that are already installed, but does not install them. | [SHORTCUTS][sc-top] | Drop | Python-specific. | Not started |

## Mouse and keyboard input, selection and copying

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| IN-1 | Viewer actions use a prefix: press `Ctrl-g`, release it, then the action key. | [README][rd-keys], [GUIDE][g-keys] | Keep | The app reads the prefix itself, with no tmux prefix table. | Not started |
| IN-2 | Escape cancels a pending prefix. Pressing the prefix twice sends it to the terminal literally. | [ARCHITECTURE][ar-input], [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| IN-3 | One-press Command shortcuts need the dedicated `./ghostty` profile. In any other terminal, keys that the terminal app consumes stay that app's shortcuts, and the viewer never changes terminal configuration. | [GUIDE][g-keys], [SHORTCUTS][sc-other] | Change | The app is a native window and owns its shortcuts directly. | Not started |
| IN-4 | In choosers and option menus, Up/Down or `Ctrl-p`/`Ctrl-n` move the highlight, Enter activates it, Home/End jump to the first or last row, PageUp/PageDown move through longer lists, and the list scrolls to keep the selection visible. | [SHORTCUTS][sc-cover], [GUIDE][g-keys] | Keep | | Not started |
| IN-5 | Escape or **‹ Back** closes the whole menu and returns to the pane, even from a nested chooser; in an empty workspace, focus stays on the navigation panel. | [SHORTCUTS][sc-cover], [GUIDE][g-keys] | Keep | | Not started |
| IN-6 | While a menu has focus, its input belongs to the navigation panel. | [SHORTCUTS][sc-cover] | Keep | | Not started |
| IN-7 | Each menu row ends with its prefix key. Highlighting a row with the arrow keys shows every way to reach it, prefix and terminal key, in the status row. | [GUIDE][g-split], [UI_PREVIEW][up-org] | Keep | | Not started |
| IN-8 | Mouse: click to select, double-click the active tab or workspace name to rename it, right-click for options. | [README][rd-keys], [SHORTCUTS][sc-right] | Keep | | Not started |
| IN-9 | Right-click means a secondary or two-finger click; holding Shift may make the terminal (Ghostty) handle the click itself instead of the viewer. | [SHORTCUTS][sc-right], [INLINE_RENAME][ir-use] | Drop | The Shift caveat is about a hosting terminal; the app receives every click. | Not started |
| IN-10 | After a sidebar click or a shortcut, typed text waits until navigation and focus finish, so each keystroke reaches the intended shell exactly once, including during rapid and burst navigation. | [ARCHITECTURE][ar-input], [SHORTCUTS][sc-impl], [PERFORMANCE][pf-nav], [ACCEPTANCE][ac-checks] | Keep | Same guarantee, done in-process: input is queued until the focus change is applied. | Not started |
| IN-11 | If a navigation fails part way, typing focus returns to the navigation panel so input cannot land in the wrong pane. | [ARCHITECTURE][ar-nav] | Keep | | Not started |
| IN-12 | Clicks, scrolling and right-clicks inside a content pane reach the program running there. Drag selection and double- or triple-click selection belong to the viewer. | [SHORTCUTS][sc-impl], [ARCHITECTURE][ar-input] | Keep | The app encodes mouse reports for programs that ask for them and sends them through tmux. | Not started |
| IN-13 | Dragging in a pane selects text; releasing keeps the highlight without copying. Double-click selects a word; triple-click selects a line. | [SHORTCUTS][sc-copy] | Keep | | Not started |
| IN-14 | Selection works whether or not the program asks for mouse events or uses the alternate screen. It stays within its pane, and background output does not clear it. | [SHORTCUTS][sc-copy] | Keep | | Not started |
| IN-15 | `Ctrl-g y` (or `Command-C` in a dedicated Ghostty window) copies the selection. The highlight stays after copying. Copying with no selection does nothing. | [SHORTCUTS][sc-copy], [GUIDE][g-keys] | Keep | | Not started |
| IN-16 | Escape or a click outside the selection clears it; a click elsewhere in the same pane also returns to the live terminal. | [SHORTCUTS][sc-copy], [GUIDE][g-keys] | Keep | | Not started |
| IN-17 | Copying writes the clipboard through tmux's OSC 52 output, so the terminal must support and allow clipboard writes. | [SHORTCUTS][sc-copy], [GUIDE][g-limits], [ACCEPTANCE][ac-limits] | Change | The app writes the system clipboard directly. | Not started |
| IN-18 | A resize or layout change may clear the selection. | [SHORTCUTS][sc-copy] | Change | The selection lives in the app's terminal state, so it survives tab switches. A resize that reflows text may still clear it. | Not started |
| IN-19 | `Command-V` paste, font-size keys, app preferences and normal shell Control shortcuts keep their usual behaviour. | [SHORTCUTS][sc-ws] | Change | The app implements paste, font size and preferences itself. Paste uses bracketed paste when the program has turned it on. | Not started |
| IN-20 | Bracketed paste works with an ordinary interactive shell. | [ARCHITECTURE][ar-input], [PERFORMANCE][pf-nav] | Keep | | Not started |
| IN-21 | The private viewer turns off tmux's `assume-paste-time`, so rapid text cannot slip past shortcut bindings. | [ARCHITECTURE][ar-input] | Drop | No private tmux server reads the keyboard. | Not started |
| IN-22 | tmux's late `DoubleClick1Pane` notification is ignored; renaming uses the panel's own click timing. | [ARCHITECTURE][ar-input] | Drop | The app gets native click events. | Not started |
| IN-23 | Fragmented Unicode input is handled correctly. | [PERFORMANCE][pf-nav] | Keep | Includes native input methods such as dead keys and composed or CJK input. | Not started |
| IN-24 | Mouse reporting must be on in the terminal for the viewer's mouse controls. | [GUIDE][g-keys] | Drop | The app receives mouse events natively. | Not started |
| IN-25 | Scrolling inside a pane depends on the terminal and the inner session's mouse settings. | [GUIDE][g-limits] | Change | The app keeps native scrollback per pane. The wheel scrolls history, or goes to the program when it has asked for mouse events. | Not started |

### Default key bindings

Prefix keys are pressed after `Ctrl-g`. Direct keys are written in
`keymap.toml` with `super`, which is Command on macOS. The full shipped file is
[integrations/keymap.toml][km].

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| KB-1 | The prefix is `Ctrl-g` (`prefix = "C-g"`). | [keymap.toml][km], [README][rd-keys] | Keep | | Not started |
| KB-2 | `new-tab`: prefix `t` or `c`; direct `Command-T`. Opens a new tab as a chooser. | [keymap.toml][km], [SHORTCUTS][sc-tabs], [GUIDE][g-keys] | Keep | | Not started |
| KB-3 | `split-right`: prefix `v` or `%`; direct `Command-D`. | [keymap.toml][km], [SHORTCUTS][sc-tabs], [GUIDE][g-keys] | Keep | | Not started |
| KB-4 | `split-below`: prefix `h` or `"`; direct `Command-Shift-D`. | [keymap.toml][km], [SHORTCUTS][sc-tabs], [GUIDE][g-keys] | Keep | | Not started |
| KB-5 | `attach`: prefix `a`; direct `Command-Shift-A`. Attaches a session to the focused pane. | [keymap.toml][km], [SHORTCUTS][sc-ws] | Keep | | Not started |
| KB-6 | `rename-tab`: prefix `r`; direct `Command-R`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-7 | `tab-options`: prefix `m`; no direct key. | [keymap.toml][km], [GUIDE][g-keys] | Keep | | Not started |
| KB-8 | `next-tab`: prefix `n`; direct `Command-Shift-]`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-9 | `previous-tab`: prefix `p`; direct `Command-Shift-[`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-10 | `next-pane`: prefix `o`; direct `Command-]`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-11 | `previous-pane`: prefix `O`; direct `Command-[`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-12 | `focus` (focus one pane, or restore the layout): prefix `z`; direct `Command-Shift-Enter`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-13 | `workspaces` (the workspace chooser): prefix `w`; no direct key. | [keymap.toml][km], [GUIDE][g-keys] | Keep | | Not started |
| KB-14 | `workspace-options`: prefix `M`; no direct key. | [keymap.toml][km], [GUIDE][g-keys] | Keep | | Not started |
| KB-15 | `new-workspace`: prefix `W`; direct `Command-Shift-N` (create and name a workspace). | [keymap.toml][km], [SHORTCUTS][sc-ws] | Keep | | Not started |
| KB-16 | `rename-workspace`: prefix `R`; direct `Command-Shift-R`. | [keymap.toml][km], [SHORTCUTS][sc-ws] | Keep | | Not started |
| KB-17 | `next-workspace`: prefix `]`; direct `Command-Option-Right`. | [keymap.toml][km], [SHORTCUTS][sc-ws] | Keep | | Not started |
| KB-18 | `previous-workspace`: prefix `[`; direct `Command-Option-Left`. | [keymap.toml][km], [SHORTCUTS][sc-ws] | Keep | | Not started |
| KB-19 | `close-pane`: prefix `x`; direct `Command-W`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-20 | `close-tab`: prefix `&`; direct `Command-Shift-W`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-21 | `copy-selection`: prefix `y`; direct `Command-C`. | [keymap.toml][km], [SHORTCUTS][sc-copy] | Keep | | Not started |
| KB-22 | `sidebar` (focus the navigation panel): prefix `s`; direct `Command-B`. | [keymap.toml][km], [SHORTCUTS][sc-ws] | Keep | | Not started |
| KB-23 | `refresh-viewer`: prefix `f`; no direct key. | [keymap.toml][km], [SHORTCUTS][sc-top] | Change | Runs the in-process reload from RF-1. | Not started |
| KB-24 | `show-agents`: prefix `A`; no direct key. | [keymap.toml][km], [GUIDE][g-keys] | Keep | | Not started |
| KB-25 | `quit` (detach, keeping every terminal): prefix `d`; no direct key. | [keymap.toml][km], [GUIDE][g-keys], [SHORTCUTS][sc-other] | Keep | | Not started |
| KB-26 | `select-tab-1` to `select-tab-9`: no prefix key; direct `Command-1` to `Command-9`. | [keymap.toml][km], [SHORTCUTS][sc-tabs] | Keep | | Not started |
| KB-27 | `select-workspace-1` to `select-workspace-9`: no prefix key; direct `Command-Option-1` to `Command-Option-9`. | [keymap.toml][km], [SHORTCUTS][sc-ws] | Keep | | Not started |
| KB-28 | `Command-N` opens another window running the same viewer and library. It is a Ghostty action, not a keymap entry. | [SHORTCUTS][sc-ws] | Change | A native New Window command. | Not started |
| KB-29 | Direct keys are written with `super`, which is Command on macOS; for example `super+alt+t` is `Command-Option-T`. | [keymap.toml][km], [SHORTCUTS][sc-keymaps] | Open | Decide the Linux defaults. Desktop environments often reserve Super, so Linux may need another modifier. | Not started |

## Status row and visual layout

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| VL-1 | The navigation panel is 22 columns wide, flush with the window's left edge, and dragging pane separators never changes its width. | [GUIDE][g-resize], [UI_PREVIEW][up-org], [THEMES][th-beyond] | Open | The width becomes a native size. Decide whether it stays fixed or the user can drag the sidebar edge. | Not started |
| VL-2 | Terminals sit on a slightly lighter `surface` beside the `panel`. One thin line in the `outline` colour separates the panel from the content, and split panes from each other, with no extra border or padding column. | [GUIDE][g-resize], [THEMES][th-beyond], [UI_PREVIEW][up-org] | Keep | | Not started |
| VL-3 | No border marks the focused pane; the cursor does. | [GUIDE][g-resize], [THEMES][th-beyond], [UI_PREVIEW][up-org] | Open | Decide whether the app adds a focus cue, such as dimming inactive panes. | Not started |
| VL-4 | The panel's first row is the workspace heading on the header bar: the workspace icon when one is set, the name, and a `▾` chooser at the right. | [UI_PREVIEW][up-org] | Keep | | Not started |
| VL-5 | A blank row follows the heading; the inline-rename hint uses it. | [UI_PREVIEW][up-org] | Keep | | Not started |
| VL-6 | Then the `TABS` label with the `+` that adds a tab, then one row per tab: its number, its name and one `▮` per pane. Nothing else repeats what a tab's panes hold. | [UI_PREVIEW][up-org], [README][rd-top] | Keep | | Not started |
| VL-7 | The selected tab is one filled row with its name in bold and a `⋯` at the right end. Long names end with an ellipsis. | [UI_PREVIEW][up-org] | Keep | | Not started |
| VL-8 | From the last row up: the workspace icon row, **Configure…**, then a blank row. The AGENTS section sits above that when shown. | [UI_PREVIEW][up-org] | Keep | | Not started |
| VL-9 | **Configure…** lists Edit theme…, Edit shortcuts…, View shortcuts…, Refresh viewer…, then Show agents and Agent status…, then Detach after a rule. | [UI_PREVIEW][up-org], [GUIDE][g-settings] | Keep | Refresh viewer… becomes the reload action (RF-1). | Not started |
| VL-10 | Menus take the place of the tab list in the panel, headed by **‹ Back**, with each row's prefix key at its right end. | [UI_PREVIEW][up-org], [THEMES][th-roles] | Open | Decide between in-panel menus, as in the Python tool, and native context menus or popovers. Keyboard navigation (IN-4) stays either way. | Not started |
| VL-11 | The navigation panel scrolls with the wheel or its arrow controls. | [GUIDE][g-ws] | Keep | | Not started |
| VL-12 | A navigation panel shorter than 16 rows shows a hint to enlarge the window, and core shortcuts keep working. | [UI_PREVIEW][up-try2] | Change | Set a native minimum window size, and keep a hint if content still cannot fit. | Not started |
| VL-13 | The status row runs along the bottom of the window on the `panel` colour, with one cell of inset on each side, no line above it and no blank padding rows. | [GUIDE][g-settings], [THEMES][th-beyond], [UI_PREVIEW][up-org] | Keep | | Not started |
| VL-14 | Its left part says where you are: the workspace, tab and focused pane (for example `Development · web · pane 1/2`), or the highlighted menu row and its keys, or the open popup. | [GUIDE][g-settings], [README][rd-top] | Keep | | Not started |
| VL-15 | Its centre part shows the few keys that matter in the current mode (for example `^g t new · ^g v split`). | [GUIDE][g-settings], [README][rd-top] | Keep | | Not started |
| VL-16 | Its right part shows the agents toggle (`agents shown ^g A`) when an agent source is connected, and a small green light while the saved arrangement is current. A message replaces the light when something needs attention. | [GUIDE][g-settings], [README][rd-top], [THEMES][th-beyond] | Keep | | Not started |
| VL-17 | Status text uses the `muted` colour, with `normal` and `accent` for emphasised words. | [THEMES][th-beyond] | Keep | | Not started |
| VL-18 | Popups (the settings editor and the shortcut reference) open centred over the panes. | [JSON_SETTINGS][js-top], [GUIDE][g-settings], [CHANGELOG][cl] | Keep | | Not started |
| VL-19 | The heading and the status band are one text row each, with no vertical padding of their own. | [THEMES][th-roles] | Change | Native rows can have their own padding. | Not started |
| VL-20 | The dedicated Ghostty profile removes window padding, and leftover fractional cell space takes the adjacent cell's background, so the surface and status band reach the window edges. | [THEMES][th-roles] | Change | The app lays out its own window. Space left over after the cell grid takes the adjacent background. | Not started |
| VL-21 | In macOS full screen, the temporary title bar can cover the workspace heading while the pointer is at the top of the screen. | [THEMES][th-roles] | Change | The app controls its own title-bar area and should keep the heading visible. | Not started |
| VL-22 | A glyph such as `▶` or `─` that shows as a box is a font problem in the terminal, not the theme. | [THEMES][th-keeps] | Change | The app chooses its fonts and must supply fallback glyphs for the symbols it draws. | Not started |

## Startup, preflight and recovery

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| SU-1 | Requirements are Python 3.11+, tmux 3.3+ and a Unix terminal with Python curses support and a usable colour terminfo entry. No third-party Python packages are needed. | [README][rd-try], [STARTUP][su-top], [GUIDE][g-top] | Change | A Rust binary. The Python, curses and terminfo requirements go away; tmux stays the one runtime dependency. | Not started |
| SU-2 | tmux 3.3 or newer is required. | [README][rd-try], [STARTUP][su-top] | Open | Confirm that every control-mode feature the app needs exists in tmux 3.3, or raise the floor with evidence. Evidence: [tmux control mode research](research/tmux-control-mode.md#version-and-implementation-baseline). | Not started |
| SU-3 | The version check runs `tmux -V`, which contacts no server. Patch letters and numbered development suffixes pass when the major and minor version meet the floor; an unnumbered build cannot establish compatibility. | [STARTUP][su-top] | Keep | | Not started |
| SU-4 | A missing tmux, a failed version command or an unknown version reports a remedy, for example `Workspace viewer: tmux 3.2a detected; tmux 3.3 or newer is required. Upgrade tmux.` | [STARTUP][su-top] | Keep | Also show it in a native dialog, since a Dock launch has no terminal. | Not started |
| SU-5 | tmux is found through `PATH`; installed wrappers put the chosen tools' directories at the front of `PATH`. | [PACKAGING][pk-layout], [PACKAGING][pk-upgrade] | Open | An app started from the Dock gets a minimal `PATH` that usually lacks a Homebrew tmux. Decide how the app finds tmux (known locations, the login shell's `PATH`, a setting). | Not started |
| SU-6 | Every entry path rejects an older Python before importing the application, for example `Python 3.9.6 is unsupported; Python 3.11+ is required.` | [STARTUP][su-top] | Drop | Python-specific. | Not started |
| SU-7 | Before opening the library, startup needs terminal input and output, a non-empty `TERM`, Python curses, and terminfo with at least 8 colours, 6 colour pairs, cursor positioning, screen clearing and attribute reset. | [STARTUP][su-term] | Drop | The app does not run inside a terminal. | Not started |
| SU-8 | An unknown `TERM` or a monochrome entry such as `vt100` is refused with advice to install the matching terminfo; a Python without curses is refused; a later curses failure reports a terminfo or ncurses remedy. | [STARTUP][su-term] | Drop | Same reason as SU-7. | Not started |
| SU-9 | A failed prerequisite leaves a fresh library empty, creates no runtime sockets, and prints an actionable message without a traceback or screen-control sequences. | [STARTUP][su-verify], [ACCEPTANCE][ac-checks] | Keep | | Not started |
| SU-10 | A partial startup cleans up what it created and keeps the original error. Ordinary shells and external sessions are never cleanup targets. | [STARTUP][su-term], [ARCHITECTURE][ar-life] | Keep | | Not started |
| SU-11 | `--help` and `--print-keymap` need no tmux, terminal or valid `TERM`, and open no library. | [STARTUP][su-diag], [SHORTCUTS][sc-keymaps] | Keep | | Not started |
| SU-12 | Configuration is selected and validated before any runtime resource is created. | [ARCHITECTURE][ar-input] | Keep | | Not started |
| SU-13 | An invalid library stops the launch before any server or shell starts. The error names `layouts.db` and the invalid field or database problem. Nothing is repaired automatically, and older tables are never used as a fallback. | [RECOVERY][rc-top], [ARCHITECTURE][ar-valid] | Keep | | Not started |
| SU-14 | A running viewer refuses to refresh or save over an invalid current record. | [RECOVERY][rc-top], [ARCHITECTURE][ar-valid] | Keep | | Not started |
| SU-15 | Recovery advice: do not delete `layouts.db`; keep the `-wal` and `-shm` files; exit every viewer, then copy the whole directory to a new location; use SQLite's backup tools if a writer cannot be closed. | [RECOVERY][rc-keep] | Keep | Carry this advice into the Rust app's docs. | Not started |
| SU-16 | `--data-dir` with a new directory starts an independent library and shell server and leaves the original untouched. A backup restored into another directory opens the same way. | [RECOVERY][rc-continue], [RECOVERY][rc-copy] | Keep | | Not started |
| SU-17 | There is no automatic repair or export command. | [RECOVERY][rc-copy] | Keep | | Not started |
| SU-18 | Split dragging, clicks on panes while an attachment starts, and input sent during startup are each handled once, in order. | [CHANGELOG][cl] | Keep | | Not started |
| SU-19 | Losing the private display server unexpectedly ends the viewer with a non-zero exit status. | [ARCHITECTURE][ar-life] | Change | There is no display server. A lost tmux connection is reported per pane, and the window stays open. | Not started |
| SU-20 | macOS and Linux are tested; WSL is unverified; there is no native Windows runtime. | [README][rd-try], [GUIDE][g-top], [ACCEPTANCE][ac-limits] | Keep | macOS first, Linux from the same code. Windows is optional. | Not started |

## Remote use over SSH

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| RM-1 | The viewer runs on the tmux host, locally or through SSH, and needs no desktop. It has no built-in remote terminal transport. | [README][rd-get], [GUIDE][g-limits], [ACCEPTANCE][ac-limits] | Open | A native app needs a desktop. Decide whether remote hosts are reached by running tmux control mode over SSH, or left to the Python tool for now. | Not started |
| RM-2 | Shells keep the SSH agent and XDG paths the user started with. | [README][rd-get], [SHELL_ENVIRONMENT][se-vars] | Keep | Locally these come from the app's own environment (SH-4). | Not started |
| RM-3 | The library's shell server can outlive the SSH connection that started it. After reconnecting, launching again with the same library gives new shells the current SSH and XDG values. | [SHELL_ENVIRONMENT][se-ssh] | Keep | The same rule covers a changed local agent socket. | Not started |
| RM-4 | To update a running shell's agent socket, the user prints `$SSH_AUTH_SOCK` in a fresh login and exports it in the old shell, or unsets it; the tool does not do this for them. | [SHELL_ENVIRONMENT][se-ssh] | Keep | | Not started |
| RM-5 | `TERMINFO` and `TERMINFO_DIRS` are preserved for terminals, such as Ghostty, that ship their own definitions; an unknown `TERM` needs the matching terminfo on the SSH host. | [GUIDE][g-limits], [STARTUP][su-term] | Change | Locally the app is the terminal. If remote support comes (RM-1), the remote tmux still needs a usable `TERM` for its panes. | Not started |
| RM-6 | Linux CI includes an authenticated loopback SSH test; native macOS SSH and real network latency are unverified. | [ACCEPTANCE][ac-platform], [ACCEPTANCE][ac-checks] | Open | Depends on RM-1. | Not started |
| RM-7 | The viewer may run inside WSL, but WSL behaviour is unverified. | [GUIDE][g-top], [GUIDE][g-limits] | Open | Windows support is optional. Decide whether WSL is a target. | Not started |

## Launchers, TPM and packaging

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| LP-1 | `./run` in a checkout and `python3 -m tmux_workspaces` start the same CLI; a wheel provides the `tmux-workspaces` console command. | [README][rd-try], [ARCHITECTURE][ar-top] | Change | One Rust binary named `tmux-workspaces`, plus a macOS app bundle. | Not started |
| LP-2 | To install, link `run` to `~/.local/bin/tmux-workspaces` and keep the checkout in place. If that command already exists, choose another name instead of replacing it. | [README][rd-install] | Open | The Rust binary has the same name as the Python command. Decide how both can be installed side by side. | Not started |
| LP-3 | On macOS, `./ghostty` opens a dedicated Ghostty instance with Command-key shortcuts on the same default library. It never edits or reloads the user's Ghostty configuration. | [README][rd-try], [SHORTCUTS][sc-top], [GUIDE][g-keys] | Drop | The app is a native window with its own shortcuts. | Not started |
| LP-4 | The dedicated Ghostty window shows the user's usual Ghostty appearance. | [README][rd-try], [SHORTCUTS][sc-top] | Open | Decide whether the app reads the user's terminal settings (for example Ghostty's font and colours), or only its own. | Not started |
| LP-5 | `./ghostty` accepts `--data-dir`, `--backbone`, `--backbone-data-dir`, `--keymap`, `--no-keymap`, `--ghostty-app PATH`, `--dry-run` and `--help`. It needs macOS and an executable Ghostty app, and checks tmux and curses first. | [SHORTCUTS][sc-top], [SHORTCUTS][sc-keymaps], [STARTUP][su-diag] | Drop | The app takes the main flags directly. | Not started |
| LP-6 | The Ghostty instance shares Ghostty's app identity and window-restoration preferences; it is a shortcut profile, not a separate app. | [SHORTCUTS][sc-top], [SHORTCUTS][sc-exec] | Drop | The app has its own identity. | Not started |
| LP-7 | A launch error `exec: exec: not found` is fixed by closing only the failed Ghostty surfaces and running `./ghostty` again; no data repair is needed. | [SHORTCUTS][sc-exec] | Drop | Only happens with the Ghostty launcher (LP-3). | Not started |
| LP-8 | With TPM, the checkout is linked to `~/.tmux/plugins/tmux-workspaces` and `set -g @plugin 'tmux-workspaces'` goes before TPM's initialization line. After a reload, the outer prefix then `W` opens a workspace window inside tmux. | [GUIDE][g-tpm], [README][rd-install] | Open | The app is not drawn inside tmux. Decide whether the TPM key opens the native app on the same library, or TPM support is dropped. | Not started |
| LP-9 | The TPM options `@tmux-workspaces-key` (launch key, default `W`) and `@tmux-workspaces-data-dir` (an explicit library) go before TPM's initialization line. | [GUIDE][g-tpm], [NAMING][nm-compat] | Open | Depends on LP-8. Keep the option names if TPM support stays. | Not started |
| LP-10 | The TPM plugin reads sessions from the hosting server, does not change that server's pane options, and shares the standalone launcher's library. | [GUIDE][g-tpm] | Open | Depends on LP-8. | Not started |
| LP-11 | Running viewers and loaded TPM bindings embed absolute checkout paths, so old entry scripts are kept as small shims, and moving a checkout can break them. | [ARCHITECTURE][ar-entry], [NAMING][nm-compat] | Drop | Python entry-path compatibility. | Not started |
| LP-12 | `scripts/install_bundle.py --prefix NEW --python ABS --tmux ABS` installs `bin/tmux-workspaces`, `bin/tmux-workspaces-ghostty`, `libexec/tmux-workspaces/` and `share/tmux-workspaces/tmux-workspaces.tmux`. It refuses an existing prefix (even an empty one), accepts a prefix with spaces and quotes, and needs tool paths without whitespace; `--runtime-prefix` writes final Homebrew paths. Its wrappers pin the interpreter and put the tools on `PATH`. | [PACKAGING][pk-layout], [PACKAGING][pk-install] | Change | Rust packaging is a later decision. Publishing anything needs the maintainer's approval. | Not started |
| LP-13 | `scripts/package_source.py --revision HEAD --output NEW` builds a reproducible source archive, a `manifest.json` with the commit and SHA-256, and a local Homebrew recipe (selecting `python@3.14` and `tmux`) with a `file:` URL; `check_package.py` checks the digest before extraction. Nothing is uploaded, and there is no published tap or release asset. | [PACKAGING][pk-archive], [PACKAGING][pk-top] | Change | Same rule: nothing is published without approval. | Not started |
| LP-14 | A library is never stored inside an installation, keg, virtual environment, TPM clone or bundle prefix. Configuration, sockets and layouts are not install payloads and do not depend on the install location. | [PACKAGING][pk-layout] | Keep | | Not started |
| LP-15 | Upgrade: detach every viewer, install into a new prefix, repoint owned links, reopen the same library and verify. Roll back by reopening through the old command; across versions, check schema compatibility first. | [PACKAGING][pk-upgrade] | Keep | Shells live in tmux, so replacing the app never ends them. | Not started |
| LP-16 | A tmux client and server version mismatch is never fixed by killing the user's servers; arbitrary tmux upgrades are not proven compatible. | [PACKAGING][pk-upgrade] | Keep | Matters for control mode too: the app runs the `tmux` client against servers that may be older. | Not started |
| LP-17 | Uninstalling removes only owned files and links. It never kills processes or deletes libraries, sockets, shells, sessions or settings. | [PACKAGING][pk-upgrade] | Keep | | Not started |
| LP-18 | There is no `brew services` unit, background updater or automatic cleanup. | [PACKAGING][pk-upgrade] | Keep | Fits the rule that the app runs no daemon. | Not started |
| LP-19 | Renaming the project keeps environment variable names, XDG configuration paths, `tmux-workspaces.tmux` and the `@tmux-workspaces-*` options. The proposed future name, Muxstead, has no command yet. | [NAMING][nm-compat], [NAMING][nm-top] | Keep | Use the same environment variable names and configuration paths. | Not started |
| LP-20 | No terminal plugin, browser or Backbone service is required. | [GUIDE][g-top] | Keep | | Not started |
| LP-21 | CI runs the lint, unit and real-terminal suites on Ubuntu and macOS. | [ACCEPTANCE][ac-platform] | Keep | Rust CI covers macOS and Linux. | Not started |
| LP-22 | Contributors run `make check` (lint and unit tests), `make smoke` (real-terminal suites), `make package-smoke` (two relocated installs of the committed HEAD) and `make benchmark` (options through `BENCHMARK_ARGS`). | [README][rd-top], [ACCEPTANCE][ac-platform], [PACKAGING][pk-verify], [PERFORMANCE][pf-run] | Change | Cargo-based equivalents (`cargo fmt --check`, `cargo clippy`, `cargo test`) plus real-terminal and benchmark suites. | Not started |
| LP-23 | No installer edits `~/.tmux.conf`, installs TPM, reloads a live tmux server or changes Ghostty settings. | [PACKAGING][pk-install] | Keep | | Not started |
| LP-24 | Windows opened by the Ghostty launcher can edit their own shortcuts. | [CHANGELOG][cl] | Drop | Every app window can edit shortcuts (ED-1). | Not started |

## Performance budgets

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| PF-1 | The engineering target for every one- and four-pane tab and workspace navigation path, by click and by direct shortcut, is p50 ≤ 150 ms and p95 ≤ 250 ms, for both shell-client readiness and routed-input acknowledgement. These are acceptance budgets, not machine guarantees; historical quiet-host runs met them. | [PERFORMANCE][pf-top], [PERFORMANCE][pf-gate] | Change | Keep these as ceilings; the app should beat them easily, since switching starts no client. Add native measures: time to the first presented frame, typing latency and frame pacing against plain Ghostty, and bulk output against Ghostty running tmux. | Not started |
| PF-2 | The benchmark uses only disposable resources: unique sockets, temporary directories and a temporary `HOME`. It never uses the default tmux socket or an existing library, and cleanup touches only its own servers. | [PERFORMANCE][pf-run] | Keep | | Not started |
| PF-3 | The default workload runs one-, four- and eight-pane layouts, three 30-second idle samples per layout and 20 events per navigation group, with two tabs per workspace. `--viewers 2` adds a second viewer that stays on its tab. | [PERFORMANCE][pf-run] | Keep | Port the workload; flag names may differ. | Not started |
| PF-4 | Shell-client readiness is the time until the destination layout has the expected leaves and every ordinary shell has a client from the exact display pane. | [PERFORMANCE][pf-json] | Change | There are no per-pane clients. Measure until the app holds the destination's terminal state and has presented it. | Not started |
| PF-5 | Routed acknowledgement: a unique `printf` probe typed after each navigation must reach only the intended shell; duplicate delivery fails the run. | [PERFORMANCE][pf-json], [PERFORMANCE][pf-not] | Keep | | Not started |
| PF-6 | "First PTY output" is recorded but is not native paint or frame completion. | [PERFORMANCE][pf-json] | Change | The app can time presented frames directly. | Not started |
| PF-7 | Observer cost (tmux query count and time) is recorded with each event and never subtracted. | [PERFORMANCE][pf-json] | Keep | | Not started |
| PF-8 | Idle CPU covers the whole owned process tree (viewer, tmux servers and shells), where 100% is one core. The reference result is 0.629% of one core for one pane and 0.706% for four, on one Apple silicon host. | [PERFORMANCE][pf-json], [PERFORMANCE][pf-nav] | Keep | Use as a comparison baseline; the app should not idle higher. | Not started |
| PF-9 | The raw JSON records the revision, dirty state, a diff fingerprint, OS, architecture, CPU count, versions, dimensions, workload, every sample, the median and nearest-rank p95. It lists changed tracked paths, but no host name, operator paths, credentials or shell history. Dirty or traced runs are `acceptance_eligible: false`. | [PERFORMANCE][pf-json] | Keep | | Not started |
| PF-10 | Acceptance needs at least three independent untraced runs under comparable load and power settings, recorded with the results. | [PERFORMANCE][pf-json], [PERFORMANCE][pf-diag] | Keep | | Not started |
| PF-11 | Resize and attached-session offline and reconnect probes run on each fixture as single diagnostic samples. | [PERFORMANCE][pf-run], [PERFORMANCE][pf-not] | Keep | | Not started |
| PF-12 | Same-write and burst tests type immediately alongside a gesture in one- and four-pane layouts and check that each marker arrives exactly once. | [PERFORMANCE][pf-nav], [PERFORMANCE][pf-not] | Keep | | Not started |
| PF-13 | Human acceptance on the real terminal: alternate tabs and workspaces at least 20 times by click and by shortcut, watching for a highlight flash, missing names, transient layout expansion or late selection; type a marker right after each gesture; resize; repeat with two windows; reconnect an attached session; record the terminal, version, dimensions and whether the link was local or SSH. | [PERFORMANCE][pf-human], [ACCEPTANCE][ac-perf] | Keep | | Not started |
| PF-14 | A speed claim needs measured budget compliance, correct input routing, preserved ownership and layout, passing integration checks and the observed interaction; the same bar applies to later changes. | [PERFORMANCE][pf-human] | Keep | | Not started |
| PF-15 | The manual acceptance checklist covers: switching and typing markers; opening, splitting, dragging and resizing; attaching, returning to the shell and going offline; renaming and checking light and dark themes; detaching and reopening, with two viewers; and recording the environment. | [ACCEPTANCE][ac-perf] | Keep | | Not started |
| PF-16 | Actions wake the controller directly instead of waiting for its 150 ms fallback poll, and navigation uses bounded tmux command batches. | [PERFORMANCE][pf-nav] | Drop | Internals of the Python controller. | Not started |
| PF-17 | The benchmark accepts `--panes`, `--samples`, `--idle-samples`, `--idle-seconds`, `--warmup`, `--viewers` and `--trace-commands`. A quick run with small values checks the harness and is not an acceptance measurement. | [PERFORMANCE][pf-run], [PERFORMANCE][pf-diag] | Change | A Rust harness with equivalent controls. | Not started |
| PF-18 | Traced runs count the application's tmux invocations separately from observer queries; they are compared only with traced runs and never used for the latency gate. | [PERFORMANCE][pf-diag] | Keep | | Not started |
| PF-19 | CPU is read with `ps -S` cumulative self and waited-child time on macOS and from `/proc/PID/stat` on Linux, counting each owned live process once; samples whose process set changed are invalidated. | [PERFORMANCE][pf-json] | Keep | | Not started |

## Demo and preview modes

| # | Behaviour (Python tool) | Source | Decision | Notes | Status |
| --- | --- | --- | --- | --- | --- |
| DM-1 | `--demo` runs with disposable sample attachment sessions that are recreated on each launch, so their processes reset. | [REFRESH][rf-preserve], [GUIDE][g-iso], [PACKAGING][pk-layout] | Keep | | Not started |
| DM-2 | Demo layouts and ordinary terminals use a `demo/` subdirectory of the library. Terminals created with **+** persist like those in a normal library. | [GUIDE][g-iso] | Keep | | Not started |
| DM-3 | Demo attachment references follow the new disposable fixture server on each launch. | [GUIDE][g-iso], [ARCHITECTURE][ar-provider] | Keep | | Not started |
| DM-4 | The demo is a state-reporting source, so the roster appears. Demo agent states are fixtures, and demo shells are not AI agents. | [GUIDE][g-bb], [GUIDE][g-limits] | Keep | | Not started |
| DM-5 | An installed trial uses `tmux-workspaces --data-dir <disposable library> --demo`. | [PACKAGING][pk-layout] | Keep | | Not started |
| DM-6 | `./preview` opens the same UI on a separate sample library in a new Ghostty window (macOS); `./preview --terminal` uses the current terminal. Existing viewers can stay open. | [UI_PREVIEW][up-try], [INLINE_RENAME][ir-sample] | Change | One way to open the sample library in a new native window, such as a flag. | Not started |
| DM-7 | The sample has three workspaces, six tabs in Development and a four-pane tab. Two panes attach to disposable demo shells; no live agents are used. | [UI_PREVIEW][up-try] | Keep | | Not started |
| DM-8 | The sample library lives under `.backbone/ui-preview/demo/` in the Python tool's checkout. Repeated launches keep sample edits and shells and recreate the fixtures; nothing is copied into the normal library. | [UI_PREVIEW][up-try], [PACKAGING][pk-layout] | Change | Use a disposable location outside the source tree, as the Python docs already ask of any installed preview. | Not started |
| DM-9 | The preview checks prerequisites before seeding the sample. | [STARTUP][su-diag] | Keep | | Not started |
| DM-10 | A disposable library for manual checks is `TMUX_WORKSPACES_DATA_DIR=$(mktemp -d) ./run`. | [THEMES][th-notest] | Keep | The same variable works with the Rust binary. | Not started |

## Known design differences

- **No second tmux layer.** The Python tool shows every pane through a nested
  `tmux attach` client on a private display server. The app is a control-mode
  client and draws panes itself. Tab switches start no process, layout changes
  cause no redraw flash, and nested-tmux sizing and scrolling quirks go away
  (SP-13 to SP-15, SH-15, CH-33).
- **Keys and mouse are native.** There is no action socket, no tmux prefix
  table, no CSI translation and no Ghostty profile. The `Ctrl-g` prefix and the
  Command shortcuts are both read by the app, and the `keymap.toml` format stays
  the same (IN-1, IN-3, KM-11, KM-15 to KM-17).
- **Selection, copy and scrollback belong to the app.** Copying uses the system
  clipboard instead of OSC 52, and a selection survives tab switches (IN-17,
  IN-18, IN-25).
- **No terminal capability checks.** curses, terminfo, colour-pair and palette
  fallbacks are dropped. In return the app must own fonts and the terminal
  colour palette (TH-22 to TH-25, TH-27, SU-7, SU-8).
- **Reload instead of refresh.** Theme and keymap changes apply in-process. New
  code means an app restart, which is cheap because shells live in tmux. The
  interpreter replacement, supervising parent and recovery shell are dropped
  (RF-1, RF-5 to RF-8).
- **One native app instead of several launchers.** A single binary and a macOS
  app bundle replace `./run`, `./ghostty`, `./preview`, the wheel and the bundle
  installer. The role of TPM is open (LP-1, LP-3, LP-8).
- **Remote use needs a new answer.** The Python tool runs inside an SSH session
  on the tmux host. A desktop app cannot, so remote hosts are an open question
  (RM-1).
- **Same data, read first.** The library path, `layouts.db` schema version 2,
  the shell server socket and session names, and the helper-session marker all
  stay the same. The app reads the store first and writes it only after the
  merge rules land (PE-1, PE-2, PE-14, PE-15, CH-21).
- **No recursive-host case.** The app is never hosted inside a tmux pane, so the
  Python tool's protection against attaching its own host session is not needed
  (CH-30).
- **Performance targets get stricter.** The Python budgets (p50 ≤ 150 ms,
  p95 ≤ 250 ms) become ceilings, and native frame, typing and bulk-output
  measures are added (PF-1, PF-4, PF-6).

## Open questions

1. **CH-3**: What directory does **Open terminal** use when the app was started
   from the Dock or a desktop launcher, and does a command-line launch keep the
   invoking directory?
2. **CH-32**: Inside an attached session, what happens to that session's own
   tmux prefix keys (such as `Ctrl-b`)?
3. **CH-33**: How does the app draw an attached session's current window when
   that window has its own panes, and how does the user switch its windows?
4. **PE-4**: Does the Rust app perform the one-time import of older layout
   tables, or ask the user to open the library once with the Python tool?
5. **PE-21**: Until write support lands, are navigation state and edits kept in
   memory only, or is editing disabled?
6. **SH-2**: How does an app started from the Dock obtain the user's shell
   environment, and what do `TERM` and `COLORTERM` mean without an outer
   terminal?
7. **TH-9**: What does the colour `default` resolve to in a native app, so the
   `plain`, `forest` and `mono` presets still work?
8. **TH-27**: Where do font family, size, fallback glyphs and the terminal
   colour palette live, given that `theme.toml` must stay readable by the
   Python tool?
9. **TH-28**: Does a saved theme apply to every window of the app, or only to
   the window that saved it?
10. **KB-29**: Which modifier do direct shortcuts use by default on Linux, where
    Super is often reserved?
11. **VL-1**: Does the sidebar keep a fixed width, or can the user resize it?
12. **VL-3**: Does the app add a cue for the focused pane?
13. **VL-10**: Are menus drawn inside the sidebar, as in the Python tool, or as
    native context menus and popovers?
14. **SU-2**: Does tmux 3.3 support every control-mode feature the app needs?
15. **SU-5**: How does the app find tmux when it is started without the user's
    `PATH`?
16. **RM-1**: Are remote hosts supported through tmux control mode over SSH, or
    left to the Python tool for now?
17. **RM-6**: How is remote use verified, if it is supported? Depends on RM-1.
18. **RM-7**: Is WSL a target?
19. **LP-2**: How can the Rust app and the Python tool be installed side by
    side when both use the command name `tmux-workspaces`?
20. **LP-4**: Should the app read the user's terminal settings, such as
    Ghostty's font and colours?
21. **LP-8**: What does the TPM launch key do for a native app: open it on the
    same library, or is TPM support dropped?
22. **LP-9**: Do the `@tmux-workspaces-key` and `@tmux-workspaces-data-dir`
    options stay? Depends on LP-8.
23. **LP-10**: Does a TPM launch still read sessions from the hosting server
    and share the standalone library? Depends on LP-8.

[rd-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/README.md#tmux-workspaces
[rd-try]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/README.md#try-it
[rd-first]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/README.md#make-your-first-workspace
[rd-keys]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/README.md#the-keys
[rd-get]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/README.md#what-you-get
[rd-install]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/README.md#install
[g-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#use-tmux-workspaces
[g-tpm]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#launch-from-tmux-with-tpm
[g-organize]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#organize-your-own-terminals
[g-split]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#split-and-navigate-panes
[g-attach]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#attach-an-existing-session
[g-resize]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#resize-and-choose-an-attachment-destination
[g-helper]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#attachment-helper-discovery
[g-ws]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#switch-and-organize-workspaces
[g-settings]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#settings-and-leaving-the-viewer
[g-keys]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#keyboard-shortcuts
[g-iso]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#isolation-and-saved-state
[g-bb]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#optional-backbone-agent-status
[g-limits]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/GUIDE.md#experimental-limits
[ac-platform]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ACCEPTANCE.md#platform-coverage
[ac-checks]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ACCEPTANCE.md#what-the-checks-cover
[ac-perf]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ACCEPTANCE.md#performance-and-visual-checks
[ac-limits]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ACCEPTANCE.md#product-limits
[ar-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#architecture-and-ownership
[ar-modules]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#module-responsibilities
[it-req]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/INITIAL_TASK.md#required-behavior
[ar-life]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#process-and-data-lifetimes
[ar-input]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#input-ordering
[ar-nav]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#navigation-reads-and-attachment-startup
[ar-entry]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#existing-terminals-and-entry-paths
[ar-provider]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#read-only-provider-contract
[ar-valid]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/ARCHITECTURE.md#layout-validation
[sc-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#direct-shortcuts-and-right-click-menus
[sc-tabs]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#tabs-and-splits
[sc-ws]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#workspaces-and-other-controls
[sc-copy]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#selecting-and-copying-terminal-text
[sc-right]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#right-click
[sc-other]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#other-terminals-and-the-existing-launcher
[sc-impl]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#verification-and-implementation-notes
[sc-exec]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#launch-error-exec-exec-not-found
[sc-cover]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#keyboard-coverage-and-customization
[sc-view]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#viewing-and-editing-shortcuts
[sc-keymaps]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHORTCUTS.md#user-keymaps
[th-keeps]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#what-the-terminal-keeps
[th-file]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#the-file
[th-presets]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#presets
[th-grounds]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#the-grounds
[th-roles]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#roles
[th-beyond]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#beyond-the-sidebar
[th-edit]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#editing-the-theme
[th-palettes]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#palettes-and-which-terminal-is-asked
[th-notest]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/THEMES.md#not-established-by-those-tests
[rf-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/REFRESH.md#refresh-the-viewer-safely
[rf-reopen]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/REFRESH.md#refresh-or-reopen
[rf-preserve]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/REFRESH.md#preservation-and-failure-behavior
[rc-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/RECOVERY.md#preserve-and-recover-an-arrangement
[rc-keep]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/RECOVERY.md#keep-the-original
[rc-continue]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/RECOVERY.md#continue-separately
[rc-copy]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/RECOVERY.md#recover-into-a-copy
[rc-valid]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/RECOVERY.md#validation-scope
[su-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/STARTUP.md#startup-requirements-and-errors
[su-term]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/STARTUP.md#terminal-checks
[su-diag]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/STARTUP.md#ghostty-and-diagnostic-commands
[su-verify]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/STARTUP.md#verification
[se-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHELL_ENVIRONMENT.md#ordinary-shell-environment
[se-vars]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHELL_ENVIRONMENT.md#supported-variables
[se-ssh]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/SHELL_ENVIRONMENT.md#reconnecting-over-ssh
[js-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/JSON_SETTINGS.md#edit-settings
[js-save]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/JSON_SETTINGS.md#saving-and-applying
[js-ex]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/JSON_SETTINGS.md#examples
[js-view]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/JSON_SETTINGS.md#view-shortcuts
[ir-use]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/INLINE_RENAME.md#use-it
[ir-sample]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/INLINE_RENAME.md#separate-sample-library
[nm-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/NAMING.md#project-identity-and-compatibility
[nm-compat]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/NAMING.md#compatibility-boundaries
[pf-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#performance-measurement
[pf-run]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#run-on-disposable-terminals
[pf-json]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#what-the-json-measures
[pf-diag]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#command-and-interpreter-diagnostics
[pf-human]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#human-acceptance-on-the-real-terminal
[pf-nav]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#navigation-implementation-and-comparison
[pf-gate]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#release-gate-result
[pf-not]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PERFORMANCE.md#what-this-does-not-establish
[pk-top]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PACKAGING.md#installation-bundles-and-upgrades
[pk-layout]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PACKAGING.md#installed-layout
[pk-install]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PACKAGING.md#install-a-local-bundle
[pk-archive]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PACKAGING.md#build-a-reproducible-source-archive
[pk-upgrade]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PACKAGING.md#upgrade-and-uninstall-contract
[pk-verify]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/PACKAGING.md#verify-an-installation
[up-try]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/UI_PREVIEW.md#try-a-separate-sample
[up-org]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/UI_PREVIEW.md#organization
[up-try2]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/docs/UI_PREVIEW.md#try-these-interactions
[cl]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/CHANGELOG.md#011
[km]: https://github.com/eandualem/tmux-workspaces/blob/51de6757cd0aeda3c19fe68fa4ec5b93ab206804/integrations/keymap.toml
