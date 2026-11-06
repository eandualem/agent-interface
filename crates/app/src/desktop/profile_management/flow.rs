//! New profile, in three steps beside its workspace: name, sign in, first agent.
//! Each step runs the same lifecycle, selection and setup requests as before.
use super::{Open, Ticket, choices, setup, ui};
use crate::{
    desktop::{
        design::{colors, tokens},
        name::NameField,
    },
    profiles::{
        Client,
        controller::Controller,
        journal,
        selection::{Selected, Selection},
    },
};
use agent_interface_roster::profiles::{
    Profile,
    operations::{Prepared, Status},
    runtime_catalog::{Authentication, Catalog},
};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

/// The only baseline the provider can create today, so nobody has to know it.
const BASELINE: &str = "core-v1";

pub(in crate::desktop) enum Event {
    /// The new profile is ready: bind the workspace to it.
    Bind(Box<Selected>),
    /// A sign-in or agent session to show in that workspace.
    Open(Open),
    /// Finish later; whatever was made stays.
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Name,
    SignIn,
    Agent,
}

/// Why the name step stopped, and how to go on from there.
enum Stalled {
    /// The create request's outcome is not known: check that same request.
    Unconfirmed(String),
    /// Made, but not started: start it again.
    NotStarted,
    /// Nothing usable was made: a new request is safe.
    Failed,
}

struct Made {
    library: PathBuf,
    profile: Profile,
}

pub(in crate::desktop) struct Flow {
    client: Client,
    directory: Option<PathBuf>,
    /// The name the person gave the workspace, empty for the profile's own.
    pub workspace: String,
    /// The workspace it was opened from, where it shows until its profile exists.
    pub home: Option<String>,
    name: Entity<NameField>,
    _name: Subscription,
    step: Step,
    busy: Option<&'static str>,
    stalled: Option<Stalled>,
    error: Option<&'static str>,
    library: Option<PathBuf>,
    profile: Option<Profile>,
    /// Copy the person's Git name and email into the new profile. Off is a clean start.
    copy_git: bool,
    selection: Selection,
    catalog: Option<Catalog>,
    /// Sign-ins opened beside the steps.
    signing: BTreeSet<String>,
    form: Option<(Entity<choices::Form>, Subscription)>,
    setup: Option<(Entity<setup::Pane>, Subscription)>,
    ticket: Ticket,
    pub focus: FocusHandle,
}
impl EventEmitter<Event> for Flow {}

