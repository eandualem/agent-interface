//! First run: a short opening scene, a check of this computer that shows
//! itself only when something it needs is missing, and then the person's
//! first profile, made with the same steps as New profile. Once the profile
//! exists, setup hands those steps to the workspace window, where signing in
//! needs panes. It opens instead of the workspace window on a fresh install,
//! and from the application menu at any time after.

pub(super) mod install;
pub(super) mod probe;
pub(super) mod scene;

use std::{collections::HashMap, time::Duration};

use agent_interface_roster::profiles::{Connection, Item as Listed};
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Global, KeyDownEvent, MouseButton, Pixels,
    SharedString, Size, Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle,
    WindowOptions, div, point, prelude::*, px, size,
};

use self::probe::{Backbone, Probe};
use super::{
    components::{self as ui, Button},
    design::{colors, tokens},
    profile_management::flow::{self, Flow},
    settings::{self, Settings},
};
use crate::profiles::Client;

const TAGLINE: &str = "Your agents. Working together. On your machine.";
/// How often setup checks again while a system installer it opened runs.
const WATCH: Duration = Duration::from_secs(4);
/// How long the workspace window gets to load its library before it takes
/// over the profile's steps.
const ADOPT_TRIES: usize = 200;

/// Something setup can fix for the person when they ask.
#[derive(Clone, Copy)]
enum Fix {
    /// Install Git or tmux with the system's installer.
    Package(&'static str),
    /// Restart Agent Backbone's service.
    Restart,
}

/// A fix under way, or how it ended when it needs the person.
#[derive(Clone, PartialEq, Eq)]
enum Fixing {
    Running,
    /// The system's own installer is open; setup checks until it is done.
    Opened,
    Failed(String),
}

/// Whether this launch opens through setup instead of the workspace window:
/// only a completed setup skips onboarding. Existing libraries and preferences
/// may predate onboarding and do not establish that setup was completed.
pub fn first_run(preferences: &Settings) -> bool {
    !preferences.setup_done
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The opening scene, over whichever page comes next.
    Scene,
    /// Something this computer needs is missing. Shown only then.
    Blockers,
    /// The person's first profile, until it exists.
    Profile,
}

pub struct Onboarding {
    stage: Stage,
    probe: Option<Probe>,
    checking: bool,
    /// Another check was asked for while one ran.
    recheck: bool,
    /// Fixes by what they fix: "git", "tmux" or "backbone".
    fixing: HashMap<&'static str, Fixing>,
    watching: bool,
    open_error: Option<&'static str>,
    /// The opening scene's clock; Skip moves it to the handoff.
    clock: scene::Clock,
    /// Takes the keys that skip the scene.
    focus: FocusHandle,
    /// Where the face and the profile's mark were laid out, for the open
    /// eye's flight between them.
    tile: scene::Place,
    marker: scene::Place,
    /// The first profile's steps, made once the computer is ready.
    flow: Option<(Entity<Flow>, Subscription)>,
    /// Before you start has listed something missing, so the person moves
    /// on from it themselves; until then a ready check moves on at once.
    shown_missing: bool,
    _activation: Subscription,
}

struct Open(WindowHandle<Onboarding>);
impl Global for Open {}

/// Open setup, with the opening scene first when `intro` is set.
pub fn show(intro: bool, cx: &mut App) -> Result<(), String> {
    let open = cx.try_global::<Open>().map(|Open(handle)| *handle);
    if let Some(handle) = open
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let t = tokens();
    let options = WindowOptions {
        app_id: Some(super::APP_ID.into()),
        window_decorations: cfg!(target_os = "linux").then_some(gpui::WindowDecorations::Client),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(t.size.settings_width), px(t.size.setup_height)),
            cx,
        ))),
        is_resizable: true,
        titlebar: Some(TitlebarOptions {
            title: Some("Welcome to Agent Interface".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(t.size.window_controls_x),
                px(t.size.window_controls_y),
            )),
        }),
        ..Default::default()
    };
    let intro = intro && !reduce_motion();
    if !intro {
        // Without the scene, record now that setup began, so an interrupted
        // setup still resumes after other settings create the file.
        remember(|settings| settings.intro_seen = true);
    }
    let handle = cx
        .open_window(options, |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                // Coming back to setup, from an installer or a terminal, checks again.
                let activation =
                    cx.observe_window_activation(window, |view: &mut Onboarding, window, cx| {
                        if window.is_window_active() && view.stage == Stage::Blockers {
                            view.check(cx);
                        }
                    });
                let mut view = Onboarding {
                    stage: if intro { Stage::Scene } else { Stage::Blockers },
                    probe: None,
                    checking: false,
                    recheck: false,
                    fixing: HashMap::new(),
                    watching: false,
                    open_error: None,
                    clock: Default::default(),
                    focus,
                    tile: Default::default(),
                    marker: Default::default(),
                    flow: None,
                    shown_missing: false,
                    _activation: activation,
                };
                // The scene covers the check, so what follows it is known.
                view.check(cx);
                view
            })
        })
        .map_err(|error| error.to_string())?;
    cx.set_global(Open(handle));
    Ok(())
}

