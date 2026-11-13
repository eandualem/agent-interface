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
    theme_editor::{EditorEvent, ThemeEditor},
    themes,
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
    Profiles,
}

pub struct SettingsWindow {
    page: Page,
    /// Built when Profiles is first chosen, so other pages never ask Agent Backbone.
    profiles: Option<gpui::Entity<super::settings_profiles::Profiles>>,
    error: Option<String>,
    focus: gpui::FocusHandle,
    /// The command waiting for its new keys, and why the last keys were refused.
    recording: Option<shortcuts::Command>,
    refusal: Option<String>,
    /// Every theme, with its colours for a preview, or none when it cannot be read.
    themes: Vec<(themes::Entry, Option<design::Colors>)>,
    /// The colour theme chosen in VS Code on this machine, if it is found.
    vscode: Option<themes::vscode::Chosen>,
    /// What the last theme action did, or why it failed.
    theme_note: Option<String>,
    /// The theme file open in the editor.
    editing: Option<Editing>,
    _keys: gpui::Subscription,
}

/// A theme of the person's own, open in the JSON editor.
struct Editing {
    id: String,
    editor: gpui::Entity<ThemeEditor>,
    /// The text as last saved, for Revert and the unsaved mark.
    saved: String,
    /// Why the text is not a theme, if it is not.
    problem: Option<themes::Error>,
    /// Back was pressed with unsaved changes; pressing it again discards them.
    discarding: bool,
    _events: gpui::Subscription,
}

/// Every theme, with the colours its card previews.
fn theme_list() -> Vec<(themes::Entry, Option<design::Colors>)> {
    themes::list()
        .into_iter()
        .map(|entry| {
            let colours = themes::resolve(&entry.id).ok().map(|design| design.colors);
            (entry, colours)
        })
        .collect()
}

struct Open(WindowHandle<SettingsWindow>);
impl Global for Open {}

/// Tell an open Settings › Profiles page that a profile's sign-in pane closed.
pub fn sign_in_ended(profile_id: &str, runtime: &str, cx: &mut App) {
    let Some(handle) = cx.try_global::<Open>().map(|Open(handle)| *handle) else {
        return;
    };
    let _ = handle.update(cx, |view, _, cx| {
        if let Some(profiles) = &view.profiles {
            profiles.update(cx, |page, cx| page.sign_in_ended(profile_id, runtime, cx));
        }
    });
}

/// Whether `window` may close now: always, unless it is Settings holding
/// unsaved theme edits, which asks on the first request.
pub fn may_close_window(window: gpui::AnyWindowHandle, cx: &mut App) -> bool {
    let Some(handle) = cx.try_global::<Open>().map(|Open(handle)| *handle) else {
        return true;
    };
    if handle.window_id() != window.window_id() {
        return true;
    }
    handle
        .update(cx, |view, _, cx| view.may_close(cx))
        .unwrap_or(true)
}

