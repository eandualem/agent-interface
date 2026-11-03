//! A native card whose saved registration and connection never change.

use super::{list, ui};
use crate::desktop::design::{colors, tokens};
use agent_interface_core::View;
use agent_interface_roster::{
    profiles::{
        Connection,
        runtime_context::{self, Permissions, Target},
    },
    views::Agent,
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
    fn read(&mut self) -> Result<Snapshot, runtime_context::Error> {
        let agent = if let Some(target) = &self.target {
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
        let permissions = self
            .target
            .as_ref()
            .expect("inspected target")
            .permissions()?;
        Ok(Snapshot { agent, permissions })
    }
}

pub(super) struct Card {
    reader: Option<Reader>,
    snapshot: Option<Snapshot>,
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
            reader: Some(Reader {
                connection,
                name: name.clone(),
                registration,
                target: None,
            }),
            snapshot: None,
            name,
            error: None,
            visible,
            _poll: poll,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.visible.get() {
            return;
        }
        let Some(mut reader) = self.reader.take() else {
            return;
        };
        let work = cx.background_executor().spawn(async move {
            let result = reader.read();
            (reader, result)
        });
        cx.spawn(async move |this, cx| {
            let (reader, result) = work.await;
            let _ = this.update(cx, |card, cx| {
                card.reader = Some(reader);
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
        if let Some(message) = self.error {
            body = body.child(ui::text(div().text_color(c.muted), &t.text.row).child(message));
        } else if let Some(snapshot) = &self.snapshot {
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
            let permissions = &snapshot.permissions;
            body = body.child(ui::group_title("Permissions", ""));
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(
                ui::view_header("user", self.name.clone()).child(
                    ui::segment("bound-agent-refresh", "Refresh", false)
                        .on_click(cx.listener(|card, _, _, cx| card.refresh(cx))),
                ),
            )
            .child(body)
    }
}

#[cfg(test)]
mod tests;
