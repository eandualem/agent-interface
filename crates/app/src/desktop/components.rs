//! Reusable pieces of the chrome, drawn only from the design tokens.
//!
//! Views compose these rather than styling elements themselves, so a change
//! to `assets/design/tokens.toml` reaches every use. `docs/DESIGN.md` lists
//! each component and its states.

use gpui::{
    Animation, AnimationExt, AnyElement, BoxShadow, Div, Rgba, SharedString, Stateful, Svg,
    Transformation, div, percentage, point, prelude::*, px, relative, svg,
};

use super::{
    design::{TextStyle, colors, tokens},
    icons,
};

/// Apply a type style.
pub fn text<E: Styled>(element: E, style: &TextStyle) -> E {
    element.text_size(style.size()).font_weight(style.weight())
}

/// An icon in one colour.
pub fn icon(name: &str, size: f32, color: Rgba) -> Svg {
    svg()
        .path(icons::path(name))
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
}

/// A square button showing one icon; `label` names it for its tooltip.
pub fn icon_button(id: impl Into<SharedString>, name: &str) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    div()
        .id(id.into())
        .flex_shrink_0()
        .size(px(t.size.icon_button))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(t.radius.icon_button))
        .cursor_pointer()
        .text_color(c.muted)
        .hover(|el| el.bg(colors().selected).text_color(colors().text))
        .child(icon(name, t.size.icon, c.muted))
}

/// A list row in the panel: rounded, filled when selected or hovered.
pub fn row(id: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.row))
            .px(px(t.space.s10))
            .rounded(px(t.radius.row))
            .overflow_hidden()
            .whitespace_nowrap()
            .cursor_pointer(),
        &t.text.row,
    )
    .when(selected, |el| el.bg(c.selected))
    .when(!selected, |el| el.hover(|el| el.bg(colors().hover)))
}

/// An uppercase section label with an optional trailing control.
pub fn section(label: &'static str) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(px(t.size.section_label))
            .pl(px(t.space.s18))
            .pr(px(t.space.s10))
            .text_color(c.muted),
        &t.text.label,
    )
    .child(label)
}

/// A key or shortcut in a keycap.
pub fn keycap(label: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex_shrink_0()
            .px(px(t.space.s4))
            .h(px(t.size.keycap))
            .flex()
            .items_center()
            .rounded(px(t.radius.key))
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .bg(c.field)
            .font_family(t.font.mono.clone())
            .text_color(c.text_2),
        &t.text.key,
    )
    .child(label.into())
}

/// A key in the mono key style without a cap, as shown at a row's end.
pub fn key_hint(label: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex_shrink_0()
            .font_family(t.font.mono.clone())
            .text_color(c.muted),
        &t.text.key,
    )
    .child(label.into())
}

/// The kinds of text button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
    Danger,
}

/// A text button.
pub fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: Button,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    let base = text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .h(px(t.size.button))
            .px(px(t.space.s14))
            .rounded(px(t.radius.row))
            .cursor_pointer(),
        &t.text.row,
    );
    match kind {
        Button::Primary => base
            .bg(c.accent)
            .text_color(c.on_accent)
            .font_weight(t.text.row_selected.weight()),
        Button::Secondary => base
            .bg(c.field)
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .text_color(c.text)
            .hover(|el| el.bg(colors().hover)),
        Button::Danger => base
            .text_color(c.attention_text)
            .hover(|el| el.bg(colors().attention_soft)),
    }
    .child(label.into())
}

/// A small filled dot, such as the needs-you mark on a tab or slot.
pub fn dot(color: Rgba) -> Div {
    let t = tokens();
    div()
        .flex_shrink_0()
        .size(px(t.size.dot))
        .rounded_full()
        .bg(color)
}

/// The frame of a popover menu.
pub fn menu() -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_col()
            .min_w(px(t.size.popover_min_width))
            .p(px(t.size.popover_padding))
            .bg(c.field)
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .rounded(px(t.radius.popover))
            .shadow(vec![BoxShadow {
                color: c.shadow.into(),
                offset: point(px(0.0), px(t.size.popover_shadow_offset)),
                blur_radius: px(t.size.popover_shadow_blur),
                spread_radius: px(0.0),
                inset: false,
            }])
            .font_family(t.font.ui.clone())
            .text_color(c.text),
        &t.text.row,
    )
}

/// A menu choice: a mark or icon slot, the label, and a key or value.
pub fn menu_item(
    id: impl Into<SharedString>,
    mark: Option<Svg>,
    label: impl Into<SharedString>,
    right: Option<String>,
) -> Stateful<Div> {
    let t = tokens();
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(t.space.s10))
        .h(px(t.size.row))
        .px(px(t.space.s10))
        .rounded(px(t.radius.menu_item))
        .cursor_pointer()
        .hover(|el| el.bg(colors().selected))
        .child(div().w(px(t.size.icon)).flex_shrink_0().children(mark))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.into()),
        )
        .children(right.map(key_hint))
}

/// A heading inside a menu.
pub fn menu_heading(label: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .h(px(t.size.menu_heading))
            .px(px(t.space.s10))
            .flex()
            .items_center()
            .text_color(c.muted),
        &t.text.label,
    )
    .child(label.into())
}

/// A rule between menu groups.
pub fn menu_rule() -> Div {
    let (t, c) = (tokens(), colors());
    div().my(px(t.space.s4)).h(px(t.size.hairline)).bg(c.line)
}

/// How an agent is doing, as Agent Backbone reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    NeedsYou,
    Blocked,
    Working,
    Starting,
    Idle,
    Unknown,
}

impl AgentState {
    /// Preserve reported states while sharing their visual categories.
    pub fn from_report(state: &str) -> Self {
        match state {
            "waiting_for_human" => Self::NeedsYou,
            "blocked" => Self::Blocked,
            "busy" => Self::Working,
            "starting" => Self::Starting,
            "idle" | "running" => Self::Idle,
            _ => Self::Unknown,
        }
    }

    pub fn needs_you(self) -> bool {
        matches!(self, Self::NeedsYou | Self::Blocked)
    }

    pub fn working(self) -> bool {
        matches!(self, Self::Working | Self::Starting)
    }

    /// The short word shown beside a working or needs-you agent.
    pub fn word(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs you",
            Self::Blocked => "blocked",
            Self::Working => "working",
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
        }
    }
}

/// An agent's state symbol: shape first, colour second.
pub fn agent_symbol(state: AgentState) -> Div {
    let (t, c) = (tokens(), colors());
    let size = t.size.agent_symbol;
    let frame = div()
        .flex_shrink_0()
        .size(px(size))
        .relative()
        .flex()
        .items_center()
        .justify_center();
    let mark = |glyph: &'static str, color: Rgba| {
        text(div().text_color(color), &t.text.key)
            .font_weight(gpui::FontWeight::BOLD)
            .child(glyph)
    };
    match state {
        AgentState::NeedsYou | AgentState::Blocked => frame
            .rounded_full()
            .bg(c.attention)
            .child(mark("!", c.on_attention)),
        AgentState::Working | AgentState::Starting => frame
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .border(px(t.size.minimap_stroke * 1.5))
                    .border_color(c.accent_soft),
            )
            .child(turning_arc(size))
            .child(icon("agent-play", size, c.accent)),
        AgentState::Idle => frame.child(
            div()
                .size(px(size - t.space.s4))
                .rounded_full()
                .border(px(t.size.minimap_stroke * 1.5))
                .border_color(c.faint),
        ),
        AgentState::Unknown => frame
            .rounded_full()
            .border(px(t.size.minimap_stroke * 1.5))
            .border_dashed()
            .border_color(c.muted)
            .child(mark("?", c.muted)),
    }
}

