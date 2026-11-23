//! A native card whose saved registration and connection never change.

use super::{Target as Row, agent, list, ui};
use crate::desktop::{
    agent_status::{AgentState, LiveAgents, Status},
    design::{colors, tokens},
};
use agent_interface_core::{AgentSection, View};
use agent_interface_roster::{
    library_snapshots::{self, Kind},
    profiles::{
        Connection, Error as ProfileError,
        runtime_context::{self, Target},
    },
    views::{Agent, Part, content},
};
use gpui::{Context, Task, Window, div, prelude::*, px};
use std::{cell::Cell, rc::Rc, time::Duration};

struct Reader {
    connection: Connection,
    name: String,
    registration: String,
    target: Option<Target>,
}

struct Snapshot {
    agent: Agent,
}

impl Reader {
    fn read(&mut self, section: AgentSection) -> Result<Snapshot, runtime_context::Error> {
        let mut agent = if let Some(target) = &self.target {
            target.refresh()?
        } else {
            let inspection = runtime_context::inspect(
                self.connection.clone(),
                &self.name,
                Some(&self.registration),
            )?;
            self.target = Some(inspection.target);
            inspection.agent
        };
        let part = match section {
            AgentSection::Skills => Part::Skills,
            AgentSection::Policies => Part::Policies,
            AgentSection::Usage => Part::Usage,
            _ => Part::Card,
        };
        // A section's own read failing leaves the card; the section says why.
        let _ = content::agent_part(&self.connection, &part, &mut agent, &|| true);
        if let Some(Ok(loadout)) = &mut agent.skills
            && !loadout.skills.is_empty()
        {
            attach_skill_artifacts(&self.connection, &mut loadout.skills);
        }
        Ok(Snapshot { agent })
    }
}

/// Give each store skill the identity the profile's skill catalog lists for
/// it, so opening its row reads that skill's own snapshot. The preview names a
/// skill but carries no identity; a skill the catalog cannot match opens
/// without a snapshot.
fn attach_skill_artifacts(
    connection: &Connection,
    skills: &mut [agent_interface_roster::views::Skill],
) {
    let Ok(catalog) = library_snapshots::list(connection, Kind::Skill, &|| true) else {
        return;
    };
    for skill in skills {
        if let Some(entry) = catalog
            .iter()
            .find(|entry| entry.name == skill.name && entry.path == skill.path)
        {
            skill.artifact_id = entry
                .artifact
                .as_ref()
                .map(|artifact| artifact.id().to_owned());
        }
    }
}

/// A skill or policy row the person chose, for the pane to open in its reader
/// with the agent as the card last read it.
pub(super) struct Opened {
    pub target: Row,
    pub agent: Agent,
    pub section: AgentSection,
}

/// The session of a run's view the person chose, to open beside the card.
pub(super) struct RunView(pub String);

/// The agent's state as the card last read it.
fn state(agent: &Agent) -> AgentState {
    AgentState::from_runtime(&agent.state)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Removal {
    None,
    Confirming,
    Running,
    Done,
}

/// The card's registration is no longer the agent under this name.
const REPLACED: &str =
    "This agent was removed or replaced. Reopen it from Team if it is still listed.";

fn removal_refusal(error: ProfileError) -> &'static str {
    match error {
        ProfileError::RegistrationChanged => REPLACED,
        ProfileError::Refused(409) => "This agent is still running. Try again in a moment.",
        ProfileError::Refused(401 | 403) => "This profile refused to remove the agent.",
        ProfileError::Refused(_) => "The profile could not remove this agent.",
        _ => "The workspace profile connection is unavailable.",
    }
}

/// Remove the agent only if it is still the registration the card shows. The
/// provider checks the registration again on stop and delete, so a replacement
/// made after this check is refused rather than removed.
fn remove_checked(
    connection: &Connection,
    name: &str,
    registration: &str,
) -> Result<(), &'static str> {
    match runtime_context::inspect(connection.clone(), name, Some(registration)) {
        Ok(_) => connection
            .remove_agent(name, registration)
            .map_err(removal_refusal),
        // No agent has this name any more, so there is nothing to remove.
        Err(runtime_context::Error::Unsupported) => Ok(()),
        Err(runtime_context::Error::RegistrationChanged) => Err(REPLACED),
        Err(_) => Err("Couldn't confirm this is the same agent. Try again."),
    }
}

