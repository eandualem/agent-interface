//! Team: who works where. Agents grouped by repository, tag or swarm, with
//! their states; a click shows an agent's terminal, ⌥-click opens its card
//! beside this pane.

use std::collections::BTreeMap;

use agent_interface_core::View;
use agent_interface_roster::views::{Member, Team};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use super::{Grouping, ViewEvent, ViewPane, home, mono, since};
use crate::desktop::{
    components::{self as ui, AgentState, Tone},
    design::{colors, tokens},
};

fn state(member: &Member, cx: &gpui::App) -> AgentState {
    crate::desktop::agent_status::LiveAgents::shared(cx)
        .get(&member.name)
        .state
}

fn rank(member: &Member, cx: &gpui::App) -> (u8, String) {
    let status = crate::desktop::agent_status::LiveAgents::shared(cx).get(&member.name);
    (status.rank(), member.name.to_ascii_lowercase())
}

pub(super) fn body(pane: &ViewPane, team: &Team, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let online = team
        .members
        .iter()
        .filter(|m| {
            crate::desktop::agent_status::LiveAgents::shared(cx)
                .get(&m.name)
                .online
        })
        .count();
    let mut grouping = ui::segments();
    for (choice, label) in [
        (Grouping::Folder, "Folder"),
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

    let tally = |wanted: AgentState| {
        team.members
            .iter()
            .filter(|m| state(m, cx) == wanted)
            .count()
    };
    let needing = team
        .members
        .iter()
        .filter(|m| state(m, cx).needs_you())
        .count();
    let working = team
        .members
        .iter()
        .filter(|m| state(m, cx).working())
        .count();
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
            .child(ui::agent_symbol(AgentState::NeedsYou))
            .child(format!("{needing} need you")),
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .text_color(c.accent_text)
            .child(ui::agent_symbol(if working > 0 {
                AgentState::Working
            } else {
                AgentState::Idle
            }))
            .child(format!("{working} active")),
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .child(ui::agent_symbol(AgentState::Idle))
            .child(format!("{} idle", tally(AgentState::Idle))),
    )
    .child(format!("{} offline", tally(AgentState::Offline)))
    .when(tally(AgentState::Unknown) > 0, |el| {
        el.child(format!("{} status unknown", tally(AgentState::Unknown)))
    })
    .when(tally(AgentState::Unavailable) > 0, |el| {
        el.child(format!(
            "{} status unavailable",
            tally(AgentState::Unavailable)
        ))
    });

    let column = match pane.grouping {
        Grouping::Folder => folders(team, cx),
        Grouping::Tag => by_tag(team, cx),
        Grouping::Swarm => swarms(team, cx),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(summary)
        .child(ui::grouped_page("team-rows", column))
        .child(ui::view_footer(
            &[
                ("click", "show its terminal"),
                ("⌥ click", "agent card beside"),
            ],
            "updates every 3 s",
        ))
        .into_any_element()
}

fn plural(count: usize) -> String {
    format!("{count} agent{}", if count == 1 { "" } else { "s" })
}

fn name_of(path: &str) -> &str {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent)
}

/// Whether an agent coordinates its folder: its own folder is named so.
fn coordinates(member: &Member) -> bool {
    name_of(&member.dir) == "coordinator"
}

/// One card of agents, with tree guides for the agents a coordinator leads.
fn card(
    members: &[&Member],
    group: &str,
    lead: Option<&Member>,
    cx: &mut Context<ViewPane>,
) -> gpui::Div {
    let mut card = ui::group_card();
    let mut first = true;
    if let Some(lead) = lead {
        card = card.child(agent_row(lead, "", group, true, cx));
        first = false;
    }
    // A coordinator leads the agents in its own folder; others on the card
    // follow without a guide.
    let (led, rest): (Vec<&Member>, Vec<&Member>) = members
        .iter()
        .copied()
        .filter(|member| lead.is_none_or(|lead| lead.name != member.name))
        .partition(|member| lead.is_some_and(|lead| parent(&member.dir) == parent(&lead.dir)));
    for (index, member) in led.iter().enumerate() {
        let guide = if index + 1 == led.len() {
            "└ "
        } else {
            "├ "
        };
        card = card.child(agent_row(member, guide, group, first, cx));
        first = false;
    }
    for member in rest {
        card = card.child(agent_row(member, "", group, first, cx));
        first = false;
    }
    card
}

