//! First-run onboarding: a short opening scene, then a setup window that
//! checks this computer, finds agent tools and shows the main shortcuts.
//! It opens instead of the workspace window on a fresh install, and from
//! the application menu at any time after.

mod first;
mod probe;
mod setup;

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, App, Bounds, BoxShadow, ClipboardItem, Context, Entity, Global,
    Pixels, Rgba, SharedString, Size, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};

use self::probe::{Backbone, Probe, TOOLS};
use super::{
    components::{self as ui, Button, Tone},
    design::{colors, tokens},
    settings::{self, Settings},
    shortcuts::{self, Command},
};

/// The opening scene's length; it plays once, without a skip.
const SCENE: Duration = Duration::from_millis(8000);
const TAGLINE: &str = "Your agents. Working together. On your machine.";
const STEPS: [&str; 4] = ["Check", "Agent tools", "Your agent", "Shortcuts"];

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
    copied: Option<&'static str>,
    open_error: Option<&'static str>,
    first: Option<Entity<first::FirstAgent>>,
    first_ready: bool,
    first_agent: Option<String>,
    first_events: Option<Subscription>,
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
            size(px(t.size.settings_width), px(t.size.settings_height)),
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
        .open_window(options, |_, cx| {
            cx.new(|cx| {
                let mut view = Onboarding {
                    stage: if intro { Stage::Scene } else { Stage::Step(0) },
                    probe: None,
                    checking: false,
                    copied: None,
                    open_error: None,
                    first: None,
                    first_ready: false,
                    first_agent: None,
                    first_events: None,
                };
                // The scene covers the first check, so step one opens with answers.
                view.check(cx);
                if intro {
                    cx.spawn(async |view, cx| {
                        cx.background_executor().timer(SCENE).await;
                        let _ = view.update(cx, |view, cx| {
                            view.stage = Stage::Step(0);
                            remember(|settings| settings.intro_seen = true);
                            cx.notify();
                        });
                    })
                    .detach();
                }
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
    fn scale_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
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
                                                px(tokens().size.settings_height),
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
                                        px(tokens().size.settings_height),
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
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_end()
            .gap(px(t.space.s8))
            .px(px(t.space.s24))
            .py(px(t.space.s8))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .child("Interface size")
            .child(button(
                "setup-smaller",
                "−",
                at.checked_sub(1).map(|at| sizes[at]),
            ))
            .child(format!("{percent}%"))
            .child(button("setup-larger", "+", sizes.get(at + 1).copied()))
    }

    fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.spawn(async |view, cx| {
            let probe = cx.background_executor().spawn(async { Probe::run() }).await;
            let _ = view.update(cx, |view, cx| {
                if let Some(first) = &view.first {
                    first.update(cx, |first, cx| first.refresh(&probe, cx));
                }
                view.probe = Some(probe);
                view.checking = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn copy(&mut self, id: &'static str, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
        self.copied = Some(id);
        cx.notify();
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

    fn scene(&self) -> gpui::AnyElement {
        div()
            .size_full()
            .with_animation("opening-scene", Animation::new(SCENE), |scene, delta| {
                scene.child(frame(Frame::at(delta * SCENE.as_millis() as f32)))
            })
            .into_any_element()
    }

    /// A check row: name, what it is for, its state, and an optional command to copy.
    fn row(
        &self,
        id: &'static str,
        name: &str,
        note: impl Into<SharedString>,
        state: (Tone, &str),
        command: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let copied = self.copied == Some(id);
        div()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .py(px(t.space.s10))
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_color(c.text).child(name.to_owned()))
                            .child(
                                ui::text(div().text_color(c.muted), &t.text.status)
                                    .child(note.into()),
                            ),
                    )
                    .child(match state.0 {
                        // Ready reads as done, in the same green as a finished step.
                        Tone::Tag => div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s4))
                            .text_color(c.ok)
                            .child(ui::icon("check", t.size.icon, c.ok))
                            .child(state.1.to_owned()),
                        tone => ui::tag(state.1.to_owned(), tone),
                    }),
            )
            .when_some(command, |el, command| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s8))
                        .child(
                            ui::text(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px(px(t.space.s8))
                                    .py(px(t.space.s4))
                                    .rounded(px(t.radius.row))
                                    .bg(c.field)
                                    .text_color(c.code)
                                    .font_family(t.font.mono.clone()),
                                &t.text.status,
                            )
                            .child(command),
                        )
                        .child(
                            ui::button(
                                SharedString::from(format!("copy-{id}")),
                                if copied { "Copied" } else { "Copy" },
                                Button::Secondary,
                            )
                            .on_click(
                                cx.listener(move |view, _, _, cx| view.copy(id, command, cx)),
                            ),
                        ),
                )
            })
    }

    fn check_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let probe = self.probe.as_ref();
        let found = |command| probe.and_then(|probe| probe.path(command)).is_some();
        let tmux = probe.is_some_and(|probe| probe.tmux.is_some());
        let ready = (Tone::Tag, "Ready");
        let missing = (Tone::Attention, "Not found");
        let mac = cfg!(target_os = "macos");
        let mut page = heading(
            "Check this computer",
            "Terminals run in tmux; agents run through Agent Backbone.",
        );
        page = page.child(self.row(
            "tmux",
            "tmux",
            "Keeps every terminal running, even when the window is closed.",
            if tmux { ready } else { missing },
            (probe.is_some() && !tmux).then_some(if mac {
                "brew install tmux"
            } else {
                "sudo apt install tmux"
            }),
            cx,
        ));
        let (state, note, command): (_, String, _) = match probe.map(|probe| &probe.backbone) {
            None => (
                (Tone::Plain, "Checking"),
                "Looking for Agent Backbone.".into(),
                None,
            ),
            Some(Backbone::Running { agents }) => (
                ready,
                match agents {
                    0 => "Running. Your first agent comes next.".into(),
                    1 => "Running with 1 agent. Your agents stay as they are.".into(),
                    n => format!("Running with {n} agents. Your agents stay as they are."),
                },
                None,
            ),
            Some(Backbone::Unreachable(error)) => (
                (Tone::Attention, "Not answering"),
                format!("Set up, but not answering ({error})."),
                Some("backbone doctor"),
            ),
            Some(Backbone::NotSetUp) => (
                (Tone::Attention, "Not set up"),
                "Run the Agent Interface installer again to finish setting up Agent Backbone."
                    .into(),
                None,
            ),
        };
        page = page.child(self.row("backbone", "Agent Backbone", note, state, command, cx));
        page = page.child(self.row(
            "git",
            "Git",
            "Agents use it for code work.",
            if found("git") { ready } else { missing },
            (!found("git")).then_some(if mac {
                "xcode-select --install"
            } else {
                "sudo apt install git"
            }),
            cx,
        ));
        page.child(self.row(
            "gh",
            "GitHub CLI",
            "Optional: lets agents work with issues and pull requests.",
            if found("gh") {
                ready
            } else {
                (Tone::Plain, "Optional")
            },
            (!found("gh")).then_some(if mac {
                "brew install gh"
            } else {
                "sudo apt install gh"
            }),
            cx,
        ))
    }

    fn tools_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let probe = self.probe.as_ref();
        let mut page = heading(
            "Agent tools",
            "Your agents run in these. You sign in inside each tool the first time you open it.",
        );
        let mut any = false;
        for (command, name, install) in TOOLS {
            let path = probe.and_then(|probe| probe.path(command));
            any |= path.is_some();
            let mut note = match path {
                Some(path) => format!("Installed at {}", path.display()),
                None if cfg!(target_os = "linux") => {
                    "Not installed. Run this in a Linux terminal; on Windows, use WSL.".into()
                }
                None => "Not installed. Paste this in a terminal to add it.".into(),
            };
            if *command == "opencode" {
                note.push_str(" Agent setup requires OpenCode 2.1.4 or newer; this command installs or updates OpenCode 2.");
            } else if *command == "gemini" && path.is_none() {
                note.push_str(" Gemini CLI requires Node.js 20 or newer with npm. Claude Code, Codex and OpenCode use standalone installers.");
            }
            let state = match (probe, path) {
                (None, _) => (Tone::Plain, "Checking"),
                (_, Some(_)) => (Tone::Tag, "Installed"),
                (_, None) => (Tone::Plain, "Not installed"),
            };
            page = page.child(self.row(
                command,
                name,
                note,
                state,
                (path.is_none() || *command == "opencode").then_some(*install),
                cx,
            ));
        }
        page.when(probe.is_some() && !any, |el| {
            el.child(
                ui::text(
                    div().pt(px(t.space.s12)).text_color(c.text_2),
                    &t.text.status,
                )
                .child("You can add one later. Terminals work without agents."),
            )
        })
    }

    fn shortcuts_step(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let mut page = heading(
            "Shortcuts",
            "The keys you will use most. Change any of them in Settings.",
        );
        for command in [
            Command::Jump,
            Command::NewTab,
            Command::SplitRight,
            Command::FocusPanel,
        ] {
            let label = shortcuts::COMMANDS
                .iter()
                .find(|(each, ..)| *each == command)
                .map_or("", |(_, _, label, ..)| label);
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(t.size.slot))
                    .border_b(px(t.size.hairline))
                    .border_color(c.line)
                    .child(div().flex_1().text_color(c.text_2).child(label))
                    .child(ui::keycap(shortcuts::label(command))),
            );
        }
        page.child(
            div().pt(px(t.space.s16)).child(
                ui::button("open-settings", "Change in Settings", Button::Secondary)
                    .on_click(cx.listener(|_, _, _, cx| cx.defer(super::settings_window::show))),
            ),
        )
    }

    fn rail(&self) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let current = match self.stage {
            Stage::Step(index) => index,
            _ => STEPS.len(),
        };
        let mut rail = div()
            .w(px(t.size.settings_nav))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(t.space.s4))
            .px(px(t.space.s12))
            .pt(px(t.size.titlebar))
            .bg(c.panel)
            .border_r(px(t.size.hairline))
            .border_color(c.line);
        for (index, name) in STEPS.iter().enumerate() {
            let (fill, text) = if index < current {
                (c.ok, c.text_2)
            } else if index == current {
                (c.accent, c.text)
            } else {
                (c.line_strong, c.muted)
            };
            rail = rail.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.row))
                    .text_color(text)
                    .child(number(index, fill, index < current))
                    .child(*name),
            );
        }
        rail
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let root = ui::text(
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(c.surface)
                .text_color(c.text)
                .font_family(t.font.ui.clone()),
            &t.text.row,
        );
        let Stage::Step(index) = self.stage else {
            let body = root
                .child(div().flex_1().min_h_0().child(self.scene()))
                .child(self.scale_controls(cx));
            return super::window_controls::frame(body, "Welcome", window);
        };
        if index == 2
            && self.first.is_none()
            && let Some(probe) = &self.probe
            && let Ok(Some(directory)) =
                crate::roster::provider_directory(Some(true), None, None, false)
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
                    first::FirstEvent::Pending => {
                        view.first_ready = false;
                        view.first_agent = None;
                    }
                    first::FirstEvent::Ready(name) => {
                        view.first_ready = true;
                        view.first_agent = Some(name.clone());
                    }
                    first::FirstEvent::Existing => {
                        view.first_ready = true;
                        view.first_agent = None;
                    }
                    first::FirstEvent::OpenTerminal(name) => {
                        view.open_error = show_agent(name.clone(), cx).err();
                    }
                }
                cx.notify();
            }));
            self.first = Some(first);
        }
        let page = match index {
            0 => self.check_step(cx),
            1 => self.tools_step(cx),
            2 => match &self.first {
                Some(first) => div().child(first.clone()),
                None if self.probe.is_none() => heading("Your first agent", "Checking your tools and shell environment…"),
                None => heading("Your first agent", "The setup engine is unavailable. Return to Check, then install or reconnect the engine before creating an agent.")
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
        let last = index + 1 == STEPS.len();
        let can_continue = index != 2 || self.first_ready;
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
                    ui::button("back", "Back", Button::Secondary).on_click(cx.listener(
                        move |view, _, _, cx| {
                            view.stage = Stage::Step(index - 1);
                            view.copied = None;
                            cx.notify();
                        },
                    )),
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
            .child(div().flex_1())
            .when(
                index == 2
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
                            view.first_ready = true;
                            view.stage = Stage::Step(3);
                            cx.notify();
                        })),
                    )
                },
            )
            .child(
                ui::button(
                    "continue",
                    if last {
                        "Open Agent Interface"
                    } else {
                        "Continue"
                    },
                    Button::Primary,
                )
                .opacity(if can_continue { 1.0 } else { 0.45 })
                .on_click(cx.listener(move |view, _, window, cx| {
                    if !can_continue {
                        return;
                    }
                    if last {
                        view.finish(window, cx);
                    } else {
                        view.stage = Stage::Step(index + 1);
                        view.copied = None;
                        cx.notify();
                    }
                })),
            );
        let body = root
            .child(
                div().flex().flex_1().min_h_0().child(self.rail()).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("setup-page")
                                .flex_1()
                                .min_h_0()
                                .pt(px(t.size.titlebar))
                                .px(px(t.space.s24))
                                .overflow_y_scroll()
                                .child(page),
                        )
                        .child(footer),
                ),
            )
            .child(self.scale_controls(cx));
        super::window_controls::frame(body, "Welcome", window)
    }
}

