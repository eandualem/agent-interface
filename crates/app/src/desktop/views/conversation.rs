//! The conversation beside an authored record. Every reader shows this one
//! component: a rail while closed, a thread and a composer while open.
//!
//! Agent Backbone does not yet keep conversation history, so the thread holds
//! the messages sent from this pane. A record's conversation sends each once
//! through `roster::feedback` with a reference to the record; a view with its
//! own delivery, such as Sheets, receives [`ConversationEvent`]s and reports
//! each outcome back with [`Conversation::complete`].

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use agent_interface_roster::{
    feedback::{self, Outcome},
    views::{ReportContent, ReportItem},
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, SharedString, Subscription,
    Window, div, prelude::*, px,
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
    /// Not sent, and moved back into the composer to edit.
    Moved,
}

struct Message {
    text: String,
    /// What it referred to when it was written.
    anchor: Option<String>,
    at: f64,
    delivery: Delivery,
}

/// What a view with its own delivery is asked to do with message `index`.
pub(super) enum ConversationEvent {
    /// Send it for the first time.
    Send(usize),
    /// Send the same request again.
    Retry(usize),
}

impl EventEmitter<ConversationEvent> for Conversation {}

pub(super) struct Conversation {
    /// The record messages are about and where to send them; `None` when the
    /// owning view delivers them itself.
    record: Option<(ReportItem, PathBuf)>,
    agent: String,
    about: &'static str,
    anchor: Option<String>,
    /// Input is held while the owning view cannot accept a message.
    blocked: bool,
    field: Entity<NameField>,
    /// The document surface, which takes focus back when the panel closes.
    surface: Option<Rc<RefCell<super::artifact_web::Surface>>>,
    open: bool,
    /// A key asked to open or close the panel; done when it next renders.
    toggle_requested: bool,
    messages: Vec<Message>,
    return_focus: Option<FocusHandle>,
    _input: Subscription,
}

impl Conversation {
    /// The conversation about an authored record, sent to its author.
    pub fn new(
        item: ReportItem,
        directory: PathBuf,
        surface: Rc<RefCell<super::artifact_web::Surface>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut conversation = Self::create(subject(&item), window, cx);
        conversation.agent = item.author.clone();
        conversation.anchor = Some(format!("#{}", item.id));
        conversation.record = Some((item, directory));
        conversation.surface = Some(surface);
        conversation
    }

    /// A conversation whose owning view delivers each message.
    pub fn delivered_by_owner(
        about: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(about, window, cx)
    }

    fn create(about: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            record: None,
            agent: String::new(),
            about,
            anchor: None,
            blocked: false,
            field,
            surface: None,
            open: false,
            toggle_requested: false,
            messages: Vec::new(),
            return_focus: None,
            _input: input,
        }
    }

    /// Who receives messages, what they refer to, and whether input is held.
    pub fn set_context(
        &mut self,
        agent: String,
        anchor: Option<String>,
        blocked: bool,
        cx: &mut Context<Self>,
    ) {
        if self.agent != agent || self.anchor != anchor || self.blocked != blocked {
            self.agent = agent;
            self.anchor = anchor;
            self.hold(blocked, cx);
        }
    }

    /// Hold or release input at once, before the owning view next renders.
    pub fn hold(&mut self, blocked: bool, cx: &mut Context<Self>) {
        self.blocked = blocked;
        self.field
            .update(cx, |field, _| field.set_read_only(blocked));
        cx.notify();
    }

    /// The unsent draft.
    pub fn draft<'a>(&'a self, cx: &'a App) -> &'a str {
        self.field.read(cx).value()
    }

    /// The text of message `index`.
    pub fn text(&self, index: usize) -> &str {
        &self.messages[index].text
    }

    /// Message `index` is being sent again by the owning view.
    pub fn resending(&mut self, index: usize, cx: &mut Context<Self>) {
        self.messages[index].delivery = Delivery::Sending;
        cx.notify();
    }

    /// What became of message `index`, as the owning view's delivery found.
    pub fn complete(&mut self, index: usize, outcome: Outcome, cx: &mut Context<Self>) {
        self.messages[index].delivery = match outcome {
            Outcome::Accepted(_) => Delivery::Sent,
            Outcome::Refused(reason) => Delivery::NotSent(reason),
            Outcome::Unknown(reason) => Delivery::Unconfirmed(reason),
        };
        cx.notify();
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
        if let Some(surface) = &self.surface {
            surface.borrow().focus();
        }
        cx.notify();
    }

    fn can_send(&self, cx: &gpui::App) -> bool {
        let field = self.field.read(cx);
        !self.blocked && !field.composing() && !field.value().trim().is_empty()
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
            anchor: self.anchor.clone(),
            at,
            delivery: Delivery::Sending,
        });
        let index = self.messages.len() - 1;
        if self.record.is_some() {
            self.deliver(index, cx);
        } else {
            cx.emit(ConversationEvent::Send(index));
            cx.notify();
        }
    }

    /// Send message `index` again: a record's message when nothing was
    /// accepted; an owning view's same saved request when the outcome was
    /// unknown. An owning view's refused message is edited and sent anew, since
    /// its context may have changed.
    fn retry(&mut self, index: usize, cx: &mut Context<Self>) {
        match (&self.record, &self.messages[index].delivery) {
            (Some(_), Delivery::NotSent(_)) => self.deliver(index, cx),
            (None, Delivery::Unconfirmed(_)) => cx.emit(ConversationEvent::Retry(index)),
            _ => {}
        }
    }

    /// Move a message that was not sent back into an empty composer.
    fn edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draft(cx).is_empty()
            || !matches!(self.messages[index].delivery, Delivery::NotSent(_))
        {
            return;
        }
        let text = self.messages[index].text.clone();
        self.messages[index].delivery = Delivery::Moved;
        self.field
            .update(cx, |field, cx| field.set_value(&text, cx));
        let focus = self.field.read(cx).focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Send message `index` once, on an I/O worker.
    fn deliver(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((item, directory)) = self.record.clone() else {
            return;
        };
        let message = &mut self.messages[index];
        message.delivery = Delivery::Sending;
        let text = message.text.clone();
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
                conversation.complete(index, outcome, cx)
            });
        })
        .detach();
        cx.notify();
    }
}

