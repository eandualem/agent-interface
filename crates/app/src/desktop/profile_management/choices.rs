//! Explicit first-agent choices in one captured profile and operator context.
use super::{Ticket, ui};
use crate::{
    desktop::{
        design::{colors, tokens},
        name::NameField,
        onboarding::install::home_relative,
    },
    profiles::{Client, journal, setup_controller::Controller},
};
use agent_interface_roster::profiles::{
    Profile,
    runtime_catalog::{Authentication, Catalog, Runtime},
    setup::{Agent, Brief, Prepared},
};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, PathPromptOptions, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

pub(super) enum Event {
    Back,
    Recover,
    Submit,
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
            .map(|field| cx.observe(field, |_, _, cx| cx.notify()))
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
            focus: cx.focus_handle(),
        };
        form.load(cx);
        form
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.state = State::Loading;
        self.runtime = None;
        self.model = None;
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
                        // Start on a tool that's ready to work.
                        if let State::Ready { catalog, .. } = &state {
                            form.runtime = form.prefer.clone().filter(|id| {
                                catalog.items().iter().any(|r| &r.id == id && eligible(r))
                            });
                        }
                        form.state = state;
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
    /// folder of its own, both shown and editable in place.
    pub fn as_guide(&mut self, cx: &mut Context<Self>) {
        self.guide = true;
        self.name.update(cx, |field, cx| field.set_value(GUIDE, cx));
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .child(ui::button("recheck-setup", "Check again", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.load(cx)))),
                    )
            }
            State::Failed => {
                body = body.child(ui::button("retry-setup-catalog", "Check again", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.load(cx))))
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
                        ui::tool_card(format!("runtime-{id}"), runtime.display_name.clone(), card_state(runtime.authentication), false, self.runtime.as_ref() == Some(&id), false)
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
    }
}

#[cfg(test)]
mod tests;