/// The system asks for less motion: open straight on the first step.
fn reduce_motion() -> bool {
    cfg!(target_os = "macos")
        && std::process::Command::new("/usr/bin/defaults")
            .args(["read", "com.apple.universalaccess", "reduceMotion"])
            .output()
            .is_ok_and(|output| output.stdout.trim_ascii() == b"1")
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

fn resize_limit(
    current: Bounds<Pixels>,
    visible: Bounds<Pixels>,
    bottom_anchor: bool,
) -> Size<Pixels> {
    // AppKit keeps the bottom edge fixed; Linux grows from the top edge.
    let height = if bottom_anchor {
        current.bottom() - visible.origin.y
    } else {
        visible.bottom() - current.origin.y
    };
    size(
        (visible.right() - current.origin.x - px(16.)).max(px(1.)),
        (height - px(16.)).max(px(1.)),
    )
}

impl Onboarding {
    /// Jump the opening scene to its handoff, so setup follows at once.
    fn skip(&mut self, cx: &mut Context<Self>) {
        let ms = self.clock.ms();
        let to = scene::skipped(ms);
        if self.stage == Stage::Scene && to > ms {
            self.clock.set(to);
            cx.notify();
        }
    }

    /// The interface size, offered throughout. During the opening scene it
    /// fades in beside Skip, and its rule appears with setup.
    fn scale_controls(&self, scene: Option<&scene::Scene>, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let percent = Settings::current().interface_size;
        let sizes = super::zoom::SIZES;
        let at = sizes.iter().position(|size| *size == percent).unwrap_or(1);
        let button = |id, label, next: Option<u32>| {
            ui::button(id, label, Button::Secondary)
                .opacity(if next.is_some() { 1.0 } else { 0.4 })
                .when_some(next, |el, next| {
                    el.on_click(cx.listener(move |_, _, window, cx| {
                        remember(|settings| settings.interface_size = next);
                        if !window.is_maximized() && !window.is_fullscreen() {
                            let wayland = raw_window_handle::HasWindowHandle::window_handle(window)
                                .is_ok_and(|handle| {
                                    matches!(
                                        handle.as_raw(),
                                        raw_window_handle::RawWindowHandle::Wayland(_)
                                    )
                                });
                            if wayland {
                                // Wayland owns placement and does not expose a
                                // global window origin. Let the compositor fit
                                // larger text instead of mixing local/global bounds.
                                if next > percent {
                                    if window.window_controls().maximize {
                                        window.zoom_window();
                                    } else if let Some(display) = window.display(cx) {
                                        // Some compositors do not offer maximize.
                                        // Only the output's extent is meaningful here.
                                        window.resize(
                                            size(
                                                px(tokens().size.settings_width),
                                                px(tokens().size.setup_height),
                                            )
                                            .min(&display.visible_bounds().size),
                                        );
                                    }
                                }
                            } else {
                                // Both bounds use the new zoom factor. Respect the
                                // platform's fixed edge when growing the window.
                                let current = window.bounds();
                                let available =
                                    window.display(cx).map_or(current.size, |display| {
                                        resize_limit(
                                            current,
                                            display.visible_bounds(),
                                            cfg!(target_os = "macos"),
                                        )
                                    });
                                window.resize(
                                    size(
                                        px(tokens().size.settings_width),
                                        px(tokens().size.setup_height),
                                    )
                                    .min(&available),
                                );
                            }
                        }
                        cx.defer(|cx| {
                            for handle in cx.windows() {
                                let _ = handle.update(cx, |_, window, cx| {
                                    #[cfg(target_os = "macos")]
                                    window.set_traffic_light_position(point(
                                        px(tokens().size.window_controls_x),
                                        px(tokens().size.window_controls_y),
                                    ));
                                    window.bounds_changed(cx);
                                });
                            }
                        });
                        cx.refresh_windows();
                    }))
                })
        };
        let mut rule = c.line;
        if let Some(scene) = scene {
            rule.a *= scene.setup;
        }
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_end()
            .gap(px(t.space.s8))
            .px(px(t.space.s24))
            .py(px(t.space.s8))
            .border_t(px(t.size.hairline))
            .border_color(rule)
            .when_some(scene.copied(), |bar, scene| {
                bar.opacity(scene.controls)
                    .child(
                        ui::text(
                            div()
                                .id("skip-scene")
                                .flex()
                                .items_center()
                                .gap(px(t.space.s8))
                                .h(px(t.size.button))
                                .ml(px(-t.space.s8))
                                .px(px(t.space.s8))
                                .rounded(px(t.radius.row))
                                .cursor_pointer()
                                .text_color(c.muted)
                                .hover(|el| el.bg(colors().hover).text_color(colors().text))
                                .opacity(1.0 - scene.leave),
                            &t.text.row,
                        )
                        .child("Skip")
                        .child(ui::keycap("esc"))
                        .on_click(cx.listener(|view, _, _, cx| view.skip(cx))),
                    )
                    .child(div().flex_1())
            })
            .child("Interface size")
            .child(button(
                "setup-smaller",
                "−",
                at.checked_sub(1).map(|at| sizes[at]),
            ))
            .child(format!("{percent}%"))
            .child(button("setup-larger", "+", sizes.get(at + 1).copied()))
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
                // Whatever is now found needs no fix, whoever installed it.
                view.fixing
                    .retain(|key, fixing| *fixing == Fixing::Running || !found(&probe, key));
                // Without the scene, a ready computer shows no check at all.
                if view.stage == Stage::Blockers && !view.shown_missing && ready(&probe) {
                    view.stage = Stage::Profile;
                }
                view.probe = Some(probe);
                view.checking = false;
                if std::mem::take(&mut view.recheck) {
                    view.check(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// What follows the scene: the first profile when this computer has all
    /// it needs, else what's missing.
    fn after_scene(&self) -> Stage {
        match &self.probe {
            Some(probe) if ready(probe) => Stage::Profile,
            _ => Stage::Blockers,
        }
    }

    /// Make a fix the person asked for, then check the computer again.
    fn fix(&mut self, key: &'static str, fix: Fix, cx: &mut Context<Self>) {
        if self.fixing.get(key) == Some(&Fixing::Running) {
            return;
        }
        let path_env = self.probe.as_ref().map_or_else(
            || std::env::var("PATH").unwrap_or_default(),
            |probe| probe.path_env.clone(),
        );
        self.fixing.insert(key, Fixing::Running);
        cx.spawn(async move |view, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    match fix {
                        Fix::Package(name) => install::package(name, &path_env),
                        Fix::Restart => install::restart_backbone(&path_env),
                    }
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                match outcome {
                    install::Outcome::Done => {
                        view.fixing.remove(key);
                    }
                    install::Outcome::Opened => {
                        view.fixing.insert(key, Fixing::Opened);
                        view.watch(cx);
                    }
                    install::Outcome::Failed(reason) => {
                        view.fixing.insert(key, Fixing::Failed(reason));
                    }
                }
                view.check(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// While a system installer setup opened is running, check now and then
    /// until what it installs is found.
    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.watching {
            return;
        }
        self.watching = true;
        cx.spawn(async |view, cx| {
            loop {
                cx.background_executor().timer(WATCH).await;
                let open = view.update(cx, |view, cx| {
                    let open = view.fixing.values().any(|fixing| *fixing == Fixing::Opened);
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

    /// Open the workspace window without a profile, for terminals alone.
    fn terminals_only(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if super::browser_window(cx).is_none() {
            let source = crate::sessions::source_socket(None).map_err(|e| e.to_string());
            if let Err(error) = super::open_browser(None, source, cx) {
                tracing::warn!(event = "setup_open_failed", %error, "Workspace window not opened");
                self.open_error = Some("The workspace window could not open. Try again.");
                cx.notify();
                return;
            }
        }
        remember(|settings| settings.setup_done = true);
        window.remove_window();
    }

    /// The first profile exists: open the workspace window, which binds a
    /// workspace to it and carries on with the same steps beside a pane.
    fn hand_off(
        &mut self,
        flow: Entity<Flow>,
        item: Listed,
        connection: Connection,
        client: Client,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if super::browser_window(cx).is_none() {
            let source = crate::sessions::source_socket(None).map_err(|e| e.to_string());
            if let Err(error) = super::open_browser(None, source, cx) {
                tracing::warn!(event = "setup_open_failed", %error, "Workspace window not opened");
                self.open_error = Some(
                    "Your profile is ready, but the workspace window could not open. Open Agent Interface again to continue.",
                );
                cx.notify();
                return;
            }
        }
        let Some(handle) = super::browser_window(cx) else {
            return;
        };
        remember(|settings| settings.setup_done = true);
        adopt(
            handle,
            window.window_handle(),
            flow,
            item,
            connection,
            client,
            cx,
        );
    }

    /// What a row says while its fix runs, or after it failed.
    fn with_fixing(&self, mut item: Item) -> Item {
        match self.fixing.get(item.id) {
            Some(Fixing::Running) => {
                item.mark = ui::Step::Now;
                item.note = match item.action {
                    Some(("Restart", _)) => "Restarting…",
                    Some(("Update", _)) => "Updating…",
                    _ => "Installing…",
                }
                .into();
                item.attention = false;
                item.action = None;
            }
            Some(Fixing::Opened) => {
                item.mark = ui::Step::Now;
                item.note =
                    "Finish the installer that opened. Setup notices when it's done.".into();
                item.attention = false;
                item.action = None;
            }
            Some(Fixing::Failed(reason)) => {
                item.mark = ui::Step::Waits;
                item.note = reason.clone().into();
                item.attention = true;
                item.action = item.action.map(|(_, fix)| ("Try again", fix));
            }
            None => {}
        }
        item
    }

    /// Rows on one card: a mark, what it is, and the fix it offers.
    fn items(&self, items: Vec<Item>, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let mut card = ui::group_card();
        for (index, item) in items.into_iter().enumerate() {
            let Item {
                id,
                name,
                mark,
                note,
                attention,
                action,
            } = self.with_fixing(item);
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s12))
                    .min_h(px(t.size.group_row + t.space.s12))
                    .px(px(t.space.s14))
                    .py(px(t.space.s8))
                    .when(index > 0, |row| {
                        row.border_t(px(t.size.hairline)).border_color(c.line)
                    })
                    .child(ui::step_dot(mark))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_color(c.text).child(name))
                            .child(
                                ui::text(
                                    div().text_color(if attention {
                                        c.attention_text
                                    } else {
                                        c.muted
                                    }),
                                    &t.text.meta,
                                )
                                .child(note),
                            ),
                    )
                    .when_some(action, |row, (label, fix)| {
                        row.child(
                            ui::button(
                                SharedString::from(format!("fix-{id}")),
                                label,
                                Button::Secondary,
                            )
                            .on_click(cx.listener(move |view, _, _, cx| view.fix(id, fix, cx))),
                        )
                    }),
            );
        }
        card
    }

    /// What this computer is missing, each with its fix. Shown only when
    /// something is, and left behind for good once it's done.
    fn blockers_page(&mut self, scene: Option<&scene::Scene>, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let probe = self.probe.as_ref();
        let items = match probe {
            None => vec![Item::new(
                "check",
                "This computer",
                ui::Step::Now,
                "Checking…",
            )],
            Some(probe) => {
                let mut items = Vec::new();
                if probe.tmux.is_none() {
                    items.push(
                        Item::new(
                            "tmux",
                            "tmux",
                            ui::Step::Waits,
                            "Keeps every terminal running, even when the window is closed.",
                        )
                        .fix("Install", Fix::Package("tmux")),
                    );
                }
                if probe.path("git").is_none() {
                    items.push(
                        Item::new(
                            "git",
                            "Git",
                            ui::Step::Waits,
                            "Agents need it to work with your code.",
                        )
                        .fix("Install", Fix::Package("git")),
                    );
                }
                match &probe.backbone {
                    Backbone::Running { .. } if probe.client().is_some() => {}
                    Backbone::Running { .. } | Backbone::NotSetUp => items.push(Item::new(
                        "backbone",
                        "Agent Backbone",
                        ui::Step::Waits,
                        "Agents and profiles run on it. The Agent Interface installer sets it up.",
                    )),
                    Backbone::Unreachable(_) => items.push(
                        Item::new(
                            "backbone",
                            "Agent Backbone",
                            ui::Step::Waits,
                            "Installed, but not answering.",
                        )
                        .fix("Restart", Fix::Restart),
                    ),
                }
                items
            }
        };
        let done = probe.is_some_and(ready);
        self.shown_missing |= probe.is_some() && !done;
        let summary = match (probe, items.len()) {
            (None, _) => line(ui::Step::Now, "Checking this computer…"),
            (Some(_), _) if done => line(ui::Step::Done, "This computer is ready."),
            (Some(_), 1) => line(ui::Step::Waits, "One thing to fix before you start."),
            (Some(_), missing) => line(
                ui::Step::Waits,
                format!("{missing} things to fix before you start."),
            ),
        };
        let terminals = probe.is_some_and(|probe| probe.tmux.is_some());
        let page = div()
            .id("setup-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pt(px(t.size.titlebar))
            .px(px(t.space.s24))
            .pb(px(t.space.s16))
            .child(
                heading(
                    "Before you start",
                    "Agent Interface needs these on this computer. Fix them here; setup notices by itself.",
                )
                .child(summary)
                .when(!done, |page| page.child(self.items(items, cx)))
                .when_some(self.open_error, |page, error| {
                    page.child(div().pt(px(t.space.s12)).text_color(c.attention_text).child(error))
                }),
            );
        let footer = div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .px(px(t.space.s24))
            .py(px(t.space.s16))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .when(terminals && !done, |footer| {
                footer.child(
                    ui::button("terminals-only", "Use terminals only", Button::Secondary).on_click(
                        cx.listener(|view, _, window, cx| view.terminals_only(window, cx)),
                    ),
                )
            })
            .child(div().flex_1())
            .child(if done {
                ui::button("continue", "Continue", Button::Primary).on_click(cx.listener(
                    |view, _, _, cx| {
                        view.stage = Stage::Profile;
                        cx.notify();
                    },
                ))
            } else {
                quiet_button("Continue")
            });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when_some(scene, |page, scene| {
                page.relative()
                    .top(px(scene.page_offset()))
                    .opacity(scene.setup)
            })
            .child(page)
            .child(footer)
    }

    /// The first profile's steps, the same as New profile's. The scene's eye
    /// lands on the profile's mark.
    fn profile_page(
        &mut self,
        scene: Option<&scene::Scene>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        if self.flow.is_none()
            && let Some(client) = self.probe.as_ref().and_then(Probe::client)
        {
            let flow = cx.new(|cx| {
                let mut flow = Flow::new(client.clone(), None, String::new(), None, window, cx);
                flow.for_first_run(cx);
                flow
            });
            // Its name field takes focus as it's made; during the scene keys
            // skip the scene instead, so focus comes back until the scene ends.
            if scene.is_none() {
                window.focus(&flow.read(cx).focus_handle(cx), cx);
            } else {
                window.focus(&self.focus, cx);
            }
            let events = cx.subscribe_in(&flow, window, move |view, flow, event, window, cx| {
                // Until the profile exists nothing else leaves these steps.
                if let flow::Event::Bind(selected) = event {
                    view.hand_off(
                        flow.clone(),
                        selected.item().clone(),
                        selected.connection().clone(),
                        client.clone(),
                        window,
                        cx,
                    );
                }
            });
            self.flow = Some((flow, events));
        }
        let Some((flow, _)) = &self.flow else {
            return div().flex_1();
        };
        let landing = scene.map(|scene| {
            (
                self.marker.clone(),
                if scene.landed() { scene.numeral } else { 0.0 },
            )
        });
        flow.update(cx, |flow, _| flow.set_landing(landing));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c.ground)
            .pt(px(t.size.titlebar))
            .px(px(t.size.window_inset))
            .pb(px(t.size.window_inset))
            .when_some(scene, |page, scene| {
                page.relative()
                    .top(px(scene.page_offset()))
                    .opacity(scene.setup)
            })
            .child(flow.clone())
            .when_some(self.open_error, |page, error| {
                page.child(
                    div()
                        .pt(px(t.space.s8))
                        .text_color(c.attention_text)
                        .child(error),
                )
            })
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let handoff = t.motion.scene_handoff_ms;
        // What follows the scene must be known at its handoff, so the scene
        // waits there, at rest on the icon, until the check has answered.
        if self.stage == Stage::Scene && self.probe.is_none() && self.clock.ms() >= handoff {
            self.clock.set(handoff - 1.0);
        }
        let scene = match self.stage {
            Stage::Scene if self.clock.ms() >= t.motion.scene_ms => {
                self.stage = self.after_scene();
                remember(|settings| settings.intro_seen = true);
                if let Some((flow, _)) = &self.flow {
                    window.focus(&flow.read(cx).focus_handle(cx), cx);
                }
                None
            }
            Stage::Scene => Some(scene::Scene::at(self.clock.ms())),
            _ => None,
        };
        let page = match self.stage {
            Stage::Scene => self.after_scene(),
            stage => stage,
        };
        let root = ui::text(
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(c.surface)
                .text_color(c.text)
                .font_family(t.font.ui.clone()),
            &t.text.row,
        )
        .track_focus(&self.focus)
        .when(scene.is_some(), |root| {
            root.on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                let keys = &event.keystroke.modifiers;
                if !(keys.platform || keys.control || keys.alt) {
                    view.skip(cx);
                    cx.stop_propagation();
                }
            }))
        });
        let content = match page {
            Stage::Profile => self.profile_page(scene.as_ref(), window, cx),
            _ => self.blockers_page(scene.as_ref(), cx),
        };
        // Only the first profile's mark can take the eye; elsewhere it stays
        // in the face as the face leaves.
        let marker = (page == Stage::Profile).then(|| self.marker.clone());
        let body = root
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(content)
                    .when_some(scene, |body, scene| {
                        body.child(
                            scene::overlay(
                                &scene,
                                self.clock.clone(),
                                self.tile.clone(),
                                marker,
                                TAGLINE,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _, cx| view.skip(cx)),
                            ),
                        )
                    }),
            )
            .child(self.scale_controls(scene.as_ref(), cx));
        super::window_controls::frame(body, "Welcome", window, cx)
    }
}

/// Give the first profile's steps to the workspace window. Its library may
/// still be loading, so it is asked again until it can take them.
/// Hand the profile flow to the workspace window once its library has
/// loaded, then close setup, so there is no moment without either.
fn adopt(
    handle: WindowHandle<super::Browser>,
    setup: gpui::AnyWindowHandle,
    flow: Entity<Flow>,
    item: Listed,
    connection: Connection,
    client: Client,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        for _ in 0..ADOPT_TRIES {
            let adopted = handle
                .update(cx, |browser, window, cx| {
                    browser.adopt_profile_flow(
                        flow.clone(),
                        item.clone(),
                        connection.clone(),
                        client.clone(),
                        window,
                        cx,
                    )
                })
                .unwrap_or(true);
            if adopted {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
        }
        let _ = setup.update(cx, |_, window, _| window.remove_window());
    })
    .detach();
}

/// Whether this computer has what a profile needs: tmux, Git, and an
/// answering Agent Backbone setup can reach.
fn ready(probe: &Probe) -> bool {
    check_items(probe).all(|ready| ready) && probe.client().is_some()
}

fn heading(title: &str, note: &str) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_col()
        .child(ui::text(div().text_color(c.text), &t.text.page_title).child(title.to_owned()))
        .child(
            ui::text(div().pb(px(t.space.s8)).text_color(c.text_2), &t.text.body)
                .child(note.to_owned()),
        )
}

