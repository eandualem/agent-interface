//! Activity: recent deliveries to agents and events Backbone received,
//! newest first, filtered by kind.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative};
use tmux_workspaces_roster::views::{Activity, Entry};

use super::{Records, ViewPane, line, list, mono, since};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

fn held(entry: &Entry) -> bool {
    matches!(
        entry.outcome.as_str(),
        "queued" | "held" | "expired" | "stored" | "agent_working" | "deferred"
    )
}

pub(super) fn body(pane: &ViewPane, activity: &Activity, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header = ui::view_header("activity", "Activity")
        .child("deliveries and events, all agents")
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s6))
                .child(ui::dot(if pane.error.is_none() { c.ok } else { c.muted }))
                .child(if pane.error.is_none() {
                    "live"
                } else {
                    "stale"
                }),
        );
    let mut filters = ui::view_toolbar().gap(px(t.space.s6));
    for (choice, label) in [
        (Records::All, "All"),
        (Records::Deliveries, "Deliveries"),
        (Records::GitHub, "GitHub"),
        (Records::Held, "Held or expired"),
    ] {
        filters = filters.child(
            ui::chip(
                SharedString::from(format!("activity-{label}")),
                label,
                choice == pane.records,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.records = choice;
                cx.notify();
            })),
        );
    }
    let mut rows = list("activity-rows");
    for entry in activity.entries.iter().filter(|entry| match pane.records {
        Records::All => true,
        Records::Deliveries => !entry.inbound,
        Records::GitHub => entry.inbound,
        Records::Held => held(entry),
    }) {
        let outcome = match entry.outcome.as_str() {
            "delivered" | "handed_off" => c.ok,
            "expired" | "failed" | "rejected" => c.attention_text,
            _ if held(entry) => c.accent_text,
            _ => c.muted,
        };
        rows = rows.child(
            ui::text(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.row))
                    .px(px(t.space.s6))
                    .border_b(px(t.size.hairline))
                    .border_color(c.line),
                &t.text.status,
            )
            .child(
                mono(since(entry.at))
                    .flex_shrink_0()
                    .w(px(t.size.pane_header + t.space.s16))
                    .text_color(c.muted),
            )
            .child(
                mono(entry.kind.clone())
                    .flex_shrink_0()
                    .w(relative(0.13))
                    .text_color(if entry.inbound { c.info } else { c.text_2 }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(relative(0.24))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.text_2)
                    .child(line(&entry.route)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(relative(0.12))
                    .flex()
                    .items_center()
                    .gap(px(t.space.s6))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(outcome)
                    .child(ui::dot(outcome))
                    .child(entry.outcome.replace('_', " ")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(line(&entry.preview)),
            ),
        );
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(filters)
        .child(rows)
        .child(ui::view_footer(&[], "recent records, newest first"))
        .into_any_element()
}
