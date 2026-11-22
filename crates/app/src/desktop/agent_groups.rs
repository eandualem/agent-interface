//! Swarm members folded under the agent that started their swarm, and
//! multi-machine roster grouping by full machine ID.

use std::collections::{BTreeMap, HashMap, HashSet};

use agent_interface_roster::{
    Machine, MachineState, ProfileMachine, Swarm,
    profiles::setup::one_line,
    views::{epoch_seconds, now},
};

use super::agent_status::AgentState;

/// One line of the agents list.
#[derive(Debug, PartialEq)]
pub(super) enum Row {
    /// A top-level agent, with the swarm members folded under it.
    Agent {
        name: String,
        state: AgentState,
        /// How many running swarms list under this agent; the row expands when any do.
        swarms: usize,
        /// How many listed agents are folded under this one.
        members: usize,
        /// Whether a folded member needs the user.
        needs_you: bool,
        expanded: bool,
    },
    /// A swarm under its expanded initiator, with every member for its tab.
    Swarm {
        initiator: String,
        name: String,
        members: Vec<String>,
    },
    /// A listed member of the swarm row above it.
    Member {
        swarm: String,
        name: String,
        state: AgentState,
    },
}

/// Arrange listed agents. A swarm lists under its initiator when the initiator
/// is listed, folding whichever members are listed too, even none, so it can
/// still be opened; an agent that started a swarm of its own
/// always keeps its top-level row. `entries` arrive with agents needing the
/// user first, then working ones; an agent takes the most urgent place among
/// itself and the members folded under it, so folding never hides attention.
pub(super) fn rows(
    entries: &[(String, AgentState)],
    swarms: &[Swarm],
    expanded: &HashSet<String>,
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
    let mut groups: Vec<((bool, bool), Vec<Row>)> = Vec::new();
    for (name, state) in entries {
        if hidden.contains(name) {
            continue;
        }
        let own: Vec<_> = grouped
            .iter()
            .filter(|swarm| swarm.initiator == *name)
            .map(|swarm| (*swarm, folded(swarm)))
            .collect();
        let open = expanded.contains(name);
        let all = || own.iter().flat_map(|(_, members)| members);
        let needs_you = all().any(|(_, state)| state.needs_you());
        let working = all().any(|(_, state)| state.working());
        let mut rows = vec![Row::Agent {
            name: name.clone(),
            state: *state,
            swarms: own.len(),
            // An agent in two of this agent's swarms is one folded agent.
            members: all().map(|(name, _)| name).collect::<HashSet<_>>().len(),
            needs_you,
            expanded: open,
        }];
        for (swarm, members) in own.into_iter().filter(|_| open) {
            rows.push(Row::Swarm {
                initiator: name.clone(),
                name: swarm.name.clone(),
                members: swarm.members.clone(),
            });
            rows.extend(members.into_iter().map(|(name, state)| Row::Member {
                swarm: swarm.name.clone(),
                name,
                state,
            }));
        }
        groups.push((
            (
                !(state.needs_you() || needs_you),
                !(state.working() || working),
            ),
            rows,
        ));
    }
    // Stable, so agents keep their given order within each urgency.
    groups.sort_by_key(|(urgency, _)| *urgency);
    groups.into_iter().flat_map(|(_, rows)| rows).collect()
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

/// Reachability of a machine group as shown in the roster.
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

/// Header metadata for a remote machine group in the sidebar roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MachineHeader {
    pub id: String,
    pub name: String,
    pub presence: MachinePresence,
}

/// One section of the sidebar roster: either local (including `machine: null`)
/// or one remote machine keyed by its full ID.
#[derive(Debug, PartialEq)]
pub(super) struct MachineSection {
    pub machine: Option<MachineHeader>,
    pub rows: Vec<Row>,
    pub offline_agents: Vec<String>,
}