/// A tab's layout thumbnail: one frame, split once or more.
pub fn minimap(panes: usize, color: Rgba) -> Div {
    let t = tokens();
    let line = |vertical: bool| {
        let el = div().flex_shrink_0().bg(color);
        if vertical {
            el.w(px(t.size.minimap_stroke)).h_full()
        } else {
            el.h(px(t.size.minimap_stroke)).w_full()
        }
    };
    let frame = div()
        .flex_shrink_0()
        .w(px(t.size.minimap_width))
        .h(px(t.size.minimap_height))
        .rounded(px(t.radius.key / 2.0))
        .border(px(t.size.minimap_stroke))
        .border_color(color)
        .flex();
    match panes {
        0 | 1 => frame,
        2 => frame
            .child(div().flex_1())
            .child(line(true))
            .child(div().flex_1()),
        _ => frame.child(div().flex_1()).child(line(true)).child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .child(div().flex_1())
                .child(line(false))
                .child(div().flex_1()),
        ),
    }
}

/// An icon tile above an empty or error message.
pub fn empty_tile(name: &str, fill: Rgba, color: Rgba) -> Div {
    let t = tokens();
    div()
        .flex_shrink_0()
        .size(px(t.size.empty_icon))
        .rounded(px(t.radius.popover))
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(name, t.size.icon_large + t.space.s2, color))
}

/// A centred empty or error state: tile, title, explanation, actions.
pub fn empty_state(tile: Div, title: impl Into<SharedString>, body: impl IntoElement) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(t.space.s10))
        .p(px(t.space.s24))
        .text_center()
        .child(tile)
        .child(text(div().text_color(c.text), &t.text.empty_title).child(title.into()))
        .child(
            text(
                div().max_w(px(t.size.empty_width)).text_color(c.muted),
                &t.text.body,
            )
            .child(body),
        )
}

/// A text field's frame, outlined in the accent while it has focus.
pub fn field_frame(focused: bool) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s8))
            .h(px(t.size.jump_field))
            .px(px(t.space.s10))
            .rounded(px(t.radius.row))
            .bg(c.field)
            .border(px(t.size.hairline))
            .text_color(c.muted),
        &t.text.body,
    )
    .when(focused, |el| el.border_color(c.accent))
    .when(!focused, |el| el.border_color(c.line))
}

// ---------------------------------------------------------------- views
//
// The pieces Backbone views are built from. Each view composes these, so a
// view never styles its own headings, controls, bars or footers.

/// A view's 28 px header: its icon in the accent and its title. Callers add
/// the subtitle and any controls on the right.
pub fn view_header(name: &str, title: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .cursor(gpui::CursorStyle::OpenHand)
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .h(px(t.size.pane_header))
            .px(px(t.space.s12))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted)
            .overflow_hidden()
            .whitespace_nowrap(),
        &t.text.status,
    )
    .child(icon(name, t.size.icon - 1.0, c.accent))
    .child(
        div()
            .text_color(c.text)
            .font_weight(t.text.row_selected.weight())
            .child(title.into()),
    )
}

/// A back button: a chevron and where it returns to.
pub fn back_button(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s6))
            .h(px(t.size.chip + t.space.s4))
            .pl(px(t.space.s6))
            .pr(px(t.space.s8 + 1.0))
            .rounded(px(t.radius.icon_button))
            .bg(c.hover)
            .text_color(c.text_2)
            .cursor_pointer()
            .hover(|el| el.bg(colors().selected)),
        &t.text.status,
    )
    .child(icon("chevron-left", t.size.icon - 1.0, c.text_2))
    .child(label.into())
}

/// A rounded tag, such as a skill's or agent's tag.
pub fn pill(label: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex_shrink_0()
            .px(px(t.space.s6 + 1.0))
            .rounded(px(t.radius.pill))
            .bg(c.selected)
            .text_color(c.text_2),
        &t.text.meta,
    )
    .child(label.into())
}

/// How a [`tag`] pill reads: what delivers a thing to an agent.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Everyone, or a plain fact such as a CLI: neutral fill.
    Plain,
    /// A tag: the accent tint.
    Tag,
    /// Given by name, or otherwise secondary: an outline only.
    Quiet,
    /// Waiting on the person: the attention tint.
    Attention,
    /// Finished: an outline with the text in `ok`.
    Done,
}

/// A pill naming how something is delivered: `all`, a tag, `direct`.
pub fn tag(label: impl Into<SharedString>, tone: Tone) -> Div {
    let (t, c) = (tokens(), colors());
    let pill = text(
        div()
            .flex_shrink_0()
            .px(px(t.space.s6 + 1.0))
            .rounded(px(t.radius.pill))
            .border(px(t.size.hairline))
            .whitespace_nowrap(),
        &t.text.meta,
    );
    match tone {
        Tone::Plain => pill
            .bg(c.selected)
            .border_color(c.selected)
            .text_color(c.text),
        Tone::Tag => pill
            .bg(c.accent_soft)
            .border_color(gpui::transparent_black())
            .text_color(c.accent_text),
        Tone::Quiet => pill.border_color(c.line_strong).text_color(c.muted),
        Tone::Attention => pill
            .bg(c.attention_soft)
            .border_color(gpui::transparent_black())
            .text_color(c.attention_text),
        Tone::Done => pill.border_color(c.line_strong).text_color(c.ok),
    }
    .child(label.into())
}

/// A tag's card: the tag and how many agents carry it on the left, and
/// labelled lines of what it brings on the right. `lines` pairs a label with
/// mono names; an empty list shows a dash.
pub fn tag_card(
    id: impl Into<SharedString>,
    tag_pill: Div,
    carried: impl Into<SharedString>,
    lines: &[(&'static str, Vec<String>)],
    special: bool,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    let mut right = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s6));
    for (label, names) in lines {
        right = right.child(
            div()
                .flex()
                .items_baseline()
                .gap(px(t.space.s8))
                .child(
                    text(
                        div()
                            .flex_shrink_0()
                            .w(px(t.size.view_choice + t.space.s6))
                            .text_color(c.muted),
                        &t.text.label,
                    )
                    .child(*label),
                )
                .child(
                    text(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .gap_x(px(t.space.s10))
                            .gap_y(px(t.space.s4))
                            .font_family(t.font.mono.clone())
                            .text_color(if names.is_empty() { c.muted } else { c.text_2 }),
                        &t.text.meta,
                    )
                    .when(names.is_empty(), |el| el.child("—"))
                    .children(names.iter().map(|name| div().child(name.clone()))),
                ),
        );
    }
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .gap(px(t.space.s14))
        .px(px(t.space.s12))
        .py(px(t.space.s10 + 1.0))
        .rounded(px(t.radius.slot))
        .border(px(t.size.hairline))
        .border_color(c.line_strong)
        .when(special, |el| el.bg(c.panel))
        .cursor_pointer()
        .hover(|el| el.bg(colors().hover))
        .child(
            div()
                .flex_shrink_0()
                .w(px(t.size.tag_card_label))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(t.space.s4))
                .child(tag_pill)
                .child(text(div().text_color(c.muted), &t.text.meta).child(carried.into())),
        )
        .child(right)
}

