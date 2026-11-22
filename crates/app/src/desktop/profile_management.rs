//! A profile's page in Settings. Provider receipts never activate a workspace.

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
    Connection, Item, Profile, State,
    operations::{Action, Status},
    runtime_catalog::Authentication,
};
use gpui::{Context, EventEmitter, FocusHandle, SharedString, Window, div, prelude::*, px};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

mod accounts;
mod choices;
pub(super) mod flow;
mod secrets;
mod setup;
mod sign_in;

const PAGE_SIZE: usize = 16;

/// Each CLI's sign-in, one request each. An unanswered read can't be told from
/// a signed-out CLI, so it's reported as unavailable. Runs on an I/O worker.
fn read_accounts(connection: &Connection, ids: Vec<String>) -> Vec<(String, Authentication)> {
    ids.into_iter()
        .map(|id| {
            let status = connection
                .account_status(&id)
                .unwrap_or(Authentication::Unavailable);
            (id, status)
        })
        .collect()
}

pub(super) struct Back;

/// A session setup leaves waiting for the person, to open in a workspace.
#[derive(Clone)]
pub(super) enum Open {
    Agent {
        profile_id: String,
        name: String,
    },
    SignIn {
        profile_id: String,
        runtime: String,
    },
    /// A profile shell to show, by its session on the profile's tmux server.
    Shell {
        profile_id: String,
        session: String,
    },
}

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
    client: Option<Client>,
    /// The one profile this page is for; unfinished creates can lack a profile ID.
    profile_id: Option<String>,
    directory: Option<PathBuf>,
    setup: Option<(gpui::Entity<setup::Pane>, gpui::Subscription)>,
    choices: Option<(gpui::Entity<choices::Form>, gpui::Subscription)>,
    /// A shell in a profile on the Rust service, with Git's commands to run there.
    sign_in: Option<(gpui::Entity<sign_in::Pane>, gpui::Subscription)>,
    /// The secrets of a profile on the Rust service.
    secrets: Option<gpui::Entity<secrets::Section>>,
    /// The CLI sign-ins of a profile on the Rust service.
    logins: Option<(gpui::Entity<accounts::Section>, gpui::Subscription)>,
    controller: Option<Arc<Controller>>,
    page: Page,
    ticket: Ticket,
    busy: bool,
    error: Option<controller::Error>,
    /// The ⋯ menu is open.
    more: bool,
    pub focus: FocusHandle,
}
impl EventEmitter<Back> for Management {}
impl EventEmitter<Open> for Management {}
impl Management {
    pub fn new(
        client: Option<Client>,
        directory: Option<PathBuf>,
        profile: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            client: client.clone(),
            profile_id: profile.clone(),
            directory: None,
            setup: None,
            choices: None,
            sign_in: None,
            secrets: None,
            logins: None,
            controller: None,
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: true,
            error: None,
            more: false,
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
                        pane.served_panes(cx);
                    }
                    Err(error) => pane.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        pane
    }

    /// A sign-in pane for this profile closed: the Sign-in section reads that
    /// CLI again.
    pub fn sign_in_ended(&mut self, profile_id: &str, runtime: &str, cx: &mut Context<Self>) {
        if self.profile_id.as_deref() != Some(profile_id) {
            return;
        }
        if let Some((logins, _)) = &self.logins {
            logins.update(cx, |section, cx| section.sign_in_ended(runtime, cx));
        }
    }

    /// A profile's Sign-in and Secrets panes, which only a serving profile has.
    fn served_panes(&mut self, cx: &mut Context<Self>) {
        let Page::Detail(profile) = &self.page else {
            return;
        };
        if !has_served_panes(profile.item()) {
            return;
        }
        let logins = cx.new(|cx| accounts::Section::new(profile.clone(), cx));
        let subscription = cx.subscribe(&logins, |_, _, open: &Open, cx| cx.emit(open.clone()));
        self.logins = Some((logins, subscription));
        let pane = cx.new(|_| sign_in::Pane::new(profile.clone()));
        let subscription = cx.subscribe(&pane, |_, _, open: &Open, cx| cx.emit(open.clone()));
        self.sign_in = Some((pane, subscription));
        let profile = profile.clone();
        self.secrets = Some(cx.new(|cx| secrets::Section::new(profile, cx)));
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
    pub(super) fn leave(&mut self) {
        self.ticket.cancel();
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        self.ticket.cancel();
        cx.emit(Back);
    }
    fn setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.setup.is_some() || self.choices.is_some() {
            return;
        }
        let Some(directory) = self.directory.clone() else {
            return;
        };
        let Some(client) = self.client.clone() else {
            self.error = Some(controller::Error::Client(
                crate::profiles::Error::Unavailable,
            ));
            cx.notify();
            return;
        };
        self.ticket.begin();
        let profile = self.profile_id.clone();
        let setup = cx.new(|cx| setup::Pane::new(client, directory, profile, cx));
        self.attach_setup(setup, window, cx);
    }
    fn attach_setup(
        &mut self,
        setup: gpui::Entity<setup::Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&setup.read(cx).focus.clone(), cx);
        cx.subscribe(&setup, |_, _, open: &Open, cx| cx.emit(open.clone()))
            .detach();
        cx.subscribe(&setup, |pane, _, _: &setup::Again, cx| {
            pane.setup = None;
            cx.notify();
        })
        .detach();
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
        let Some(client) = self.client.clone() else {
            return;
        };
        self.ticket.begin();
        let form =
            cx.new(|cx| choices::Form::new(client, directory, profile, "Cancel", window, cx));
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
                    // Only a guide opens its own pane; this form adds agents.
                    choices::Event::Back | choices::Event::Opened(_) => None,
                    choices::Event::Recover => {
                        let Some(controller) = form.read(cx).controller() else {
                            return;
                        };
                        let profile = Some(form.read(cx).profile_id().to_owned());
                        Some(cx.new(|cx| setup::Pane::from_controller(controller, profile, cx)))
                    }
                    choices::Event::Submit => {
                        let Some(request) = form.update(cx, |form, _| form.take_served()) else {
                            return;
                        };
                        Some(cx.new(|cx| setup::Pane::served(request, "Back", cx)))
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
        let profile = self.profile_id.clone();
        let work = cx.background_executor().spawn(async move {
            command.run(
                &controller,
                &key,
                profile.as_deref(),
                &|| worker.load(Ordering::Acquire),
                super::onboarding::probe::profile_client,
            )
        });
        self.observe_operation(ticket, work, cx);
    }
    /// Forget the open record when it can't be read; nothing is sent.
    fn forget(&mut self, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.ticket.accepts(expected) {
            return;
        }
        let Page::Operation { key, .. } = &self.page else {
            return;
        };
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let key = key.clone();
        self.busy = true;
        self.error = None;
        let ticket = self.ticket.begin();
        let work = cx
            .background_executor()
            .spawn(async move { controller.forget_unreadable(&key) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if !pane.ticket.accepts(&ticket) {
                    return;
                }
                pane.busy = false;
                match result {
                    Ok(()) => pane.history(0, false, cx),
                    Err(error) => pane.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn history(&mut self, offset: usize, retain_ids: bool, cx: &mut Context<Self>) {
        if self.busy || self.setup.is_some() || self.choices.is_some() {
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
        let profile = self.profile_id.clone();
        let work = cx
            .background_executor()
            .spawn(async move { history(&controller, profile.as_deref(), ids, offset, &worker) });
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
    Retry,
}
impl Command {
    fn run(
        self,
        controller: &Controller,
        key: &str,
        profile: Option<&str>,
        live: &dyn Fn() -> bool,
        resolve: impl FnOnce() -> Option<Client>,
    ) -> (
        Result<Snapshot, controller::Error>,
        Option<controller::Error>,
    ) {
        let read = || scoped_record(controller, key, profile);
        let snapshot = read();
        if matches!(self, Self::Read) || snapshot.is_err() {
            return (snapshot, None);
        }
        let result = controller
            .ensure_provider(resolve)
            .and_then(|()| match self {
                Self::Submit => controller.submit(key, live),
                Self::Retry => controller.retry(key, live),
                Self::Read => unreachable!(),
            });
        (read(), result.err())
    }

    fn allowed(self, snapshot: Option<&Snapshot>) -> bool {
        if matches!(self, Self::Read) {
            return true;
        }
        snapshot.is_some_and(|snapshot| {
            snapshot.terminal.is_none()
                && match self {
                    Self::Read => true,
                    Self::Submit => snapshot.phase == Phase::Prepared,
                    Self::Retry => snapshot.phase == Phase::Submitted,
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
    client: Option<Client>,
    directory: Option<PathBuf>,
    profile: Option<String>,
    live: &AtomicBool,
) -> Result<Opened, controller::Error> {
    let directory = crate::library::location::library_directory(directory.as_deref())
        .map_err(|_| controller::Error::Storage(journal::Error::Unavailable))?;
    // Keep the resolved library location even if this independent journal is
    // unsafe. Setup recovery must still enforce and use its own journal checks.
    let controller = journal::Journal::open(&directory.join("profile-operations"))
        .map(|journal| Arc::new(Controller::for_history(journal, client.as_ref())))
        .map_err(controller::Error::Storage);
    let page = match &controller {
        Ok(controller) => match profile {
            Some(id) => client
                .as_ref()
                .ok_or(controller::Error::Client(
                    crate::profiles::Error::Unavailable,
                ))?
                .show(&id, &|| live.load(Ordering::Acquire))
                .map(Page::Detail)
                .map_err(controller::Error::Client),
            None => history(controller, None, None, 0, live),
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
    profile: Option<&str>,
    ids: Option<Vec<String>>,
    offset: usize,
    live: &AtomicBool,
) -> Result<Page, controller::Error> {
    let cached = ids.is_some();
    let mut ids = match ids {
        Some(ids) => ids,
        None => controller.ids()?,
    };
    // The requested offset belongs to the previously drawn list. Keep its
    // first surviving unseen row when a request before it changes scope.
    let mut removed_before = 0;
    if profile.is_some() {
        let mut position = 0;
        ids.retain(|key| {
            let keep = !matches!(
                scoped_record(controller, key, profile),
                Err(controller::Error::Missing)
            );
            if cached && !keep && position < offset {
                removed_before += 1;
            }
            position += 1;
            keep
        });
    }
    let offset = offset.saturating_sub(removed_before);
    let offset = if offset >= ids.len() {
        ids.len().saturating_sub(1) / PAGE_SIZE * PAGE_SIZE
    } else {
        offset
    };
    let mut rows = Vec::new();
    for key in ids.iter().skip(offset).take(PAGE_SIZE) {
        if !live.load(Ordering::Acquire) {
            return Err(controller::Error::Client(crate::profiles::Error::Cancelled));
        }
        let record = scoped_record(controller, key, profile);
        if !matches!(record, Err(controller::Error::Missing)) {
            rows.push((key.clone(), record));
        }
    }
    Ok(Page::History { ids, offset, rows })
}
fn scoped_record(
    controller: &Controller,
    key: &str,
    profile: Option<&str>,
) -> Result<Snapshot, controller::Error> {
    let saved = controller.read(key)?;
    if profile.is_some_and(|id| match &saved.identity {
        Some(identity) => identity.profile_id != id,
        None => saved.terminal.is_some(),
    }) {
        return Err(controller::Error::Missing);
    }
    Ok(saved)
}
fn action_label(action: Action) -> &'static str {
    match action {
        Action::Create => "Create profile",
    }
}
fn status(snapshot: &Snapshot) -> &'static str {
    match snapshot.terminal.as_ref().map(|receipt| receipt.status) {
        Some(Status::Succeeded) => "Succeeded",
        Some(Status::Failed) => "Failed",
        None if snapshot.phase == Phase::Prepared => "Prepared, not submitted",
        None => "Outcome not yet confirmed",
    }
}
/// The line under a profile's name. A failed profile shows the service's own
/// sentence, as given.
fn state_line(item: &Item) -> SharedString {
    match (&item.state, &item.reason) {
        (State::Failed, Some(reason)) => reason.clone().into(),
        (State::Failed, None) => "Creation didn't finish".into(),
        (State::Ready, _) => "Its own agents, skills and policies".into(),
    }
}

/// Only a profile the service serves has Sign-in, a shell and Secrets.
fn has_served_panes(item: &Item) -> bool {
    item.state == State::Ready
}

/// Whether a saved record can't be read, so it can only be forgotten.
fn unreadable(error: controller::Error) -> bool {
    matches!(
        error,
        controller::Error::Invalid
            | controller::Error::Storage(journal::Error::Invalid | journal::Error::TooLarge)
    )
}

fn message(error: controller::Error) -> &'static str {
    use controller::Error::*;
    match error {
        Missing => "This operation record is missing. No request was sent by this read.",
        Invalid | Storage(journal::Error::Invalid) => {
            "This saved request could not be read. Forget it to continue."
        }
        WrongPhase | Completed => "The operation changed. Reload its saved state.",
        Client(_) => {
            "The service's answer could not be confirmed. Keep this request and try the same request again."
        }
        Storage(journal::Error::Unsafe) => "Operation storage is not private or safe to use.",
        Storage(journal::Error::Busy | journal::Error::Conflict) => {
            "Another operation update won. Reload the saved state."
        }
        // Too large to read, or too large to save: both can be forgotten.
        Storage(journal::Error::TooLarge) => {
            "This saved request is too large, or request storage is full. Forget it, or other requests, to continue."
        }
        Storage(_) => "Operation storage could not be confirmed. Reload before retrying.",
    }
}

impl Render for Management {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((setup, _)) = &self.setup {
            return div()
                .id("profile-management-setup")
                .size_full()
                .child(setup.clone());
        }
        let (t, c) = (tokens(), colors());
        let ticket = self.ticket.0.clone();
        let meta =
            |text: SharedString| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let mut body = div().flex().flex_col().gap(px(t.space.s8)).child(
            div().flex().child(
                ui::back_button("management-back", "Profiles")
                    .on_click(cx.listener(|pane, _, _, cx| pane.back(cx))),
            ),
        );
        match &self.page {
            Page::Loading => body = body.child(meta("Opening…".into())),
            Page::Detail(profile) => {
                let item = profile.item();
                let ready = item.state == State::Ready && !self.busy;
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s12))
                        .py(px(t.space.s8))
                        .child(ui::profile_mark(&item.display_name, t.size.slot))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    ui::text(div().text_color(c.text), &t.text.group_title)
                                        .child(item.display_name.clone()),
                                )
                                .child(meta(state_line(item)))
                                .children(
                                    super::profile_picker::whose_accounts(item.accounts)
                                        .map(|words| meta(words.into())),
                                ),
                        )
                        .child(ui::icon_button("management-more", "ellipsis").on_click(
                            cx.listener(|pane, _, _, cx| {
                                pane.more = !pane.more;
                                cx.notify();
                            }),
                        )),
                );
                if self.more && !self.busy {
                    let mut menu = ui::menu();
                    if self.controller.is_some() {
                        menu = menu.child(
                            ui::menu_item("management-history", None, "Saved requests", None)
                                .on_click(cx.listener(|pane, _, _, cx| {
                                    pane.more = false;
                                    pane.history(0, false, cx)
                                })),
                        );
                    }
                    if self.directory.is_some() {
                        menu = menu.child(
                            ui::menu_item("saved-setup", None, "Earlier agent setups", None)
                                .on_click(cx.listener(|pane, _, window, cx| {
                                    pane.more = false;
                                    pane.setup(window, cx)
                                })),
                        );
                    }
                    body = body.child(div().flex().justify_end().child(menu));
                }
                if self.logins.is_some() {
                    body = body.child(ui::group_title("Sign-in", ""));
                }
                if let Some((section, _)) = &self.logins {
                    body = body.child(section.clone());
                }
                if let Some((pane, _)) = &self.sign_in {
                    body = body.child(pane.clone());
                }
                if let Some(section) = &self.secrets {
                    body = body.child(section.clone());
                }
                if ready {
                    let expected = ticket.clone();
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(ui::group_title("Agents", ""))
                            .child(
                                ui::button("new-agent-setup", "Add agent", ui::Button::Secondary)
                                    .on_click(cx.listener(move |pane, _, window, cx| {
                                        pane.choices(&expected, window, cx)
                                    })),
                            ),
                    );
                    if let Some((form, _)) = &self.choices {
                        body = body.child(form.clone());
                    }
                }
            }
            Page::Operation { snapshot, .. } => {
                body = body.child(ui::group_title(
                    snapshot
                        .as_ref()
                        .map_or("Request", |s| action_label(s.action)),
                    "Leaving stops waiting for it. Try the same request again to finish it.",
                ));
                if let Some(snapshot) = snapshot {
                    body = body
                        .child(meta(status(snapshot).into()))
                        .child(meta(format!("Request ID {}", snapshot.request_id).into()));
                }
                if !self.busy {
                    let mut row = div().flex().flex_wrap().gap(px(t.space.s8));
                    for (command, label) in [
                        (Command::Retry, "Try the same request again"),
                        (Command::Submit, "Send it"),
                        (Command::Read, "Reload"),
                    ] {
                        if command.allowed(snapshot.as_ref()) {
                            let ticket = ticket.clone();
                            row = row.child(
                                ui::button(label, label, ui::Button::Secondary).on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        pane.recover(command, &ticket, cx)
                                    }),
                                ),
                            );
                        }
                    }
                    // A record that can't be read can only be forgotten.
                    if snapshot.is_none() && self.error.is_some_and(unreadable) {
                        let ticket = ticket.clone();
                        row = row.child(
                            ui::button(
                                "operation-forget",
                                "Forget this request",
                                ui::Button::Secondary,
                            )
                            .on_click(cx.listener(move |pane, _, _, cx| pane.forget(&ticket, cx))),
                        );
                    }
                    body = body.child(row);
                }
            }
            Page::History { ids, offset, rows } => {
                body = body.child(ui::group_title(
                    "Saved requests",
                    "Requests to create a profile. Unfinished requests can have no assigned profile yet.",
                ));
                if rows.is_empty() {
                    body = body.child(ui::group_note("No saved requests."));
                }
                let mut card = ui::group_card();
                for (index, (key, result)) in rows.iter().enumerate() {
                    let (label, detail) = match result {
                        Ok(snapshot) => (
                            snapshot
                                .display_name
                                .clone()
                                .unwrap_or_else(|| action_label(snapshot.action).into()),
                            status(snapshot).to_owned(),
                        ),
                        Err(error) => (message(*error).to_owned(), String::new()),
                    };
                    let key = key.clone();
                    let ticket = ticket.clone();
                    card = card.child(
                        ui::group_row(
                            SharedString::from(format!("operation-{key}")),
                            index == 0,
                            None,
                            label,
                            detail,
                        )
                        .on_click(
                            cx.listener(move |pane, _, _, cx| pane.open_record(&key, &ticket, cx)),
                        ),
                    );
                }
                if !rows.is_empty() {
                    body = body.child(card);
                }
                if !self.busy {
                    let mut pages = div().flex().gap(px(t.space.s8));
                    for (offset, label) in [
                        (
                            (*offset > 0).then(|| offset.saturating_sub(PAGE_SIZE)),
                            "Previous",
                        ),
                        (
                            (offset + PAGE_SIZE < ids.len()).then_some(offset + PAGE_SIZE),
                            "Next",
                        ),
                    ] {
                        if let Some(offset) = offset {
                            pages = pages.child(
                                ui::button(label, label, ui::Button::Secondary).on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        pane.history(offset, true, cx)
                                    }),
                                ),
                            );
                        }
                    }
                    body = body.child(pages);
                }
            }
        }
        if self.busy && !matches!(self.page, Page::Loading) {
            body = body.child(meta("Working…".into()));
        }
        if let Some(error) = self.error {
            body = body.child(
                ui::text(div().text_color(c.attention_text), &t.text.meta).child(message(error)),
            );
        }
        div()
            .id("profile-management-body")
            .size_full()
            .overflow_y_scroll()
            .pb(px(t.space.s24))
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|pane, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && pane.choices.is_none() {
                    pane.back(cx);
                    cx.stop_propagation();
                }
            }))
            .child(body)
    }
}

#[cfg(test)]
mod tests;