/// What the conversation is about, as its header and composer name it.
fn subject(item: &ReportItem) -> &'static str {
    match item.content {
        ReportContent::Progress { .. } => "about this report",
        ReportContent::Response { .. } => "about this answer",
        ReportContent::Document(_) => "about this document",
    }
}

impl Conversation {
    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        ui::conversation_rail(
            div().id("conversation-open").on_click(
                cx.listener(|conversation, _, window, cx| conversation.toggle(window, cx)),
            ),
            self.mark(),
            false,
        )
        .into_any_element()
    }

    fn mark(&self) -> Option<gpui::Div> {
        self.record
            .as_ref()
            .and_then(|(item, _)| reports::mark(item))
    }

    fn thread(&self, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let kind = self.about.trim_start_matches("about this ");
        if self.messages.is_empty() {
            return div()
                .flex_1()
                .min_h_0()
                .child(ui::empty_state(
                    ui::empty_tile("message-square", c.selected, c.text_2),
                    format!("Talk to {} here", self.agent),
                    format!(
                        "Messages you send here go to {} with a reference to this {kind}.",
                        self.agent
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
            if message.delivery == Delivery::Moved {
                continue;
            }
            let local = Local::at(message.at);
            if let (Some(local), Some(now)) = (local, now)
                && day.is_none_or(|day| !day.same_day(&local))
            {
                thread = thread.child(ui::thread_divider(local.day_name(&now), false));
                day = Some(local);
            }
            let time = local.map(|local| local.time()).unwrap_or_default();
            let meta = ui::message_meta().children(message.anchor.clone().map(ui::anchor));
            let action = |id: &str, label: &'static str| {
                ui::link(
                    div()
                        .id(SharedString::from(format!("conversation-{id}-{index}")))
                        .text_color(c.attention_text)
                        .underline()
                        .child(label),
                )
            };
            let meta = match &message.delivery {
                Delivery::Moved => continue,
                Delivery::Sending => meta.child(ui::sending_spinner()).child("Sending…"),
                Delivery::Sent => meta.child(format!("You · {time}")),
                Delivery::NotSent(reason) => {
                    meta.text_color(c.attention_text)
                        .child(SharedString::from(format!("Not sent: {reason}")))
                        .when(self.record.is_some(), |meta| {
                            meta.child(action("retry", "Retry").on_click(cx.listener(
                                move |conversation, _, _, cx| conversation.retry(index, cx),
                            )))
                        })
                        .child(action("edit", "Edit").on_click(cx.listener(
                            move |conversation, _, window, cx| conversation.edit(index, window, cx),
                        )))
                }
                Delivery::Unconfirmed(reason) if self.record.is_none() => meta
                    .child(SharedString::from(format!("Not confirmed: {reason}")))
                    .child(ui::link(
                        div()
                            .id(SharedString::from(format!("conversation-retry-{index}")))
                            .underline()
                            .child("Retry the same request")
                            .on_click(cx.listener(move |conversation, _, _, cx| {
                                conversation.retry(index, cx)
                            })),
                    )),
                Delivery::Unconfirmed(reason) => meta.child(SharedString::from(format!(
                    "Not confirmed: {reason} Ask {} before sending it again.",
                    self.agent
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
        let can_send = self.can_send(cx);
        ui::conversation_panel()
            .child(ui::conversation_header(
                self.mark(),
                self.agent.clone(),
                self.about,
                ui::icon_button("conversation-close", "panel-right-close").on_click(
                    cx.listener(|conversation, _, window, cx| conversation.close(window, cx)),
                ),
            ))
            .child(self.thread(cx))
            .child(ui::composer(
                div().text_size(t.text.row.size()).child(self.field.clone()),
                self.anchor.clone().map(ui::anchor),
                ui::send_button("conversation-send", can_send)
                    .on_click(cx.listener(|conversation, _, _, cx| conversation.send(cx))),
            ))
            .into_any_element()
    }
}