/// Agents by the folder they work in, ordered by path so a folder's
/// subfolders follow it, lone folders last. A folder's coordinator comes first and leads the
/// others; a subfolder without one says which coordinator above leads it.
fn folders(team: &Team, cx: &mut Context<ViewPane>) -> gpui::Div {
    let mut groups: BTreeMap<(bool, String), Vec<&Member>> = BTreeMap::new();
    for member in &team.members {
        let folder = parent(&member.dir).to_owned();
        groups
            .entry((folder.is_empty(), folder))
            .or_default()
            .push(member);
    }
    // Each folder's coordinator, to find the nearest one above a folder.
    let leads: BTreeMap<&str, &Member> = groups
        .iter()
        .filter_map(|((_, folder), members)| {
            let lead = members.iter().copied().find(|member| coordinates(member))?;
            Some((folder.as_str(), lead))
        })
        .collect();
    // Team folders first; a folder holding one agent that no coordinator
    // leads, such as a scratch directory, follows them.
    let mut order: Vec<_> = groups.iter().collect();
    order.sort_by_key(|((empty, folder), members)| {
        let lone = members.len() == 1
            && !leads.contains_key(folder.as_str())
            && led_from(&leads, folder).is_none();
        (*empty, lone, folder.clone())
    });
    let mut column = div();
    for ((_, folder), members) in order {
        let mut members = members.clone();
        members.sort_by_key(|member| rank(member, cx));
        let lead = leads.get(folder.as_str()).copied();
        let above = lead.is_none().then(|| led_from(&leads, folder)).flatten();
        let (title, mut note) = if folder.is_empty() {
            ("No folder".to_owned(), plural(members.len()))
        } else {
            (
                name_of(folder).to_owned(),
                format!("{} · {}", home(folder), plural(members.len())),
            )
        };
        if let Some(above) = above {
            note.push_str(&format!(" · led by {}", above.name));
        }
        column = column
            .child(ui::group_title(title, note))
            .child(card(&members, folder, lead, cx));
    }
    column
}

/// The coordinator of the nearest folder above `folder` that has one.
fn led_from<'a>(leads: &BTreeMap<&str, &'a Member>, folder: &str) -> Option<&'a Member> {
    let mut at = parent(folder);
    while !at.is_empty() {
        if let Some(lead) = leads.get(at) {
            return Some(lead);
        }
        at = parent(at);
    }
    None
}

fn by_tag(team: &Team, cx: &mut Context<ViewPane>) -> gpui::Div {
    let mut groups: BTreeMap<(bool, String), Vec<&Member>> = BTreeMap::new();
    for member in &team.members {
        if member.tags.is_empty() {
            groups
                .entry((true, String::new()))
                .or_default()
                .push(member);
        }
        for tag in &member.tags {
            groups.entry((false, tag.clone())).or_default().push(member);
        }
    }
    let mut column = div();
    for ((_, tag), mut members) in groups {
        members.sort_by_key(|member| rank(member, cx));
        let label = if tag.is_empty() {
            ui::tag("no tag", Tone::Quiet)
        } else {
            ui::tag(tag.clone(), Tone::Tag)
        };
        column = column
            .child(ui::group_label(label, plural(members.len())))
            .child(card(&members, &tag, None, cx));
    }
    column
}

