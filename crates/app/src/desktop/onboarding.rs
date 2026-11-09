//! First-run onboarding: a short opening scene, then a setup window that
//! checks this computer, finds agent tools and shows the main shortcuts.
//! It opens instead of the workspace window on a fresh install, and from
//! the application menu at any time after.

mod first;
mod install;
pub(super) mod probe;
mod scene;
mod setup;

use std::{collections::HashMap, time::Duration};

use gpui::{
    Animation, AnimationExt, App, Bounds, Context, Entity, FocusHandle, Global, KeyDownEvent,
    MouseButton, Pixels, SharedString, Size, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};

use self::probe::{Backbone, Probe, TOOLS};
use super::{
    components::{self as ui, Button},
    design::{colors, tokens},
    settings::{self, Settings},
    shortcuts::{self, Command},
};

const TAGLINE: &str = "Your agents. Working together. On your machine.";
const STEPS: [&str; 4] = ["Check", "Agent tools", "Your agent", "Shortcuts"];
/// How often setup checks again while a system installer it opened runs.
const WATCH: Duration = Duration::from_secs(4);

/// Something setup can fix for the person when they ask.
#[derive(Clone, Copy)]
enum Fix {
    /// Install Git or tmux with the system's installer.
    Package(&'static str),
    /// Install an agent tool with its official command.
    Tool(&'static str),
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
    Scene,
    Step(usize),
}

pub struct Onboarding {
    stage: Stage,
    probe: Option<Probe>,
    checking: bool,
    /// Another check was asked for while one ran.
    recheck: bool,
    /// Fixes by what they fix: "git", "tmux", "backbone" or a tool's command.
    fixing: HashMap<&'static str, Fixing>,
    watching: bool,
    /// Pages rise into place once the person has moved between them.
    navigated: bool,
    /// The system asks for less motion.
    still: bool,
    open_error: Option<&'static str>,
    first: Option<Entity<first::FirstAgent>>,
    first_ready: bool,
    first_agent: Option<String>,
    first_events: Option<Subscription>,
    /// The opening scene's clock; Skip moves it to the handoff.
    clock: scene::Clock,
    /// Takes the keys that skip the scene.
    focus: FocusHandle,
    /// Where the face and the first step's marker were laid out, for the
    /// open eye's flight between them.
    tile: scene::Place,
    marker: scene::Place,
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
    let still = reduce_motion();
    let intro = intro && !still;
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
                        if window.is_window_active() && matches!(view.stage, Stage::Step(0 | 1)) {
                            view.check(cx);
                        }
                    });
                let mut view = Onboarding {
                    stage: if intro { Stage::Scene } else { Stage::Step(0) },
                    probe: None,
                    checking: false,
                    recheck: false,
                    fixing: HashMap::new(),
                    watching: false,
                    navigated: false,
                    still,
                    open_error: None,
                    first: None,
                    first_ready: false,
                    first_agent: None,
                    first_events: None,
                    clock: Default::default(),
                    focus,
                    tile: Default::default(),
                    marker: Default::default(),
                    _activation: activation,
                };
                // The scene covers the first check, so step one opens with answers.
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
                if let Some(first) = &view.first {
                    first.update(cx, |first, cx| first.refresh(&probe, cx));
                }
                // Whatever is now found needs no fix, whoever installed it.
                view.fixing
                    .retain(|key, fixing| *fixing == Fixing::Running || !found(&probe, key));
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
                        Fix::Tool(command) => install::tool(command, &path_env),
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

    /// Move to another step; pages rise into place from here on.
    fn go(&mut self, index: usize, cx: &mut Context<Self>) {
        self.stage = Stage::Step(index);
        self.navigated = true;
        cx.notify();
    }

    /// What a finished step found, under its name in the rail.
    fn found_line(&self, index: usize) -> Option<SharedString> {
        let probe = self.probe.as_ref()?;
        Some(match index {
            0 => match check_items(probe).filter(|ready| !ready).count() {
                0 => "All ready".into(),
                missing => format!("{missing} to fix").into(),
            },
            1 => {
                let names: Vec<&str> = TOOLS
                    .iter()
                    .filter(|(command, ..)| probe.path(command).is_some())
                    .map(|(_, name, _)| *name)
                    .collect();
                if names.is_empty() {
                    "None yet".into()
                } else {
                    names.join(", ").into()
                }
            }
            2 => match (&self.first_agent, self.first_ready) {
                (Some(name), true) => name.clone().into(),
                _ => "Terminals for now".into(),
            },
            _ => return None,
        })
    }