/// Whether the app may quit now. With unsaved theme edits, the first
/// request brings Settings forward and asks; a second quits.
pub fn may_quit(cx: &mut App) -> bool {
    let Some(handle) = cx.try_global::<Open>().map(|Open(handle)| *handle) else {
        return true;
    };
    handle
        .update(cx, |view, window, cx| {
            let may = view.may_close(cx);
            if !may {
                window.activate_window();
            }
            may
        })
        .unwrap_or(true)
}

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
        window_decorations: cfg!(target_os = "linux").then_some(gpui::WindowDecorations::Client),
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
            let closing = cx.entity().downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                closing
                    .update(cx, |view: &mut SettingsWindow, cx| view.may_close(cx))
                    .unwrap_or(true)
            });
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
                profiles: None,
                error: None,
                focus: cx.focus_handle(),
                recording: None,
                refusal: None,
                themes: theme_list(),
                vscode: themes::vscode::chosen(),
                theme_note: None,
                editing: None,
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
        let resized = next.interface_size != Settings::current().interface_size;
        let controls = |size: &design::Size| (size.window_controls_x, size.window_controls_y);
        let before = controls(&tokens().size);
        next.apply();
        // A theme may move the native window buttons, as a new scale does.
        if resized || controls(&tokens().size) != before {
            // Each window takes the new scale and lays out again; deferred so
            // this window is no longer borrowed.
            cx.defer(|cx| {
                #[cfg(target_os = "macos")]
                let controls = gpui::point(
                    px(tokens().size.window_controls_x),
                    px(tokens().size.window_controls_y),
                );
                for window in cx.windows() {
                    let _ = window.update(cx, |_, window, cx| {
                        // Native window buttons keep their place in the panel's top strip.
                        #[cfg(target_os = "macos")]
                        window.set_traffic_light_position(controls);
                        window.bounds_changed(cx)
                    });
                }
            });
        }
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

    /// Whether the window may close now. Unsaved theme edits are only
    /// dropped on a second request: the first shows the editor and asks.
    fn may_close(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(editing) = &mut self.editing else {
            return true;
        };
        if editing.discarding || editing.editor.read(cx).value() == editing.saved {
            return true;
        }
        editing.discarding = true;
        self.page = Page::Appearance;
        cx.notify();
        false
    }

    /// Open the person's theme `id` in the JSON editor.
    fn edit_theme(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = match themes::source(id) {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(error) => {
                self.theme_note = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let editor = cx.new(|cx| ThemeEditor::new(&text, cx));
        let events = cx.subscribe(&editor, |view, editor, event, cx| match event {
            EditorEvent::Changed => {
                if let Some(editing) = &mut view.editing {
                    let text = editor.read(cx).value().to_owned();
                    editing.problem = themes::resolve_text(&editing.id, &text).err();
                    editing.discarding = false;
                    cx.notify();
                }
            }
            EditorEvent::Save => view.save_theme(cx),
        });
        let focus = editor.read(cx).focus.clone();
        window.focus(&focus, cx);
        self.editing = Some(Editing {
            id: id.to_owned(),
            problem: themes::resolve_text(id, &text).err(),
            editor,
            saved: text,
            discarding: false,
            _events: events,
        });
        self.theme_note = None;
        cx.notify();
    }

    /// Save the open theme and use it, unless it is not a theme yet.
    fn save_theme(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = &mut self.editing else {
            return;
        };
        let text = editing.editor.read(cx).value().to_owned();
        if let Err(error) = themes::resolve_text(&editing.id, &text) {
            editing.editor.update(cx, |editor, cx| {
                editor.show_mistake(error.line, error.column, cx)
            });
            editing.problem = Some(error);
            cx.notify();
            return;
        }
        if let Err(error) = themes::save(&editing.id, &text) {
            editing.problem = Some(themes::Error::new(format!("Not saved: {error}")));
            cx.notify();
            return;
        }
        editing.saved = text;
        editing.problem = None;
        let id = editing.id.clone();
        self.themes = theme_list();
        self.change(|settings| settings.theme = id, cx);
    }

    /// Copy the theme in use as one of the person's own and open it.
    fn customize_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match themes::customize(&themes::current()) {
            Ok(id) => {
                self.themes = theme_list();
                let chosen = id.clone();
                self.change(|settings| settings.theme = chosen, cx);
                self.edit_theme(&id, window, cx);
            }
            Err(error) => {
                self.theme_note = Some(error.to_string());
                cx.notify();
            }
        }
    }

    /// Import the VS Code colour theme at `path` as one of the person's own
    /// themes and use it.
    fn import_theme(
        &mut self,
        path: &std::path::Path,
        light: Option<bool>,
        name: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let imported = themes::vscode::import_file(path, light, name).and_then(|(name, json)| {
            let id = themes::new_id(&name);
            themes::save(&id, &json)
                .map(|()| id)
                .map_err(|error| themes::Error::new(format!("Not saved: {error}")))
        });
        match imported {
            Ok(id) => {
                self.themes = theme_list();
                self.theme_note = None;
                self.change(|settings| settings.theme = id, cx);
            }
            Err(error) => {
                self.theme_note = Some(format!("Not imported. {error}"));
                cx.notify();
            }
        }
    }

    /// Ask for a VS Code colour theme file and import it.
    fn import_theme_file(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |view, cx| view.import_theme(&path, None, None, cx));
        })
        .detach();
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

    fn general(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = tokens();
        let library = crate::library::library_directory(None)
            .map_or_else(|error| error.to_string(), |path| path.display().to_string());
        let file = settings::path().map_or_else(
            || "unavailable".to_owned(),
            |path| path.display().to_string(),
        );
        let current = Settings::current().logging;
        let mut levels = div().flex().gap(px(t.space.s8));
        for (level, name, label) in crate::diagnostics::Level::ALL {
            levels = levels.child(
                ui::button(
                    SharedString::from(format!("logging-{name}")),
                    label,
                    if current == level {
                        ui::Button::Primary
                    } else {
                        ui::Button::Secondary
                    },
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.change(|settings| settings.logging = level, cx);
                })),
            );
        }
        let logs = crate::diagnostics::directory()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unavailable".into());
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
            .child(Self::section([
                Self::label("DIAGNOSTIC LOGGING").into_any_element(),
                levels.into_any_element(),
                Self::note("Off disables diagnostic records. Warnings is the quiet default. Debug records lifecycle events; Trace adds operation timings. Changes apply now and on future launches.").into_any_element(),
                Self::value(logs).into_any_element(),
                Self::note("Local logs use at most 16 MiB. Detailed logs can reveal identifiers and error context; review before sharing. Nothing is uploaded. Off keeps existing files and functional app data.").into_any_element(),
            ]))
            .when_some(crate::diagnostics::problem(), |el, error| {
                el.child(Self::note(error).text_color(colors().attention_text))
            })
    }

    /// The open theme file in the JSON editor, with Save, Revert and Back.
    fn theme_editor(&self, editing: &Editing, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let unsaved = editing.editor.read(cx).value() != editing.saved;
        let name = self
            .themes
            .iter()
            .find(|(entry, _)| entry.id == editing.id)
            .map_or(editing.id.clone(), |(entry, _)| entry.name.clone());
        let file = themes::file(&editing.id)
            .map(|path| {
                let path = path.display().to_string();
                match std::env::var("HOME") {
                    Ok(home) if !home.is_empty() && path.starts_with(&home) => {
                        format!("~{}", &path[home.len()..])
                    }
                    _ => path,
                }
            })
            .unwrap_or_default();
        let save_key = if cfg!(target_os = "macos") {
            "⌘S"
        } else {
            "Ctrl+S"
        };
        let status = match &editing.problem {
            // The warning comes first: another request drops the edits.
            _ if editing.discarding => ui::text(
                div().text_color(c.attention_text),
                &t.text.status,
            )
            .child(format!(
                "This theme has unsaved changes. {save_key} saves them; Discard changes, or closing or quitting again, drops them.{}",
                editing
                    .problem
                    .as_ref()
                    .map_or(String::new(), |problem| format!(" {problem}"))
            )),
            Some(problem) => ui::text(div().text_color(c.attention_text), &t.text.status)
                .child(problem.to_string()),
            None => Self::note(if unsaved {
                format!("Unsaved changes. {save_key} saves and applies the theme.")
            } else {
                "Saved and in use. Values you leave out come from the theme it extends; docs/DESIGN.md lists every token.".into()
            }),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(t.space.s12))
            .pb(px(t.space.s16))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        ui::button(
                            "theme-back",
                            if editing.discarding {
                                "Discard changes"
                            } else {
                                "Back"
                            },
                            if editing.discarding {
                                ui::Button::Danger
                            } else {
                                ui::Button::Secondary
                            },
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            // Unsaved changes are only dropped on a second press.
                            let unsaved = view.editing.as_ref().is_some_and(|editing| {
                                editing.editor.read(cx).value() != editing.saved
                            });
                            match &mut view.editing {
                                Some(editing) if unsaved && !editing.discarding => {
                                    editing.discarding = true
                                }
                                _ => view.editing = None,
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                ui::text(div().text_color(c.text), &t.text.group_title).child(name),
                            )
                            .child(
                                ui::text(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(c.muted)
                                        .font_family(t.font.mono.clone()),
                                    &t.text.meta,
                                )
                                .child(file),
                            ),
                    )
                    .child(
                        ui::button("theme-revert", "Revert", ui::Button::Secondary)
                            .when(!unsaved, |el| el.opacity(0.5))
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(editing) = &view.editing {
                                    let saved = editing.saved.clone();
                                    editing
                                        .editor
                                        .update(cx, |editor, cx| editor.set_value(&saved, cx));
                                }
                            })),
                    )
                    .child(
                        ui::button("theme-save", "Save", ui::Button::Primary)
                            .on_click(cx.listener(|view, _, _, cx| view.save_theme(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(t.space.s8))
                    .rounded(px(t.radius.row))
                    .bg(c.field)
                    .border(px(t.size.hairline))
                    .border_color(if editing.problem.is_some() {
                        c.attention
                    } else {
                        c.line_strong
                    })
                    .child(editing.editor.clone()),
            )
            .child(status)
    }

    fn appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let current = Settings::current();
        let own = self
            .themes
            .iter()
            .any(|(entry, _)| entry.id == current.theme && entry.origin == themes::Origin::User);
        let card = |entry: &themes::Entry, colours: &design::Colors| {
            let theme = entry.id.clone();
            let chosen = current.theme == theme;
            let bar = |fill: gpui::Rgba, width: f32| {
                div()
                    .h(px(t.space.s4))
                    .w(gpui::relative(width))
                    .rounded(px(t.space.s2))
                    .bg(fill)
            };
            div()
                .id(SharedString::from(format!("theme-card-{theme}")))
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
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(entry.name.clone()),
                        )
                        .when(chosen, |el| {
                            el.child(ui::icon("check", t.size.icon, c.accent))
                        }),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.theme_note = None;
                    view.change(|settings| settings.theme = theme.clone(), cx)
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
        let sizes = super::zoom::SIZES;
        let at = sizes
            .iter()
            .position(|&percent| percent == current.interface_size);
        let resize = |id: &'static str, glyph: &'static str, next: Option<u32>| {
            div()
                .id(id)
                .w(px(t.size.button))
                .h(px(t.size.button - t.space.s2))
                .flex()
                .items_center()
                .justify_center()
                .text_color(if next.is_some() { c.text_2 } else { c.faint })
                .when_some(next, |el, next| {
                    el.cursor_pointer()
                        .hover(|el| el.bg(colors().hover))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.change(|settings| settings.interface_size = next, cx)
                        }))
                })
                .child(glyph)
        };
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
                    .flex_wrap()
                    .gap(px(t.space.s12))
                    .children(self.themes.iter().filter_map(|(entry, colours)| {
                        Some(card(entry, colours.as_ref()?))
                    }))
                    .into_any_element(),
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(t.space.s8))
                    .pt(px(t.space.s4))
                    .child(
                        ui::button("theme-customize", "Customize…", ui::Button::Secondary)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.customize_theme(window, cx)
                            })),
                    )
                    .when(own, |el| {
                        el.child(
                            ui::button("theme-edit", "Edit JSON", ui::Button::Secondary)
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.edit_theme(&themes::current(), window, cx)
                                })),
                        )
                    })
                    .when_some(self.vscode.clone(), |el, chosen| {
                        el.child(
                            ui::button(
                                "theme-import-current",
                                format!("Import \"{}\" from VS Code", chosen.name),
                                ui::Button::Secondary,
                            )
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    view.import_theme(
                                        &chosen.path,
                                        chosen.light,
                                        Some(&chosen.name),
                                        cx,
                                    )
                                },
                            )),
                        )
                    })
                    .child(
                        ui::button(
                            "theme-import",
                            "Import VS Code theme file…",
                            ui::Button::Secondary,
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.import_theme_file(cx))),
                    )
                    .into_any_element(),
                Self::note(if own {
                    "Your theme is a JSON file you can edit here. Unset values come from the theme it extends."
                } else {
                    "Customize copies this theme into a JSON file of your own to edit."
                })
                .into_any_element(),
            ]))
            .when_some(
                self.theme_note.clone().or_else(themes::problem),
                |el, note| {
                    el.child(
                        ui::text(div().pb(px(t.space.s16)).text_color(c.attention_text), &t.text.status)
                            .child(note),
                    )
                },
            )
            .child(Self::section([
                Self::label("INTERFACE SIZE").into_any_element(),
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
                            .child(resize(
                                "interface-smaller",
                                "−",
                                at.and_then(|at| at.checked_sub(1)).map(|at| sizes[at]),
                            ))
                            .child(
                                div()
                                    .w(px(t.size.button + t.space.s10))
                                    .flex()
                                    .justify_center()
                                    .child(format!("{}%", current.interface_size)),
                            )
                            .child(resize(
                                "interface-larger",
                                "+",
                                at.and_then(|at| sizes.get(at + 1).copied()),
                            )),
                    )
                    .child(Self::note(
                        "Scales the whole window, terminals included, for screens that draw it too small or too large.",
                    ))
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
                                    let chord = shortcuts::Bindings::default()
                                        .chord(command)
                                        .unwrap()
                                        .clone();
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
                                    bindings.chord(command).map_or_else(
                                        || "Not assigned".into(),
                                        shortcuts::Chord::label,
                                    )
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
        let (connected, detail) = match crate::roster::provider_directory(false) {
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
                Self::note("States refresh every two seconds.").into_any_element(),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let pages = [
            (Page::General, "General", "settings"),
            (Page::Appearance, "Appearance", "palette"),
            (Page::Shortcuts, "Shortcuts", "keyboard"),
            (Page::Agents, "Agents", "bot"),
            (Page::Profiles, "Profiles", "layers"),
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
        let editing = self.page == Page::Appearance && self.editing.is_some();
        let content = match self.page {
            Page::General => self.general(cx),
            Page::Appearance => match &self.editing {
                Some(editing) => self.theme_editor(editing, cx),
                None => self.appearance(cx),
            },
            Page::Shortcuts => self.shortcuts(cx),
            Page::Agents => self.agents(cx),
            Page::Profiles => {
                let profiles = self
                    .profiles
                    .get_or_insert_with(|| cx.new(super::settings_profiles::Profiles::new))
                    .clone();
                div().flex().flex_col().pt(px(t.space.s12)).child(profiles)
            }
        };
        let body = ui::text(
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
                // The editor scrolls itself and fills the page.
                .when(editing, |el| el.flex().flex_col())
                .when(!editing, |el| el.overflow_y_scroll())
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
        );
        super::window_controls::frame(body, "Settings", window, cx)
    }
}