// ---------------------------------------------------------------- grouped lists
//
// How a view lays out a list people browse: a padded page, major sections
// with a title, subgroups with a small label, and rows on rounded cards. A
// row holds one line; longer text belongs in the reader.

/// The scrolling page of a grouped list: padded, with a centred column no
/// wider than reads well. Add sections to the returned column.
pub fn grouped_page(id: impl Into<SharedString>, column: Div) -> Stateful<Div> {
    let t = tokens();
    div()
        .id(id.into())
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(px(t.space.s20))
        .pt(px(t.space.s8))
        .pb(px(t.space.s24))
        .flex()
        // The column keeps its own height so the page can scroll through it.
        .items_start()
        .justify_center()
        .child(
            column
                .w_full()
                .max_w(px(t.size.grouped_width))
                .flex()
                .flex_col(),
        )
}

/// A major section of a grouped list: its title and, under it, one line
/// saying what the section holds.
pub fn group_title(title: impl Into<SharedString>, note: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    let note: SharedString = note.into();
    div()
        .flex()
        .flex_shrink_0()
        .flex_col()
        .gap(px(t.space.s2))
        .px(px(t.space.s4))
        .pt(px(t.space.s20))
        .pb(px(t.space.s8))
        .child(text(div().text_color(c.text), &t.text.group_title).child(title.into()))
        .when(!note.is_empty(), |el| {
            el.child(text(div().text_color(c.muted), &t.text.status).child(note))
        })
}

/// A subgroup's label above its card: a label, which may be a pill, and a
/// note after it.
pub fn group_label(label: impl IntoElement, note: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .px(px(t.space.s4))
            .pt(px(t.space.s12))
            .pb(px(t.space.s6))
            .text_color(c.muted)
            .overflow_hidden()
            .whitespace_nowrap(),
        &t.text.status,
    )
    .child(label)
    .child(
        div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .child(note.into()),
    )
}

/// The rounded card a subgroup's rows sit on.
pub fn group_card() -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .flex_col()
        .rounded(px(t.radius.popover))
        .bg(c.panel)
        .border(px(t.size.hairline))
        .border_color(c.line)
        .overflow_hidden()
}

/// One line on a [`group_card`]: an optional leading mark, a name, what it
/// says on the right and a chevron. Rows after the first are separated by a
/// hairline.
pub fn group_row(
    id: impl Into<SharedString>,
    first: bool,
    leading: Option<AnyElement>,
    name: impl Into<SharedString>,
    trailing: impl IntoElement,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.group_row))
            .px(px(t.space.s14))
            .when(!first, |el| {
                el.border_t(px(t.size.hairline)).border_color(c.line)
            })
            .cursor_pointer()
            .hover(|el| el.bg(colors().hover)),
        &t.text.row,
    )
    .children(leading)
    .child(
        div()
            .flex_shrink_0()
            .max_w(relative(0.6))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_family(t.font.mono.clone())
            .text_color(c.text)
            .child(name.into()),
    )
    .child(
        text(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_end()
                .items_center()
                .gap(px(t.space.s6))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(c.muted),
            &t.text.meta,
        )
        .child(trailing),
    )
    .child(icon("chevron-right", t.size.icon - 2.0, c.faint))
}

/// A quiet line inside a grouped list, for what a section does not list.
pub fn group_note(note: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex_shrink_0()
            .px(px(t.space.s4))
            .py(px(t.space.s6))
            .text_color(c.muted),
        &t.text.status,
    )
    .child(note.into())
}

/// A round tile holding a position, such as a policy's place in a brief.
pub fn number_tile(number: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex_shrink_0()
            .size(px(t.size.chip))
            .rounded_full()
            .bg(c.hover)
            .flex()
            .items_center()
            .justify_center()
            .font_family(t.font.mono.clone())
            .text_color(c.muted),
        &t.text.meta,
    )
    .child(number.into())
}

/// A tinted panel holding a document's or agent's summary.
pub fn info_card() -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .flex_col()
            .gap(px(t.space.s10))
            .px(px(t.space.s14))
            .py(px(t.space.s12))
            .rounded(px(t.radius.slot))
            .bg(c.panel)
            .border(px(t.size.hairline))
            .border_color(c.line),
        &t.text.status,
    )
}

/// A view's section heading: an uppercase label, and a note on the right.
pub fn view_heading(label: impl Into<SharedString>, note: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_end()
            .justify_between()
            .gap(px(t.space.s8))
            .h(px(t.size.pane_header))
            .px(px(t.space.s8))
            .pb(px(t.space.s6))
            .text_color(c.muted),
        &t.text.label,
    )
    .child(label.into())
    .child(text(div().overflow_hidden().whitespace_nowrap(), &t.text.meta).child(note.into()))
}

/// The row of controls under a view's header.
pub fn view_toolbar() -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .h(px(t.size.view_toolbar))
            .px(px(t.space.s12))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted),
        &t.text.status,
    )
}

/// A view's bottom row: keys with what they do, then a note on the right.
pub fn view_footer(hints: &[(&'static str, &'static str)], note: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    let mut footer = text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s16))
            .h(px(t.size.view_footer))
            .px(px(t.space.s14))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted)
            .overflow_hidden()
            .whitespace_nowrap(),
        &t.text.meta,
    );
    for (key, action) in hints {
        footer = footer.child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s4))
                .child(key_hint(*key).text_color(c.text_2))
                .child(*action),
        );
    }
    footer.child(div().flex_1()).child(note.into())
}

/// A pill that filters a list; filled while it is on.
pub fn chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(t.size.chip))
            .px(px(t.space.s10))
            .rounded(px(t.radius.pill))
            .border(px(t.size.hairline))
            .cursor_pointer(),
        &t.text.meta,
    )
    .when(on, |el| {
        el.bg(c.selected)
            .border_color(c.selected)
            .text_color(c.text)
    })
    .when(!on, |el| {
        el.border_color(c.line_strong)
            .text_color(c.muted)
            .hover(|el| el.text_color(colors().text))
    })
    .child(label.into())
}

/// The frame of a segmented control; its children are [`segment`]s.
pub fn segments() -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .gap(px(t.size.hairline))
        .p(px(t.space.s2))
        .rounded(px(t.radius.icon_button))
        .bg(c.panel)
}

/// One choice in a segmented control.
pub fn segment(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .items_center()
            .h(px(t.size.chip - t.space.s4))
            .px(px(t.space.s10))
            .rounded(px(t.radius.key))
            .cursor_pointer(),
        &t.text.meta,
    )
    .when(on, |el| el.bg(c.selected).text_color(c.text))
    .when(!on, |el| {
        el.text_color(c.muted)
            .hover(|el| el.text_color(colors().text))
    })
    .child(label.into())
}

