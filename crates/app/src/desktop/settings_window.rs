//! The native Settings window: appearance applies to every open window at
//! once and is saved in the settings file. Built from the design components.

use gpui::{
    App, Bounds, Context, Global, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};

use super::{
    Browser, components as ui,
    design::{self, colors, tokens},
    settings::{self, PaneHeaders, Settings},
    shortcuts,
};

#[cfg(target_os = "macos")]
const FIXED: &[(&str, &str, &str)] = &[
    ("Select tab 1–9", "⌘1–9", "Tabs"),
    ("Select workspace 1–9", "⌥⌘1–9", "Workspaces"),
    ("Copy selection", "⌘C", "Terminal"),
    ("Paste", "⌘V", "Terminal"),
    ("Scroll history", "⇧PgUp  ⇧PgDn", "Terminal"),
    ("Select locally in mouse programs", "⇧ drag", "Terminal"),
    ("Settings", "⌘,", "Application"),
    ("New window", "⌘N", "Application"),
    ("Open library", "⌘O", "Application"),
    ("Quit", "⌘Q", "Application"),
];

#[cfg(not(target_os = "macos"))]
const FIXED: &[(&str, &str, &str)] = &[
    ("Select tab 1–9", "Alt+1–9", "Tabs"),
    ("Select workspace 1–9", "Ctrl+Alt+1–9", "Workspaces"),
    ("Copy selection", "Ctrl+Shift+C", "Terminal"),
    ("Paste", "Ctrl+Shift+V", "Terminal"),
    ("Scroll history", "Shift+PgUp  Shift+PgDn", "Terminal"),
    ("Select locally in mouse programs", "Shift+drag", "Terminal"),
    ("Settings", "Ctrl+,", "Application"),
    ("New window", "Ctrl+Shift+N", "Application"),
    ("Open library", "Ctrl+Shift+O", "Application"),
    ("Quit", "Ctrl+Shift+Q", "Application"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Appearance,
    Shortcuts,
    Agents,
}

pub struct SettingsWindow {
    page: Page,
    error: Option<String>,
    focus: gpui::FocusHandle,
    /// The command waiting for its new keys, and why the last keys were refused.
    recording: Option<shortcuts::Command>,
    refusal: Option<String>,
    _keys: gpui::Subscription,
}

struct Open(WindowHandle<SettingsWindow>);
impl Global for Open {}

/// Bring the Settings window forward, opening it if it is not open.
pub fn show(cx: &mut App) {
    let open = cx.try_global::<Open>().map(|Open(handle)| *handle);
    if let Some(handle) = open
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let t = tokens();
    let options = WindowOptions {
        app_id: Some(super::APP_ID.into()),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(t.size.settings_width), px(t.size.settings_height)),
            cx,
        ))),
        window_min_size: Some(size(
            px(t.size.settings_width - t.size.panel_width / 2.0),
            px(t.size.settings_height - t.size.panel_width / 2.0),
        )),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(t.size.window_controls_x),
                px(t.size.window_controls_y),
            )),
        }),
        ..Default::default()
    };
    if let Ok(handle) = cx.open_window(options, |window, cx| {
        let id = window.window_handle().window_id();
        cx.new(|cx| {
            // Record before shortcut dispatch, so keys such as Command-W are
            // assigned instead of closing the window.
            let view = cx.entity().downgrade();
            let keys = cx.intercept_keystrokes(move |event, window, cx| {
                if window.window_handle().window_id() != id {
                    return;
                }
                let recorded = view
                    .update(cx, |view: &mut SettingsWindow, cx| {
                        view.record(&event.keystroke, cx)
                    })
                    .unwrap_or(false);
                if recorded {
                    cx.stop_propagation();
                }
            });
            SettingsWindow {
                page: Page::Appearance,
                error: None,
                focus: cx.focus_handle(),
                recording: None,
                refusal: None,
                _keys: keys,
            }
        })
    }) {
        cx.set_global(Open(handle));
    }
}

