//! Swarm members folded under the agent that started their swarm.

use std::collections::{HashMap, HashSet};

use agent_interface_roster::Swarm;

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
}
