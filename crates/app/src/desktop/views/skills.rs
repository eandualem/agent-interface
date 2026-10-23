//! Skills: the shared store, with the tags and agents each skill reaches.

use gpui::{AnyElement, div, prelude::*, px};
use tmux_workspaces_roster::views::Skills;

use super::{line, list, mono};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

pub(super) fn body(skills: &Skills) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let mut rows = list("skill-rows").child(ui::view_heading("SHARED STORE", ""));
    for skill in &skills.items {
        let reach = match skill.reaches.len() {
            0 => "no agent".to_owned(),
            1 => "1 agent".to_owned(),
            n => format!("{n} agents"),
        };
        let (detail, detail_color) = match &skill.error {
            Some(error) => (line(error), c.attention_text),
            None => (line(&skill.description), c.muted),
        };
        rows = rows.child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(t.space.s12))
                .px(px(t.space.s8))
                .py(px(t.space.s8))
                .rounded(px(t.radius.row))
                .hover(|el| el.bg(colors().hover))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s2))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(t.space.s8))
                                .child(mono(skill.name.clone()).text_color(c.text))
                                .child(
                                    ui::text(div().text_color(c.muted), &t.text.meta)
                                        .child(skill.tags.join(" · ")),
                                ),
                        )
                        .child(
                            ui::text(
                                div()
                                    .text_color(detail_color)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis(),
                                &t.text.status,
                            )
                            .child(detail),
                        ),
                )
                .child(
                    ui::text(div().flex_shrink_0().text_color(c.muted), &t.text.meta).child(reach),
                ),
        );
    }
    let note = match &skills.store {
        Some(store) => format!("store {store} · {} skills", skills.items.len()),
        None => "the skills store is turned off".to_owned(),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(rows)
        .child(ui::view_footer(&[], note))
        .into_any_element()
}
