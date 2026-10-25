//! A native reply composer outside the authored document's web surface.

use std::path::PathBuf;

use gpui::{Context, Entity, Subscription, Window, div, prelude::*, px, relative};
use tmux_workspaces_roster::{
    feedback::{self, Outcome},
    views::ReportItem,
};

use crate::desktop::{
    components as ui,
    design::{colors, tokens},
    name::{NameEvent, NameField},
};

pub(super) struct Feedback {
    item: ReportItem,
    directory: PathBuf,
    field: Entity<NameField>,
    sending: bool,
    outcome: Option<Outcome>,
    _input: Subscription,
}

impl Feedback {
    pub fn new(
        item: ReportItem,
        directory: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| NameField::feedback(window, cx));
        let input = cx.subscribe(&field, |composer, _, event, cx| {
            if matches!(event, NameEvent::Changed(_)) {
                if matches!(
                    composer.outcome,
                    Some(Outcome::Accepted(_) | Outcome::Refused(_))
                ) {
                    composer.outcome = None;
                }
                cx.notify();
            }
        });
        Self {
            item,
            directory,
            field,
            sending: false,
            outcome: None,
            _input: input,
        }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.field.read(cx).value().to_owned();
        if self.sending
            || self.field.read(cx).composing()
            || text.trim().is_empty()
            || matches!(self.outcome, Some(Outcome::Unknown(_)))
        {
            return;
        }
        self.sending = true;
        self.outcome = None;
        let directory = self.directory.clone();
        let item = self.item.clone();
        let submitted = text.clone();
        let worker = cx
            .background_executor()
            .spawn(async move { feedback::send(&directory, &item, &submitted) });
        cx.spawn(async move |this, cx| {
            let outcome = worker.await;
            let _ = this.update(cx, |composer, cx| {
                composer.sending = false;
                if matches!(outcome, Outcome::Accepted(_))
                    && composer.field.read(cx).value() == text
                {
                    composer.field.update(cx, |field, cx| field.clear(cx));
                }
                composer.outcome = Some(outcome);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for Feedback {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let text = self.field.read(cx).value();
        let count = text.chars().count();
        let can_send = !self.sending
            && !self.field.read(cx).composing()
            && !text.trim().is_empty()
            && !matches!(self.outcome, Some(Outcome::Unknown(_)));
        let content = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .p(px(t.space.s10))
            .border_t_1()
            .border_color(c.line)
            .bg(c.surface)
            .child(
                ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                    "Feedback to {} · #{}",
                    self.item.author, self.item.id
                )),
            )
            .child(self.field.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        ui::text(div().text_color(c.muted), &t.text.meta)
                            .child(format!("{count} / {}", feedback::TEXT_LIMIT)),
                    )
                    .child(
                        ui::button(
                            "send-feedback",
                            if self.sending {
                                "Sending…"
                            } else {
                                "Send feedback"
                            },
                            ui::Button::Primary,
                        )
                        .when(!can_send, |button| button.opacity(0.5))
                        .on_click(cx.listener(|composer, _, _, cx| composer.send(cx))),
                    ),
            )
            .when_some(self.outcome.as_ref(), |body, outcome| {
                let (message, attention) = match outcome {
                    Outcome::Accepted(text) => (text, false),
                    Outcome::Refused(text) | Outcome::Unknown(text) => (text, true),
                };
                body.child(
                    ui::text(
                        div().text_color(if attention { c.attention_text } else { c.muted }),
                        &t.text.meta,
                    )
                    .child(message.clone()),
                )
                .when(matches!(outcome, Outcome::Unknown(_)), |body| {
                    body.child(
                        ui::button("new-feedback", "Start a new reply", ui::Button::Secondary)
                            .on_click(cx.listener(|composer, _, _, cx| {
                                composer.field.update(cx, |field, cx| field.clear(cx));
                                composer.outcome = None;
                                cx.notify();
                            })),
                    )
                })
            });
        div()
            .id("feedback-scroll")
            .flex_shrink_0()
            .max_h(relative(0.5))
            .min_h(px(0.))
            .overflow_y_scroll()
            .child(content)
    }
}
