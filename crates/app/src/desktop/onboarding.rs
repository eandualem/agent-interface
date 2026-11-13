//! First run, as a layer over the workspace window. The app's face winks,
//! then settles at the top of one column where each question appears in turn:
//! the person's first profile, then its guide, the same screens as New
//! profile. The agent service starts quietly while the face plays; tmux or
//! Git, when missing, is one Install button. When the guide's pane opens the
//! layer fades and the workspace is there.

pub(super) mod install;
pub(super) mod probe;
pub(super) mod scene;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use agent_interface_roster::profiles::{Connection, Item as Listed};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, KeyDownEvent, MouseButton,
    SharedString, Subscription, Window, div, prelude::*, px,
};

use self::probe::{Backbone, Probe};
use super::{
    components as ui,
    design::{colors, tokens},
    profile_management::{
        Open,
        flow::{self, Flow},
    },
    settings::{self, Settings},
};
use crate::profiles::Client;

const TAGLINE: &str = "Your agents. Working together. On your machine.";
/// How often first run checks again while a system installer it opened runs.
const WATCH: Duration = Duration::from_secs(4);
/// How long the agent service gets to answer after the app starts it.
const SERVICE_WAIT: Duration = Duration::from_secs(30);
const SERVICE_POLL: Duration = Duration::from_secs(1);

/// Whether this launch opens with first run: only a completed setup skips it.
/// Existing libraries and preferences may predate first run and do not
/// establish that it was completed.
pub fn first_run(preferences: &Settings) -> bool {
    !preferences.setup_done
}

/// The system asks for less motion: no scene, and screens change at once.
/// Read once, the first time it is asked.
pub(in crate::desktop) fn reduce_motion() -> bool {
    static REDUCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REDUCE.get_or_init(|| {
        cfg!(target_os = "macos")
            && std::process::Command::new("/usr/bin/defaults")
                .args(["read", "com.apple.universalaccess", "reduceMotion"])
                .output()
                .is_ok_and(|output| output.stdout.trim_ascii() == b"1")
    })
}

/// Apply a settings change everywhere and save it.
fn remember(update: impl FnOnce(&mut Settings)) {
    let mut next = Settings::current();
    update(&mut next);
    next.apply();
    settings::save_in_background(next, |result| {
        if let Err(error) = result {
            tracing::warn!(event = "setup_save_failed", %error, "Setup state not saved");
        }
    });
}

pub(in crate::desktop) enum Event {
    /// The first profile exists: bind the starting workspace to it.
    Bind(Box<FirstBind>),
    /// The guide is up: open its pane.
    Open(Open),
    /// No guide for now: the workspace opens on a terminal.
    Terminal,
    /// The layer has faded away.
    Done,
}

/// The first profile, for the workspace window to bind its starting
/// workspace to once its library has loaded.
#[derive(Clone)]
pub(in crate::desktop) struct FirstBind {
    pub item: Listed,
    pub connection: Connection,
    pub client: Client,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The opening scene, over whichever screen comes next.
    Scene,
    /// Something needs the person, or the service is still starting.
    Needs,
    /// The first profile and its guide.
    Profile,
}

/// The agent service, as first run sees it.
#[derive(Clone, PartialEq, Eq)]
enum Service {
    Checking,
    /// The app is starting or setting it up.
    Starting,
    Ready,
    /// It couldn't, and why, in technical words for Details.
    Failed(String),
}

/// An install the person asked for.
#[derive(Clone, PartialEq, Eq)]
enum Installing {
    Running,
    /// The system's own installer is open; first run checks until it is done.
    Opened,
    Failed(String),
}