/// A short bar filled to `fraction` of its width.
pub fn meter(fraction: f32, color: Rgba) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_1()
        .min_w_0()
        .h(px(t.size.meter))
        .rounded(px(t.size.meter / 2.0))
        .bg(c.selected)
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(gpui::relative(fraction.clamp(0.0, 1.0)))
                .rounded(px(t.size.meter / 2.0))
                .bg(color),
        )
}

/// A bar split into coloured parts by their shares of the whole.
pub fn stacked_meter(parts: &[(f32, Rgba)]) -> Div {
    let t = tokens();
    let total: f32 = parts.iter().map(|(share, _)| share.max(0.0)).sum();
    let mut bar = div()
        .flex()
        .w_full()
        .h(px(t.size.meter_large))
        .gap(px(t.space.s2))
        .rounded(px(t.size.meter_large / 2.0))
        .overflow_hidden();
    for (share, color) in parts {
        if *share > 0.0 && total > 0.0 {
            bar = bar.child(
                div()
                    .h_full()
                    .flex_grow(*share / total)
                    .flex_basis(px(0.0))
                    .bg(*color),
            );
        }
    }
    bar
}

/// A legend entry: a colour swatch, its label and its value.
pub fn legend(color: Rgba, label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .items_center()
        .gap(px(t.space.s6))
        .text_color(c.muted)
        .child(
            div()
                .size(px(t.size.dot + t.space.s2 + 1.0))
                .rounded(px(t.space.s2))
                .bg(color),
        )
        .child(label.into())
        .child(div().text_color(c.text).child(value.into()))
}

/// A small mono badge, such as the CLI an agent runs in.
pub fn badge(label: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex_shrink_0()
            .px(px(t.space.s4 + 1.0))
            .h(px(t.size.keycap))
            .flex()
            .items_center()
            .rounded(px(t.radius.key))
            .bg(c.hover)
            .font_family(t.font.mono.clone())
            .text_color(c.muted),
        &t.text.key,
    )
    .child(label.into())
}

/// A row of the new-pane views list: a glyph tile, a label and description,
/// what it covers and its number key. Selected rows take the accent tile and
/// ring.
pub fn view_choice(
    id: impl Into<SharedString>,
    glyph: &'static str,
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    scope: impl Into<SharedString>,
    key: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    choice_frame(id, selected)
        .h(px(t.size.view_choice))
        .gap(px(t.space.s12))
        .child(
            text(
                div()
                    .flex_shrink_0()
                    .size(px(t.size.view_tile))
                    .rounded(px(t.radius.row))
                    .bg(if selected { c.accent_soft } else { c.selected })
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(t.font.mono.clone())
                    .text_color(if selected { c.accent_text } else { c.text_2 }),
                &t.text.row,
            )
            .child(glyph),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s2))
                .child(text(div().text_color(c.text), &t.text.row_selected).child(label.into()))
                .child(
                    text(
                        div()
                            .text_color(c.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis(),
                        &t.text.meta,
                    )
                    .child(description.into()),
                ),
        )
        .child(text(div().flex_shrink_0().text_color(c.muted), &t.text.meta).child(scope.into()))
        .child(choice_key(key))
}

/// The frame of a chooser row: transparent, or filled with the accent ring
/// when selected.
pub fn choice_frame(id: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s10))
        .h(px(t.size.choice_row))
        .px(px(t.space.s10))
        .rounded(px(t.radius.row))
        .cursor_pointer()
        .when(selected, |el| {
            el.bg(c.selected).shadow(vec![BoxShadow {
                color: c.focus.into(),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(t.size.focus_ring),
                inset: false,
            }])
        })
        .when(!selected, |el| el.hover(|el| el.bg(colors().hover)))
}

/// A chooser row's number key, right-aligned.
pub fn choice_key(key: impl Into<SharedString>) -> Div {
    let t = tokens();
    key_hint(key)
        .w(px(t.size.agent_symbol))
        .flex()
        .justify_end()
}

/// One of the two buttons atop the new-pane chooser: an icon, a label and its
/// key. `on` shows it in the accent while its list is open.
pub fn choice_button(
    id: impl Into<SharedString>,
    name: &str,
    label: impl Into<SharedString>,
    key: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    let color = if on { c.accent_text } else { c.text };
    text(
        div()
            .id(id.into())
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.choice_button))
            .px(px(t.space.s14))
            .rounded(px(t.radius.slot))
            .border(px(t.size.hairline))
            .cursor_pointer()
            .text_color(color),
        &t.text.workspace,
    )
    .font_weight(t.text.row_selected.weight())
    .when(on, |el| el.bg(c.accent_soft).border_color(c.focus))
    .when(!on, |el| {
        el.bg(c.field)
            .border_color(c.line_strong)
            .hover(|el| el.bg(colors().hover))
    })
    .child(icon(
        name,
        t.size.icon + 1.0,
        if on { c.accent_text } else { c.muted },
    ))
    .child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(label.into()),
    )
    .child(key_hint(key).text_color(if on { c.accent_text } else { c.muted }))
}

/// The chooser's filter field frame.
pub fn filter_frame() -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .h(px(t.size.filter))
            .px(px(t.space.s10))
            .rounded(px(t.radius.row))
            .bg(c.panel)
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .text_color(c.muted),
        &t.text.row,
    )
    .child(icon("search", t.size.icon, c.muted))
}

/// A centred row of keys and what they do, then a closing note.
pub fn hint_bar(hints: &[(&'static str, SharedString)], note: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    let mut bar = text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .gap(px(t.space.s16))
            .h(px(t.size.hint_bar))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted)
            .overflow_hidden()
            .whitespace_nowrap(),
        &t.text.meta,
    );
    for (key, action) in hints {
        bar = bar.child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s4))
                .child(key_hint(*key).text_color(c.text_2))
                .child(action.clone()),
        );
    }
    bar.child(note.into())
}

/// A label and its value, one line of an agent's facts.
pub fn fact(label: impl Into<SharedString>, value: impl IntoElement) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .gap(px(t.space.s12))
            .min_h(px(t.size.keycap + t.space.s4)),
        &t.text.body,
    )
    .child(
        div()
            .flex_shrink_0()
            .w(px(t.size.fact_label))
            .text_color(c.muted)
            .child(label.into()),
    )
    .child(div().flex_1().min_w_0().text_color(c.text_2).child(value))
}

// ---------------------------------------------------------------- records
//
// Authored records from Agent Backbone: the rows that list them and the
// pieces their readers are made of.

/// A finished record's mark: an `ok` ring with a check. Other report states
/// use [`agent_symbol`].
pub fn done_symbol() -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_shrink_0()
        .size(px(t.size.agent_symbol))
        .rounded_full()
        .border(px(t.size.minimap_stroke * 1.5))
        .border_color(c.ok)
        .flex()
        .items_center()
        .justify_center()
        .child(icon("step-check", t.size.agent_symbol * 9.0 / 16.0, c.ok))
}

