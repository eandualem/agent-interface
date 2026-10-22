# 0034. Native Settings window

- **Status:** Implemented; desktop visual acceptance pending.
- **Date:** 2026-10-22

## Decision

Settings open in their own native window, from the panel's Settings row, the
application menu or Command-comma. It has four pages:

- **General:** the default library location, the settings file and the version.
  Another library is opened from the menu bar (Command-O).
- **Appearance:** a Dark or Light theme, shown as previews, and the terminal
  text size from 9 to 24 points. Changes apply to every open window at once:
  the panel, menus, status row and terminal colours repaint, and terminals with
  a new text size declare their new cell size to tmux
  ([0032](0032-source-equivalent-chrome-and-sizing.md)).
- **Shortcuts:** sixteen workspace commands (new tab, splits, attach, rename,
  pane, tab and workspace navigation, focus) are configurable: click one and
  press the new keys. A chord needs ⌘, ⌃ or ⌥ so terminal typing is
  unaffected; keys owned by copy, paste, the application menu or numbered
  selection are refused, as is a chord another command uses. Each command can
  be reset, and all can be restored. The window's key handling, tab menu and
  status-row hints read the same table. Numbered selection, copy, paste,
  history and the application menu's keys stay fixed and are listed.
- **Agents:** whether Agent Backbone was found, and how the section is shown.

**Themes.** Dark is the approved look with Ghostty's default terminal colours.
Light uses the previous tool's paper chrome colours with Tomorrow's light
terminal palette, so default terminal text stays readable on white. The
previous tool's other presets only meant "use the terminal's own colours",
which has no meaning once this app is the terminal.

**Storage.** `$XDG_CONFIG_HOME/tmux-workspaces/settings.toml`, else
`~/.config/tmux-workspaces/settings.toml`, with `theme = "dark" | "light"` and
`font_size`, `sidebar_width` once the navigation panel has been dragged
(140 to 600 pixels; the default stays 22 terminal cells), and a `[shortcuts]` table holding only the commands changed
from their defaults (for example `new-tab = "ctrl-alt-t"`). Saved entries load
together, so two commands may swap keys; an unknown command or an unusable or
clashing chord keeps its default. It is read at startup with the existing `toml` dependency
(already pinned for the core's theme and keymap readers, no new licence) and
written atomically through a sibling file and rename. A missing file, unknown
keys and invalid values leave the defaults. The previous tool's `theme.toml`
and keymap are not changed.

**Closing a window.** The panel's old Detach entry becomes the standard Close
Window command (Command-W) in the application menu; sessions keep running.

### Linux defaults

Linux uses Ctrl+Shift for most commands and Ctrl+Page Up/Down for tabs;
Alt+1–9 selects tabs. These follow familiar
[GNOME Terminal bindings](https://help.gnome.org/gnome-terminal/adv-keyboard-shortcuts.html).
Workspace navigation uses Ctrl+Shift+Page Up/Down. New and rename workspace
use Ctrl+Alt+Shift+N/R to remain distinct from New Window and Rename Tab.
The full table is in the [README](../../README.md#linux-keyboard-shortcuts).

Keeping Command mapped to Super was rejected because desktop environments own
many Super combinations. Translating workspace navigation to Ctrl+Alt+arrows
was also rejected: [GNOME reserves those for desktop workspaces](https://help.gnome.org/gnome-help/shell-workspaces-switch.html).
These references were checked on 22 October 2026; customized desktop bindings
may still conflict. Existing configurable commands remain editable. Linux labels
spell out Ctrl, Alt, Shift and Super, and saved chords are not translated.
macOS defaults and labels stay unchanged. This adds no dependency.

## Limits

Per-role colour editing and syncing a changed settings file
from another process into running windows are not implemented.

## Verification

A unit test round-trips the settings file, keeps defaults for invalid values
and saves into a missing directory. Shortcut tests cover parsing and labels,
matching layout-shifted keys such as `}` for Command-Shift-], refusing
reserved, bare and clashing chords, saving only changes and loading swaps. Live repainting and resizing on desktop are
not yet accepted on screen.
