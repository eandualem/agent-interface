//! A native card whose saved registration and connection never change.

use super::{agent, list, ui};
use crate::desktop::design::{colors, tokens};
use agent_interface_core::{AgentSection, View};
use agent_interface_roster::{
    profiles::{
        Connection, Error as ProfileError,
        runtime_context::{self, Permissions, Target},
    },
    views::{Agent, Part, Range, content},
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
    permissions: Permissions,
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
        // A provider without permission inspection still shows the rest of the
        // card; the Permissions section says it is unsupported.
        let permissions = match self
            .target
            .as_ref()
            .expect("inspected target")
            .permissions()
        {
            Err(runtime_context::Error::Unsupported) => Permissions::unsupported(),
            result => result?,
        };
        let part = match section {
            AgentSection::Skills => Part::Skills,
            AgentSection::Policies => Part::Policies,
            AgentSection::Usage => Part::Usage(Range::Day),
            _ => Part::Card,
        };
        // A section's own read failing leaves the card; the section says why.
        let _ = content::agent_part(&self.connection, &part, &mut agent, &|| true);
        Ok(Snapshot { agent, permissions })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Removal {
    None,
    Confirming,
    Running,
    Done,
}

fn removal_refusal(error: ProfileError) -> &'static str {
    match error {
        ProfileError::RegistrationChanged => {
            "This agent registration changed. Reopen it from Team."
        }
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
        Err(runtime_context::Error::RegistrationChanged) => {
            Err("This agent registration changed. Reopen it from Team.")
        }
        Err(_) => Err("Couldn't confirm this is the same agent. Try again."),
    }
}

pub(super) struct Card {
    connection: Connection,
    /// The registration this card shows; removal checks it first.
    registration: String,
    removal: Removal,
    removal_error: Option<&'static str>,
    reader: Option<Reader>,
    snapshot: Option<Snapshot>,
    section: AgentSection,
    name: String,
    error: Option<&'static str>,
    visible: Rc<Cell<bool>>,
    _poll: Task<()>,
}

impl Card {
    pub fn new(
        connection: Connection,
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
                if this.update(cx, |card, cx| card.refresh(cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        });
        Self {
            connection: connection.clone(),
            registration: registration.clone(),
            removal: Removal::None,
            removal_error: None,
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
            _poll: poll,
        }
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
                            runtime_context::Error::RegistrationChanged => {
                                "This agent registration changed. Reopen it from Team."
                            }
                            runtime_context::Error::Unsupported => {
                                "This provider cannot supply the agent's profile context."
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

impl Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let mut body = list("bound-agent-card").gap(px(t.space.s8));
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
                AgentSection::Skills => agent::skills_section::<Self>(&snapshot.agent, cx, None),
                AgentSection::Policies => {
                    agent::policies_section::<Self>(&snapshot.agent, cx, None)
                }
                _ => agent::usage_section(&snapshot.agent),
            };
            body = body.child(section);
        } else if let Some(snapshot) = &self.snapshot
            && self.section == AgentSection::Overview
        {
            for (label, value) in [
                ("Runtime", snapshot.agent.runtime.as_str()),
                ("Repository", snapshot.agent.repo.as_str()),
                ("State", snapshot.agent.state.as_str()),
                ("Project", snapshot.agent.dir.as_str()),
            ] {
                body = body.child(
                    ui::text(div().text_color(c.text), &t.text.row)
                        .child(format!("{label}: {value}")),
                );
            }
        } else if let Some(snapshot) = &self.snapshot {
            let permissions = &snapshot.permissions;
            if let Some(message) = permissions.message() {
                body = body.child(ui::text(div().text_color(c.muted), &t.text.row).child(message));
            } else {
                body = body.child(ui::text(div().text_color(c.muted), &t.text.meta).child(
                    "Settings reported by this agent's runtime; launch overrides may differ.",
                ));
                for mode in &permissions.modes {
                    body = body.child(
                        ui::text(div().text_color(c.text), &t.text.row)
                            .child(format!("{}: {}", mode.name, mode.value)),
                    );
                }
                for (label, rules) in [
                    ("Allow", &permissions.allow),
                    ("Deny", &permissions.deny),
                    ("Ask", &permissions.ask),
                ] {
                    for rule in rules {
                        body = body.child(
                            ui::text(div().text_color(c.text), &t.text.row)
                                .child(format!("{label}: {rule}")),
                        );
                    }
                }
                if permissions.modes.is_empty()
                    && permissions.allow.is_empty()
                    && permissions.deny.is_empty()
                    && permissions.ask.is_empty()
                {
                    body = body.child(
                        ui::text(div().text_color(c.muted), &t.text.row)
                            .child("No explicit approval settings reported."),
                    );
                }
            }
            for file in &permissions.files {
                let scope = match file.scope {
                    runtime_context::Scope::Runtime => "Runtime settings",
                    runtime_context::Scope::Project => "Project settings",
                    runtime_context::Scope::ProjectLocal => "Local project settings",
                };
                let state = match file.state {
                    runtime_context::FileState::Read => "read",
                    runtime_context::FileState::Missing => "not present",
                    runtime_context::FileState::Invalid => "invalid",
                    runtime_context::FileState::Unreadable => "unreadable",
                };
                body = body.child(
                    ui::text(div().text_color(c.muted), &t.text.meta)
                        .child(format!("{scope}: {} ({state})", file.display_path)),
                );
            }
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
                    .child(
                        ui::segment("bound-agent-refresh", "Refresh", false)
                            .on_click(cx.listener(|card, _, _, cx| card.refresh(cx))),
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