/// Show the actual agent terminal for login/permissions or the introduction.
/// The saved library may still be loading when setup opens the workspace.
fn show_agent(name: String, cx: &mut App) -> Result<(), &'static str> {
    if super::browser_window(cx).is_none() {
        let source = crate::sessions::source_socket(None).map_err(|error| error.to_string());
        super::open_browser(None, source, cx)
            .map_err(|_| "The agent window could not open. Try again.")?;
    }
    let handle = super::browser_window(cx).ok_or("The agent window could not open. Try again.")?;
    cx.spawn(async move |cx| {
        for _ in 0..50 {
            let opened = handle
                .update(cx, |browser, window, cx| {
                    browser.open_setup_agent(&name, window, cx)
                })
                .unwrap_or(true);
            if opened {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
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

/// A step's number in a circle, or a tick once it is done.
fn number(index: usize, fill: Rgba, done: bool) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let base = div()
        .flex_shrink_0()
        .size(px(t.size.agent_symbol))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(fill);
    if done {
        base.child(ui::icon(
            "check",
            t.size.agent_symbol - t.space.s6,
            c.on_accent,
        ))
    } else {
        ui::text(base.text_color(c.on_accent), &t.text.status).child(format!("{}", index + 1))
    }
}

/// What the opening scene shows at one moment, each part from 0 (hidden) to 1.
#[derive(Debug, PartialEq)]
struct Frame {
    light: f32,
    panes: [f32; 3],
    cursor: bool,
    link: f32,
    name: f32,
    tagline: f32,
    steps: f32,
    lit: usize,
    leave: f32,
}

impl Frame {
    /// The scene at `ms` milliseconds: a light grows, a terminal splits into
    /// three panes, a line links them, the name and line appear, then the
    /// setup steps light one by one and everything dissolves.
    fn at(ms: f32) -> Self {
        let rise = |from: f32, length: f32| ((ms - from) / length).clamp(0.0, 1.0);
        Self {
            light: rise(0.0, 1000.0),
            panes: [rise(600.0, 400.0), rise(1600.0, 500.0), rise(2300.0, 500.0)],
            cursor: ((ms / 500.0) as u32).is_multiple_of(2),
            link: rise(3000.0, 600.0),
            name: rise(3300.0, 600.0),
            tagline: rise(4000.0, 500.0) * (1.0 - rise(5000.0, 400.0)),
            steps: rise(5200.0, 400.0),
            lit: (0..STEPS.len())
                .filter(|step| ms >= 5500.0 + *step as f32 * 300.0)
                .count(),
            leave: rise(7000.0, 1000.0),
        }
    }
}

fn frame(frame: Frame) -> gpui::Div {
    let t = tokens();
    let ground = rgba(0x0e0f13, 1.0);
    let text = rgba(0xeceef3, 1.0);
    let glow = rgba(0x7887ff, 1.0);
    let shown = 1.0 - frame.leave;
    let mut panes = div().flex().gap(px(6.0)).w(px(180.0)).h(px(72.0));
    for (index, rise) in frame.panes.into_iter().enumerate() {
        panes = panes.child(
            div()
                .relative()
                .h_full()
                // Each new pane grows out of the one before it.
                .flex_basis(px(0.0))
                .flex_grow(1.0)
                .when(index > 0, |el| el.max_w(px(180.0 * rise)))
                .rounded(px(6.0))
                .border(px(1.5))
                .border_color(rgba(0xeceef3, 0.8 * rise * shown))
                .when(index == 0 && frame.cursor, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(px(8.0))
                            .top(px(8.0))
                            .w(px(7.0))
                            .h(px(12.0))
                            .bg(rgba(0xeceef3, rise * shown)),
                    )
                }),
        );
    }
    let link = div()
        .relative()
        .w(px(180.0))
        .h(px(18.0))
        .child(
            div()
                .absolute()
                .top(px(9.0))
                .left(px(30.0))
                .right(px(30.0))
                .h(px(1.5))
                .bg(rgba(0x8a96ff, 0.9 * frame.link * shown)),
        )
        .children([24.0, 84.0, 144.0].map(|left| {
            div()
                .absolute()
                .top(px(6.0))
                .left(px(left))
                .size(px(7.0))
                .rounded_full()
                .bg(rgba(0x8a96ff, frame.link * shown))
        }));
    let mut steps = div().flex().gap(px(16.0)).justify_center();
    for (index, name) in STEPS.iter().enumerate() {
        let lit = if index < frame.lit { 1.0 } else { 0.3 };
        steps = steps.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_color(rgba(0xeceef3, lit * frame.steps * shown))
                .child(format!("{}  {name}", index + 1)),
        );
    }
    div()
        .size_full()
        .relative()
        .bg(ground)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(t.space.s16))
        .child(
            div()
                .absolute()
                .size(px(1.0))
                .rounded_full()
                .shadow(vec![BoxShadow {
                    color: rgba(0x7887ff, 0.35 * frame.light * shown).into(),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(160.0 * frame.light),
                    spread_radius: px(70.0 * frame.light),
                    inset: false,
                }])
                .bg(glow),
        )
        .child(panes)
        .child(link)
        .child(
            div()
                .text_size(px(24.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgba(0xeceef3, frame.name * shown))
                .child("Agent Interface"),
        )
        .child(
            div()
                .h(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .absolute()
                        .whitespace_nowrap()
                        .text_color(rgba(0xeceef3, frame.tagline * shown))
                        .child(TAGLINE),
                )
                .child(steps),
        )
        .text_color(text)
}

fn rgba(hex: u32, alpha: f32) -> Rgba {
    let mut color = gpui::rgb(hex);
    color.a = alpha.clamp(0.0, 1.0);
    color
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

    #[test]
    fn the_scene_builds_then_dissolves() {
        let start = Frame::at(0.0);
        assert_eq!((start.light, start.panes, start.lit), (0.0, [0.0; 3], 0));
        let middle = Frame::at(4500.0);
        assert_eq!(middle.panes, [1.0; 3]);
        assert_eq!((middle.name, middle.tagline, middle.steps), (1.0, 1.0, 0.0));
        let steps = Frame::at(6500.0);
        assert_eq!(
            (steps.tagline, steps.steps, steps.lit),
            (0.0, 1.0, STEPS.len())
        );
        assert_eq!(Frame::at(8000.0).leave, 1.0);
    }
}
