//! A native reply composer outside the authored document's web surface.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use agent_interface_roster::{
    feedback::{self, Outcome},
    views::ReportItem,
};
use gpui::{
    App, Context, Entity, EventEmitter, Subscription, Window, div, prelude::*, px, relative,
};

use crate::desktop::{
    components as ui,
    design::{colors, tokens},
    name::{NameEvent, NameField},
};

pub(super) enum FeedbackEvent {
    Send,
    Retry,
}
impl EventEmitter<FeedbackEvent> for Feedback {}

pub(super) struct Feedback {
    report: Option<(ReportItem, PathBuf)>,
    label: String,
    sheet_target: Option<String>,
    blocked: bool,
    submitted: String,
    field: Entity<NameField>,
    surface: Option<Rc<RefCell<super::artifact_web::Surface>>>,
    expanded: bool,
    return_focus: Option<gpui::FocusHandle>,
    sending: bool,
    outcome: Option<Outcome>,
    _input: Subscription,
}

impl Feedback {
    pub fn new(
        item: ReportItem,
        directory: PathBuf,
        surface: Rc<RefCell<super::artifact_web::Surface>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(
            format!("Feedback to {} · #{}", item.author, item.id),
            Some((item, directory)),
            Some(surface),
            window,
            cx,
        )
    }

    pub fn for_sheet(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::create(String::new(), None, None, window, cx)
    }

    fn create(
        label: String,
        report: Option<(ReportItem, PathBuf)>,
        surface: Option<Rc<RefCell<super::artifact_web::Surface>>>,
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
            report,
            label,
            sheet_target: None,
            blocked: false,
            submitted: String::new(),
            field,
            surface,
            expanded: false,
            return_focus: None,
            sending: false,
            outcome: None,
            _input: input,
        }
    }

    pub fn value<'a>(&'a self, cx: &'a App) -> &'a str {
        self.field.read(cx).value()
    }

    pub fn sheet_context(
        &mut self,
        target: Option<String>,
        label: String,
        blocked: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = self.sheet_target != target || self.label != label || self.blocked != blocked;
        if self.sheet_target != target && !self.sending && self.value(cx).is_empty() {
            self.outcome = None;
            self.expanded = false;
        }
        self.sheet_target = target;
        self.label = label;
        self.blocked = blocked;
        self.field
            .update(cx, |field, _| field.set_read_only(blocked));
        if changed {
            cx.notify();
        }
    }

    pub fn block_input(&mut self, cx: &mut Context<Self>) {
        self.blocked = true;
        self.field.update(cx, |field, _| field.set_read_only(true));
        cx.notify();
    }

    pub fn retrying(&mut self, cx: &mut Context<Self>) {
        self.sending = true;
        cx.notify();
    }

    pub fn complete(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        self.sending = false;
        if matches!(outcome, Outcome::Accepted(_)) && self.field.read(cx).value() == self.submitted
        {
            self.field.update(cx, |field, cx| field.clear(cx));
        }
        self.outcome = Some(outcome);
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.field.read(cx).value().to_owned();
        if self.sending
            || self.blocked
            || self.field.read(cx).composing()
            || text.trim().is_empty()
            || matches!(self.outcome, Some(Outcome::Unknown(_)))
        {
            return;
        }
        self.sending = true;
        self.outcome = None;
        self.submitted = text.clone();
        let Some((item, directory)) = self.report.clone() else {
            cx.emit(FeedbackEvent::Send);
            cx.notify();
            return;
        };
        let submitted = text.clone();
        let worker = cx.background_executor().spawn(async move {
            let started = std::time::Instant::now();
            let outcome = feedback::send(&directory, &item, &submitted);
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
            let _ = this.update(cx, |composer, cx| {
                composer.complete(outcome, cx);
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for Feedback {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        if !self.expanded {
            return div()
                .flex_shrink_0()
                .flex()
                .justify_end()
                .px(px(t.space.s10))
                .py(px(t.space.s4))
                .child(
                    ui::chip("open-feedback", "Send feedback", false).on_click(cx.listener(
                        |composer, _, window, cx| {
                            composer.return_focus = window.focused(cx);
                            composer.expanded = true;
                            let focus = composer.field.read(cx).focus.clone();
                            window.focus(&focus, cx);
                            cx.notify();
                        },
                    )),
                )
                .into_any_element();
        }
        let text = self.field.read(cx).value();
        let count = text.chars().count();
        let can_send = !self.sending
            && !self.blocked
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
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(t.space.s6))
                    .child(
                        ui::text(div().text_color(c.muted), &t.text.meta).child(self.label.clone()),
                    )
                    .child(
                        ui::chip("close-feedback", "Close", false).on_click(cx.listener(
                            |composer, _, window, cx| {
                                composer.expanded = false;
                                match composer.return_focus.take() {
                                    Some(focus) => window.focus(&focus, cx),
                                    None => window.blur(cx),
                                }
                                if let Some(surface) = &composer.surface {
                                    surface.borrow().focus();
                                }
                                cx.notify();
                            },
                        )),
                    ),
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
                        ui::button(
                            "new-feedback",
                            if self.report.is_some() {
                                "Start a new reply"
                            } else {
                                "Retry the same request"
                            },
                            ui::Button::Secondary,
                        )
                        .on_click(cx.listener(|composer, _, _, cx| {
                            if composer.sending {
                                return;
                            }
                            if composer.report.is_none() {
                                cx.emit(FeedbackEvent::Retry);
                            } else {
                                composer.field.update(cx, |field, cx| field.clear(cx));
                                composer.outcome = None;
                            }
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
            .into_any_element()
    }
}
