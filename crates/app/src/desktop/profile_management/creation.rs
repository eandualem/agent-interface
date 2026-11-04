//! Explicit fresh-profile and adoption choices; no lifecycle execution here.
use super::{Ticket, ui};
use crate::{
    desktop::{
        design::{colors, tokens},
        name::NameField,
    },
    profiles::{Client, Error},
};
use agent_interface_roster::profiles::operations::{AdoptionPreview, Prepared};
use gpui::{Context, Entity, EventEmitter, FocusHandle, Subscription, Window, div, prelude::*, px};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Create,
    Adopt,
}
/// The only baseline the provider can create today, so nobody has to know it.
const BASELINE: &str = "core-v1";

pub(super) enum Event {
    Submit,
    Back,
}

pub(super) struct Form {
    mode: Mode,
    client: Client,
    name: Option<Entity<NameField>>,
    _fields: Vec<Subscription>,
    preview: Option<AdoptionPreview>,
    ticket: Ticket,
    busy: bool,
    error: Option<&'static str>,
    prepared: Option<Prepared>,
    finished: bool,
    pub focus: FocusHandle,
}
impl EventEmitter<Event> for Form {}
impl Form {
    pub fn new(mode: Mode, client: Client, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = (mode == Mode::Create).then(|| cx.new(|cx| NameField::filter(window, cx)));
        let fields = name
            .iter()
            .map(|field| cx.observe(field, |_, _, cx| cx.notify()))
            .collect();
        let mut form = Self {
            mode,
            client,
            name,
            _fields: fields,
            preview: None,
            ticket: Ticket::default(),
            busy: false,
            error: None,
            prepared: None,
            finished: false,
            focus: cx.focus_handle(),
        };
        form.ticket.begin();
        if mode == Mode::Adopt {
            form.refresh(cx);
        }
        form
    }
    pub fn take_prepared(&mut self) -> Option<Prepared> {
        self.prepared.take()
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.finished || self.busy || self.mode != Mode::Adopt {
            return;
        }
        self.preview = None;
        self.error = None;
        self.busy = true;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let client = self.client.clone();
        let work = cx
            .background_executor()
            .spawn(async move { client.adoption_preview(&|| worker.load(Ordering::Acquire)) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |form, cx| {
                if !form.ticket.accepts(&ticket) || form.finished {
                    return;
                }
                form.busy = false;
                match result {
                    Ok(preview) => form.preview = Some(preview),
                    Err(error) => {
                        form.error = Some(match error {
                            Error::ContextChanged => "The original operator context changed.",
                            _ => "Adoption preflight is unavailable. No adoption was submitted.",
                        })
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn valid(&self, cx: &Context<Self>) -> bool {
        if self.busy || self.finished {
            return false;
        }
        match self.mode {
            Mode::Create => self.name.as_ref().is_some_and(|name| {
                let name = name.read(cx);
                !name.composing() && Prepared::create("validation", name.value(), BASELINE).is_ok()
            }),
            Mode::Adopt => self.preview.is_some(),
        }
    }
    fn submit(&mut self, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if !self.ticket.accepts(expected) || !self.valid(cx) {
            return;
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let prepared = match self.mode {
            Mode::Create => {
                Prepared::create(&id, self.name.as_ref().unwrap().read(cx).value(), BASELINE)
            }
            Mode::Adopt => Prepared::adopt_default(&id, self.preview.as_ref().unwrap()),
        };
        match prepared {
            Ok(prepared) => {
                // Emit once. Later field edits/previews cannot rewrite these bytes.
                self.finished = true;
                self.ticket.cancel();
                self.prepared = Some(prepared);
                cx.emit(Event::Submit);
            }
            Err(_) => self.error = Some("Check the profile name."),
        }
        cx.notify();
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ticket.cancel();
        cx.emit(Event::Back);
    }
}
impl Render for Form {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let confirmation = self.ticket.0.clone();
        let mut body = div()
            .id("profile-creation-fields")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(t.space.s12))
            .flex()
            .flex_col()
            .gap(px(t.space.s8));
        let title = match self.mode {
            Mode::Create => "Create an isolated profile",
            Mode::Adopt => "Adopt Default in place",
        };
        match self.mode {
            Mode::Create => {
                body = body
                    .child(ui::group_note(
                        "The new profile starts with separate mutable data and accounts.",
                    ))
                    .child(ui::group_title("Profile name", ""))
                    .child(self.name.as_ref().unwrap().clone());
            }
            Mode::Adopt => {
                body = body.child(ui::group_note("Adoption registers the existing Default installation in place. It preserves its data, accounts and service mapping; it does not copy them into a new profile."));
                if self.busy {
                    body = body.child(ui::group_note("Reading adoption preflight…"));
                }
                if let Some(preview) = &self.preview {
                    for (label, value) in [
                        ("Installation", preview.installation_id().to_owned()),
                        ("Baseline", preview.baseline_version().to_owned()),
                        (
                            "Existing data",
                            preview.data_directory().display().to_string(),
                        ),
                        (
                            "Existing tmux socket",
                            preview.tmux_socket().display().to_string(),
                        ),
                    ] {
                        body = body.child(format!("{label}: {value}"));
                    }
                }
                if !self.busy && !self.finished {
                    body = body.child(
                        ui::button(
                            "adoption-refresh",
                            "Refresh preflight",
                            ui::Button::Secondary,
                        )
                        .on_click(cx.listener(|form, _, _, cx| form.refresh(cx))),
                    );
                }
            }
        }
        if let Some(error) = self.error {
            body = body.child(ui::group_note(error));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|form, event: &gpui::KeyDownEvent, _, cx| {
                let composing = form
                    .name
                    .as_ref()
                    .is_some_and(|field| field.read(cx).composing());
                if !composing && event.keystroke.key == "escape" {
                    form.back(cx);
                    cx.stop_propagation();
                }
            }))
            .child(ui::view_header("layers", title))
            .child(body)
            .child(
                ui::view_toolbar()
                    .px(px(t.space.s12))
                    .gap(px(t.space.s8))
                    .child(
                        ui::button("creation-back", "Back", ui::Button::Secondary)
                            .on_click(cx.listener(|form, _, _, cx| form.back(cx))),
                    )
                    .when(self.valid(cx), |bar| {
                        bar.child(
                            ui::button(
                                "creation-submit",
                                if self.mode == Mode::Create {
                                    "Create profile"
                                } else {
                                    "Adopt this installation"
                                },
                                ui::Button::Primary,
                            )
                            .on_click(
                                cx.listener(move |form, _, _, cx| form.submit(&confirmation, cx)),
                            ),
                        )
                    }),
            )
    }
}
