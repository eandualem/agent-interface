//! Reusable pieces of the chrome, drawn only from the design tokens.
//!
//! Views compose these rather than styling elements themselves, so a change
//! to `assets/design/tokens.toml` reaches every use. `docs/DESIGN.md` lists
//! each component and its states.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, BoxShadow, Div, Rgba, SharedString, Stateful, Svg, Transformation,
    div, percentage, point, prelude::*, px, svg,
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
    Working,
    Idle,
    Unknown,
}

impl AgentState {
    /// Fold a reported state onto the four the design shows.
    pub fn from_report(state: &str) -> Self {
        match state {
            "waiting_for_human" | "blocked" => Self::NeedsYou,
            "busy" | "starting" => Self::Working,
            "idle" | "running" => Self::Idle,
            _ => Self::Unknown,
        }
    }

    /// The short word shown beside a working or needs-you agent.
    pub fn word(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs you",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
        }
    }
}

/// An agent's state symbol: shape first, colour second.
pub fn agent_symbol(state: AgentState, id: impl Into<SharedString>) -> Div {
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
        AgentState::NeedsYou => frame
            .rounded_full()
            .bg(c.attention)
            .child(mark("!", c.on_attention)),
        AgentState::Working => {
            let turn = Duration::from_millis(t.motion.spinner_ms as u64);
            frame
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .border(px(t.size.minimap_stroke * 1.5))
                        .border_color(c.accent_soft),
                )
                .child(div().absolute().inset_0().child(
                    icon("agent-arc", size, c.accent).with_animation(
                        id.into(),
                        Animation::new(turn).repeat(),
                        |arc, delta| {
                            arc.with_transformation(Transformation::rotate(percentage(delta)))
                        },
                    ),
                ))
                .child(icon("agent-play", size, c.accent))
        }
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
    }
    .child(label.into())
}

/// A library's layer: a numbered tile, an uppercase label, a mono path and
/// a note on the right saying who it reaches. `quiet` dims the tile, for a
/// layer nobody receives.
pub fn layer_heading(
    number: impl Into<SharedString>,
    label: impl Into<SharedString>,
    path: impl Into<SharedString>,
    note: impl Into<SharedString>,
    quiet: bool,
) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .px(px(t.space.s8))
            .pt(px(t.space.s14))
            .pb(px(t.space.s6))
            .text_color(c.muted)
            .overflow_hidden()
            .whitespace_nowrap(),
        &t.text.meta,
    )
    .child(
        text(
            div()
                .flex_shrink_0()
                .size(px(t.size.layer_tile))
                .rounded(px(t.radius.icon_button))
                .flex()
                .items_center()
                .justify_center()
                .font_family(t.font.mono.clone())
                .bg(if quiet { c.selected } else { c.accent_soft })
                .text_color(if quiet { c.muted } else { c.accent_text }),
            &t.text.meta,
        )
        .child(number.into()),
    )
    .child(text(div().flex_shrink_0().text_color(c.text_2), &t.text.label).child(label.into()))
    .child(
        div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .font_family(t.font.mono.clone())
            .child(path.into()),
    )
    .child(div().flex_1())
    .child(div().flex_shrink_0().child(note.into()))
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

/// A list row that opens something: an optional leading mark such as a
/// [`number_tile`], a mono name with a one-line description under it, pills or
/// a note on the right, and a chevron. `narrow` moves the
/// right side under the description so a name is never cut.
pub fn list_row(
    id: impl Into<SharedString>,
    leading: Option<Div>,
    name: impl Into<SharedString>,
    description: impl Into<SharedString>,
    description_color: Rgba,
    trailing: Div,
    narrow: bool,
) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    let description = text(
        div()
            .text_color(description_color)
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis(),
        &t.text.status,
    )
    .child(description.into());
    let name = div()
        .font_family(t.font.mono.clone())
        .text_color(c.text)
        .overflow_hidden()
        .text_ellipsis()
        .child(name.into());
    let text_column = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s2 + 1.0))
        .child(name)
        .child(description);
    let trailing = text(trailing.flex_shrink_0().text_color(c.muted), &t.text.meta);
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s12))
        .min_h(px(t.size.view_choice - t.space.s2))
        .px(px(t.space.s8))
        .py(px(t.space.s6))
        .rounded(px(t.radius.row))
        .cursor_pointer()
        .hover(|el| el.bg(colors().hover))
        .children(leading)
        .map(|row| {
            if narrow {
                row.child(text_column.child(trailing))
            } else {
                row.child(text_column).child(trailing)
            }
        })
        .child(icon("chevron-right", t.size.icon - 2.0, c.faint))
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

#[cfg(test)]
mod tests {
    use super::AgentState;

    #[test]
    fn reported_states_fold_onto_the_four_the_design_shows() {
        assert_eq!(
            AgentState::from_report("waiting_for_human"),
            AgentState::NeedsYou
        );
        assert_eq!(AgentState::from_report("blocked"), AgentState::NeedsYou);
        assert_eq!(AgentState::from_report("busy"), AgentState::Working);
        assert_eq!(AgentState::from_report("starting"), AgentState::Working);
        assert_eq!(AgentState::from_report("idle"), AgentState::Idle);
        assert_eq!(AgentState::from_report(""), AgentState::Unknown);
        assert_eq!(
            AgentState::from_report("something new"),
            AgentState::Unknown
        );
    }
}
