# Design language

The native app's look is called **Quiet Instrument**: a calm, dark-first
workspace where the terminals are the focus, state is shown by shape before
colour, and the chrome stays out of the way. This page explains how the look
is built, where each part lives, and how to change it.

## Where the design lives

| What | File | Change it to… |
| --- | --- | --- |
| Every colour, size, radius, type style and timing | [`crates/app/assets/design/tokens.toml`](../crates/app/assets/design/tokens.toml) | restyle the app without touching Rust |
| The typed loader and its checks | `crates/app/src/desktop/design.rs` | add a new token |
| Shared pieces (rows, buttons, keycaps, menus, agent symbols…) | `crates/app/src/desktop/components.rs` | change how a component is built |
| The navigation panel | `desktop/sidebar.rs` | rearrange the panel |
| Pane cards, headers, the empty pane | `desktop/pane_cards.rs` | change the panes |
| Content area, empty states, popover placement | `desktop/frame.rs` | change empty states or where popovers open |
| The status row | `desktop/status.rs` | change the bottom row |
| Tab, workspace and icon menus | `desktop/menus.rs` | change menus |
| Jump (Command-K) | `desktop/jump.rs` | change search results |
| The Settings window | `desktop/settings_window.rs` | change Settings pages |
| Icons | `crates/app/assets/icons/`, listed in `desktop/icons.rs` | add an icon |

**The rule:** views use tokens and components, never their own values. A
colour, pixel size, radius or font in a view is a bug: add or reuse a token
instead. Tests check that the bundled `tokens.toml` is complete and well
formed, so a missing or misspelt token fails the build's tests rather than
the app.

### Changing the look

1. Edit `tokens.toml`. Colours are `#rrggbb` or `#rrggbbaa`; sizes are pixels.
2. Run `cargo test -p agent-interface-app design` to check the file.
3. Run the app. Every window uses the new values.

### Adding a token

Add it to `tokens.toml` with a comment saying where it is used, add the field
to its group in `design.rs` (the `group!` lists are in the same order as the
file), then use it from a component or view.

### Adding a component

Write it in `components.rs` from tokens only, document it in the table below,
and use it from the views. Prefer extending an existing component to drawing
a similar one in a view.

## Window layout

```text
┌───────────────────────┬──────────────────────────────────────────────────┐
│ ● ● ●                  │ ╭──────────────────╮ ╭────────────────────────╮  │  titlebar 44
│ ┌──┐ Workspace     ⌃⌄ │ │ ▸ zsh  ~/project 1│ │ git  develop         2 │  │  pane header 28
│ └──┘ 3 tabs            │ │                  │ ╰────────────────────────╯  │
│ [⌕ Jump to…      ⌘K ]  │ │   focused pane   │ ╭────────────────────────╮  │  pane gap 6
│ TABS               +   │ │   (accent ring)  │ │ ⚙ builder  ▸ working 3 │  │
│ 1 native-app      ▣   │ │                  │ │   dimmed when unfocused│  │
│ 2 roster      ●   ▥   │ ╰──────────────────╯ ╰────────────────────────╯  │
│ ─────────────────────  │                                                  │
│ AGENTS      2 need you │                                                  │
│ ! reviewer      roster │                                                  │
│ ◔ builder   native-app │                                                  │
│ ─────────────────────  │                                                  │
│ [◆][★][⌂] 4      [≡]   │                                                  │  switcher 52
├────────────────────────┴──────────────────────────────────────────────────┤
│ Workspace › native-app › pane 1 of 3   ⌘D split  ⌘⇧D below   shown ● Saved │  status 26
└────────────────────────────────────────────────────────────────────────────┘
   panel 232 (drag 140–600)
```

- **Panel** (`sidebar.rs`): titlebar strip with the window controls, the
  workspace block (icon tile, name, tab count, chooser), the jump field, the
  tab list, the agents section (when Agent Backbone is installed) and the
  workspace switcher with Settings. Its right edge drags to resize.