/// Group sidebar roster entries by full machine ID.
///
/// A one-machine profile (no other machine in `machines` and no agent hosted
/// on another machine) returns `(false, [local_section])` so it renders
/// identically to today.
#[allow(clippy::too_many_arguments)]
pub(super) fn grouped_by_machine(
    local_machine_id: Option<&str>,
    machines: &[ProfileMachine],
    machines_read: bool,
    entries: &[(String, AgentState)],
    offline_agents: &[(String, Option<Machine>)],
    machine_of: impl Fn(&str) -> Option<Machine>,
    swarms: &[Swarm],
    expanded: &HashSet<String>,
) -> (bool, Vec<MachineSection>) {
    let is_remote = |id: &str| local_machine_id.is_some_and(|local| local != id);
    let has_remote_profile_machine = machines.iter().any(|machine| is_remote(&machine.id));
    let mut local_entries: Vec<(String, AgentState)> = Vec::new();
    let mut remote_active: BTreeMap<String, Vec<(String, AgentState)>> = BTreeMap::new();
    let mut remote_names: HashMap<String, String> = HashMap::new();
    let listed_names: HashSet<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();

    for (name, state) in entries {
        match machine_of(name) {
            Some(machine) if is_remote(&machine.id) => {
                remote_names
                    .entry(machine.id.clone())
                    .or_insert_with(|| machine.name.clone());
                remote_active
                    .entry(machine.id)
                    .or_default()
                    .push((name.clone(), *state));
            }
            // Local machine match OR `machine: null` (never started) stays local.
            _ => local_entries.push((name.clone(), *state)),
        }
    }

    let mut remote_offline: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, machine) in offline_agents {
        // An offline agent already listed in `entries` (e.g. a swarm initiator or
        // an offline agent with background runs) must not appear twice.
        if listed_names.contains(name.as_str()) {
            continue;
        }
        if let Some(machine) = machine
            && is_remote(&machine.id)
        {
            remote_names
                .entry(machine.id.clone())
                .or_insert_with(|| machine.name.clone());
            remote_offline
                .entry(machine.id.clone())
                .or_default()
                .push(name.clone());
        }
    }
    for names in remote_offline.values_mut() {
        names.sort_by(|a, b| {
            a.to_ascii_lowercase()
                .cmp(&b.to_ascii_lowercase())
                .then(a.cmp(b))
        });
    }

    let multi_machine =
        has_remote_profile_machine || !remote_active.is_empty() || !remote_offline.is_empty();
    if !multi_machine {
        return (
            false,
            vec![MachineSection {
                machine: None,
                rows: rows(entries, swarms, expanded),
                offline_agents: Vec::new(),
            }],
        );
    }

    // Only fold swarm members that live in the same section, so a remote member
    // never hides under a local initiator's row and cross-machine swarms never
    // leave a phantom empty swarm disclosure under the initiator.
    let section_rows = |section_entries: &[(String, AgentState)]| {
        let names: HashSet<&str> = section_entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let scoped: Vec<Swarm> = swarms
            .iter()
            .filter(|swarm| names.contains(swarm.initiator.as_str()))
            .filter_map(|swarm| {
                let members: Vec<String> = swarm
                    .members
                    .iter()
                    .filter(|member| {
                        // Drop members listed in another section; keep members in
                        // this section and offline members that aren't listed elsewhere.
                        names.contains(member.as_str()) || !listed_names.contains(member.as_str())
                    })
                    .cloned()
                    .collect();
                let keep = !members.is_empty() || swarm.members.is_empty();
                keep.then(|| Swarm {
                    name: swarm.name.clone(),
                    initiator: swarm.initiator.clone(),
                    members,
                })
            })
            .collect();
        rows(section_entries, &scoped, expanded)
    };

    let mut sections = Vec::new();
    if !local_entries.is_empty() {
        sections.push(MachineSection {
            machine: None,
            rows: section_rows(&local_entries),
            offline_agents: Vec::new(),
        });
    }

    // Order known remote machines in the service's order, then unknown IDs.
    let mut ordered_ids: Vec<String> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    for machine in machines {
        if is_remote(&machine.id)
            && (remote_active.contains_key(&machine.id) || remote_offline.contains_key(&machine.id))
            && seen_ids.insert(machine.id.clone())
        {
            ordered_ids.push(machine.id.clone());
        }
    }
    let mut unknown_ids: Vec<(String, String)> = remote_active
        .keys()
        .chain(remote_offline.keys())
        .filter(|id| !seen_ids.contains(*id))
        .map(|id| {
            let display =
                machine_display_name(remote_names.get(id).map(String::as_str).unwrap_or(""), id);
            (display.to_ascii_lowercase(), id.clone())
        })
        .collect();
    unknown_ids.sort();
    unknown_ids.dedup();
    ordered_ids.extend(unknown_ids.into_iter().map(|(_, id)| id));

    for id in ordered_ids {
        let profile_machine = machines.iter().find(|machine| machine.id == id);
        let raw_name = profile_machine
            .map(|machine| machine.name.as_str())
            .filter(|name| !name.trim().is_empty())
            .or_else(|| remote_names.get(&id).map(String::as_str))
            .unwrap_or("");
        let name = machine_display_name(raw_name, &id);
        let presence = machine_presence(&id, machines, machines_read);
        let active = remote_active.remove(&id).unwrap_or_default();
        let offline = remote_offline.remove(&id).unwrap_or_default();
        sections.push(MachineSection {
            machine: Some(MachineHeader { id, name, presence }),
            rows: section_rows(&active),
            offline_agents: offline,
        });
    }

    (true, sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offline_initiator_is_listed_while_its_swarm_has_listed_members() {
        let entries = vec![("fix-coder".to_owned(), AgentState::Idle)];
        let swarm = |name: &str, initiator: &str, member: &str| Swarm {
            name: name.into(),
            initiator: initiator.into(),
            members: vec![member.into()],
        };
        let swarms = [
            swarm("fix", "lead", "fix-coder"),
            swarm("more", "lead", "fix-coder"),
            // Not an agent Backbone knows, and a swarm with nobody listed.
            swarm("lost", "gone", "fix-coder"),
            swarm("quiet", "resting", "absent"),
        ];
        let state = |name: &str| match name {
            "gone" => AgentState::Unknown,
            _ => AgentState::Offline,
        };
        assert_eq!(offline_leads(&entries, &swarms, state), ["lead"]);
    }

    #[test]
    fn members_fold_under_a_listed_initiator_and_show_when_expanded() {
        let entries: Vec<(String, AgentState)> = [
            ("fix-coder", AgentState::NeedsYou),
            ("lead", AgentState::Working),
            ("fix-coordinator", AgentState::Working),
            ("other", AgentState::Idle),
            ("lost-coder", AgentState::Idle),
        ]
        .map(|(name, state)| (name.to_owned(), state))
        .to_vec();
        let swarm = |name: &str, initiator: &str, members: &[&str]| Swarm {
            name: name.into(),
            initiator: initiator.into(),
            members: members.iter().map(|member| (*member).to_owned()).collect(),
        };
        let swarms = [
            swarm(
                "fix",
                "lead",
                &["fix-coordinator", "fix-coder", "fix-offline"],
            ),
            // Its initiator is not listed, so its member stays where it was.
            swarm("lost", "gone", &["lost-coder"]),
        ];
        let agent = |name: &str, state, members: usize, needs_you, expanded| Row::Agent {
            name: name.into(),
            state,
            swarms: members.min(1),
            members,
            needs_you,
            expanded,
        };
        let closed = rows(&entries, &swarms, &HashSet::new());
        assert_eq!(
            closed,
            [
                agent("lead", AgentState::Working, 2, true, false),
                agent("other", AgentState::Idle, 0, false, false),
                agent("lost-coder", AgentState::Idle, 0, false, false),
            ]
        );
        let open = rows(&entries, &swarms, &HashSet::from(["lead".to_owned()]));
        assert_eq!(open.len(), 6);
        assert_eq!(open[0], agent("lead", AgentState::Working, 2, true, true));
        assert_eq!(
            open[1],
            Row::Swarm {
                initiator: "lead".into(),
                name: "fix".into(),
                members: vec![
                    "fix-coordinator".into(),
                    "fix-coder".into(),
                    "fix-offline".into()
                ],
            }
        );
        assert!(matches!(&open[2], Row::Member { name, .. } if name == "fix-coordinator"));
        assert!(matches!(&open[3], Row::Member { name, .. } if name == "fix-coder"));

        // A member leading a swarm of its own keeps its row, and the agent
        // above it still expands although nothing folds under it.
        let nested = [
            swarm("fix", "lead", &["fix-coordinator"]),
            swarm("deeper", "fix-coordinator", &["fix-coder"]),
        ];
        let closed = rows(&entries, &nested, &HashSet::new());
        // The coordinator comes first: its folded member needs the user.
        assert!(matches!(
            &closed[0],
            Row::Agent { name, swarms: 1, members: 1, .. } if name == "fix-coordinator"
        ));
        assert!(matches!(
            &closed[1],
            Row::Agent { name, swarms: 1, members: 0, .. } if name == "lead"
        ));

        // A swarm with no listed member still lists under its initiator.
        let quiet = rows(
            &entries,
            &[swarm("empty", "other", &["offline"])],
            &HashSet::from(["other".to_owned()]),
        );
        assert!(quiet.contains(&Row::Swarm {
            initiator: "other".into(),
            name: "empty".into(),
            members: vec!["offline".into()],
        }));

        // An idle agent rises above working ones when a folded member needs
        // the user, and a member of two of its swarms counts once.
        let entries: Vec<(String, AgentState)> = [
            ("m", AgentState::NeedsYou),
            ("busy", AgentState::Working),
            ("z-lead", AgentState::Idle),
        ]
        .map(|(name, state)| (name.to_owned(), state))
        .to_vec();
        let twice = [swarm("s1", "z-lead", &["m"]), swarm("s2", "z-lead", &["m"])];
        let raised = rows(&entries, &twice, &HashSet::new());
        assert!(matches!(
            &raised[0],
            Row::Agent { name, swarms: 2, members: 1, needs_you: true, .. } if name == "z-lead"
        ));
        assert!(matches!(&raised[1], Row::Agent { name, .. } if name == "busy"));
    }

    #[test]
    fn one_machine_profile_stays_unchanged_and_handles_null_machine() {
        let local_id = "a".repeat(64);
        let machines = [ProfileMachine {
            id: local_id.clone(),
            name: "laptop".into(),
            home: true,
            state: MachineState::Online,
            joined_at: None,
            last_seen: None,
        }];
        let entries = vec![
            ("worker".into(), AgentState::Working),
            ("unplaced".into(), AgentState::Idle),
        ];
        let (multi, sections) = grouped_by_machine(
            Some(&local_id),
            &machines,
            true,
            &entries,
            &[("never-started".into(), None)],
            |name| {
                (name == "worker").then(|| Machine {
                    id: local_id.clone(),
                    name: "laptop".into(),
                })
            },
            &[],
            &HashSet::new(),
        );
        assert!(!multi);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].machine.is_none());
        assert_eq!(sections[0].rows.len(), 2);
    }

    #[test]
    fn multi_machine_groups_by_full_64_hex_id_and_tracks_offline_unknown_and_departed() {
        let local_id = "a".repeat(64);
        let remote_a = format!("{}b", "a".repeat(63));
        let remote_b = format!("{}c", "a".repeat(63));
        let unknown_id = "d".repeat(64);
        let machines = vec![
            ProfileMachine {
                id: local_id.clone(),
                name: "laptop".into(),
                home: true,
                state: MachineState::Online,
                joined_at: None,
                last_seen: Some("2026-11-22T12:00:00Z".into()),
            },
            ProfileMachine {
                id: remote_a.clone(),
                name: "build-box".into(),
                home: false,
                state: MachineState::Online,
                joined_at: None,
                last_seen: Some("2026-11-22T12:00:00Z".into()),
            },
            // Same display name as remote_a, different 64-hex ID!
            ProfileMachine {
                id: remote_b.clone(),
                name: "build-box".into(),
                home: false,
                state: MachineState::Offline,
                joined_at: None,
                last_seen: Some("2026-11-22T11:55:00Z".into()),
            },
        ];
        let entries = vec![
            ("local-agent".into(), AgentState::Working),
            ("null-agent".into(), AgentState::Idle),
            ("alpha".into(), AgentState::Working),
            ("beta".into(), AgentState::Idle),
            ("orphan".into(), AgentState::Unknown),
        ];
        let map: HashMap<&str, Machine> = HashMap::from([
            (
                "local-agent",
                Machine {
                    id: local_id.clone(),
                    name: "laptop".into(),
                },
            ),
            (
                "alpha",
                Machine {
                    id: remote_a.clone(),
                    name: "build-box".into(),
                },
            ),
            (
                "beta",
                Machine {
                    id: remote_b.clone(),
                    name: "build-box".into(),
                },
            ),
            (
                "orphan",
                Machine {
                    id: unknown_id.clone(),
                    name: "departed-box".into(),
                },
            ),
        ]);
        let (multi, sections) = grouped_by_machine(
            Some(&local_id),
            &machines,
            true,
            &entries,
            &[(
                "stopped-remote".into(),
                Some(Machine {
                    id: remote_a.clone(),
                    name: "build-box".into(),
                }),
            )],
            |name| map.get(name).cloned(),
            &[],
            &HashSet::new(),
        );
        assert!(multi);
        assert_eq!(sections.len(), 4);
        // Local section includes local-agent and machine: null agent.
        assert!(sections[0].machine.is_none());
        assert_eq!(sections[0].rows.len(), 2);
        // Remote A: online + 1 offline agent.
        let header_a = sections[1].machine.as_ref().unwrap();
        assert_eq!(header_a.id, remote_a);
        assert_eq!(header_a.presence, MachinePresence::Online);
        assert_eq!(sections[1].offline_agents, ["stopped-remote"]);
        // Remote B: distinct group despite identical display name, offline with last_seen.
        let header_b = sections[2].machine.as_ref().unwrap();
        assert_eq!(header_b.id, remote_b);
        let now_epoch = epoch_seconds("2026-11-22T12:00:00Z").unwrap();
        assert_eq!(
            header_b.presence.label_at(now_epoch),
            "offline · last seen 5m ago"
        );
        // Unknown machine ID when services was read -> NotInProfile.
        let header_unknown = sections[3].machine.as_ref().unwrap();
        assert_eq!(header_unknown.id, unknown_id);
        assert_eq!(header_unknown.name, "departed-box");
        assert_eq!(header_unknown.presence, MachinePresence::NotInProfile);

        // When services was not read (older service), unknown ID shows Unknown instead.
        let (_, unread_sections) = grouped_by_machine(
            Some(&local_id),
            &[],
            false,
            &[("orphan".into(), AgentState::Idle)],
            &[],
            |name| map.get(name).cloned(),
            &[],
            &HashSet::new(),
        );
        assert_eq!(
            unread_sections[0].machine.as_ref().unwrap().presence,
            MachinePresence::Unknown
        );

        // Once the remote machine leaves and no agents remain on it, profile reverts to one-machine.
        let (multi_after_leave, after_leave) = grouped_by_machine(
            Some(&local_id),
            &machines[..1],
            true,
            &[("local-agent".into(), AgentState::Working)],
            &[],
            |name| map.get(name).cloned(),
            &[],
            &HashSet::new(),
        );
        assert!(!multi_after_leave);
        assert_eq!(after_leave.len(), 1);
        assert!(after_leave[0].machine.is_none());
    }

    #[test]
    fn unknown_local_id_never_treats_machines_as_remote_and_swarms_scope_cleanly() {
        let local_id = "a".repeat(64);
        let remote_id = "b".repeat(64);
        let machines = vec![
            ProfileMachine {
                id: local_id.clone(),
                name: "laptop".into(),
                home: true,
                state: MachineState::Online,
                joined_at: None,
                last_seen: None,
            },
            ProfileMachine {
                id: remote_id.clone(),
                name: "build-box".into(),
                home: false,
                state: MachineState::Online,
                joined_at: None,
                last_seen: None,
            },
        ];
        let map: HashMap<&str, Machine> = HashMap::from([
            (
                "local-lead",
                Machine {
                    id: local_id.clone(),
                    name: "laptop".into(),
                },
            ),
            (
                "remote-lead",
                Machine {
                    id: remote_id.clone(),
                    name: "build-box".into(),
                },
            ),
            (
                "remote-coder",
                Machine {
                    id: remote_id.clone(),
                    name: "build-box".into(),
                },
            ),
            (
                "cross-member",
                Machine {
                    id: remote_id.clone(),
                    name: "build-box".into(),
                },
            ),
        ]);

        // Without a known local machine ID, nothing is classified as remote.
        let (multi_none, sections_none) = grouped_by_machine(
            None,
            &machines[..1],
            true,
            &[("local-lead".into(), AgentState::Working)],
            &[],
            |name| map.get(name).cloned(),
            &[],
            &HashSet::new(),
        );
        assert!(!multi_none);
        assert_eq!(sections_none.len(), 1);
        assert!(sections_none[0].machine.is_none());

        // Cross-machine swarm does not leave a phantom disclosure on local-lead,
        // and an offline remote initiator already in `entries` is not duplicated in offline_agents.
        let entries = vec![
            ("local-lead".into(), AgentState::Working),
            ("remote-lead".into(), AgentState::Offline),
            ("remote-coder".into(), AgentState::Working),
            ("cross-member".into(), AgentState::Idle),
        ];
        let swarms = vec![
            Swarm {
                name: "cross".into(),
                initiator: "local-lead".into(),
                members: vec!["cross-member".into()],
            },
            Swarm {
                name: "remote-fix".into(),
                initiator: "remote-lead".into(),
                members: vec!["remote-coder".into()],
            },
        ];
        let (multi, sections) = grouped_by_machine(
            Some(&local_id),
            &machines,
            true,
            &entries,
            &[(
                "remote-lead".into(),
                Some(Machine {
                    id: remote_id.clone(),
                    name: "build-box".into(),
                }),
            )],
            |name| map.get(name).cloned(),
            &swarms,
            &HashSet::new(),
        );
        assert!(multi);
        assert_eq!(sections.len(), 2);
        // Local section: local-lead has swarms == 0 (no phantom empty swarm).
        assert_eq!(
            sections[0].rows,
            [Row::Agent {
                name: "local-lead".into(),
                state: AgentState::Working,
                swarms: 0,
                members: 0,
                needs_you: false,
                expanded: false,
            }]
        );
        // Remote section: remote-lead folds remote-coder, cross-member stays visible,
        // and remote-lead is NOT duplicated in offline_agents.
        assert!(sections[1].offline_agents.is_empty());
        assert_eq!(
            sections[1].rows,
            [
                Row::Agent {
                    name: "remote-lead".into(),
                    state: AgentState::Offline,
                    swarms: 1,
                    members: 1,
                    needs_you: false,
                    expanded: false,
                },
                Row::Agent {
                    name: "cross-member".into(),
                    state: AgentState::Idle,
                    swarms: 0,
                    members: 0,
                    needs_you: false,
                    expanded: false,
                },
            ]
        );
    }
}
