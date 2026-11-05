//! Explicit lifecycle controls. Provider receipts never activate a workspace.

use super::{
    components as ui,
    design::{colors, tokens},
};
use crate::profiles::{
    Client,
    controller::{self, Controller, Phase, Snapshot},
    journal,
};
use agent_interface_roster::profiles::{
    Kind, Profile, State,
    operations::{Action, Prepared, Status},
};
use gpui::{Context, EventEmitter, FocusHandle, SharedString, Window, div, prelude::*, px};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

mod choices;
mod creation;
mod setup;

const PAGE_SIZE: usize = 16;
pub(super) struct Back;

#[derive(Default)]
struct Ticket(Arc<AtomicBool>);
impl Ticket {
    fn begin(&mut self) -> Arc<AtomicBool> {
        self.0.store(false, Ordering::Release);
        self.0 = Arc::new(AtomicBool::new(true));
        self.0.clone()
    }
    fn accepts(&self, ticket: &Arc<AtomicBool>) -> bool {
        Arc::ptr_eq(&self.0, ticket) && ticket.load(Ordering::Acquire)
    }
    fn cancel(&self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel();
    }
}

enum Page {
    Loading,
    Detail(Profile),
    History {
        ids: Vec<String>,
        offset: usize,
        rows: Vec<(String, Result<Snapshot, controller::Error>)>,
    },
    Operation {
        key: String,
        snapshot: Option<Snapshot>,
    },
}