impl Flow {
    pub fn new(
        client: Client,
        directory: Option<PathBuf>,
        workspace: String,
        home: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| NameField::search(window, cx));
        let observe = cx.observe(&name, |_, _, cx| cx.notify());
        Self {
            client,
            directory,
            workspace,
            home,
            name,
            _name: observe,
            step: Step::Name,
            busy: None,
            stalled: None,
            error: None,
            library: None,
            profile: None,
            copy_git: false,
            selection: Selection::default(),
            catalog: None,
            signing: BTreeSet::new(),
            form: None,
            setup: None,
            ticket: Ticket::default(),
            focus: cx.focus_handle(),
        }
    }

    /// The deepest focusable part of the current step.
    pub fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        if let Some((setup, _)) = &self.setup {
            return setup.read(cx).focus.clone();
        }
        if let Some((form, _)) = &self.form {
            return form.read(cx).focus.clone();
        }
        if self.step == Step::Name {
            return self.name.read(cx).focus.clone();
        }
        self.focus.clone()
    }

    pub fn profile_id(&self) -> Option<&str> {
        self.profile
            .as_ref()
            .map(|profile| profile.item().profile_id.as_str())
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.ticket.cancel();
        self.selection.cancel();
        cx.emit(Event::Close);
    }

    /// Continue from the name: create the profile, start it, then connect.
    fn create(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.step != Step::Name || self.name.read(cx).composing() {
            return;
        }
        let display = self.name.read(cx).value().trim().to_owned();
        let resume = self.stalled.take();
        let request = uuid::Uuid::new_v4().simple().to_string();
        let prepared = match &resume {
            Some(Stalled::Unconfirmed(_) | Stalled::NotStarted) => None,
            _ => match if self.copy_git {
                Prepared::create_copying_git_identity(&request, &display, BASELINE)
            } else {
                Prepared::create(&request, &display, BASELINE)
            } {
                Ok(prepared) => Some(prepared),
                Err(_) => {
                    self.error = Some("Give the profile a name.");
                    cx.notify();
                    return;
                }
            },
        };
        self.error = None;
        self.busy = Some(if matches!(resume, Some(Stalled::NotStarted)) {
            "Starting…"
        } else {
            "Creating…"
        });
        let (client, directory) = (self.client.clone(), self.directory.clone());
        let made = self.profile.clone();
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let work = cx
            .background_executor()
            .spawn(async move { make(client, directory, prepared, resume, made, &worker) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |flow, cx| {
                if !flow.ticket.accepts(&ticket) {
                    return;
                }
                match result {
                    Ok(made) => {
                        flow.library = Some(made.library);
                        flow.profile = Some(made.profile);
                        flow.connect(cx);
                    }
                    Err((stalled, profile, message)) => {
                        flow.busy = None;
                        if let Some(profile) = profile {
                            flow.profile = Some(*profile);
                        }
                        flow.stalled = Some(stalled);
                        flow.error = Some(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Verify the started profile, then hand it to the workspace.
    fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.profile_id().map(str::to_owned) else {
            return;
        };
        self.busy = Some("Connecting…");
        let attempt = match self.selection.begin(&self.client, &id) {
            Ok(attempt) => attempt,
            Err(error) => {
                self.busy = None;
                self.stalled = Some(Stalled::NotStarted);
                self.error = Some(crate::desktop::profile_picker::message(&error));
                return;
            }
        };
        let work = cx.background_executor().spawn(async move { attempt.run() });
        cx.spawn(async move |this, cx| {
            let completion = work.await;
            let _ = this.update(cx, |flow, cx| {
                let Some(result) = flow.selection.accept(completion) else {
                    return;
                };
                flow.busy = None;
                match result {
                    Ok(selected) => {
                        cx.emit(Event::Bind(Box::new(selected)));
                        flow.step = Step::SignIn;
                        flow.read_catalog(cx);
                    }
                    Err(error) => {
                        flow.stalled = Some(Stalled::NotStarted);
                        flow.error = Some(crate::desktop::profile_picker::message(&error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Read which tools the profile has, once. Discovery does not check accounts,
    /// so a sign-in's result is never reported and is not waited for.
    fn read_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(identity) = self
            .profile
            .as_ref()
            .and_then(|profile| profile.expected_identity().ok())
        else {
            return;
        };
        let client = self.client.clone();
        let ticket = self.ticket.0.clone();
        let worker = ticket.clone();
        let work = cx
            .background_executor()
            .spawn(async move { client.runtimes(&identity, &|| worker.load(Ordering::Acquire)) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |flow, cx| {
                if !flow.ticket.accepts(&ticket) || flow.step != Step::SignIn {
                    return;
                }
                match result {
                    Ok(catalog) => {
                        flow.catalog = Some(catalog);
                        flow.error = None;
                    }
                    Err(_) => flow.error = Some("This profile's tools could not be read."),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_in(&mut self, runtime: String, cx: &mut Context<Self>) {
        let Some(profile_id) = self.profile_id().map(str::to_owned) else {
            return;
        };
        self.signing.insert(runtime.clone());
        cx.emit(Event::Open(Open::SignIn {
            profile_id,
            runtime,
        }));
        cx.notify();
    }

    fn first_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(library), Some(profile)) = (self.library.clone(), self.profile.clone()) else {
            return;
        };
        self.step = Step::Agent;
        self.signing.clear();
        self.ticket.begin();
        let client = self.client.clone();
        let form = cx.new(|cx| {
            choices::Form::new(
                client,
                library,
                profile,
                "Skip, open a terminal",
                window,
                cx,
            )
        });
        window.focus(&form.read(cx).focus.clone(), cx);
        let events = cx.subscribe_in(&form, window, |flow, form, event, window, cx| {
            if !flow.form.as_ref().is_some_and(|(active, _)| active == form) {
                return;
            }
            let setup = match event {
                choices::Event::Back => {
                    flow.close(cx);
                    return;
                }
                choices::Event::Recover => {
                    let Some(controller) = form.read(cx).controller() else {
                        return;
                    };
                    cx.new(|cx| setup::Pane::from_controller(controller, cx))
                }
                choices::Event::Submit => {
                    let Some((prepared, controller)) =
                        form.update(cx, |form, _| form.take_prepared())
                    else {
                        return;
                    };
                    cx.new(|cx| setup::Pane::from_prepared(controller, prepared, cx))
                }
            };
            flow.form = None;
            flow.show_setup(setup, window, cx);
        });
        self.form = Some((form, events));
        cx.notify();
    }

    fn show_setup(
        &mut self,
        setup: Entity<setup::Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&setup.read(cx).focus.clone(), cx);
        cx.subscribe(&setup, |flow, _, open: &Open, cx| {
            let agent = matches!(open, Open::Agent { .. });
            cx.emit(Event::Open(open.clone()));
            if agent {
                flow.close(cx);
            }
        })
        .detach();
        let back = cx.subscribe(&setup, |flow, _, _: &super::Back, cx| flow.close(cx));
        self.setup = Some((setup, back));
        cx.notify();
    }
}

type Failure = (Stalled, Option<Box<Profile>>, &'static str);

/// Create (or confirm) the profile, then start it. Runs off the main thread.
fn make(
    client: Client,
    directory: Option<PathBuf>,
    prepared: Option<Prepared>,
    resume: Option<Stalled>,
    made: Option<Profile>,
    live: &AtomicBool,
) -> Result<Made, Failure> {
    let alive = || live.load(Ordering::Acquire);
    let storage = "Private storage for profile requests is unavailable.";
    let library = crate::library::location::library_directory(directory.as_deref())
        .map_err(|_| (Stalled::Failed, None, storage))?;
    let controller = journal::Journal::open(&library.join("profile-operations"))
        .map(|journal| Controller::new(journal, &client))
        .map_err(|_| (Stalled::Failed, None, storage))?;
    let profile = match (made, resume) {
        (Some(profile), Some(Stalled::NotStarted)) => profile,
        (_, resume) => {
            let receipt = match (prepared, resume) {
                (Some(prepared), _) => {
                    let key = Controller::key(prepared.request_id());
                    let result = controller
                        .enqueue(prepared)
                        .and_then(|_| controller.submit(&key, &alive));
                    result.map_err(|_| unconfirmed(key))?
                }
                (None, Some(Stalled::Unconfirmed(key))) => controller
                    .refresh(&key, &alive)
                    .map_err(|_| unconfirmed(key.clone()))?,
                _ => return Err((Stalled::Failed, None, "Give the profile a name.")),
            };
            match (receipt.status, receipt.identity) {
                (Status::Succeeded, Some(identity)) => client
                    .show(&identity.profile_id, &alive)
                    .map_err(|_| unconfirmed(Controller::key(&receipt.request_id)))?,
                (Status::Pending, _) | (Status::Succeeded, None) => {
                    return Err(unconfirmed(Controller::key(&receipt.request_id)));
                }
                (Status::Failed, _) => {
                    return Err((
                        Stalled::Failed,
                        None,
                        if receipt.error_code.as_deref() == Some("git_identity_unavailable") {
                            "Your Git name and email aren't both set. Set them, or turn this off."
                        } else {
                            "The profile could not be made. Try again."
                        },
                    ));
                }
            }
        }
    };
    let not_started = |profile: &Profile| {
        (
            Stalled::NotStarted,
            Some(Box::new(profile.clone())),
            "The profile was made but didn't start. Try again.",
        )
    };
    let request = uuid::Uuid::new_v4().simple().to_string();
    let start = Prepared::start(&request, &profile).map_err(|_| not_started(&profile))?;
    let key = Controller::key(start.request_id());
    let receipt = controller
        .enqueue(start)
        .and_then(|_| controller.submit(&key, &alive))
        .map_err(|_| not_started(&profile))?;
    if receipt.status != Status::Succeeded {
        return Err(not_started(&profile));
    }
    Ok(Made { library, profile })
}

fn unconfirmed(key: String) -> Failure {
    (
        Stalled::Unconfirmed(key),
        None,
        "It isn't confirmed yet whether the profile was made. Check again.",
    )
}

impl Render for Flow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let meta =
            |text: SharedString| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let title = self
            .profile
            .as_ref()
            .map(|profile| profile.item().display_name.clone())
            .unwrap_or_else(|| self.name.read(cx).value().trim().to_owned());
        let id = self.profile_id().unwrap_or("new").to_owned();
        let header = div()
            .flex()
            .items_center()
            .gap(px(t.space.s12))
            .pb(px(t.space.s20))
            .child(ui::profile_mark(&id, &title, false, t.size.slot))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s2))
                    .child(
                        ui::text(div().text_color(c.text), &t.text.group_title).child(
                            if self.profile.is_some() {
                                title.clone()
                            } else {
                                "New profile".into()
                            },
                        ),
                    )
                    .child(meta(if self.profile.is_some() {
                        "New profile".into()
                    } else {
                        "Its own accounts, agents, skills and policies, kept apart from your other work.".into()
                    })),
            );
        let steps = [
            (Step::Name, "Name"),
            (Step::SignIn, "Sign in"),
            (Step::Agent, "First agent"),
        ];
        let at = steps
            .iter()
            .position(|(step, _)| *step == self.step)
            .unwrap_or(0);
        let mut column = div().flex().flex_col().child(header);
        for (index, (step, label)) in steps.into_iter().enumerate() {
            let state = if index < at {
                ui::Step::Done
            } else if index == at {
                if self.error.is_some() && step == Step::Name {
                    ui::Step::Waits
                } else {
                    ui::Step::Now
                }
            } else {
                ui::Step::Ahead
            };
            let last = index + 1 == 3;
            let summary: Option<SharedString> = match (state, step) {
                (ui::Step::Done, Step::Name) => Some(title.clone().into()),
                (ui::Step::Done, Step::SignIn) => Some(
                    self.catalog
                        .as_ref()
                        .map(|catalog| {
                            catalog
                                .items()
                                .iter()
                                .filter(|r| r.authentication == Authentication::Authenticated)
                                .map(|r| r.display_name.clone())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .filter(|names| !names.is_empty())
                        .unwrap_or_else(|| "Skipped".into())
                        .into(),
                ),
                _ => None,
            };
            let mut content = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s10))
                .pb(px(t.space.s20))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .min_h(px(t.size.plan_dot))
                        .child(
                            ui::text(
                                div().text_color(if state == ui::Step::Ahead {
                                    c.muted
                                } else {
                                    c.text
                                }),
                                &t.text.row_selected,
                            )
                            .child(label),
                        )
                        .children(summary.map(meta)),
                );
            if index == at {
                content = content.child(self.step_body(cx));
            }
            column = column.child(
                div()
                    .flex()
                    .gap(px(t.space.s12))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(t.space.s4))
                            .child(ui::step_dot(state))
                            .when(!last, |el| {
                                el.child(div().flex_1().w(px(t.size.hairline)).bg(c.line_strong))
                            }),
                    )
                    .child(content),
            );
        }
        let hints: &[(&'static str, &'static str)] = match self.step {
            Step::Name => &[("↵", "continue"), ("esc", "cancel")],
            Step::SignIn => &[("esc", "finish later")],
            Step::Agent => &[("⌘↵", "start agent"), ("esc", "finish later")],
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .rounded(px(t.radius.pane))
            .border(px(t.size.hairline))
            .border_color(c.edge)
            .overflow_hidden()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|flow, event: &gpui::KeyDownEvent, _, cx| {
                if flow.name.read(cx).composing() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" if flow.form.is_none() && flow.setup.is_none() => flow.close(cx),
                    "enter" if flow.step == Step::Name => flow.create(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("profile-flow")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(t.space.s28))
                    .pt(px(t.space.s24))
                    .child(column.max_w(px(t.size.profile_flow))),
            )
            .child(ui::view_footer(hints, ""))
    }
}

impl Flow {
    fn step_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (t, c) = (tokens(), colors());
        let meta =
            |text: SharedString| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let error = self
            .error
            .map(|error| ui::text(div().text_color(c.attention_text), &t.text.meta).child(error));
        match self.step {
            Step::Name => {
                let label = match (&self.busy, &self.stalled) {
                    (Some(busy), _) => *busy,
                    (None, Some(Stalled::Unconfirmed(_))) => "Check again",
                    (None, Some(Stalled::NotStarted | Stalled::Failed)) => "Try again",
                    (None, None) => "Continue",
                };
                let empty = self.name.read(cx).value().is_empty();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s10))
                    .child(
                        div()
                            .relative()
                            .child(self.name.clone())
                            .when(empty, |el| {
                                el.child(
                                    ui::text(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left(px(t.space.s6 + t.size.hairline))
                                            .h(px(t.size.jump_field))
                                            .flex()
                                            .items_center()
                                            .text_color(c.faint),
                                        &t.text.body,
                                    )
                                    .child("Profile name, such as Client work"),
                                )
                            }),
                    )
                    .child(meta(
                        "Starts with the standard skills and policies. Nothing is copied from your other profiles.".into(),
                    ))
                    .when(
                        !matches!(
                            self.stalled,
                            Some(Stalled::Unconfirmed(_) | Stalled::NotStarted)
                        ),
                        |el| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(t.space.s8))
                                    .child(ui::switch("profile-flow-git", self.copy_git).on_click(
                                        cx.listener(|flow, _, _, cx| {
                                            if flow.busy.is_none() {
                                                flow.copy_git = !flow.copy_git;
                                                cx.notify();
                                            }
                                        }),
                                    ))
                                    .child(meta("Use my Git name and email".into())),
                            )
                        },
                    )
                    .children(error)
                    .child(
                        div().flex().child(
                            ui::button("profile-flow-continue", label, ui::Button::Primary)
                                .when(self.busy.is_some(), |el| el.opacity(0.6).cursor_default())
                                .on_click(cx.listener(|flow, _, _, cx| flow.create(cx))),
                        ),
                    )
                    .into_any_element()
            }
            Step::SignIn => {
                let mut card = ui::group_card();
                match &self.catalog {
                    None => {
                        card = card.child(
                            div()
                                .p(px(t.space.s12))
                                .child(meta("Reading this profile's tools…".into())),
                        )
                    }
                    Some(catalog) => {
                        for (index, runtime) in catalog
                            .items()
                            .iter()
                            .filter(|runtime| runtime.id != "shell")
                            .enumerate()
                        {
                            let id = runtime.id.clone();
                            let right = if !runtime.available {
                                meta("Not installed".into()).into_any_element()
                            } else if runtime.authentication == Authentication::Authenticated {
                                meta("Signed in".into()).into_any_element()
                            } else if runtime.authentication == Authentication::NotApplicable {
                                meta("No account needed".into()).into_any_element()
                            } else {
                                // The provider may never report the result, so an
                                // opened sign-in stays open to retry.
                                let opened = self.signing.contains(&id);
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(t.space.s8))
                                    .when(opened, |el| el.child(meta("Opened beside".into())))
                                    .child(
                                        ui::button(
                                            SharedString::from(format!("sign-in-{id}")),
                                            if opened { "Sign in again" } else { "Sign in" },
                                            ui::Button::Secondary,
                                        )
                                        .on_click(
                                            cx.listener(move |flow, _, _, cx| {
                                                flow.sign_in(id.clone(), cx)
                                            }),
                                        ),
                                    )
                                    .into_any_element()
                            };
                            card = card.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(t.space.s10))
                                    .h(px(t.size.group_row + t.space.s8))
                                    .px(px(t.space.s14))
                                    .when(index > 0, |el| {
                                        el.border_t(px(t.size.hairline)).border_color(c.line)
                                    })
                                    .when(!runtime.available, |el| el.opacity(0.6))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_color(c.text)
                                            .child(runtime.display_name.clone()),
                                    )
                                    .child(right),
                            );
                        }
                    }
                }
                let any_signed = self.catalog.as_ref().is_some_and(|catalog| {
                    catalog
                        .items()
                        .iter()
                        .any(|r| r.authentication == Authentication::Authenticated)
                }) || !self.signing.is_empty();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s10))
                    .child(meta("Accounts you add here stay in this profile.".into()))
                    .child(card)
                    .children(error)
                    .child(
                        div().flex().items_center().gap(px(t.space.s12)).child(
                            ui::button(
                                "profile-flow-agent",
                                if any_signed {
                                    "Continue"
                                } else {
                                    "Skip for now"
                                },
                                if any_signed {
                                    ui::Button::Primary
                                } else {
                                    ui::Button::Secondary
                                },
                            )
                            .on_click(
                                cx.listener(|flow, _, window, cx| flow.first_agent(window, cx)),
                            ),
                        ),
                    )
                    .into_any_element()
            }
            Step::Agent => {
                let body = div().flex().flex_col().gap(px(t.space.s10));
                if let Some((setup, _)) = &self.setup {
                    return body.child(setup.clone()).into_any_element();
                }
                body.child(meta(
                    "It works in this folder and can help you set up the rest.".into(),
                ))
                .children(self.form.as_ref().map(|(form, _)| form.clone()))
                .into_any_element()
            }
        }
    }
}
