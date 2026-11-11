//! A new profile in two screens, each the one first-run shape: its name,
//! with the Git name and email its agents commit as, then its guide. First
//! run makes the person's first profile with it; New profile shows the same
//! screens later, inside the content area, with a way to cancel.
use super::{Open, Ticket, choices, setup, ui};
use crate::{
    desktop::{
        design::{colors, tokens},
        name::NameField,
        onboarding::{self, scene},
    },
    profiles::{
        Client,
        controller::{self, Controller, Phase},
        journal,
        selection::{Selected, Selection},
    },
};
use agent_interface_roster::profiles::{
    Connection, Profile,
    operations::{Prepared, Receipt, Status},
};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// The only baseline the provider can create today, so nobody has to know it.
const BASELINE: &str = "core-v1";
/// The name a first profile starts with; the person can change it.
const FIRST_NAME: &str = "Personal";
const GIT_MISSING: &str =
    "This computer's Git name and email aren't both set. Fill them in, or leave both empty.";

pub(in crate::desktop) enum Event {
    /// The new profile is ready: bind the workspace to it.
    Bind(Box<Selected>),
    /// The guide is up: open its pane in that workspace.
    Open(Open),
    /// No guide for now: leave the workspace on a terminal.
    Terminal,
    /// Cancelled, or finished; whatever was made stays.
    Close,
    /// A request to make the profile, under this key, is about to be sent,
    /// with the Git name and email typed for it.
    Creating(String, Option<(String, String)>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Name,
    Guide,
}

/// Why the name step stopped, and how to go on from there.
enum Stalled {
    /// The create request's outcome is not known: check that same request.
    Unconfirmed(String),
    /// Made, but not started: start it again.
    NotStarted,
    /// Nothing usable was made: a new request is safe.
    Failed,
    /// A profile made before couldn't be read: read it again.
    Lookup(String),
}

struct Made {
    library: PathBuf,
    profile: Profile,
}

/// The Git name and email a new profile's agents commit as. A profile starts
/// with none, so this computer's own are offered, and can be changed.
struct Git {
    /// This computer's own name and email, once read; either may be empty.
    computer: Option<(String, String)>,
    /// The two fields are open.
    open: bool,
    name: Entity<NameField>,
    email: Entity<NameField>,
}

/// What the new profile's agents commit as.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    /// This computer's own, which the provider copies in.
    Computer,
    /// None yet; the profile can set them later.
    None,
    /// Typed here, set inside the profile once it is running.
    Typed(String, String),
}

/// A first-run screen's parts, for the stage that draws them.
pub(in crate::desktop) struct Parts {
    /// Changes with the screen, so the words can cross-fade.
    pub key: &'static str,
    pub title: SharedString,
    pub line: SharedString,
    pub body: AnyElement,
}

pub(in crate::desktop) struct Flow {
    client: Client,
    directory: Option<PathBuf>,
    /// The name the person gave the workspace, empty for the profile's own.
    pub workspace: String,
    /// The workspace it was opened from, where it shows until its profile exists.
    pub home: Option<String>,
    name: Entity<NameField>,
    _fields: Vec<Subscription>,
    screen: Screen,
    busy: bool,
    stalled: Option<Stalled>,
    error: Option<&'static str>,
    library: Option<PathBuf>,
    profile: Option<Profile>,
    git: Git,
    /// The verified connection, once the profile is bound.
    connection: Option<Connection>,
    selection: Selection,
    /// The guide's choices, waiting for a window to be made in.
    pending_guide: Option<(PathBuf, Profile)>,
    form: Option<(Entity<choices::Form>, Subscription)>,
    /// Earlier starts of the guide, opened from its screen.
    setup: Option<(Entity<setup::Pane>, Subscription)>,
    ticket: Ticket,
    pub focus: FocusHandle,
    /// This is the person's first profile, made as the app is set up.
    first: bool,
    /// What the profile's agents commit as, fixed when Continue was pressed.
    chosen: Option<Identity>,
    /// The screen last drawn, and when it arrived, for its fade.
    arrived: Option<(&'static str, std::time::Instant)>,
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
        let name = cx.new(|cx| NameField::bare("", window, cx));
        let git_name = cx.new(|cx| NameField::bare("", window, cx));
        let git_email = cx.new(|cx| NameField::bare("", window, cx));
        let fields = [&name, &git_name, &git_email]
            .into_iter()
            .map(|field| cx.observe(field, |_, _, cx| cx.notify()))
            .collect();
        let work = cx
            .background_executor()
            .spawn(async { computer_identity() });
        cx.spawn(async move |this, cx| {
            let computer = work.await;
            let _ = this.update(cx, |flow, cx| flow.found_identity(computer, cx));
        })
        .detach();
        window.focus(&name.read(cx).focus.clone(), cx);
        Self {
            client,
            directory,
            workspace,
            home,
            name,
            _fields: fields,
            screen: Screen::Name,
            busy: false,
            stalled: None,
            error: None,
            library: None,
            profile: None,
            git: Git {
                computer: None,
                open: false,
                name: git_name,
                email: git_email,
            },
            connection: None,
            selection: Selection::default(),
            pending_guide: None,
            form: None,
            setup: None,
            ticket: Ticket::default(),
            focus: cx.focus_handle(),
            first: false,
            chosen: None,
            arrived: None,
        }
    }

