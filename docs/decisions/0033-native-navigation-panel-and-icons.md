# 0033. Native navigation panel and workspace icons

- **Status:** Implemented; desktop visual acceptance pending.
- **Date:** 2026-10-22

## Context

[0032](0032-source-equivalent-chrome-and-sizing.md) reproduced the Python
viewer's layout, drawing the navigation panel on a terminal-cell grid. The
layout and the use of space are kept. This app now replaces the Python tool
entirely, and the reason for a native app is a panel that is native UI
rather than text in terminal cells, with real icons for workspaces.

## Decision

**Panel.** Keep the panel's width (22 terminal cells) and its order: the
workspace heading, the tab list, Settings and the workspace switcher at the
bottom. Draw it in the system font with native rows: a rounded highlight on
the selected tab, pane markers drawn as small bars, icon buttons for adding a
tab and for a tab's options. The terminals, the separator and the status row
keep their cell layout; the status row uses the system font.

**Menus.** The workspace, tab and settings menus open as popovers anchored
where they were opened, so the tab list stays visible. Escape or a click
outside closes them.

**Icons.** Bundle a curated subset of [Lucide](https://lucide.dev) 0.460.0
(`lucide-static` on npm, integrity
`sha512-X6pIdg7jVxv7YQ/uR241hwhNiztcAfmj181TbcX7HCxxk/3mGaRtAc6b2ftUvQBufbJE6ehgyzO2uVsa604tWg==`),
licensed ISC, which is compatible with this project's MIT licence; its notice is
kept in `crates/app/assets/icons/LICENSE-lucide`. The SVG files are compiled
into the binary and served through GPUI's asset source, so no crate dependency
is added. GPUI draws them as masks in the theme's colours. Choose icon… offers
54 workspace icons in a grid, and Show number instead clears it.

**Storage.** A workspace's chosen icon is saved by name in a new optional
`symbol` field: 1 to 40 lowercase letters, digits and hyphens. The existing
one- or two-character `icon` field is unchanged. Older glyph icons are shown
as their native counterparts (for example `★` as `star`). While the Python
viewer may still read the same library, it keeps unknown fields and ignores
`symbol`; clearing the icon clears both fields.

**Agents.** When Agent Backbone is installed (its data directory exists, or it
is configured as decision 0010 describes), each window polls it with the roster
provider and shows an AGENTS section between the tabs and Settings: agents
needing you first, then working agents, then the rest, each with a coloured dot
(red, amber, or a grey ring) and a short state word. Clicking an agent shows the
tab holding its session, preferring the current tab; nothing is attached or
started. The status row's **agents shown** toggle hides the section. Without
Backbone there is no section or toggle.

## Alternatives

Platform symbol fonts (SF Symbols) are macOS-only and their licence restricts
use to Apple platforms; Linux is a target. Phosphor (MIT) and Tabler (MIT)
were comparable; Lucide's stroke style matches the panel's thin separators.
Reusing the one-character `icon` field would limit icons to glyphs.

## Limits

Not yet native: a Settings window with theme and shortcut editing. No
screenshot acceptance has been recorded.

## Verification

Tests check that every offered icon is bundled, that older glyphs map to icons,
and that a `symbol` saves, rejects invalid names and clears together with the
older icon. A unit test checks that an agent resolves to the current tab
showing it, else the first, and only for this window's source.
