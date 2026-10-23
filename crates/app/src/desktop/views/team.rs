//! Team: who works where. Agents grouped by repository, tag or swarm, with
//! their states; a click shows an agent's terminal, ⌥-click opens its card
//! beside this pane.

use std::collections::BTreeMap;

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_core::{View, ViewKind};
use tmux_workspaces_roster::views::{Member, Team};

use super::{Grouping, ViewEvent, ViewPane, list, mono, since};
use crate::desktop::{
    components::{self as ui, AgentState},
    design::{colors, tokens},
};

fn state(member: &Member) -> Option<AgentState> {
    (member.state != "offline").then(|| AgentState::from_report(&member.state))
}

fn rank(member: &Member) -> (u8, String) {
    let rank = match state(member) {
        Some(AgentState::NeedsYou) => 0,
        Some(AgentState::Working) => 1,
        Some(_) => 2,
        None => 3,
    };
    (rank, member.name.to_ascii_lowercase())
}

pub(super) fn body(pane: &ViewPane, team: &Team, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let online = team.members.iter().filter(|m| state(m).is_some()).count();
    let mut grouping = ui::segments();
    for (choice, label) in [
        (Grouping::Repository, "Repository"),
        (Grouping::Tag, "Tag"),
        (Grouping::Swarm, "Swarm"),
    ] {
        grouping = grouping.child(
            ui::segment(
                SharedString::from(format!("team-by-{label}")),
                label,
                choice == pane.grouping,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.grouping = choice;
                cx.notify();
            })),
        );
    }
    let header = ui::view_header("bot", "Team")
        .pr(px(t.space.s6))
        .child(format!("{} agents · {online} online", team.members.len()))
        .child(div().flex_1())
        .child(grouping);

    let tally =
        |wanted: Option<AgentState>| team.members.iter().filter(|m| state(m) == wanted).count();
    let summary = ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s16))
            .h(px(t.size.view_toolbar))
            .px(px(t.space.s14))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.muted),
        &t.text.status,
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .text_color(c.attention_text)
            .child(ui::agent_symbol(AgentState::NeedsYou, "team-tally-need"))
            .child(format!("{} need you", tally(Some(AgentState::NeedsYou)))),
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .text_color(c.accent_text)
            .child(ui::agent_symbol(AgentState::Working, "team-tally-work"))
            .child(format!("{} working", tally(Some(AgentState::Working)))),
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .child(ui::agent_symbol(AgentState::Idle, "team-tally-idle"))
            .child(format!("{} idle", tally(Some(AgentState::Idle)))),
    )
    .child(format!("{} offline", tally(None)));

    // Groups in name order; agents with no group last.
    let mut groups: BTreeMap<(bool, String), Vec<&Member>> = BTreeMap::new();
    for member in &team.members {
        let keys: Vec<String> = match pane.grouping {
            Grouping::Repository => vec![member.repo.clone()],
            Grouping::Tag if member.tags.is_empty() => vec![String::new()],
            Grouping::Tag => member.tags.clone(),
            Grouping::Swarm => {
                let swarms: Vec<String> = team
                    .swarms
                    .iter()
                    .filter(|swarm| swarm.members.contains(&member.name))
                    .map(|swarm| swarm.name.clone())
                    .collect();
                if swarms.is_empty() {
                    vec![String::new()]
                } else {
                    swarms
                }
            }
        };
        for key in keys {
            groups
                .entry((key.is_empty(), key))
                .or_default()
                .push(member);
        }
    }
    let empty = match pane.grouping {
        Grouping::Repository => "no repository",
        Grouping::Tag => "no tag",
        Grouping::Swarm => "not in a swarm",
    };
    let mut rows = list("team-rows");
    for ((_, key), mut members) in groups {
        members.sort_by_key(|member| rank(member));
        let count = members.len();
        let label = if key.is_empty() {
            empty.to_owned()
        } else {
            key.clone()
        };
        rows = rows.child(group_row(
            &label,
            &format!("{count} agent{}", if count == 1 { "" } else { "s" }),
        ));
        // Under a repository, its running swarms and their members.
        let swarms: Vec<_> = if pane.grouping == Grouping::Repository {
            team.swarms
                .iter()
                .filter(|swarm| swarm.repo == key && !key.is_empty())
                .collect()
        } else {
            Vec::new()
        };
        let in_swarm = |member: &Member| {
            swarms
                .iter()
                .any(|swarm| swarm.members.contains(&member.name))
        };
        for member in members.iter().filter(|m| !in_swarm(m)) {
            rows = rows.child(agent_row(member, "", cx));
        }
        for swarm in &swarms {
            let issue = swarm
                .issue
                .map(|n| format!("issue #{n}"))
                .unwrap_or_default();
            rows = rows.child(
                ui::text(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(t.space.s8))
                        .h(px(t.size.row - t.space.s2))
                        .px(px(t.space.s8))
                        .text_color(c.muted),
                    &t.text.status,
                )
                .child(mono("└ ").text_color(c.faint))
                .child(ui::icon("git-branch", t.size.icon - 1.0, c.violet))
                .child(
                    div()
                        .flex()
                        .gap(px(t.space.s4))
                        .text_color(c.text_2)
                        .child("swarm")
                        .child(
                            div()
                                .text_color(c.text)
                                .font_weight(t.text.row_selected.weight())
                                .child(swarm.name.clone()),
                        ),
                )
                .child(issue),
            );
            let inside: Vec<_> = members
                .iter()
                .filter(|m| swarm.members.contains(&m.name))
                .collect();
            for (index, member) in inside.iter().enumerate() {
                let guide = if index + 1 == inside.len() {
                    "   └ "
                } else {
                    "   ├ "
                };
                rows = rows.child(agent_row(member, guide, cx));
            }
        }
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(summary)
        .child(rows)
        .child(ui::view_footer(
            &[
                ("click", "show its terminal"),
                ("⌥ click", "agent card beside"),
            ],
            "updates every 3 s",
        ))
        .into_any_element()
}