impl SettingsWindow {
    /// Apply new settings everywhere, then save them.
    fn change(&mut self, update: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let mut next = Settings::current();
        update(&mut next);
        next.apply();
        super::configure_menu(cx);
        // Apply at once; save on a worker and report the outcome when it ends.
        let (done, saved) = futures::channel::oneshot::channel();
        settings::save_in_background(next, move |result| {
            let _ = done.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = saved.await {
                let _ = this.update(cx, |view, cx| {
                    view.error = result.err().map(|error| format!("Not saved: {error}"));
                    cx.notify();
                });
            }
        })
        .detach();
        cx.refresh_windows();
    }

    /// While a shortcut is recording, the next key press becomes its chord.
    /// Returns whether the key press was taken.
    fn record(&mut self, key: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(command) = self.recording else {
            return false;
        };
        if key.key == "escape" && !key.modifiers.modified() {
            self.recording = None;
            self.refusal = None;
            cx.notify();
            return true;
        }
        let Some(chord) = shortcuts::Chord::from_keystroke(key) else {
            if key.modifiers.function {
                self.refusal = Some("Fn combinations are not supported".into());
                cx.notify();
            }
            return true;
        };
        let mut bindings = shortcuts::current();
        match bindings.assign(command, chord) {
            Ok(()) => {
                self.recording = None;
                self.refusal = None;
                self.change(|settings| settings.shortcuts = bindings, cx);
            }
            Err(reason) => {
                self.refusal = Some(reason);
                cx.notify();
            }
        }
        true
    }

    fn heading(label: &'static str) -> gpui::Div {
        let t = tokens();
        ui::text(
            div().pb(px(t.space.s16)).text_color(colors().text),
            &t.text.page_title,
        )
        .child(label)
    }

    fn label(label: impl Into<SharedString>) -> gpui::Div {
        let t = tokens();
        ui::text(
            div().pb(px(t.space.s8)).text_color(colors().muted),
            &t.text.label,
        )
        .child(label.into())
    }

    fn note(text: impl Into<SharedString>) -> gpui::Div {
        let t = tokens();
        ui::text(div().text_color(colors().muted), &t.text.status).child(text.into())
    }

    /// A read-only value in a field frame.
    fn value(text: impl Into<SharedString>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        ui::text(
            div()
                .flex_1()
                .min_w_0()
                .h(px(t.size.button))
                .px(px(t.space.s10))
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .rounded(px(t.radius.row))
                .bg(c.field)
                .border(px(t.size.hairline))
                .border_color(c.line)
                .font_family(t.font.mono.clone())
                .text_color(c.text_2),
            &t.text.status,
        )
        .child(text.into())
    }

    fn section(children: impl IntoIterator<Item = gpui::AnyElement>) -> gpui::Div {
        let t = tokens();
        div()
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .pb(px(t.space.s20))
            .children(children)
    }