pub(super) struct Card {
    connection: Connection,
    /// The profile's roster, whose book of runs the Overview lists.
    agents: LiveAgents,
    /// The registration this card shows; removal checks it first.
    registration: String,
    removal: Removal,
    removal_error: Option<&'static str>,
    action_in_flight: bool,
    action_notice: Option<String>,
    reader: Option<Reader>,
    snapshot: Option<Snapshot>,
    section: AgentSection,
    name: String,
    error: Option<&'static str>,
    visible: Rc<Cell<bool>>,
    /// Seconds since the last read began; `u64::MAX` before the first.
    waited: u64,
    _poll: Task<()>,
}

impl Card {
    pub fn new(
        connection: Connection,
        agents: LiveAgents,
        view: &View,
        visible: Rc<Cell<bool>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = view.agent.clone().expect("validated agent view");
        let registration = view
            .registration_id
            .clone()
            .expect("validated registration");
        let poll = cx.spawn(async move |this, cx| {
            loop {
                // An agent's libraries change rarely. Waking every three
                // seconds lets a section change take its own pace at once.
                let Ok(()) = this.update(cx, |card, cx| {
                    let every = match card.section {
                        AgentSection::Skills | AgentSection::Policies => 30,
                        AgentSection::Usage => 15,
                        _ => 3,
                    };
                    if card.waited >= every {
                        card.refresh(cx);
                    }
                    card.waited = card.waited.saturating_add(3);
                }) else {
                    return;
                };
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        });
        Self {
            connection: connection.clone(),
            agents,
            registration: registration.clone(),
            removal: Removal::None,
            removal_error: None,
            action_in_flight: false,
            action_notice: None,
            reader: Some(Reader {
                connection,
                name: name.clone(),
                registration,
                target: None,
            }),
            snapshot: None,
            section: view.section.unwrap_or_default(),
            name,
            error: None,
            visible,
            waited: u64::MAX,
            _poll: poll,
        }
    }

    fn open_row(&mut self, target: Row, cx: &mut Context<Self>) {
        if let Some(snapshot) = &self.snapshot {
            cx.emit(Opened {
                target,
                agent: snapshot.agent.clone(),
                section: self.section,
            });
        }
    }

    fn open_view(&mut self, session: String, cx: &mut Context<Self>) {
        cx.emit(RunView(session));
    }

    /// Runs only after the person confirmed it. A closed pane still finishes the
    /// removal it asked for; only the result goes unseen.
    fn remove(&mut self, cx: &mut Context<Self>) {
        if self.removal != Removal::Confirming {
            return;
        }
        self.removal = Removal::Running;
        self.removal_error = None;
        let connection = self.connection.clone();
        let name = self.name.clone();
        let registration = self.registration.clone();
        let work = cx
            .background_executor()
            .spawn(async move { remove_checked(&connection, &name, &registration) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |card, cx| {
                match result {
                    Ok(()) => {
                        card.removal = Removal::Done;
                        card.snapshot = None;
                        card.error = None;
                    }
                    Err(message) => {
                        card.removal = Removal::None;
                        card.removal_error = Some(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run_lifecycle(&mut self, action: super::TeamAgentAction, cx: &mut Context<Self>) {
        if self.action_in_flight || self.removal == Removal::Done {
            return;
        }
        self.action_in_flight = true;
        let connection = self.connection.clone();
        let name = self.name.clone();
        let registration = self.registration.clone();
        self.action_notice = Some(match action {
            super::TeamAgentAction::Start { resume: true } => format!("Resuming {name}…"),
            super::TeamAgentAction::Start { resume: false } => format!("Starting {name}…"),
            super::TeamAgentAction::Stop => format!("Stopping {name}…"),
            super::TeamAgentAction::Restart { .. } => format!("Restarting {name}…"),
        });
        let work = cx.background_executor().spawn(async move {
            match action {
                super::TeamAgentAction::Start { resume } => {
                    connection.start_agent(&name, resume).roster_message()
                }
                super::TeamAgentAction::Stop => {
                    connection.stop_agent(&name, Some(&registration)).message()
                }
                super::TeamAgentAction::Restart { resume } => {
                    connection.restart_agent(&name, resume).message()
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let message = work.await;
            let _ = this.update(cx, |card, cx| {
                card.action_in_flight = false;
                card.action_notice = Some(message);
                card.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_section(&mut self, section: AgentSection, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            self.refresh(cx);
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.visible.get() || self.removal == Removal::Done {
            return;
        }
        let Some(mut reader) = self.reader.take() else {
            return;
        };
        let section = self.section;
        self.waited = 0;
        let work = cx.background_executor().spawn(async move {
            let result = reader.read(section);
            (reader, result)
        });
        cx.spawn(async move |this, cx| {
            let (reader, result) = work.await;
            let _ = this.update(cx, |card, cx| {
                card.reader = Some(reader);
                if card.removal == Removal::Done {
                    return;
                }
                if card.section != section {
                    // The section changed while this read ran: read it now
                    // instead of showing another section's snapshot.
                    card.refresh(cx);
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        card.snapshot = Some(snapshot);
                        card.error = None;
                    }
                    Err(error) => {
                        // A refused/replaced registration must not leave a card
                        // looking current under a different agent's live state.
                        card.snapshot = None;
                        card.error = Some(match error {
                            runtime_context::Error::RegistrationChanged => REPLACED,
                            runtime_context::Error::Unsupported => {
                                "This agent is no longer available."
                            }
                            _ => "The original agent's profile context is unavailable.",
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl gpui::EventEmitter<Opened> for Card {}
impl gpui::EventEmitter<RunView> for Card {}

impl Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let mut body = list("bound-agent-card").gap(px(t.space.s8));
        if let Some(message) = &self.action_notice {
            body = body.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s8))
                    .px(px(t.space.s14))
                    .child(
                        ui::text(div().flex_1().min_w_0().text_color(c.text_2), &t.text.row)
                            .child(message.clone()),
                    )
                    .child(ui::icon_button("bound-agent-notice-dismiss", "x").on_click(
                        cx.listener(|card, _, _, cx| {
                            card.action_notice = None;
                            cx.notify();
                        }),
                    )),
            );
        }
        if let Some(message) = self.removal_error {
            body = body.child(ui::text(div().text_color(c.text), &t.text.row).child(message));
        }
        if self.removal == Removal::Confirming {
            body = body
                .child(ui::group_note(format!(
                    "Remove {}? Its session stops and it leaves this profile. Your project folder and files stay. Backbone removes only the skill links it added.",
                    self.name
                )))
                .child(
                    ui::button("bound-agent-remove-confirm", "Remove agent", ui::Button::Danger)
                        .on_click(cx.listener(|card, _, _, cx| card.remove(cx))),
                )
                .child(
                    ui::button("bound-agent-remove-keep", "Keep agent", ui::Button::Secondary)
                        .on_click(cx.listener(|card, _, _, cx| {
                            card.removal = Removal::None;
                            cx.notify();
                        })),
                );
        }
        if self.removal == Removal::Done {
            let done = "This agent was removed from the profile. Your project folder and files stay. Backbone removed only the skill links it added.";
            body = body.child(ui::text(div().text_color(c.muted), &t.text.row).child(done));
        } else if let Some(message) = self.error {
            body = body.child(ui::text(div().text_color(c.muted), &t.text.row).child(message));
        } else if let Some(snapshot) = self.snapshot.as_ref().filter(|_| {
            !matches!(
                self.section,
                AgentSection::Overview | AgentSection::Permissions
            )
        }) {
            let section = match self.section {
                AgentSection::Connections => agent::connections(&snapshot.agent),
                AgentSection::Skills => {
                    agent::skills_section(&snapshot.agent, cx, Some(Self::open_row))
                }
                AgentSection::Policies => {
                    agent::policies_section(&snapshot.agent, cx, Some(Self::open_row))
                }
                _ => agent::usage_section(&snapshot.agent),
            };
            body = body.child(section);
        } else if let Some(snapshot) = &self.snapshot
            && self.section == AgentSection::Overview
        {
            // The card reads the agent itself, so its details are its state's.
            // Its runs come from the profile's roster, which the stream wakes.
            let runs = self.agents.runs_of(&self.registration);
            let local_id = self.connection.identity().installation_id.as_str();
            let (machines, machines_read) = self.agents.machines();
            let multi_machine = self.agents.is_multi_machine(&[])
                || snapshot
                    .agent
                    .machine
                    .as_ref()
                    .is_some_and(|machine| machine.id != local_id);
            let machine_label = multi_machine.then(|| match &snapshot.agent.machine {
                Some(machine) if machine.id == local_id => {
                    let name = self.agents.machine_display_name(machine);
                    format!("{name} · this machine")
                }
                Some(machine) => {
                    let name = self.agents.machine_display_name(machine);
                    let presence = crate::desktop::agent_groups::machine_presence(
                        &machine.id,
                        &machines,
                        machines_read,
                    );
                    format!("{name} · {}", presence.label())
                }
                None => "Never started".into(),
            });
            // A run's view is on the tmux server of its agent's machine.
            let here = snapshot
                .agent
                .machine
                .as_ref()
                .is_none_or(|machine| machine.id == local_id);
            body = body.child(agent::overview(
                &snapshot.agent,
                state(&snapshot.agent),
                &runs,
                machine_label,
                cx,
                here.then_some(Self::open_view),
            ));
        } else if self.snapshot.is_some() {
            // The service reports no permissions of an agent's runtime.
            body = body.child(
                ui::text(div().text_color(c.muted), &t.text.row)
                    .child("Runtime permission inspection is unsupported."),
            );
        } else {
            body = body
                .child(ui::text(div().text_color(c.muted), &t.text.row).child("Reading agent…"));
        }
        let mut nav = div()
            .id("bound-agent-sections")
            .flex()
            .flex_shrink_0()
            .items_end()
            .gap(px(t.space.s2))
            .h(px(t.size.view_toolbar))
            .px(px(t.space.s10))
            .overflow_x_scroll()
            .border_b(px(t.size.hairline))
            .border_color(c.line);
        for section in AgentSection::ALL {
            nav = nav.child(
                agent::tab(
                    format!("bound-agent-section-{}", section.as_str()).into(),
                    agent::label(section),
                    String::new(),
                    section == self.section,
                )
                .on_click(cx.listener(move |card, _, _, cx| card.set_section(section, cx))),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(
                ui::view_header("user", self.name.clone())
                    .children(
                        self.snapshot
                            .as_ref()
                            .map(|snapshot| ui::badge(snapshot.agent.runtime.clone())),
                    )
                    .children(self.snapshot.as_ref().map(|snapshot| {
                        ui::agent_status(&Status::from_stale(
                            state(&snapshot.agent),
                            snapshot.agent.state_source.as_deref(),
                            &snapshot.agent.evidence,
                        ))
                    }))
                    .when_some(
                        self.snapshot
                            .as_ref()
                            .filter(|_| self.removal == Removal::None),
                        |header, snapshot| {
                            let agent_state = state(&snapshot.agent);
                            if agent_state == AgentState::Offline {
                                header
                                    .child(
                                        ui::segment("bound-agent-resume", "Resume", false)
                                            .when(self.action_in_flight, |el| el.opacity(0.4))
                                            .on_click(cx.listener(|card, _, _, cx| {
                                                card.run_lifecycle(
                                                    super::TeamAgentAction::Start { resume: true },
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        ui::segment("bound-agent-start", "Start", false)
                                            .when(self.action_in_flight, |el| el.opacity(0.4))
                                            .on_click(cx.listener(|card, _, _, cx| {
                                                card.run_lifecycle(
                                                    super::TeamAgentAction::Start { resume: false },
                                                    cx,
                                                );
                                            })),
                                    )
                            } else {
                                header
                                    .child(
                                        ui::segment("bound-agent-restart", "Restart", false)
                                            .when(self.action_in_flight, |el| el.opacity(0.4))
                                            .on_click(cx.listener(|card, _, _, cx| {
                                                card.run_lifecycle(
                                                    super::TeamAgentAction::Restart {
                                                        resume: false,
                                                    },
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        ui::segment("bound-agent-stop", "Stop", false)
                                            .when(self.action_in_flight, |el| el.opacity(0.4))
                                            .on_click(cx.listener(|card, _, _, cx| {
                                                card.run_lifecycle(
                                                    super::TeamAgentAction::Stop,
                                                    cx,
                                                );
                                            })),
                                    )
                            }
                        },
                    )
                    .child(
                        ui::segment("bound-agent-refresh", "Refresh", false).on_click(cx.listener(
                            |card, _, _, cx| {
                                card.action_notice = None;
                                card.refresh(cx);
                                cx.notify();
                            },
                        )),
                    )
                    .when(self.removal == Removal::None, |header| {
                        header.child(ui::segment("bound-agent-remove", "Remove", false).on_click(
                            cx.listener(|card, _, _, cx| {
                                card.removal = Removal::Confirming;
                                card.removal_error = None;
                                cx.notify();
                            }),
                        ))
                    }),
            )
            .child(nav)
            .child(body)
    }
}

#[cfg(test)]
mod tests;