- **Content** (`frame.rs`, `pane_cards.rs`): panes as rounded cards on the
  window's `ground`, separated by `pane_gap`, which is also the resize handle.
  The focused card has a `focus` border and a `focus_halo` glow; the others
  have an `edge` border and their content dimmed to `pane_dim`. A pane whose
  agent needs you has an `attention` border.
- **Pane header**: what runs there, where, and the pane's number. Settings can
  show it always, only on hover (overlaid), or never.
- **Status row** (`status.rs`): breadcrumb on the left, the current mode's keys
  as keycaps in the centre, the agents toggle and the saved state on the right.
  A failed save shows its error with Retry saving and Discard.
- **Popovers** (`frame.rs`): menus and Jump open where they were invoked, over
  everything, and close on Escape or a click outside.

## Colour tokens

Each theme defines the same names (`[color.dark]`, `[color.light]` in
`tokens.toml`). Views read the current theme through `design::colors()`.

| Token | Role |
| --- | --- |
| `ground` | Window, behind and between the panes |
| `panel` | Panel, status row, Settings navigation |
| `surface` | Panes and Settings pages |
| `field` | Inputs, keycaps, popovers |
| `hover` | Row and button hover |
| `selected` | Selected row, menu highlight |
| `line`, `line_strong` | Hairlines and rules; field and keycap borders |
| `edge` | Unfocused pane border |
| `text`, `text_2` | Body and selected text; unselected rows and pane titles |
| `muted` | Labels, meta, inactive icons |
| `faint` | Shapes only (idle ring, thumbnails), never text |
| `accent`, `accent_text`, `on_accent`, `accent_soft` | Selection, working and focus; as text; text on an accent fill; tint fills |
| `focus`, `focus_halo` | Focused pane border and focus rings; glow around the focused pane |
| `attention`, `attention_text`, `on_attention`, `attention_soft` | Needs you, destructive actions, errors |
| `ok` | Saved light, connected |
| `info`, `violet`, `green` | Inbound events and cache reads; cache writes; output tokens (Backbone views) |
| `shadow` | Popover shadow |
| `pane_dim` | Opacity of unfocused panes' content |

Terminal defaults per theme (`[terminal.dark]`, `[terminal.light]`): the
default text, background and cursor colours and the 16 ANSI colours. The light
terminal and native readers share a warm off-white background. Darker neutral
gaps and borders separate the panes; an amber outline identifies the focused
pane. Dark keeps Ghostty's
defaults; light is Tomorrow, darkened to pass 4.5:1.

## Type

`[font]`: `ui` (the platform interface font), `mono` (keys, numbers,
terminals on macOS), `mono_linux` (DejaVu Sans Mono for Linux), and the default terminal size. `[text.*]` styles, each a size and
weight, applied with `components::text`:

| Style | Use |
| --- | --- |
| `page_title` | Settings page titles |
| `workspace` | Workspace name in the panel |
| `row`, `row_selected` | Rows and menus; the selected row |
| `status` | Status row, pane headers |
| `meta` | Secondary details on rows |
| `label` | Uppercase section labels (TABS, AGENTS) |
| `key` | Keycaps and numbers, in `mono` |
| `empty_title`, `body` | Empty and error states; explanatory text |
| `figure` | The headline number in a view |

## Spacing, radii and sizes

- `[space]`: a 2 px scale named by value (`s2` … `s24`). Views use these names.
- `[radius]`: by use — `key`, `icon_button`, `menu_item`, `row`, `slot`,
  `pane`, `popover`, `pill`.
- `[size]`: every component dimension, from `titlebar` and `panel_width` to
  `row`, `pane_header`, `status_row`, `slot`, `keycap`, `hairline` and the
  Settings window. Each is commented in `tokens.toml`.

## Components

All in `components.rs`; each takes its look from the tokens.