/// A "● live from Agent Backbone" note for the right of a view's header.
pub fn live_note() -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .items_center()
        .gap(px(t.space.s8))
        .child(div().flex_1())
        .child(dot(c.ok))
        .child("live from Agent Backbone")
}

/// A selected row inside a card: the selected fill with an inset focus ring.
fn card_selected<E: Styled>(element: E) -> E {
    let (t, c) = (tokens(), colors());
    element.bg(c.selected).shadow(vec![BoxShadow {
        color: c.focus.into(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(t.size.focus_ring),
        inset: true,
    }])
}

/// A record's age in the mono key style, right-aligned in its column.
fn age(label: impl Into<SharedString>) -> Div {
    let t = tokens();
    key_hint(label)
        .w(px(t.size.record_age))
        .flex()
        .justify_end()
}

/// One update on a [`group_card`]: its state mark, author, summary, a tag
/// for its state, its age and a chevron. An update the person has not opened
/// shows its summary in `text` at the selected weight.
#[allow(clippy::too_many_arguments)]
pub fn record_row(
    id: impl Into<SharedString>,
    first: bool,
    selected: bool,
    unseen: bool,
    mark: Div,
    who: impl Into<SharedString>,
    summary: impl Into<SharedString>,
    state: Div,
    ago: impl Into<SharedString>,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.group_row))
            .px(px(t.space.s14))
            .when(!first, |el| {
                el.border_t(px(t.size.hairline)).border_color(c.line)
            })
            .cursor_pointer()
            .text_color(c.text_2),
        &t.text.row,
    )
    .when(selected, card_selected)
    .when(!selected, |el| el.hover(|el| el.bg(colors().hover)))
    .child(mark)
    .child(
        div()
            .flex_shrink_0()
            .w(px(t.size.record_who))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_family(t.font.mono.clone())
            .text_color(c.text)
            .child(who.into()),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .when(unseen, |el| {
                el.text_color(c.text)
                    .font_weight(t.text.row_selected.weight())
            })
            .child(summary.into()),
    )
    .child(state)
    .child(age(ago))
    .child(icon("chevron-right", t.size.icon - 2.0, c.faint))
}

/// One answer on a [`group_card`]: who answered, the question over how the
/// answer begins, an optional tag, its age and a chevron. An answer the person
/// has not opened shows its question in `text` at the selected weight.
#[allow(clippy::too_many_arguments)]
pub fn question_row(
    id: impl Into<SharedString>,
    first: bool,
    selected: bool,
    unseen: bool,
    who: impl Into<SharedString>,
    question: impl Into<SharedString>,
    answer: impl Into<SharedString>,
    note: Option<Div>,
    ago: impl Into<SharedString>,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    let line = || {
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
    };
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s10))
        .h(px(t.size.question_row))
        .px(px(t.space.s14))
        .when(!first, |el| {
            el.border_t(px(t.size.hairline)).border_color(c.line)
        })
        .cursor_pointer()
        .when(selected, card_selected)
        .when(!selected, |el| el.hover(|el| el.bg(colors().hover)))
        .child(unseen_mark(unseen))
        .child(
            text(
                line()
                    .flex_shrink_0()
                    .w(px(t.size.record_who))
                    .font_family(t.font.mono.clone())
                    .text_color(c.text),
                &t.text.row,
            )
            .child(who.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s2))
                .child(
                    text(
                        line().text_color(if unseen { c.text } else { c.text_2 }),
                        if unseen {
                            &t.text.row_selected
                        } else {
                            &t.text.row
                        },
                    )
                    .child(question.into()),
                )
                .child(text(line().text_color(c.muted), &t.text.status).child(answer.into())),
        )
        .children(note)
        .child(age(ago))
        .child(icon("chevron-right", t.size.icon - 2.0, c.faint))
}

/// A record the person has not opened: an accent dot; otherwise the same
/// space left empty, so rows stay aligned.
pub fn unseen_mark(unseen: bool) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_shrink_0()
        .size(px(t.size.dot))
        .rounded_full()
        .when(unseen, |el| el.bg(c.accent))
}

/// A row of [`shelf_card`]s, three to a line.
pub fn shelf() -> Div {
    let t = tokens();
    div()
        .grid()
        .grid_cols(3)
        .gap(px(t.space.s10))
        .flex_shrink_0()
}

/// A document or workbook on a shelf: its icon on a tile and its tags, the title on up to two
/// lines, and the author, age and size at the bottom.
#[allow(clippy::too_many_arguments)]
pub fn shelf_card(
    id: impl Into<SharedString>,
    name: &str,
    selected: bool,
    tags: Vec<Div>,
    title: impl Into<SharedString>,
    who: impl Into<SharedString>,
    ago: impl Into<SharedString>,
    size: impl Into<SharedString>,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .gap(px(t.space.s10))
        .min_h(px(t.size.shelf_card))
        .p(px(t.space.s14))
        .rounded(px(t.radius.popover))
        .bg(c.panel)
        .border(px(t.size.hairline))
        .border_color(c.line)
        .cursor_pointer()
        .hover(|el| el.bg(colors().hover))
        .when(selected, |el| {
            el.shadow(vec![BoxShadow {
                color: c.focus.into(),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(t.size.focus_ring),
                inset: false,
            }])
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(t.size.view_tile))
                        .rounded(px(t.radius.row))
                        .bg(if selected { c.accent_soft } else { c.selected })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(
                            name,
                            t.size.icon,
                            if selected { c.accent_text } else { c.text_2 },
                        )),
                )
                .child(div().flex_1())
                .children(tags),
        )
        .child(text(div().text_color(c.text).line_clamp(2), &t.text.workspace).child(title.into()))
        .child(
            text(
                div()
                    .mt_auto()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s6))
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(
                div()
                    .font_family(t.font.mono.clone())
                    .text_color(c.text_2)
                    .child(who.into()),
            )
            .child("·")
            .child(ago.into())
            .child(div().flex_1())
            .child(key_hint(size)),
        )
}

// A plan: one card per agent on the list, its steps on a track and what
// they wait on as short tags; an opened plan draws the same steps as a graph.

/// Where a step stands in its plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Done,
    /// Being worked on now.
    Now,
    /// Waiting on someone or something.
    Waits,
    /// Still to come.
    Ahead,
}

/// A step's mark: a check in `ok` once done, the turning arc while now, a
/// faint ring ahead, and "!" on the attention fill while it waits.
pub fn step_dot(step: Step) -> Div {
    let (t, c) = (tokens(), colors());
    let size = t.size.plan_dot;
    let frame = div()
        .flex_shrink_0()
        .size(px(size))
        .relative()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center();
    let ring = |frame: Div, color| {
        frame
            .border(px(t.size.minimap_stroke * 1.5))
            .border_color(color)
    };
    match step {
        Step::Done => ring(frame, c.ok).child(icon("step-check", size * 2.0 / 3.0, c.ok)),
        Step::Now => ring(frame, c.accent_soft).child(turning_arc(size)),
        Step::Ahead => ring(frame, c.faint),
        Step::Waits => frame.bg(c.attention).child(
            text(
                div()
                    .font_family(t.font.mono.clone())
                    .text_color(c.on_attention),
                &t.text.step_mark,
            )
            .child("!"),
        ),
    }
}