fn swarms(team: &Team, cx: &mut Context<ViewPane>) -> gpui::Div {
    let mut column = div();
    if team.swarms.is_empty() {
        column = column.child(ui::group_note("No swarm is running."));
    }
    for swarm in &team.swarms {
        let members: Vec<&Member> = team
            .members
            .iter()
            .filter(|member| swarm.members.contains(&member.name))
            .collect();
        let issue = swarm
            .issue
            .map(|n| format!(" · issue #{n}"))
            .unwrap_or_default();
        column = column
            .child(ui::group_title(
                swarm.name.clone(),
                format!("{}{issue} · {}", swarm.repo, plural(members.len())),
            ))
            .child(card(&members, &swarm.name, None, cx));
    }
    let outside: Vec<&Member> = team
        .members
        .iter()
        .filter(|member| {
            !team
                .swarms
                .iter()
                .any(|swarm| swarm.members.contains(&member.name))
        })
        .collect();
    if !outside.is_empty() {
        column = column
            .child(ui::group_title("Not in a swarm", plural(outside.len())))
            .child(card(&outside, "", None, cx));
    }
    column
}

/// One agent on a card: its state, name, CLI and tags, what it is doing and
/// for how long. `guide` draws its place under a coordinator.
pub(super) fn agent_row(
    member: &Member,
    guide: &'static str,
    group: &str,
    first: bool,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let status = crate::desktop::agent_status::LiveAgents::shared(cx).get(&member.name);
    let state = state(member, cx);
    let name = member.name.clone();
    // An agent can appear in several groups; each row needs its own identity.
    let id = SharedString::from(format!("team-{group}-{name}-{}", guide.len()));
    div()
        .id(id.clone())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s8))
        .h(px(t.size.group_row))
        .px(px(t.space.s14))
        .when(!first, |el| {
            el.border_t(px(t.size.hairline)).border_color(c.line)
        })
        .cursor_pointer()
        .hover(|el| el.bg(colors().hover))
        .when(state == AgentState::Offline, |el| el.opacity(0.7))
        .when(!guide.is_empty(), |el| {
            el.child(mono(guide).text_color(c.faint))
        })
        .child(ui::agent_symbol(status.state))
        .child(
            ui::text(div().flex_shrink_0(), &t.text.row)
                .text_color(if state.working() || state.needs_you() {
                    c.text
                } else {
                    c.muted
                })
                .child(name.clone()),
        )
        .child(ui::badge(member.runtime.clone()))
        .when(coordinates(member), |el| {
            el.child(ui::tag("coordinator", Tone::Plain))
        })
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
        .child(ui::text(
            ui::agent_status_text(&status).flex_shrink_0(),
            &t.text.meta,
        ))
        .child(
            ui::text(
                div()
                    .flex_shrink_0()
                    .w(px(t.size.pane_header + t.space.s8))
                    .text_right()
                    .text_color(c.muted),
                &t.text.meta,
            )
            // Backbone's time for a live agent is when it was last read, not
            // when its state began; only an offline agent's says something.
            .child(if state == AgentState::Offline {
                since(status.since)
            } else {
                String::new()
            }),
        )
        .on_click(cx.listener(move |_, event: &gpui::ClickEvent, _, cx| {
            if event.modifiers().alt {
                cx.emit(ViewEvent::OpenBeside(View::agent(name.clone(), None)));
            } else {
                cx.emit(ViewEvent::ShowAgent(name.clone()));
            }
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_coordinators_come_from_the_workspace_path() {
        assert_eq!(parent("/home/me/ws/career/desk"), "/home/me/ws/career");
        assert_eq!(name_of("/home/me/ws/career/"), "career");
        let lead = Member {
            dir: "/home/me/ws/career/coordinator".into(),
            ..Member::default()
        };
        assert!(coordinates(&lead));
        assert!(!coordinates(&Member::default()));
        let lead = Member {
            name: "lead".into(),
            ..lead
        };
        let leads = BTreeMap::from([("/home/me/ws/learning", &lead)]);
        assert_eq!(
            led_from(&leads, "/home/me/ws/learning/rust").map(|m| m.name.as_str()),
            Some("lead")
        );
        assert!(led_from(&leads, "/home/me/ws/career").is_none());
    }
}
