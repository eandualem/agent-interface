//! Explicit fresh-profile choices; no lifecycle execution here.
use super::{Ticket, ui};
use crate::desktop::{
    design::{colors, tokens},
    name::NameField,
};
use agent_interface_roster::profiles::operations::Prepared;
use gpui::{Context, Entity, EventEmitter, FocusHandle, Subscription, Window, div, prelude::*, px};
use std::sync::{Arc, atomic::AtomicBool};

/// The only baseline the provider can create today, so nobody has to know it.
const BASELINE: &str = "core-v1";

pub(super) enum Event {
    Submit,
    Back,
}

pub(super) struct Form {
    name: Entity<NameField>,
    _field: Subscription,
    ticket: Ticket,
    error: Option<&'static str>,
    prepared: Option<Prepared>,
    finished: bool,
    pub focus: FocusHandle,
}
impl EventEmitter<Event> for Form {}
impl Form {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| NameField::filter(window, cx));
        let field = cx.observe(&name, |_, _, cx| cx.notify());
        let mut form = Self {
            name,
            _field: field,
            ticket: Ticket::default(),
            error: None,
            prepared: None,
            finished: false,
            focus: cx.focus_handle(),
        };
        form.ticket.begin();
        form
    }
    pub fn take_prepared(&mut self) -> Option<Prepared> {
        self.prepared.take()
    }
    fn valid(&self, cx: &Context<Self>) -> bool {
        if self.finished {
            return false;
        }
        let name = self.name.read(cx);
        !name.composing() && Prepared::create("validation", name.value(), BASELINE).is_ok()
    }
    fn submit(&mut self, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if !self.ticket.accepts(expected) || !self.valid(cx) {
            return;
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        match Prepared::create(&id, self.name.read(cx).value(), BASELINE) {
            Ok(prepared) => {
                // Emit once. Later field edits cannot rewrite these bytes.
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
            .gap(px(t.space.s8))
            .child(ui::group_note(
                "The new profile starts with separate mutable data and accounts.",
            ))
            .child(ui::group_title("Profile name", ""))
            .child(self.name.clone());
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
                let composing = form.name.read(cx).composing();
                if !composing && event.keystroke.key == "escape" {
                    form.back(cx);
                    cx.stop_propagation();
                }
            }))
            .child(ui::view_header("layers", "Create an isolated profile"))
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
                            ui::button("creation-submit", "Create profile", ui::Button::Primary)
                                .on_click(cx.listener(move |form, _, _, cx| {
                                    form.submit(&confirmation, cx)
                                })),
                        )
                    }),
            )
    }
}
