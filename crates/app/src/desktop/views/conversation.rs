//! The conversation beside an authored record. Every reader shows this one
//! component: a rail while closed, a thread and a composer while open.
//!
//! Agent Backbone does not yet keep conversation history, so the thread holds
//! the messages sent from this pane. Each is sent once through
//! `roster::feedback` with a reference to the record.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use agent_interface_roster::{
    feedback::{self, Outcome},
    views::{ReportContent, ReportItem},
};
use gpui::{
    AnyElement, Context, Entity, FocusHandle, SharedString, Subscription, Window, div, prelude::*,
    px,
};

use super::{clock::Local, reports};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
    name::{NameEvent, NameField},
};

/// What became of one message.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Delivery {
    Sending,
    Sent,
    /// Nothing was accepted; sending again is safe.
    NotSent(String),
    /// It may have arrived; sending again could repeat it.
    Unconfirmed(String),
}

struct Message {
    text: String,
    at: f64,
    delivery: Delivery,
}

pub(super) struct Conversation {
    item: ReportItem,
    directory: PathBuf,
    field: Entity<NameField>,
    /// The document surface, which takes focus back when the panel closes.
    surface: Rc<RefCell<super::artifact_web::Surface>>,
    open: bool,
    /// A key asked to open or close the panel; done when it next renders.
    toggle_requested: bool,
    messages: Vec<Message>,
    return_focus: Option<FocusHandle>,
    _input: Subscription,
}

