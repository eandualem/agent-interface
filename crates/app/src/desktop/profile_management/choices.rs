//! Explicit first-agent choices in one captured profile and operator context.
//! As a profile's guide, the same choices are the second first-run screen:
//! a name, the tool it runs in, Start, and the rest under More options.
use super::{Open, Ticket, setup, ui};
use crate::{
    desktop::{
        design::{colors, tokens},
        name::{NameEvent, NameField},
        onboarding::{
            install::{self, home_relative},
            probe,
        },
    },
    profiles::{Client, journal, setup_controller::Controller},
};
use agent_interface_roster::profiles::{
    Connection, Profile,
    runtime_catalog::{Authentication, Catalog, Runtime},
    setup::{
        Agent, Brief, Prepared,
        start::{Start, repository},
    },
};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, PathPromptOptions, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

pub(super) enum Event {
    Back,
    Recover,
    Submit,
    /// A guide is up: open its pane.
    Opened(Open),
}
enum State {
    Loading,
    /// Saved requests could not be checked, so new choices wait for recovery.
    Unchecked(Arc<Controller>),
    Ready {
        /// The setup journal, which a guide's start is saved in; Add agent
        /// starts with the service's own start and opens none.
        controller: Option<Arc<Controller>>,
        catalog: Catalog,
        /// Open first-agent requests of this profile: while there is one, a
        /// guide makes no new request.
        pending: usize,
    },
    Failed,
}
pub(super) struct Form {
    client: Client,
    library: PathBuf,
    profile: Profile,
    state: State,
    name: Entity<NameField>,
    brief: Entity<NameField>,
    custom_brief: bool,
    /// A repository the agent watches, as `owner/name`, under the Rust service.
    repository: Entity<NameField>,
    _fields: Vec<Subscription>,
    project: Option<PathBuf>,
    runtime: Option<String>,
    model: Option<String>,
    greeting: bool,
    /// What Submit asks the Rust service to start.
    served: Option<setup::Request>,
    error: Option<&'static str>,
    ticket: Ticket,
    finished: bool,
    /// The name last filled in from the folder, kept until the person edits it.
    folder_name: Option<String>,
    /// Model, instructions and greeting are open.
    more: bool,
    /// What leaving the form is called where it is shown.
    skip: &'static str,
    /// The tool to start on, when one is known to be signed in.
    prefer: Option<String>,
    /// This is a profile's first agent: its guide, which starts named and
    /// with a folder made for it, both shown and editable in place.
    guide: bool,
    /// The guide is part of first run, which has nothing to go back to.
    first: bool,
    /// Agent tools being installed or updated from their cards: none while it
    /// runs, or why it failed.
    installing: BTreeMap<String, Option<String>>,
    /// Select this tool once the catalog shows it ready, after an install.
    select: Option<String>,
    /// The profile's verified connection, to read which tools are signed in.
    connection: Option<Connection>,
    /// What each tool last reported about its sign-in, read once.
    accounts: BTreeMap<String, Authentication>,
    /// The guide's start, while it runs or after it stopped.
    start: Option<(Entity<setup::Pane>, Vec<Subscription>)>,
    /// The revision of the start's saved request after which the tools and
    /// open setups were last read again.
    refreshed: Option<u64>,
    /// Something was changed since the start stopped, so Try again sends the
    /// new choices rather than the same request.
    changed: bool,
    /// The person picked a tool. Unlike `changed`, a start never clears it,
    /// so a later account read never swaps it for a signed-in one.
    picked: bool,
    pub focus: FocusHandle,
}
impl EventEmitter<Event> for Form {}
impl Form {
    pub fn new(
        client: Client,
        library: PathBuf,
        profile: Profile,
        skip: &'static str,
        prefer: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| NameField::filter(window, cx));
        let brief = cx.new(|cx| NameField::feedback(window, cx));
        let repository = cx.new(|cx| NameField::bare("", window, cx));
        let fields = [&name, &brief, &repository]
            .into_iter()
            .map(|field| {
                // Only an edit changes the request; a click or caret move doesn't.
                cx.subscribe(field, |form: &mut Self, _, event: &NameEvent, cx| {
                    if let NameEvent::Changed(_) = event {
                        form.changed = true;
                        cx.notify()
                    }
                })
            })
            .collect();
        let mut form = Self {
            client,
            library,
            profile,
            state: State::Loading,
            name,
            brief,
            custom_brief: false,
            repository,
            _fields: fields,
            project: None,
            runtime: None,
            model: None,
            greeting: true,
            served: None,
            error: None,
            ticket: Ticket::default(),
            finished: false,
            folder_name: None,
            more: false,
            skip,
            prefer,
            guide: false,
            first: false,
            installing: BTreeMap::new(),
            select: None,
            connection: None,
            accounts: BTreeMap::new(),
            start: None,
            refreshed: None,
            changed: false,
            picked: false,
            focus: cx.focus_handle(),
        };
        form.load(false, cx);
        form
    }
    /// Read the profile's tools and open setups. `keep` leaves what is shown,
    /// and the chosen tool, in place until the new read arrives.
    fn load(&mut self, keep: bool, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        // Setup schema 3 is the Rust service's: an earlier provider takes none.
        if !self.profile.served() {
            self.state = State::Failed;
            self.error = Some(
                "This profile runs on an earlier Agent Backbone service, which this app no longer adds agents to.",
            );
            return;
        }
        if !keep {
            self.state = State::Loading;
            self.runtime = None;
            self.model = None;
        }
        self.error = None;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let client = self.client.clone();
        let directory = self.library.clone();
        let expected = self.profile.expected_identity();
        let served = self.profile.served();
        let setup = self.guide;
        let work = cx.background_executor().spawn(async move {
            let identity = expected.map_err(|_| "This profile has no ready instance for setup.")?;
            load_choices(client, directory, &identity, served, setup, &|| {
                worker.load(Ordering::Acquire)
            })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |form, cx| {
                if form.finished || !form.ticket.accepts(&ticket) {
                    return;
                }
                match result {
                    Ok(state) => {
                        // Start on a tool that's ready to work: one just
                        // installed, else the one asked for, else a signed-in
                        // one, else any that can run, else none.
                        if let State::Ready { catalog, .. } = &state {
                            let setup = form.guide;
                            let ready = |id: &String| {
                                catalog
                                    .items()
                                    .iter()
                                    .any(|r| &r.id == id && eligible(r, setup))
                            };
                            let before = form.runtime.clone();
                            let kept = before.clone().filter(ready);
                            form.runtime = form
                                .select
                                .take()
                                .filter(ready)
                                .or(kept)
                                .or_else(|| form.prefer.clone().filter(ready))
                                .or_else(|| first_choice(catalog, setup));
                            // After a start, another tool is another request:
                            // Try again sends what the screen now shows.
                            if form.start.is_some() && form.runtime != before {
                                form.changed = true;
                            }
                            if form.model.as_ref().is_some_and(|model| {
                                !catalog.items().iter().any(|r| {
                                    Some(&r.id) == form.runtime.as_ref() && r.models.contains(model)
                                })
                            }) {
                                form.model = None;
                            }
                        }
                        form.state = state;
                        form.read_accounts(cx);
                    }
                    Err(error) => {
                        // A refresh that fails keeps what was read before, and
                        // with it the way to an earlier, open request.
                        if !(keep && matches!(form.state, State::Ready { .. })) {
                            form.state = State::Failed;
                        }
                        form.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub fn controller(&self) -> Option<Arc<Controller>> {
        match &self.state {
            State::Unchecked(controller) => Some(controller.clone()),
            State::Ready { controller, .. } => controller.clone(),
            _ => None,
        }
    }
    /// What Submit asks the Rust service to start, for `setup::Pane::served`.
    pub fn take_served(&mut self) -> Option<setup::Request> {
        self.served.take()
    }
    fn runtime(&self) -> Option<&Runtime> {
        let State::Ready { catalog, .. } = &self.state else {
            return None;
        };
        catalog
            .items()
            .iter()
            .find(|r| Some(&r.id) == self.runtime.as_ref() && eligible(r, self.guide))
    }
    /// Make this a profile's first agent, its guide: named "guide", with a
    /// folder of its own, both shown and editable in place. `first` is set
    /// during first run, where there is nothing to go back to.
    pub fn as_guide(
        &mut self,
        first: bool,
        connection: Option<Connection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.guide = true;
        self.first = first;
        self.connection = connection;
        // Read again as a guide's: its start is saved before it runs.
        self.load(false, cx);
        self.name = cx.new(|cx| NameField::bare(GUIDE, window, cx));
        self._fields.push(
            cx.subscribe(&self.name, |form: &mut Self, _, event: &NameEvent, cx| {
                if let NameEvent::Changed(_) = event {
                    form.changed = true;
                    cx.notify()
                }
            }),
        );
    }

    /// Read which installed tools are signed in, once each, so a guide's
    /// cards can say so. On the earlier provider each read starts the tool's
    /// own check; the Rust service reads only whether a login is stored.
    fn read_accounts(&mut self, cx: &mut Context<Self>) {
        let (Some(connection), State::Ready { catalog, .. }) =
            (self.connection.clone(), &self.state)
        else {
            return;
        };
        let ids: Vec<String> = catalog
            .items()
            .iter()
            .filter(|r| r.available && r.id != "shell" && !self.accounts.contains_key(&r.id))
            .map(|r| r.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let work = cx
            .background_executor()
            .spawn(async move { super::read_accounts(&connection, ids) });
        cx.spawn(async move |this, cx| {
            let read = work.await;
            let _ = this.update(cx, |form, cx| {
                form.accounts.extend(read);
                // Prefer a signed-in tool while the person hasn't chosen and
                // nothing has started, so Try again matches what is shown.
                if !form.changed
                    && !form.picked
                    && form.start.is_none()
                    && let State::Ready { catalog, .. } = &form.state
                    && let Some(id) = catalog
                        .items()
                        .iter()
                        .find(|r| {
                            eligible(r, form.guide)
                                && form.accounts.get(&r.id) == Some(&Authentication::Authenticated)
                        })
                        .map(|r| r.id.clone())
                {
                    form.runtime = Some(id);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The guide's name field, for focus on arrival.
    pub fn name_focus(&self, cx: &gpui::App) -> FocusHandle {
        self.name.read(cx).focus.clone()
    }

    /// The guide is starting: its choices wait.
    pub fn busy(&self, cx: &gpui::App) -> bool {
        self.start
            .as_ref()
            .is_some_and(|(pane, _)| pane.read(cx).starting())
    }

    /// Choose the previous or next tool that can run.
    fn step_tool(&mut self, step: isize, cx: &mut Context<Self>) {
        let State::Ready { catalog, .. } = &self.state else {
            return;
        };
        let ready: Vec<String> = catalog
            .items()
            .iter()
            .filter(|r| eligible(r, self.guide))
            .map(|r| r.id.clone())
            .collect();
        if ready.is_empty() {
            return;
        }
        let at = self
            .runtime
            .as_ref()
            .and_then(|id| ready.iter().position(|r| r == id));
        let next = match at {
            Some(at) => (at as isize + step).rem_euclid(ready.len() as isize) as usize,
            None => 0,
        };
        if choose_tool(&mut self.runtime, &mut self.model, &ready[next]) {
            self.changed = true;
        }
        self.picked = true;
        cx.notify();
    }

    /// Install or update a tool with its official installer, using the PATH a
    /// terminal would have, then read the profile's tools again and pick it.
    fn install(&mut self, runtime: String, cx: &mut Context<Self>) {
        let Some(&(_, _, command)) = probe::TOOLS.iter().find(|(id, ..)| *id == runtime) else {
            return;
        };
        if self.installing.get(&runtime) == Some(&None) {
            return;
        }
        self.installing.insert(runtime.clone(), None);
        let work = cx.background_executor().spawn(async move {
            let outcome = install::tool(command, &probe::login_path());
            // An installer can add a folder to the PATH that wasn't there.
            (outcome, probe::login_path())
        });
        cx.spawn(async move |this, cx| {
            let (outcome, path_env) = work.await;
            let _ = this.update(cx, |form, cx| {
                form.client = form.client.with_path(path_env.into());
                match outcome {
                    install::Outcome::Failed(reason) => {
                        form.installing.insert(runtime, Some(reason));
                    }
                    install::Outcome::Done | install::Outcome::Opened => {
                        form.installing.remove(&runtime);
                        form.select = Some(runtime);
                        form.changed = true;
                        form.picked = true;
                        form.load(true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Start the guide, or after it stopped, try again: the same request when
    /// nothing changed and the provider allows it, else the new choices.
    fn start_guide(&mut self, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        // While the request a start saved is open, Try again sends that
        // request, whatever was changed since; a new one only once it is
        // closed, or cancelled or forgotten under Earlier setups.
        if let Some((pane, _)) = &self.start
            && pane.read(cx).saved_open().is_some()
        {
            if pane.update(cx, |pane, cx| pane.retry(cx)) {
                cx.notify();
            }
            return;
        }
        // Saved before it runs, so a lost answer is asked about again under
        // the same request.
        let Some(prepared) = self.prepare(&uuid::Uuid::now_v7().simple().to_string(), cx) else {
            return;
        };
        let Some(controller) = self.controller() else {
            return;
        };
        let pane = cx.new(|cx| setup::Pane::from_prepared(controller, prepared, self.skip, cx));
        let subscriptions = vec![
            cx.subscribe(&pane, |form, _, open: &Open, cx| {
                form.finished = true;
                form.ticket.cancel();
                cx.emit(Event::Opened(open.clone()));
            }),
            cx.observe(&pane, |form: &mut Self, pane, cx| {
                // After each result a start stops on, read the open setups
                // again: one left open shows under Earlier setups, and one
                // closed frees Start.
                let settled = pane.read(cx).settled();
                if read_again(form.refreshed, settled) {
                    form.refreshed = settled;
                    form.load(true, cx);
                }
                cx.notify()
            }),
        ];
        self.refreshed = None;
        self.start = Some((pane, subscriptions));
        self.changed = false;
        self.error = None;
        cx.notify();
    }

    /// Where the agent works, and whether setup makes that folder: the one
    /// chosen, or for a guide, a folder of its own under ~/Agents.
    fn folder(&self, cx: &gpui::App) -> Option<(PathBuf, bool)> {
        if let Some(project) = &self.project {
            return Some((project.clone(), false));
        }
        let name = agent_name(self.name.read(cx).value());
        (self.guide && agent_interface_roster::valid_agent(&name))
            .then(|| std::env::var_os("HOME"))
            .flatten()
            .map(|home| (PathBuf::from(home).join("Agents").join(name), true))
    }

    /// The profile this form adds an agent to.
    pub fn profile_id(&self) -> &str {
        &self.profile.item().profile_id
    }
    fn prepare(&self, request_id: &str, cx: &Context<Self>) -> Option<Prepared> {
        if self.finished || self.name.read(cx).composing() || self.brief.read(cx).composing() {
            return None;
        }
        let State::Ready { catalog, .. } = &self.state else {
            return None;
        };
        let (project, create) = self.folder(cx)?;
        freeze(
            request_id,
            &self.profile,
            catalog,
            Selection {
                name: &agent_name(self.name.read(cx).value()),
                project: &project,
                create,
                runtime: self.runtime.as_deref()?,
                model: self.model.as_deref(),
                brief: self
                    .custom_brief
                    .then(|| self.brief.read(cx).value().trim()),
                greeting: self.greeting,
            },
        )
    }

    /// What the Rust service starts, and the folder to make for it first.
    fn served_start(&self, cx: &Context<Self>) -> Option<(Start, Option<PathBuf>)> {
        if self.finished || self.name.read(cx).composing() || self.repository.read(cx).composing() {
            return None;
        }
        let State::Ready { catalog, .. } = &self.state else {
            return None;
        };
        if catalog.identity() != &self.profile.expected_identity().ok()? {
            return None;
        }
        let runtime = self.runtime()?;
        if self
            .model
            .as_ref()
            .is_some_and(|model| !runtime.models.contains(model))
        {
            return None;
        }
        let (folder, create) = self.folder(cx)?;
        let watch = self.repository.read(cx).value().trim();
        let start = Start::new(
            &agent_name(self.name.read(cx).value()),
            &folder,
            &runtime.id,
            self.model.as_deref(),
            (!watch.is_empty()).then_some(watch),
        )
        .ok()?;
        Some((start, create.then_some(folder)))
    }

    /// What the screen shows, as a request to the Rust service.
    fn served_request(&self, request_id: String, cx: &Context<Self>) -> Option<setup::Request> {
        let (start, create) = self.served_start(cx)?;
        Some(setup::Request {
            profile: self.profile.clone(),
            connection: self.connection.clone(),
            start,
            create,
            request_id,
        })
    }

    /// Whether Start can send what the screen shows.
    fn can_start(&self, cx: &Context<Self>) -> bool {
        let saved_open = self
            .start
            .as_ref()
            .and_then(|(pane, _)| pane.read(cx).saved_open());
        let held = self.guide && matches!(self.state, State::Ready { pending, .. } if pending > 0);
        may_start(saved_open, held, || {
            if self.profile.served() && !self.guide {
                self.served_start(cx).is_some()
            } else {
                // Any request ID of the form setup takes.
                self.prepare(&uuid::Uuid::now_v7().simple().to_string(), cx)
                    .is_some()
            }
        })
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        if self.finished || !matches!(self.state, State::Ready { .. }) {
            return;
        }
        let ticket = self.ticket.0.clone();
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Agent project folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = selection.await {
                let _ = this.update(cx, |form, cx| {
                    if !form.finished
                        && form.ticket.accepts(&ticket)
                        && matches!(form.state, State::Ready { .. })
                    {
                        form.project = paths.into_iter().next().filter(|p| p.is_absolute());
                        form.changed = true;
                        form.name_from_folder(cx);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
    /// Name the agent after its folder, unless the person typed a name.
    fn name_from_folder(&mut self, cx: &mut Context<Self>) {
        let current = self.name.read(cx).value().to_owned();
        if !current.is_empty() && self.folder_name.as_deref() != Some(current.as_str()) {
            return;
        }
        let Some(name) = self.project.as_deref().and_then(folder_name) else {
            return;
        };
        self.name.update(cx, |field, cx| field.set_value(&name, cx));
        self.folder_name = Some(name);
    }
    fn submit(&mut self, expected: &Arc<std::sync::atomic::AtomicBool>, cx: &mut Context<Self>) {
        // Enter and a click pass the same gate as the button shows.
        if !self.ticket.accepts(expected) || !self.can_start(cx) {
            return;
        }
        if self.guide {
            self.start_guide(cx);
            return;
        }
        // Add agent starts with the Rust service's own start; a profile it
        // doesn't serve takes none from this app.
        let request_id = uuid::Uuid::now_v7().simple().to_string();
        let Some(request) = self.served_request(request_id, cx) else {
            return;
        };
        self.served = Some(request);
        self.finish(Event::Submit, cx);
    }
    /// Back from earlier setups: the choices stay, but a start that stopped
    /// may have been cancelled or forgotten there, so it and the open setups
    /// are read again and the next start is a new request.
    pub fn returned(&mut self, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        self.start = None;
        self.refreshed = None;
        self.load(true, cx);
    }

    /// Show earlier setups. The form stays live behind them, so Back returns
    /// to it as it was.
    fn recover(&mut self, cx: &mut Context<Self>) {
        if !self.finished {
            cx.emit(Event::Recover);
        }
    }

    fn finish(&mut self, event: Event, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ticket.cancel();
        cx.emit(event);
    }
}
fn load_choices(
    client: Client,
    directory: PathBuf,
    identity: &agent_interface_roster::profiles::Identity,
    served: bool,
    setup: bool,
    live: &dyn Fn() -> bool,
) -> Result<State, &'static str> {
    // A guide's start is saved in the setup journal first, so it waits for
    // a read of it; Add agent starts with the service's own start and needs
    // no journal at all.
    let (controller, pending) = if setup {
        let journal = journal::Journal::open(&directory.join("profile-setup"))
            .map_err(|_| "Private setup storage is unavailable.")?;
        let controller = Arc::new(Controller::new(journal, &client));
        let Ok(pending) = controller.pending(&identity.profile_id, live) else {
            return Ok(State::Unchecked(controller));
        };
        (Some(controller), pending.len())
    } else {
        (None, 0)
    };
    let catalog = if served {
        client.served_runtimes(identity, live)
    } else {
        client.runtimes(identity, live)
    }
    .map_err(
        |_| "The runtime catalog for this profile instance is unavailable. No setup was submitted.",
    )?;
    Ok(State::Ready {
        controller,
        catalog,
        pending,
    })
}
struct Selection<'a> {
    name: &'a str,
    project: &'a std::path::Path,
    /// Setup makes the folder: a guide's own, which doesn't exist yet.
    create: bool,
    runtime: &'a str,
    model: Option<&'a str>,
    /// Own instructions; `None` is the coordinating brief a fresh install starts with.
    brief: Option<&'a str>,
    greeting: bool,
}
fn freeze(
    request_id: &str,
    profile: &Profile,
    catalog: &Catalog,
    choice: Selection<'_>,
) -> Option<Prepared> {
    if catalog.identity() != &profile.expected_identity().ok()? {
        return None;
    }
    let runtime = catalog
        .items()
        .iter()
        .find(|r| r.id == choice.runtime && eligible(r, true))?;
    if choice
        .model
        .is_some_and(|model| !runtime.models.iter().any(|m| m == model))
    {
        return None;
    }
    Prepared::first_agent(
        request_id,
        profile,
        Agent {
            name: choice.name.into(),
            directory: choice.project.into(),
            create_directory: choice.create,
            runtime: runtime.id.clone(),
            model: choice.model.map(str::to_owned),
            brief: match choice.brief {
                Some(text) => Brief::Text { text: text.into() },
                None => Brief::Template {
                    template: "orchestrator-v1".into(),
                },
            },
        },
        // A tool that takes the default brief as its first turn already
        // greets, since that brief asks it to; custom instructions may not.
        choice.greeting
            && !(choice.brief.is_none() && BRIEF_AS_FIRST_TURN.contains(&runtime.id.as_str())),
    )
    .ok()
}
/// Whether the open setups are read again: after each result a start stops
/// on, which its saved revision tells apart from the last one read after.
fn read_again(last: Option<u64>, settled: Option<u64>) -> bool {
    settled.is_some() && settled != last
}
/// Tools that can't take a brief silently: Agent Backbone sends it as the
/// agent's first turn, and the default brief asks it to greet.
const BRIEF_AS_FIRST_TURN: [&str; 1] = ["antigravity"];
/// The name a profile's first agent starts with.
const GUIDE: &str = "guide";
/// A typed name as an agent name: spaces as hyphens.
fn agent_name(typed: &str) -> String {
    typed.split_whitespace().collect::<Vec<_>>().join("-")
}
/// A folder's name as an agent name: lower case, spaces as hyphens.
fn folder_name(path: &std::path::Path) -> Option<String> {
    let name = path
        .file_name()?
        .to_str()?
        .trim()
        .to_lowercase()
        .replace(' ', "-");
    (!name.is_empty()).then_some(name)
}
/// A text field of the Add agent form, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Name,
    Brief,
    Repository,
}
/// Where Tab goes from `at`: through the name, then the instructions and
/// the repository when each is shown, back to the name. From no field, or
/// one no longer shown, the name.
fn next_field(at: Option<Field>, brief: bool, repository: bool) -> Field {
    let shown: Vec<Field> = [
        Some(Field::Name),
        brief.then_some(Field::Brief),
        repository.then_some(Field::Repository),
    ]
    .into_iter()
    .flatten()
    .collect();
    at.and_then(|at| shown.iter().position(|field| *field == at))
        .map_or(Field::Name, |at| shown[(at + 1) % shown.len()])
}
/// Choose the tool `clicked`. The one already chosen changes nothing; another
/// starts on its default model. Returns whether the choice changed.
fn choose_tool(runtime: &mut Option<String>, model: &mut Option<String>, clicked: &str) -> bool {
    if runtime.as_deref() == Some(clicked) {
        return false;
    }
    *runtime = Some(clicked.to_owned());
    *model = None;
    true
}
/// The tool a form starts on when nothing else says: a signed-in one, else
/// any that can run. `setup`: the agent starts through setup.
fn first_choice(catalog: &Catalog, setup: bool) -> Option<String> {
    let ready = || catalog.items().iter().filter(|r| eligible(r, setup));
    ready()
        .find(|r| r.authentication == Authentication::Authenticated)
        .or_else(|| ready().next())
        .map(|r| r.id.clone())
}
/// What a guide's tool card says, and the action it offers.
fn guide_card(
    runtime: &Runtime,
    signed_in: bool,
    installing: Option<&Option<String>>,
) -> (&'static str, bool, bool, Option<&'static str>) {
    let installable = probe::TOOLS.iter().any(|(id, ..)| *id == runtime.id);
    match installing {
        Some(None) => ("Installing…", false, true, None),
        Some(Some(_)) => ("Couldn't install", true, false, Some("Try again")),
        None if !runtime.available => (
            "Not installed",
            false,
            false,
            installable.then_some("Install"),
        ),
        None if !eligible(runtime, true) => (
            "Needs an update",
            false,
            false,
            installable.then_some("Update"),
        ),
        None if signed_in => ("Signed in", false, false, None),
        None => ("Sign in when it starts", false, false, None),
    }
}
/// Whether an agent can start in `runtime`. Through setup (`setup`), as a
/// guide starts: setup starts Claude Code, Codex and Antigravity only. Else
/// through the service's start, as Add agent starts one.
fn eligible(runtime: &Runtime, setup: bool) -> bool {
    if setup {
        return runtime.available && SETUP_RUNTIMES.contains(&runtime.id.as_str());
    }
    runtime.id != "shell"
        && runtime.available
        && runtime.setup_supported
        && runtime.setup_compatibility.compatible
}
/// The tools setup starts a first agent in.
const SETUP_RUNTIMES: [&str; 3] = ["claude", "codex", "antigravity"];
/// What a tool's card says about it: whether the agent can start signed in.
fn card_state(value: Authentication) -> &'static str {
    match value {
        Authentication::Authenticated => "Signed in",
        Authentication::NotAuthenticated => "Asks you to sign in",
        _ => "Ready",
    }
}
impl Render for Form {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.guide {
            return self.guide_body(window, cx).into_any_element();
        }
        let (t, c) = (tokens(), colors());
        let ticket = self.ticket.0.clone();
        let line = |label: &'static str, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s12))
                .child(
                    ui::text(
                        div()
                            .w(px(t.space.s28 * 2.5))
                            .flex_shrink_0()
                            .text_color(c.muted),
                        &t.text.meta,
                    )
                    .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .child(control),
                )
        };
        let note =
            |text: SharedString| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let mut body = div().flex().flex_col().gap(px(t.space.s10));
        match &self.state {
            State::Loading => body = body.child(note("Checking this profile's tools…".into())),
            State::Unchecked(_) => {
                body = body
                    .child(note("An earlier setup could not be read. Open it to finish or forget it, then check again.".into()))
                    .child(
                        div().flex().gap(px(t.space.s8))
                            .child(ui::button("recover-pending-setup", "Earlier setups", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.recover(cx))))
                            .child(ui::button("recheck-setup", "Check again", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.load(false, cx)))),
                    )
            }
            State::Failed => {
                body = body.child(ui::button("retry-setup-catalog", "Check again", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.load(false, cx))))
            }
            State::Ready { catalog, pending, .. } => {
                if *pending > 0 {
                    body = body.child(
                        div().flex().items_center().gap(px(t.space.s8))
                            .child(ui::step_dot(ui::Step::Waits))
                            .child(div().flex_1().child(note(if *pending == 1 { "An earlier setup in this profile didn't finish.".into() } else { format!("{pending} earlier setups in this profile didn't finish.").into() })))
                            .child(ui::button("recover-pending-setup", "Open", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.recover(cx)))),
                    );
                }
                let typed = self.name.read(cx).value().to_owned();
                let name = agent_name(&typed);
                let from_folder = self.folder_name.as_deref().is_some_and(|name| name == typed);
                // The agent as it will be: its name, what it does, where it works.
                let folder = self.folder(cx);
                let card = div()
                    .flex()
                    .gap(px(t.space.s14))
                    .p(px(t.space.s14))
                    .rounded(px(t.radius.popover))
                    .bg(c.panel)
                    .border(px(t.size.hairline))
                    .border_color(c.line)
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(t.size.view_tile + t.space.s8))
                            .rounded(px(t.radius.slot))
                            .bg(c.accent_soft)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(ui::icon("bot", t.size.icon_large, c.accent_text)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s8))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(ui::text(div().text_color(c.text), &t.text.card_title).child("Your agent"))
                                    // The Rust service takes no instructions yet.
                                    .children((!self.profile.served()).then(|| note(
                                        if self.custom_brief {
                                            "Follows the instructions you give it below."
                                        } else {
                                            "Coordinates your other agents: it learns your goals and proposes project agents for you to approve."
                                        }
                                        .into(),
                                    ))),
                            )
                            .child(line(
                                "Name",
                                div().flex().flex_1().min_w_0().items_center().gap(px(t.space.s8))
                                    .child(div().flex_1().min_w_0().child(self.name.clone()))
                                    .map(|el| {
                                        if from_folder {
                                            el.child(note("From the folder".into()))
                                        } else if name.is_empty() {
                                            el
                                        } else if !agent_interface_roster::valid_agent(&name) {
                                            el.child(ui::text(div().text_color(c.attention_text), &t.text.meta).child("Use letters, numbers, - _ or ."))
                                        } else if name != typed {
                                            el.child(note(format!("Named {name}").into()))
                                        } else {
                                            el
                                        }
                                    })
                                    .into_any_element(),
                            ))
                            .child(line(
                                "Folder",
                                div().flex().flex_1().min_w_0().items_center().gap(px(t.space.s8))
                                    .child(ui::icon("folder", t.size.icon, c.muted))
                                    .child(
                                        div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                                            .text_color(if folder.is_some() { c.code } else { c.faint })
                                            .child(match &folder {
                                                Some((path, _)) => home_relative(path),
                                                None => "Choose the folder it works in".into(),
                                            }),
                                    )
                                    // Setup makes this folder for the agent.
                                    .when(folder.as_ref().is_some_and(|(_, made)| *made), |el| {
                                        el.child(
                                            ui::text(
                                                div().flex_shrink_0().px(px(t.space.s6)).rounded(px(t.radius.row))
                                                    .border(px(t.size.hairline)).border_color(c.line_strong).text_color(c.muted),
                                                &t.text.meta,
                                            )
                                            .child("New"),
                                        )
                                    })
                                    .child(div().flex_1())
                                    .child(ui::button("setup-folder", if self.project.is_some() || folder.is_some() { "Change…" } else { "Choose…" }, ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.choose_folder(cx))))
                                    .into_any_element(),
                            )),
                    );
                body = body.child(card);
                // The same cards onboarding's first agent uses.
                let mut tools = div().w_full().grid().grid_cols(2).gap(px(t.space.s8));
                for runtime in catalog.items().iter().filter(|r| eligible(r, false)) {
                    let id = runtime.id.clone();
                    let expected = ticket.clone();
                    tools = tools.child(
                        ui::tool_card(format!("runtime-{id}"), runtime.display_name.clone(), card_state(runtime.authentication), false, self.runtime.as_ref() == Some(&id), false, None)
                            .on_click(cx.listener(move |form, _, _, cx| {
                                if form.finished || !form.ticket.accepts(&expected) { return; }
                                let State::Ready { catalog, .. } = &form.state else { return; };
                                if catalog.items().iter().any(|r| r.id == id && eligible(r, false)) { choose_tool(&mut form.runtime, &mut form.model, &id); cx.notify(); }
                            })),
                    );
                }
                body = body.child(line(
                    "Runs in",
                    if catalog.items().iter().any(|r| eligible(r, false)) {
                        tools.into_any_element()
                    } else {
                        note("No tool in this profile can set up an agent yet.".into()).into_any_element()
                    },
                ));
                // The Rust service's Add agent takes no instructions or greeting.
                let served = self.profile.served();
                if !served {
                body = body.child(line(
                    "Role",
                    ui::segments()
                        .child(ui::segment("brief-coordinating", "Coordinating agent", !self.custom_brief).on_click(cx.listener(|form, _, _, cx| { form.custom_brief = false; cx.notify(); })))
                        .child(ui::segment("brief-own", "My own instructions", self.custom_brief).on_click(cx.listener(|form, _, _, cx| { form.custom_brief = true; cx.notify(); })))
                        .into_any_element(),
                ));
                if self.custom_brief {
                    body = body.child(line("", self.brief.clone().into_any_element()));
                }
                }
                body = body.child(
                    div().id("setup-more").flex().items_center().gap(px(t.space.s6)).cursor_pointer().text_color(c.muted)
                        .child(ui::icon(if self.more { "chevron-down" } else { "chevron-right" }, t.size.icon, c.muted))
                        .child(ui::text(div(), &t.text.meta).child(if served { "Model and repository" } else { "Model and greeting" }))
                        .on_click(cx.listener(|form, _, _, cx| { form.more = !form.more; cx.notify(); })),
                );
                if self.more {
                    if let Some(runtime) = self.runtime() {
                        let runtime_id = runtime.id.clone();
                        let mut models = ui::segments().flex_wrap();
                        for model in std::iter::once(None).chain(runtime.models.iter().cloned().map(Some)) {
                            let expected = ticket.clone();
                            let selected = self.model == model;
                            let label = model.clone().unwrap_or_else(|| "Tool's default".into());
                            let origin_runtime = runtime_id.clone();
                            models = models.child(ui::segment(SharedString::from(format!("model-{label}")), label, selected)
                                .on_click(cx.listener(move |form, _, _, cx| {
                                    if form.finished || !form.ticket.accepts(&expected) || form.runtime.as_ref() != Some(&origin_runtime) { return; }
                                    if form.runtime().is_some_and(|r| model.as_ref().is_none_or(|m| r.models.contains(m))) { form.model = model.clone(); cx.notify(); }
                                })));
                        }
                        body = body.child(line("Model", models.into_any_element()));
                    }
                    if served {
                        let typed = self.repository.read(cx).value().trim().to_owned();
                        body = body.child(line(
                            "Repository",
                            ui::field_frame(self.repository.read(cx).focus.is_focused(window))
                                .flex_1()
                                .child(div().flex_1().min_w_0().child(self.repository.clone()))
                                .into_any_element(),
                        ));
                        body = body.child(line(
                            "",
                            if typed.is_empty() || repository(&typed) {
                                note("Optional: a GitHub repository it watches, as owner/name".into())
                            } else {
                                ui::text(div().text_color(c.attention_text), &t.text.meta).child("Use owner/name, such as acme/app")
                            }
                            .into_any_element(),
                        ));
                    }
                    if !served {
                        let expected = ticket.clone();
                        body = body.child(line(
                            "Greeting",
                            div().flex().items_center().gap(px(t.space.s8))
                                .child(ui::switch("Greeting", self.greeting).on_click(cx.listener(move |form, _, _, cx| {
                                    if form.finished || !form.ticket.accepts(&expected) { return; }
                                    form.greeting = !form.greeting; cx.notify();
                                })))
                                .child(note("Send the agent's greeting".into()))
                                .into_any_element(),
                        ));
                    }
                }
            }
        }
        if let Some(error) = self.error {
            body =
                body.child(ui::text(div().text_color(c.attention_text), &t.text.meta).child(error));
        }
        let ready = self.can_start(cx);
        // The button says which agent it starts.
        let start = match agent_name(self.name.read(cx).value()) {
            name if agent_interface_roster::valid_agent(&name) => format!("Start {name}"),
            _ => "Start agent".to_owned(),
        };
        body.track_focus(&self.focus)
            .capture_key_down(cx.listener(|form, event: &gpui::KeyDownEvent, window, cx| {
                if form.name.read(cx).composing()
                    || form.brief.read(cx).composing()
                    || form.repository.read(cx).composing()
                {
                    return;
                }
                let keys = &event.keystroke;
                match keys.key.as_str() {
                    "escape" => {
                        form.finish(Event::Back, cx);
                        cx.stop_propagation();
                    }
                    "enter" if keys.modifiers.platform || keys.modifiers.control => {
                        let ticket = form.ticket.0.clone();
                        form.submit(&ticket, cx);
                        cx.stop_propagation();
                    }
                    "tab" if matches!(form.state, State::Ready { .. }) => {
                        let served = form.profile.served();
                        let focus = |field: &Entity<NameField>| field.read(cx).focus.clone();
                        let fields = [
                            (Field::Name, focus(&form.name)),
                            (Field::Brief, focus(&form.brief)),
                            (Field::Repository, focus(&form.repository)),
                        ];
                        let at = fields
                            .iter()
                            .find(|(_, handle)| handle.is_focused(window))
                            .map(|(field, _)| *field);
                        let next =
                            next_field(at, form.custom_brief && !served, served && form.more);
                        if let Some((_, handle)) = fields.iter().find(|(field, _)| *field == next) {
                            window.focus(handle, cx);
                        }
                        cx.stop_propagation();
                    }
                    _ => (),
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s12))
                    .pt(px(t.space.s6))
                    .child(
                        ui::button("setup-choice-submit", start, ui::Button::Primary)
                            .when(!ready, |el| el.opacity(0.5).cursor_default())
                            .on_click(cx.listener(move |form, _, _, cx| form.submit(&ticket, cx))),
                    )
                    .child(
                        ui::text(
                            div()
                                .id("setup-choice-back")
                                .cursor_pointer()
                                .text_color(c.muted)
                                .hover(|el| el.text_color(colors().text)),
                            &t.text.row,
                        )
                        .child(self.skip)
                        .on_click(cx.listener(|form, _, _, cx| form.finish(Event::Back, cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Form {
    /// The guide screen's body: its name, the tool it runs in, Start, and
    /// everything else under More options.
    fn guide_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (t, c) = (tokens(), colors());
        let label = |text: &'static str| {
            ui::text(div().pb(px(t.space.s6)).text_color(c.muted), &t.text.meta).child(text)
        };
        let note = |text: SharedString, attention: bool| {
            ui::text(
                div().text_color(if attention { c.attention_text } else { c.muted }),
                &t.text.meta,
            )
            .child(text)
        };
        let busy = self.busy(cx);
        let failure = self
            .start
            .as_ref()
            .and_then(|(pane, _)| pane.read(cx).failure());
        let typed = self.name.read(cx).value().to_owned();
        let name = agent_name(&typed);
        let valid = agent_interface_roster::valid_agent(&name);
        let mut body = div().w_full().flex().flex_col().gap(px(t.space.s16));
        // Its name, with the agent's mark.
        body = body.child(
            div()
                .flex()
                .flex_col()
                .child(label("Name"))
                .child(
                    ui::setup_field(self.name.read(cx).focus.is_focused(window))
                        .child(ui::icon("bot", t.size.icon_large, c.muted))
                        .child(div().flex_1().min_w_0().child(self.name.clone())),
                )
                .when(!name.is_empty() && !valid, |el| {
                    el.child(
                        div()
                            .pt(px(t.space.s6))
                            .child(note("Use letters, numbers, - _ or .".into(), true)),
                    )
                }),
        );
        // The tool it runs in.
        let ticket = self.ticket.0.clone();
        let cards = match &self.state {
            State::Loading | State::Failed => note(
                if matches!(self.state, State::Failed) {
                    "This profile's tools could not be read."
                } else {
                    "Checking this profile's tools…"
                }
                .into(),
                matches!(self.state, State::Failed),
            )
            .into_any_element(),
            State::Unchecked(_) => note(
                "An earlier start in this profile could not be read. Open it under Earlier setups."
                    .into(),
                true,
            )
            .into_any_element(),
            State::Ready { catalog, .. } => {
                let mut grid = div().w_full().grid().grid_cols(2).gap(px(t.space.s8));
                let mut shown = 0;
                for runtime in catalog
                    .items()
                    .iter()
                    .filter(|r| SETUP_RUNTIMES.contains(&r.id.as_str()))
                {
                    let signed_in = self
                        .accounts
                        .get(&runtime.id)
                        .copied()
                        .unwrap_or(runtime.authentication)
                        == Authentication::Authenticated;
                    let (state, attention, installing, action) =
                        guide_card(runtime, signed_in, self.installing.get(&runtime.id));
                    if !runtime.available && action.is_none() && !installing {
                        continue;
                    }
                    shown += 1;
                    let id = runtime.id.clone();
                    let ready = eligible(runtime, true);
                    let expected = ticket.clone();
                    grid = grid.child(
                        ui::tool_card(
                            format!("runtime-{id}"),
                            runtime.display_name.clone(),
                            state,
                            attention,
                            self.runtime.as_ref() == Some(&id) && ready,
                            installing,
                            action,
                        )
                        .on_click(cx.listener(move |form, _, _, cx| {
                            if form.finished || form.busy(cx) || !form.ticket.accepts(&expected) {
                                return;
                            }
                            if ready {
                                if choose_tool(&mut form.runtime, &mut form.model, &id) {
                                    form.changed = true;
                                }
                                form.picked = true;
                                cx.notify();
                            } else {
                                form.install(id.clone(), cx);
                            }
                        })),
                    );
                }
                if shown == 0 {
                    note("No tool in this profile can run a guide yet.".into(), true)
                        .into_any_element()
                } else {
                    grid.into_any_element()
                }
            }
        };
        body = body.child(div().flex().flex_col().child(label("Runs in")).child(cards));
        let install_failed = self.installing.values().find_map(|failed| failed.clone());
        if let State::Ready { pending, .. } = &self.state
            && *pending > 0
        {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(t.space.s8))
                    .child(note(
                        if *pending == 1 {
                            "An earlier start in this profile didn't finish.".into()
                        } else {
                            format!("{pending} earlier starts in this profile didn't finish.")
                                .into()
                        },
                        false,
                    ))
                    .child(
                        ui::text_action("recover-pending-setup", "Earlier setups")
                            .on_click(cx.listener(|form, _, _, cx| form.recover(cx))),
                    ),
            );
        }
        if matches!(self.state, State::Failed) {
            body = body.child(
                div().flex().justify_end().child(
                    ui::text_action("retry-setup-catalog", "Check again")
                        .on_click(cx.listener(|form, _, _, cx| form.load(true, cx))),
                ),
            );
        }
        if matches!(self.state, State::Unchecked(_)) {
            body = body.child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(t.space.s12))
                    .child(
                        ui::text_action("recover-pending-setup", "Earlier setups")
                            .on_click(cx.listener(|form, _, _, cx| form.recover(cx))),
                    )
                    .child(
                        ui::text_action("recheck-setup", "Check again")
                            .on_click(cx.listener(|form, _, _, cx| form.load(false, cx))),
                    ),
            );
        }
        if self.more {
            body = body.child(self.more_options(cx));
        }
        // Start, and what it is doing.
        let tool = self.runtime().is_some();
        let ready = self.can_start(cx);
        let again = failure.as_ref().map(|_| {
            self.start
                .as_ref()
                .and_then(|(pane, _)| pane.read(cx).again_label())
        });
        let start = start_label(busy, again, tool, valid, &name);
        let status = self
            .start
            .as_ref()
            .filter(|_| busy)
            .map(|(pane, _)| setup::stage(pane.read(cx).step(), &name));
        let problem = failure
            .map(|failure| {
                match self.start.as_ref().and_then(|(pane, _)| pane.read(cx).saved_open()) {
                    Some(true) if self.changed => format!(
                        "{failure} Try again sends it as it was; to change it, cancel or forget it under Earlier setups."
                    ),
                    Some(false) => format!(
                        "{failure} It can't be sent again: cancel or forget it under Earlier setups."
                    ),
                    _ => failure,
                }
            })
            .or(install_failed)
            .or(self.error.map(str::to_owned));
        let expected = ticket.clone();
        let tail =
            div()
                .w_full()
                .flex()
                .flex_col()
                .pt(px(t.space.s20))
                .children(start.map(|start| {
                    ui::setup_button("setup-choice-submit", start, ready, busy).on_click(
                        cx.listener(move |form, _, _, cx| {
                            if ready {
                                form.submit(&expected, cx)
                            }
                        }),
                    )
                }))
                .children(status.map(|status| {
                    div()
                        .pt(px(t.space.s10))
                        .flex()
                        .justify_center()
                        .child(note(status.into(), false))
                }))
                .children(problem.map(|problem| {
                    div()
                        .pt(px(t.space.s10))
                        .flex()
                        .justify_center()
                        .text_center()
                        .child(note(problem.into(), true))
                }))
                .child(
                    div()
                        .pt(px(t.space.s12))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            ui::text_action("setup-more", "More options")
                                .child(ui::icon(
                                    if self.more {
                                        "chevron-down"
                                    } else {
                                        "chevron-right"
                                    },
                                    t.size.icon - t.space.s2,
                                    c.muted,
                                ))
                                .on_click(cx.listener(|form, _, _, cx| {
                                    form.more = !form.more;
                                    cx.notify();
                                })),
                        )
                        .child(ui::text_action("setup-choice-back", self.skip).on_click(
                            cx.listener(|form, _, _, cx| {
                                if !form.busy(cx) {
                                    form.finish(Event::Back, cx)
                                }
                            }),
                        )),
                );
        let expected = ticket.clone();
        div()
            .id("guide-choices")
            .w_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .capture_key_down(
                cx.listener(move |form, event: &gpui::KeyDownEvent, window, cx| {
                    if form.name.read(cx).composing()
                        || form.brief.read(cx).composing()
                        || form.repository.read(cx).composing()
                    {
                        return;
                    }
                    let brief = form.brief.read(cx).focus.is_focused(window);
                    // The repository field sits under the model choice: its
                    // arrow keys never change the tool.
                    let repository = form.repository.read(cx).focus.is_focused(window);
                    match event.keystroke.key.as_str() {
                        "escape" if !form.first && !form.busy(cx) => form.finish(Event::Back, cx),
                        "enter" if !brief => form.submit(&expected, cx),
                        // Up and down move between the tools that can run.
                        key @ ("up" | "down") if !brief && !repository && !form.busy(cx) => {
                            form.step_tool(if key == "up" { -1 } else { 1 }, cx)
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .child(body)
            .child(tail)
    }

    /// Folder, instructions, model and greeting, in one group: what setup
    /// takes for a first agent.
    fn more_options(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let ticket = self.ticket.0.clone();
        let row = |label: &'static str, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s10))
                .min_h(px(t.size.action))
                .child(
                    ui::text(
                        div()
                            .w(px(t.space.s28 * 3.0))
                            .flex_shrink_0()
                            .text_color(c.muted),
                        &t.text.meta,
                    )
                    .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .child(control),
                )
        };
        let quiet =
            |text: &'static str| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let folder = self.folder(cx);
        let mut group = div()
            .flex()
            .flex_col()
            .gap(px(t.space.s10))
            .p(px(t.space.s12))
            .rounded(px(t.radius.slot))
            .border(px(t.size.hairline))
            .border_color(c.line)
            .bg(c.panel)
            .child(row(
                "Folder",
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if folder.is_some() { c.code } else { c.faint })
                            .child(match &folder {
                                Some((path, _)) => home_relative(path),
                                None => "Choose the folder it works in".into(),
                            }),
                    )
                    .when(folder.as_ref().is_some_and(|(_, made)| *made), |el| {
                        el.child(
                            ui::text(
                                div()
                                    .flex_shrink_0()
                                    .px(px(t.space.s6))
                                    .rounded(px(t.radius.row))
                                    .border(px(t.size.hairline))
                                    .border_color(c.line_strong)
                                    .text_color(c.muted),
                                &t.text.meta,
                            )
                            .child("New"),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        ui::text_action("setup-folder", "Change…")
                            .on_click(cx.listener(|form, _, _, cx| form.choose_folder(cx))),
                    )
                    .into_any_element(),
            ));
        group = group.child(row(
            "Instructions",
            ui::segments()
                .child(
                    ui::segment(
                        "brief-coordinating",
                        "Coordinating agent",
                        !self.custom_brief,
                    )
                    .on_click(cx.listener(|form, _, _, cx| {
                        form.custom_brief = false;
                        form.changed = true;
                        cx.notify();
                    })),
                )
                .child(
                    ui::segment("brief-own", "My own", self.custom_brief).on_click(cx.listener(
                        |form, _, _, cx| {
                            form.custom_brief = true;
                            form.changed = true;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
        ));
        if self.custom_brief {
            group = group.child(self.brief.clone());
        }
        if let Some(runtime) = self.runtime() {
            let runtime_id = runtime.id.clone();
            let mut models = ui::segments().flex_wrap();
            for model in std::iter::once(None).chain(runtime.models.iter().cloned().map(Some)) {
                let expected = ticket.clone();
                let selected = self.model == model;
                let label = model.clone().unwrap_or_else(|| "Tool's default".into());
                let origin_runtime = runtime_id.clone();
                models = models.child(
                    ui::segment(
                        SharedString::from(format!("model-{label}")),
                        label,
                        selected,
                    )
                    .on_click(cx.listener(move |form, _, _, cx| {
                        if form.finished
                            || !form.ticket.accepts(&expected)
                            || form.runtime.as_ref() != Some(&origin_runtime)
                        {
                            return;
                        }
                        if form
                            .runtime()
                            .is_some_and(|r| model.as_ref().is_none_or(|m| r.models.contains(m)))
                        {
                            form.model = model.clone();
                            form.changed = true;
                            cx.notify();
                        }
                    })),
                );
            }
            group = group.child(row("Model", models.into_any_element()));
        }
        let expected = ticket.clone();
        group = group.child(row(
            "Greeting",
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .child(ui::switch("Greeting", self.greeting).on_click(cx.listener(
                    move |form, _, _, cx| {
                        if form.finished || !form.ticket.accepts(&expected) {
                            return;
                        }
                        form.greeting = !form.greeting;
                        form.changed = true;
                        cx.notify();
                    },
                )))
                .child(quiet("The guide says hello first"))
                .into_any_element(),
        ));
        group
    }
}

/// Whether Start may run, for its button and for Enter alike: a start's
/// open request is sent again, or nothing is; another open request of the
/// profile, `held`, holds a guide back until it is finished, cancelled or
/// forgotten under Earlier setups, as admission would refuse it anyway;
/// else the choices must make a request, which only then is tried.
fn may_start(saved_open: Option<bool>, held: bool, preparable: impl FnOnce() -> bool) -> bool {
    match saved_open {
        Some(sendable) => sendable,
        None => !held && preparable(),
    }
}
/// The Start button's words, or none: after a start stopped, `again` is
/// the button its saved request offers, none while it is open with
/// nothing it may send.
fn start_label(
    busy: bool,
    again: Option<Option<&'static str>>,
    tool: bool,
    valid: bool,
    name: &str,
) -> Option<String> {
    Some(if busy {
        format!("Starting {name}…")
    } else if let Some(again) = again {
        again?.to_owned()
    } else if !tool {
        "Choose a tool".to_owned()
    } else if valid {
        format!("Start {name}")
    } else {
        "Start guide".to_owned()
    })
}
#[cfg(test)]
mod tests;