    /// Make this the person's first profile: it says so, suggests a name
    /// ready to type over, and has nothing to go back to.
    pub fn for_first_run(&mut self, cx: &mut Context<Self>) {
        self.first = true;
        self.name.update(cx, |field, cx| {
            field.set_value(FIRST_NAME, cx);
            field.select_all(cx);
        });
    }

    /// Carry on with the first profile made before, once it is read. Only a
    /// list without it says it is gone; a failed read keeps it to read again.
    pub fn resume_saved(&mut self, id: String, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        let client = self.client.clone();
        let work = cx.background_executor().spawn(async move {
            let profile = client.show(&id, &|| true);
            let gone = profile.is_err()
                && client
                    .list(&|| true)
                    .is_ok_and(|items| items.iter().all(|item| item.profile_id != id));
            (id, profile, gone)
        });
        cx.spawn(async move |this, cx| {
            let (id, profile, gone) = work.await;
            let _ = this.update(cx, |flow, cx| {
                match profile {
                    Ok(profile) => return flow.resume(profile, cx),
                    Err(_) if gone => {}
                    Err(_) => {
                        flow.stalled = Some(Stalled::Lookup(id));
                        flow.error = Some("The profile made before couldn't be read. Check again.");
                    }
                }
                flow.busy = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// First run closed while its profile was being made: Check again looks
    /// up that same request rather than making another profile.
    pub fn resume_request(
        &mut self,
        key: String,
        typed: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        // The request's own Git choice: typed values are set once it is made.
        self.chosen = Some(match typed {
            Some((name, email)) => Identity::Typed(name, email),
            None => Identity::None,
        });
        self.stalled = Some(Stalled::Unconfirmed(key));
        self.error = Some("Setup closed while the profile was being made. Check again.");
        cx.notify();
    }

    /// Carry on with a first profile made before the app last closed: connect
    /// to it and go to its guide.
    pub fn resume(&mut self, profile: Profile, cx: &mut Context<Self>) {
        let display = profile.item().display_name.clone();
        self.name
            .update(cx, |field, cx| field.set_value(&display, cx));
        match crate::library::location::create_library_directory(self.directory.as_deref()) {
            Ok(library) => self.library = Some(library),
            Err(_) => {
                self.busy = false;
                self.stalled = Some(Stalled::NotStarted);
                self.profile = Some(profile);
                self.error = Some("Private storage for profile requests is unavailable.");
                cx.notify();
                return;
            }
        }
        self.profile = Some(profile);
        self.connect(cx);
    }

    /// This computer's Git name and email are known. With neither set, the
    /// fields open, since there is nothing to copy.
    fn found_identity(&mut self, computer: (String, String), cx: &mut Context<Self>) {
        let (name, email) = computer.clone();
        self.git
            .name
            .update(cx, |field, cx| field.set_value(&name, cx));
        self.git
            .email
            .update(cx, |field, cx| field.set_value(&email, cx));
        if name.is_empty() || email.is_empty() {
            self.git.open = true;
        }
        self.git.computer = Some(computer);
        cx.notify();
    }

    /// What the new profile's agents will commit as.
    fn identity(&self, cx: &gpui::App) -> Identity {
        let typed = (
            self.git.name.read(cx).value().trim().to_owned(),
            self.git.email.read(cx).value().trim().to_owned(),
        );
        let complete = |(name, email): &(String, String)| !name.is_empty() && !email.is_empty();
        match &self.git.computer {
            None => Identity::None,
            Some(computer) if !self.git.open || &typed == computer => {
                if complete(computer) {
                    Identity::Computer
                } else {
                    Identity::None
                }
            }
            Some(_) if typed.0.is_empty() && typed.1.is_empty() => Identity::None,
            Some(_) => Identity::Typed(typed.0, typed.1),
        }
    }

    /// The deepest focusable part of the current screen.
    pub fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        if let Some((setup, _)) = &self.setup {
            return setup.read(cx).focus.clone();
        }
        if let Some((form, _)) = &self.form {
            return form.read(cx).name_focus(cx);
        }
        if self.screen == Screen::Name {
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
        if !self.busy
            && let Some(Stalled::Lookup(id)) = &self.stalled
        {
            let id = id.clone();
            self.stalled = None;
            return self.resume_saved(id, cx);
        }
        // What its agents commit as is decided before the profile is made.
        if self.busy
            || self.screen != Screen::Name
            || self.git.computer.is_none()
            || self.name.read(cx).composing()
        {
            return;
        }
        let display = self.name.read(cx).value().trim().to_owned();
        if display.is_empty() {
            return;
        }
        let resume = self.stalled.take();
        let request = uuid::Uuid::new_v4().simple().to_string();
        let prepared = match &resume {
            Some(Stalled::Unconfirmed(_) | Stalled::NotStarted | Stalled::Lookup(_)) => None,
            _ => match if self.identity(cx) == Identity::Computer {
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
        if let Some(prepared) = &prepared {
            let typed = match self.identity(cx) {
                Identity::Typed(name, email) => Some((name, email)),
                _ => None,
            };
            cx.emit(Event::Creating(
                Controller::key(prepared.request_id()),
                typed,
            ));
        }
        self.error = None;
        self.busy = true;
        // One choice for the whole operation, whatever the fields show later.
        // A new create request takes the fields as they are now; going on
        // with the same request or profile keeps the earlier choice.
        if prepared.is_some() || self.chosen.is_none() {
            self.chosen = Some(self.identity(cx));
        }
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
                        flow.busy = false;
                        if let Some(profile) = profile {
                            flow.profile = Some(*profile);
                        }
                        if message == GIT_MISSING {
                            flow.git.open = true;
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

    /// Verify the started profile, hand it to the workspace, then go on to
    /// its guide.
    fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.profile_id().map(str::to_owned) else {
            return;
        };
        self.busy = true;
        let attempt = match self.selection.begin(&self.client, &id) {
            Ok(attempt) => attempt,
            Err(error) => {
                self.busy = false;
                self.stalled = Some(Stalled::NotStarted);
                self.error = Some(crate::desktop::profile_picker::message(&error));
                return;
            }
        };
        let identity = self.chosen.clone().unwrap_or(Identity::None);
        let work = cx.background_executor().spawn(async move { attempt.run() });
        cx.spawn(async move |this, cx| {
            let completion = work.await;
            let _ = this.update(cx, |flow, cx| {
                let Some(result) = flow.selection.accept(completion) else {
                    return;
                };
                flow.busy = false;
                match result {
                    Ok(selected) => {
                        let connection = selected.connection().clone();
                        if let Identity::Typed(name, email) = identity {
                            set_identity(connection.clone(), name, email, cx);
                        }
                        flow.connection = Some(connection);
                        cx.emit(Event::Bind(Box::new(selected)));
                        flow.screen = Screen::Guide;
                        flow.guide(cx);
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
        cx.notify();
    }

    /// The guide's screen: its choices, made once the profile is bound.
    fn guide(&mut self, cx: &mut Context<Self>) {
        let (Some(library), Some(profile)) = (self.library.clone(), self.profile.clone()) else {
            return;
        };
        self.ticket.begin();
        // The form is made in the window that shows it, on the next frame.
        self.pending_guide = Some((library, profile));
        cx.notify();
    }

    /// Make the guide's choices, focused on its name.
    fn show_guide(
        &mut self,
        library: PathBuf,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        let (first, connection) = (self.first, self.connection.clone());
        let skip = if first {
            "Use a terminal instead"
        } else {
            "Skip for now"
        };
        let form = cx.new(|cx| {
            let mut form = choices::Form::new(client, library, profile, skip, None, window, cx);
            form.as_guide(first, connection, window, cx);
            form
        });
        window.focus(&form.read(cx).name_focus(cx), cx);
        let events = cx.subscribe_in(&form, window, |flow, form, event, window, cx| {
            if !flow.form.as_ref().is_some_and(|(active, _)| active == form) {
                return;
            }
            match event {
                choices::Event::Back => {
                    cx.emit(Event::Terminal);
                    flow.close(cx);
                }
                choices::Event::Opened(open) => {
                    cx.emit(Event::Open(open.clone()));
                    flow.close(cx);
                }
                choices::Event::Recover => {
                    let Some(controller) = form.read(cx).controller() else {
                        return;
                    };
                    let profile = Some(form.read(cx).profile_id().to_owned());
                    let setup = cx.new(|cx| setup::Pane::from_controller(controller, profile, cx));
                    // The choices stay behind it, so Back returns to them as they were.
                    flow.show_setup(setup, window, cx);
                }
                // A guide starts itself.
                choices::Event::Submit => {}
            }
        });
        self.form = Some((form, events));
    }

    /// Earlier starts of the guide; Back returns to its choices.
    fn show_setup(
        &mut self,
        setup: Entity<setup::Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&setup.read(cx).focus.clone(), cx);
        cx.subscribe(&setup, |_, _, open: &Open, cx| {
            cx.emit(Event::Open(open.clone()))
        })
        .detach();
        cx.observe(&setup, |_, _, cx| cx.notify()).detach();
        let back = cx.subscribe_in(&setup, window, |flow, _, _: &super::Back, window, cx| {
            flow.setup = None;
            match &flow.form {
                Some((form, _)) => {
                    form.update(cx, |form, cx| form.returned(cx));
                    window.focus(&form.read(cx).name_focus(cx), cx);
                }
                None => flow.guide(cx),
            }
            cx.notify();
        });
        self.setup = Some((setup, back));
        cx.notify();
    }

    /// The current screen's title, line and body, for the stage to draw.
    pub fn parts(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Parts {
        if let Some((library, profile)) = self.pending_guide.take() {
            self.show_guide(library, profile, window, cx);
        }
        match self.screen {
            Screen::Name => Parts {
                key: "name",
                title: if self.first {
                    "Name your first profile"
                } else {
                    "New profile"
                }
                .into(),
                line: "A profile keeps its own sign-ins, agents and settings, apart from your other work.".into(),
                body: self.name_body(window, cx).into_any_element(),
            },
            Screen::Guide => Parts {
                key: "guide",
                title: if self.first {
                    "Start your guide"
                } else {
                    "Start its guide"
                }
                .into(),
                line: "Your guide learns what you\u{2019}re working on and proposes the agents for it. You approve each one.".into(),
                body: match (&self.setup, &self.form) {
                    (Some((setup, _)), _) => div()
                        .w_full()
                        .h(px(tokens().size.setup_stage))
                        .child(setup.clone())
                        .into_any_element(),
                    (None, Some((form, _))) => form.clone().into_any_element(),
                    (None, None) => div().into_any_element(),
                },
            },
        }
    }

    /// The name screen: the profile's name, what its agents commit as, and
    /// Continue.
    fn name_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (t, c) = (tokens(), colors());
        let meta =
            |text: SharedString, color| ui::text(div().text_color(color), &t.text.meta).child(text);
        let typed = self.name.read(cx).value().trim().to_owned();
        let label = match (&self.busy, &self.stalled) {
            (true, _) => format!(
                "Setting up {}…",
                if typed.is_empty() { "profile" } else { &typed }
            ),
            (false, Some(Stalled::Unconfirmed(_) | Stalled::Lookup(_))) => "Check again".to_owned(),
            (false, Some(Stalled::NotStarted | Stalled::Failed)) => "Try again".to_owned(),
            (false, None) => "Continue".to_owned(),
        };
        let name_field = ui::setup_field(self.name.read(cx).focus.is_focused(window))
            .child(ui::profile_mark(
                "new",
                if typed.is_empty() { "?" } else { &typed },
                true,
                t.size.profile_mark,
            ))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .child(self.name.clone())
                    .when(typed.is_empty() && !self.first, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .h_full()
                                .flex()
                                .items_center()
                                .text_color(c.faint)
                                .child("Profile name, such as Client work"),
                        )
                    }),
            );
        // What its agents commit as: this computer's own, which can change.
        let locked = self.busy
            || matches!(
                self.stalled,
                Some(Stalled::Unconfirmed(_) | Stalled::NotStarted | Stalled::Lookup(_))
            );
        let git = if self.git.open {
            let field = |entity: &Entity<NameField>, hint: &'static str| {
                let empty = entity.read(cx).value().is_empty();
                ui::setup_field(entity.read(cx).focus.is_focused(window)).child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .child(entity.clone())
                        .when(empty, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .text_color(c.faint)
                                    .child(hint),
                            )
                        }),
                )
            };
            let empty = self.git.name.read(cx).value().is_empty()
                && self.git.email.read(cx).value().is_empty();
            div()
                .pt(px(t.space.s16))
                .flex()
                .flex_col()
                .child(
                    div()
                        .pb(px(t.space.s6))
                        .child(meta("Git commits as".into(), c.muted)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(t.space.s8))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(field(&self.git.name, "Your name")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(field(&self.git.email, "Email")),
                        ),
                )
                .when(empty, |el| {
                    el.child(div().pt(px(t.space.s6)).child(meta(
                        "Leave these empty to set them later, inside the profile.".into(),
                        c.muted,
                    )))
                })
                .into_any_element()
        } else if let Some((name, email)) = &self.git.computer {
            ui::text(
                div()
                    .pt(px(t.space.s12))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(t.space.s8))
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s4))
                    .whitespace_nowrap()
                    .child(div().flex_shrink_0().child("Git commits as"))
                    .child(div().flex_shrink_0().text_color(c.text).child(name.clone()))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(format!("<{email}>")),
                    ),
            )
            .when(!locked, |el| {
                el.child(
                    ui::text_action("git-change", "Change").on_click(cx.listener(
                        |flow, _, window, cx| {
                            flow.git.open = true;
                            window.focus(&flow.git.name.read(cx).focus.clone(), cx);
                            cx.notify();
                        },
                    )),
                )
            })
            .into_any_element()
        } else {
            div().into_any_element()
        };
        div()
            .id("profile-name")
            .w_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .child(name_field)
            .child(git)
            .child(
                div().pt(px(t.space.s20)).child(
                    ui::setup_button(
                        "profile-flow-continue",
                        label,
                        !typed.is_empty() && self.git.computer.is_some(),
                        self.busy,
                    )
                    .on_click(cx.listener(|flow, _, _, cx| flow.create(cx))),
                ),
            )
            .children(self.error.map(|error| {
                div()
                    .pt(px(t.space.s10))
                    .flex()
                    .justify_center()
                    .text_center()
                    .child(meta(error.into(), c.attention_text))
            }))
            .when(!self.first, |el| {
                el.child(
                    div().pt(px(t.space.s12)).flex().justify_center().child(
                        ui::text_action("profile-flow-cancel", "Cancel")
                            .on_click(cx.listener(|flow, _, _, cx| flow.close(cx))),
                    ),
                )
            })
    }

    /// Return and esc on the name screen: continue, and for New profile, cancel.
    fn key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.screen != Screen::Name
            || [&self.name, &self.git.name, &self.git.email]
                .iter()
                .any(|field| field.read(cx).composing())
        {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" if !self.first => self.close(cx),
            "enter" => self.create(cx),
            _ => return false,
        }
        true
    }

    /// Tab moves between the name and, while open, the Git fields.
    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.screen != Screen::Name {
            return false;
        }
        let mut fields = vec![self.name.read(cx).focus.clone()];
        if self.git.open {
            fields.push(self.git.name.read(cx).focus.clone());
            fields.push(self.git.email.read(cx).focus.clone());
        }
        let at = fields
            .iter()
            .position(|f| f.is_focused(window))
            .unwrap_or(0);
        let next = if back {
            (at + fields.len() - 1) % fields.len()
        } else {
            (at + 1) % fields.len()
        };
        window.focus(&fields[next], cx);
        true
    }

