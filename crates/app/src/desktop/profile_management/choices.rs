//! Explicit first-agent choices in one captured profile and operator context.
use super::{Ticket, ui};
use crate::{
    desktop::{
        design::{colors, tokens},
        name::NameField,
    },
    profiles::{Client, journal, setup_controller::Controller},
};
use agent_interface_roster::profiles::{
    Profile,
    runtime_catalog::{Authentication, Catalog, Runtime},
    setup::{Agent, Brief, Prepared},
};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, PathPromptOptions, Subscription, Window, div,
    prelude::*, px,
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
    pub focus: FocusHandle,
}
impl EventEmitter<Event> for Form {}
impl Form {
    pub fn new(
        client: Client,
        library: PathBuf,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut field = NameField::filter(window, cx);
            field.set_value("guide", cx);
            field
        });
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
                    Ok(state) => form.state = state,
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
    fn prepare(&self, request_id: &str, cx: &Context<Self>) -> Option<Prepared> {
        if self.finished || self.name.read(cx).composing() || self.brief.read(cx).composing() {
            return None;
        }
        let State::Ready { catalog, .. } = &self.state else {
            return None;
        };
        freeze(
            request_id,
            &self.profile,
            catalog,
            Selection {
                name: self.name.read(cx).value().trim(),
                project: self.project.as_deref()?,
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
                        cx.notify();
                    }
                });
            }
        })
        .detach();
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
            create_directory: false,
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
fn eligible(runtime: &Runtime) -> bool {
    runtime.id != "shell"
        && runtime.available
        && runtime.setup_supported
        && runtime.setup_compatibility.compatible
}
fn authentication(value: Authentication) -> &'static str {
    match value {
        Authentication::Authenticated => "Provider probe reports authenticated.",
        Authentication::NotAuthenticated => "Provider probe reports not authenticated.",
        Authentication::Unavailable => "Authentication probe is unavailable.",
        Authentication::Unsupported => "This runtime has no supported authentication probe.",
        Authentication::NotApplicable => "This runtime does not use an account.",
        Authentication::Unknown => "Authentication has not been established.",
    }
}
impl Render for Form {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let ticket = self.ticket.0.clone();
        let mut body = div()
            .id("setup-choice-fields")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(t.space.s12))
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .child(self.profile.item().display_name.clone());
        match &self.state {
            State::Loading => body = body.child(ui::group_note("Checking saved requests and this profile's runtime catalog…")),
            State::Unchecked(_) => body = body.child(ui::group_note("A saved setup request could not be read. Open it to forget it, then check again."))
                .child(ui::button("recover-pending-setup", "Saved setup requests", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.finish(Event::Recover, cx))))
                .child(ui::button("recheck-setup", "Check again", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.load(cx)))),
            State::Failed => body = body.child(ui::button("retry-setup-catalog", "Check again", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.load(cx)))),
            State::Ready { catalog, pending, .. } => {
                if *pending > 0 {
                    body = body.child(ui::group_note(if *pending == 1 { "One earlier setup in this profile still needs attention.".to_owned() } else { format!("{pending} earlier setups in this profile still need attention.") }))
                        .child(ui::button("recover-pending-setup", "Saved setup requests", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.finish(Event::Recover, cx))));
                }
                body = body.child(ui::group_note("Choose a project and an available runtime from this profile. Setup preserves these choices for retries; it does not connect a workspace or establish account access."))
                    .child(ui::group_title("Agent name", "")).child(self.name.clone())
                    .child(ui::group_title("Project folder", ""))
                    .child(self.project.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Choose an existing project folder.".into()))
                    .child(ui::button("setup-folder", "Choose folder…", ui::Button::Secondary).on_click(cx.listener(|form, _, _, cx| form.choose_folder(cx))))
                    .child(ui::group_title("Runtime", ""));
                if !catalog.items().iter().any(eligible) { body = body.child(ui::group_note("No runtime passed this profile's setup compatibility check.")); }
                for runtime in catalog.items() {
                    let label = format!("{}{}", runtime.display_name, if eligible(runtime) { "" } else { " · unavailable for setup" });
                    let id = runtime.id.clone();
                    let expected = ticket.clone();
                    body = body.child(ui::choice_frame(format!("runtime-{id}"), self.runtime.as_ref() == Some(&id))
                        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(label))
                        .on_click(cx.listener(move |form, _, _, cx| {
                            if form.finished || !form.ticket.accepts(&expected) { return; }
                            let State::Ready { catalog, .. } = &form.state else { return; };
                            if catalog.items().iter().any(|r| r.id == id && eligible(r)) { form.runtime = Some(id.clone()); form.model = None; cx.notify(); }
                        })));
                }
                if let Some(runtime) = self.runtime() {
                    body = body.child(ui::group_note(authentication(runtime.authentication))).child(ui::group_title("Model", ""));
                    let runtime_id = runtime.id.clone();
                    for model in std::iter::once(None).chain(runtime.models.iter().cloned().map(Some)) {
                        let expected = ticket.clone();
                        let selected = self.model == model;
                        let label = model.clone().unwrap_or_else(|| "Provider default model".into());
                        let origin_runtime = runtime_id.clone();
                        body = body.child(ui::choice_frame(format!("model-{label}"), selected).child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(label))
                            .on_click(cx.listener(move |form, _, _, cx| {
                                if form.finished || !form.ticket.accepts(&expected) || form.runtime.as_ref() != Some(&origin_runtime) { return; }
                                if form.runtime().is_some_and(|r| model.as_ref().is_none_or(|m| r.models.contains(m))) { form.model = model.clone(); cx.notify(); }
                            })));
                    }
                }
                body = body.child(ui::group_title("Instructions", "")).child(
                    ui::segments()
                        .child(ui::segment("brief-coordinating", "Coordinating brief", !self.custom_brief).on_click(cx.listener(|form, _, _, cx| { form.custom_brief = false; cx.notify(); })))
                        .child(ui::segment("brief-own", "My own", self.custom_brief).on_click(cx.listener(|form, _, _, cx| { form.custom_brief = true; cx.notify(); }))),
                );
                if self.custom_brief {
                    body = body.child(self.brief.clone());
                }
                for (hooks, value, label) in [(true, self.hooks, "Install selected runtime hooks"), (false, self.greeting, "Send the agent's greeting")] {
                    let expected = ticket.clone();
                    body = body.child(ui::choice_frame(label, value).child(format!("{} · {}", label, if value { "Yes" } else { "No" })).on_click(cx.listener(move |form, _, _, cx| {
                        if form.finished || !form.ticket.accepts(&expected) { return; }
                        if hooks { form.hooks = !form.hooks; } else { form.greeting = !form.greeting; } cx.notify();
                    })));
                }
            }
        }
        if let Some(error) = self.error {
            body = body.child(ui::group_note(error));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|form, event: &gpui::KeyDownEvent, window, cx| {
                if form.name.read(cx).composing() || form.brief.read(cx).composing() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" => {
                        form.finish(Event::Back, cx);
                        cx.stop_propagation();
                    }
                    "tab" if matches!(form.state, State::Ready { .. }) => {
                        let name = form.name.read(cx).focus.clone();
                        let brief = form.brief.read(cx).focus.clone();
                        window.focus(
                            if name.is_focused(window) && form.custom_brief {
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
            .child(ui::view_header("layers", "Add agent"))
            .child(body)
            .child(
                ui::view_toolbar()
                    .px(px(t.space.s12))
                    .gap(px(t.space.s8))
                    .child(
                        ui::button("setup-choice-back", "Back", ui::Button::Secondary)
                            .on_click(cx.listener(|form, _, _, cx| form.finish(Event::Back, cx))),
                    )
                    .when(self.prepare("validation", cx).is_some(), |bar| {
                        bar.child(
                            ui::button("setup-choice-submit", "Set up agent", ui::Button::Primary)
                                .on_click(
                                    cx.listener(move |form, _, _, cx| form.submit(&ticket, cx)),
                                ),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests;
