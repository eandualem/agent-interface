//! Supply a caption only when a Linux compositor requires client decorations.
//! WSLg's Wayland compositor can omit server decorations entirely.

use gpui::{Decorations, Div, IntoElement, MouseButton, ResizeEdge, Window, div, prelude::*, px};

use super::{
    components as ui,
    design::{colors, tokens},
};

pub(super) fn frame(content: impl IntoElement, title: &'static str, window: &Window) -> Div {
    let client = cfg!(target_os = "linux")
        && matches!(window.window_decorations(), Decorations::Client { .. })
        && !window.is_fullscreen();
    let (t, c) = (tokens(), colors());
    let mut frame = div().relative().size_full().flex().flex_col();
    if client {
        frame = frame.child(
            div()
                .id("window-caption")
                .flex()
                .items_center()
                .flex_shrink_0()
                .h(px(t.size.titlebar))
                .pl(px(t.space.s12))
                .bg(c.panel)
                .text_color(c.text_2)
                .font_family(t.font.ui.clone())
                .text_size(t.text.status.size())
                .on_mouse_down(MouseButton::Left, |event, window, _| {
                    if event.click_count == 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                })
                .child(div().flex_1().child(title))
                .child(button("window-minimize", "minus", |window| {
                    window.minimize_window()
                }))
                .child(button(
                    "window-maximize",
                    if window.is_maximized() {
                        "layers"
                    } else {
                        "square"
                    },
                    |window| window.zoom_window(),
                ))
                .child(button("window-close", "x", |window| window.remove_window())),
        );
    }
    frame = frame.child(div().flex_1().min_h_0().w_full().child(content));
    if client && !window.is_maximized() {
        // Client decorations also own edge resizing. Corners are painted last.
        for (id, edge) in [
            ("resize-top", ResizeEdge::Top),
            ("resize-bottom", ResizeEdge::Bottom),
            ("resize-left", ResizeEdge::Left),
            ("resize-right", ResizeEdge::Right),
            ("resize-top-left", ResizeEdge::TopLeft),
            ("resize-top-right", ResizeEdge::TopRight),
            ("resize-bottom-left", ResizeEdge::BottomLeft),
            ("resize-bottom-right", ResizeEdge::BottomRight),
        ] {
            let top = matches!(
                edge,
                ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
            );
            let bottom = matches!(
                edge,
                ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
            );
            let left = matches!(
                edge,
                ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
            );
            let right = matches!(
                edge,
                ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
            );
            frame = frame.child(
                div()
                    .id(id)
                    .absolute()
                    .when(top, |el| el.top_0())
                    .when(bottom, |el| el.bottom_0())
                    .when(left, |el| el.left_0())
                    .when(right, |el| el.right_0())
                    .when(top || bottom, |el| el.h(px(t.space.s4)))
                    .when(left || right, |el| el.w(px(t.space.s4)))
                    .when(!left && !right, |el| el.w_full())
                    .when(!top && !bottom, |el| el.h_full())
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.start_window_resize(edge);
                        cx.stop_propagation();
                    }),
            );
        }
    }
    frame
}

fn button(
    id: &'static str,
    label: &'static str,
    action: impl Fn(&mut Window) + 'static,
) -> impl IntoElement {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .h_full()
            .w(px(t.size.titlebar))
            .flex()
            .items_center()
            .justify_center()
            .text_color(c.text_2)
            .hover(|el| el.bg(c.hover))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                action(window);
                cx.stop_propagation();
            })
            .child(ui::icon(label, t.size.icon, c.text_2)),
        &t.text.body,
    )
}