/// The accent arc that turns around a working mark of `size`.
fn turning_arc(size: f32) -> Div {
    let t = tokens();
    div()
        .absolute()
        .inset_0()
        .child(icon("agent-arc", size, colors().accent).with_animation(
            "working-arc",
            Animation::new(std::time::Duration::from_millis(t.motion.spinner_ms as u64)).repeat(),
            |arc, delta| arc.with_transformation(Transformation::rotate(percentage(delta))),
        ))
}

/// One step on a plan card's track: its mark and name, done in `muted`, now
/// on the accent tint, ahead in `text_2`, waiting in a dashed attention
/// outline. A long name ends in an ellipsis rather than wrapping the track.
pub fn plan_step(label: impl Into<SharedString>, step: Step) -> Div {
    let (t, c) = (tokens(), colors());
    let pill = text(
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(t.space.s6))
            .h(px(t.size.plan_step))
            .pl(px(t.space.s6))
            .pr(px(t.space.s8 + 1.0))
            .rounded(px(t.size.plan_step / 2.0))
            .border(px(t.size.hairline))
            .border_color(gpui::transparent_black()),
        &t.text.status,
    );
    let pill = match step {
        Step::Done => pill.text_color(c.muted),
        Step::Now => pill
            .bg(c.accent_soft)
            .text_color(c.accent_text)
            .font_weight(t.text.row_selected.weight()),
        Step::Ahead => pill.text_color(c.text_2),
        Step::Waits => pill
            .border_dashed()
            .border_color(c.attention)
            .text_color(c.attention_text),
    };
    pill.child(step_dot(step)).child(
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(label.into()),
    )
}

/// The line from one step on a track to the next: `muted` once the step
/// before it is done, else `line_strong`.
pub fn plan_link(done: bool) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_1()
        .min_w(px(t.space.s10))
        .h(px(t.size.hairline))
        .bg(if done { c.muted } else { c.line_strong })
}

/// What a step waits on, under a card's track: who in bold, then what, then
/// the step it holds, such as "**You** · scope decision → Publish". Waiting
/// on the person takes the attention tint.
pub fn waits_tag(
    who: impl Into<SharedString>,
    what: Option<SharedString>,
    step: impl Into<SharedString>,
    you: bool,
) -> Div {
    let (t, c) = (tokens(), colors());
    let tag = text(
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(t.space.s4 + 1.0))
            .h(px(t.size.plan_tag))
            .px(px(t.space.s8))
            .rounded(px(t.radius.pill))
            .border(px(t.size.hairline))
            .whitespace_nowrap()
            .overflow_hidden(),
        &t.text.meta,
    );
    let (tag, strong) = if you {
        (
            tag.bg(c.attention_soft)
                .border_color(gpui::transparent_black())
                .text_color(c.attention_text),
            c.attention_text,
        )
    } else {
        (
            tag.border_color(c.line_strong).text_color(c.muted),
            c.text_2,
        )
    };
    let mut said = String::new();
    if let Some(what) = what {
        said.push_str(" · ");
        said.push_str(&what);
    }
    said.push_str(" → ");
    said.push_str(&step.into());
    tag.child(
        div()
            .flex_shrink_0()
            .font_weight(t.text.row_selected.weight())
            .text_color(strong)
            .child(who.into()),
    )
    .child(
        div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .child(said),
    )
}

/// A plan's card on the list: hovered, and ringed while selected.
pub fn plan_card(id: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w_full()
        .gap(px(t.space.s8 + 1.0))
        .px(px(t.space.s14))
        .py(px(t.space.s12))
        .rounded(px(t.radius.popover))
        .bg(c.panel)
        .border(px(t.size.hairline))
        .border_color(c.line)
        .cursor_pointer()
        .relative()
        .hover(|el| el.bg(colors().hover))
        .when(selected, |el| el.child(selection_ring(t.radius.popover)))
}

/// The ring around a selected card or node, drawn just outside its border
/// as an outline: a shadow would show through a translucent fill.
fn selection_ring(radius: f32) -> Div {
    let (t, c) = (tokens(), colors());
    let outset = t.size.focus_ring + t.size.hairline;
    div()
        .absolute()
        .top(px(-outset))
        .left(px(-outset))
        .right(px(-outset))
        .bottom(px(-outset))
        .rounded(px(radius + outset))
        .border(px(t.size.focus_ring))
        .border_color(c.focus)
}

/// A node of an opened plan's graph, at its place in the graph: a mark, a
/// key such as DONE or NOW, the step's name over at most two lines and a
/// short note. The caller positions it.
pub fn plan_node(
    id: impl Into<SharedString>,
    step: Step,
    key: impl Into<SharedString>,
    title: impl Into<SharedString>,
    note: Option<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    let frame = text(
        div()
            .id(id.into())
            .absolute()
            .w(px(t.size.plan_node_width))
            .h(px(t.size.plan_node_height))
            .flex()
            .items_start()
            .gap(px(t.space.s8))
            .px(px(t.space.s12))
            .py(px(t.space.s10))
            .rounded(px(t.radius.popover))
            .border(px(t.size.hairline))
            .line_height(px(16.0))
            .cursor_pointer(),
        &t.text.body,
    );
    let (frame, mark, key_color) = match step {
        Step::Done => (
            frame.bg(c.panel).border_color(c.line).text_color(c.text_2),
            done_symbol(),
            c.ok,
        ),
        Step::Now => (
            frame
                .bg(c.accent_soft)
                .border(px(t.size.minimap_stroke * 1.5))
                .border_color(c.accent)
                .text_color(c.text),
            agent_symbol(AgentState::Working),
            c.accent_text,
        ),
        Step::Ahead => (
            frame
                .bg(c.surface)
                .border_color(c.line_strong)
                .text_color(c.text),
            agent_symbol(AgentState::Idle),
            c.muted,
        ),
        Step::Waits => (
            frame
                .bg(c.attention_soft)
                .border_dashed()
                .border_color(c.attention)
                .text_color(c.text),
            agent_symbol(AgentState::NeedsYou),
            c.attention_text,
        ),
    };
    let mut words = div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(
            text(div().text_color(key_color), &t.text.step_key)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(key.into()),
        )
        .child(
            div()
                .font_weight(t.text.workspace.weight())
                .line_clamp(2)
                .text_ellipsis()
                .child(title.into()),
        );
    if let Some(note) = note {
        words = words.child(
            text(div().text_color(c.muted), &t.text.meta)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(note),
        );
    }
    frame
        .child(mark)
        .child(words)
        .when(selected, |el| el.child(selection_ring(t.radius.popover)))
}

/// The arrow from one step of an opened plan's graph to the next: from
/// `(x, y)` rightwards for `length`, its head at the end, in `color`.
pub fn plan_edge(x: f32, y: f32, length: f32, color: Rgba) -> Div {
    let t = tokens();
    let (w, head) = (t.size.plan_edge, t.size.plan_arrow);
    div()
        .absolute()
        .left(px(x))
        .top(px(y - head / 2.0))
        .w(px(length))
        .h(px(head))
        .flex()
        .items_center()
        .child(div().flex_1().h(px(w)).bg(color))
        .child(icon("arrow-head", head, color))
}