    /// A screen's words and body, cross-faded in as the screen arrives.
    pub fn stage(
        &mut self,
        id: &'static str,
        top: f32,
        mark: impl IntoElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let Parts {
            key,
            title,
            line,
            body,
        } = self.parts(window, cx);
        // A screen fades in as it arrives; the first one is already there.
        let step = tokens().motion.step_ms;
        let now = std::time::Instant::now();
        match self.arrived {
            Some((shown, _)) if shown == key => {}
            Some(_) if !onboarding::reduce_motion() => self.arrived = Some((key, now)),
            _ => {
                let past = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
                self.arrived = Some((key, past));
            }
        }
        let shown = self.arrived.map_or(1.0, |(_, since)| {
            (since.elapsed().as_secs_f32() * 1000.0 / step).min(1.0)
        });
        if shown < 1.0 {
            window.request_animation_frame();
        }
        let fade = |element: AnyElement| div().w_full().opacity(shown).child(element);
        ui::setup_stage(
            id,
            top,
            mark,
            fade(ui::setup_title(title).into_any_element()),
            fade(ui::setup_line(line).into_any_element()),
            fade(body),
        )
        .capture_key_down(cx.listener(|flow, event: &gpui::KeyDownEvent, window, cx| {
            let tab = event.keystroke.key == "tab"
                && flow.tab(event.keystroke.modifiers.shift, window, cx);
            if tab || flow.key(event, cx) {
                cx.stop_propagation();
            }
        }))
    }
}