impl Conversation {
    pub fn new(
        item: ReportItem,
        directory: PathBuf,
        surface: Rc<RefCell<super::artifact_web::Surface>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| NameField::feedback(window, cx));
        let input = cx.subscribe_in(
            &field,
            window,
            |conversation, _, event, window, cx| match event {
                NameEvent::Save => conversation.send(cx),
                NameEvent::Cancel => conversation.close(window, cx),
                NameEvent::Changed(_) => cx.notify(),
            },
        );
        Self {
            item,
            directory,
            field,
            surface,
            open: false,
            toggle_requested: false,
            messages: Vec::new(),
            return_focus: None,
            _input: input,
        }
    }

    /// Open or close the panel from a key, which has no window to focus with.
    pub fn request_toggle(&mut self, cx: &mut Context<Self>) {
        self.toggle_requested = true;
        cx.notify();
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
        } else {
            self.return_focus = window.focused(cx);
            self.open = true;
            let focus = self.field.read(cx).focus.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        match self.return_focus.take() {
            Some(focus) => window.focus(&focus, cx),
            None => window.blur(cx),
        }
        self.surface.borrow().focus();
        cx.notify();
    }

    fn can_send(&self, cx: &gpui::App) -> bool {
        let field = self.field.read(cx);
        !field.composing() && !field.value().trim().is_empty()
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if !self.can_send(cx) {
            return;
        }
        let text = self.field.read(cx).value().to_owned();
        self.field.update(cx, |field, cx| field.clear(cx));
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |now| now.as_secs_f64());
        self.messages.push(Message {
            text,
            at,
            delivery: Delivery::Sending,
        });
        self.deliver(self.messages.len() - 1, cx);
    }

    /// Send message `index` once, on an I/O worker.
    fn deliver(&mut self, index: usize, cx: &mut Context<Self>) {
        let message = &mut self.messages[index];
        message.delivery = Delivery::Sending;
        let (directory, item, text) = (
            self.directory.clone(),
            self.item.clone(),
            message.text.clone(),
        );
        let worker = cx.background_executor().spawn(async move {
            let started = std::time::Instant::now();
            let outcome = feedback::send(&directory, &item, &text);
            let result = match &outcome {
                Outcome::Accepted(_) => "accepted",
                Outcome::Refused(_) => "refused",
                Outcome::Unknown(_) => "unknown",
            };
            tracing::debug!(
                event = "feedback_finished",
                result,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Feedback submission finished"
            );
            outcome
        });
        cx.spawn(async move |this, cx| {
            let outcome = worker.await;
            let _ = this.update(cx, |conversation, cx| {
                conversation.messages[index].delivery = match outcome {
                    Outcome::Accepted(_) => Delivery::Sent,
                    Outcome::Refused(reason) => Delivery::NotSent(reason),
                    Outcome::Unknown(reason) => Delivery::Unconfirmed(reason),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// What the conversation is about, as its header and composer name it.
fn subject(item: &ReportItem) -> (&'static str, &'static str) {
    match item.content {
        ReportContent::Progress { .. } => ("about this report", "report"),
        ReportContent::Response { .. } => ("about this answer", "answer"),
        ReportContent::Document(_) => ("about this document", "document"),
    }
}

impl Conversation {
    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        ui::conversation_rail(
            div().id("conversation-open").on_click(
                cx.listener(|conversation, _, window, cx| conversation.toggle(window, cx)),
            ),
            reports::mark(&self.item),
            false,
        )
        .into_any_element()
    }

    fn thread(&self, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let (_, kind) = subject(&self.item);
        if self.messages.is_empty() {
            return div()
                .flex_1()
                .min_h_0()
                .child(ui::empty_state(
                    ui::empty_tile("message-square", c.selected, c.text_2),
                    format!("Talk to {} here", self.item.author),
                    format!(
                        "Messages you send here go to {} with a reference to this {kind}.",
                        self.item.author
                    ),
                ))
                .into_any_element();
        }
        let now = Local::now();
        let mut thread = div()
            .id("conversation-thread")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(t.space.s12))
            .p(px(t.space.s14));
        let mut day: Option<Local> = None;
        for (index, message) in self.messages.iter().enumerate() {
            let local = Local::at(message.at);
            if let (Some(local), Some(now)) = (local, now)
                && day.is_none_or(|day| !day.same_day(&local))
            {
                thread = thread.child(ui::thread_divider(local.day_name(&now), false));
                day = Some(local);
            }
            let time = local.map(|local| local.time()).unwrap_or_default();
            let meta = ui::message_meta();
            let meta = match &message.delivery {
                Delivery::Sending => meta.child(ui::sending_spinner()).child("Sending…"),
                Delivery::Sent => meta.child(format!("You · {time}")),
                Delivery::NotSent(reason) => meta
                    .text_color(c.attention_text)
                    .child(SharedString::from(format!("Not sent: {reason}")))
                    .child(ui::link(
                        div()
                            .id(SharedString::from(format!("conversation-retry-{index}")))
                            .text_color(c.attention_text)
                            .underline()
                            .child("Retry")
                            .on_click(cx.listener(move |conversation, _, _, cx| {
                                conversation.deliver(index, cx)
                            })),
                    )),
                Delivery::Unconfirmed(reason) => meta.child(SharedString::from(format!(
                    "Not confirmed: {reason} Ask {} before sending it again.",
                    self.item.author
                ))),
            };
            thread = thread.child(ui::message(
                true,
                matches!(message.delivery, Delivery::NotSent(_)),
                meta,
                message.text.clone(),
            ));
        }
        thread.into_any_element()
    }
}

impl Render for Conversation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.toggle_requested) {
            self.toggle(window, cx);
        }
        if !self.open {
            return self.rail(cx);
        }
        let t = tokens();
        let (about, _) = subject(&self.item);
        let can_send = self.can_send(cx);
        ui::conversation_panel()
            .child(ui::conversation_header(
                reports::mark(&self.item),
                self.item.author.clone(),
                about,
                ui::icon_button("conversation-close", "panel-right-close").on_click(
                    cx.listener(|conversation, _, window, cx| conversation.close(window, cx)),
                ),
            ))
            .child(self.thread(cx))
            .child(ui::composer(
                div().text_size(t.text.row.size()).child(self.field.clone()),
                Some(ui::anchor(format!("#{}", self.item.id))),
                ui::send_button("conversation-send", can_send)
                    .on_click(cx.listener(|conversation, _, _, cx| conversation.send(cx))),
            ))
            .into_any_element()
    }
}
