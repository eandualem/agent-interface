//! The agents list as one tree: everything an agent started sits under it,
//! and a machine is a tag on the agent rather than a group.

use std::collections::{HashMap, HashSet};

use agent_interface_roster::{
    Machine, MachineState, ProfileMachine, Swarm,
    profiles::setup::one_line,
    views::{epoch_seconds, now},
};

use super::agent_status::AgentState;

/// A review run under the agent that started it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Review {
    /// Who reviews: its model, else its CLI, else the run's label.
    pub reviewer: String,
    pub effort: Option<String>,
    /// Running, rather than queued.
    pub running: bool,
}

/// What an agent started besides swarms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Started {
    /// Its active reviews.
    pub reviews: Vec<Review>,
    /// Its browser's label, when it has one.
    pub browser: Option<String>,
}

/// What an agent's row counts of everything under it, open or folded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    /// Running reviews.
    pub reviews: usize,
    /// Listed agents in its swarms, each once.
    pub members: usize,
    pub members_working: bool,
    pub members_need_you: bool,
    pub browsers: usize,
}

/// One item of an agent's glance counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Glance {
    Reviews,
    Members,
    Browsers,
}

/// The colour of a glance item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Muted,
    /// Something in it runs.
    Accent,
    /// Something in it needs the person.
    Attention,
}

impl Counts {
    /// The items to draw, in order, each with the number it shows. Zero
    /// items are left out, and a single browser shows no number.
    pub(super) fn items(&self) -> Vec<(Glance, Option<usize>, Tone)> {
        let mut items = Vec::new();
        if self.reviews > 0 {
            items.push((Glance::Reviews, Some(self.reviews), Tone::Accent));
        }
        if self.members > 0 {
            let tone = if self.members_need_you {
                Tone::Attention
            } else if self.members_working {
                Tone::Accent
            } else {
                Tone::Muted
            };
            items.push((Glance::Members, Some(self.members), tone));
        }
        if self.browsers > 0 {
            let shown = (self.browsers > 1).then_some(self.browsers);
            items.push((Glance::Browsers, shown, Tone::Muted));
        }
        items
    }
}

/// One line of the agents list.
#[derive(Debug, PartialEq)]
pub(super) enum Row {
    /// A top-level agent.
    Agent {
        name: String,
        state: AgentState,
        /// Its machine, when that is not this one.
        machine: Option<MachineHeader>,
        counts: Counts,
        /// Whether anything sits under it, so it has a chevron.
        children: bool,
        expanded: bool,
    },
    /// A review run under `agent`, or under the member `agent` of `swarm`.
    Review {
        agent: String,
        swarm: Option<String>,
        index: usize,
        review: Review,
    },
    /// The browser of `agent`, or of the member `agent` of `swarm`.
    Browser {
        agent: String,
        swarm: Option<String>,
        label: String,
    },
    /// A swarm under its expanded initiator, with every member for its tab.
    Swarm {
        initiator: String,
        name: String,
        members: Vec<String>,
        /// How many of its members are listed under it.
        listed: usize,
        expanded: bool,
    },
    /// A listed member of the swarm row above it.
    Member {
        swarm: String,
        name: String,
        /// Its name without the swarm's name and hyphen in front.
        short: String,
        state: AgentState,
        machine: Option<MachineHeader>,
    },
}

impl Row {
    /// How many levels in the row sits.
    pub(super) fn depth(&self) -> usize {
        match self {
            Self::Agent { .. } => 0,
            Self::Swarm { .. } => 1,
            Self::Member { .. } => 2,
            Self::Review { swarm, .. } | Self::Browser { swarm, .. } => {
                if swarm.is_some() {
                    3
                } else {
                    1
                }
            }
        }
    }
}

/// The key of a swarm's fold: its initiator and name.
pub(super) fn swarm_key(initiator: &str, swarm: &str) -> String {
    // Names never hold a slash, so these cannot collide.
    format!("{initiator}/{swarm}")
}

