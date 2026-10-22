//! Native building blocks for the navigation panel: system-font rows, icon
//! buttons and popover menus, in the theme's colors.

use gpui::{Div, FontWeight, SharedString, Stateful, Svg, div, prelude::*, px, rgb, svg};

use super::{icons, theme};

/// The platform's interface font.
pub const FONT: &str = ".SystemUIFont";
/// Row height of lists and menus.
pub const ROW: f32 = 26.0;

/// An icon drawn in `color` at `size` pixels.
pub fn icon(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(icons::path(name))
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(color))
}

/// A square, borderless button showing one icon.
pub fn icon_button(id: impl Into<SharedString>, name: &str, color: u32) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_shrink_0()
        .size(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(|el| el.bg(rgb(theme::ACTIVE)))
        .child(icon(name, 14.0, color))
}

/// A list row inset from the panel's edges, highlighted when `active`.
pub fn row(id: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.0))
        .h(px(ROW))
        .mx(px(6.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .cursor_pointer()
        .when(active, |el| el.bg(rgb(theme::ACTIVE)))
        .when(!active, |el| el.hover(|el| el.bg(rgb(theme::HEADER))))
}

/// A small uppercase heading over a group of rows.
pub fn section(label: &'static str) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .h(px(28.0))
        .px(px(14.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::MUTED))
        .child(label)
}

/// One pane marker for a tab row.
pub fn pane_mark(color: u32) -> Div {
    div().w(px(3.0)).h(px(10.0)).rounded(px(1.5)).bg(rgb(color))
}

/// The frame of a popover menu.
pub fn menu() -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(220.0))
        .p(px(4.0))
        .bg(rgb(theme::HEADER))
        .border_1()
        .border_color(rgb(theme::OUTLINE))
        .rounded(px(8.0))
        .shadow_lg()
        .font_family(FONT)
        .text_size(px(13.0))
        .text_color(rgb(theme::NORMAL))
}

/// One menu choice: an optional icon, the label and a shortcut or value.
pub fn menu_item(
    id: impl Into<SharedString>,
    icon_name: Option<(&str, u32)>,
    label: impl Into<SharedString>,
    color: u32,
    right: Option<String>,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(8.0))
        .h(px(ROW))
        .px(px(8.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(|el| el.bg(rgb(theme::ACTIVE)))
        .child(
            div()
                .w(px(14.0))
                .flex_shrink_0()
                .when_some(icon_name, |el, (name, color)| {
                    el.child(icon(name, 14.0, color))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(rgb(color))
                .child(label.into()),
        )
        .when_some(right, |el, right| {
            el.child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(theme::MUTED))
                    .child(right),
            )
        })
}

/// A heading inside a menu.
pub fn menu_heading(label: impl Into<SharedString>) -> Div {
    div()
        .h(px(24.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::MUTED))
        .child(label.into())
}

/// A thin rule between menu groups.
pub fn menu_rule() -> Div {
    div().my(px(4.0)).h(px(1.0)).bg(rgb(theme::OUTLINE))
}