| Component | Anatomy and states |
| --- | --- |
| `row` | Rounded row; `selected` fill when selected, `hover` fill on hover |
| `section` | Uppercase label with an optional trailing control |
| `icon`, `icon_button` | Lucide icon in one colour; square button, `selected` fill on hover |
| `keycap`, `key_hint` | A key in a cap (status row, Jump field); a key at a row's end |
| `button` | `Primary` (accent fill), `Secondary` (field with border), `Danger` (attention text) |
| `dot` | Small filled mark: needs you on tabs and slots, the saved light |
| `menu`, `menu_item`, `menu_heading`, `menu_rule` | Popover frame on `field` with shadow; item with mark slot, label and key; uppercase heading; rule |
| `agent_symbol` | Needs you: filled `attention` circle with "!". Working/starting: rotating `accent` arc with a play mark (`motion.spinner_ms` per turn). Idle: `faint` ring. Unknown: dashed `muted` ring with "?" |
| `minimap` | A tab's layout thumbnail: one frame, split once, or split twice |
| `empty_tile`, `empty_state` | Icon tile, title, explanation and actions, centred |
| `field_frame` | Text field frame, `accent` border when focused |
| `view_heading`, `view_toolbar`, `view_footer` | A view's uppercase section label with a note; its control row; its bottom row of keys and a note |
| `chip`, `segments`, `segment` | Filter pill, filled when on; a segmented control's frame and its choices |
| `meter`, `stacked_meter`, `legend` | A bar filled to a share; a bar split by shares; a swatch, label and value |
| `badge` | Small mono label, such as an agent's CLI |
| `view_choice` | A row of the new-pane views list: icon tile, label, description, scope |
| `fact` | A label and value, one line of an agent's card |
| `choice_button`, `filter_frame`, `choice_frame`, `choice_key`, `hint_bar` | The chooser's two buttons, filter, selectable row with its ring, number key and centred key row |
| `view_header`, `back_button`, `pill`, `info_card` | A view's 28 px header; the reading view's way back; a tag; a summary panel |
| `tag` with `Tone` | How something is delivered: `Plain` for `all` and plain facts, `Tag` in the accent tint for a tag, `Quiet` outline for a direct assignment; a record's state: `Tag` for active, `Attention` for waiting on the person, `Done` for complete |
| `grouped_page`, `group_title`, `group_label`, `group_card`, `group_row`, `group_note` | How a view lays out a list people browse: a padded page with a centred column no wider than `grouped_width`; a major section's title (`group_title` text) and one line saying what it holds; a subgroup's label, which may be a `tag`; rows on a rounded `panel` card, one line each, `group_row` tall, separated by hairlines, with a name, meta on the right and a chevron; a quiet line for what a section does not list |
| `number_tile` | A round tile with a position, such as a policy's place in a brief |
| `done_symbol`, `live_note` | A finished record's `ok` ring with a check; the header's "live from Agent Backbone" note |
| `record_row`, `question_row` | An update on a `group_card`: state mark, mono author (`record_who` wide), summary, state `tag`, age (`record_age`) and chevron; an answer: a `?` tile, the question over who answered and how the answer begins, an optional tag, age and chevron, `question_row` tall |
| `shelf`, `shelf_card` | Documents three to a line: a file tile and tags, the title on up to two lines, author, age and size; at least `shelf_card` tall |
| `reader_column`, `reader_label`, `reader_section`, `callout`, `notice`, `reference`, `link` | A record reader's centred column (`reader_width`); an uppercase label; a labelled section with its label in a `reader_label` column under a hairline; a tinted block (`Attention` for what waits on the person, `Follows` for what follows an answer); a notice such as a correction; an authored link shown as text (`reference_row` tall); accent text that acts |
| `conversation_rail`, `conversation_panel`, `conversation_header`, `thread_divider`, `message`, `message_meta`, `anchor`, `composer`, `send_button`, `sending_spinner` | The one conversation beside every reader. Closed: a `conversation_rail` wide rail with its button, the agent's mark and a label, a dot for new replies. Open: a `conversation_width` panel with the agent and subject, the thread (day dividers; the person's messages on the right on `selected`, an agent's on the left on `surface`, `bubble` corners with a `key` tail; a dashed `attention` outline for one not sent) and the composer (field, `anchor` for what the message refers to, ⌘↵ and `send_button`) |
| `tag_card` | A tag on the left (`tag_card_label` wide) with how many agents carry it; labelled lines of mono names on the right (agents, skills, policies), a dash when empty; `panel` fill for `all` |