fn group_row(label: &str, meta: &str) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .h(px(t.size.section_label))
            .px(px(t.space.s8))
            .pt(px(t.space.s6))
            .text_color(c.muted),
        &t.text.status,
    )
    .child(ui::icon("git-branch", t.size.icon - 1.0, c.muted))
    .child(mono(label.to_owned()).flex_shrink_0().text_color(c.text_2))
    .child(div().flex_1().h(px(t.size.hairline)).bg(c.line))
    .child(meta.to_owned())
}

fn agent_row(member: &Member, guide: &'static str, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let state = state(member);
    let name = member.name.clone();
    let id = SharedString::from(format!("team-{name}-{}", guide.len()));
    let word = match (state, &member.reason) {
        (None, _) => "offline".to_owned(),
        (Some(AgentState::NeedsYou), Some(reason)) => format!("waiting · {reason}"),
        (Some(AgentState::NeedsYou), None) => "waiting".to_owned(),
        (Some(AgentState::Working), _) => match member.current_issue {
            Some(issue) => format!("busy · #{issue}"),
            None => "busy".to_owned(),
        },
        (Some(state), _) => state.word().to_owned(),
    };
    let word_color = match state {
        Some(AgentState::NeedsYou) => c.attention_text,
        Some(AgentState::Working) => c.accent_text,
        _ => c.muted,
    };
    div()
        .id(id.clone())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s8))
        .h(px(t.size.row))
        .px(px(t.space.s8))
        .rounded(px(t.radius.row))
        .cursor_pointer()
        .hover(|el| el.bg(colors().hover))
        .when(state.is_none(), |el| el.opacity(0.7))
        .when(!guide.is_empty(), |el| {
            el.child(mono(guide).text_color(c.faint))
        })
        .child(match state {
            Some(state) => ui::agent_symbol(state, id),
            None => div()
                .size(px(t.size.agent_symbol))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(t.space.s8))
                        .h(px(t.size.hairline + 0.5))
                        .bg(c.faint),
                ),
        })
        .child(
            ui::text(div().flex_shrink_0(), &t.text.row)
                .text_color(if state.is_some_and(|s| s != AgentState::Idle) {
                    c.text
                } else {
                    c.muted
                })
                .child(name.clone()),
        )
        .child(ui::badge(member.runtime.clone()))
        .child(
            ui::text(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(member.tags.join(" · ")),
        )
        .child(ui::text(div().flex_shrink_0().text_color(word_color), &t.text.meta).child(word))
        .child(
            ui::text(
                div()
                    .flex_shrink_0()
                    .w(px(t.size.pane_header + t.space.s8))
                    .text_right()
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(since(member.state_since)),
        )
        .on_click(cx.listener(move |_, event: &gpui::ClickEvent, _, cx| {
            if event.modifiers().alt {
                cx.emit(ViewEvent::OpenBeside(View {
                    kind: ViewKind::Agent,
                    agent: Some(name.clone()),
                }));
            } else {
                cx.emit(ViewEvent::ShowAgent(name.clone()));
            }
        }))
        .into_any_element()
}