pub(in crate::desktop) struct Onboarding {
    stage: Stage,
    probe: Option<Probe>,
    checking: bool,
    /// Another check was asked for while one ran.
    recheck: bool,
    service: Service,
    installing: Option<Installing>,
    watching: bool,
    /// The opening scene's clock; Skip moves it to the handoff.
    clock: scene::Clock,
    focus: FocusHandle,
    /// The flow's client, waiting for a window to make the flow in.
    pending_flow: Option<Client>,
    flow: Option<(Entity<Flow>, Subscription)>,
    /// The service's technical reason is shown.
    details: bool,
    /// The layer began to fade.
    leaving: Option<Instant>,
    /// A first profile made before the app last closed, to carry on with.
    resume: Option<String>,
    /// The request making it, when the app closed before it was made.
    resume_request: Option<(String, Option<(String, String)>)>,
    /// The service was restarted or set up once; only Try again repeats it.
    repaired: bool,
    /// The guide's pane asked for, and why it didn't open if it didn't.
    opening: Option<(Open, Option<&'static str>)>,
    directory: Option<PathBuf>,
}
impl EventEmitter<Event> for Onboarding {}

impl Onboarding {
    pub fn new(directory: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let preferences = Settings::current();
        let resume = preferences.setup_profile.clone();
        let resume_request = preferences
            .setup_request
            .clone()
            .map(|key| (key, preferences.setup_identity.clone()));
        // A profile made before is carried on with straight away, no scene.
        let intro = !preferences.intro_seen
            && resume.is_none()
            && resume_request.is_none()
            && !reduce_motion();
        if !intro {
            // Record now that first run began, so an interrupted one resumes
            // without the scene after other settings create the file.
            remember(|settings| settings.intro_seen = true);
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut view = Self {
            stage: if intro { Stage::Scene } else { Stage::Needs },
            probe: None,
            checking: false,
            recheck: false,
            service: Service::Checking,
            installing: None,
            watching: false,
            clock: Default::default(),
            focus,
            pending_flow: None,
            flow: None,
            details: false,
            leaving: None,
            resume,
            resume_request,
            repaired: false,
            opening: None,
            directory,
        };
        // The scene covers the check, so what follows it is known.
        view.check(cx);
        view
    }

    /// How far the layer has faded, while it is leaving.
    pub fn reveal(&self) -> Option<f32> {
        self.leaving.map(|since| {
            (since.elapsed().as_secs_f32() * 1000.0 / tokens().motion.reveal_ms).clamp(0.0, 1.0)
        })
    }

    /// First run is over: save that, and fade away.
    fn leave(&mut self, cx: &mut Context<Self>) {
        if self.leaving.is_some() {
            return;
        }
        remember(|settings| {
            settings.setup_done = true;
            settings.setup_profile = None;
            settings.setup_request = None;
            settings.setup_identity = None;
        });
        let now = Instant::now();
        self.leaving = Some(if reduce_motion() {
            now.checked_sub(Duration::from_secs(1)).unwrap_or(now)
        } else {
            now
        });
        cx.notify();
    }

    /// The workspace says whether the guide's pane opened: first run ends
    /// only once it has.
    pub fn opened(&mut self, result: Result<(), &'static str>, cx: &mut Context<Self>) {
        match result {
            Ok(()) => {
                self.opening = None;
                self.leave(cx);
            }
            Err(reason) => {
                if let Some((_, failed)) = &mut self.opening {
                    *failed = Some(reason);
                }
                cx.notify();
            }
        }
    }

    /// Jump the opening scene to its handoff.
    fn skip(&mut self, cx: &mut Context<Self>) {
        let ms = self.clock.ms();
        let to = scene::skipped(ms);
        if self.stage == Stage::Scene && to > ms {
            self.clock.set(to);
            cx.notify();
        }
    }

    /// Check the computer again. A request during a check runs after it.
    fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            self.recheck = true;
            return;
        }
        self.checking = true;
        cx.spawn(async |view, cx| {
            let probe = cx.background_executor().spawn(async { Probe::run() }).await;
            let _ = view.update(cx, |view, cx| {
                if view.installing != Some(Installing::Running) && missing(&probe).is_empty() {
                    view.installing = None;
                }
                let start = matches!(view.service, Service::Checking | Service::Ready);
                view.probe = Some(probe);
                view.checking = false;
                if start {
                    view.start_service(cx);
                }
                if std::mem::take(&mut view.recheck) {
                    view.check(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Start the agent service without asking: restarted once if it doesn't
    /// answer, or set up as the installer does if it never was.
    fn start_service(&mut self, cx: &mut Context<Self>) {
        let Some(probe) = self.probe.clone() else {
            return;
        };
        if matches!(probe.backbone, Backbone::Running { .. }) && probe.client().is_some() {
            self.service = Service::Ready;
            if self.flow.is_none() {
                self.pending_flow = probe.client();
            }
            return;
        }
        // Repair runs once; if the service still doesn't answer, say so.
        if self.repaired {
            self.service = Service::Failed(match &probe.backbone {
                Backbone::Unreachable(reason) => {
                    format!("Agent Backbone stopped answering after it was started ({reason}).")
                }
                _ => "Agent Backbone stopped answering after it was started.".to_owned(),
            });
            return;
        }
        self.repaired = true;
        self.service = Service::Starting;
        let work = cx.background_executor().spawn(async move { start(&probe) });
        cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = view.update(cx, |view, cx| {
                match result {
                    Ok(()) => {
                        view.service = Service::Ready;
                        view.check(cx);
                    }
                    Err(reason) => {
                        tracing::warn!(event = "setup_service_failed", %reason, "Agent service not started");
                        view.service = Service::Failed(reason);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Make the first profile's flow, or carry on with the one made before.
    fn make_flow(&mut self, client: Client, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self.directory.clone();
        let flow = cx.new(|cx| {
            let mut flow = Flow::new(client.clone(), directory, String::new(), None, window, cx);
            flow.for_first_run(cx);
            flow
        });
        // During the scene, keys skip it; the name field takes them after.
        if self.stage == Stage::Scene {
            window.focus(&self.focus, cx);
        } else {
            window.focus(&flow.read(cx).focus_handle(cx), cx);
        }
        let events = cx.subscribe_in(&flow, window, move |view, _, event, _, cx| match event {
            flow::Event::Bind(selected) => {
                let id = selected.item().profile_id.clone();
                remember(|settings| {
                    settings.setup_profile = Some(id);
                    settings.setup_request = None;
                    settings.setup_identity = None;
                });
                cx.emit(Event::Bind(Box::new(FirstBind {
                    item: selected.item().clone(),
                    connection: selected.connection().clone(),
                    client: client.clone(),
                })));
            }
            flow::Event::Open(open) => {
                view.opening = Some((open.clone(), None));
                cx.emit(Event::Open(open.clone()));
            }
            flow::Event::Terminal => {
                cx.emit(Event::Terminal);
                view.leave(cx);
            }
            flow::Event::Creating(key, typed) => {
                let (key, typed) = (key.clone(), typed.clone());
                remember(|settings| {
                    // A new request replaces a saved profile that is gone.
                    settings.setup_profile = None;
                    settings.setup_request = Some(key);
                    settings.setup_identity = typed;
                });
            }
            flow::Event::Close => {}
        });
        if let Some(id) = self.resume.take() {
            flow.update(cx, |flow, cx| flow.resume_saved(id, cx));
        } else if let Some((key, typed)) = self.resume_request.take() {
            flow.update(cx, |flow, cx| flow.resume_request(key, typed, cx));
        }
        self.flow = Some((flow, events));
    }

    /// Install what's missing, then check again.
    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(probe) = &self.probe else {
            return;
        };
        let names = missing(probe);
        // An installer that was opened and closed without installing can be
        // opened again.
        if names.is_empty() || self.installing == Some(Installing::Running) {
            return;
        }
        let path_env = probe.path_env.clone();
        self.installing = Some(Installing::Running);
        let work = cx.background_executor().spawn(async move {
            let mut outcome = install::Outcome::Done;
            for name in names {
                outcome = install::package(name, &path_env);
                if outcome != install::Outcome::Done {
                    break;
                }
            }
            outcome
        });
        cx.spawn(async move |view, cx| {
            let outcome = work.await;
            let _ = view.update(cx, |view, cx| {
                view.installing = match outcome {
                    install::Outcome::Done => None,
                    install::Outcome::Opened => {
                        view.watch(cx);
                        Some(Installing::Opened)
                    }
                    install::Outcome::Failed(reason) => Some(Installing::Failed(reason)),
                };
                view.check(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// While a system installer first run opened is running, check now and
    /// then until what it installs is found.
    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.watching {
            return;
        }
        self.watching = true;
        cx.spawn(async |view, cx| {
            loop {
                cx.background_executor().timer(WATCH).await;
                let open = view.update(cx, |view, cx| {
                    let open = view.installing == Some(Installing::Opened);
                    if open {
                        view.check(cx);
                    } else {
                        view.watching = false;
                    }
                    open
                });
                if !matches!(open, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// What the person sees next, once it is known: what needs them, else the
    /// first profile. `None` while the check or the service is still going.
    fn next(&self) -> Option<Stage> {
        if matches!(self.opening, Some((_, Some(_)))) {
            return Some(Stage::Needs);
        }
        let probe = self.probe.as_ref()?;
        if !missing(probe).is_empty() {
            return Some(Stage::Needs);
        }
        match (&self.service, &self.flow) {
            (Service::Failed(_), _) => Some(Stage::Needs),
            (Service::Ready, Some(_)) => Some(Stage::Profile),
            _ => None,
        }
    }

    /// A screen that needs the person: an install, or a service that won't
    /// start. One button, and for the service, a way on with terminals.
    fn needs(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(SharedString, SharedString, AnyElement)> {
        let (t, c) = (tokens(), colors());
        if let Some((open, Some(reason))) = &self.opening {
            let open = open.clone();
            let body = div()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    ui::setup_button("open-again", "Try again", true, false).on_click(cx.listener(
                        move |view, _, _, cx| {
                            view.opening = Some((open.clone(), None));
                            cx.emit(Event::Open(open.clone()));
                            cx.notify();
                        },
                    )),
                )
                .child(div().pt(px(t.space.s12)).flex().justify_center().child(
                    ui::text_action("terminals-only", "Use a terminal instead").on_click(
                        cx.listener(|view, _, _, cx| {
                            view.opening = None;
                            cx.emit(Event::Terminal);
                            view.leave(cx);
                        }),
                    ),
                ))
                .into_any_element();
            return Some((
                "Your guide couldn\u{2019}t open".into(),
                (*reason).into(),
                body,
            ));
        }
        let probe = self.probe.as_ref()?;
        let names = missing(probe);
        let note = |text: SharedString, attention: bool| {
            div()
                .pt(px(t.space.s10))
                .flex()
                .justify_center()
                .text_center()
                .child(
                    ui::text(
                        div().text_color(if attention { c.attention_text } else { c.muted }),
                        &t.text.meta,
                    )
                    .child(text),
                )
        };
        if !names.is_empty() {
            let what = names
                .iter()
                .map(|name| if *name == "git" { "Git" } else { name })
                .collect::<Vec<_>>()
                .join(" and ");
            let line = match names.as_slice() {
                ["tmux"] => "Agent Interface keeps your terminals running with tmux.",
                ["git"] => "Your agents use Git for their work.",
                _ => {
                    "Agent Interface keeps your terminals running with tmux, and your agents use Git for their work."
                }
            };
            // Terminals need only tmux: without Git, they can still go on.
            let terminals = !names.contains(&"tmux");
            let busy = self.installing == Some(Installing::Running);
            let label = if busy {
                format!("Installing {what}\u{2026}")
            } else if matches!(self.installing, Some(Installing::Failed(_))) {
                "Try again".to_owned()
            } else {
                format!("Install {what}")
            };
            let body = div()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    ui::setup_button("install-missing", label, true, busy)
                        .on_click(cx.listener(|view, _, _, cx| view.install(cx))),
                )
                .children(match &self.installing {
                    Some(Installing::Opened) => Some(note(
                        "Finish the installer that opened. This screen moves on by itself.".into(),
                        false,
                    )),
                    Some(Installing::Failed(reason)) => Some(note(reason.clone().into(), true)),
                    _ => None,
                })
                .when(terminals, |body| {
                    body.child(div().pt(px(t.space.s12)).flex().justify_center().child(
                        ui::text_action("terminals-only", "Continue with terminals").on_click(
                            cx.listener(|view, _, _, cx| {
                                cx.emit(Event::Terminal);
                                view.leave(cx);
                            }),
                        ),
                    ))
                })
                .into_any_element();
            return Some((
                format!("Install {what} to continue").into(),
                line.into(),
                body,
            ));
        }
        let Service::Failed(reason) = &self.service else {
            return None;
        };
        let reason = reason.clone();
        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                ui::setup_button("service-again", "Try again", true, false).on_click(cx.listener(
                    |view, _, _, cx| {
                        view.service = Service::Checking;
                        view.repaired = false;
                        view.details = false;
                        view.check(cx);
                    },
                )),
            )
            .child(
                div()
                    .pt(px(t.space.s12))
                    .flex()
                    .justify_center()
                    .gap(px(t.space.s20))
                    .child(
                        ui::text_action("terminals-only", "Continue with terminals").on_click(
                            cx.listener(|view, _, _, cx| {
                                cx.emit(Event::Terminal);
                                view.leave(cx);
                            }),
                        ),
                    )
                    .child(
                        ui::text_action("service-details", "Details")
                            .child(ui::icon(
                                if self.details {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                t.size.icon - t.space.s2,
                                c.muted,
                            ))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.details = !view.details;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.details, |body| {
                body.child(
                    div().pt(px(t.space.s10)).child(
                        ui::text(
                            div()
                                .px(px(t.space.s10))
                                .py(px(t.space.s8))
                                .rounded(px(t.radius.row))
                                .bg(c.field)
                                .border(px(t.size.hairline))
                                .border_color(c.line)
                                .text_color(c.muted)
                                .font_family(t.font.mono.clone()),
                            &t.text.meta,
                        )
                        .child(reason),
                    ),
                )
            })
            .into_any_element();
        Some((
            "Agents can\u{2019}t start yet".into(),
            "Agent Interface couldn\u{2019}t start the service that runs your agents. Your terminals still work.".into(),
            body,
        ))
    }

    /// Skip, during the scene, and the interface size, throughout.
    fn corners(&self, scene: Option<&scene::Scene>, cx: &mut Context<Self>) -> [AnyElement; 2] {
        let t = tokens();
        let percent = Settings::current().interface_size;
        let sizes = super::zoom::SIZES;
        let at = sizes.iter().position(|size| *size == percent).unwrap_or(1);
        let step = |id: &'static str, label: &'static str, next: Option<u32>| {
            ui::text_action(id, label)
                .border(px(t.size.hairline))
                .border_color(colors().line_strong)
                .opacity(if next.is_some() { 1.0 } else { 0.4 })
                .when_some(next, |el, next| {
                    el.on_click(cx.listener(move |_, _, _, cx| {
                        remember(|settings| settings.interface_size = next);
                        cx.refresh_windows();
                    }))
                })
        };
        let corner = |left: bool| {
            ui::text(
                div()
                    .absolute()
                    .bottom(px(t.space.s14))
                    .when(left, |el| el.left(px(t.space.s18)))
                    .when(!left, |el| el.right(px(t.space.s18)))
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .text_color(colors().muted),
                &t.text.meta,
            )
        };
        let skip = corner(true)
            .when_some(scene.copied(), |el, scene| {
                el.opacity(scene.controls).child(
                    ui::text_action("skip-scene", "Skip")
                        .child(ui::keycap("esc"))
                        .on_click(cx.listener(|view, _, _, cx| view.skip(cx))),
                )
            })
            .into_any_element();
        let size = corner(false)
            .child("Interface size")
            .child(step(
                "setup-smaller",
                "\u{2212}",
                at.checked_sub(1).map(|at| sizes[at]),
            ))
            .child(format!("{percent}%"))
            .child(step("setup-larger", "+", sizes.get(at + 1).copied()))
            .into_any_element();
        [skip, size]
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        if let Some(client) = self.pending_flow.take() {
            self.make_flow(client, window, cx);
        }
        if let Some(reveal) = self.reveal() {
            if reveal >= 1.0 {
                cx.emit(Event::Done);
            } else {
                window.request_animation_frame();
            }
        }
        let handoff = t.motion.scene_handoff_ms;
        let next = self.next();
        // What follows the scene must be known at its handoff, so the face
        // waits there, at rest on the icon, until it is.
        let waiting =
            self.stage == Stage::Scene && next.is_none() && self.clock.ms() >= handoff - 1.0;
        if waiting {
            self.clock.set(handoff - 1.0);
        }
        if self.stage == Stage::Scene
            && self.clock.ms() >= t.motion.scene_ms
            && let Some(next) = next
        {
            self.stage = next;
            remember(|settings| settings.intro_seen = true);
            if let Some((flow, _)) = &self.flow {
                window.focus(&flow.read(cx).focus_handle(cx), cx);
            }
        } else if self.stage != Stage::Scene
            && let Some(next) = next
            && next != self.stage
        {
            self.stage = next;
            if let Some((flow, _)) = self.flow.as_ref().filter(|_| next == Stage::Profile) {
                window.focus(&flow.read(cx).focus_handle(cx), cx);
            }
        }
        let scene = (self.stage == Stage::Scene).then(|| scene::Scene::at(self.clock.ms()));
        // Without the scene, an unknown next screen waits on the settled face.
        let getting_ready = waiting || (scene.is_none() && next.is_none());
        let height = f32::from(window.viewport_size().height);
        let setup_top = ui::setup_top(height);
        // The scene's face and words, centred.
        let scene_block = t.size.scene_mark
            + t.space.s16
            + t.space.s6
            + f32::from(t.text.scene_title.size()) * 1.3
            + f32::from(t.text.scene_line.size()) * 1.6;
        let scene_top = ((height - scene_block) / 2.0).max(setup_top);
        let settled = || scene::mark(t.size.setup_mark, scene::Face::icon(), 1.0);
        let flow = self.flow.as_ref().map(|(flow, _)| flow.clone());
        let content: AnyElement = match (&scene, next, flow) {
            // After the scene, the flow draws its own screens.
            (None, Some(Stage::Profile), Some(flow)) => flow
                .update(cx, |flow, cx| {
                    flow.stage("first-run", setup_top, settled(), window, cx)
                })
                .into_any_element(),
            (scene, next, flow) => {
                let parts = match next {
                    Some(Stage::Needs) => self.needs(cx),
                    Some(Stage::Profile) => flow.map(|flow| {
                        let parts = flow.update(cx, |flow, cx| flow.parts(window, cx));
                        (parts.title, parts.line, parts.body)
                    }),
                    _ => None,
                };
                let shown = scene.map_or(1.0, |scene| scene.into);
                let form = scene.map_or(1.0, |scene| scene.form);
                let (title, line, body) = match parts {
                    Some((title, line, body)) => (
                        ui::setup_title(title).opacity(shown).into_any_element(),
                        ui::setup_line(line).opacity(shown).into_any_element(),
                        div()
                            .relative()
                            .top(px(scene::WORDS_PX * (1.0 - form)))
                            .opacity(form)
                            .child(body)
                            .into_any_element(),
                    ),
                    None => (
                        div().into_any_element(),
                        div().into_any_element(),
                        div().into_any_element(),
                    ),
                };
                // The app's name and tagline sit where the screen's words will.
                let (name, tagline, mark, top) = match scene {
                    Some(scene) => (
                        scene.name,
                        scene.line,
                        scene::mark(scene.mark_size(), scene.face, scene.tile),
                        scene.top(scene_top, setup_top),
                    ),
                    // Without the scene, the name stays while things get ready.
                    None => (
                        if getting_ready { 1.0 } else { 0.0 },
                        0.0,
                        settled(),
                        setup_top,
                    ),
                };
                let (tagline, tagline_shown): (AnyElement, f32) = if getting_ready {
                    (
                        div()
                            .flex()
                            .justify_center()
                            .items_center()
                            .gap(px(t.space.s8))
                            .child(ui::sending_spinner())
                            .child(
                                ui::text(div().text_color(c.muted), &t.text.scene_line)
                                    .child("Getting things ready\u{2026}"),
                            )
                            .into_any_element(),
                        1.0,
                    )
                } else {
                    (ui::setup_line(TAGLINE).into_any_element(), tagline)
                };
                let swap = |under: AnyElement, over: AnyElement, shown: f32| {
                    div().relative().w_full().child(under).child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w_full()
                            .opacity(shown)
                            .child(over),
                    )
                };
                ui::setup_stage(
                    "first-run",
                    top,
                    mark,
                    swap(
                        title,
                        ui::setup_title("Agent Interface").into_any_element(),
                        name,
                    ),
                    swap(line, tagline, tagline_shown),
                    div().when(form < 1.0, |el| el.occlude()).child(body),
                )
                .into_any_element()
            }
        };
        let [skip, size] = self.corners(scene.as_ref(), cx);
        div()
            .absolute()
            .inset_0()
            .bg(c.ground)
            .text_color(c.text)
            .font_family(t.font.ui.clone())
            .occlude()
            .opacity(1.0 - self.reveal().unwrap_or(0.0))
            .when(self.stage == Stage::Scene, |root| {
                root.track_focus(&self.focus)
                    .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                        let keys = &event.keystroke.modifiers;
                        if !(keys.platform || keys.control || keys.alt) {
                            view.skip(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _, cx| view.skip(cx)),
                    )
                    .child(scene::ticker(self.clock.clone()))
            })
            .child(content)
            .child(skip)
            .child(size)
    }
}

/// Start the agent service, or set it up, and wait for it to answer.
/// Blocks; runs off the UI thread.
fn start(probe: &Probe) -> Result<(), String> {
    let logs = if cfg!(target_os = "macos") {
        "Its log is backbone.log in its data folder."
    } else {
        "Its log: journalctl --user -u agent-backbone.service -n 80"
    };
    match &probe.backbone {
        Backbone::Running { .. } => Err(format!(
            "Agent Backbone answered, but its backbone command was not found on this computer's PATH ({}).",
            probe.path_env
        )),
        Backbone::Unreachable(reason) => {
            if let install::Outcome::Failed(why) = install::restart_backbone(&probe.path_env) {
                return Err(format!(
                    "Agent Backbone did not answer ({reason}), and restarting its service failed: {why}"
                ));
            }
            answered().ok_or_else(|| {
                format!(
                    "Agent Backbone did not answer after one restart (waited {} s). {logs}",
                    SERVICE_WAIT.as_secs()
                )
            })
        }
        Backbone::NotSetUp => {
            let Some(backbone) = probe.path("backbone") else {
                return Err(
                    "Agent Backbone isn't installed. Run the Agent Interface installer again."
                        .into(),
                );
            };
            install::initialize_backbone(backbone, &probe.path_env)?;
            answered().ok_or_else(|| {
                format!(
                    "Agent Backbone was set up but did not answer (waited {} s). {logs}",
                    SERVICE_WAIT.as_secs()
                )
            })
        }
    }
}

/// Wait for the agent service to answer, for up to [`SERVICE_WAIT`].
fn answered() -> Option<()> {
    let deadline = Instant::now() + SERVICE_WAIT;
    while Instant::now() < deadline {
        if matches!(probe::backbone(), Backbone::Running { .. }) {
            return Some(());
        }
        std::thread::sleep(SERVICE_POLL);
    }
    None
}

/// What this computer is missing for terminals and agents: tmux, Git.
fn missing(probe: &Probe) -> Vec<&'static str> {
    let mut names = Vec::new();
    if probe.tmux.is_none() {
        names.push("tmux");
    }
    if probe.path("git").is_none() {
        names.push("git");
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_completed_setup_skips_onboarding() {
        let mut preferences = Settings::parse("theme = \"light\"\nfont_size = 14\n");
        assert!(first_run(&preferences));
        preferences.intro_seen = true;
        assert!(first_run(&preferences));
        preferences.setup_done = true;
        assert!(!first_run(&preferences));
    }
}
