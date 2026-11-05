//! Recovery of original first-agent requests, independent of catalog readiness.
use super::{Back, Open, PAGE_SIZE, Ticket, ui};
use crate::{
    desktop::design::{colors, tokens},
    profiles::{
        Client, journal,
        setup_controller::{Controller, Error, Phase, Progress, Snapshot},
    },
};
use agent_interface_roster::profiles::setup::{Event, Prepared};
use futures::{StreamExt, channel::mpsc};
use gpui::{Context, EventEmitter, FocusHandle, SharedString, Window, div, prelude::*, px};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Read,
    Submit,
    Retry,
    Cancel,
    RetryCancel,
}
impl Command {
    fn allowed(self, snapshot: Option<&Snapshot>) -> bool {
        if self == Self::Read {
            return true;
        }
        let Some(s) = snapshot else {
            return false;
        };
        if s.closed() {
            return false;
        }
        let retry = |event: Option<&Event>| {
            event.is_none_or(|e| e.retry.as_deref() == Some("same_request"))
        };
        match self {
            Self::Read => true,
            Self::Submit => s.phase == Phase::Prepared,
            Self::Cancel => s.phase == Phase::Submitted && s.cancellation_result.is_none(),
            Self::Retry => {
                s.phase != Phase::Prepared
                    && retry(s.first_result.as_ref())
                    && (s.phase != Phase::CancelPending
                        || s.cancellation_result.as_ref().is_some_and(|e| {
                            e.status == "action_required"
                                && e.code.as_deref() == Some("cancellation_unsafe")
                        }))
            }
            Self::RetryCancel => {
                s.phase != Phase::Prepared
                    && (s.phase == Phase::CancelPending || s.cancellation_result.is_some())
                    && retry(s.cancellation_result.as_ref())
            }
        }
    }
}
fn status(s: &Snapshot) -> &'static str {
    if s.cancellation_result.as_ref().is_some_and(|e| e.cancelled) {
        "Safely cancelled"
    } else if s
        .first_result
        .as_ref()
        .is_some_and(|e| e.ready_agent.is_some())
    {
        "Agent readiness confirmed"
    } else if s.phase == Phase::Prepared {
        "Saved, not submitted"
    } else if s.phase == Phase::CancelPending && s.cancellation_result.is_none() {
        "Cancellation outcome not confirmed"
    } else if s.first_result.is_none() && s.cancellation_result.is_none() {
        "Setup outcome not confirmed"
    } else if s.closed() {
        "Agent not added"
    } else {
        "Needs attention"
    }
}
/// The agent exists but needs the person, so its terminal is the next step.
fn agent_waiting(s: &Snapshot) -> bool {
    !s.closed()
        && s.phase == Phase::Submitted
        && s.first_result.as_ref().is_some_and(|event| {
            matches!(
                event.code.as_deref(),
                Some("launch_timeout" | "runtime_permission")
            )
        })
}
/// What the latest result means for the person, for each code the provider sends.
fn outcome(s: &Snapshot) -> Option<String> {
    let event = if s.phase == Phase::CancelPending {
        s.cancellation_result.as_ref()
    } else {
        s.first_result.as_ref()
    }?;
    Some(match event.code.as_deref()? {
        "agent_ready" | "request_cancelled" => return None,
        "agent_conflict" => format!(
            "An agent or terminal in this profile already uses the name {} or its folder. Go back and choose another.",
            s.agent_name
        ),
        "invalid_request" => "The agent name, folder or model was refused. Go back and change it.".into(),
        "launch_timeout" => format!(
            "{} was added but has not reported ready. Open it, or sign in if it asks, then retry.",
            s.agent_name
        ),
        "runtime_permission" => format!(
            "{} is waiting for you, often for a sign-in or a permission. Open it to continue, then retry.",
            s.agent_name
        ),
        "runtime_missing" => "This CLI is not installed where the profile can use it. Install it, then retry.".into(),
        "starter_conflict" => "The profile's starter files or this brief need attention. Fix them, then retry.".into(),
        "launch_failed" => format!("{} could not start. Check it in the workspace.", s.agent_name),
        "outcome_unknown" => format!(
            "The result is not known. Check whether {} is in the workspace before adding it again.",
            s.agent_name
        ),
        "cancellation_unsafe" => "Setup may already have made changes, so it could not be cancelled. The request and any agent were kept.".into(),
        code => format!("The profile answered: {code}."),
    })
}
fn message(error: Error) -> &'static str {
    match error {
        Error::Missing => "The original saved request is missing. No replacement was created.",
        Error::Invalid => "This saved setup request could not be validated.",
        Error::WrongPhase | Error::Completed => {
            "The saved request changed. Reload its original state."
        }
        Error::Pending => {
            "A saved request for this agent name is still open. Open Saved setup requests to finish or forget it, or use another name."
        }
        Error::RetryRefused => "The retained provider result does not allow this retry.",
        Error::Storage(journal::Error::Unsafe) => "Setup storage is not private or safe to use.",
        Error::Storage(journal::Error::Busy | journal::Error::Conflict) => {
            "Another update won. Reload the original request."
        }
        Error::Storage(_) => "Saved setup state could not be confirmed. Reload before retrying.",
        Error::Client(_) => {
            "The outcome could not be confirmed. Keep and recover this original request."
        }
    }
}

