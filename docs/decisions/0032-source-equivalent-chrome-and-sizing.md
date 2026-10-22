# 0032. Source-equivalent window chrome and terminal sizing

- **Status:** Implemented; desktop visual acceptance pending.
- **Date:** 2026-10-22

## Context

The native window drew each saved pane at the geometry another client had
given its tmux window and scaled the text down to fit. A session sized by a
large terminal therefore appeared as very small text, and a window with a
different aspect ratio left unused space below it. The panel, status row and
pane headers used their own layout and colours rather than the Python viewer's.

In the Python viewer, every content pane runs an ordinary tmux client (`attach-session -E`, directly or
through its grouped helper), so tmux sizes the session's windows to that pane
under the session's `window-size` option. Without `aggressive-resize`, a client
sizes every window of its session, not only the one it shows. The panel is a
curses program in the terminal font: 22 columns wide, one cell per row, painted
in the theme's colours.

## Decision

**Sizing.** Connections still attach with `no-output,ignore-size`, so connecting,
capture recovery and tests keep the session's geometry. Once a view has bounds,
it declares the whole cells of the unscaled font that fit with
`refresh-client -f '!ignore-size' -C <cols>x<rows>` and repeats this only when
that size changes. From then on the control client counts as an ordinary client
does: under tmux's default `latest` policy, the most recent client sizes the
session. tmux 3.3 accepts this syntax. While another client keeps the window
larger, the view still scales down until tmux reports the new layout. This
settles the question left open in [0003](0003-control-mode-connection-and-attachment-model.md)
the way the Python viewer already does. The app still never changes a source
session's options, detaches its clients or creates sessions.

Setting a per-window size (`-C @id:WxH`) was rejected: once a control client
declares any size, tmux sizes the session's current window from its default
80x24 unless the client size is set as well, so a viewed non-current window
would shrink the source's current one.

**Terminal appearance.** Menlo at 13 points (Ghostty's macOS default size), with
Ghostty's default foreground and 16-colour palette, on the theme's surface
colour. The Python viewer leaves font and program colours to the terminal; the
native app is now that terminal. Ghostty's default font, JetBrains Mono, is not
bundled: that would add a font dependency.

**Chrome.** Use the Python viewer's `default` preset (panel `#15171c`, surface
`#1b1e24`, outline `#2a2e36`, header `#1e2128`, selection `#262a33`, text
`#d4d6db`, secondary `#7d828c`, accent `#e0a458`, danger `#e06c75`, saved light
`#98c379`) and lay the chrome out on the terminal font's cell grid:

- A 22-column panel: the workspace heading on the header bar with its **▾**
  chooser; a blank row, or the inline-rename hint; `TABS` with **+**; one row
  per tab with its number, name, one **▮** per pane (at most six) and, on the
  selected row, **⋯**. Menus replace the tab list under **‹ Back** and `esc`
  on the header bar, with each row's shortcut at its right end.
- The panel's bottom three rows: a blank row, **Configure…**, and the workspace
  slots. Each slot is three cells showing the workspace's icon or number, the
  current one filled; a right-click opens that workspace's options and, when
  the row is full, its last slot, **…**, opens the list. **Configure…** offers
  **Detach** only, because the theme, shortcut and refresh editors are not
  implemented yet.
- One cell of surface with a thin outline line between the panel and the
  content, and thin outline lines between split panes. No pane header or focus
  border.
- A status row along the bottom on the panel colour: the workspace, tab and
  pane on the left; the current mode's keys in the centre, dropped when they
  would meet the sides; the saved-state light, or a message, on the right.
- An attached session has one row above its terminal naming it on the right in
  the accent colour, as the Python viewer's grouped session does. An ordinary
  shell has none. When a session has several windows, their names share that
  row, because control-mode input cannot reach tmux's own window keys. The list
  follows tmux by polling `list-windows` every two seconds while connected.
- An empty pane asks what it should run: **Open terminal**, or **Attach a
  session**, which opens the existing chooser in that pane.

**Library choice.** The Python viewer chooses its library at launch. The native
**Open Library…** moves from the panel to the application menu (Command-O).

## Limits

The status row shows native Command shortcuts rather than the Python prefix
keys. Not yet matched: the agents section, the Configure editors, workspace
icon selection, keyboard movement within menus, Move to workspace as a submenu,
and the Python chooser listing sessions directly in the empty pane. Close pane and Close tab
remain separate work. No screenshot comparison with the Python viewer has been
made, so this record makes no pixel-equivalence claim.

## Verification

A private-server test connects to a two-window session, checks that connecting
alone leaves both windows at their original size, declares 60x20 and waits for
the layout and both windows to follow. Another checks that the window list
follows a new window and rejects a replaced session. Unit tests cover name
truncation and slot labels. No new dependencies or licences are introduced.
