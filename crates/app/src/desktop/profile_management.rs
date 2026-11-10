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
    Connection, Kind, Profile, State,
    operations::{Action, Prepared, Status},
    runtime_catalog::{Authentication, Catalog},
};
use gpui::{Context, EventEmitter, FocusHandle, SharedString, Window, div, prelude::*, px};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

mod choices;
pub(super) mod flow;
mod setup;

const PAGE_SIZE: usize = 16;
/// How many checks a sign-in opened from Settings gets, about five minutes.
pub(super) const SIGN_IN_CHECKS: usize = 150;
/// How often an open sign-in is checked. The provider's check is local and quick.
const SIGN_IN_CHECK: std::time::Duration = std::time::Duration::from_secs(2);

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
    Agent { profile_id: String, name: String },
    SignIn { profile_id: String, runtime: String },
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
    client: Client,
    /// The one profile this page is for; its lists show only that profile's requests.
    profile_id: Option<String>,
    directory: Option<PathBuf>,
    setup: Option<(gpui::Entity<setup::Pane>, gpui::Subscription)>,
    choices: Option<(gpui::Entity<choices::Form>, gpui::Subscription)>,
    controller: Option<Arc<Controller>>,
    page: Page,
    ticket: Ticket,
    busy: bool,
    error: Option<controller::Error>,
    /// The profile's tools and sign-ins, when its service answered.
    accounts: Option<Catalog>,
    /// The profile's connection, when its service answered.
    connection: Option<Connection>,
    /// What each CLI last reported about its sign-in.
    signed: BTreeMap<String, Authentication>,
    /// Sign-ins opened from here and not yet reported as finished.
    signing: BTreeSet<String>,
    watching: Option<gpui::Task<()>>,
    /// Lifecycle requests for this profile without a confirmed outcome.
    unfinished: usize,
    /// The ⋯ menu is open.
    more: bool,
    pub focus: FocusHandle,
}
impl EventEmitter<Back> for Management {}
impl EventEmitter<Open> for Management {}
impl Management {
    pub fn new(
        client: Client,
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
            controller: None,
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: true,
            error: None,
            accounts: None,
            connection: None,
            signed: BTreeMap::new(),
            signing: BTreeSet::new(),
            watching: None,
            unfinished: 0,
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
                        pane.accounts = opened.accounts;
                        pane.connection = opened.connection;
                        pane.signed = opened.signed;
                        pane.unfinished = opened.unfinished;
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

    /// Open a sign-in in the profile's workspace and show its result here.
    fn sign_in(&mut self, runtime: String, cx: &mut Context<Self>) {
        let Page::Detail(profile) = &self.page else {
            return;
        };
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let profile_id = profile.item().profile_id.clone();
        self.signing.insert(runtime.clone());
        cx.emit(Open::SignIn {
            profile_id,
            runtime,
        });
        if self.watching.is_none() {
            self.watching = Some(cx.spawn(async move |this, cx| {
                for _ in 0..SIGN_IN_CHECKS {
                    cx.background_executor().timer(SIGN_IN_CHECK).await;
                    let Ok(ids) = this.read_with(cx, |pane, _| pane.signing.clone()) else {
                        return;
                    };
                    let connection = connection.clone();
                    let read = cx
                        .background_executor()
                        .spawn(async move { read_accounts(&connection, ids.into_iter().collect()) })
                        .await;
                    let finished = this.update(cx, |pane, cx| {
                        for (id, authentication) in read {
                            if authentication == Authentication::Authenticated {
                                pane.signing.remove(&id);
                            }
                            pane.signed.insert(id, authentication);
                        }
                        cx.notify();
                        pane.signing.is_empty()
                    });
                    if finished.unwrap_or(true) {
                        break;
                    }
                }
                let _ = this.update(cx, |pane, cx| {
                    pane.signing.clear();
                    pane.watching = None;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    /// A sign-in pane for this profile closed: read that CLI once more and
    /// stop waiting for it, rather than until the checks run out.
    pub fn sign_in_ended(&mut self, profile_id: &str, runtime: &str, cx: &mut Context<Self>) {
        if self.profile_id.as_deref() != Some(profile_id) || !self.signing.contains(runtime) {
            return;
        }
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let runtime = runtime.to_owned();
        let id = runtime.clone();
        let read = cx
            .background_executor()
            .spawn(async move { read_accounts(&connection, vec![id]) });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |pane, cx| {
                pane.signed.extend(read);
                pane.signing.remove(&runtime);
                if pane.signing.is_empty() {
                    pane.watching = None;
                }
                cx.notify();
            });
        })
        .detach();
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
    fn setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.setup.is_some() || self.choices.is_some() {
            return;
        }
        let Some(directory) = self.directory.clone() else {
            return;
        };
        self.ticket.begin();
        let client = self.client.clone();
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
        let client = self.client.clone();
        self.ticket.begin();
        let signed_in = self
            .signed
            .iter()
            .find(|(_, status)| **status == Authentication::Authenticated)
            .map(|(id, _)| id.clone());
        let form = cx.new(|cx| {
            choices::Form::new(client, directory, profile, "Cancel", signed_in, window, cx)
        });
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
                let setup =
                    match event {
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
                            let Some((prepared, controller)) =
                                form.update(cx, |form, _| form.take_prepared())
                            else {
                                return;
                            };
                            Some(cx.new(|cx| {
                                setup::Pane::from_prepared(controller, prepared, "Back", cx)
                            }))
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
    accounts: Option<Catalog>,
    connection: Option<Connection>,
    signed: BTreeMap<String, Authentication>,
    unfinished: usize,
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
            None => history(controller, None, None, 0, live),
        },
        Err(error) => Err(*error),
    };
    let live = || live.load(Ordering::Acquire);
    let (mut accounts, mut unfinished) = (None, 0);
    let (mut connection, mut signed) = (None, BTreeMap::new());
    if let (Ok(Page::Detail(profile)), Ok(controller)) = (&page, &controller) {
        let id = &profile.item().profile_id;
        unfinished = controller
            .ids()
            .unwrap_or_default()
            .iter()
            .filter_map(|key| controller.read(key).ok())
            .filter(|s| {
                s.terminal.is_none() && s.identity.as_ref().is_some_and(|i| &i.profile_id == id)
            })
            .count();
        accounts = profile
            .expected_identity()
            .ok()
            .and_then(|identity| client.runtimes(&identity, &live).ok());
        if profile.item().kind == Kind::Isolated
            && let (Some(catalog), Ok(verified)) = (&accounts, profile.connect())
        {
            let ids = catalog
                .items()
                .iter()
                .filter(|runtime| runtime.available && runtime.id != "shell")
                .map(|runtime| runtime.id.clone())
                .collect();
            signed = read_accounts(&verified, ids).into_iter().collect();
            connection = Some(verified);
        }
    }
    Ok(Opened {
        directory,
        controller: controller.ok(),
        page,
        accounts,
        connection,
        signed,
        unfinished,
    })
}

fn history(
    controller: &Controller,
    profile: Option<&str>,
    ids: Option<Vec<String>>,
    offset: usize,
    live: &AtomicBool,
) -> Result<Page, controller::Error> {
    let ids = match ids {
        Some(ids) => ids,
        None => controller
            .ids()?
            .into_iter()
            .filter(|key| {
                // A record that can't be read may be this profile's, so it stays.
                profile.is_none_or(|id| match controller.read(key) {
                    Ok(s) => s.identity.as_ref().is_some_and(|i| i.profile_id == id),
                    Err(_) => true,
                })
            })
            .collect(),
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
        Action::Start => "Start profile",
        Action::Stop => "Stop profile",
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
                        .child(ui::profile_mark(
                            &item.profile_id,
                            &item.display_name,
                            item.kind == Kind::LegacyDefault,
                            t.size.slot,
                        ))
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
                                .child(meta(match (&item.state, &item.kind) {
                                    (State::Creating, _) => "Being created".into(),
                                    (State::Failed, _) => "Creation didn't finish".into(),
                                    (_, Kind::LegacyDefault) => "Uses your own sign-ins".into(),
                                    _ => "Its own accounts, agents, skills and policies".into(),
                                })),
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
                    if ready {
                        let ticket = ticket.clone();
                        menu = menu.child(
                            ui::menu_item("management-start", None, "Start profile", None)
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    pane.more = false;
                                    pane.start(Action::Start, &ticket, cx)
                                })),
                        );
                    }
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
                if self.unfinished > 0 {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s10))
                            .p(px(t.space.s12))
                            .rounded(px(t.radius.popover))
                            .bg(c.attention_soft)
                            .child(ui::step_dot(ui::Step::Waits))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_color(c.text).child(
                                        if self.unfinished == 1 {
                                            "A request for this profile didn't finish".to_owned()
                                        } else {
                                            format!(
                                                "{} requests for this profile didn't finish",
                                                self.unfinished
                                            )
                                        },
                                    ))
                                    .child(meta(
                                        "It may already have worked. Check it to be sure.".into(),
                                    )),
                            )
                            .child(
                                ui::button("management-check", "Check", ui::Button::Secondary)
                                    .on_click(
                                        cx.listener(|pane, _, _, cx| pane.history(0, false, cx)),
                                    ),
                            ),
                    );
                }
                body = body.child(ui::group_title("Accounts", ""));
                match &self.accounts {
                    None => {
                        body = body.child(ui::group_note("Start the profile to see its accounts."))
                    }
                    Some(catalog) => {
                        let mut card = ui::group_card();
                        for (index, runtime) in catalog
                            .items()
                            .iter()
                            .filter(|r| r.id != "shell")
                            .enumerate()
                        {
                            let waiting = self.signing.contains(&runtime.id);
                            let signed = self.signed.get(&runtime.id).copied();
                            let state = if !runtime.available {
                                "Not installed"
                            } else if waiting {
                                "Waiting for sign-in"
                            } else {
                                match signed.unwrap_or(runtime.authentication) {
                                    Authentication::Authenticated => "Signed in",
                                    Authentication::NotAuthenticated => "Not signed in",
                                    Authentication::NotApplicable => "No account needed",
                                    _ => "Couldn't check",
                                }
                            };
                            let can_sign_in = runtime.available
                                && item.kind == Kind::Isolated
                                && signed != Some(Authentication::Authenticated);
                            let id = runtime.id.clone();
                            card = card.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(t.space.s10))
                                    .h(px(t.size.group_row))
                                    .px(px(t.space.s14))
                                    .when(index > 0, |el| {
                                        el.border_t(px(t.size.hairline)).border_color(c.line)
                                    })
                                    .when(!runtime.available, |el| el.opacity(0.6))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_color(c.text)
                                            .child(runtime.display_name.clone()),
                                    )
                                    .child(meta(state.into()))
                                    .when(can_sign_in, |el| {
                                        el.child(
                                            ui::button(
                                                SharedString::from(format!("account-{id}")),
                                                if waiting { "Sign in again" } else { "Sign in" },
                                                ui::Button::Secondary,
                                            )
                                            .on_click(
                                                cx.listener(move |pane, _, _, cx| {
                                                    pane.sign_in(id.clone(), cx)
                                                }),
                                            ),
                                        )
                                    }),
                            );
                        }
                        body = body.child(card);
                    }
                }
                if ready
                    && (item.kind == Kind::LegacyDefault
                        || (self.directory.is_some() && item.kind == Kind::Isolated))
                {
                    let expected = ticket.clone();
                    let default = item.kind == Kind::LegacyDefault;
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(ui::group_title("Agents", ""))
                            .child(
                                ui::button("new-agent-setup", "Add agent", ui::Button::Secondary)
                                    .on_click(cx.listener(move |pane, _, window, cx| {
                                        // Default keeps its host environment, so the provider refuses its
                                        // bound setup; a new profile, with its own guide, is the way on.
                                        if default {
                                            cx.defer(super::new_profile);
                                        } else {
                                            pane.choices(&expected, window, cx)
                                        }
                                    })),
                            ),
                    );
                    if let Some((form, _)) = &self.choices {
                        body = body.child(form.clone());
                    }
                }
                if ready {
                    let ticket = ticket.clone();
                    body = body.child(
                        div().flex().items_center().gap(px(t.space.s12)).pt(px(t.space.s20))
                            .child(div().flex_1().min_w_0().child(meta("Stopping keeps everything. Start it again from ⋯ or one of its workspaces.".into())))
                            .child(ui::button("Stop service", "Stop profile", ui::Button::Secondary)
                                .flex_shrink_0()
                                .on_click(cx.listener(move |pane, _, _, cx| pane.start(Action::Stop, &ticket, cx)))),
                    );
                }
            }
            Page::Operation { snapshot, .. } => {
                body = body.child(ui::group_title(
                    snapshot
                        .as_ref()
                        .map_or("Request", |s| action_label(s.action)),
                    "Leaving doesn't stop it. It finishes on its own.",
                ));
                if let Some(snapshot) = snapshot {
                    body = body.child(meta(status(snapshot).into()));
                }
                if !self.busy {
                    let mut row = div().flex().flex_wrap().gap(px(t.space.s8));
                    for (command, label) in [
                        (Command::Refresh, "Check again"),
                        (Command::Submit, "Send it"),
                        (Command::Retry, "Try the same request again"),
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
                    body = body.child(row);
                }
            }
            Page::History { ids, offset, rows } => {
                body = body.child(ui::group_title(
                    "Saved requests",
                    "Creating, starting and stopping profiles.",
                ));
                if rows.is_empty() {
                    body = body.child(ui::group_note("No saved requests."));
                }
                let mut card = ui::group_card();
                for (index, (key, result)) in rows.iter().enumerate() {
                    let (label, detail) = match result {
                        Ok(snapshot) => (
                            action_label(snapshot.action).to_owned(),
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
                        (offset.checked_sub(PAGE_SIZE), "Previous"),
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
