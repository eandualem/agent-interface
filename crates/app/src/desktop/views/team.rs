//! Team: every agent by repository, with its state and running swarms.

use std::collections::BTreeMap;

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use tmux_workspaces_roster::views::{Member, Team};

use super::{ViewEvent, ViewPane, list, mono, since};
use crate::desktop::{
    components::{self as ui, AgentState},
    design::{colors, tokens},
};

pub(super) fn body(team: &Team, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let state = |member: &Member| {
        if member.state == "offline" {
            None
        } else {
            Some(AgentState::from_report(&member.state))
        }
    };
    let tally = |wanted: Option<AgentState>| {
        team.members
            .iter()
            .filter(|member| state(member) == wanted)
            .count()
    };
    let summary = ui::view_toolbar()
        .gap(px(t.space.s16))
        .child(
            div()
                .text_color(c.attention_text)
                .child(format!("{} need you", tally(Some(AgentState::NeedsYou)))),
        )
        .child(
            div()
                .text_color(c.accent_text)
                .child(format!("{} working", tally(Some(AgentState::Working)))),
        )
        .child(format!("{} idle", tally(Some(AgentState::Idle))))
        .child(format!("{} offline", tally(None)));
    // Repositories in name order, agents without one last.
    let mut groups: BTreeMap<(bool, &str), Vec<&Member>> = BTreeMap::new();
    for member in &team.members {
        groups
            .entry((member.repo.is_empty(), member.repo.as_str()))
            .or_default()
            .push(member);
    }
    let mut rows = list("team-rows");
    for ((_, repo), mut members) in groups {
        members.sort_by_key(|member| {
            let rank = match state(member) {
                Some(AgentState::NeedsYou) => 0,
                Some(AgentState::Working) => 1,
                Some(_) => 2,
                None => 3,
            };
            (rank, member.name.to_ascii_lowercase())
        });
        let label = if repo.is_empty() {
            "no repository"
        } else {
            repo
        };
        let count = members.len();
        rows = rows.child(
            ui::view_heading(
                label.to_owned(),
                format!("{count} agent{}", if count == 1 { "" } else { "s" }),
            )
            .mt(px(t.space.s6)),
        );
        for swarm in team.swarms.iter().filter(|swarm| swarm.repo == repo) {
            let issue = swarm.issue.map(|n| format!(" · #{n}")).unwrap_or_default();
            rows = rows.child(
                ui::text(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .h(px(t.size.row - t.space.s2))
                        .px(px(t.space.s8))
                        .text_color(c.muted),
                    &t.text.status,
                )
                .child(ui::icon("git-branch", t.size.icon, c.violet))
                .child(
                    div()
                        .text_color(c.text_2)
                        .child(format!("swarm {}", swarm.name)),
                )
                .child(format!("{issue} · {}", swarm.members.join(", "))),
            );
        }
        for member in members {
            rows = rows.child(row(member, state(member), cx));
        }
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(summary)
        .child(rows)
        .child(ui::view_footer(&[], "click an agent to show its terminal"))
        .into_any_element()
}

fn row(member: &Member, state: Option<AgentState>, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let name = member.name.clone();
    let id = SharedString::from(format!("team-{name}"));
    let word = match (state, &member.reason) {
        (None, _) => "offline".to_owned(),
        (Some(AgentState::NeedsYou), Some(reason)) => format!("needs you · {reason}"),
        (Some(state), _) => match member.current_issue {
            Some(issue) if state == AgentState::Working => format!("working · #{issue}"),
            _ => state.word().to_owned(),
        },
    };
    let word_color = match state {
        Some(AgentState::NeedsYou) => c.attention_text,
        Some(AgentState::Working) => c.accent_text,
        _ => c.muted,
    };
    ui::row(id.clone(), false)
        .gap(px(t.space.s8))
        .px(px(t.space.s8))
        .when(state.is_none(), |el| el.opacity(0.7))
        .child(match state {
            Some(state) => ui::agent_symbol(state, id),
            None => div().size(px(t.size.agent_symbol)).flex_shrink_0(),
        })
        .child(
            mono(name.clone())
                .flex_shrink_0()
                .text_color(if state.is_some() { c.text } else { c.muted }),
        )
        .child(ui::badge(member.runtime.clone()))
        .child(
            ui::text(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
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
                    .w(px(t.size.button + t.space.s6))
                    .text_right()
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(since(member.state_since)),
        )
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(ViewEvent::ShowAgent(name.clone()))))
        .into_any_element()
}