/// A member's name without its swarm's name and the joining hyphen in
/// front, or the whole name when it does not start with them.
pub(super) fn short_name<'a>(swarm: &str, member: &'a str) -> &'a str {
    member
        .strip_prefix(swarm)
        .and_then(|rest| rest.strip_prefix('-'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(member)
}

/// Arrange listed agents as one tree. A swarm lists under its initiator when
/// the initiator is listed, folding whichever members are listed too, even
/// none, so it can still be opened; an agent that started a swarm of its own
/// always keeps its top-level row. Under an expanded agent come its reviews,
/// its browser and its swarms, in that order; a member's own reviews and
/// browser sit under the member.
///
/// Agents needing the user come first, then working ones, each urgency by
/// name. An agent takes the most urgent place among itself and everything
/// under it, so folding never hides attention, and an offline agent on
/// another machine goes last.
pub(super) fn tree(
    entries: &[(String, AgentState)],
    swarms: &[Swarm],
    started: impl Fn(&str) -> Started,
    machine: impl Fn(&str) -> Option<MachineHeader>,
    expanded: &HashSet<String>,
    collapsed_swarms: &HashSet<String>,
) -> Vec<Row> {
    let states: HashMap<&str, AgentState> = entries
        .iter()
        .map(|(name, state)| (name.as_str(), *state))
        .collect();
    let grouped: Vec<&Swarm> = swarms
        .iter()
        .filter(|swarm| states.contains_key(swarm.initiator.as_str()))
        .collect();
    let leads: HashSet<&str> = grouped
        .iter()
        .map(|swarm| swarm.initiator.as_str())
        .collect();
    let folded = |swarm: &Swarm| -> Vec<(String, AgentState)> {
        swarm
            .members
            .iter()
            .filter(|member| !leads.contains(member.as_str()))
            .filter_map(|member| Some((member.clone(), *states.get(member.as_str())?)))
            .collect()
    };
    let hidden: HashSet<String> = grouped
        .iter()
        .flat_map(|swarm| folded(swarm))
        .map(|(name, _)| name)
        .collect();
    let running = |started: &Started| started.reviews.iter().filter(|r| r.running).count();
    let mut groups: Vec<(Urgency, &str, Vec<Row>)> = Vec::new();
    for (name, state) in entries {
        if hidden.contains(name) {
            continue;
        }
        let own: Vec<_> = grouped
            .iter()
            .filter(|swarm| swarm.initiator == *name)
            .map(|swarm| (*swarm, folded(swarm)))
            .collect();
        let mine = started(name);
        // An agent in two of this agent's swarms is one folded agent.
        let members: HashMap<&str, AgentState> = own
            .iter()
            .flat_map(|(_, members)| members)
            .map(|(name, state)| (name.as_str(), *state))
            .collect();
        let theirs: HashMap<&str, Started> = members
            .keys()
            .map(|member| (*member, started(member)))
            .collect();
        let counts = Counts {
            reviews: running(&mine) + theirs.values().map(running).sum::<usize>(),
            members: members.len(),
            members_working: members.values().any(|state| state.working()),
            members_need_you: members.values().any(|state| state.needs_you()),
            browsers: usize::from(mine.browser.is_some())
                + theirs.values().filter(|s| s.browser.is_some()).count(),
        };
        let children = !mine.reviews.is_empty() || mine.browser.is_some() || !own.is_empty();
        let open = children && expanded.contains(name);
        let here = machine(name);
        let away =
            |name: &str, state: AgentState| state == AgentState::Offline && machine(name).is_some();
        // Anything drawn under it keeps it among the rest: a review, running
        // or queued, or a browser, its own or a member's, or a member that is
        // not itself away.
        let shows = |started: &Started| !started.reviews.is_empty() || started.browser.is_some();
        let lifted = shows(&mine)
            || theirs.values().any(shows)
            || members.iter().any(|(member, state)| !away(member, *state));
        let rank = if state.needs_you() || counts.members_need_you {
            Urgency::NeedsYou
        } else if state.working() || counts.members_working || counts.reviews > 0 {
            Urgency::Working
        } else if away(name, *state) && !lifted {
            Urgency::Away
        } else {
            Urgency::Rest
        };
        let mut rows = vec![Row::Agent {
            name: name.clone(),
            state: *state,
            machine: here,
            counts,
            children,
            expanded: open,
        }];
        if open {
            push_started(&mut rows, name, None, mine);
            for (swarm, members) in &own {
                let key = swarm_key(name, &swarm.name);
                let swarm_open = !collapsed_swarms.contains(&key);
                rows.push(Row::Swarm {
                    initiator: name.clone(),
                    name: swarm.name.clone(),
                    members: swarm.members.clone(),
                    listed: members.len(),
                    expanded: swarm_open,
                });
                for (member, state) in members.iter().filter(|_| swarm_open) {
                    rows.push(Row::Member {
                        swarm: swarm.name.clone(),
                        short: short_name(&swarm.name, member).to_owned(),
                        state: *state,
                        machine: machine(member),
                        name: member.clone(),
                    });
                    let theirs = theirs.get(member.as_str()).cloned().unwrap_or_default();
                    push_started(&mut rows, member, Some(&swarm.name), theirs);
                }
            }
        }
        groups.push((rank, name, rows));
    }
    // By name within each urgency, so an agent lifted by what is under it
    // takes its place among that urgency's agents, not after them.
    groups.sort_by(|(a, a_name, _), (b, b_name, _)| {
        (a, a_name.to_ascii_lowercase(), a_name).cmp(&(b, b_name.to_ascii_lowercase(), b_name))
    });
    groups.into_iter().flat_map(|(_, _, rows)| rows).collect()
}

/// Where an agent sorts, by itself and everything under it: one tier each,
/// earliest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Urgency {
    /// It, or something under it, needs the person.
    NeedsYou,
    /// It, or something under it, works or runs a review.
    Working,
    Rest,
    /// An offline agent on another machine with nothing under it lifting it.
    Away,
}