/// This computer's own Git name and email, either empty when unset.
fn computer_identity() -> (String, String) {
    let path = onboarding::probe::login_path();
    let read = |key: &str| {
        std::process::Command::new("git")
            .env("PATH", &path)
            .args(["config", "--global", "--get", key])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default()
    };
    (read("user.name"), read("user.email"))
}

/// Set a Git name and email typed for a new profile inside it: a shell in
/// the profile, with its own home, runs `git config --global` and exits.
/// Agent Backbone can copy only this computer's own identity into a profile,
/// so this is how a different one is set. Best effort: a failure is logged,
/// and the profile still works.
fn set_identity(connection: Connection, name: String, email: String, cx: &mut Context<Flow>) {
    cx.background_executor()
        .spawn(async move {
            if let Err(error) = typed_identity(&connection, &name, &email) {
                tracing::warn!(event = "profile_git_identity_failed", %error, "Git identity not set");
            }
        })
        .detach();
}

fn typed_identity(connection: &Connection, name: &str, email: &str) -> Result<(), String> {
    use std::io::Write;
    // The profile's shell may be fish, tcsh or nushell, which quote differently,
    // so it is typed only `exec /bin/sh <file>`, read alike by all of them. The
    // script removes its file, and the session ends when it finishes.
    let script = identity_script(name, email);
    let session = connection
        .ensure_shell("git-identity", None)
        .map_err(|error| format!("{error:?}"))?;
    let mut file = tempfile::Builder::new()
        .prefix("agent-interface-identity-")
        .tempfile()
        .map_err(|error| error.to_string())?;
    file.write_all(script.as_bytes())
        .map_err(|error| error.to_string())?;
    let path = file
        .path()
        .to_str()
        .filter(|path| {
            path.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
        })
        .ok_or("the temporary folder's path needs quoting")?;
    let command = format!("exec /bin/sh {path}");
    let tmux =
        agent_interface_tmux::connection::tmux_binary().map_err(|error| error.to_string())?;
    let target = format!("={session}:");
    for keys in [["-l", command.as_str()], ["Enter", ""]] {
        let mut command = std::process::Command::new(&tmux);
        command
            .arg("-S")
            .arg(connection.tmux_socket())
            .args(["send-keys", "-t", &target])
            .arg(keys[0]);
        if !keys[1].is_empty() {
            command.arg(keys[1]);
        }
        let status = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("tmux send-keys exited with {status}"));
        }
    }
    // The shell reads the file after this returns; the script removes it.
    file.keep().map_err(|error| error.to_string())?;
    Ok(())
}

