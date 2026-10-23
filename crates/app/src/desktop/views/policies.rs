//! Policies: one agent's brief, in the order it receives it, then one policy
//! to read with a way back, previous and next, and its source.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::{Instructions, Source};

use super::{ViewPane, list, markdown, mono};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

/// The group a source belongs to, as the design heads them.
fn group(source: &Source) -> (&'static str, &'static str) {
    match source.scope.as_str() {
        "base" => ("BASE", "base.md"),
        "swarm" => ("SWARM ROLE", "only for swarm members"),
        scope if scope.split(", ").any(|part| part == "global") => {
            ("SHARED, IN ORDER", "agents.shared_policy")
        }
        _ => ("BY TAG", "agents.tag_policy"),
    }
}

/// A policy's title: its first heading, else its name.
fn title(source: &Source) -> String {
    source
        .text
        .as_deref()
        .and_then(|text| {
            markdown::parse(text)
                .into_iter()
                .find_map(|block| match block {
                    markdown::Block::Heading(_, text) => Some(text),
                    _ => None,
                })
        })
        .unwrap_or_else(|| source.name.clone())
}

fn reach(source: &Source) -> String {
    match source.scope.as_str() {
        "base" | "global" => "every agent".into(),
        scope => scope.replace("tag:", "from tag "),
    }
}

pub(super) fn body(
    pane: &ViewPane,
    brief: &Instructions,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    match pane.policy.filter(|at| *at < brief.sources.len()) {
        Some(at) => detail(pane, brief, at, cx),
        None => index(brief, cx),
    }
}

fn index(brief: &Instructions, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header = ui::view_header("align-left", "Policies").child(brief.name.clone());
    let toolbar = ui::view_toolbar()
        .gap(px(t.space.s10))
        .px(px(t.space.s14))
        .child(ui::segments().child(ui::segment("policies-brief", "One agent's brief", true)))
        .child(mono(brief.name.clone()).text_color(c.text))
        .child(format!(
            "{} · {}",
            brief.runtime,
            if brief.enabled {
                "injected at start"
            } else {
                "not injected"
            }
        ));
    let mut rows = list("policy-rows");
    let mut previous = "";
    for (at, source) in brief.sources.iter().enumerate() {
        let (heading, note) = group(source);
        if heading != previous {
            rows = rows.child(ui::view_heading(heading, "").child(mono(note).text_color(c.muted)));
            previous = heading;
        }
        rows = rows.child(
            div()
                .id(SharedString::from(format!("policy-{at}")))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(t.space.s12))
                .min_h(px(t.size.slot_row))
                .px(px(t.space.s8))
                .py(px(t.space.s6))
                .rounded(px(t.radius.row))
                .cursor_pointer()
                .hover(|el| el.bg(colors().hover))
                .when(!source.applied, |el| el.opacity(0.45))
                .child(
                    ui::text(
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
                    .child((at + 1).to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s2 + 1.0))
                        .child(mono(source.name.clone()).text_color(c.text))
                        .child(
                            ui::text(
                                div()
                                    .text_color(c.muted)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis(),
                                &t.text.status,
                            )
                            .child(title(source)),
                        ),
                )
                .child(
                    ui::text(div().flex_shrink_0().text_color(c.muted), &t.text.meta)
                        .child(reach(source)),
                )
                .child(ui::icon("chevron-right", t.size.icon - 2.0, c.faint))
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.policy = Some(at);
                    cx.notify();
                })),
        );
    }
    for notice in &brief.notices {
        rows = rows.child(
            ui::text(
                div()
                    .px(px(t.space.s8))
                    .pt(px(t.space.s8))
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(notice.clone()),
        );
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(toolbar)
        .child(rows)
        .child(ui::view_footer(
            &[("click", "read")],
            format!("in the order {} receives them", brief.name),
        ))
        .into_any_element()
}

fn detail(
    pane: &ViewPane,
    brief: &Instructions,
    at: usize,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let source = &brief.sources[at];
    let count = brief.sources.len();
    let header = ui::view_header("align-left", "Policies")
        .child(format!("{} · {}", brief.name, source.name));
    let mut display = ui::segments();
    for (raw, label) in [(false, "Rendered"), (true, "Source")] {
        display = display.child(
            ui::segment(
                SharedString::from(format!("policy-display-{label}")),
                label,
                pane.policy_source == raw,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.policy_source = raw;
                cx.notify();
            })),
        );
    }
    let bar = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s8))
        .h(px(t.size.row + t.space.s10))
        .px(px(t.space.s10))
        .border_b(px(t.size.hairline))
        .border_color(c.line)
        .child(
            ui::back_button("policy-back", format!("{}'s brief", brief.name)).on_click(
                cx.listener(|pane, _, _, cx| {
                    pane.policy = None;
                    cx.notify();
                }),
            ),
        )
        .child(div().flex_1())
        .child(
            ui::icon_button("policy-previous", "chevron-left").on_click(cx.listener(
                move |pane, _, _, cx| {
                    pane.policy = Some(at.saturating_sub(1));
                    cx.notify();
                },
            )),
        )
        .child(
            ui::text(div().text_color(c.muted), &t.text.status)
                .child(format!("{} of {count}", at + 1)),
        )
        .child(
            ui::icon_button("policy-next", "chevron-right").on_click(cx.listener(
                move |pane, _, _, cx| {
                    pane.policy = Some((at + 1).min(count - 1));
                    cx.notify();
                },
            )),
        )
        .child(display);
    let card = ui::info_card().child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s14))
            .text_color(c.muted)
            .child(mono(source.path.clone()).text_color(c.text_2))
            .child(reach(source))
            .child(if source.applied {
                "applied at the next start"
            } else {
                "not applied"
            }),
    );
    let content: AnyElement = match &source.text {
        None => ui::text(div().text_color(c.muted), &t.text.body)
            .child("Backbone could not read this file.")
            .into_any_element(),
        Some(text) if pane.policy_source => ui::text(
            div().font_family(t.font.mono.clone()).text_color(c.text_2),
            &t.text.status,
        )
        .child(text.clone())
        .into_any_element(),
        Some(text) => markdown::render(&markdown::parse(text)).into_any_element(),
    };
    let page = div()
        .id("policy-page")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s16))
        .overflow_y_scroll()
        .px(px(t.space.s24))
        .py(px(t.space.s20))
        .child(card)
        .child(content);
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(bar)
        .child(page)
        .child(ui::view_footer(
            &[
                ("esc", "back to the brief"),
                ("[ ]", "previous, next in order"),
                ("S", "source"),
            ],
            "read-only",
        ))
        .into_any_element()
}