/// The reviews, then the browser, of `agent`.
fn push_started(rows: &mut Vec<Row>, agent: &str, swarm: Option<&str>, started: Started) {
    rows.extend(
        started
            .reviews
            .into_iter()
            .enumerate()
            .map(|(index, review)| Row::Review {
                agent: agent.to_owned(),
                swarm: swarm.map(str::to_owned),
                index,
                review,
            }),
    );
    rows.extend(started.browser.map(|label| Row::Browser {
        agent: agent.to_owned(),
        swarm: swarm.map(str::to_owned),
        label,
    }));
}

/// Offline agents whose running swarms still have a listed member. Listing
/// them keeps a swarm together while the agent that started it restarts.
pub(super) fn offline_leads(
    entries: &[(String, AgentState)],
    swarms: &[Swarm],
    state: impl Fn(&str) -> AgentState,
) -> Vec<String> {
    let listed: HashSet<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    let mut leads: Vec<String> = Vec::new();
    for swarm in swarms {
        let lead = swarm.initiator.as_str();
        if !listed.contains(lead)
            && !leads.iter().any(|name| name == lead)
            && swarm
                .members
                .iter()
                .any(|member| listed.contains(member.as_str()))
            && state(lead) == AgentState::Offline
        {
            leads.push(lead.to_owned());
        }
    }
    leads
}

/// Reachability of an agent's machine, as its tag shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MachinePresence {
    Online,
    Offline { last_seen: Option<String> },
    NotInProfile,
    Unknown,
}

impl MachinePresence {
    /// Whether this presence indicates an unreachable or departed machine.
    pub(super) fn attention(&self) -> bool {
        matches!(self, Self::Offline { .. } | Self::NotInProfile)
    }

    /// Human-readable status text beside a remote machine's name.
    pub(super) fn label(&self) -> String {
        self.label_at(now() as f64)
    }

    pub(super) fn label_at(&self, current_epoch: f64) -> String {
        match self {
            Self::Online => "online".into(),
            Self::Offline { last_seen } => match last_seen
                .as_deref()
                .and_then(epoch_seconds)
                .map(|seen| relative_seen(current_epoch - seen))
            {
                Some(when) => format!("offline · last seen {when}"),
                None => "offline".into(),
            },
            Self::NotInProfile => "not in profile".into(),
            Self::Unknown => "status unknown".into(),
        }
    }
}

