//! Activity: a timeline of the last hour, one lane per agent with what
//! reached it and its state now, then recent deliveries and events, newest
//! first, filtered by kind.

use gpui::{AnyElement, Context, Rgba, SharedString, div, prelude::*, px, relative};
use tmux_workspaces_roster::views::{Activity, Entry, Member, now};

use super::{Records, ViewPane, line, list, mono, since};
use crate::desktop::{
    components::{self as ui, AgentState},
    design::{colors, tokens},
};

/// How far back the timeline reaches.
const HOUR: f64 = 3_600.0;
/// The most lanes the timeline draws.
const LANES: usize = 10;

/// The colour of an agent's state on the timeline.
fn state_color(member: &Member) -> Rgba {
    let c = colors();
    match (member.state != "offline").then(|| AgentState::from_report(&member.state)) {
        Some(AgentState::NeedsYou) => c.attention,
        Some(AgentState::Working) => c.accent,
        Some(_) => c.line_strong,
        None => c.selected,
    }
}

/// One lane per agent that received something in the last hour or is not
/// idle now, most recently active first: when each delivery arrived, as a
/// share of the hour, and its state now.
fn lanes(activity: &Activity, end: f64) -> Vec<(&Member, Vec<f32>)> {
    let start = end - HOUR;
    let mut lanes: Vec<(&Member, Vec<f32>, f64)> = Vec::new();
    for member in &activity.members {
        let ticks: Vec<f64> = activity
            .entries
            .iter()
            .filter(|entry| entry.agent == member.name)
            .filter_map(|entry| entry.at.filter(|at| *at >= start && *at <= end))
            .collect();
        let active = !matches!(
            AgentState::from_report(&member.state),
            AgentState::Idle | AgentState::Unknown
        ) && member.state != "offline";
        if ticks.is_empty() && !active {
            continue;
        }
        let latest = ticks
            .iter()
            .copied()
            .fold(if active { end } else { start }, f64::max);
        let shares = ticks
            .iter()
            .map(|at| ((at - start) / HOUR) as f32)
            .collect();
        lanes.push((member, shares, latest));
    }
    lanes.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.name.cmp(&b.0.name)));
    lanes
        .into_iter()
        .take(LANES)
        .map(|(member, ticks, _)| (member, ticks))
        .collect()
}

fn timeline(activity: &Activity) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let name_width = t.size.tag_card_label + t.space.s24;
    let lanes = lanes(activity, now() as f64);
    let mut axis = div().flex_1().flex().justify_between();
    for label in ["60 min ago", "45", "30", "15", "now"] {
        axis = axis.child(label);
    }
    let mut section = ui::text(
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .px(px(t.space.s18))
            .py(px(t.space.s14))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted),
        &t.text.meta,
    )
    .child(
        div()
            .flex()
            .items_center()
            .child(
                ui::text(div().w(px(name_width)).text_color(c.text_2), &t.text.label)
                    .child("LAST HOUR"),
            )
            .child(axis),
    );
    if lanes.is_empty() {
        section =
            section.child("Nothing reached an agent in the last hour, and every agent is idle.");
    }
    for (member, ticks) in lanes {
        let mut track = div()
            .relative()
            .flex_1()
            .h(px(t.space.s12))
            .rounded(px(t.radius.key))
            .bg(c.panel)
            .overflow_hidden();
        for share in ticks {
            track = track.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(relative(share.clamp(0.0, 0.995)))
                    .w(px(t.space.s2))
                    .h_full()
                    .bg(c.text_2),
            );
        }
        // Its state now, at the end of the lane. Backbone does not yet keep
        // earlier states, so the lane does not claim them.
        track = track.child(
            div()
                .absolute()
                .top(px(0.0))
                .right(px(0.0))
                .w(relative(0.03))
                .h_full()
                .bg(state_color(member)),
        );
        section = section.child(
            div()
                .flex()
                .items_center()
                .h(px(t.space.s20))
                .child(
                    mono(member.name.clone())
                        .w(px(name_width))
                        .pr(px(t.space.s10))
                        .text_color(c.text_2),
                )
                .child(track),
        );
    }
    let swatch = |color: Rgba, label: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .child(div().size(px(t.space.s8)).rounded(px(t.space.s2)).bg(color))
            .child(label)
    };
    section.child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s16))
            .pl(px(name_width))
            .pt(px(t.space.s4))
            .child(swatch(c.text_2, "message delivered"))
            .child(swatch(c.accent, "busy now"))
            .child(swatch(c.attention, "waiting or blocked now"))
            .child(swatch(c.line_strong, "idle now"))
            .child("earlier states need Backbone's state history"),
    )
}

fn held(entry: &Entry) -> bool {
    matches!(
        entry.outcome.as_str(),
        "queued" | "held" | "expired" | "stored" | "agent_working" | "deferred"
    )
}

pub(super) fn body(pane: &ViewPane, activity: &Activity, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header = ui::view_header("activity", "Activity")
        .child("states and deliveries, all agents")
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
                mono(match entry.kind.as_str() {
                    "direct_message" => "message".to_owned(),
                    kind => kind.replace('_', " "),
                })
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
        .child(timeline(activity))
        .child(filters)
        .child(rows)
        .child(ui::view_footer(&[], "recent records, newest first"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_hold_recent_deliveries_and_busy_agents_most_recent_first() {
        let member = |name: &str, state: &str| Member {
            name: name.into(),
            state: state.into(),
            ..Member::default()
        };
        let delivery = |agent: &str, at: f64| Entry {
            agent: agent.into(),
            at: Some(at),
            ..Entry::default()
        };
        let end = 10_000.0;
        let activity = Activity {
            entries: vec![
                delivery("worker", end - 900.0),
                delivery("helper", end - 2.0 * HOUR),
            ],
            members: vec![
                member("worker", "idle"),
                member("helper", "idle"),
                member("builder", "busy"),
            ],
        };
        let lanes = lanes(&activity, end);
        let names: Vec<&str> = lanes.iter().map(|(m, _)| m.name.as_str()).collect();
        assert_eq!(names, ["builder", "worker"]);
        assert_eq!(lanes[1].1, [0.75]);
    }
}
