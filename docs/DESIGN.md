# Design language

This page describes how the native app looks today and where each part of
that look lives in the code. It is the starting point for design work: a new
design can be specified against the tokens and components below, and then
implemented by changing those same places.

The app has two kinds of surface:

- **Chrome**: the navigation panel, menus, the status row and the Settings
  window. These are native UI in the platform's interface font.
- **Terminals**: the tmux panes. They are drawn on a character-cell grid in a
  monospaced font, with terminal colours. Their layout follows tmux.

## Window layout

```text
┌─────────────────────┬─┬───────────────────────────────────────────────┐
│ ◆ Workspace name  ⌄ │ │ attached-session name (attachments only)    │  header 36 px
│                     │ │                                               │
│ TABS             +  │ │                                               │  section 28 px
│ 1  Editor   ▮▮   ⋯  │ │            terminal panes (tmux)              │  rows 26 px
│ 2  Logs     ▮       │ │        split by 5 px dividers with a          │
│ …                   │ │        1 px outline line                      │
│                     │ │                                               │
│ AGENTS           3  │ │                                               │
│ ● reviewer  needs you│ │                                               │
│ ● builder   working │ │                                               │
│ ○ docs              │ │                                               │
│ ⚙ Settings          │ │                                               │
│ [◆][★][⌂][3][⋯]     │ │                                               │  slots 28 px in a 40 px row
├─────────────────────┴─┴───────────────────────────────────────────────┤
│ Workspace · Tab · pane 1/2      ⌘T new · ⌘D split …    agents shown ● │  status row, one cell high
└───────────────────────────────────────────────────────────────────────┘
  panel: 22 terminal cells by    separator: one cell of surface
  default, draggable 140–600 px  with a 1 px outline line
```

- **Panel** (`crates/app/src/desktop.rs`, the `sidebar` element in
  `Browser::render`). Width: 22 terminal cells, or the dragged
  `sidebar_width`. Order from the top: workspace header, tabs section, tab
  list (scrolls), agents section (when Agent Backbone is installed, at most
  40 % of the window height), Settings row, workspace switcher.
- **Separator**: one terminal cell of the surface colour with a 1 px outline
  line (`theme::Cells::separator`); dragging it resizes the panel.
- **Content**: the selected tab's split tree (`Browser::tree`). Split
  dividers are 5 px of surface with a centred 1 px outline line
  (`split_resize::DIVIDER`).
- **Status row**: full width, one terminal cell high, panel colour, interface
  font at 12 px. Left: workspace, tab and pane. Centre: keys for the current
  mode, hidden when they would meet the sides. Right: the agents toggle, then
  the saved-state light or a message.
- **Popovers**: menus open where they were invoked (`anchored`), over
  everything, and close on Escape or a click outside.

The Settings window (`crates/app/src/desktop/settings_window.rs`) is 680 by
480 px (minimum 560 by 380): a 180 px navigation column on the panel colour,
then a scrolling page on the surface colour with 20 px padding.

## Colour tokens

Defined in `crates/app/src/desktop/theme.rs` as `theme::DARK` and
`theme::LIGHT`; read at render time through `theme::panel()`,
`theme::accent()` and so on, so every window repaints when the theme changes.

| Token | Role | Dark | Light |
| --- | --- | --- | --- |
| `panel` | Panel, status row, Settings navigation | `#15171c` | `#f8f8f8` |
| `surface` | Terminals, separators, Settings pages | `#1b1e24` | `#ffffff` |
| `outline` | Separator lines, borders, menu rules | `#2a2e36` | `#e5e5e5` |
| `header` | Workspace header, popover frames, row hover | `#1e2128` | `#ececec` |
| `active` | Selected row, chosen slot, badges, hover in menus | `#262a33` | `#e8e8e8` |
| `active_text` | Text on `active` | `#ffffff` | `#1f1f1f` |
| `normal` | Body text | `#d4d6db` | `#3b3b3b` |
| `muted` | Secondary text, labels, inactive icons | `#7d828c` | `#6e6e6e` |
| `accent` | Selection markers, current workspace icon, add buttons, working state | `#e0a458` | `#b8651b` |
| `danger` | Destructive items, "needs you" state, errors | `#e06c75` | `#c0392b` |
| `ok` | Saved-state light | `#98c379` | `#2e7d32` |
| `terminal_text` | Default terminal text and cursor | `#ffffff` | `#4d4d4c` |
| `terminal[0..16]` | ANSI colours | Ghostty defaults | Tomorrow (light) |

## Typography

| Use | Font | Size | Weight |
| --- | --- | --- | --- |
| Panel rows, menus, Settings body | System UI (`panel::FONT`) | 13 px | Regular; selected tab Medium |
| Workspace name in the header | System UI | 13 px | Semibold |
| Section labels (TABS, AGENTS), menu headings | System UI | 11 px, uppercase | Semibold |
| Tab numbers, agent state words | System UI | 12 / 11 px | Regular |
| Status row | System UI | 12 px | Regular |
| Settings page titles | System UI | 18 px | Semibold |
| Terminals | Menlo | 13 pt default, 9 to 24 in Settings | Regular, bold from programs |

## Spacing and shape