// A reader's bar: the revision being read, and actions with a label.

/// An icon and a label that act, such as Copy values in a workbook's bar.
pub fn action(
    id: impl Into<SharedString>,
    name: &str,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s6))
            .h(px(t.size.action))
            .px(px(t.space.s8))
            .rounded(px(t.radius.icon_button))
            .text_color(c.text_2)
            .cursor_pointer()
            .hover(|el| el.bg(colors().selected).text_color(colors().text)),
        &t.text.status,
    )
    .child(icon(name, t.size.icon - 1.0, c.muted))
    .child(label.into())
}

/// One arrow of a [`revision_stepper`]; faded and inert when `enabled` is
/// false.
pub fn revision_step(id: impl Into<SharedString>, name: &str, enabled: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(t.size.revision_step))
        .rounded(px(t.radius.key + 1.0))
        .when(enabled, |el| {
            el.cursor_pointer().hover(|el| el.bg(colors().selected))
        })
        .when(!enabled, |el| el.opacity(0.35))
        .child(icon(name, t.size.icon - 1.0, c.text_2))
}

/// Which revision is being read, between its older and newer arrows, with
/// `latest` marked.
pub fn revision_stepper(
    older: Stateful<Div>,
    label: impl Into<SharedString>,
    latest: bool,
    newer: Stateful<Div>,
) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s6))
            .h(px(t.size.revision_step + t.space.s4))
            .px(px(t.space.s2))
            .rounded(px(t.radius.row))
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .bg(c.field)
            .text_color(c.muted),
        &t.text.status,
    )
    .child(older)
    .child(
        text(
            div().font_family(t.font.mono.clone()).text_color(c.text),
            &t.text.meta,
        )
        .child(label.into()),
    )
    .when(latest, |el| el.child(tag("latest", Tone::Tag)))
    .child(newer)
}

/// The centred column of a record reader, no wider than reads well.
pub fn reader_column() -> Div {
    let t = tokens();
    div()
        .w_full()
        .max_w(px(t.size.reader_width))
        .flex()
        .flex_col()
}

/// An uppercase label in a reader, such as GOAL or YOU ASKED.
pub fn reader_label(label: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(div().flex_shrink_0().text_color(c.muted), &t.text.label).child(label.into())
}

/// A labelled section of a report: the label in its own column and the
/// body beside it, under a hairline.
pub fn reader_section(label: impl Into<SharedString>, body: impl IntoElement) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .gap(px(t.space.s16))
        .py(px(t.space.s14))
        .border_t(px(t.size.hairline))
        .border_color(c.line)
        .child(
            reader_label(label)
                .w(px(t.size.reader_label))
                .pt(px(t.space.s2 + 1.0)),
        )
        .child(
            text(
                div().flex_1().min_w_0().text_color(c.text),
                &t.text.doc_body,
            )
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .child(body),
        )
}

/// What a callout says about its record.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Callout {
    /// The person is needed: the attention tint, with a title.
    Attention,
    /// What follows from an answer: the accent tint, with an icon.
    Follows,
}

/// A tinted block inside a reader.
pub fn callout(kind: Callout, title: Option<&'static str>, content: impl IntoElement) -> Div {
    let (t, c) = (tokens(), colors());
    let frame = div()
        .flex()
        .flex_shrink_0()
        .gap(px(t.space.s10))
        .px(px(t.space.s12))
        .py(px(t.space.s10))
        .rounded(px(t.radius.slot));
    let column = text(
        div().flex_1().min_w_0().text_color(c.text),
        &t.text.doc_body,
    )
    .flex()
    .flex_col()
    .gap(px(t.space.s6));
    match kind {
        Callout::Attention => frame.bg(c.attention_soft).child(
            column
                .children(title.map(|title| {
                    text(div().text_color(c.attention_text), &t.text.meta)
                        .font_weight(t.text.label.weight())
                        .child(title)
                }))
                .child(content),
        ),
        Callout::Follows => frame
            .bg(c.accent_soft)
            .child(div().pt(px(t.space.s2)).child(icon(
                "corner-down-right",
                t.size.icon,
                c.accent_text,
            )))
            .child(column.child(content)),
    }
}

/// A notice above a record, such as a correction: an icon, what happened and
/// an action on the right.
pub fn notice(name: &str, message: impl Into<SharedString>, action: Option<Stateful<Div>>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .px(px(t.space.s12))
            .py(px(t.space.s8 + 1.0))
            .rounded(px(t.radius.slot))
            .bg(c.panel)
            .border(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.text_2),
        &t.text.body,
    )
    .child(icon(name, t.size.icon, c.accent))
    .child(div().flex_1().min_w_0().child(message.into()))
    .children(action.map(link))
}

/// Accent text that acts, such as "Retry" or "Read correction →".
pub fn link(element: Stateful<Div>) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        element
            .flex_shrink_0()
            .cursor_pointer()
            .text_color(c.accent_text),
        &t.text.status,
    )
}

/// An authored link in a report, on one line: its title, and where it
/// leads on the right in `muted`.
pub fn reference(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    target: impl Into<SharedString>,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.reference_row))
            .px(px(t.space.s12))
            .rounded(px(t.radius.row))
            .bg(c.panel)
            .border(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.text)
            .cursor_pointer()
            .hover(|el| el.bg(colors().hover)),
        &t.text.body,
    )
    .child(icon("link-2", t.size.icon, c.muted))
    .child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(title.into()),
    )
    .child(
        text(div().flex_shrink_0().text_color(c.muted), &t.text.meta)
            .whitespace_nowrap()
            .child(target.into()),
    )
}

// ---------------------------------------------------------------- conversation
//
// One conversation beside every authored thing a person can talk about. The
// pieces stay the same whatever the conversation is about; only the anchor
// and the subject line change.

/// A closed conversation: a rail on the reader's right edge with the button
/// that opens it and the agent's mark. `unread` puts a dot on the button.
pub fn conversation_rail(button: Stateful<Div>, mark: Option<Div>, unread: bool) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_shrink_0()
        .w(px(t.size.conversation_rail))
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(t.space.s12))
        .py(px(t.space.s8))
        .border_l(px(t.size.hairline))
        .border_color(c.line)
        .bg(c.panel)
        .child(
            button
                .relative()
                .size(px(t.size.rail_button))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(t.radius.row))
                .cursor_pointer()
                .when(unread, |el| el.bg(c.accent_soft))
                .when(!unread, |el| el.hover(|el| el.bg(colors().selected)))
                .child(icon(
                    "message-square",
                    t.size.icon_large,
                    if unread { c.accent_text } else { c.text_2 },
                ))
                .when(unread, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(t.space.s4 + 1.0))
                            .right(px(t.space.s4 + 1.0))
                            .size(px(t.size.dot + 1.0))
                            .rounded_full()
                            .bg(c.accent)
                            .border(px(t.space.s2))
                            .border_color(c.panel),
                    )
                }),
        )
        .children(mark)
}

