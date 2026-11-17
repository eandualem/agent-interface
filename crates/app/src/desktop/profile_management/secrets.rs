//! The secrets of a profile on the Rust service, such as `GITHUB_TOKEN`: their
//! names, whether each is set, and a masked field to add, replace or remove
//! one. A value is typed, sent once and wiped; it is never shown or kept.

use super::ui;
use crate::desktop::{
    design::{colors, tokens},
    name::{NameEvent, NameField},
};
use agent_interface_roster::profiles::{
    AddError, Connection, Error, Profile,
    secrets::{self as service, Change, Secret},
};
use field::{Event, Masked, wipe};
use gpui::{Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};

mod field;

/// GitHub's token, which agents use for GitHub. Its row is always first,
/// set or not.
const GITHUB: &str = "GITHUB_TOKEN";
/// Where a person makes a GitHub token.
const GITHUB_TOKENS: &str = "https://github.com/settings/tokens";

/// How wide a new secret's name field is.
const NAME_WIDTH: f32 = 200.;

enum List {
    Loading,
    /// Why there is no list, to show.
    Unavailable(&'static str),
    Shown(Vec<Secret>),
}

/// A value being typed, for the secret `name`, or for a new secret whose
/// name is typed in `naming`.
struct Edit {
    name: Option<String>,
    naming: Option<Entity<NameField>>,
    value: Entity<Masked>,
    _events: Vec<Subscription>,
}

pub(super) struct Section {
    connection: Option<Connection>,
    list: List,
    edit: Option<Edit>,
    /// A secret whose removal waits to be confirmed.
    removing: Option<String>,
    busy: bool,
    /// What the last change came to, or the service's reason for refusing it.
    note: Option<String>,
    /// The last change was made, and waits for the service to start again.
    applied: bool,
}

impl Section {
    /// Connect to the profile and read its secrets on an I/O worker.
    pub(super) fn new(profile: Profile, cx: &mut Context<Self>) -> Self {
        let work = cx.background_executor().spawn(async move {
            let connection = profile.connect().ok()?;
            let list = connection.secrets();
            Some((connection, list))
        });
        cx.spawn(async move |this, cx| {
            let read = work.await;
            let _ = this.update(cx, |section, cx| {
                match read {
                    Some((connection, list)) => {
                        section.list = listed(list);
                        section.connection = Some(connection);
                    }
                    None => {
                        section.list = List::Unavailable("Start the profile to see its secrets.")
                    }
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            connection: None,
            list: List::Loading,
            edit: None,
            removing: None,
            busy: false,
            note: None,
            applied: false,
        }
    }

    /// Type a value for `name`, or for a new secret when there is none.
    fn edit(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let naming = name
            .is_none()
            .then(|| cx.new(|cx| NameField::bare("", window, cx)));
        let value = cx.new(|cx| Masked::new(window, cx));
        let mut events =
            vec![cx.subscribe_in(
                &value,
                window,
                |section, _, event, window, cx| match event {
                    Event::Save => section.save(window, cx),
                    Event::Cancel => section.close(cx),
                },
            )];
        if let Some(naming) = &naming {
            window.focus(&naming.read(cx).focus.clone(), cx);
            events.push(cx.subscribe_in(
                naming,
                window,
                |section, _, event, window, cx| match event {
                    NameEvent::Save => section.save(window, cx),
                    NameEvent::Cancel => section.close(cx),
                    NameEvent::Changed(_) => {}
                },
            ));
        }
        self.edit = Some(Edit {
            name,
            naming,
            value,
            _events: events,
        });
        self.removing = None;
        self.note = None;
        self.applied = false;
        cx.notify();
    }

    /// Close the field, wiping what was typed in it.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.edit = None;
        cx.notify();
    }

    /// Send the typed value. The field closes as it goes, so nothing of the
    /// value stays here while the service answers.
    fn save(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else {
            return;
        };
        let name = match (&edit.name, &edit.naming) {
            (Some(name), _) => name.clone(),
            (None, Some(naming)) => naming.read(cx).value().trim().to_owned(),
            (None, None) => return,
        };
        if !service::is_name(&name) {
            self.note = Some(
                "A name is capital letters, digits and _, starting with a letter, such as GITHUB_TOKEN."
                    .into(),
            );
            cx.notify();
            return;
        }
        if !edit.value.read(cx).takes() {
            self.note = Some("A value is one line of text.".into());
            cx.notify();
            return;
        }
        let value = edit.value.update(cx, |field, _| field.take());
        self.edit = None;
        self.send(name, Some(value), cx);
    }

    /// Set `name` to `value`, or remove it when there is no value, then read
    /// the list again.
    fn send(&mut self, name: String, value: Option<String>, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            if let Some(value) = value {
                wipe(value);
            }
            return;
        };
        self.busy = true;
        self.removing = None;
        self.note = None;
        self.applied = false;
        let request = uuid::Uuid::now_v7().simple().to_string();
        let work = cx.background_executor().spawn(async move {
            let changed = match value {
                Some(value) => {
                    let changed = connection.set_secret(&name, &value, &request);
                    wipe(value);
                    changed
                }
                None => connection.remove_secret(&name, &request),
            };
            (changed, connection.secrets())
        });
        cx.spawn(async move |this, cx| {
            let (changed, list) = work.await;
            let _ = this.update(cx, |section, cx| {
                section.busy = false;
                section.applied = changed.is_ok();
                section.note = Some(outcome(changed));
                section.list = listed(list);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// The secrets in the order they're shown: GitHub's token first, set or
/// not, then the rest as the service lists them.
fn ordered(secrets: &[Secret]) -> Vec<Secret> {
    let github = secrets.iter().find(|secret| secret.name == GITHUB);
    let mut shown = vec![Secret {
        name: GITHUB.into(),
        set: github.is_some_and(|secret| secret.set),
    }];
    shown.extend(
        secrets
            .iter()
            .filter(|secret| secret.name != GITHUB)
            .cloned(),
    );
    shown
}

fn listed(list: Result<Vec<Secret>, AddError>) -> List {
    match list {
        Ok(secrets) => List::Shown(secrets),
        Err(_) => List::Unavailable("Couldn't read the profile's secrets."),
    }
}

/// What a change came to, or the service's own reason for refusing it.
fn outcome(changed: Result<Change, AddError>) -> String {
    match changed {
        Ok(Change { name, set, was_set }) => match (set, was_set) {
            (true, true) => format!("{name} was replaced."),
            (true, false) => format!("{name} was added."),
            (false, true) => format!("{name} was removed."),
            (false, false) => format!("{name} had no value."),
        },
        Err(AddError::Refused(message)) => message,
        Err(AddError::Connection(Error::Refused(status))) => {
            format!("The service refused it ({status}).")
        }
        Err(AddError::Connection(Error::Invalid)) => {
            "That value is too long, or the service's answer couldn't be read.".into()
        }
        Err(AddError::Connection(_)) => "Couldn't reach the profile's service.".into(),
    }
}

/// What a change needs to take effect on macOS: the paragraph, and the
/// commands it names, each shown with Copy.
const MACOS_RESTART: (&str, &[&str]) = (
    "The change takes effect when the service next starts. If Backbone runs as an installed login service, run \"backbone service restart\": running agents keep running. If you started it with \"backbone up --detach\", run \"backbone down\", then \"backbone up --detach\": running agents keep running.",
    &[
        "backbone service restart",
        "backbone down",
        "backbone up --detach",
    ],
);
/// The same on every other system, where only a stop and a start apply.
const OTHER_RESTART: (&str, &[&str]) = (
    "The change takes effect when the service next starts. If you started Backbone with \"backbone up --detach\", run \"backbone down\", then \"backbone up --detach\": running agents keep running. If Backbone runs as an installed login service, those commands do not apply, and restarting it may stop the running agents on this machine.",
    &["backbone down", "backbone up --detach"],
);
/// The notice this build shows. Both are compiled in, so tests read both.
const RESTART: (&str, &[&str]) = if cfg!(target_os = "macos") {
    MACOS_RESTART
} else {
    OTHER_RESTART
};

/// A secret takes effect when the service next starts. The app runs nothing:
/// it shows the commands, each with Copy.
fn restart_notice() -> gpui::Div {
    let t = tokens();
    let (paragraph, commands) = RESTART;
    let mut notice = div()
        .flex()
        .flex_col()
        .gap(px(t.space.s4))
        .child(ui::group_note(paragraph));
    for (index, command) in commands.iter().copied().enumerate() {
        notice = notice.child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .child(ui::mono(command))
                .child(
                    ui::text_action(
                        SharedString::from(format!("secret-restart-{index}")),
                        "Copy",
                    )
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(command.into()))
                    }),
                ),
        );
    }
    notice
}

impl Render for Section {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let meta =
            |text: SharedString| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .child(ui::group_title("Secrets", "Values are never shown."));
        let rows = match &self.list {
            List::Loading => return body.child(ui::group_note("Reading secrets…")),
            List::Unavailable(why) => return body.child(ui::group_note(*why)),
            List::Shown(rows) => rows,
        };
        let editing = self.edit.as_ref().map(|edit| edit.name.clone());
        let value_row = |edit: &Edit, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .px(px(t.space.s14))
                .py(px(t.space.s8))
                .children(edit.naming.as_ref().map(|naming| {
                    div().w(px(NAME_WIDTH)).child(
                        ui::setup_field(naming.read(cx).focus.is_focused(window))
                            .child(naming.clone()),
                    )
                }))
                .child(div().flex_1().min_w_0().child(edit.value.clone()))
                .child(
                    ui::button("secret-save", "Save", ui::Button::Primary)
                        .on_click(cx.listener(|section, _, window, cx| section.save(window, cx))),
                )
                .child(
                    ui::button("secret-cancel", "Cancel", ui::Button::Secondary)
                        .on_click(cx.listener(|section, _, _, cx| section.close(cx))),
                )
        };
        let mut card = ui::group_card();
        for (index, secret) in ordered(rows).iter().enumerate() {
            let name = secret.name.clone();
            let confirming = self.removing.as_deref() == Some(name.as_str());
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(t.space.s10))
                .h(px(t.size.group_row))
                .px(px(t.space.s14))
                .child(if name == GITHUB {
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .child(div().text_color(c.text).child("GitHub"))
                        .child(meta(GITHUB.into()).font_family(t.font.mono.clone()))
                        .child(
                            ui::text_action("secret-github-tokens", "Make a token")
                                .on_click(|_, _, cx| cx.open_url(GITHUB_TOKENS)),
                        )
                } else {
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(t.font.mono.clone())
                        .text_color(c.text)
                        .child(name.clone())
                });
            if confirming {
                let removed = name.clone();
                row =
                    row.child(meta(format!("Remove {name}?").into()))
                        .child(
                            ui::button("secret-remove-confirm", "Remove", ui::Button::Danger)
                                .on_click(cx.listener(move |section, _, _, cx| {
                                    section.send(removed.clone(), None, cx)
                                })),
                        )
                        .child(
                            ui::button("secret-remove-cancel", "Cancel", ui::Button::Secondary)
                                .on_click(cx.listener(|section, _, _, cx| {
                                    section.removing = None;
                                    cx.notify();
                                })),
                        );
            } else {
                let edited = name.clone();
                row = row
                    .child(meta(if secret.set { "Set" } else { "Not set" }.into()))
                    .when(!self.busy, |row| {
                        row.child(
                            ui::button(
                                SharedString::from(format!("secret-set-{name}")),
                                if secret.set { "Replace" } else { "Set" },
                                ui::Button::Secondary,
                            )
                            .on_click(cx.listener(
                                move |section, _, window, cx| {
                                    section.edit(Some(edited.clone()), window, cx)
                                },
                            )),
                        )
                    })
                    .when(!self.busy && secret.set, |row| {
                        let removing = name.clone();
                        row.child(
                            ui::button(
                                SharedString::from(format!("secret-remove-{name}")),
                                "Remove",
                                ui::Button::Danger,
                            )
                            .on_click(cx.listener(
                                move |section, _, _, cx| {
                                    section.edit = None;
                                    section.removing = Some(removing.clone());
                                    cx.notify();
                                },
                            )),
                        )
                    });
            }
            card = card.child(
                div()
                    .when(index > 0, |el| {
                        el.border_t(px(t.size.hairline)).border_color(c.line)
                    })
                    .child(row)
                    .when(editing.as_ref() == Some(&Some(name.clone())), |el| {
                        el.children(self.edit.as_ref().map(|edit| value_row(edit, cx)))
                    }),
            );
        }
        if editing == Some(None) {
            card = card.child(
                div()
                    .border_t(px(t.size.hairline))
                    .border_color(c.line)
                    .children(self.edit.as_ref().map(|edit| value_row(edit, cx))),
            );
        }
        body = body.child(card);
        if let Some(note) = &self.note {
            body = body.child(ui::group_note(note.clone()));
        }
        if self.applied {
            body = body.child(restart_notice());
        }
        if self.busy {
            body = body.child(ui::group_note("Sending…"));
        } else if editing.is_none() {
            body =
                body.child(div().flex().child(
                    ui::button("secret-new", "Add secret", ui::Button::Secondary).on_click(
                        cx.listener(|section, _, window, cx| section.edit(None, window, cx)),
                    ),
                ));
        }
        body
    }
}

#[cfg(test)]
mod tests;