enum Page {
    Loading,
    History {
        ids: Vec<String>,
        offset: usize,
        rows: Vec<(String, Result<Snapshot, Error>)>,
    },
    Operation {
        key: String,
        snapshot: Option<Box<Snapshot>>,
        progress: Option<Box<Progress>>,
    },
}
enum Update {
    Saved(Box<Snapshot>),
    Progress(Box<Progress>),
    Finished(Box<Result<Snapshot, Error>>, Option<Error>),
}
pub(super) struct Pane {
    controller: Option<Arc<Controller>>,
    page: Page,
    ticket: Ticket,
    busy: bool,
    finished: bool,
    error: Option<Error>,
    pub focus: FocusHandle,
}
impl EventEmitter<Back> for Pane {}
impl EventEmitter<Open> for Pane {}
impl Pane {
    pub fn new(client: Client, directory: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut pane = Self {
            controller: None,
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: true,
            finished: false,
            error: None,
            focus: cx.focus_handle(),
        };
        let ticket = pane.ticket.begin();
        let worker = ticket.clone();
        let work = cx.background_executor().spawn(async move {
            let controller = Arc::new(Controller::new(
                journal::Journal::open(&directory.join("profile-setup"))?,
                &client,
            ));
            let page = history(&controller, None, 0, &worker);
            Ok::<_, Error>((controller, page))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if !pane.accepts(&ticket) {
                    return;
                }
                pane.busy = false;
                match result {
                    Ok((controller, page)) => {
                        pane.controller = Some(controller);
                        pane.complete(page);
                    }
                    Err(error) => pane.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        pane
    }
    pub fn from_controller(controller: Arc<Controller>, cx: &mut Context<Self>) -> Self {
        let mut pane = Self {
            controller: Some(controller),
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: false,
            finished: false,
            error: None,
            focus: cx.focus_handle(),
        };
        let ticket = pane.ticket.begin();
        pane.history(0, &ticket, cx);
        pane
    }
    pub fn from_prepared(
        controller: Arc<Controller>,
        prepared: Prepared,
        cx: &mut Context<Self>,
    ) -> Self {
        // Keep the original key before persistence: an enqueue sync failure can
        // already have saved intent, and must never allocate a replacement.
        let mut pane = Self {
            controller: Some(controller),
            page: Page::Operation {
                key: Controller::key(prepared.request_id()),
                snapshot: None,
                progress: None,
            },
            ticket: Ticket::default(),
            busy: false,
            finished: false,
            error: None,
            focus: cx.focus_handle(),
        };
        pane.run(Command::Submit, Some(prepared), cx);
        pane
    }
    fn accepts(&self, ticket: &Arc<AtomicBool>) -> bool {
        !self.finished && self.ticket.accepts(ticket)
    }
    fn complete(&mut self, result: Result<Page, Error>) {
        self.busy = false;
        match result {
            Ok(page) => {
                self.page = page;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ticket.cancel();
        cx.emit(Back);
    }
    fn history(&mut self, offset: usize, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let ids = match &self.page {
            Page::History { ids, .. } if offset > 0 => Some(ids.clone()),
            _ => None,
        };
        self.busy = true;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let work = cx
            .background_executor()
            .spawn(async move { history(&controller, ids, offset, &worker) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if pane.accepts(&ticket) {
                    pane.complete(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn open(&mut self, key: &str, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        self.page = Page::Operation {
            key: key.into(),
            snapshot: None,
            progress: None,
        };
        self.invoke(Command::Read, expected, cx);
    }
    fn invoke(&mut self, command: Command, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        let Page::Operation { snapshot, .. } = &self.page else {
            return;
        };
        if !command.allowed(snapshot.as_deref()) {
            return;
        }
        self.run(command, None, cx);
    }
    fn run(&mut self, command: Command, prepared: Option<Prepared>, cx: &mut Context<Self>) {
        let Page::Operation { key, .. } = &self.page else {
            return;
        };
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let key = key.clone();
        self.busy = true;
        self.error = None;
        if let Page::Operation { progress, .. } = &mut self.page {
            *progress = None;
        }
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        // The protocol bounds every invocation to 1024 events. This channel
        // carries only validated progress and one final durable observation.
        let (sender, mut receiver) = mpsc::unbounded();
        cx.background_executor()
            .spawn(async move {
                let live = || worker.load(Ordering::Acquire);
                let emit = |progress| {
                    let _ = sender.unbounded_send(Update::Progress(Box::new(progress)));
                };
                let admission = match prepared {
                    Some(prepared) => controller.enqueue(prepared).map(|snapshot| {
                        let _ = sender.unbounded_send(Update::Saved(Box::new(snapshot)));
                    }),
                    None => Ok(()),
                };
                let result = admission.and_then(|_| match command {
                    Command::Read => Ok(()),
                    Command::Submit => controller.submit(&key, &live, emit).map(|_| ()),
                    Command::Retry => controller.retry(&key, &live, emit).map(|_| ()),
                    Command::Cancel => controller.cancel(&key, &live, emit).map(|_| ()),
                    Command::RetryCancel => controller.retry_cancel(&key, &live, emit).map(|_| ()),
                });
                let _ = sender.unbounded_send(Update::Finished(
                    Box::new(controller.read(&key)),
                    result.err(),
                ));
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Some(update) = receiver.next().await {
                let accepted = this.update(cx, |pane, cx| {
                    if !pane.accepts(&ticket) {
                        return false;
                    }
                    let Page::Operation {
                        key,
                        snapshot,
                        progress,
                    } = &mut pane.page
                    else {
                        return false;
                    };
                    match update {
                        Update::Saved(next) => {
                            if next.key == *key && snapshot.is_none() {
                                *snapshot = Some(next);
                            }
                        }
                        Update::Progress(next) => {
                            if progress_matches(
                                snapshot.as_deref(),
                                progress.as_deref(),
                                key,
                                &next,
                            ) {
                                *progress = Some(next);
                            }
                        }
                        Update::Finished(result, error) => {
                            pane.busy = false;
                            *progress = None;
                            match *result {
                                Ok(next) if next.key == *key => {
                                    *snapshot = Some(Box::new(next));
                                    pane.error = error;
                                }
                                Ok(_) => {
                                    *snapshot = None;
                                    pane.error = Some(Error::Invalid);
                                }
                                Err(failure) => {
                                    *snapshot = None;
                                    pane.error = Some(error.unwrap_or(failure));
                                }
                            }
                        }
                    }
                    cx.notify();
                    true
                });
                if !matches!(accepted, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }
    fn forget(&mut self, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        let Page::Operation { key, snapshot, .. } = &self.page else {
            return;
        };
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let (key, revision) = (key.clone(), snapshot.as_ref().map(|s| s.revision));
        self.busy = true;
        self.error = None;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let work = cx.background_executor().spawn(async move {
            controller.forget(&key, revision)?;
            history(&controller, None, 0, &worker)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if pane.accepts(&ticket) {
                    pane.complete(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn stop_observing(&mut self, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if !self.busy || !self.accepts(expected) || !matches!(self.page, Page::Operation { .. }) {
            return;
        }
        // Interrupt observation, then read the same durable key. This is not a
        // cancellation receipt and never unlocks replacement setup choices.
        let ticket = self.ticket.begin();
        self.busy = false;
        self.invoke(Command::Read, &ticket, cx);
    }
}
fn progress_matches(
    snapshot: Option<&Snapshot>,
    previous: Option<&Progress>,
    key: &str,
    next: &Progress,
) -> bool {
    let Some(snapshot) = snapshot else {
        return false;
    };
    next.key == key
        && next.event.identity == snapshot.identity
        && next.event.request_id == snapshot.request_id
        && next.revision > snapshot.revision
        && previous.is_none_or(|old| {
            next.revision > old.revision
                || (next.revision == old.revision && next.event.sequence > old.event.sequence)
        })
}
fn history(
    controller: &Controller,
    ids: Option<Vec<String>>,
    offset: usize,
    live: &AtomicBool,
) -> Result<Page, Error> {
    let ids = match ids {
        Some(ids) => ids,
        None => controller.ids()?,
    };
    if offset > ids.len() {
        return Err(Error::Invalid);
    }
    let mut rows = Vec::new();
    for key in ids.iter().skip(offset).take(PAGE_SIZE) {
        if !live.load(Ordering::Acquire) {
            return Err(Error::Client(crate::profiles::Error::Cancelled));
        }
        rows.push((key.clone(), controller.read(key)));
    }
    Ok(Page::History { ids, offset, rows })
}
impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let ticket = self.ticket.0.clone();
        let mut body = div().id("setup-recovery-body").flex_1().min_h_0().overflow_y_scroll().p(px(t.space.s12)).flex().flex_col().gap(px(t.space.s8))
            .child(ui::group_note("Closing interrupts the local setup command. The provider outcome may remain unknown; closing does not confirm safe cancellation. Recovery keeps the original profile, choices and request; confirmed readiness does not connect a workspace."));
        match &self.page {
            Page::Loading => body = body.child(ui::group_note("Opening saved setup requests…")),
            Page::History { ids, offset, rows } => {
                if rows.is_empty() {
                    body = body.child(ui::group_note("No saved agent setup requests."));
                }
                for (key, result) in rows {
                    let label = match result {
                        Ok(s) => format!("{} · {}", s.identity.profile_id, status(s)),
                        Err(e) => message(*e).into(),
                    };
                    let key = key.clone();
                    let ticket = ticket.clone();
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("setup-{key}")))
                            .w_full()
                            .min_w_0()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .p(px(t.space.s12))
                            .gap(px(t.space.s8))
                            .bg(c.field)
                            .rounded(px(t.radius.row))
                            .cursor_pointer()
                            .hover(|row| row.bg(colors().hover))
                            .child(div().w_full().whitespace_normal().child(label))
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(format!("Operation {key}")),
                            )
                            .on_click(
                                cx.listener(move |pane, _, _, cx| pane.open(&key, &ticket, cx)),
                            ),
                    );
                }
                if !self.busy {
                    for (offset, label) in [
                        (offset.checked_sub(PAGE_SIZE), "Previous"),
                        (
                            (offset + PAGE_SIZE < ids.len()).then_some(offset + PAGE_SIZE),
                            "Next",
                        ),
                    ] {
                        if let Some(offset) = offset {
                            let ticket = ticket.clone();
                            body = body.child(
                                ui::button(label, label, ui::Button::Secondary).on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        pane.history(offset, &ticket, cx)
                                    }),
                                ),
                            );
                        }
                    }
                }
            }
            Page::Operation {
                key,
                snapshot,
                progress,
            } => {
                body = body.child(format!("Operation {key}"));
                if let Some(s) = snapshot {
                    body = body
                        .child(status(s))
                        .child(format!("Request {}", s.request_id))
                        .child(format!(
                            "Profile {} · installation {} · instance {}",
                            s.identity.profile_id,
                            s.identity.installation_id,
                            s.identity.lifecycle_id
                        ));
                    if let Some(name) = s.first_result.as_ref().and_then(|e| e.ready_agent.as_ref())
                    {
                        body = body.child(format!("Ready agent: {name}"));
                    }
                    if let Some(text) = outcome(s) {
                        body = body.child(ui::group_note(text));
                    }
                    if !self.busy && agent_waiting(s) {
                        let (agent, sign_in) = (
                            Open::Agent {
                                profile_id: s.identity.profile_id.clone(),
                                name: s.agent_name.clone(),
                            },
                            s.runtime.clone().map(|runtime| Open::SignIn {
                                profile_id: s.identity.profile_id.clone(),
                                runtime,
                            }),
                        );
                        body = body.child(
                            ui::button("setup-open-agent", "Open agent", ui::Button::Primary)
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(agent.clone()))),
                        );
                        if let Some(sign_in) = sign_in {
                            body = body.child(
                                ui::button("setup-sign-in", "Sign in", ui::Button::Secondary)
                                    .on_click(
                                        cx.listener(move |_, _, _, cx| cx.emit(sign_in.clone())),
                                    ),
                            );
                        }
                    }
                }
                if progress.is_some() {
                    body = body.child(ui::group_note(
                        "Provider progress received. Waiting for a durable result…",
                    ));
                }
                if self.busy {
                    let ticket = ticket.clone();
                    body = body.child(
                        ui::button(
                            "setup-stop-observing",
                            "Stop waiting and reload",
                            ui::Button::Secondary,
                        )
                        .on_click(
                            cx.listener(move |pane, _, _, cx| pane.stop_observing(&ticket, cx)),
                        ),
                    );
                } else {
                    for (command, label) in [
                        (Command::Read, "Reload saved state"),
                        (Command::Submit, "Submit saved setup"),
                        (Command::Retry, "Retry identical setup"),
                        (Command::Cancel, "Cancel original setup"),
                        (Command::RetryCancel, "Retry identical cancellation"),
                    ] {
                        if command.allowed(snapshot.as_deref()) {
                            let ticket = ticket.clone();
                            body = body.child(
                                ui::button(
                                    label,
                                    label,
                                    if matches!(command, Command::Cancel | Command::RetryCancel) {
                                        ui::Button::Danger
                                    } else {
                                        ui::Button::Secondary
                                    },
                                )
                                .on_click(cx.listener(
                                    move |pane, _, _, cx| pane.invoke(command, &ticket, cx),
                                )),
                            );
                        }
                    }
                    if snapshot.is_some() || self.error == Some(Error::Invalid) {
                        let ticket = ticket.clone();
                        body = body
                            .child(ui::group_note("Forgetting removes this saved request from the app. It does not stop or remove an agent."))
                            .child(
                                ui::button("setup-forget", "Forget this request", ui::Button::Secondary)
                                    .on_click(cx.listener(move |pane, _, _, cx| pane.forget(&ticket, cx))),
                            );
                    }
                }
            }
        }
        if self.busy {
            body = body.child(ui::group_note("Working…"));
        }
        if let Some(error) = self.error {
            body = body.child(ui::group_note(message(error)));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|pane, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    pane.back(cx);
                    cx.stop_propagation();
                }
            }))
            .child(ui::view_header("layers", "Saved agent setup"))
            .child(body)
            .child(
                ui::view_toolbar()
                    .px(px(t.space.s12))
                    .gap(px(t.space.s8))
                    .child(
                        ui::button("setup-back", "Back", ui::Button::Secondary)
                            .on_click(cx.listener(|pane, _, _, cx| pane.back(cx))),
                    )
                    .when(self.controller.is_some() && !self.busy, |bar| {
                        bar.child(
                            ui::button(
                                "setup-history",
                                "Saved setup requests",
                                ui::Button::Secondary,
                            )
                            .on_click(
                                cx.listener(move |pane, _, _, cx| pane.history(0, &ticket, cx)),
                            ),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests;
