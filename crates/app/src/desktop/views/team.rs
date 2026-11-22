//! Team: who works where. Agents grouped by repository, tag or swarm, with
//! their states; ⌥-click opens an agent's card beside this pane.

use std::collections::BTreeMap;

use agent_interface_core::View;
use agent_interface_roster::views::{Member, Team};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use super::{Grouping, TeamAgentAction, ViewEvent, ViewPane, home, line, mono, since};
use crate::desktop::{
    agent_groups::{self, MachinePresence},
    components::{self as ui, AgentState, Tone},
    design::{colors, tokens},
};

fn state(member: &Member, pane: &ViewPane) -> AgentState {
    pane.agents.get(&member.name).state
}

fn rank(member: &Member, pane: &ViewPane) -> (u8, String) {
    let status = pane.agents.get(&member.name);
    (status.rank(), member.name.to_ascii_lowercase())
}

pub(super) fn body(pane: &ViewPane, team: &Team, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let multi_machine = pane.agents.is_multi_machine(&team.members);
    let active_grouping = if pane.grouping == Grouping::Machine && !multi_machine {
        Grouping::Folder
    } else {
        pane.grouping
    };
    let online = team
        .members
        .iter()
        .filter(|m| {
            let status = pane.agents.get(&m.name);
            status.online && !status.stale
        })
        .count();
    let mut grouping = ui::segments();
    let choices: &[(&'static str, Grouping)] = if multi_machine {
        &[
            ("Folder", Grouping::Folder),
            ("Tag", Grouping::Tag),
            ("Swarm", Grouping::Swarm),
            ("Machine", Grouping::Machine),
        ]
    } else {
        &[
            ("Folder", Grouping::Folder),
            ("Tag", Grouping::Tag),
            ("Swarm", Grouping::Swarm),
        ]
    };
    for &(label, choice) in choices {
        grouping = grouping.child(
            ui::segment(
                SharedString::from(format!("team-by-{label}")),
                label,
                choice == active_grouping,
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

    let is_stale = |m: &Member| pane.agents.get(&m.name).stale;
    let tally = |wanted: AgentState| {
        team.members
            .iter()
            .filter(|m| !is_stale(m) && state(m, pane) == wanted)
            .count()
    };
    let stale_count = team.members.iter().filter(|m| is_stale(m)).count();
    let needing = team
        .members
        .iter()
        .filter(|m| !is_stale(m) && state(m, pane).needs_you())
        .count();
    let working = team
        .members
        .iter()
        .filter(|m| !is_stale(m) && state(m, pane).working())
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
    .when(stale_count > 0, |el| {
        el.child(
            div()
                .text_color(c.attention_text)
                .child(format!("{stale_count} stale")),
        )
    })
    .when(tally(AgentState::Unknown) > 0, |el| {
        el.child(format!("{} status unknown", tally(AgentState::Unknown)))
    })
    .when(tally(AgentState::Unavailable) > 0, |el| {
        el.child(format!(
            "{} status unavailable",
            tally(AgentState::Unavailable)
        ))
    });

    let column = match active_grouping {
        Grouping::Folder => folders(pane, team, cx),
        Grouping::Tag => by_tag(pane, team, cx),
        Grouping::Swarm => swarms(pane, team, cx),
        Grouping::Machine => by_machine(pane, team, cx),
    };
    let notice = pane.team_notice.as_ref().map(|message| {
        ui::text(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s8))
                .px(px(t.space.s14))
                .py(px(t.space.s6))
                .bg(c.panel)
                .border_b(px(t.size.hairline))
                .border_color(c.line)
                .text_color(c.text_2),
            &t.text.status,
        )
        .child(div().flex_1().min_w_0().child(line(message)))
        .child(
            ui::icon_button("team-notice-dismiss", "x").on_click(cx.listener(|pane, _, _, cx| {
                pane.team_notice = None;
                cx.notify();
            })),
        )
    });
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(summary)
        .children(notice)
        .child(ui::grouped_page("team-rows", column))
        .child(ui::view_footer(
            &[("⌥ click", "agent card beside")],
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
    pane: &ViewPane,
    members: &[&Member],
    group: &str,
    lead: Option<&Member>,
    cx: &mut Context<ViewPane>,
) -> gpui::Div {
    let mut card = ui::group_card();
    let mut first = true;
    if let Some(lead) = lead {
        card = card.child(agent_row(pane, lead, "", group, true, cx));
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
        card = card.child(agent_row(pane, member, guide, group, first, cx));
        first = false;
    }
    for member in rest {
        card = card.child(agent_row(pane, member, "", group, first, cx));
        first = false;
    }
    card
}

/// Agents by the folder they work in, ordered by path so a folder's
/// subfolders follow it, lone folders last. A folder's coordinator comes first and leads the
/// others; a subfolder without one says which coordinator above leads it.
fn folders(pane: &ViewPane, team: &Team, cx: &mut Context<ViewPane>) -> gpui::Div {
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
        members.sort_by_key(|member| rank(member, pane));
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
            .child(card(pane, &members, folder, lead, cx));
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

fn by_tag(pane: &ViewPane, team: &Team, cx: &mut Context<ViewPane>) -> gpui::Div {
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
        members.sort_by_key(|member| rank(member, pane));
        let label = if tag.is_empty() {
            ui::tag("no tag", Tone::Quiet)
        } else {
            ui::tag(tag.clone(), Tone::Tag)
        };
        column = column
            .child(ui::group_label(label, plural(members.len())))
            .child(card(pane, &members, &tag, None, cx));
    }
    column
}

fn swarms(pane: &ViewPane, team: &Team, cx: &mut Context<ViewPane>) -> gpui::Div {
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
            .child(card(pane, &members, &swarm.name, None, cx));
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
            .child(card(pane, &outside, "", None, cx));
    }
    column
}

/// Group all configured agents by their full machine ID, showing each machine's
/// live reachability (`online`, `offline · last seen …`, `not in profile`, or
/// `status unknown`), with local agents first and `machine: null` separately.
fn by_machine(pane: &ViewPane, team: &Team, cx: &mut Context<ViewPane>) -> gpui::Div {
    let local_id = pane.agents.local_machine_id();
    let (machines, machines_read) = pane.agents.machines();
    let mut local: Vec<&Member> = Vec::new();
    let mut unplaced: Vec<&Member> = Vec::new();
    let mut remote: BTreeMap<String, Vec<&Member>> = BTreeMap::new();
    let mut remote_names: BTreeMap<String, String> = BTreeMap::new();

    for member in &team.members {
        match &member.machine {
            None => unplaced.push(member),
            Some(machine)
                if local_id
                    .as_deref()
                    .is_none_or(|local| local == machine.id.as_str()) =>
            {
                local.push(member);
            }
            Some(machine) => {
                remote_names
                    .entry(machine.id.clone())
                    .or_insert_with(|| machine.name.clone());
                remote.entry(machine.id.clone()).or_default().push(member);
            }
        }
    }

    let mut column = div();
    let local_profile = local_id
        .as_deref()
        .and_then(|id| machines.iter().find(|machine| machine.id == id));
    let local_name = local_profile
        .map(|machine| agent_groups::machine_display_name(&machine.name, &machine.id))
        .or_else(|| {
            pane.connection
                .as_ref()
                .and_then(|conn| conn.machine_name().map(str::to_owned))
        })
        .unwrap_or_else(|| "This machine".into());
    let local_note = format!("this machine · online · {}", plural(local.len()));
    local.sort_by_key(|member| rank(member, pane));
    column = column.child(ui::group_title(local_name, local_note));
    if !local.is_empty() {
        column = column.child(card(pane, &local, "machine-local", None, cx));
    }

    // Known remote machines in the service's order first, then unknown IDs.
    let mut ordered: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for machine in &machines {
        if local_id
            .as_deref()
            .is_some_and(|local| local != machine.id.as_str())
            && seen.insert(machine.id.clone())
        {
            ordered.push(machine.id.clone());
        }
    }
    let mut unknown: Vec<(String, String)> = remote
        .keys()
        .filter(|id| !seen.contains(*id))
        .map(|id| {
            let name = agent_groups::machine_display_name(
                remote_names.get(id).map(String::as_str).unwrap_or(""),
                id,
            );
            (name.to_ascii_lowercase(), id.clone())
        })
        .collect();
    unknown.sort();
    ordered.extend(unknown.into_iter().map(|(_, id)| id));

    for id in ordered {
        let profile_machine = machines.iter().find(|machine| machine.id == id);
        let raw_name = profile_machine
            .map(|machine| machine.name.as_str())
            .filter(|name| !name.trim().is_empty())
            .or_else(|| remote_names.get(&id).map(String::as_str))
            .unwrap_or("");
        let name = agent_groups::machine_display_name(raw_name, &id);
        let presence: MachinePresence =
            agent_groups::machine_presence(&id, &machines, machines_read);
        let mut members = remote.remove(&id).unwrap_or_default();
        members.sort_by_key(|member| rank(member, pane));
        let note = format!("{} · {}", presence.label(), plural(members.len()));
        column = column.child(ui::group_title(name, note));
        if !members.is_empty() {
            column = column.child(card(pane, &members, &format!("machine-{id}"), None, cx));
        }
    }

    if !unplaced.is_empty() {
        unplaced.sort_by_key(|member| rank(member, pane));
        column = column
            .child(ui::group_title(
                "Never started",
                format!("no machine yet · {}", plural(unplaced.len())),
            ))
            .child(card(pane, &unplaced, "machine-null", None, cx));
    }

    column
}

/// One agent on a card: its state, name, CLI and tags, what it is doing and
/// for how long. `guide` draws its place under a coordinator.
pub(super) fn agent_row(
    pane: &ViewPane,
    member: &Member,
    guide: &'static str,
    group: &str,
    first: bool,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let status = pane.agents.get(&member.name);
    let state = state(member, pane);
    let name = member.name.clone();
    let registration = member.registration_id.clone();
    let local_id = pane.agents.local_machine_id();
    let remote_badge = (pane.grouping != Grouping::Machine)
        .then(|| {
            member
                .machine
                .as_ref()
                .filter(|machine| {
                    local_id
                        .as_deref()
                        .is_some_and(|local| local != machine.id.as_str())
                })
                .map(|machine| agent_groups::machine_display_name(&machine.name, &machine.id))
        })
        .flatten();
    let busy_action = pane.team_actions_in_flight.contains(&name);
    let can_act = pane.connection.is_some();
    // An agent can appear in several groups; each row needs its own identity.
    let id = SharedString::from(format!("team-{group}-{name}-{}", guide.len()));
    let action_btn = |label: &'static str, suffix: &'static str, action: TeamAgentAction| {
        let agent_name = name.clone();
        let reg = registration.clone();
        ui::segment(SharedString::from(format!("{id}-{suffix}")), label, false)
            .when(busy_action, |el| el.opacity(0.4))
            .on_click(cx.listener(move |pane, _, _, cx| {
                cx.stop_propagation();
                pane.run_team_action(agent_name.clone(), reg.clone(), action, cx);
            }))
    };
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
        .when(registration.is_some(), |el| {
            el.cursor_pointer().hover(|el| el.bg(colors().hover))
        })
        .when(state == AgentState::Offline || status.stale, |el| {
            el.opacity(0.75)
        })
        .when(!guide.is_empty(), |el| {
            el.child(mono(guide).text_color(c.faint))
        })
        .child(ui::agent_symbol(if status.stale {
            AgentState::Unknown
        } else {
            status.state
        }))
        .child(
            ui::text(div().flex_shrink_0(), &t.text.row)
                .text_color(if !status.stale && (state.working() || state.needs_you()) {
                    c.text
                } else {
                    c.muted
                })
                .child(name.clone()),
        )
        .child(ui::badge(member.runtime.clone()))
        .when_some(remote_badge, |el, machine| {
            el.child(ui::tag(machine, Tone::Quiet))
        })
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
        // What it waits on and its run counts give way before its state.
        .children(status.line.as_deref().map(|text| {
            ui::text(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(if status.stale {
                        c.attention_text
                    } else {
                        c.muted
                    }),
                &t.text.meta,
            )
            .child(line(text))
        }))
        .child(ui::text(
            ui::agent_status_text(&status).flex_shrink_0(),
            &t.text.meta,
        ))
        .when(can_act, |el| {
            let mut controls = div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s4));
            if state == AgentState::Offline {
                controls = controls
                    .child(action_btn(
                        "Resume",
                        "resume",
                        TeamAgentAction::Start { resume: true },
                    ))
                    .child(action_btn(
                        "Start",
                        "start",
                        TeamAgentAction::Start { resume: false },
                    ));
            } else {
                controls = controls
                    .child(action_btn(
                        "Restart",
                        "restart",
                        TeamAgentAction::Restart { resume: false },
                    ))
                    .child(action_btn("Stop", "stop", TeamAgentAction::Stop));
            }
            el.child(controls)
        })
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
            if let Some(event) = row_event(&name, registration.as_deref(), event.modifiers().alt) {
                cx.emit(event);
            }
        }))
        .into_any_element()
}