    fn general(&self) -> gpui::Div {
        let t = tokens();
        let library = crate::library::library_directory(None)
            .map_or_else(|error| error.to_string(), |path| path.display().to_string());
        let file = settings::path().map_or_else(
            || "unavailable".to_owned(),
            |path| path.display().to_string(),
        );
        div()
            .flex()
            .flex_col()
            .child(Self::heading("General"))
            .child(Self::section([
                Self::label("DEFAULT LIBRARY").into_any_element(),
                div()
                    .flex()
                    .gap(px(t.space.s8))
                    .child(Self::value(library))
                    .child(
                        ui::button("open-library", "Open another", ui::Button::Secondary)
                            .child(ui::key_hint(
                                shortcuts::Chord::parse(shortcuts::platform_key("cmd-o"))
                                    .unwrap()
                                    .label(),
                            ))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(super::OpenLibrary), cx)
                            }),
                    )
                    .into_any_element(),
                Self::note(
                    "The folder that holds layouts.db. Each window can open a different one.",
                )
                .into_any_element(),
            ]))
            .child(Self::section([
                Self::label("SETTINGS FILE").into_any_element(),
                Self::value(file).into_any_element(),
            ]))
            .child(Self::section([
                Self::label("VERSION").into_any_element(),
                div()
                    .text_color(colors().text_2)
                    .child(concat!("Agent Interface ", env!("CARGO_PKG_VERSION")))
                    .into_any_element(),
            ]))
    }

    fn appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let current = Settings::current();
        let card = |id: &'static str,
                    name: &'static str,
                    colours: &'static design::Colors,
                    light: bool| {
            let chosen = current.light == light;
            let bar = |fill: gpui::Rgba, width: f32| {
                div()
                    .h(px(t.space.s4))
                    .w(gpui::relative(width))
                    .rounded(px(t.space.s2))
                    .bg(fill)
            };
            div()
                .id(id)
                .w(px(t.size.theme_card))
                .flex()
                .flex_col()
                .gap(px(t.space.s8))
                .p(px(t.space.s6))
                .rounded(px(t.radius.popover))
                .border(px(t.size.focus_ring))
                .border_color(if chosen { c.accent } else { c.line })
                .cursor_pointer()
                .child(
                    div()
                        .h(px(t.size.empty_icon * 1.85))
                        .flex()
                        .rounded(px(t.radius.row))
                        .overflow_hidden()
                        .bg(colours.ground)
                        .border(px(t.size.hairline))
                        .border_color(colours.edge)
                        .child(
                            div()
                                .w(px(t.size.empty_icon - t.space.s6))
                                .bg(colours.panel)
                                .flex()
                                .flex_col()
                                .gap(px(t.space.s4))
                                .px(px(t.space.s4))
                                .py(px(t.space.s8))
                                .child(bar(colours.accent, 0.7))
                                .child(bar(colours.line_strong, 1.0))
                                .child(bar(colours.line_strong, 0.8)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .gap(px(t.space.s2))
                                .p(px(t.space.s4))
                                .child(
                                    div()
                                        .flex_1()
                                        .rounded(px(t.space.s2))
                                        .bg(colours.surface)
                                        .border(px(t.size.hairline))
                                        .border_color(colours.focus),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(px(t.space.s2))
                                        .child(
                                            div()
                                                .flex_1()
                                                .rounded(px(t.space.s2))
                                                .bg(colours.surface),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .rounded(px(t.space.s2))
                                                .bg(colours.surface),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(t.space.s2))
                        .when(chosen, |el| el.font_weight(t.text.row_selected.weight()))
                        .child(name)
                        .when(chosen, |el| {
                            el.child(ui::icon("check", t.size.icon, c.accent))
                        }),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.change(|settings| settings.light = light, cx)
                }))
        };
        let size = current.font_size as u32;
        let step = |id: &'static str, glyph: &'static str, next: u32, allowed: bool| {
            div()
                .id(id)
                .w(px(t.size.button))
                .h(px(t.size.button - t.space.s2))
                .flex()
                .items_center()
                .justify_center()
                .text_color(if allowed { c.text_2 } else { c.faint })
                .when(allowed, |el| {
                    el.cursor_pointer()
                        .hover(|el| el.bg(colors().hover))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.change(|settings| settings.font_size = next as f32, cx)
                        }))
                })
                .child(glyph)
        };
        let (min, max) = (9, 24);
        let mut modes = div()
            .flex()
            .gap(px(t.space.s2))
            .p(px(t.space.s2))
            .rounded(px(t.radius.slot))
            .bg(c.panel)
            .border(px(t.size.hairline))
            .border_color(c.line);
        for (mode, _, label) in PaneHeaders::ALL {
            let chosen = current.pane_headers == mode;
            modes = modes.child(
                ui::text(
                    div()
                        .id(SharedString::from(format!("pane-headers-{label}")))
                        .h(px(t.size.button - t.space.s4))
                        .px(px(t.space.s12))
                        .flex()
                        .items_center()
                        .rounded(px(t.radius.icon_button))
                        .cursor_pointer()
                        .text_color(if chosen { c.text } else { c.muted })
                        .when(chosen, |el| {
                            el.bg(c.field)
                                .border(px(t.size.hairline))
                                .border_color(c.line_strong)
                        }),
                    &t.text.body,
                )
                .child(label)
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.change(|settings| settings.pane_headers = mode, cx)
                })),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(Self::heading("Appearance"))
            .child(Self::section([
                Self::label("THEME").into_any_element(),
                div()
                    .flex()
                    .gap(px(t.space.s12))
                    .child(card("theme-dark", "Dark", &design::tokens().dark, false))
                    .child(card("theme-light", "Light", &design::tokens().light, true))
                    .into_any_element(),
            ]))
            .child(Self::section([
                Self::label("TERMINAL TEXT SIZE").into_any_element(),
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s14))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .rounded(px(t.radius.row))
                            .border(px(t.size.hairline))
                            .border_color(c.line_strong)
                            .bg(c.field)
                            .child(step("smaller", "−", size.saturating_sub(1), size > min))
                            .child(
                                div()
                                    .w(px(t.size.button + t.space.s10))
                                    .flex()
                                    .justify_center()
                                    .child(size.to_string()),
                            )
                            .child(step("larger", "+", size + 1, size < max)),
                    )
                    .child(Self::note(
                        "Terminals resize their sessions to fit. 9 to 24 pt.",
                    ))
                    .into_any_element(),
            ]))
            .child(Self::section([
                Self::label("PANE HEADERS").into_any_element(),
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s14))
                    .child(modes)
                    .child(Self::note(format!(
                        "Off gives each pane its {} px back.",
                        t.size.pane_header
                    )))
                    .into_any_element(),
            ]))
    }

    fn shortcuts(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let bindings = shortcuts::current();
        let defaults = shortcuts::Bindings::default();
        let mut page = div()
            .flex()
            .flex_col()
            .child(Self::heading("Shortcuts"))
            .child(
                Self::note("Click Change, then press the new shortcut. Changes save automatically; Escape cancels.")
                    .pb(px(t.space.s12)),
            );
        let keycap = |label: String, recording: bool| {
            ui::text(
                div()
                    .min_w(px(t.size.icon_button * 2.0 - t.space.s4))
                    .h(px(t.size.button - t.space.s8))
                    .px(px(t.space.s6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(t.radius.key + 1.0))
                    .border(px(t.size.hairline))
                    .font_family(t.font.mono.clone())
                    .when(recording, |el| {
                        el.bg(c.accent)
                            .border_color(c.accent)
                            .text_color(c.on_accent)
                    })
                    .when(!recording, |el| {
                        el.bg(c.field)
                            .border_color(c.line_strong)
                            .text_color(c.text_2)
                    }),
                &t.text.key,
            )
            .child(label)
        };
        for (section, description) in [
            ("Workspaces", "A workspace is a collection of tabs."),
            ("Tabs", "A tab contains one or more panes."),
            (
                "Panes",
                "A pane holds a terminal session or a native view, such as Skills. Close removes the pane; Detach keeps it and returns to the chooser.",
            ),
            (
                "Navigation",
                "Find work and move between the sidebar and content.",
            ),
            ("Terminal", "Standard terminal controls."),
            ("Application", "Window and application controls."),
        ] {
            page = page
                .child(Self::label(section.to_uppercase()).pt(px(t.space.s8)))
                .child(Self::note(description).pb(px(t.space.s8)));
            for (command, id, label, _, _) in shortcuts::COMMANDS
                .iter()
                .filter(|(_, _, _, group, _)| *group == section)
            {
                let command = *command;
                let recording = self.recording == Some(command);
                let changed = bindings.chord(command) != defaults.chord(command);
                let row = div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.slot))
                    .border_b(px(t.size.hairline))
                    .border_color(c.line)
                    .child(div().flex_1().text_color(c.text_2).child(*label))
                    .when(changed && !recording, |el| {
                        el.child(
                            ui::text(
                                div()
                                    .id(SharedString::from(format!("reset-{id}")))
                                    .cursor_pointer()
                                    .text_color(c.accent_text),
                                &t.text.status,
                            )
                            .child("Reset")
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    let mut bindings = shortcuts::current();
                                    let chord =
                                        shortcuts::Bindings::default().chord(command).clone();
                                    match bindings.assign(command, chord) {
                                        Ok(()) => {
                                            view.recording = None;
                                            view.refusal = None;
                                            view.change(
                                                |settings| settings.shortcuts = bindings,
                                                cx,
                                            );
                                        }
                                        Err(reason) => {
                                            view.recording = Some(command);
                                            view.refusal = Some(reason);
                                            cx.notify();
                                        }
                                    }
                                },
                            )),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("shortcut-{id}")))
                            .flex()
                            .items_center()
                            .gap(px(t.space.s8))
                            .cursor_pointer()
                            .child(
                                ui::text(div().text_color(c.accent_text), &t.text.status)
                                    .child(if recording { "Cancel" } else { "Change" }),
                            )
                            .child(keycap(
                                if recording {
                                    "Press keys…".into()
                                } else {
                                    bindings.chord(command).label()
                                },
                                recording,
                            ))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.recording =
                                    (view.recording != Some(command)).then_some(command);
                                view.refusal = None;
                                window.focus(&view.focus, cx);
                                cx.notify();
                            })),
                    );
                page = page.child(div().child(row).when(recording, |el| {
                    el.when_some(self.refusal.clone(), |el, refusal| {
                        el.child(
                            Self::note(refusal)
                                .text_color(c.attention_text)
                                .py(px(t.space.s8)),
                        )
                    })
                }));
            }
            for (action, key, _) in FIXED.iter().filter(|(_, _, group)| *group == section) {
                page = page.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s10))
                        .h(px(t.size.slot))
                        .border_b(px(t.size.hairline))
                        .border_color(c.line)
                        .child(div().flex_1().text_color(c.muted).child(*action))
                        .child(Self::note("Fixed"))
                        .child(keycap((*key).into(), false)),
                );
            }
        }
        page = page.child(
            div().pt(px(t.space.s12)).pb(px(t.space.s20)).child(
                ui::button(
                    "restore-shortcuts",
                    "Restore default shortcuts",
                    ui::Button::Secondary,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.recording = None;
                    view.refusal = None;
                    view.change(
                        |settings| settings.shortcuts = shortcuts::Bindings::default(),
                        cx,
                    );
                })),
            ),
        );
        page
    }

    fn agents(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let (connected, detail) = match crate::roster::provider_directory(None, None, None, false) {
            Ok(Some(path)) => (true, path.display().to_string()),
            Ok(None) => (
                false,
                "Agent Backbone is not installed; the panel has no agents section.".into(),
            ),
            Err(error) => (false, format!("Unavailable: {error}")),
        };
        // The preference lives with each window's library; show the first's.
        let shown = cx
            .windows()
            .into_iter()
            .filter_map(|window| window.downcast::<Browser>())
            .find_map(|window| {
                window.read(cx).ok().and_then(|browser| {
                    browser
                        .layout
                        .as_ref()
                        .map(|layout| layout.model().show_agents)
                })
            })
            .unwrap_or(true);
        let legend = |state: ui::AgentState| {
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .child(ui::agent_symbol(state))
                .child(state.word())
        };
        div()
            .flex()
            .flex_col()
            .child(Self::heading("Agents"))
            .child(Self::section([
                Self::label("AGENT BACKBONE").into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s6))
                    .px(px(t.space.s14))
                    .py(px(t.space.s12))
                    .rounded(px(t.radius.pane))
                    .bg(c.panel)
                    .border(px(t.size.hairline))
                    .border_color(c.line)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s8))
                            .font_weight(t.text.row_selected.weight())
                            .child(ui::dot(if connected { c.ok } else { c.faint }))
                            .child(if connected { "Connected" } else { "Not connected" }),
                    )
                    .child(ui::text(div().text_color(c.muted), &t.text.meta).child(detail))
                    .into_any_element(),
                Self::note("States refresh every two seconds. The app only reads from Agent Backbone.").into_any_element(),
            ]))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s14))
                    .pb(px(t.space.s20))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s2))
                            .child("Show agents in the panel")
                            .child(Self::note("Same as \"agents shown\" in the status row. Saved with the library.")),
                    )
                    .child(
                        div()
                            .id("show-agents")
                            .w(px(t.size.slot + t.space.s4))
                            .h(px(t.space.s20))
                            .p(px(t.space.s2))
                            .rounded_full()
                            .flex()
                            .when(shown, |el| el.justify_end().bg(c.accent))
                            .when(!shown, |el| el.bg(c.line_strong))
                            .cursor_pointer()
                            .child(div().size(px(t.space.s16)).rounded_full().bg(gpui::white()))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                // One requested value for every window, so
                                // their libraries agree with the switch.
                                for window in cx.windows().into_iter().filter_map(|w| w.downcast::<Browser>()) {
                                    let _ = window.update(cx, |browser, _, cx| browser.set_show_agents(!shown, cx));
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(Self::section([
                Self::label("STATES").into_any_element(),
                div()
                    .flex()
                    .gap(px(t.space.s20))
                    .child(legend(ui::AgentState::NeedsYou))
                    .child(legend(ui::AgentState::Working))
                    .child(legend(ui::AgentState::Idle))
                    .child(legend(ui::AgentState::Unknown))
                    .into_any_element(),
            ]))
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let pages = [
            (Page::General, "General", "settings"),
            (Page::Appearance, "Appearance", "palette"),
            (Page::Shortcuts, "Shortcuts", "keyboard"),
            (Page::Agents, "Agents", "bot"),
        ];
        let mut nav = div()
            .w(px(t.size.settings_nav))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(t.size.hairline))
            .px(px(t.space.s8))
            .bg(c.panel)
            .border_r(px(t.size.hairline))
            .border_color(c.line)
            .child(
                div()
                    .id("settings-titlebar")
                    .h(px(t.size.titlebar))
                    .flex_shrink_0()
                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                        window.start_window_move()
                    }),
            );
        for (page, name, icon) in pages {
            let chosen = self.page == page;
            nav = nav.child(
                ui::row(SharedString::from(name), chosen)
                    .text_color(if chosen { c.text } else { c.text_2 })
                    .when(chosen, |el| el.font_weight(t.text.row_selected.weight()))
                    .child(ui::icon(
                        icon,
                        t.size.icon,
                        if chosen { c.accent } else { c.muted },
                    ))
                    .child(name)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.page = page;
                        view.recording = None;
                        view.refusal = None;
                        cx.notify();
                    })),
            );
        }
        let content = match self.page {
            Page::General => self.general(),
            Page::Appearance => self.appearance(cx),
            Page::Shortcuts => self.shortcuts(cx),
            Page::Agents => self.agents(cx),
        };
        ui::text(
            div()
                .size_full()
                .flex()
                .track_focus(&self.focus)
                .bg(c.surface)
                .text_color(c.text)
                .font_family(t.font.ui.clone()),
            &t.text.row,
        )
        .child(nav)
        .child(
            div()
                .id("settings-page")
                .flex_1()
                .min_w_0()
                .h_full()
                .pt(px(t.size.titlebar - t.space.s24))
                .px(px(t.space.s24))
                .overflow_y_scroll()
                .child(content)
                .when_some(self.error.clone(), |el, error| {
                    el.child(
                        ui::text(
                            div().pb(px(t.space.s12)).text_color(c.attention_text),
                            &t.text.status,
                        )
                        .child(error),
                    )
                }),
        )
    }
}