/// One thing a step checks: its mark, its name, what its state means for the
/// person, and the fix it offers.
struct Item {
    id: &'static str,
    name: &'static str,
    mark: ui::Step,
    note: SharedString,
    attention: bool,
    action: Option<(&'static str, Fix)>,
}

impl Item {
    fn new(
        id: &'static str,
        name: &'static str,
        mark: ui::Step,
        note: impl Into<SharedString>,
    ) -> Self {
        Self {
            id,
            name,
            mark,
            note: note.into(),
            attention: false,
            action: None,
        }
    }

    fn fix(mut self, label: &'static str, fix: Fix) -> Self {
        self.action = Some((label, fix));
        self
    }
}

/// Whether each thing the Check step needs is ready: tmux, Git, Backbone.
fn check_items(probe: &Probe) -> impl Iterator<Item = bool> {
    [
        probe.tmux.is_some(),
        probe.path("git").is_some(),
        matches!(probe.backbone, Backbone::Running { .. }),
    ]
    .into_iter()
}

/// Whether the probe now finds what a fix was for.
fn found(probe: &Probe, key: &str) -> bool {
    match key {
        "tmux" => probe.tmux.is_some(),
        "backbone" => matches!(probe.backbone, Backbone::Running { .. }),
        command => probe.path(command).is_some(),
    }
}

/// A page's one-line summary, led by the same mark its rows use.
fn line(mark: ui::Step, text: impl Into<SharedString>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .items_center()
        .gap(px(t.space.s10))
        .pt(px(t.space.s4))
        .pb(px(t.space.s12))
        .child(ui::step_dot(mark))
        .child(ui::text(div().text_color(c.text), &t.text.row_selected).child(text.into()))
}

/// The main button while it waits: legible, quiet, and inactive.
fn quiet_button(label: &'static str) -> gpui::Stateful<gpui::Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id("continue")
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(t.size.button))
            .px(px(t.space.s14))
            .rounded(px(t.radius.row))
            .bg(c.field)
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .text_color(c.muted),
        &t.text.row,
    )
    .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enlarged_setup_stays_visible_for_both_resize_anchors() {
        // A 768px display at 200%, with setup near either vertical edge.
        let visible = Bounds::new(point(px(0.), px(12.)), size(px(683.), px(360.)));
        for top in [px(20.), px(120.)] {
            let current = Bounds::new(point(px(170.), top), size(px(340.), px(240.)));
            for bottom_anchor in [false, true] {
                let extent =
                    size(px(680.), px(480.)).min(&resize_limit(current, visible, bottom_anchor));
                let y = if bottom_anchor {
                    current.bottom() - extent.height
                } else {
                    top
                };
                let resized = Bounds::new(point(current.origin.x, y), extent);
                assert!(resized.origin.y >= visible.origin.y);
                assert!(resized.bottom() <= visible.bottom());
                assert!(resized.right() <= visible.right());
            }
        }
    }

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
