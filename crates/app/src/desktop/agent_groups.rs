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

/// Arrange listed agents, keeping `entries` order. A swarm folds its listed
/// members under its initiator when the initiator is listed too; an agent that
/// started a swarm of its own always keeps its top-level row.
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
        .filter(|swarm| {
            states.contains_key(swarm.initiator.as_str())
                && swarm
                    .members
                    .iter()
                    .any(|member| states.contains_key(member.as_str()))
        })
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
    let mut rows = Vec::new();
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
        rows.push(Row::Agent {
            name: name.clone(),
            state: *state,
            members: own.iter().map(|(_, members)| members.len()).sum(),
            needs_you: own
                .iter()
                .flat_map(|(_, members)| members)
                .any(|(_, state)| state.needs_you()),
            expanded: open,
        });
        if !open {
            continue;
        }
        for (swarm, members) in own {
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
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let agent = |name: &str, state, members, needs_you, expanded| Row::Agent {
            name: name.into(),
            state,
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
    }
}