/// The frame of an open conversation, `conversation_width` wide.
pub fn conversation_panel() -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_shrink_0()
        .w(px(t.size.conversation_width))
        .h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .border_l(px(t.size.hairline))
        .border_color(c.line)
        .bg(c.panel)
}

/// The top of an open conversation: the agent's mark and name, what the
/// conversation is about and the button that closes it.
pub fn conversation_header(
    mark: Option<Div>,
    agent: impl Into<SharedString>,
    about: impl Into<SharedString>,
    close: Stateful<Div>,
) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s8))
        .h(px(t.size.conversation_header))
        .pl(px(t.space.s14))
        .pr(px(t.space.s8))
        .border_b(px(t.size.hairline))
        .border_color(c.line)
        .children(mark)
        .child(
            text(
                div().font_family(t.font.mono.clone()).text_color(c.text),
                &t.text.row,
            )
            .child(agent.into()),
        )
        .child(text(div().text_color(c.muted), &t.text.meta).child(about.into()))
        .child(div().flex_1())
        .child(close)
}

/// A rule across a thread with a label in it: a day, or how many messages
/// are new (`new` draws it in the accent).
pub fn thread_divider(label: impl Into<SharedString>, new: bool) -> Div {
    let (t, c) = (tokens(), colors());
    let rule = || {
        div()
            .flex_1()
            .h(px(t.size.hairline))
            .bg(if new { c.focus } else { c.line })
    };
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .text_color(if new { c.accent_text } else { c.muted }),
        &t.text.label,
    )
    .font_weight(t.text.meta.weight())
    .child(rule())
    .child(label.into())
    .child(rule())
}

/// Something the artifact did, in its thread: an icon, the agent in mono,
/// what happened, and at most one action at the far end.
pub fn thread_event(
    icon: &'static str,
    actor: impl Into<SharedString>,
    happened: impl Into<SharedString>,
    action: Option<Stateful<Div>>,
) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s6))
            .py(px(t.space.s2))
            .text_color(c.muted),
        &t.text.meta,
    )
    .child(self::icon(icon, t.size.icon, c.muted))
    .child(
        div()
            .font_family(t.font.mono.clone())
            .text_color(c.text_2)
            .child(actor.into()),
    )
    .child(happened.into())
    .child(div().flex_1())
    .children(action.map(|action| action.text_color(c.accent_text).cursor_pointer()))
}

/// Who sent a message and when, above or below its bubble.
pub fn message_meta() -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .text_color(c.muted),
        &t.text.label,
    )
    .font_weight(t.text.meta.weight())
}

/// One message: the person's on the right on `selected`, an agent's on the
/// left on `surface` with a hairline; a message that was not sent has a
/// dashed `attention` outline. `meta` goes above an agent's message and
/// below the person's.
pub fn message(from_you: bool, failed: bool, meta: Div, body: impl IntoElement) -> Div {
    let (t, c) = (tokens(), colors());
    let tail = px(t.radius.key);
    let bubble = text(
        div()
            .px(px(t.space.s10 + 1.0))
            .py(px(t.space.s8))
            .rounded(px(t.radius.bubble))
            .text_color(c.text),
        &t.text.row,
    )
    .child(body);
    let bubble = match (from_you, failed) {
        (_, true) => bubble
            .border(px(t.size.hairline))
            .border_dashed()
            .border_color(c.attention),
        (true, false) => bubble.bg(c.selected).rounded_br(tail),
        (false, false) => bubble
            .bg(c.surface)
            .border(px(t.size.hairline))
            .border_color(c.line)
            .rounded_bl(tail),
    };
    let frame = div()
        .flex()
        .flex_shrink_0()
        .flex_col()
        .gap(px(t.space.s4))
        .max_w(relative(0.9));
    if from_you {
        frame.self_end().items_end().child(bubble).child(meta)
    } else {
        frame.items_start().child(meta).child(bubble)
    }
}

/// What a message refers to, such as a revision or a cell range.
pub fn anchor(label: impl Into<SharedString>) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(t.size.anchor))
            .px(px(t.space.s6))
            .rounded(px(t.radius.key))
            .bg(c.accent_soft)
            .font_family(t.font.mono.clone())
            .text_color(c.accent_text),
        &t.text.key,
    )
    .font_weight(t.text.meta.weight())
    .child(label.into())
}

/// The message box at the bottom of a conversation: the field, then the
/// anchor, the send key and the send button.
pub fn composer(field: impl IntoElement, anchor: Option<Div>, send: Stateful<Div>) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex_shrink_0()
        .border_t(px(t.size.hairline))
        .border_color(c.line)
        .px(px(t.space.s12))
        .pt(px(t.space.s10))
        .pb(px(t.space.s12))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(t.space.s6))
                .p(px(t.space.s8))
                .pl(px(t.space.s10))
                .rounded(px(t.radius.pane))
                .bg(c.field)
                .border(px(t.size.hairline))
                .border_color(c.line_strong)
                .child(field)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .children(anchor)
                        .child(div().flex_1())
                        .child(key_hint("⌘↵"))
                        .child(send),
                ),
        )
}

/// The composer's send button; dimmed while there is nothing to send.
pub fn send_button(id: impl Into<SharedString>, enabled: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    div()
        .id(id.into())
        .flex_shrink_0()
        .size(px(t.size.send_button))
        .rounded(px(t.radius.row))
        .bg(c.accent)
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |el| el.cursor_pointer())
        .when(!enabled, |el| el.opacity(0.45))
        .child(icon("send", t.size.icon - 1.0, c.on_accent))
}

/// A small spinner beside a message that is still sending.
pub fn sending_spinner() -> impl IntoElement {
    let (t, c) = (tokens(), colors());
    let size = t.size.dot + t.space.s4;
    div()
        .size(px(size))
        .child(icon("agent-arc", size, c.accent).with_animation(
            "sending-arc",
            Animation::new(std::time::Duration::from_millis(t.motion.spinner_ms as u64)).repeat(),
            |arc, delta| arc.with_transformation(Transformation::rotate(percentage(delta))),
        ))
}

#[cfg(test)]
mod tests {
    use super::AgentState;

    #[test]
    fn reported_states_keep_distinct_labels_and_visual_categories() {
        assert_eq!(
            AgentState::from_report("waiting_for_human"),
            AgentState::NeedsYou
        );
        assert_eq!(AgentState::from_report("blocked"), AgentState::Blocked);
        assert_eq!(AgentState::from_report("busy"), AgentState::Working);
        assert_eq!(AgentState::from_report("starting"), AgentState::Starting);
        assert!(AgentState::Blocked.needs_you());
        assert!(AgentState::Starting.working());
        assert!(!AgentState::Idle.working());
        assert_eq!(AgentState::Blocked.word(), "blocked");
        assert_eq!(AgentState::Starting.word(), "starting");
        assert_eq!(AgentState::from_report("idle"), AgentState::Idle);
        assert_eq!(AgentState::from_report(""), AgentState::Unknown);
        assert_eq!(
            AgentState::from_report("something new"),
            AgentState::Unknown
        );
    }
}