    /// Open the workspace before completing setup, so a failed window open
    /// leaves the last page usable and can be retried.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if super::browser_window(cx).is_none() {
            let source = crate::sessions::source_socket(None).map_err(|e| e.to_string());
            if let Err(error) = super::open_browser(None, source, cx) {
                tracing::warn!(event = "setup_open_failed", %error, "Workspace window not opened");
                self.open_error = Some("The workspace window could not open. Try again.");
                cx.notify();
                return;
            }
        }
        if let Some(name) = &self.first_agent
            && let Err(error) = show_agent(name.clone(), cx)
        {
            self.open_error = Some(error);
            cx.notify();
            return;
        }
        remember(|settings| settings.setup_done = true);
        window.remove_window();
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

    fn check_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let probe = self.probe.as_ref();
        let checking = |id, name| Item::new(id, name, ui::Step::Now, "Checking…");
        let items = match probe {
            None => vec![
                checking("tmux", "tmux"),
                checking("git", "Git"),
                checking("backbone", "Agent Backbone"),
            ],
            Some(probe) => vec![
                if probe.tmux.is_some() {
                    Item::new(
                        "tmux",
                        "tmux",
                        ui::Step::Done,
                        "Keeps every terminal running, even when the window is closed.",
                    )
                } else {
                    Item::new("tmux", "tmux", ui::Step::Waits, "Terminals need it.")
                        .fix("Install", Fix::Package("tmux"))
                },
                if probe.path("git").is_some() {
                    Item::new(
                        "git",
                        "Git",
                        ui::Step::Done,
                        "Lets agents work with your code.",
                    )
                } else {
                    Item::new(
                        "git",
                        "Git",
                        ui::Step::Waits,
                        "Agents need it to work with code.",
                    )
                    .fix("Install", Fix::Package("git"))
                },
                match &probe.backbone {
                    Backbone::Running { agents } => Item::new(
                        "backbone",
                        "Agent Backbone",
                        ui::Step::Done,
                        match agents {
                            0 => "Running. Your first agent comes next.".into(),
                            1 => "Running with 1 agent, which stays as it is.".into(),
                            n => format!("Running with {n} agents, which stay as they are."),
                        },
                    ),
                    Backbone::Unreachable(_) => Item::new(
                        "backbone",
                        "Agent Backbone",
                        ui::Step::Waits,
                        "Installed, but not answering.",
                    )
                    .fix("Restart", Fix::Restart),
                    Backbone::NotSetUp => Item::new(
                        "backbone",
                        "Agent Backbone",
                        ui::Step::Waits,
                        "Not set up. The Agent Interface installer adds it; terminals work without it.",
                    ),
                },
            ],
        };
        let summary = match probe {
            None => line(ui::Step::Now, "Checking this computer…"),
            Some(probe) => match check_items(probe).filter(|ready| !ready).count() {
                0 => line(ui::Step::Done, "This computer is ready."),
                1 => line(ui::Step::Waits, "One thing needs attention."),
                missing => line(ui::Step::Waits, format!("{missing} things need attention.")),
            },
        };
        heading(
            "Check this computer",
            "Terminals run in tmux, agents run through Agent Backbone, and Git lets them work with code.",
        )
        .child(summary)
        .child(self.items(items, cx))
    }

    fn tools_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let probe = self.probe.as_ref();
        let mut ready = Vec::new();
        let items = TOOLS
            .iter()
            .map(
                |&(command, name, install)| match probe.map(|probe| probe.path(command)) {
                    None => Item::new(command, name, ui::Step::Now, "Checking…"),
                    Some(Some(path)) => {
                        ready.push(name);
                        let item =
                            Item::new(command, name, ui::Step::Done, install::home_relative(path));
                        // Agent setup needs a recent OpenCode; its installer also updates.
                        if command == "opencode" {
                            item.fix("Update", Fix::Tool(install))
                        } else {
                            item
                        }
                    }
                    Some(None) => Item::new(command, name, ui::Step::Ahead, about(command))
                        .fix("Install", Fix::Tool(install)),
                },
            )
            .collect();
        let summary = match (probe, ready.as_slice()) {
            (None, _) => line(ui::Step::Now, "Checking your agent tools…"),
            (Some(_), []) => line(
                ui::Step::Ahead,
                "None yet. Install one to create an agent; terminals work without one.",
            ),
            (Some(_), names) => line(ui::Step::Done, format!("{} ready.", list(names))),
        };
        heading(
            "Agent tools",
            "Your agents run in one of these. Each asks you to sign in the first time it opens.",
        )
        .child(summary)
        .child(self.items(items, cx))
    }

    fn shortcuts_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let mut grid = div().grid().grid_cols(2).gap(px(t.space.s10));
        for (command, about) in [
            (Command::Jump, "Find any tab, pane, agent or command."),
            (Command::NewTab, "Open a terminal or a view."),
            (Command::SplitRight, "Put two panes side by side."),
            (Command::FocusPanel, "Move to your tabs and agents."),
        ] {
            let label = shortcuts::COMMANDS
                .iter()
                .find(|(each, ..)| *each == command)
                .map_or("", |(_, _, label, ..)| label);
            grid = grid.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s8))
                    .p(px(t.space.s14))
                    .rounded(px(t.radius.popover))
                    .bg(c.panel)
                    .border(px(t.size.hairline))
                    .border_color(c.line)
                    .child(div().flex().child(ui::keycap(shortcuts::label(command))))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                ui::text(div().text_color(c.text), &t.text.row_selected)
                                    .child(label),
                            )
                            .child(ui::text(div().text_color(c.muted), &t.text.meta).child(about)),
                    ),
            );
        }
        let summary = match (&self.first_agent, self.first_ready) {
            (Some(name), true) => line(
                ui::Step::Done,
                format!("{name} opens in its terminal with your workspace."),
            ),
            _ => line(
                ui::Step::Done,
                "Your terminals are ready. Add an agent any time in Settings › Profiles.",
            ),
        };
        heading(
            "You're ready",
            "The keys you'll use most. Change any of them in Settings.",
        )
        .child(summary)
        .child(grid)
        .child(
            div().flex().pt(px(t.space.s16)).child(
                ui::button("open-settings", "Change in Settings", Button::Secondary)
                    .on_click(cx.listener(|_, _, _, cx| cx.defer(super::settings_window::show))),
            ),
        )
    }

    /// The steps beside the page, on one track: a number while ahead or
    /// current, a check ring and what the step found once it is done. During
    /// the opening scene it fades in with setup, and the first step's marker
    /// waits for the face's eye to land.
    fn rail(&self, current: usize, scene: Option<&scene::Scene>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        // Each marker sits centred on its step's first line.
        let mark_top = (t.text.row.size * LINE_HEIGHT - t.size.agent_symbol) / 2.0;
        let mut rail = div()
            .w(px(t.size.settings_nav))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .px(px(t.space.s12))
            .pt(px(t.size.titlebar))
            .bg(c.panel)
            .border_r(px(t.size.hairline))
            .border_color(c.line)
            .child(
                ui::text(
                    div()
                        .pl(px(t.size.agent_symbol + t.space.s10))
                        .pb(px(t.space.s10))
                        .text_color(c.muted),
                    &t.text.label,
                )
                .child("SETUP"),
            );
        for (index, name) in STEPS.iter().enumerate() {
            // A finished Check that left something to fix says so, as its rows do.
            let unfinished = index == 0
                && self
                    .probe
                    .as_ref()
                    .is_some_and(|probe| check_items(probe).any(|ready| !ready));
            let state = if index < current && unfinished {
                ui::Step::Waits
            } else if index < current {
                ui::Step::Done
            } else if index == current {
                ui::Step::Now
            } else {
                ui::Step::Ahead
            };
            let mut mark = step_mark(index, state);
            if index == 0
                && let Some(scene) = scene
            {
                let mut numeral = c.on_accent;
                numeral.a *= scene.numeral;
                mark = mark
                    .relative()
                    .text_color(numeral)
                    .opacity(if scene.landed() { 1.0 } else { 0.0 })
                    .child(scene::marker_place(self.marker.clone()));
            }
            let (color, style) = match state {
                ui::Step::Now => (c.text, &t.text.row_selected),
                ui::Step::Done | ui::Step::Waits => (c.text_2, &t.text.row),
                _ => (c.muted, &t.text.row),
            };
            let found = (index < current).then(|| self.found_line(index)).flatten();
            rail = rail.child(
                div()
                    .relative()
                    .flex()
                    .gap(px(t.space.s10))
                    .pb(px(t.space.s14))
                    .when(index + 1 < STEPS.len(), |step| {
                        step.child(
                            div()
                                .absolute()
                                .left(px((t.size.agent_symbol - t.size.hairline) / 2.0))
                                .top(px(mark_top + t.size.agent_symbol + t.space.s4))
                                .bottom(px(t.space.s2))
                                .w(px(t.size.hairline))
                                .bg(c.line_strong),
                        )
                    })
                    .child(div().flex_shrink_0().pt(px(mark_top)).child(mark))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(ui::text(div().text_color(color), style).child(*name))
                            .when_some(found, |text, found| {
                                text.child(
                                    ui::text(
                                        div()
                                            .text_color(c.muted)
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis(),
                                        &t.text.meta,
                                    )
                                    .child(found),
                                )
                            }),
                    ),
            );
        }
        rail.when_some(scene, |rail, scene| rail.opacity(scene.setup))
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        // The scene runs on its own clock and hands over to the first step.
        let scene = match self.stage {
            Stage::Scene if self.clock.ms() >= t.motion.scene_ms => {
                self.stage = Stage::Step(0);
                remember(|settings| settings.intro_seen = true);
                None
            }
            Stage::Scene => Some(scene::Scene::at(self.clock.ms())),
            Stage::Step(_) => None,
        };
        // Setup's first step is laid out under the scene, ready to take over.
        let index = match self.stage {
            Stage::Step(index) => index,
            Stage::Scene => 0,
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
        if index == 2
            && self.first.is_none()
            && let Some(probe) = &self.probe
            && let Ok(Some(directory)) = crate::roster::provider_directory(true)
        {
            let executable =
                setup::active_engine(&directory).or_else(|| probe.path("backbone").cloned());
            let existing = matches!(probe.backbone, Backbone::Running { agents } if agents > 0);
            let first = cx.new(|cx| {
                first::FirstAgent::new(
                    executable,
                    probe.path_env.clone(),
                    directory,
                    existing,
                    window,
                    cx,
                )
            });
            self.first_events = Some(cx.subscribe(&first, |view, _, event, cx| {
                match event {
                    first::FirstEvent::RefreshTools => view.check(cx),
                    first::FirstEvent::Install(command) => {
                        if let Some(&(key, _, install)) =
                            TOOLS.iter().find(|(each, ..)| *each == command.as_str())
                        {
                            view.fix(key, Fix::Tool(install), cx);
                        }
                    }
                    first::FirstEvent::Pending => {
                        view.first_ready = false;
                        view.first_agent = None;
                    }
                    first::FirstEvent::Ready(name) => {
                        view.first_ready = true;
                        view.first_agent = Some(name.clone());
                    }
                    first::FirstEvent::Existing => {
                        // Choosing existing agents skips creation. Returning to
                        // this form must still allow starting a new agent.
                        view.first_ready = false;
                        view.first_agent = None;
                        view.go(3, cx);
                    }
                    first::FirstEvent::OpenTerminal(name) => {
                        view.open_error = show_agent(name.clone(), cx).err();
                    }
                }
                cx.notify();
            }));
            self.first = Some(first);
        }
        if let Some(first) = &self.first {
            let installing: Vec<&'static str> = TOOLS
                .iter()
                .filter(|(command, ..)| self.fixing.get(command) == Some(&Fixing::Running))
                .map(|(command, ..)| *command)
                .collect();
            let failed: Vec<(&'static str, String)> = TOOLS
                .iter()
                .filter_map(|(command, ..)| match self.fixing.get(command) {
                    Some(Fixing::Failed(reason)) => Some((*command, reason.clone())),
                    _ => None,
                })
                .collect();
            first.update(cx, |first, cx| first.set_installs(installing, failed, cx));
        }
        let page = match index {
            0 => self.check_step(cx),
            1 => self.tools_step(cx),
            2 => match &self.first {
                Some(first) => div().child(first.clone()),
                None if self.probe.is_none() => heading("Your first agent", "Checking your tools and shell environment…"),
                None => heading("Your first agent", "Agent Backbone isn't available, so an agent can't be made yet. Use terminals for now; you can add an agent later from Settings › Profiles.")
                    .when(self.probe.as_ref().is_some_and(|probe| matches!(probe.backbone, Backbone::Running { agents } if agents > 0)), |page| {
                        page.child(ui::button("keep-existing-agents", "Use my existing agents", Button::Secondary)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.first_ready = true;
                                view.first_agent = None;
                                cx.notify();
                            })))
                    }),
            },
            _ => self.shortcuts_step(cx),
        }
        .when_some(self.open_error, |page, error| {
            page.child(
                div()
                    .pt(px(t.space.s12))
                    .text_color(c.attention_text)
                    .child(error),
            )
        });
        // Where this page sits in setup, above its title.
        let page = div()
            .flex()
            .flex_col()
            .child(
                ui::text(div().pb(px(t.space.s2)).text_color(c.muted), &t.text.label)
                    .child(format!("STEP {} OF {}", index + 1, STEPS.len())),
            )
            .child(page);
        // After Continue or Back the new page rises into place.
        let page = if self.navigated && !self.still {
            let rise = t.space.s6;
            div()
                .relative()
                .child(page)
                .with_animation(
                    ("setup-step", index),
                    Animation::new(Duration::from_secs_f32(t.motion.page_ms / 1000.0))
                        .with_easing(gpui::ease_out_quint()),
                    move |page, delta| page.opacity(delta).top(px(rise * (1.0 - delta))),
                )
                .into_any_element()
        } else {
            page.into_any_element()
        };
        let last = index + 1 == STEPS.len();
        // On the agent step the main button starts or repairs setup until the
        // agent runs. While it waits, the reason sits beside a quiet button.
        let primary = match (index, &self.first) {
            (2, _) if self.first_ready => first::Primary::Continue,
            (2, Some(first)) => first.read(cx).primary(),
            (2, None) if self.probe.is_none() => first::Primary::Waiting("Checking…".into()),
            (2, None) => first::Primary::Waiting("Agent Backbone isn't available".into()),
            _ => first::Primary::Continue,
        };
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
            .when(index > 0, |el| {
                el.child(
                    ui::button("back", "Back", Button::Secondary)
                        .on_click(cx.listener(move |view, _, _, cx| view.go(index - 1, cx))),
                )
            })
            .when(index < 2, |el| {
                el.child(
                    ui::button(
                        "check-again",
                        if self.checking {
                            "Checking…"
                        } else {
                            "Check again"
                        },
                        Button::Secondary,
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.check(cx))),
                )
            })
            .when_some(
                match &primary {
                    first::Primary::Waiting(reason) => Some(reason.clone()),
                    _ => None,
                },
                |el, reason| el.child(waiting_note(reason)),
            )
            .child(div().flex_1())
            .when(
                index == 2
                    && !self.first_ready
                    && self
                        .first
                        .as_ref()
                        .is_none_or(|first| first.read(cx).can_use_terminals()),
                |footer| {
                    footer.child(
                        ui::button(
                            "terminals-for-now",
                            "Use terminals for now",
                            Button::Secondary,
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.first_agent = None;
                            // Skipping advances navigation without declaring an
                            // unfinished agent ready; Back must still offer Start.
                            view.go(3, cx);
                        })),
                    )
                },
            )
            .child(match primary {
                first::Primary::Continue => ui::button(
                    "continue",
                    if last {
                        "Open Agent Interface"
                    } else {
                        "Continue"
                    },
                    Button::Primary,
                )
                .on_click(cx.listener(move |view, _, window, cx| {
                    if last {
                        view.finish(window, cx);
                    } else {
                        view.go(index + 1, cx);
                    }
                })),
                first::Primary::Waiting(_) => {
                    quiet_button(if index == 2 { "Start" } else { "Continue" })
                }
                first::Primary::Start(label) => ui::button("continue", label, Button::Primary)
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(first) = &view.first {
                            first.update(cx, |first, cx| first.start(cx));
                        }
                    })),
                first::Primary::ChangeTool(label) => ui::button("continue", label, Button::Primary)
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(first) = &view.first {
                            first.update(cx, |first, cx| first.edit_choices(cx));
                        }
                    })),
            });
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when_some(scene, |main, scene| main.opacity(scene.setup))
            .child(
                div()
                    .id("setup-page")
                    .flex_1()
                    .min_h_0()
                    .pt(px(t.size.titlebar))
                    .px(px(t.space.s24))
                    .pb(px(t.space.s16))
                    .overflow_y_scroll()
                    .when_some(scene, |page, scene| {
                        page.relative().top(px(scene.page_offset()))
                    })
                    .child(page),
            )
            .child(footer);
        let body = root
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.rail(index, scene.as_ref()))
                    .child(main)
                    .when_some(scene, |body, scene| {
                        body.child(
                            scene::overlay(
                                &scene,
                                self.clock.clone(),
                                self.tile.clone(),
                                self.marker.clone(),
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

/// Show the actual agent terminal for login/permissions or the introduction.
/// The saved library may still be loading when setup opens the workspace.
fn default_profile() -> Option<String> {
    use agent_interface_roster::profiles::Kind;
    probe::profile_client()?
        .list(&|| true)
        .ok()?
        .into_iter()
        .find(|item| item.kind == Kind::LegacyDefault)
        .map(|item| item.profile_id)
}

fn show_agent(name: String, cx: &mut App) -> Result<(), &'static str> {
    if super::browser_window(cx).is_none() {
        let source = crate::sessions::source_socket(None).map_err(|error| error.to_string());
        super::open_browser(None, source, cx)
            .map_err(|_| "The agent window could not open. Try again.")?;
    }
    let handle = super::browser_window(cx).ok_or("The agent window could not open. Try again.")?;
    let default = cx.background_executor().spawn(async { default_profile() });
    cx.spawn(async move |cx| {
        // Setup adds the first agent to Default, the profile a new install starts in.
        if let Some(profile_id) = default.await {
            for _ in 0..50 {
                let opened = handle
                    .update(cx, |browser, window, cx| {
                        browser.open_setup_agent(&name, &profile_id, "Default", window, cx)
                    })
                    .unwrap_or(true);
                if opened {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        }
        let _ = handle.update(cx, |browser, _, cx| {
            browser.save_notice = Some(format!(
                "Could not open {name}. Check the workspace connection, then reopen it from setup."
            ));
            cx.notify();
        });
    })
    .detach();
    Ok(())
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

/// GPUI's default line height, relative to the text size.
const LINE_HEIGHT: f32 = 1.618;

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

/// What an agent tool is, for someone who has not met it.
fn about(command: &str) -> &'static str {
    match command {
        "claude" => "Anthropic's coding agent.",
        "codex" => "OpenAI's coding agent.",
        "gemini" => "Google's coding agent. Needs Node.js 20 or newer.",
        _ => "An open-source coding agent.",
    }
}

/// Names as a sentence: "A", "A and B", "A, B and C".
fn list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
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

/// Why the main button waits, beside it: a turning arc while something runs.
fn waiting_note(reason: SharedString) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let running = reason.ends_with('…');
    ui::text(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s8))
            .min_w_0()
            .text_color(c.muted),
        &t.text.status,
    )
    .when(running, |note| note.child(ui::step_dot(ui::Step::Now)))
    .child(
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(reason),
    )
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

/// A step's marker: its number while current or ahead, a check ring once
/// done, or `!` when it was left with something to fix, matching the step
/// dots New profile uses.
fn step_mark(index: usize, state: ui::Step) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let ring = t.size.minimap_stroke * 1.5;
    let base = div()
        .flex_shrink_0()
        .size(px(t.size.agent_symbol))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center();
    match state {
        ui::Step::Done => base.border(px(ring)).border_color(c.ok).child(ui::icon(
            "check",
            t.size.agent_symbol - t.space.s6,
            c.ok,
        )),
        ui::Step::Now => ui::text(base.bg(c.accent).text_color(c.on_accent), &t.text.step_key)
            .child(format!("{}", index + 1)),
        ui::Step::Waits => ui::text(
            base.bg(c.attention).text_color(c.on_attention),
            &t.text.step_key,
        )
        .child("!"),
        _ => ui::text(
            base.border(px(ring))
                .border_color(c.line_strong)
                .text_color(c.muted),
            &t.text.step_key,
        )
        .child(format!("{}", index + 1)),
    }
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
