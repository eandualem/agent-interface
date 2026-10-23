//! Activity: recent deliveries to agents and events Backbone received.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative};
use tmux_workspaces_roster::views::Activity;

use super::{Records, ViewPane, line, list, mono, since};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

pub(super) fn body(
    activity: &Activity,
    records: Records,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let mut toolbar = ui::view_toolbar().gap(px(t.space.s6));
    for (choice, label) in [
        (Records::All, "All"),
        (Records::Deliveries, "Deliveries"),
        (Records::Events, "Events"),
    ] {
        toolbar = toolbar.child(
            ui::chip(
                SharedString::from(format!("activity-{label}")),
                label,
                choice == records,
            )
            .on_click(cx.listener(move |pane, _, _, cx| pane.set_records(choice, cx))),
        );
    }
    let mut rows = list("activity-rows");
    for entry in activity.entries.iter().filter(|entry| match records {
        Records::All => true,
        Records::Deliveries => !entry.inbound,
        Records::Events => entry.inbound,
    }) {
        let outcome = match entry.outcome.as_str() {
            "delivered" | "handed_off" => c.ok,
            "queued" | "stored" | "agent_working" => c.accent_text,
            "expired" | "failed" | "rejected" => c.attention_text,
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
                div()
                    .flex_shrink_0()
                    .w(px(t.size.button + t.space.s6))
                    .text_color(c.muted)
                    .child(since(entry.at)),
            )
            .child(
                mono(entry.kind.clone())
                    .flex_shrink_0()
                    .w(relative(0.16))
                    .text_color(if entry.inbound { c.info } else { c.text_2 }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(relative(0.22))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.text_2)
                    .child(line(&entry.route)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(relative(0.14))
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
        .child(toolbar)
        .child(rows)
        .child(ui::view_footer(&[], "recent records, newest first"))
        .into_any_element()
}
