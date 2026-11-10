//! Explicit first-agent choices in one captured profile and operator context.
//! As a profile's guide, the same choices are the second first-run screen:
//! a name, the tool it runs in, Start, and the rest under More options.
use super::{Open, Ticket, setup, ui};
use crate::{
    desktop::{
        design::{colors, tokens},
        name::NameField,
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
    setup::{Agent, Brief, Prepared},
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
        controller: Arc<Controller>,
        catalog: Catalog,
        /// Open requests, offered for recovery without blocking other agents.
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
    _fields: Vec<Subscription>,
    project: Option<PathBuf>,
    runtime: Option<String>,
    model: Option<String>,
    hooks: bool,
    greeting: bool,
    prepared: Option<(Prepared, Arc<Controller>)>,
    error: Option<&'static str>,
    ticket: Ticket,
    finished: bool,
    /// The name last filled in from the folder, kept until the person edits it.
    folder_name: Option<String>,
    /// Model, instructions and hooks are open.
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
    /// Something was changed since the start stopped, so Try again sends the
    /// new choices rather than the same request.
    changed: bool,
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
        let fields = [&name, &brief]
            .into_iter()
            .map(|field| {
                cx.observe(field, |form: &mut Self, _, cx| {
                    form.changed = true;
                    cx.notify()
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
            _fields: fields,
            project: None,
            runtime: None,
            model: None,
            hooks: true,
            greeting: true,
            prepared: None,
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
            changed: false,
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
        let work = cx.background_executor().spawn(async move {
            let identity = expected.map_err(|_| "This profile has no ready instance for setup.")?;
            load_choices(client, directory, &identity, &|| {
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
                            let ready = |id: &String| {
                                catalog.items().iter().any(|r| &r.id == id && eligible(r))
                            };
                            let kept = form.runtime.clone().filter(ready);
                            form.runtime = form
                                .select
                                .take()
                                .filter(ready)
                                .or(kept)
                                .or_else(|| form.prefer.clone().filter(ready))
                                .or_else(|| first_choice(catalog));
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
                        form.state = State::Failed;
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
            State::Unchecked(controller) | State::Ready { controller, .. } => {
                Some(controller.clone())
            }
            _ => None,
        }
    }
    pub fn take_prepared(&mut self) -> Option<(Prepared, Arc<Controller>)> {
        self.prepared.take()
    }
    fn runtime(&self) -> Option<&Runtime> {
        let State::Ready { catalog, .. } = &self.state else {
            return None;
        };
        catalog
            .items()
            .iter()
            .find(|r| Some(&r.id) == self.runtime.as_ref() && eligible(r))
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
        self.name = cx.new(|cx| NameField::bare(GUIDE, window, cx));
        self._fields
            .push(cx.observe(&self.name, |form: &mut Self, _, cx| {
                form.changed = true;
                cx.notify()
            }));
    }

    /// Read which installed tools are signed in, once each, so a guide's
    /// cards can say so. Each read starts the tool's own check.
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
                // Prefer a signed-in tool while the person hasn't chosen.
                if !form.changed
                    && let State::Ready { catalog, .. } = &form.state
                    && let Some(id) = catalog
                        .items()
                        .iter()
                        .find(|r| {
                            eligible(r)
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
        let work = cx
            .background_executor()
            .spawn(async move { install::tool(command, &probe::login_path()) });
        cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |form, cx| {
                match outcome {
                    install::Outcome::Failed(reason) => {
                        form.installing.insert(runtime, Some(reason));
                    }
                    install::Outcome::Done | install::Outcome::Opened => {
                        form.installing.remove(&runtime);
                        form.select = Some(runtime);
                        form.changed = true;
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
        if !self.changed
            && let Some((pane, _)) = &self.start
            && pane.update(cx, |pane, cx| pane.retry(cx))
        {
            cx.notify();
            return;
        }
        let Some(prepared) = self.prepare(&uuid::Uuid::new_v4().simple().to_string(), cx) else {
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
            cx.observe(&pane, |_, _, cx| cx.notify()),
        ];
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
                hooks: self.hooks,
                greeting: self.greeting,
            },
        )
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
        if !self.ticket.accepts(expected) {
            return;
        }
        if self.guide {
            self.start_guide(cx);
            return;
        }
        let Some(prepared) = self.prepare(&uuid::Uuid::new_v4().simple().to_string(), cx) else {
            return;
        };
        let Some(controller) = self.controller() else {
            return;
        };
        self.prepared = Some((prepared, controller));
        self.finish(Event::Submit, cx);
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
    live: &dyn Fn() -> bool,
) -> Result<State, &'static str> {
    let journal = journal::Journal::open(&directory.join("profile-setup"))
        .map_err(|_| "Private setup storage is unavailable.")?;
    let controller = Arc::new(Controller::new(journal, &client));
    let Ok(pending) = controller.pending(identity, live) else {
        return Ok(State::Unchecked(controller));
    };
    let catalog = client.runtimes(identity, live).map_err(
        |_| "The runtime catalog for this profile instance is unavailable. No setup was submitted.",
    )?;
    Ok(State::Ready {
        controller,
        catalog,
        pending: pending.len(),
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
    hooks: bool,
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
        .find(|r| r.id == choice.runtime && eligible(r))?;
    if choice
        .model
        .is_some_and(|model| !runtime.models.iter().any(|m| m == model))
    {
        return None;
    }
    Prepared::first_agent(
        request_id,
        profile,
        if choice.hooks {
            vec![runtime.id.clone()]
        } else {
            Vec::new()
        },
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
        choice.greeting,
    )
    .ok()
}
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
/// The tool a guide starts on when nothing else says: a signed-in one, else
/// any that can run.
fn first_choice(catalog: &Catalog) -> Option<String> {
    let ready = || catalog.items().iter().filter(|r| eligible(r));
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
        None if !eligible(runtime) => (
            "Needs an update",
            false,
            false,
            installable.then_some("Update"),
        ),
        None if signed_in => ("Signed in", false, false, None),
        None => ("Sign in when it starts", false, false, None),
    }
}
fn eligible(runtime: &Runtime) -> bool {
    runtime.id != "shell"
        && runtime.available
        && runtime.setup_supported
        && runtime.setup_compatibility.compatible
}
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
                            .child(ui::button("recover-pending-setup", "Earlier setups", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.finish(Event::Recover, cx))))
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
                            .child(ui::button("recover-pending-setup", "Open", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.finish(Event::Recover, cx)))),
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
                                    .child(ui::text(div().text_color(c.text), &t.text.card_title).child(
                                        if self.guide && !self.custom_brief { "Your guide" } else { "Your agent" },
                                    ))
                                    .child(note(
                                        if self.custom_brief {
                                            "Follows the instructions you give it below."
                                        } else {
                                            "Coordinates your other agents: it learns your goals and proposes project agents for you to approve."
                                        }
                                        .into(),
                                    )),
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
                for runtime in catalog.items().iter().filter(|r| eligible(r)) {
                    let id = runtime.id.clone();
                    let expected = ticket.clone();
                    tools = tools.child(
                        ui::tool_card(format!("runtime-{id}"), runtime.display_name.clone(), card_state(runtime.authentication), false, self.runtime.as_ref() == Some(&id), false, None)
                            .on_click(cx.listener(move |form, _, _, cx| {
                                if form.finished || !form.ticket.accepts(&expected) { return; }
                                let State::Ready { catalog, .. } = &form.state else { return; };
                                if catalog.items().iter().any(|r| r.id == id && eligible(r)) { form.runtime = Some(id.clone()); form.model = None; cx.notify(); }
                            })),
                    );
                }
                body = body.child(line(
                    "Runs in",
                    if catalog.items().iter().any(eligible) {
                        tools.into_any_element()
                    } else {
                        note("No tool in this profile can set up an agent yet.".into()).into_any_element()
                    },
                ));
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
                body = body.child(
                    div().id("setup-more").flex().items_center().gap(px(t.space.s6)).cursor_pointer().text_color(c.muted)
                        .child(ui::icon(if self.more { "chevron-down" } else { "chevron-right" }, t.size.icon, c.muted))
                        .child(ui::text(div(), &t.text.meta).child("Model, hooks and greeting"))
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
                    for (hooks, value, label, words) in [(true, self.hooks, "Hooks", "Install this tool's hooks"), (false, self.greeting, "Greeting", "Send the agent's greeting")] {
                        let expected = ticket.clone();
                        body = body.child(line(
                            label,
                            div().flex().items_center().gap(px(t.space.s8))
                                .child(ui::switch(label, value).on_click(cx.listener(move |form, _, _, cx| {
                                    if form.finished || !form.ticket.accepts(&expected) { return; }
                                    if hooks { form.hooks = !form.hooks; } else { form.greeting = !form.greeting; } cx.notify();
                                })))
                                .child(note(words.into()))
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
        let ready = self.prepare("validation", cx).is_some();
        // The button says which agent it starts.
        let start = match agent_name(self.name.read(cx).value()) {
            name if agent_interface_roster::valid_agent(&name) => format!("Start {name}"),
            _ => "Start agent".to_owned(),
        };
        body.track_focus(&self.focus)
            .capture_key_down(cx.listener(|form, event: &gpui::KeyDownEvent, window, cx| {
                if form.name.read(cx).composing() || form.brief.read(cx).composing() {
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
                        let name = form.name.read(cx).focus.clone();
                        let brief = form.brief.read(cx).focus.clone();
                        window.focus(
                            if name.is_focused(window) && form.custom_brief && form.more {
                                &brief
                            } else {
                                &name
                            },
                            cx,
                        );
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
                for runtime in catalog.items().iter().filter(|r| r.id != "shell") {
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
                    let ready = eligible(runtime);
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
                                form.runtime = Some(id.clone());
                                form.model = None;
                                form.changed = true;
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
                        ui::text_action("recover-pending-setup", "Earlier setups").on_click(
                            cx.listener(|form, _, _, cx| form.finish(Event::Recover, cx)),
                        ),
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
                        ui::text_action("recover-pending-setup", "Earlier setups").on_click(
                            cx.listener(|form, _, _, cx| form.finish(Event::Recover, cx)),
                        ),
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
        let ready = self.prepare("validation", cx).is_some();
        let start = if busy {
            format!("Starting {name}…")
        } else if failure.is_some() {
            "Try again".to_owned()
        } else if !tool {
            "Choose a tool".to_owned()
        } else if valid {
            format!("Start {name}")
        } else {
            "Start guide".to_owned()
        };
        let status = self
            .start
            .as_ref()
            .filter(|_| busy)
            .map(|(pane, _)| setup::stage(pane.read(cx).step(), &name));
        let problem = failure.or(install_failed).or(self.error.map(str::to_owned));
        let expected = ticket.clone();
        let tail =
            div()
                .w_full()
                .flex()
                .flex_col()
                .pt(px(t.space.s20))
                .child(
                    ui::setup_button("setup-choice-submit", start, ready, busy).on_click(
                        cx.listener(move |form, _, _, cx| {
                            if ready {
                                form.submit(&expected, cx)
                            }
                        }),
                    ),
                )
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
                    if form.name.read(cx).composing() || form.brief.read(cx).composing() {
                        return;
                    }
                    let brief = form.brief.read(cx).focus.is_focused(window);
                    match event.keystroke.key.as_str() {
                        "escape" if !form.first && !form.busy(cx) => form.finish(Event::Back, cx),
                        "enter" if !brief => form.submit(&expected, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .child(body)
            .child(tail)
    }

    /// Folder, instructions, model, hooks and greeting, in one group.
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
            ))
            .child(row(
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
                        ui::segment("brief-own", "My own", self.custom_brief).on_click(
                            cx.listener(|form, _, _, cx| {
                                form.custom_brief = true;
                                form.changed = true;
                                cx.notify();
                            }),
                        ),
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
        for (hooks, value, label, words) in [
            (true, self.hooks, "Hooks", "Install this tool's hooks"),
            (
                false,
                self.greeting,
                "Greeting",
                "The guide says hello first",
            ),
        ] {
            let expected = ticket.clone();
            group = group.child(row(
                label,
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        ui::switch(label, value).on_click(cx.listener(move |form, _, _, cx| {
                            if form.finished || !form.ticket.accepts(&expected) {
                                return;
                            }
                            if hooks {
                                form.hooks = !form.hooks;
                            } else {
                                form.greeting = !form.greeting;
                            }
                            form.changed = true;
                            cx.notify();
                        })),
                    )
                    .child(quiet(words))
                    .into_any_element(),
            ));
        }
        group
    }
}

#[cfg(test)]
mod tests;
