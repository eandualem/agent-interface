//! Activity: a timeline of the last hour, one lane per agent with what
//! reached it and its state now, then recent deliveries and events, newest
//! first, filtered by kind.

use gpui::{AnyElement, Context, Rgba, SharedString, div, prelude::*, px, relative};
use tmux_workspaces_roster::views::{Activity, Change, Entry, History, Member, now};

use super::{Records, ViewPane, line, list, mono, since};
use crate::desktop::{
    components::{self as ui, AgentState},
    design::{colors, tokens},
};

/// How far back the timeline reaches.
const HOUR: f64 = 3_600.0;
/// The most lanes the timeline draws.
const LANES: usize = 10;

/// An agent's state now, its word, and whether it is on the timeline
/// without a recent delivery: busy or waiting, not idle, offline or unknown.
fn state_now(member: &Member) -> (Option<AgentState>, &'static str, bool) {
    if member.state == "offline" {
        return (None, "offline", false);
    }
    let state = AgentState::from_report(&member.state);
    let active = matches!(state, AgentState::Working | AgentState::NeedsYou);
    (Some(state), state.word(), active)
}

/// Whether a delivery reached its agent.
fn delivered(entry: &Entry) -> bool {
    matches!(entry.outcome.as_str(), "delivered" | "handed_off")
}

/// An agent's state now, beside its lane: the state symbol and its word, so
/// it reads without colour.
fn state_label(member: &Member) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let (state, word, _) = state_now(member);
    let color = match state {
        Some(AgentState::NeedsYou) => c.attention_text,
        Some(AgentState::Working) => c.accent_text,
        _ => c.muted,
    };
    div()
        .flex_shrink_0()
        .w(px(t.size.fact_label))
        .pl(px(t.space.s10))
        .flex()
        .items_center()
        .gap(px(t.space.s6))
        .text_color(color)
        .children(state.map(|state| {
            ui::agent_symbol(state, SharedString::from(format!("lane-{}", member.name)))
        }))
        .child(word)
}

/// One agent's line on the timeline, as shares of the hour.
struct Lane<'a> {
    member: &'a Member,
    /// When each delivery reached it.
    ticks: Vec<f32>,
    /// Each observed state: where it starts, where it ends, the state.
    spans: Vec<(f32, f32, &'a str)>,
}

/// One lane per agent that received something in the last hour, was busy or
/// waiting in it, or is now, most recently active first.
fn lanes(activity: &Activity, end: f64) -> Vec<Lane<'_>> {
    let start = end - HOUR;
    let share = |at: f64| ((at.clamp(start, end) - start) / HOUR) as f32;
    let mut lanes: Vec<(Lane<'_>, f64)> = Vec::new();
    for member in &activity.members {
        let ticks: Vec<f64> = activity
            .entries
            .iter()
            .filter(|entry| entry.agent == member.name && delivered(entry))
            .filter_map(|entry| entry.at.filter(|at| *at >= start && *at <= end))
            .collect();
        // Observed states, each lasting until the next observation. Before
        // the first one in the hour the lane says nothing.
        let changes: &[Change] = match &activity.history {
            History::Observed { changes, .. } => changes,
            _ => &[],
        };
        let observed: Vec<(f64, &str)> = changes
            .iter()
            .filter(|change| change.agent == member.name)
            .filter_map(|change| change.at.map(|at| (at, change.state.as_str())))
            .filter(|(at, _)| *at <= end)
            .collect();
        let spans: Vec<(f32, f32, &str)> = observed
            .iter()
            .enumerate()
            .map(|(at, (from, state))| {
                let to = observed.get(at + 1).map_or(end, |(next, _)| *next);
                (share(*from), share(to), *state)
            })
            .filter(|(from, to, _)| to > from)
            .collect();
        let (_, _, active) = state_now(member);
        // Observations of an idle or offline agent are not activity: Backbone
        // records one for every agent when it starts keeping history.
        let worked = spans.iter().any(|(_, _, state)| {
            matches!(
                AgentState::from_report(state),
                AgentState::Working | AgentState::NeedsYou
            )
        });
        if ticks.is_empty() && !active && !worked {
            continue;
        }
        let latest = ticks
            .iter()
            .chain(observed.iter().map(|(at, _)| at))
            .copied()
            .fold(if active { end } else { start }, f64::max);
        lanes.push((
            Lane {
                member,
                ticks: ticks.into_iter().map(share).collect(),
                spans,
            },
            latest,
        ));
    }
    lanes.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then(a.0.member.name.cmp(&b.0.member.name))
    });
    lanes
        .into_iter()
        .take(LANES)
        .map(|(lane, _)| lane)
        .collect()
}