/// A `/bin/sh` script, run with its own path as `$0`, that sets the identity.
fn identity_script(name: &str, email: &str) -> String {
    let mut script = String::from("rm -f -- \"$0\"\n");
    for (key, value) in [("user.name", name), ("user.email", email)] {
        if !value.is_empty() {
            script.push_str(&format!("git config --global {key} {}\n", quote(value)));
        }
    }
    script
}

/// A value as one POSIX shell word.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
    // A profile already made stays the one to start, whatever storage says.
    let unavailable = |made: &Option<Profile>| match made {
        Some(profile) => (
            Stalled::NotStarted,
            Some(Box::new(profile.clone())),
            storage,
        ),
        None => (Stalled::Failed, None, storage),
    };
    let library = crate::library::location::create_library_directory(directory.as_deref())
        .map_err(|_| unavailable(&made))?;
    let controller = journal::Journal::open(&library.join("profile-operations"))
        .map(|journal| Controller::new(journal, &client))
        .map_err(|_| unavailable(&made))?;
    let profile =
        match (made, resume) {
            // Read it again: the start names its current lifecycle, which an
            // earlier attempt may have changed.
            (Some(profile), Some(Stalled::NotStarted)) => client
                .show(&profile.item().profile_id, &alive)
                .unwrap_or(profile),
            (_, resume) => {
                let receipt = match (prepared, resume) {
                    (Some(prepared), _) => {
                        let key = Controller::key(prepared.request_id());
                        let result = controller
                            .enqueue(prepared)
                            .and_then(|_| controller.submit(&key, &alive));
                        result.map_err(|_| unconfirmed(key))?
                    }
                    (None, Some(Stalled::Unconfirmed(key))) => recover(&controller, &key, &alive)?,
                    _ => return Err((Stalled::Failed, None, "Give the profile a name.")),
                };
                match (receipt.status, receipt.identity) {
                    (Status::Succeeded, Some(identity)) => {
                        client.show(&identity.profile_id, &alive).map_err(|_| {
                        // Made, then removed: only the list can say so, and
                        // then a new request is safe.
                        let gone = client.list(&alive).is_ok_and(|items| {
                            items.iter().all(|item| item.profile_id != identity.profile_id)
                        });
                        if gone {
                            (
                                Stalled::Failed,
                                None,
                                "The profile made before was removed. Try again to make it again.",
                            )
                        } else {
                            unconfirmed(Controller::key(&receipt.request_id))
                        }
                    })?
                    }
                    (Status::Pending, _) | (Status::Succeeded, None) => {
                        return Err(unconfirmed(Controller::key(&receipt.request_id)));
                    }
                    (Status::Failed, _) => {
                        return Err((
                            Stalled::Failed,
                            None,
                            if receipt.error_code.as_deref() == Some("git_identity_unavailable") {
                                GIT_MISSING
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

/// The receipt of a create request sent before, reading what is saved first:
/// after a restart it may never have been sent, or may already have finished.
fn recover(
    controller: &Controller,
    key: &str,
    alive: &dyn Fn() -> bool,
) -> Result<Receipt, Failure> {
    match controller.read(key) {
        Err(controller::Error::Missing) => Err((
            Stalled::Failed,
            None,
            "The profile could not be made. Try again.",
        )),
        Ok(snapshot) => match (snapshot.terminal, snapshot.phase) {
            (Some(receipt), _) => Ok(receipt),
            (None, Phase::Prepared) => controller.submit(key, alive),
            (None, Phase::Submitted) => controller.refresh(key, alive),
        }
        .map_err(|_| unconfirmed(key.to_owned())),
        Err(_) => Err(unconfirmed(key.to_owned())),
    }
}

fn unconfirmed(key: String) -> Failure {
    (
        Stalled::Unconfirmed(key),
        None,
        "It isn't confirmed yet whether the profile was made. Check again.",
    )
}

impl Render for Flow {
    /// New profile, later: the same screens inside the content area.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let top = ui::setup_top(f32::from(window.viewport_size().height));
        let mark = scene::mark(t.size.setup_mark, scene::Face::icon(), 1.0);
        div().size_full().bg(colors().ground).child(self.stage(
            "profile-flow",
            top,
            mark,
            window,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_create_request_is_looked_up_never_made_again() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let provider = root.join("provider");
        std::fs::write(
            &provider,
            "#!/bin/sh\nprintf '%s\\n' \"$2\" >> \"$(dirname \"$0\")/calls\"\n/bin/cat \"$(dirname \"$0\")/reply\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            root.join("reply"),
            serde_json::json!({
                "schema_version": 1, "request_id": "request-a", "action": "create",
                "profile_id": "profile-a", "installation_id": "installation-a",
                "lifecycle_id": "lifecycle-a", "status": "succeeded", "error": null
            })
            .to_string(),
        )
        .unwrap();
        let client = Client::new(provider, "/usr/bin:/bin".into()).unwrap();
        let controller = Controller::new(
            journal::Journal::open(&root.join("journal")).unwrap(),
            &client,
        );
        let calls = || std::fs::read_to_string(root.join("calls")).unwrap_or_default();
        // Saved, but the app closed before the request was written: nothing was made.
        let missing = Controller::key("request-b");
        assert!(matches!(
            recover(&controller, &missing, &|| true),
            Err((Stalled::Failed, None, _))
        ));
        // Written, but closed before it was sent: send that same request.
        let prepared = Prepared::create("request-a", "Example", BASELINE).unwrap();
        let key = controller.enqueue(prepared).unwrap().key;
        let Ok(receipt) = recover(&controller, &key, &|| true) else {
            panic!("the saved request was not sent");
        };
        assert_eq!(receipt.status, Status::Succeeded);
        assert_eq!(calls(), "apply\n");
        // Already finished: its saved receipt, with no command at all.
        assert!(recover(&controller, &key, &|| true).is_ok_and(|saved| saved == receipt));
        assert_eq!(calls(), "apply\n");
    }

    #[test]
    fn a_typed_identity_is_one_shell_word_each() {
        assert_eq!(quote("Sam Rivera"), "'Sam Rivera'");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn the_identity_script_keeps_every_character_and_removes_itself() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("identity");
        // Never the developer's own global configuration.
        let config = home.path().join("gitconfig");
        let (name, email) = (r"Sam O'Brien \ ops!", "ops!dev@example.com");
        std::fs::write(&path, identity_script(name, email)).unwrap();
        let isolated = |command: &mut std::process::Command| {
            command
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", home.path())
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_NOSYSTEM", "1");
        };
        let mut script = std::process::Command::new("/bin/sh");
        script.arg(&path);
        isolated(&mut script);
        let status = script.status().unwrap();
        assert!(status.success());
        assert!(!path.exists());
        let read = |key: &str| {
            let mut git = std::process::Command::new("git");
            git.args(["config", "--global", key]);
            isolated(&mut git);
            let output = git.output().unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(read("user.name"), format!("{name}\n"));
        assert_eq!(read("user.email"), format!("{email}\n"));
    }
}