// ⌥-click opens the agent's card under the registration the row observed; a
// plain click does nothing.
fn row_event(name: &str, registration: Option<&str>, card: bool) -> Option<ViewEvent> {
    if !card {
        return None;
    }
    let mut view = View::agent(name.to_owned(), None);
    view.registration_id = Some(registration?.to_owned());
    view.is_valid().then_some(ViewEvent::OpenBeside(view))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_open_only_a_registered_card_on_alt_click() {
        let registration = "1234567890abcdef1234567890abcdef";
        for card in [false, true] {
            assert!(row_event("worker", None, card).is_none());
        }
        assert!(row_event("worker", Some(registration), false).is_none());
        assert!(row_event("worker", Some("bad"), true).is_none());
        let Some(ViewEvent::OpenBeside(view)) = row_event("worker", Some(registration), true)
        else {
            panic!("missing registered card")
        };
        assert_eq!(view.registration_id.as_deref(), Some(registration));
    }

    #[test]
    fn folders_and_coordinators_come_from_the_workspace_path() {
        assert_eq!(parent("/home/me/ws/product/desk"), "/home/me/ws/product");
        assert_eq!(name_of("/home/me/ws/product/"), "product");
        let lead = Member {
            dir: "/home/me/ws/product/coordinator".into(),
            ..Member::default()
        };
        assert!(coordinates(&lead));
        assert!(!coordinates(&Member::default()));
        let lead = Member {
            name: "lead".into(),
            ..lead
        };
        let leads = BTreeMap::from([("/home/me/ws/docs", &lead)]);
        assert_eq!(
            led_from(&leads, "/home/me/ws/docs/rust").map(|m| m.name.as_str()),
            Some("lead")
        );
        assert!(led_from(&leads, "/home/me/ws/product").is_none());
    }
}