/// The colour a state is drawn in on the timeline.
fn span_color(state: &str) -> Rgba {
    let c = colors();
    if state == "offline" {
        return c.selected;
    }
    match AgentState::from_report(state) {
        AgentState::NeedsYou => c.attention,
        AgentState::Working => c.accent,
        AgentState::Idle => c.line_strong,
        AgentState::Unknown => c.selected,
    }
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
            .child(axis)
            .child(div().flex_shrink_0().w(px(t.size.fact_label))),
    );
    if lanes.is_empty() {
        section = section.child(
            "No message reached an agent in the last hour, and no agent is busy or waiting.",
        );
    }
    for Lane {
        member,
        ticks,
        spans,
    } in lanes
    {
        let mut track = div()
            .relative()
            .flex_1()
            .h(px(t.space.s12))
            .rounded(px(t.radius.key))
            .bg(c.panel)
            .overflow_hidden();
        for (from, to, state) in spans {
            track = track.child(
                div()
                    .absolute()
                    .top(px(t.space.s2))
                    .bottom(px(t.space.s2))
                    .left(relative(from))
                    .w(relative(to - from))
                    .bg(span_color(state)),
            );
        }
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
                .child(track)
                .child(state_label(member)),
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
    let legend = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(t.space.s16))
        .pl(px(name_width))
        .pt(px(t.space.s4))
        .child(swatch(c.text_2, "message delivered"));
    section.child(match &activity.history {
        History::Observed { complete, .. } => legend
            .child(swatch(c.accent, "busy"))
            .child(swatch(c.attention, "waiting or blocked"))
            .child(swatch(c.line_strong, "idle"))
            .child(if *complete {
                "states as Backbone observed them; brief changes between looks can be missed"
            } else {
                "states as Backbone observed them; the oldest observations were not read, so \
                 states early in the hour may be missing"
            }),
        History::Failed(error) => legend.child(
            div()
                .text_color(c.attention_text)
                .child(format!("State history could not be read: {error}")),
        ),
        History::NotServed => {
            legend.child("state now on the right; this Backbone keeps no state history")
        }
    })
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
    // The timeline and filters scroll with the feed, so a short pane keeps
    // room for the records.
    let mut rows = list("activity-rows")
        .px(px(0.0))
        .py(px(0.0))
        .child(timeline(activity))
        .child(filters);
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
            outcome: "delivered".into(),
            ..Entry::default()
        };
        let end = 10_000.0;
        let activity = Activity {
            entries: vec![
                delivery("worker", end - 900.0),
                delivery("helper", end - 2.0 * HOUR),
                // Held, not delivered: no tick and no lane.
                Entry {
                    outcome: "queued".into(),
                    ..delivery("waiter", end - 60.0)
                },
            ],
            members: vec![
                member("worker", "idle"),
                member("helper", "idle"),
                member("builder", "busy"),
                member("waiter", "idle"),
            ],
            history: History::NotServed,
        };
        let lanes = lanes(&activity, end);
        let names: Vec<&str> = lanes.iter().map(|lane| lane.member.name.as_str()).collect();
        assert_eq!(names, ["builder", "worker"]);
        assert_eq!(lanes[1].ticks, [0.75]);
        assert!(lanes[1].spans.is_empty());
    }

    #[test]
    fn observed_states_span_until_the_next_observation() {
        let change = |state: &str, at: f64| Change {
            agent: "worker".into(),
            state: state.into(),
            at: Some(at),
            ..Default::default()
        };
        let end = 10_000.0;
        let activity = Activity {
            members: vec![Member {
                name: "worker".into(),
                state: "idle".into(),
                ..Member::default()
            }],
            // Busy since before the hour, as the lookback shows, then idle.
            history: History::Observed {
                complete: true,
                changes: vec![
                    change("busy", end - 2.0 * HOUR),
                    change("idle", end - 1_800.0),
                    change("busy", end - 900.0),
                    change("idle", end - 600.0),
                ],
            },
            ..Activity::default()
        };
        let lanes = lanes(&activity, end);
        let spans: Vec<(f32, f32, &str)> = lanes[0].spans.clone();
        assert_eq!(spans[0], (0.0, 0.5, "busy"));
        assert_eq!(spans[1], (0.5, 0.75, "idle"));
        assert_eq!(spans[2].2, "busy");
        assert_eq!(spans[3], (spans[3].0, 1.0, "idle"));
    }
}