## Backbone views

A pane can show a view instead of a terminal.
An empty pane shows the new-pane chooser (`desktop/new_pane.rs`): Open
terminal, **Reports & updates** and **Skills, usage, team…**, a filter, Backbone
agents then other tmux sessions, the six Reports & updates sections and the
views list. Tab and Shift-Tab move between the lists; a chosen section opens
once and is not saved with the pane. Each view lives in `desktop/views/` and
follows its design board with its own header (`view_header`), controls, list
and footer; a view pane has no generic pane header.
A view keeps its last data when Backbone stops answering and says so in a
band at the bottom.

Views follow four rules:

- **A view is a pane.** It never opens or takes over another pane by itself;
  only an explicit choice such as ⌥-click in Team opens one beside it.
- **List, then document, in the same pane.** Choosing a skill or policy
  replaces the list with the reader (`views/reader.rs`): a `back_button` named
  after where it returns ("Skills", "Policies", "worker's brief"), the
  document's place in that list, Rendered or Source, a summary `info_card` and
  the text. Esc goes back; `[` and `]` move through the list; S shows the
  source.
- **One agent's things live in the agent view.** Its Skills, Policies and
  Usage are sections of the Agent view. The team-wide Skills, Policies and
  Tags views never name agents in their rows; they say how a thing is
  delivered (layer, tag, `all`) and how many agents that reaches.
- **Read only.** Every view only reads; nothing approves, tags, edits or
  starts anything.
- **Group, then list.** A list people browse is grouped: sections for what
  kind of thing it is, subgroups for where it comes from or who receives it,
  and one line per row. Descriptions belong in the reader, not the list.
  Team groups agents by workspace folder, with an agent whose folder is named
  `coordinator` first and the others beneath it.

Agent states always pair a shape with a colour, so they read without colour.
Backbone's states use four visual categories (`AgentState::from_report`):
attention (`waiting_for_human`, `blocked`), active (`busy`, `starting`), idle
(`idle`, `running`) and unknown. Labels preserve the difference between
blocked and waiting, and between starting and working. The active arc rotates;
other indicators stay still.

## Interaction patterns

- Right-click opens options for a tab, the workspace block or a slot. A tab's
  layout thumbnail turns into its ⋯ on hover.
- Double-click renames the selected tab or the workspace in place; the status
  row shows the editing keys.
- Command-K opens Jump: tabs, attached panes, agents and commands across every
  workspace; Up and Down move, Enter goes, Escape closes.
- Drag the panel's right edge or the gap between panes to resize; sizes are
  saved when the drag ends.
- Settings (Command-comma or the switcher's sliders) apply at once to every
  window and are saved in `~/.config/agent-interface/settings.toml`.

## Icons

A curated subset of [Lucide](https://lucide.dev) 0.460.0 (ISC licence,
`assets/icons/LICENSE-lucide`), plus this project's `agent-arc` and
`agent-play` shapes. To add one, copy its SVG from the same Lucide release into
`crates/app/assets/icons/` and add its name to `icons!` in `desktop/icons.rs`
(and to `WORKSPACE` if workspaces may choose it).

## Handing over a design

A new direction is quickest to implement when it is expressed in these terms:
token values per theme (new tokens named with their role), components with
their states, and screens of the main window, menus, empty states and each
Settings page. Values that already have a token should reuse its name.