| Token | Value | Where |
| --- | --- | --- |
| Row height | 26 px (`panel::ROW`) | Tab, agent, Settings rows; menu items |
| Header height | 36 px | Workspace header |
| Section label height | 28 px | TABS, AGENTS |
| Switcher | 28 px slots, 4 px gap, in a 40 px row | Workspace slots |
| Row inset | 6 px outside, 8 px inside, 8 px gap | `panel::row` |
| Label inset | 14 px | `panel::section` |
| Radii | 5 px items, 6 px rows and slots, 8 px popovers, 10 px theme cards | |
| Icon sizes | 14 px in rows and buttons, 16 px in the header, slots and picker | |
| Icon button | 22 px square | `panel::icon_button` |
| Popover | at least 220 px wide, 4 px padding, 1 px outline border, large shadow | `panel::menu` |
| Pane marker | 3 by 10 px bar, 2 px gap, up to six | `panel::pane_mark` |

## Components

Shared building blocks are in `crates/app/src/desktop/panel.rs`; assembled
views are in `desktop.rs`, `desktop/terminal.rs` and
`desktop/settings_window.rs`.

| Component | Code | Anatomy and states |
| --- | --- | --- |
| Row | `panel::row` | Rounded row. Selected: `active` fill. Hover: `header` fill. |
| Icon | `panel::icon` | Lucide SVG drawn as a mask in one colour. |
| Icon button | `panel::icon_button` | Icon centred in 22 px; hover `active`. Used for +, ⋯. |
| Section label | `panel::section` | Uppercase label with optional trailing control. |
| Workspace header | `Browser::render` (`workspace-header`) | Icon (accent), name (semibold, ellipsis), chevron. Click opens the workspace menu; double-click renames in place; right-click opens the menu. |
| Tab row | `Browser::render` (`tab-…`) | Number (muted; accent when selected), name (ellipsis; `active_text` medium when selected), pane markers, ⋯ when selected. Right-click opens the tab menu; double-click on the selected tab's name renames it. |
| Agent row | `Browser::render` (`agent-…`) | 8 px dot: filled `danger` (needs you), filled `accent` (working, starting), `muted` ring (idle and others); name; state word. Click shows the agent's tab. |
| Workspace slot | `Browser::render` (`workspace-slot-…`) | 28 px square with the workspace icon or number. Current: `active` fill and accent icon. Right-click opens its menu; overflow shows ⋯. |
| Popover menu | `panel::menu`, `menu_item`, `menu_heading`, `menu_rule` | Frame on `header`; items with a 14 px icon slot, label, right-aligned key or value. Danger items in `danger`, unavailable items in `muted`. |
| Icon picker | `Browser::icon_picker` | Eight-column grid of 28 px icon buttons in a popover; the chosen icon has the `active` fill. |
| Status row | `Browser::render` (`status`) | See the layout above. |
| Attachment header | `LiveTerminal::render` | One cell row on `panel`: window names (muted, current in `normal`, scrolls sideways) and the session name in `accent` on the right. |
| Empty pane | `Browser::new_pane` | "New pane · what should it run?", Open terminal, ATTACH A SESSION, and a muted footer. |
| Name field | `desktop/name.rs` | Inline text field on `active`, 22 px high; the chooser's filter has a 24 px outlined frame. |
| Session chooser | `desktop/chooser.rs` | Filter field and a list of sessions inside the pane. |
| Settings navigation | `SettingsWindow::render` | Rows with icon and label; the open page is selected. |
| Theme card | `SettingsWindow::appearance` | 180 px card with a miniature of panel and terminal colours; the chosen card has an accent border. |
| Stepper | `SettingsWindow::appearance` | 26 px − and + buttons around the value. |
| Shortcut badge | `SettingsWindow::shortcuts` | Key label on `active`; while recording it reads "Press keys…" on `accent`. Reset link appears for changed keys. |

## Icons

Icons are a curated subset of [Lucide](https://lucide.dev) 0.460.0 (ISC
licence), compiled into the binary from `crates/app/assets/icons/` and listed
in `crates/app/src/desktop/icons.rs`. Interface icons: `chevron-down`, `plus`,
`ellipsis`, `settings`, `x`, `pencil`, `sparkles`, `palette`, `terminal`,
`bot`. Workspaces can choose from the 54 icons in `icons::WORKSPACE`; a
workspace's choice is saved by name in its `symbol` field. To add an icon,
copy its SVG from the same Lucide release into `assets/icons`, add its name to
`icons!` (and to `WORKSPACE` if workspaces may use it).

## Interaction patterns

- Menus open where they were invoked and take every pointer event over them.
  Escape, a click outside, or choosing an item closes them.
- Right-click opens the options for a tab, the workspace header or a slot.
- Double-click renames the selected tab or the workspace in place; Enter
  saves, Escape or clicking elsewhere cancels. The hint row says so.
- Drag the panel separator or a split divider to resize; sizes are saved when
  the drag ends.
- Settings apply at once to every window and are saved in
  `~/.config/tmux-workspaces/settings.toml`.

## Known gaps for a redesign

These are places where the current design is inconsistent or incomplete, and
where a new design can make a decision:

- Spacing and radii are written inline at each use rather than as named
  tokens; only colours and the row height are centralised.
- The status row and attachment header are laid out on the terminal-cell
  grid, while the rest of the chrome uses pixels.
- There are no tooltips, no keyboard movement inside menus, and no focus
  rings on panel controls.
- The light theme reuses the previous tool's paper colours and has had less
  visual tuning than the dark one.
- Agent states use colour plus a short word; there is no icon per state.
- Empty states (no library, no tabs, no agents) are plain text.

## Handing over a new design

A design is easiest to implement when it states, for each of the sections
above: the colour tokens for each theme, the type scale, the spacing and
radius scale, and each component's anatomy with its hover, selected,
disabled, danger and focus states. New icons should name a Lucide icon where
one fits. Screens for the main window with tabs, splits, agents and an open
menu, the empty states, and each Settings page cover the current surface.