pub(super) struct Management {
    client: Client,
    directory: Option<PathBuf>,
    setup: Option<(gpui::Entity<setup::Pane>, gpui::Subscription)>,
    choices: Option<(gpui::Entity<choices::Form>, gpui::Subscription)>,
    form: Option<(gpui::Entity<creation::Form>, gpui::Subscription)>,
    controller: Option<Arc<Controller>>,
    page: Page,
    ticket: Ticket,
    busy: bool,
    error: Option<controller::Error>,
    pub focus: FocusHandle,
}
impl EventEmitter<Back> for Management {}
impl Management {
    /// Restore focus to the deepest visible pane, including a creation form.
    pub fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        if let Some((pane, _)) = &self.setup {
            return pane.read(cx).focus.clone();
        }
        if let Some((form, _)) = &self.choices {
            return form.read(cx).focus.clone();
        }
        self.form.as_ref().map_or_else(
            || self.focus.clone(),
            |(form, _)| form.read(cx).focus.clone(),
        )
    }
    pub fn new(
        client: Client,
        directory: Option<PathBuf>,
        profile: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            client: client.clone(),
            directory: None,
            setup: None,
            form: None,
            choices: None,
            controller: None,
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: true,
            error: None,
            focus: cx.focus_handle(),
        };
        let ticket = pane.ticket.begin();
        let worker = ticket.clone();
        let work = cx
            .background_executor()
            .spawn(async move { open_management(client, directory, profile, &worker) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if !pane.ticket.accepts(&ticket) {
                    return;
                }
                pane.busy = false;
                match result {
                    Ok(opened) => {
                        pane.directory = Some(opened.directory);
                        pane.controller = opened.controller;
                        pane.complete(opened.page);
                    }
                    Err(error) => pane.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        pane
    }

    fn complete(&mut self, result: Result<Page, controller::Error>) {
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
        self.ticket.cancel();
        cx.emit(Back);
    }
    fn start(&mut self, action: Action, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy
            || self.form.is_some()
            || self.setup.is_some()
            || self.choices.is_some()
            || !self.ticket.accepts(expected)
        {
            return;
        }
        let Page::Detail(profile) = &self.page else {
            return;
        };
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let request = uuid::Uuid::new_v4().simple().to_string();
        let prepared = match action {
            Action::Start => Prepared::start(&request, profile),
            Action::Stop => Prepared::stop(&request, profile),
            _ => return,
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.error = Some(controller::Error::Client(crate::profiles::Error::Profile(
                    error,
                )));
                cx.notify();
                return;
            }
        };
        self.submit_prepared(prepared, controller, cx);
    }
    fn submit_prepared(
        &mut self,
        prepared: Prepared,
        controller: Arc<Controller>,
        cx: &mut Context<Self>,
    ) {
        let key = Controller::key(prepared.request_id());
        // Retain the key before even enqueueing: a failed sync can be ambiguous.
        self.page = Page::Operation {
            key: key.clone(),
            snapshot: None,
        };
        self.error = None;
        self.busy = true;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let work = cx.background_executor().spawn(async move {
            let result = controller
                .enqueue(prepared)
                .and_then(|_| controller.submit(&key, &|| worker.load(Ordering::Acquire)));
            (controller.read(&key), result.err())
        });
        self.observe_operation(ticket, work, cx);
    }
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy
            || self.form.is_some()
            || self.setup.is_some()
            || self.choices.is_some()
            || self.controller.is_none()
        {
            return;
        }
        self.ticket.begin();
        let form = cx.new(|cx| creation::Form::new(window, cx));
        window.focus(&form.read(cx).focus.clone(), cx);
        let subscription = cx.subscribe_in(
            &form,
            window,
            |pane, form, event: &creation::Event, window, cx| {
                if !pane.form.as_ref().is_some_and(|(active, _)| active == form) {
                    return;
                }
                match event {
                    creation::Event::Back => pane.form = None,
                    creation::Event::Submit => {
                        let prepared = form.update(cx, |form, _| form.take_prepared());
                        let Some(prepared) = prepared else {
                            return;
                        };
                        let Some(controller) = pane.controller.clone() else {
                            return;
                        };
                        pane.form = None;
                        pane.submit_prepared(prepared, controller, cx);
                    }
                }
                window.focus(&pane.focus, cx);
                cx.notify();
            },
        );
        self.form = Some((form, subscription));
        cx.notify();
    }
    fn setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.form.is_some() || self.setup.is_some() || self.choices.is_some() {
            return;
        }
        let Some(directory) = self.directory.clone() else {
            return;
        };
        self.ticket.begin();
        let client = self.client.clone();
        let setup = cx.new(|cx| setup::Pane::new(client, directory, cx));
        self.attach_setup(setup, window, cx);
    }
    fn attach_setup(
        &mut self,
        setup: gpui::Entity<setup::Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&setup.read(cx).focus.clone(), cx);
        let subscription = cx.subscribe_in(&setup, window, |pane, setup, _: &Back, window, cx| {
            if !pane
                .setup
                .as_ref()
                .is_some_and(|(active, _)| active == setup)
            {
                return;
            }
            pane.setup = None;
            window.focus(&pane.focus, cx);
            cx.notify();
        });
        self.setup = Some((setup, subscription));
        cx.notify();
    }
    fn choices(&mut self, expected: &Arc<AtomicBool>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy
            || self.form.is_some()
            || self.setup.is_some()
            || self.choices.is_some()
            || !self.ticket.accepts(expected)
        {
            return;
        }
        let Page::Detail(profile) = &self.page else {
            return;
        };
        if profile.item().state != State::Ready {
            return;
        }
        let Some(directory) = self.directory.clone() else {
            return;
        };
        let profile = profile.clone();
        let client = self.client.clone();
        self.ticket.begin();
        let form = cx.new(|cx| choices::Form::new(client, directory, profile, window, cx));
        window.focus(&form.read(cx).focus.clone(), cx);
        let subscription = cx.subscribe_in(
            &form,
            window,
            |pane, form, event: &choices::Event, window, cx| {
                if !pane
                    .choices
                    .as_ref()
                    .is_some_and(|(active, _)| active == form)
                {
                    return;
                }
                let setup = match event {
                    choices::Event::Back => None,
                    choices::Event::Recover => {
                        let Some(controller) = form.read(cx).controller() else {
                            return;
                        };
                        Some(cx.new(|cx| setup::Pane::from_controller(controller, cx)))
                    }
                    choices::Event::Submit => {
                        let Some((prepared, controller)) =
                            form.update(cx, |form, _| form.take_prepared())
                        else {
                            return;
                        };
                        Some(cx.new(|cx| setup::Pane::from_prepared(controller, prepared, cx)))
                    }
                };
                pane.choices = None;
                if let Some(setup) = setup {
                    pane.attach_setup(setup, window, cx);
                } else {
                    window.focus(&pane.focus, cx);
                }
                cx.notify();
            },
        );
        self.choices = Some((form, subscription));
        cx.notify();
    }
    fn observe_operation(
        &mut self,
        ticket: Arc<AtomicBool>,
        work: gpui::Task<(
            Result<Snapshot, controller::Error>,
            Option<controller::Error>,
        )>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let (snapshot, error) = work.await;
            let _ = this.update(cx, |pane, cx| {
                if !pane.ticket.accepts(&ticket) {
                    return;
                }
                pane.busy = false;
                let Page::Operation {
                    key,
                    snapshot: current,
                } = &mut pane.page
                else {
                    return;
                };
                match snapshot {
                    Ok(snapshot) if snapshot.key == *key => {
                        *current = Some(snapshot);
                        pane.error = error;
                    }
                    Ok(_) => {
                        *current = None;
                        pane.error = Some(controller::Error::Invalid);
                    }
                    Err(failure) => {
                        *current = None;
                        pane.error = Some(error.unwrap_or(failure));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn recover(&mut self, command: Command, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy
            || self.form.is_some()
            || self.setup.is_some()
            || self.choices.is_some()
            || !self.ticket.accepts(expected)
        {
            return;
        }
        let Page::Operation { key, snapshot } = &self.page else {
            return;
        };
        let Some(controller) = self.controller.clone() else {
            return;
        };
        if !command.allowed(snapshot.as_ref()) {
            return;
        }
        let key = key.clone();
        self.busy = true;
        self.error = None;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let work = cx.background_executor().spawn(async move {
            let live = || worker.load(Ordering::Acquire);
            let error = match command {
                Command::Read => None,
                Command::Submit => controller.submit(&key, &live).err(),
                Command::Refresh => controller.refresh(&key, &live).err(),
                Command::Retry => controller.retry(&key, &live).err(),
            };
            (controller.read(&key), error)
        });
        self.observe_operation(ticket, work, cx);
    }
    fn history(&mut self, offset: usize, retain_ids: bool, cx: &mut Context<Self>) {
        if self.busy || self.form.is_some() || self.setup.is_some() || self.choices.is_some() {
            return;
        }
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let ids = match &self.page {
            Page::History { ids, .. } if retain_ids => Some(ids.clone()),
            _ => None,
        };
        self.busy = true;
        self.error = None;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let work = cx
            .background_executor()
            .spawn(async move { history(&controller, ids, offset, &worker) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if pane.ticket.accepts(&ticket) {
                    pane.complete(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn open_record(&mut self, key: &str, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy
            || self.form.is_some()
            || self.setup.is_some()
            || self.choices.is_some()
            || !self.ticket.accepts(expected)
        {
            return;
        }
        let Page::History { rows, .. } = &self.page else {
            return;
        };
        if !rows.iter().any(|(id, _)| id == key) {
            return;
        }
        self.page = Page::Operation {
            key: key.into(),
            snapshot: None,
        };
        self.recover(Command::Read, expected, cx);
    }
}

#[derive(Clone, Copy)]
enum Command {
    Read,
    Submit,
    Refresh,
    Retry,
}
impl Command {
    fn allowed(self, snapshot: Option<&Snapshot>) -> bool {
        if matches!(self, Self::Read) {
            return true;
        }
        snapshot.is_some_and(|snapshot| {
            snapshot.terminal.is_none()
                && match self {
                    Self::Read => true,
                    Self::Submit => snapshot.phase == Phase::Prepared,
                    Self::Refresh | Self::Retry => snapshot.phase == Phase::Submitted,
                }
        })
    }
}
struct Opened {
    directory: PathBuf,
    controller: Option<Arc<Controller>>,
    page: Result<Page, controller::Error>,
}
fn open_management(
    client: Client,
    directory: Option<PathBuf>,
    profile: Option<String>,
    live: &AtomicBool,
) -> Result<Opened, controller::Error> {
    let directory = crate::library::location::library_directory(directory.as_deref())
        .map_err(|_| controller::Error::Storage(journal::Error::Unavailable))?;
    // Keep the resolved library location even if this independent journal is
    // unsafe. Setup recovery must still enforce and use its own journal checks.
    let controller = journal::Journal::open(&directory.join("profile-operations"))
        .map(|journal| Arc::new(Controller::new(journal, &client)))
        .map_err(controller::Error::Storage);
    let page = match &controller {
        Ok(controller) => match profile {
            Some(id) => client
                .show(&id, &|| live.load(Ordering::Acquire))
                .map(Page::Detail)
                .map_err(controller::Error::Client),
            None => history(controller, None, 0, live),
        },
        Err(error) => Err(*error),
    };
    Ok(Opened {
        directory,
        controller: controller.ok(),
        page,
    })
}

fn history(
    controller: &Controller,
    ids: Option<Vec<String>>,
    offset: usize,
    live: &AtomicBool,
) -> Result<Page, controller::Error> {
    let ids = match ids {
        Some(ids) => ids,
        None => controller.ids()?,
    };
    let offset = offset.min(ids.len().saturating_sub(1) / PAGE_SIZE * PAGE_SIZE);
    let mut rows = Vec::new();
    for key in ids.iter().skip(offset).take(PAGE_SIZE) {
        if !live.load(Ordering::Acquire) {
            return Err(controller::Error::Client(crate::profiles::Error::Cancelled));
        }
        rows.push((key.clone(), controller.read(key)));
    }
    Ok(Page::History { ids, offset, rows })
}
fn action_label(action: Action) -> &'static str {
    match action {
        Action::Create => "Create profile",
        Action::Start => "Start service",
        Action::Stop => "Stop service",
    }
}
fn status(snapshot: &Snapshot) -> &'static str {
    match snapshot.terminal.as_ref().map(|receipt| receipt.status) {
        Some(Status::Succeeded) => "Succeeded",
        Some(Status::Failed) => "Failed",
        Some(Status::Pending) => "Pending",
        None if snapshot.phase == Phase::Prepared => "Prepared, not submitted",
        None => "Outcome not yet confirmed",
    }
}
fn message(error: controller::Error) -> &'static str {
    use controller::Error::*;
    match error {
        Missing => "This operation record is missing. No request was sent by this read.",
        Invalid => "This operation record could not be validated.",
        WrongPhase | Completed => "The operation changed. Reload its saved state.",
        Client(_) => {
            "The provider result could not be confirmed. Keep this operation and check its original receipt."
        }
        Storage(journal::Error::Unsafe) => "Operation storage is not private or safe to use.",
        Storage(journal::Error::Busy | journal::Error::Conflict) => {
            "Another operation update won. Reload the saved state."
        }
        Storage(journal::Error::TooLarge) => "Operation storage has reached its capacity.",
        Storage(_) => "Operation storage could not be confirmed. Reload before retrying.",
    }
}

impl Render for Management {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((setup, _)) = &self.setup {
            return div().size_full().child(setup.clone());
        }
        if let Some((form, _)) = &self.choices {
            return div().size_full().child(form.clone());
        }
        if let Some((form, _)) = &self.form {
            return div().size_full().child(form.clone());
        }
        let (t, c) = (tokens(), colors());
        let ticket = self.ticket.0.clone();
        let mut body = div()
            .id("profile-management-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(t.space.s12))
            .flex()
            .flex_col()
            .gap(px(t.space.s8));
        match &self.page {
            Page::Loading => {
                body = body.child(ui::group_note("Opening profile controls…"));
            }
            Page::Detail(profile) => {
                body = body.child(profile.item().display_name.clone()).child(profile.item().profile_id.clone()).child(ui::group_note("Service controls preserve profile data. Connecting to a workspace is a separate action."));
                if profile.item().state == State::Ready && !self.busy {
                    // Default keeps its host environment, so its runtimes are not
                    // available through this bound setup; it keeps its unbound setup.
                    if self.directory.is_some() && profile.item().kind == Kind::Isolated {
                        let expected = ticket.clone();
                        body = body.child(
                            ui::button("new-agent-setup", "Add agent", ui::Button::Primary)
                                .on_click(cx.listener(move |pane, _, window, cx| {
                                    pane.choices(&expected, window, cx)
                                })),
                        );
                    }
                    for action in [Action::Start, Action::Stop] {
                        let ticket = ticket.clone();
                        body = body.child(
                            ui::button(
                                action_label(action),
                                action_label(action),
                                if action == Action::Stop {
                                    ui::Button::Danger
                                } else {
                                    ui::Button::Secondary
                                },
                            )
                            .on_click(
                                cx.listener(move |pane, _, _, cx| pane.start(action, &ticket, cx)),
                            ),
                        );
                    }
                }
            }
            Page::Operation { key, snapshot } => {
                body = body.child(ui::group_note("Closing interrupts the local command. The provider outcome may remain unknown; closing does not confirm cancellation or rollback. Recover the original request before retrying."));
                body = body.child(format!("Operation {key}"));
                if let Some(snapshot) = snapshot {
                    body = body
                        .child(action_label(snapshot.action))
                        .child(status(snapshot))
                        .child(format!("Request {}", snapshot.request_id));
                    if let Some(identity) = &snapshot.identity {
                        body = body.child(format!(
                            "Profile {} · instance {}",
                            identity.profile_id, identity.lifecycle_id
                        ));
                    }
                }
                if !self.busy {
                    for (command, label) in [
                        (Command::Read, "Reload saved state"),
                        (Command::Refresh, "Check original receipt"),
                        (Command::Submit, "Submit prepared request"),
                        (Command::Retry, "Retry identical request"),
                    ] {
                        if command.allowed(snapshot.as_ref()) {
                            let ticket = ticket.clone();
                            body = body.child(
                                ui::button(label, label, ui::Button::Secondary).on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        pane.recover(command, &ticket, cx)
                                    }),
                                ),
                            );
                        }
                    }
                }
            }
            Page::History { ids, offset, rows } => {
                if rows.is_empty() {
                    body = body.child(ui::group_note("No saved lifecycle operations."));
                }
                for (key, result) in rows {
                    let (label, detail) = match result {
                        Ok(snapshot) => (
                            format!("{} · {}", action_label(snapshot.action), status(snapshot)),
                            format!("Request {}", snapshot.request_id),
                        ),
                        Err(error) => (message(*error).to_owned(), format!("Operation {key}")),
                    };
                    let key = key.clone();
                    let ticket = ticket.clone();
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("operation-{key}")))
                            .w_full()
                            .min_w_0()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s8))
                            .p(px(t.space.s12))
                            .rounded(px(t.radius.row))
                            .bg(c.field)
                            .border(px(t.size.hairline))
                            .border_color(c.line_strong)
                            .cursor_pointer()
                            .hover(|row| row.bg(colors().hover))
                            .child(div().w_full().whitespace_normal().child(label))
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(detail),
                            )
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                pane.open_record(&key, &ticket, cx)
                            })),
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
                            body = body.child(
                                ui::button(label, label, ui::Button::Secondary).on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        pane.history(offset, true, cx)
                                    }),
                                ),
                            );
                        }
                    }
                }
            }
        }
        if self.directory.is_some() && !self.busy {
            body = body.child(
                ui::button("saved-setup", "Saved agent setup", ui::Button::Secondary)
                    .on_click(cx.listener(|pane, _, window, cx| pane.setup(window, cx))),
            );
        }
        if self.controller.is_some() && !self.busy {
            body = body.child(
                ui::button("Create profile", "Create profile", ui::Button::Secondary)
                    .on_click(cx.listener(|pane, _, window, cx| pane.create(window, cx))),
            );
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
            .child(ui::view_header("layers", "Profile service controls"))
            .child(body)
            .child(
                ui::view_toolbar()
                    .px(px(t.space.s12))
                    .gap(px(t.space.s8))
                    .child(
                        ui::button("management-back", "Back to profiles", ui::Button::Secondary)
                            .on_click(cx.listener(|pane, _, _, cx| pane.back(cx))),
                    )
                    .when(self.controller.is_some() && !self.busy, |row| {
                        row.child(
                            ui::button(
                                "management-history",
                                "Saved operations",
                                ui::Button::Secondary,
                            )
                            .on_click(cx.listener(|pane, _, _, cx| pane.history(0, false, cx))),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests;
