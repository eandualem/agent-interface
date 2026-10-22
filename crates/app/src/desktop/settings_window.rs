//! The native Settings window: appearance applies to every open window at
//! once and is saved in the settings file.

use gpui::{
    App, Bounds, Context, FontWeight, Global, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, prelude::*, px, rgb, size,
};

use super::{
    panel,
    settings::{self, Settings},
    shortcuts, theme,
};

/// Keys that stay fixed, shown under the configurable ones.
const FIXED: &[(&str, &str)] = &[
    ("Select tab 1–9", "⌘1–9"),
    ("Select workspace 1–9", "⌥⌘1–9"),
    ("Copy selection", "⌘C"),
    ("Paste", "⌘V"),
    ("Scroll history", "⇧PgUp  ⇧PgDn"),
    ("Select locally in mouse programs", "⇧ drag"),
    ("Settings", "⌘,"),
    ("New window", "⌘N"),
    ("Open library", "⌘O"),
    ("Close window", "⌘W"),
    ("Quit", "⌘Q"),
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
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(680.0), px(480.0)),
            cx,
        ))),
        window_min_size: Some(size(px(560.0), px(380.0))),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Ok(handle) = cx.open_window(options, |window, cx| {
        let id = window.window_handle().window_id();
        cx.new(|cx| {
            // Record before shortcut dispatch, so keys such as Command-W are
            // refused instead of closing the window.
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
        self.error = match settings::path() {
            Some(path) => settings::save(&next, &path)
                .err()
                .map(|error| format!("Not saved: {error}")),
            None => Some("Not saved: no home directory".into()),
        };
        cx.refresh_windows();
    }

    fn heading(text: &'static str) -> gpui::Div {
        div()
            .pt(px(4.0))
            .pb(px(10.0))
            .text_size(px(18.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child(text)
    }

    fn label(text: impl Into<SharedString>) -> gpui::Div {
        div()
            .pt(px(14.0))
            .pb(px(6.0))
            .text_size(px(11.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(theme::muted()))
            .child(text.into())
    }

    fn appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        let current = Settings::current();
        let card = |id: &'static str, name: &'static str, palette: &theme::Palette, light: bool| {
            let chosen = current.light == light;
            div()
                .id(id)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(8.0))
                .w(px(180.0))
                .rounded(px(10.0))
                .border_2()
                .border_color(rgb(if chosen {
                    theme::accent()
                } else {
                    theme::outline()
                }))
                .cursor_pointer()
                .child(
                    div()
                        .h(px(84.0))
                        .flex()
                        .rounded(px(6.0))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(palette.outline))
                        .child(
                            div()
                                .w(px(52.0))
                                .bg(rgb(palette.panel))
                                .p(px(6.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(div().h(px(6.0)).rounded(px(3.0)).bg(rgb(palette.accent)))
                                .child(div().h(px(6.0)).rounded(px(3.0)).bg(rgb(palette.active)))
                                .child(div().h(px(6.0)).rounded(px(3.0)).bg(rgb(palette.outline))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .bg(rgb(palette.surface))
                                .p(px(6.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(div().w(px(70.0)).h(px(4.0)).bg(rgb(palette.terminal_text)))
                                .child(div().w(px(46.0)).h(px(4.0)).bg(rgb(palette.terminal[2])))
                                .child(div().w(px(58.0)).h(px(4.0)).bg(rgb(palette.terminal[4]))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .when(chosen, |el| el.font_weight(FontWeight::SEMIBOLD))
                        .child(name),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.change(|settings| settings.light = light, cx)
                }))
        };
        let size = current.font_size as u32;
        let stepper = |id: &'static str, glyph: &'static str, next: u32| {
            div()
                .id(id)
                .size(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(theme::outline()))
                .when(settings::FONT_SIZES.contains(&next), |el| {
                    el.cursor_pointer()
                        .hover(|el| el.bg(rgb(theme::active())))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.change(|settings| settings.font_size = next as f32, cx)
                        }))
                })
                .when(!settings::FONT_SIZES.contains(&next), |el| {
                    el.text_color(rgb(theme::muted()))
                })
                .child(glyph)
        };
        div()
            .flex()
            .flex_col()
            .child(Self::heading("Appearance"))
            .child(Self::label("THEME"))
            .child(
                div()
                    .flex()
                    .gap(px(14.0))
                    .child(card("theme-dark", "Dark", &theme::DARK, false))
                    .child(card("theme-light", "Light", &theme::LIGHT, true)),
            )
            .child(Self::label("TERMINAL TEXT SIZE"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(stepper("smaller", "−", size.saturating_sub(1)))
                    .child(
                        div()
                            .w(px(48.0))
                            .flex()
                            .justify_center()
                            .child(format!("{size} pt")),
                    )
                    .child(stepper("larger", "+", size + 1))
                    .child(
                        div()
                            .text_color(rgb(theme::muted()))
                            .child("Terminals resize their sessions to fit."),
                    ),
            )
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

    fn shortcuts(&self, cx: &mut Context<Self>) -> gpui::Div {
        let bindings = shortcuts::current();
        let defaults = shortcuts::Bindings::default();
        let badge = |text: String, strong: bool| {
            div()
                .px(px(8.0))
                .h(px(22.0))
                .flex()
                .items_center()
                .rounded(px(5.0))
                .bg(rgb(if strong {
                    theme::accent()
                } else {
                    theme::active()
                }))
                .text_color(rgb(if strong {
                    theme::panel()
                } else {
                    theme::active_text()
                }))
                .child(text)
        };
        let mut page = div()
            .flex()
            .flex_col()
            .child(Self::heading("Shortcuts"))
            .child(
                div()
                    .text_color(rgb(theme::muted()))
                    .child("Click a shortcut, then press the new keys. Escape cancels."),
            )
            .when_some(self.refusal.clone(), |el, refusal| {
                el.child(
                    div()
                        .pt(px(8.0))
                        .text_color(rgb(theme::danger()))
                        .child(refusal),
                )
            });
        let mut group = "";
        for (command, id, label, section, _) in shortcuts::COMMANDS {
            if *section != group {
                group = section;
                page = page.child(Self::label(section.to_uppercase()));
            }
            let command = *command;
            let recording = self.recording == Some(command);
            let changed = bindings.chord(command) != defaults.chord(command);
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(30.0))
                    .border_b_1()
                    .border_color(rgb(theme::outline()))
                    .child(div().flex_1().child(*label))
                    .when(changed && !recording, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("reset-{id}")))
                                .cursor_pointer()
                                .text_color(rgb(theme::muted()))
                                .hover(|el| el.text_color(rgb(theme::normal())))
                                .child("Reset")
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    let mut bindings = shortcuts::current();
                                    let chord =
                                        shortcuts::Bindings::default().chord(command).clone();
                                    match bindings.assign(command, chord) {
                                        Ok(()) => {
                                            view.refusal = None;
                                            view.change(
                                                |settings| settings.shortcuts = bindings,
                                                cx,
                                            );
                                        }
                                        Err(reason) => {
                                            view.refusal = Some(reason);
                                            cx.notify();
                                        }
                                    }
                                })),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("shortcut-{id}")))
                            .cursor_pointer()
                            .child(badge(
                                if recording {
                                    "Press keys…".into()
                                } else {
                                    bindings.chord(command).label()
                                },
                                recording,
                            ))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.recording = Some(command);
                                view.refusal = None;
                                window.focus(&view.focus, cx);
                                cx.notify();
                            })),
                    ),
            );
        }
        page = page.child(
            div().pt(px(12.0)).child(
                div()
                    .id("restore-shortcuts")
                    .cursor_pointer()
                    .text_color(rgb(theme::accent()))
                    .child("Restore default shortcuts")
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
        page = page.child(Self::label("FIXED"));
        for (action, key) in FIXED {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(30.0))
                    .border_b_1()
                    .border_color(rgb(theme::outline()))
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(theme::muted()))
                            .child(*action),
                    )
                    .child(badge((*key).into(), false)),
            );
        }
        page
    }

    fn general(&self) -> gpui::Div {
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
            .child(Self::label("DEFAULT LIBRARY"))
            .child(library)
            .child(
                div()
                    .text_color(rgb(theme::muted()))
                    .child("Open another library from the menu bar with ⌘O."),
            )
            .child(Self::label("SETTINGS FILE"))
            .child(file)
            .child(Self::label("VERSION"))
            .child(concat!("tmux-workspaces ", env!("CARGO_PKG_VERSION")))
    }

    fn agents(&self) -> gpui::Div {
        let state = match crate::roster::provider_directory(None, None, None, false) {
            Ok(Some(path)) => format!("Connected through {}", path.display()),
            Ok(None) => "Agent Backbone is not installed; the panel has no agents section.".into(),
            Err(error) => format!("Unavailable: {error}"),
        };
        div()
            .flex()
            .flex_col()
            .child(Self::heading("Agents"))
            .child(Self::label("AGENT BACKBONE"))
            .child(state)
            .child(div().pt(px(8.0)).text_color(rgb(theme::muted())).child(
                "States refresh every two seconds. Show or hide the section with \
                         \"agents shown\" in a window's status row.",
            ))
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = [
            (Page::General, "General", "settings"),
            (Page::Appearance, "Appearance", "palette"),
            (Page::Shortcuts, "Shortcuts", "terminal"),
            (Page::Agents, "Agents", "bot"),
        ];
        let mut nav = div()
            .w(px(180.0))
            .flex_shrink_0()
            .h_full()
            .pt(px(10.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .bg(rgb(theme::panel()));
        for (page, name, icon) in pages {
            let chosen = self.page == page;
            nav = nav.child(
                panel::row(SharedString::from(name), chosen)
                    .child(panel::icon(
                        icon,
                        14.0,
                        if chosen {
                            theme::accent()
                        } else {
                            theme::muted()
                        },
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
            Page::Agents => self.agents(),
        };
        div()
            .size_full()
            .flex()
            .track_focus(&self.focus)
            .bg(rgb(theme::surface()))
            .text_color(rgb(theme::normal()))
            .font_family(panel::FONT)
            .text_size(px(13.0))
            .child(nav)
            .child(
                div()
                    .id("settings-page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .p(px(20.0))
                    .overflow_y_scroll()
                    .child(content)
                    .when_some(self.error.clone(), |el, error| {
                        el.child(
                            div()
                                .pt(px(12.0))
                                .text_color(rgb(theme::danger()))
                                .child(error),
                        )
                    }),
            )
    }
}