fn relative_seen(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 60.0 {
        return "just now".into();
    }
    let minutes = (seconds / 60.0).floor() as u64;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", hours / 24)
}

/// Display name for a machine, sanitized and falling back to a short ID prefix.
pub(super) fn machine_display_name(name: &str, id: &str) -> String {
    let cleaned = one_line(name);
    if !cleaned.is_empty() {
        cleaned
    } else {
        id.chars().take(8).collect()
    }
}

/// Resolve a machine's presence from the current `GET /api/status/services` list.
pub(super) fn machine_presence(
    id: &str,
    machines: &[ProfileMachine],
    machines_read: bool,
) -> MachinePresence {
    match machines.iter().find(|machine| machine.id == id) {
        Some(machine) => match machine.state {
            MachineState::Online => MachinePresence::Online,
            MachineState::Offline => MachinePresence::Offline {
                last_seen: machine.last_seen.clone(),
            },
            MachineState::Unknown => MachinePresence::Unknown,
        },
        None if machines_read => MachinePresence::NotInProfile,
        None => MachinePresence::Unknown,
    }
}

/// A machine as an agent's tag shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MachineHeader {
    pub id: String,
    pub name: String,
    pub presence: MachinePresence,
}

/// The tag for an agent placed on `machine`: none when that is this machine
/// or this machine is not known, so nothing is called remote by mistake.
pub(super) fn machine_tag(
    local_machine_id: Option<&str>,
    machines: &[ProfileMachine],
    machines_read: bool,
    machine: Option<&Machine>,
) -> Option<MachineHeader> {
    let machine = machine?;
    if !local_machine_id.is_some_and(|local| local != machine.id) {
        return None;
    }
    let raw = machines
        .iter()
        .find(|known| known.id == machine.id)
        .map(|known| known.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&machine.name);
    Some(MachineHeader {
        id: machine.id.clone(),
        name: machine_display_name(raw, &machine.id),
        presence: machine_presence(&machine.id, machines, machines_read),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(entries: &[(&str, AgentState)]) -> Vec<(String, AgentState)> {
        entries
            .iter()
            .map(|(name, state)| ((*name).to_owned(), *state))
            .collect()
    }

    fn swarm(name: &str, initiator: &str, members: &[&str]) -> Swarm {
        Swarm {
            name: name.into(),
            initiator: initiator.into(),
            members: members.iter().map(|member| (*member).to_owned()).collect(),
        }
    }

    fn review(reviewer: &str, running: bool) -> Review {
        Review {
            reviewer: reviewer.into(),
            effort: Some("high".into()),
            running,
        }
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Agent { name, .. } => name.clone(),
                Row::Review { agent, review, .. } => format!("{agent}:{}", review.reviewer),
                Row::Browser { agent, label, .. } => format!("{agent}:{label}"),
                Row::Swarm { name, .. } => format!("[{name}]"),
                Row::Member { short, .. } => format!("  {short}"),
            })
            .collect()
    }

    fn header(name: &str) -> MachineHeader {
        MachineHeader {
            id: "b".repeat(64),
            name: name.into(),
            presence: MachinePresence::Online,
        }
    }

    #[test]
    fn an_offline_initiator_is_listed_while_its_swarm_has_listed_members() {
        let entries = listed(&[("fix-coder", AgentState::Idle)]);
        let swarms = [
            swarm("fix", "lead", &["fix-coder"]),
            swarm("more", "lead", &["fix-coder"]),
            // Not an agent Backbone knows, and a swarm with nobody listed.
            swarm("lost", "gone", &["fix-coder"]),
            swarm("quiet", "resting", &["absent"]),
        ];
        let state = |name: &str| match name {
            "gone" => AgentState::Unknown,
            _ => AgentState::Offline,
        };
        assert_eq!(offline_leads(&entries, &swarms, state), ["lead"]);
    }

    #[test]
    fn short_names_drop_the_swarm_name_and_hyphen_only() {
        assert_eq!(short_name("remote-detach", "remote-detach-coder"), "coder");
        assert_eq!(short_name("fix", "fix-sonnet-reviewer"), "sonnet-reviewer");
        // Not starting with the swarm's name and a hyphen: the whole name.
        assert_eq!(short_name("fix", "fixer-coder"), "fixer-coder");
        assert_eq!(short_name("fix", "helper"), "helper");
        assert_eq!(short_name("fix", "fix-"), "fix-");
        assert_eq!(short_name("fix", "fix"), "fix");
    }

    #[test]
    fn glance_counts_leave_out_zeros_and_colour_what_runs_or_needs_you() {
        assert!(Counts::default().items().is_empty());
        let counts = Counts {
            reviews: 2,
            members: 8,
            members_working: true,
            members_need_you: false,
            browsers: 1,
        };
        assert_eq!(
            counts.items(),
            [
                (Glance::Reviews, Some(2), Tone::Accent),
                (Glance::Members, Some(8), Tone::Accent),
                // One browser shows only its icon, and never in accent.
                (Glance::Browsers, None, Tone::Muted),
            ]
        );
        let quiet = Counts {
            members: 4,
            browsers: 2,
            ..Counts::default()
        };
        assert_eq!(
            quiet.items(),
            [
                (Glance::Members, Some(4), Tone::Muted),
                (Glance::Browsers, Some(2), Tone::Muted),
            ]
        );
        let waiting = Counts {
            members: 1,
            members_working: true,
            members_need_you: true,
            ..Counts::default()
        };
        assert_eq!(
            waiting.items(),
            [(Glance::Members, Some(1), Tone::Attention)]
        );
    }

    #[test]
    fn everything_an_agent_started_sits_under_it_and_folds_with_it() {
        let entries = listed(&[
            ("lead", AgentState::Working),
            ("detach-coordinator", AgentState::Working),
            ("detach-coder", AgentState::Idle),
            ("gate-coder", AgentState::Idle),
            ("other", AgentState::Idle),
        ]);
        let swarms = [
            swarm("detach", "lead", &["detach-coordinator", "detach-coder"]),
            swarm("gate", "lead", &["gate-coder", "gate-offline"]),
        ];
        let started = |name: &str| match name {
            "lead" => Started {
                reviews: vec![review("gemini", true), review("sonnet", false)],
                browser: Some("github.com".into()),
            },
            "detach-coder" => Started {
                reviews: vec![review("codex", true)],
                browser: Some("example.com".into()),
            },
            _ => Started::default(),
        };
        let counts = Counts {
            reviews: 2,
            members: 3,
            members_working: true,
            members_need_you: false,
            browsers: 2,
        };
        let open = tree(
            &entries,
            &swarms,
            started,
            |_| None,
            &HashSet::from(["lead".to_owned()]),
            &HashSet::new(),
        );
        assert_eq!(
            names(&open),
            [
                "lead",
                "lead:gemini",
                "lead:sonnet",
                "lead:github.com",
                "[detach]",
                "  coordinator",
                "  coder",
                "detach-coder:codex",
                "detach-coder:example.com",
                "[gate]",
                "  coder",
                "other",
            ]
        );
        assert_eq!(
            open.iter().map(Row::depth).collect::<Vec<_>>(),
            [0, 1, 1, 1, 1, 2, 2, 3, 3, 1, 2, 0]
        );
        assert!(matches!(
            &open[0],
            Row::Agent { counts: c, children: true, expanded: true, .. } if *c == counts
        ));
        // The swarm row keeps every member for its tab, listed or not, and
        // counts the ones listed under it.
        assert!(matches!(
            &open[9],
            Row::Swarm { members, listed: 1, expanded: true, .. } if members.len() == 2
        ));

        // Folded, nothing under it shows and its counts stay.
        let folded = tree(
            &entries,
            &swarms,
            started,
            |_| None,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&folded), ["lead", "other"]);
        assert!(matches!(
            &folded[0],
            Row::Agent { counts: c, children: true, expanded: false, .. } if *c == counts
        ));
        // Nothing under it: no chevron, and expanding it changes nothing.
        assert!(matches!(
            &folded[1],
            Row::Agent {
                children: false,
                ..
            }
        ));

        // A folded swarm keeps its row and hides its members.
        let swarm_folded = tree(
            &entries,
            &swarms,
            started,
            |_| None,
            &HashSet::from(["lead".to_owned(), "other".to_owned()]),
            &HashSet::from([swarm_key("lead", "detach")]),
        );
        assert_eq!(
            names(&swarm_folded),
            [
                "lead",
                "lead:gemini",
                "lead:sonnet",
                "lead:github.com",
                "[detach]",
                "[gate]",
                "  coder",
                "other",
            ]
        );
        assert!(matches!(
            &swarm_folded[4],
            Row::Swarm {
                expanded: false,
                ..
            }
        ));
    }

    #[test]
    fn every_running_swarm_of_an_agent_lists_under_it_as_the_service_sends_them() {
        // The shape of a live `/api/swarms` and `/api/agents`: one idle agent
        // that started three swarms of four idle members each, an ended
        // swarm of its own, and another agent's working swarm.
        let member = |name: &str, state: &str| {
            serde_json::json!({"name": name, "role": "coder", "runtime": "claude", "state": state,
                "reason": null, "detail": null, "evidence": ["runtime: claude"]})
        };
        let swarm = |name: &str, initiator: &str, status: &str, state: &str| {
            let members: Vec<_> = ["coder", "coordinator", "reviewer-1", "reviewer-2"]
                .iter()
                .map(|role| member(&format!("{name}-{role}"), state))
                .collect();
            serde_json::json!({"name": name, "repo": "example/project", "issue_number": 1,
                "initiator": initiator, "coordinator": format!("{name}-coordinator"),
                "branch": format!("swarm/{name}"), "status": status, "created_at": "2026-11-27T08:00:00Z",
                "completed_at": null, "not_ended_at": null,
                "members": if status == "active" { members } else { Vec::new() }})
        };
        let swarms = serde_json::json!({"total": 5, "items": [
            swarm("upgrade", "lead", "active", "idle"),
            swarm("sandbox", "lead", "active", "idle"),
            swarm("tree", "other", "active", "busy"),
            swarm("browse", "lead", "active", "idle"),
            swarm("ended", "lead", "disbanded", "idle"),
        ]});
        let mut agents = vec![
            serde_json::json!({"name": "lead", "state": "idle"}),
            serde_json::json!({"name": "other", "state": "busy"}),
        ];
        for item in swarms["items"].as_array().unwrap() {
            for member in item["members"].as_array().unwrap() {
                agents.push(serde_json::json!({"name": member["name"], "state": member["state"]}));
            }
        }
        let mut snapshot = agent_interface_roster::Snapshot::default();
        snapshot.update(serde_json::json!({"items": agents}).to_string().as_bytes());
        snapshot.update_swarms(Some(swarms.to_string().as_bytes()));
        assert!(snapshot.swarms_read);
        let mut entries: Vec<(String, AgentState)> = snapshot
            .states
            .iter()
            .map(|(name, state)| (name.clone(), AgentState::from_runtime(state)))
            .collect();
        entries.sort_by_key(|(name, state)| (!state.needs_you(), !state.working(), name.clone()));
        let rows = tree(
            &entries,
            &snapshot.swarms,
            |_| Started::default(),
            |_| None,
            &HashSet::from(["lead".to_owned()]),
            &HashSet::new(),
        );
        let lead = rows
            .iter()
            .position(|row| matches!(row, Row::Agent { name, .. } if name == "lead"))
            .unwrap();
        assert!(matches!(
            &rows[lead],
            Row::Agent {
                counts: Counts { members: 12, .. },
                children: true,
                expanded: true,
                ..
            }
        ));
        let under: Vec<&str> = rows[lead + 1..]
            .iter()
            .take_while(|row| row.depth() > 0)
            .filter_map(|row| match row {
                Row::Swarm {
                    name, listed: 4, ..
                } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(under, ["upgrade", "sandbox", "browse"]);
    }

    #[test]
    fn an_agent_takes_the_most_urgent_place_of_everything_under_it() {
        let entries = listed(&[
            ("m", AgentState::NeedsYou),
            ("busy", AgentState::Working),
            ("a-quiet", AgentState::Idle),
            ("reviewing", AgentState::Idle),
            ("z-lead", AgentState::Idle),
        ]);
        // A member of two of its swarms counts once.
        let twice = [swarm("s1", "z-lead", &["m"]), swarm("s2", "z-lead", &["m"])];
        let started = |name: &str| Started {
            reviews: match name {
                "reviewing" => vec![review("sonnet", true)],
                // A queued review is not running and does not lift its agent.
                "a-quiet" => vec![review("spare", false)],
                _ => Vec::new(),
            },
            browser: None,
        };
        let rows = tree(
            &entries,
            &twice,
            started,
            |_| None,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&rows), ["z-lead", "busy", "reviewing", "a-quiet"]);

        // Lifted into a busier urgency, an agent takes its place there by
        // name: an idle agent with a running review goes before a working
        // agent that sorts after it.
        let lifting = listed(&[("zeta", AgentState::Working), ("alpha", AgentState::Idle)]);
        let lifted = tree(
            &lifting,
            &[],
            |name: &str| Started {
                reviews: if name == "alpha" {
                    vec![review("sonnet", true)]
                } else {
                    Vec::new()
                },
                browser: None,
            },
            |_| None,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&lifted), ["alpha", "zeta"]);

        // Needing the person is one tier: also having working agents under it
        // does not put an agent ahead of the others there.
        let tiers = listed(&[
            ("a-waits", AgentState::NeedsYou),
            ("b-lead", AgentState::NeedsYou),
            ("b-coder", AgentState::Working),
        ]);
        let tiered = tree(
            &tiers,
            &[swarm("b", "b-lead", &["b-coder"])],
            |_| Started::default(),
            |_| None,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&tiered), ["a-waits", "b-lead"]);
        assert!(matches!(
            &rows[0],
            Row::Agent {
                counts: Counts {
                    members: 1,
                    members_need_you: true,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            &rows[3],
            Row::Agent {
                counts: Counts { reviews: 0, .. },
                children: true,
                ..
            }
        ));

        // A member leading a swarm of its own keeps its row, and the agent
        // above it still has a swarm although nothing folds under it.
        let entries = listed(&[
            ("fix-coder", AgentState::NeedsYou),
            ("fix-coordinator", AgentState::Working),
            ("lead", AgentState::Working),
        ]);
        let nested = [
            swarm("fix", "lead", &["fix-coordinator"]),
            swarm("deeper", "fix-coordinator", &["fix-coder"]),
        ];
        let rows = tree(
            &entries,
            &nested,
            |_| Started::default(),
            |_| None,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&rows), ["fix-coordinator", "lead"]);
        assert!(matches!(
            &rows[1],
            Row::Agent {
                counts: Counts { members: 0, .. },
                children: true,
                ..
            }
        ));
    }

    #[test]
    fn offline_agents_on_another_machine_are_ordinary_rows_sorted_last() {
        let entries = listed(&[
            ("busy", AgentState::Working),
            ("away", AgentState::Offline),
            ("here-offline", AgentState::Offline),
            ("remote", AgentState::Idle),
            ("zed", AgentState::Idle),
        ]);
        let machine = |name: &str| matches!(name, "away" | "remote").then(|| header("build-box"));
        let rows = tree(
            &entries,
            &[],
            |_| Started::default(),
            machine,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(
            names(&rows),
            ["busy", "here-offline", "remote", "zed", "away"]
        );
        assert!(matches!(&rows[4], Row::Agent { machine: Some(m), .. } if m.name == "build-box"));
        assert!(matches!(&rows[1], Row::Agent { machine: None, .. }));

        // Lifted by what is under it, an offline agent on another machine
        // takes that tier by name like any other agent, so folding never
        // hides attention.
        let lifting = listed(&[
            ("busy", AgentState::Working),
            ("waits", AgentState::NeedsYou),
            ("away", AgentState::Offline),
            ("away-coder", AgentState::NeedsYou),
            ("drift", AgentState::Offline),
            ("drift-coder", AgentState::Working),
        ]);
        let lifted = tree(
            &lifting,
            &[
                swarm("away", "away", &["away-coder"]),
                swarm("drift", "drift", &["drift-coder"]),
            ],
            |_| Started::default(),
            |name| matches!(name, "away" | "drift").then(|| header("build-box")),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&lifted), ["away", "waits", "busy", "drift"]);

        // An offline agent on another machine kept for its swarm, whose
        // member here is idle, sorts among the rest by name, not last.
        let kept = listed(&[
            ("lead", AgentState::Offline),
            ("lead-coder", AgentState::Idle),
            ("quiet", AgentState::Idle),
            ("gone", AgentState::Offline),
        ]);
        let rows = tree(
            &kept,
            &[swarm("s", "lead", &["lead-coder"])],
            |_| Started::default(),
            |name| matches!(name, "lead" | "gone").then(|| header("build-box")),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&rows), ["lead", "quiet", "gone"]);

        // Kept up too by what is drawn under a member that is itself away:
        // its queued review, or its browser.
        let kept = listed(&[
            ("lead", AgentState::Offline),
            ("lead-coder", AgentState::Offline),
            ("other", AgentState::Offline),
            ("other-coder", AgentState::Offline),
            ("quiet", AgentState::Idle),
            ("gone", AgentState::Offline),
        ]);
        let rows = tree(
            &kept,
            &[
                swarm("lead", "lead", &["lead-coder"]),
                swarm("other", "other", &["other-coder"]),
            ],
            |name| Started {
                reviews: if name == "lead-coder" {
                    vec![review("spare", false)]
                } else {
                    Vec::new()
                },
                browser: (name == "other-coder").then(|| "example.com".into()),
            },
            |name| (name != "quiet").then(|| header("build-box")),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(names(&rows), ["lead", "other", "quiet", "gone"]);
    }

    #[test]
    fn a_machine_tag_shows_only_another_known_machine() {
        let local = "a".repeat(64);
        let remote = "b".repeat(64);
        let departed = "d".repeat(64);
        let machines = [
            ProfileMachine {
                id: local.clone(),
                name: "laptop".into(),
                home: true,
                state: MachineState::Online,
                joined_at: None,
                last_seen: None,
            },
            ProfileMachine {
                id: remote.clone(),
                name: "build-box".into(),
                home: false,
                state: MachineState::Offline,
                joined_at: None,
                last_seen: Some("2026-11-22T11:55:00Z".into()),
            },
        ];
        let on = |id: &str, name: &str| Machine {
            id: id.into(),
            name: name.into(),
        };
        // This machine, no machine, or an unknown local machine: no tag.
        assert_eq!(
            machine_tag(Some(&local), &machines, true, Some(&on(&local, "laptop"))),
            None
        );
        assert_eq!(machine_tag(Some(&local), &machines, true, None), None);
        assert_eq!(
            machine_tag(None, &machines, true, Some(&on(&remote, "x"))),
            None
        );
        // The profile's name wins over the agent's cached one.
        let tag = machine_tag(Some(&local), &machines, true, Some(&on(&remote, "old"))).unwrap();
        assert_eq!(tag.name, "build-box");
        let now = epoch_seconds("2026-11-22T12:00:00Z").unwrap();
        assert_eq!(tag.presence.label_at(now), "offline · last seen 5m ago");
        assert!(tag.presence.attention());
        // A machine no longer in the profile, or one read without services.
        let gone = machine_tag(
            Some(&local),
            &machines,
            true,
            Some(&on(&departed, "departed-box")),
        )
        .unwrap();
        assert_eq!(
            (gone.name.as_str(), &gone.presence),
            ("departed-box", &MachinePresence::NotInProfile)
        );
        let unread = machine_tag(Some(&local), &[], false, Some(&on(&departed, ""))).unwrap();
        assert_eq!(unread.name, "dddddddd");
        assert_eq!(unread.presence, MachinePresence::Unknown);
    }
}
