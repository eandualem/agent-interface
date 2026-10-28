//! Native workspace layout and existing tmux session connections.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

use agent_interface_core::{
    Attachment, AttachmentTarget, Direction, Layout, Leaf, NameEdit, RenameOutcome, Tab, View,
};
use agent_interface_store::Document;
use futures::{StreamExt, channel::mpsc};
use gpui::{
    AnyElement, App, Bounds, Context, KeyBinding, Menu, MenuItem, PathPromptOptions, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point, prelude::*, px,
    size,
};
use gpui_platform::application;

use crate::library::{Library, Update, WindowLayout};
mod chooser;
mod layout_edit;
mod name;
mod new_pane;
use chooser::{Choice, SessionChooser};
use layout_edit::{Create, Selection};
mod components;
mod design;
mod frame;
mod icons;
mod jump;
mod menus;
mod pane_cards;
mod pane_drag;
mod settings;
mod settings_window;
mod shortcuts;
mod sidebar;
mod split_resize;
mod status;
mod terminal;
mod views;
mod window_controls;
mod workspace;
use agent_interface_tmux::connection::SessionWindow;
use name::{NameEvent, NameField};
use terminal::{LiveTerminal, Target};

const APP_ID: &str = "io.github.eandualem.agent-interface";

actions!(
    library_browser,
    [
        /// Close the library browser without changing saved layouts or sessions.
        Quit,
        /// Open an independent native window on the current library.
        NewWindow,
        /// Choose another library folder for the active window.
        OpenLibrary,
        /// Show the Settings window.
        OpenSettings,
        /// Close the focused pane; the last pane closes its tab.
        ClosePane,
        /// Close the selected tab, leaving its sessions under tmux's ownership.
        CloseTab,
        /// Return the focused pane to its chooser.
        DetachSession,
        /// Copy selected text in the focused reader, field or terminal.
        Copy,
        /// Select all text in the focused reader or field.
        SelectAll,
        /// Close the active window; its sessions keep running.
        CloseWindow
    ]
);

/// Open the native workspace window and connect its saved, existing sessions.
/// An absent directory uses the shared library's normal path lookup.
pub fn run(directory: Option<PathBuf>) -> Result<(), String> {
    let source = crate::sessions::source_socket(None).map_err(|e| e.to_string());
    let preferences = settings::load();
    preferences.apply();
    let mut logging = crate::diagnostics::start(preferences.logging);
    let failure = Rc::new(RefCell::new(None));
    let opened = Rc::clone(&failure);
    application()
        .with_assets(icons::Assets)
        .run(move |cx: &mut App| {
            cx.set_app_identity(APP_ID, "Agent Interface");
            if cfg!(target_os = "linux") && cx.graphical_environment().is_none() {
                *opened.borrow_mut() =
                    Some("the workspace browser requires a graphical desktop".into());
                // Linux's event loop resets its stop flag when run() begins, after
                // this launch callback. Request exit on its first dispatched task.
                cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
                return;
            }
            configure_menu(cx);
            cx.on_app_quit(move |_| {
                // The synchronous observer runs before GPUI's short async quit
                // deadline. These workers finish without a foreground update.
                layout_edit::finish_saves(std::time::Duration::from_secs(10));
                settings::finish_saves(std::time::Duration::from_secs(2));
                // AppKit termination does not unwind the run() stack. Drain
                // diagnostics here, after the workers have logged their saves.
                drop(logging.take());
                std::future::ready(())
            })
            .detach();
            // Defer: a window that is dispatching this action cannot be updated
            // until dispatch returns.
            cx.on_action(|_: &OpenSettings, cx| cx.defer(settings_window::show));
            cx.on_action(|_: &ClosePane, cx| {
                if let Some(window) = cx.active_window() {
                    cx.defer(move |cx| {
                        if let Some(browser) = window.downcast::<Browser>() {
                            let _ = browser.update(cx, |browser, window, cx| {
                                browser.close_pane(window, cx);
                            });
                        } else {
                            let _ = window.update(cx, |_, window, _| window.remove_window());
                        }
                    });
                }
            });
            cx.on_action(|_: &CloseTab, cx| {
                if let Some(window) = cx.active_window() {
                    cx.defer(move |cx| {
                        if let Some(browser) = window.downcast::<Browser>() {
                            let _ = browser.update(cx, |browser, window, cx| {
                                browser.close_tab(window, cx);
                            });
                        }
                    });
                }
            });
            cx.on_action(|_: &DetachSession, cx| {
                if let Some(window) = cx
                    .active_window()
                    .and_then(|window| window.downcast::<Browser>())
                {
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |browser, window, cx| {
                            browser.detach_focused(window, cx);
                        });
                    });
                }
            });
            cx.on_action(|_: &CloseWindow, cx| {
                if let Some(window) = cx.active_window() {
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, _| window.remove_window());
                    });
                }
            });
            // The source chooses its library at launch; keep the native choice in
            // the application menu rather than in the panel.
            cx.on_action(|_: &OpenLibrary, cx| {
                if let Some(window) = browser_window(cx) {
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |browser, _, cx| browser.pick_library(cx));
                    });
                }
            });
            cx.on_action(|_: &Quit, cx| {
                cx.quit();
            });
            cx.on_action(|_: &NewWindow, cx| {
                // Dispatch currently borrows the originating window. Open after it
                // returns, retaining that window's identity even if focus changes.
                // With only Settings open, start a window on the default library.
                match browser_window(cx) {
                    Some(window) => cx.defer(move |cx| {
                        let _ = window.update(cx, |browser, _, cx| browser.open_another(cx));
                    }),
                    None => cx.defer(|cx| {
                        let source =
                            crate::sessions::source_socket(None).map_err(|e| e.to_string());
                        let _ = open_browser(None, source, cx);
                    }),
                }
            });
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            if let Err(error) = open_browser(directory, source, cx) {
                *opened.borrow_mut() = Some(format!("cannot open window: {error}"));
                cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
                return;
            }
            cx.activate(true);
        });
    failure.borrow_mut().take().map_or(Ok(()), Err)
}

fn application_bindings(bindings: &shortcuts::Bindings) -> Vec<KeyBinding> {
    use shortcuts::Command;
    let mut keys = vec![
        KeyBinding::new(shortcuts::platform_key("cmd-q"), Quit, None),
        KeyBinding::new(shortcuts::platform_key("cmd-n"), NewWindow, None),
        KeyBinding::new(shortcuts::platform_key("cmd-o"), OpenLibrary, None),
        KeyBinding::new(shortcuts::platform_key("cmd-,"), OpenSettings, None),
    ];
    if cfg!(target_os = "macos") {
        keys.push(KeyBinding::new("cmd-c", Copy, None));
        keys.push(KeyBinding::new("cmd-a", SelectAll, None));
    }
    if let Some(chord) = bindings.chord(Command::ClosePane) {
        keys.push(KeyBinding::new(&chord.to_text(), ClosePane, None));
    }
    if let Some(chord) = bindings.chord(Command::CloseTab) {
        keys.push(KeyBinding::new(&chord.to_text(), CloseTab, None));
    }
    if let Some(chord) = bindings.chord(Command::Detach) {
        keys.push(KeyBinding::new(&chord.to_text(), DetachSession, None));
    }
    keys
}

/// Replace menu bindings when preferences change, removing the previous chords.
fn configure_menu(cx: &mut App) {
    cx.clear_key_bindings();
    cx.bind_keys(application_bindings(&shortcuts::current()));
    cx.set_menus(vec![
        Menu {
            name: "Agent Interface".into(),
            items: vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Open Library…", OpenLibrary),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Close Pane", ClosePane),
                MenuItem::action("Detach", DetachSession),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Close Window", CloseWindow),
                MenuItem::action("Quit Agent Interface", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Copy", Copy, gpui::OsAction::Copy),
                MenuItem::os_action("Select All", SelectAll, gpui::OsAction::SelectAll),
            ],
            disabled: false,
        },
    ]);
}

/// Feed a native menu action through the same editor path as its shortcut.
fn edit_key(key: &str) -> gpui::KeyDownEvent {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse(&format!("{modifier}-{key}")).expect("editing shortcut"),
        is_held: false,
        prefer_character_input: false,
    }
}

/// The active workspace window, else any open one, for application commands
/// sent while another window, such as Settings, is active.
fn browser_window(cx: &App) -> Option<gpui::WindowHandle<Browser>> {
    cx.active_window()
        .and_then(|window| window.downcast::<Browser>())
        .or_else(|| {
            cx.windows()
                .into_iter()
                .find_map(|window| window.downcast::<Browser>())
        })
}

fn open_browser(
    directory: Option<PathBuf>,
    source: Result<PathBuf, String>,
    cx: &mut App,
) -> Result<(), String> {
    let options = WindowOptions {
        app_id: Some(APP_ID.into()),
        window_decorations: cfg!(target_os = "linux").then_some(gpui::WindowDecorations::Client),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(
                px(design::tokens().size.window_width),
                px(design::tokens().size.window_height),
            ),
            cx,
        ))),
        window_min_size: Some(size(
            px(design::tokens().size.window_min_width),
            px(design::tokens().size.window_min_height),
        )),
        titlebar: Some(TitlebarOptions {
            title: Some("Agent Interface".into()),
            // The window controls sit in the panel's top strip.
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(design::tokens().size.window_controls_x),
                px(design::tokens().size.window_controls_y),
            )),
        }),
        ..Default::default()
    };
    cx.open_window(options, |_, cx| {
        cx.new(|cx| {
            let mut browser = Browser {
                source: Some(source),
                shell_environment: Some(
                    agent_interface_tmux::shells::Environment::capture()
                        .map_err(|error| error.to_string()),
                ),
                navigation: Some(cx.focus_handle()),
                ..Browser::default()
            };
            browser.open(directory, cx);
            browser.start_roster(cx);
            // Stopping the library joins its storage worker, which can wait on
            // a locked database; keep that off the UI thread when the window closes.
            cx.on_release(|browser, cx| {
                if let Some(library) = browser.library.take() {
                    cx.background_executor()
                        .spawn(async move { drop(library) })
                        .detach();
                }
            })
            .detach();
            browser
        })
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

struct EditingName {
    target: Option<NameEdit>,
    tab: bool,
    field: gpui::Entity<NameField>,
    _events: gpui::Subscription,
}

/// A window's roster poller. Stopping it joins a worker that may be inside a
/// bounded request, so a closing window stops it off the UI thread.
struct Roster(Option<crate::roster::RosterSource>);

impl Roster {
    fn take_update(&self) -> Option<agent_interface_roster::Snapshot> {
        self.0
            .as_ref()
            .and_then(crate::roster::RosterSource::take_update)
    }
}

impl Drop for Roster {
    fn drop(&mut self) {
        if let Some(source) = self.0.take() {
            std::thread::spawn(move || drop(source));
        }
    }
}

#[derive(Clone, Copy)]
enum Popover {
    Workspace,
}

#[derive(Clone)]
enum TabAction {
    Rename,
    Attach,
    ReturnToShell,
    Detach,
    OpenTerminal,
    Split(Direction),
    Focus,
    Move(isize),
    Transfer(String),
}

#[derive(Clone)]
struct TabMenu {
    workspace: String,
    tab: Tab,
    shell: Option<AttachmentTarget>,
}

impl TabMenu {
    fn shell_proposal(&self, layout: &mut WindowLayout) -> Option<(Document, Layout)> {
        let (base, mut proposed) = layout.save_snapshot();
        if !self.shell.as_ref()?.apply(&mut proposed, None) {
            return None;
        }
        let _ = layout.model_mut().retain_navigation(&proposed);
        Some((base, proposed))
    }

    fn current(&self, layout: &agent_interface_core::Layout) -> bool {
        layout.selected == self.workspace && layout.tab() == Some(&self.tab)
    }
}

struct AttachmentEdit {
    tab: String,
    pane: String,
    empty: bool,
    target: AttachmentTarget,
}

impl AttachmentEdit {
    fn capture(model: &Layout) -> Option<Self> {
        Some(Self {
            tab: model.tab()?.id.clone(),
            pane: model.pane()?.id.clone(),
            empty: model.pane()?.empty,
            target: model.attachment_target()?,
        })
    }

    fn apply(&self, model: &mut Layout, attachment: Attachment) -> bool {
        if self.empty {
            let displayed = model
                .tab()
                .is_some_and(|tab| !model.focus_mode() || tab.focus == self.pane);
            displayed && model.fill_pane(&self.tab, &self.pane, Some(attachment))
        } else {
            self.target.apply(model, Some(attachment))
        }
    }

    fn displayed(&self, model: &Layout) -> bool {
        model.tab().is_some_and(|tab| {
            tab.id == self.tab
                && (!model.focus_mode() || tab.focus == self.pane)
                && tab.tree.leaves().iter().any(|leaf| leaf.id == self.pane)
        })
    }

    fn propose_shell(&self, layout: &mut WindowLayout) -> Option<(Document, Layout)> {
        let (base, mut proposed) = layout.save_snapshot();
        let leaf = proposed
            .tab()?
            .tree
            .leaves()
            .into_iter()
            .find(|leaf| leaf.id == self.pane)?;
        if leaf.attachment.is_some() {
            return None;
        }
        let applied = if self.empty {
            self.displayed(&proposed) && proposed.fill_pane(&self.tab, &self.pane, None)
        } else {
            self.target.apply(&mut proposed, None)
        };
        if !applied {
            return None;
        }
        let _ = layout.model_mut().retain_navigation(&proposed);
        Some((base, proposed))
    }

    fn propose(
        &self,
        layout: &mut WindowLayout,
        attachment: Attachment,
    ) -> Option<(Document, Layout)> {
        let (base, mut proposed) = layout.save_snapshot();
        if !self.apply(&mut proposed, attachment) {
            return None;
        }
        // Confirmation returns selection to the captured destination, but keeps
        // its original terminal until the detached proposal has committed.
        // The caller explicitly returns keyboard focus to navigation on confirmation.
        let _ = layout.model_mut().retain_navigation(&proposed);
        Some((base, proposed))
    }
}

struct Attaching {
    destination: AttachmentEdit,
    source: String,
    view: gpui::Entity<SessionChooser>,
    _events: gpui::Subscription,
}

fn close_tab_proposal(
    layout: &WindowLayout,
    workspace: &str,
    tab: &str,
) -> Option<(Document, Layout)> {
    let (base, mut proposed) = layout.save_snapshot();
    if proposed.selected != workspace || proposed.tab()?.id != tab {
        return None;
    }
    proposed.close_tab().then_some((base, proposed))
}

fn close_pane_proposal(
    layout: &WindowLayout,
    workspace: &str,
    tab: &str,
    pane: &Leaf,
) -> Option<(Document, Layout)> {
    let (base, mut proposed) = layout.save_snapshot();
    if proposed.selected != workspace || proposed.tab()?.id != tab || proposed.pane()? != pane {
        return None;
    }
    proposed.close_pane().then_some((base, proposed))
}

fn saved_view(layout: &Layout, id: &str, view: &View) -> bool {
    layout.workspaces.iter().any(|workspace| {
        workspace.tabs.iter().any(|tab| {
            tab.tree
                .leaves()
                .into_iter()
                .any(|leaf| leaf.id == id && leaf.view.as_ref() == Some(view))
        })
    })
}

#[derive(Default)]
struct Browser {
    library: Option<Library>,
    directory: Option<PathBuf>,
    source: Option<Result<PathBuf, String>>,
    source_matches: sidebar::SourceMatches,
    source_match_updates: Option<Task<()>>,
    terminals: HashMap<String, gpui::Entity<LiveTerminal>>,
    window_choices: HashMap<String, (Target, SessionWindow)>,
    layout: Option<WindowLayout>,
    error: Option<String>,
    loading: bool,
    workspace_menu: bool,
    icon_picker: bool,
    menu_at: Option<gpui::Point<gpui::Pixels>>,
    roster: Option<Roster>,
    agents: Option<agent_interface_roster::Snapshot>,
    roster_updates: Option<Task<()>>,
    jump: Option<jump::Jump>,
    /// A navigation panel drag: where it started and the width then.
    sidebar_drag: Option<(gpui::Pixels, gpui::Pixels)>,
    reset_focus: bool,
    painted_pane_focus: Option<String>,
    navigation: Option<gpui::FocusHandle>,
    updates: Option<Task<()>>,
    editing: Option<EditingName>,
    saving: bool,
    save_failed: bool,
    save_notice: Option<String>,
    name_click: Option<(String, bool)>,
    tab_menu: Option<TabMenu>,
    chooser: Option<Attaching>,
    // A failed proposed edit remains detached until its transaction succeeds.
    pending_save: Option<(Document, Layout)>,
    pending_creation: Option<(Create, String)>,
    creation_from: Option<Selection>,
    tabs_scroll: gpui::ScrollHandle,
    workspaces_scroll: gpui::ScrollHandle,
    shown_workspace: Option<(String, gpui::Pixels)>,
    shell_environment: Option<Result<agent_interface_tmux::shells::Environment, String>>,
    shell_after_save: Option<Leaf>,
    shell_to_open: Option<Leaf>,
    resize: Option<split_resize::Drag>,
    pane_drag: Option<pane_drag::Drag>,
    pane_bounds: Rc<RefCell<HashMap<String, Bounds<gpui::Pixels>>>>,
    split_bounds: Rc<RefCell<HashMap<String, Bounds<gpui::Pixels>>>>,
    resize_activation: Option<gpui::Subscription>,
    /// Agent Backbone's data directory, when it is set up.
    backbone: Option<PathBuf>,
    /// The view in each shown view pane, with its event subscription.
    views: HashMap<String, (gpui::Entity<views::ViewPane>, gpui::Subscription)>,
    /// The chooser in each shown empty pane, with its event subscription.
    new_panes: HashMap<String, (gpui::Entity<new_pane::NewPane>, gpui::Subscription)>,
    /// The section each pane opening Reports & updates shows once its view
    /// appears, and the pane whose save is still deciding its choice.
    opening_sections: HashMap<String, views::Section>,
    section_saving: Option<String>,
}

impl Browser {
    fn reader_edit(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let handled = self
            .navigation
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window))
            && self
                .layout
                .as_ref()
                .and_then(|layout| layout.model().pane())
                .and_then(|pane| self.views.get(&pane.id))
                .is_some_and(|(view, _)| {
                    view.update(cx, |view, cx| view.key(&edit_key(key).keystroke, cx))
                });
        if !handled {
            cx.propagate();
        }
    }

    fn close_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        let Some(model) = self.layout.as_ref().map(WindowLayout::model) else {
            return;
        };
        let Some((tab, pane)) = model.tab().zip(model.pane()) else {
            return;
        };
        let (workspace, tab, pane) = (model.selected.clone(), tab.id.clone(), pane.clone());
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let Some(proposal) = close_pane_proposal(layout, &workspace, &tab, &pane) else {
            cx.notify();
            return;
        };
        self.pending_save = Some(proposal);
        self.editing = None;
        self.chooser = None;
        self.close_menus();
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
    }

    fn detach_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(leaf) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().pane())
            .cloned()
        {
            let target = Target {
                leaf,
                directory: self.directory.clone(),
                source: self
                    .source
                    .clone()
                    .unwrap_or_else(|| Err("Source socket unavailable".into())),
            };
            self.detach_pane(&target, window, cx);
        }
    }

    fn close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        let Some(model) = self.layout.as_ref().map(WindowLayout::model) else {
            return;
        };
        let Some(tab) = model.tab().map(|tab| tab.id.clone()) else {
            return;
        };
        let workspace = model.selected.clone();
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let Some(proposal) = close_tab_proposal(layout, &workspace, &tab) else {
            cx.notify();
            return;
        };
        self.pending_save = Some(proposal);
        self.editing = None;
        self.chooser = None;
        self.close_menus();
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
    }

    fn detach_pane(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let (base, mut proposed) = layout.save_snapshot();
        if !target.detach_from(&mut proposed) {
            return;
        }
        self.pending_save = Some((base, proposed));
        self.editing = None;
        self.chooser = None;
        self.close_menus();
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
    }

    fn open_another(&mut self, cx: &mut Context<Self>) {
        let source = self
            .source
            .clone()
            .unwrap_or_else(|| Err("Source socket unavailable".into()));
        if let Err(error) = open_browser(self.directory.clone(), source, cx) {
            self.error = Some(format!("Cannot open window: {error}"));
            cx.notify();
        }
    }

    fn stop_reader(&mut self, cx: &mut Context<Self>) {
        self.updates = None;
        if let Some(previous) = self.library.take() {
            cx.background_executor()
                .spawn(async move { drop(previous) })
                .detach();
        }
    }

    fn open(&mut self, directory: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        self.jump = None;
        self.editing = None;
        self.chooser = None;
        self.shell_after_save = None;
        self.shell_to_open = None;
        self.resize = None;
        self.pane_drag = None;
        self.name_click = None;
        self.tab_menu = None;
        self.save_notice = None;
        self.stop_reader(cx);
        self.source_match_updates = None;
        self.source_matches = sidebar::SourceMatches::default();
        self.terminals.clear();
        self.window_choices.clear();
        self.reset_focus = true;
        self.directory = directory.clone();
        self.layout = None;
        self.error = None;
        self.loading = true;
        self.start_reader(cx);
    }

    /// Poll Agent Backbone for agent states when it is installed. Without it
    /// the panel has no agents section.
    fn start_roster(&mut self, cx: &mut Context<Self>) {
        let Ok(Some(directory)) = crate::roster::provider_directory(None, None, None, false) else {
            return;
        };
        self.backbone = Some(directory.clone());
        let (wake, mut woken) = mpsc::unbounded();
        let Ok(roster) = crate::roster::RosterSource::start(directory, None, move || {
            let _ = wake.unbounded_send(());
        }) else {
            return;
        };
        self.roster = Some(Roster(Some(roster)));
        self.roster_updates = Some(cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                let updated = this.update(cx, |browser, cx| {
                    if let Some(snapshot) = browser.roster.as_ref().and_then(Roster::take_update) {
                        let changed = browser.agents.as_ref().is_none_or(|previous| {
                            previous.states != snapshot.states || previous.error != snapshot.error
                        });
                        browser.agents = Some(snapshot);
                        browser.refresh_source_matches(cx);
                        if changed {
                            // Clear retained paint when status changes, including
                            // badges beside cached terminal bodies.
                            cx.refresh_windows();
                        }
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        }));
    }

    /// Show the tab holding this agent's session: the current tab if it does,
    /// else the first in saved order. Nothing is attached or started.
    fn go_to_agent(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let source = self
            .source
            .as_ref()
            .and_then(|source| source.as_ref().ok())
            .and_then(|path| crate::library::location::resolve_path(path).ok());
        let Some(layout) = &mut self.layout else {
            return;
        };
        let model = layout.model_mut();
        // A saved socket matches only when both resolve to the same path; two
        // failures never match.
        let found = agent_destination(model, name, |socket| match socket {
            None => source.is_some(),
            Some(socket) => {
                let saved =
                    crate::library::location::resolve_path(std::path::Path::new(socket)).ok();
                saved.is_some() && saved == source
            }
        });
        if let Some((space, tab, pane)) = found {
            model.choose_workspace(&space);
            model.choose_tab(&tab);
            model.choose_pane(&pane);
            self.save_notice = None;
        } else {
            self.save_notice = Some(format!("No tab shows {name}"));
        }
        self.close_menus();
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn start_reader(&mut self, cx: &mut Context<Self>) {
        let directory = self.directory.clone();
        let (wake, mut woken) = mpsc::unbounded();
        match Library::start(directory, move || {
            let _ = wake.unbounded_send(());
        }) {
            Ok(library) => self.library = Some(library),
            Err(error) => {
                self.error = Some(error.to_string());
                self.loading = false;
            }
        }
        self.updates = Some(cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                if this
                    .update(cx, |browser, cx| {
                        if let Some(update) =
                            browser.library.as_ref().and_then(Library::take_update)
                        {
                            browser.apply(update);
                            browser.refresh_source_matches(cx);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn apply(&mut self, update: Update) {
        self.loading = false;
        self.error = update.error.map(|error| error.to_string());
        if let Some(document) = update.document {
            if let Some(layout) = &mut self.layout {
                match layout.refresh(document, "layouts.db".as_ref()) {
                    Ok(result) if result.conflict => {
                        self.reset_focus = true;
                        self.error =
                            Some("The library changed. Showing the latest saved layout.".into());
                    }
                    Ok(result) => {
                        self.reset_focus |= result.active_tree_changed;
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            } else {
                self.layout = Some(WindowLayout::new(document));
            }
        }
    }

    fn begin_name(&mut self, tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed || self.editing.is_some() {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let target = self.layout.as_ref().and_then(|layout| {
            if tab {
                layout.model().edit_tab_name()
            } else {
                layout.model().edit_workspace_name()
            }
        });
        let Some(target) = target else {
            return;
        };
        self.edit_name(Some(target), tab, window, cx);
    }

    fn begin_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed || self.editing.is_some() || self.layout.is_none() {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        self.edit_name(None, false, window, cx);
    }

    fn edit_name(
        &mut self,
        target: Option<NameEdit>,
        tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = target.as_ref().map_or("", NameEdit::name);
        let field = cx.new(|cx| NameField::new(name, window, cx));
        let events = cx.subscribe_in(
            &field,
            window,
            |browser, field, event, window, cx| match event {
                NameEvent::Changed(_) => {}
                NameEvent::Save => browser.submit_name(window, cx),
                NameEvent::Cancel => {
                    if field.read(cx).focus.is_focused(window)
                        && let Some(focus) = &browser.navigation
                    {
                        window.focus(focus, cx);
                    }
                    browser.editing = None;
                    cx.notify();
                }
            },
        );
        self.workspace_menu = false;
        self.tab_menu = None;
        self.chooser = None;
        self.reset_focus = false;
        self.save_notice = None;
        self.editing = Some(EditingName {
            target,
            tab,
            field,
            _events: events,
        });
        cx.notify();
    }

    fn submit_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let (Some(edit), Some(layout)) = (&self.editing, &mut self.layout) else {
            return;
        };
        let outcome = if let Some(target) = &edit.target {
            target.apply(layout.model_mut(), edit.field.read(cx).value())
        } else if let Some(proposal) = workspace::create(layout, edit.field.read(cx).value()) {
            self.creation_from = Some(Selection::capture(layout.model()));
            self.pending_save = Some(proposal);
            RenameOutcome::Applied
        } else {
            RenameOutcome::Empty
        };
        match outcome {
            RenameOutcome::Empty => return,
            RenameOutcome::Changed => {
                self.editing = None;
                self.save_notice =
                    Some("The name or selection changed. Rename it again to retry.".into());
            }
            RenameOutcome::Applied => {
                self.editing = None;
                self.save(cx);
            }
        }
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn delete_workspace(&mut self, expected: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(proposal) = self
            .layout
            .as_ref()
            .and_then(|layout| workspace::delete(layout, expected))
        else {
            self.save_notice = Some(
                "Only an empty workspace can be deleted, and one workspace must remain.".into(),
            );
            cx.notify();
            return;
        };
        self.pending_save = Some(proposal);
        self.workspace_menu = false;
        self.editing = None;
        self.tab_menu = None;
        self.chooser = None;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
    }

    fn create(&mut self, action: Create, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &self.layout else {
            return;
        };
        // An app opened from the Dock or Finder runs in "/". New terminals
        // start in the home folder instead, as other terminal apps do.
        let cwd = match std::env::current_dir()
            .ok()
            .map(|path| {
                if path.as_os_str() == "/" {
                    crate::library::location::user_home(None).unwrap_or(path)
                } else {
                    path
                }
            })
            .and_then(|path| path.to_str().map(str::to_owned))
        {
            Some(cwd) => cwd,
            None => {
                self.save_notice = Some("Cannot read the launch directory as text.".into());
                cx.notify();
                return;
            }
        };
        self.creation_from = Some(Selection::capture(layout.model()));
        self.pending_creation = Some((action, cwd));
        self.editing = None;
        self.chooser = None;
        self.tab_menu = None;
        self.workspace_menu = false;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let previous = layout.model().tab().cloned();
        let creation = self.pending_creation.take();
        let (mut base, mut local) = self
            .pending_save
            .take()
            .unwrap_or_else(|| layout.save_snapshot());
        let directory = self.directory.clone();
        // Discard all pending reads before saving. A fresh reader starts only
        // after applying the result, so no older snapshot can undo the commit.
        self.stop_reader(cx);
        self.saving = true;
        self.save_failed = false;
        self.save_notice = None;
        self.error = None;
        // Closing this window does not cancel a save while the app is running.
        // SQLite and its busy timeout never run on the UI thread.
        let pending = layout_edit::PendingSave::start();
        let work = cx.background_executor().spawn(async move {
            let _pending = pending;
            let result = layout_edit::save(
                directory.as_deref(),
                &mut base,
                &mut local,
                previous.as_ref(),
                creation,
            );
            (base, local, result)
        });
        cx.spawn(async move |this, cx| {
            let (base, local, result) = work.await;
            let _ = this.update(cx, |browser, cx| {
                browser.saving = false;
                match result {
                    Ok(result) => {
                        if let Some(layout) = &mut browser.layout {
                            let follow = browser.creation_from.as_ref()
                                .is_some_and(|original| original.unchanged(layout.model()));
                            browser.reset_focus |= layout.saved(base, &result);
                            if follow && !result.conflict {
                                layout_edit::select_created(layout.model_mut(), &local);
                                browser.reset_focus = true;
                                if let Some(space) = layout.model().workspace()
                                    && let Some(index) = space.tabs.iter().position(|tab| tab.id == space.selected)
                                { browser.tabs_scroll.scroll_to_item(index); }
                            }
                        }
                        browser.creation_from = None;
                        browser.shell_to_open = if result.conflict { browser.shell_after_save = None; None } else { browser.shell_after_save.take() };
                        browser.start_reader(cx);
                        if result.conflict {
                            if let Some(pane) = browser.section_saving.take() {
                                browser.opening_sections.remove(&pane);
                            }
                            browser.save_notice = Some("The library changed. Your edit was not saved; repeat it on the latest layout to retry.".into());
                        }
                        browser.section_saving = None;
                    }
                    Err(error) => {
                        browser.save_failed = true;
                        browser.pending_save = Some((base, local));
                        browser.error = Some(format!("Changes not saved: {error}. Retry saving."));
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn discard_save(&mut self, cx: &mut Context<Self>) {
        if !self.save_failed || self.saving {
            return;
        }
        self.pending_save = None;
        if let Some(pane) = self.section_saving.take() {
            self.opening_sections.remove(&pane);
        }
        self.pending_creation = None;
        self.creation_from = None;
        self.shell_after_save = None;
        self.shell_to_open = None;
        self.resize = None;
        self.pane_drag = None;
        self.save_failed = false;
        self.save_notice = None;
        self.error = None;
        if let Some(layout) = &mut self.layout {
            self.reset_focus |= layout.discard_edits();
        }
        self.start_reader(cx);
        cx.notify();
    }

    fn open_tab_menu(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        let model = layout.model_mut();
        if !model.choose_tab(id) {
            return;
        }
        self.tab_menu = model.tab().cloned().map(|tab| TabMenu {
            workspace: model.selected.clone(),
            shell: model
                .pane()
                .and_then(|pane| pane.attachment.as_ref())
                .and_then(|_| model.attachment_target()),
            tab,
        });
        self.editing = None;
        self.chooser = None;
        self.workspace_menu = false;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn tab_action(&mut self, action: TabAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.tab_menu.take() else {
            return;
        };
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        if matches!(action, TabAction::ReturnToShell) {
            if let Some(proposal) = menu.shell_proposal(layout) {
                self.pending_save = Some(proposal);
                self.save(cx);
            } else {
                self.save_notice =
                    Some("The destination changed. Open its options again to retry.".into());
            }
            if let Some(focus) = &self.navigation {
                window.focus(focus, cx);
            }
            cx.notify();
            return;
        }
        if !menu.current(layout.model()) {
            self.save_notice = Some("The tab changed. Open its options again to retry.".into());
            cx.notify();
            return;
        }
        match action {
            TabAction::ReturnToShell => unreachable!("handled before the whole-tab move guard"),
            TabAction::Detach => {
                self.detach_focused(window, cx);
            }
            TabAction::Rename => self.begin_name(true, window, cx),
            TabAction::Attach => {
                if let Some(pane) = layout.model().pane().map(|pane| pane.id.clone()) {
                    self.open_attach(&pane, window, cx);
                }
            }
            TabAction::OpenTerminal => {
                if let Some(pane) = layout.model().pane().map(|pane| pane.id.clone()) {
                    self.open_terminal(&pane, None, window, cx);
                }
            }
            TabAction::Split(direction) => self.create(Create::Split(direction), window, cx),
            TabAction::Focus => {
                let model = layout.model_mut();
                model.set_focus_mode(!model.focus_mode());
            }
            TabAction::Move(offset) => {
                layout.model_mut().move_tab(offset);
                self.save(cx);
            }
            TabAction::Transfer(destination) => {
                if layout.model_mut().transfer_tab(&destination) {
                    self.save(cx);
                }
            }
        }
        cx.notify();
    }

    fn open_menu(
        &mut self,
        menu: Popover,
        at: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menus();
        self.menu_at = Some(at);
        match menu {
            Popover::Workspace => self.workspace_menu = true,
        }
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn close_menus(&mut self) {
        self.tab_menu = None;
        self.workspace_menu = false;
        self.icon_picker = false;
    }

    fn set_symbol(&mut self, symbol: Option<&str>, cx: &mut Context<Self>) {
        self.close_menus();
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        if layout.model_mut().set_workspace_symbol(symbol) {
            self.save(cx);
        }
        cx.notify();
    }

    /// Open a terminal from an empty pane's chooser, only if the pane is
    /// still empty once shared changes are applied.
    fn open_terminal_in_empty(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let empty = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().tab())
            .is_some_and(|tab| {
                tab.tree
                    .leaves()
                    .iter()
                    .any(|leaf| leaf.id == pane && leaf.empty)
            });
        if empty {
            self.open_terminal(pane, None, window, cx);
        } else {
            self.save_notice = Some("The pane changed. Choose again.".into());
            cx.notify();
        }
    }

    fn open_terminal(
        &mut self,
        pane: &str,
        expected: Option<&Target>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(Err(error)) = &self.shell_environment {
            self.save_notice = Some(format!("Cannot open terminal: {error}"));
            cx.notify();
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        if expected.is_some_and(|target| {
            !layout.model().tab().is_some_and(|tab| {
                tab.tree
                    .leaves()
                    .iter()
                    .any(|leaf| target.matches_leaf(leaf))
            })
        }) {
            return;
        }
        if !layout
            .model()
            .tab()
            .is_some_and(|tab| !layout.model().focus_mode() || tab.focus == pane)
            || !layout.model_mut().choose_pane(pane)
        {
            return;
        }
        let Some(target) = AttachmentEdit::capture(layout.model()) else {
            return;
        };
        let Some(proposal) = target.propose_shell(layout) else {
            self.save_notice = Some("The pane changed. Choose Open terminal again.".into());
            cx.notify();
            return;
        };
        self.shell_after_save = proposal.1.pane().cloned();
        self.pending_save = Some(proposal);
        self.editing = None;
        self.chooser = None;
        self.tab_menu = None;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
    }

    /// Show a view in an empty pane. Like opening a terminal, the pane changes
    /// only once the library has saved it.
    /// Returns whether the save started.
    fn open_view(
        &mut self,
        pane: &str,
        view: View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.saving || self.save_failed {
            return false;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return false;
        };
        if !layout
            .model()
            .tab()
            .is_some_and(|tab| !layout.model().focus_mode() || tab.focus == pane)
            || !layout.model_mut().choose_pane(pane)
        {
            return false;
        }
        let Some(target) = AttachmentEdit::capture(layout.model()) else {
            return false;
        };
        let (base, mut proposed) = layout.save_snapshot();
        if !target.displayed(&proposed) || !proposed.show_view(&target.tab, &target.pane, view) {
            self.save_notice = Some("The pane changed. Choose the view again.".into());
            cx.notify();
            return false;
        }
        let _ = layout.model_mut().retain_navigation(&proposed);
        self.pending_save = Some((base, proposed));
        self.editing = None;
        self.chooser = None;
        self.tab_menu = None;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
        true
    }

    /// Split `pane` and show `view` in the new pane to its right, saved like
    /// any split before it appears.
    fn open_beside(&mut self, pane: &str, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        if !layout.model_mut().choose_pane(pane) {
            return;
        }
        let (base, mut proposed) = layout.save_snapshot();
        let cwd = proposed
            .pane()
            .and_then(|leaf| leaf.cwd.clone())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/".into());
        if !proposed.choose_pane(pane) || !proposed.split(Direction::Right, cwd, true) {
            return;
        }
        let (Some(tab), Some(added)) = (
            proposed.tab().map(|tab| tab.id.clone()),
            proposed.pane().map(|leaf| leaf.id.clone()),
        ) else {
            return;
        };
        if !proposed.show_view(&tab, &added, view) {
            return;
        }
        // The new pane is not in the displayed tree yet; select it once saved.
        self.creation_from = Some(Selection::capture(layout.model()));
        self.pending_save = Some((base, proposed));
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        self.save(cx);
    }

    /// Attach a session chosen in an empty pane's chooser, saved like any
    /// attachment before the pane changes.
    fn attach_into(&mut self, pane: &str, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(source) = self
            .source
            .as_ref()
            .and_then(|source| source.as_ref().ok())
            .and_then(|path| path.to_str())
            .map(str::to_owned)
        else {
            self.save_notice = Some("The source tmux server is unavailable.".into());
            cx.notify();
            return;
        };
        let Some(layout) = &mut self.layout else {
            return;
        };
        if !layout
            .model()
            .tab()
            .is_some_and(|tab| !layout.model().focus_mode() || tab.focus == pane)
            || !layout.model_mut().choose_pane(pane)
        {
            return;
        }
        let Some(destination) = AttachmentEdit::capture(layout.model()) else {
            return;
        };
        // Another window may have filled the pane since the chooser drew it.
        if !destination.empty {
            self.save_notice = Some("The pane changed. Choose again.".into());
            cx.notify();
            return;
        }
        let attachment = Attachment {
            session: name.into(),
            source_socket: Some(source),
        };
        if let Some(proposal) = destination.propose(layout, attachment) {
            self.pending_save = Some(proposal);
            self.save(cx);
        } else {
            self.save_notice = Some("The pane changed. Choose the session again.".into());
        }
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn open_attach(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        if !layout
            .model()
            .tab()
            .is_some_and(|tab| !layout.model().focus_mode() || tab.focus == pane)
            || !layout.model_mut().choose_pane(pane)
        {
            return;
        }
        let Some(destination) = AttachmentEdit::capture(layout.model()) else {
            return;
        };
        let source = match &self.source {
            Some(Ok(path)) => path.clone(),
            _ => {
                self.error = Some("Source socket unavailable".into());
                cx.notify();
                return;
            }
        };
        let Some(saved_source) = source.to_str().map(str::to_owned) else {
            self.error = Some("The source socket path cannot be saved as text.".into());
            cx.notify();
            return;
        };
        self.editing = None;
        self.tab_menu = None;
        self.chooser = None;
        self.reset_focus = false;
        let view = cx.new(|cx| SessionChooser::new(source, window, cx));
        let events = cx.subscribe_in(&view, window, |browser, chooser, event, window, cx| {
            if !browser
                .chooser
                .as_ref()
                .is_some_and(|active| active.view == *chooser)
            {
                return;
            }
            match event {
                Choice::Cancel => {
                    if chooser.read(cx).focused(window, cx)
                        && let Some(focus) = &browser.navigation
                    {
                        window.focus(focus, cx);
                    }
                    browser.chooser = None;
                }
                Choice::Session(name) => browser.attach_session(name, window, cx),
            }
            cx.notify();
        });
        self.chooser = Some(Attaching {
            destination,
            source: saved_source,
            view,
            _events: events,
        });
        cx.notify();
    }

    fn attach_session(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chooser) = self.chooser.take() else {
            return;
        };
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        let attachment = Attachment {
            session: name.into(),
            source_socket: Some(chooser.source),
        };
        if let Some(proposal) = chooser.destination.propose(layout, attachment) {
            // The displayed model retains its original terminal until saving
            // completes. Failed or conflicting edits never start a replacement.
            self.pending_save = Some(proposal);
            self.save(cx);
        } else {
            self.save_notice =
                Some("The destination changed. Open Attach session again to retry.".into());
        }
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn name_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let field = self
            .editing
            .as_ref()
            .expect("active name editor")
            .field
            .clone();
        // Enter saves and Escape or clicking elsewhere cancels; the row under
        // the heading says so, as in the source's inline editor.
        div()
            .min_w_0()
            .w_full()
            .on_mouse_down_out(cx.listener(|browser, _, _, cx| {
                browser.editing = None;
                cx.notify();
            }))
            .child(field)
            .into_any_element()
    }

    fn pick_library(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open library".into()),
        });
        cx.spawn(async move |this, cx| match paths.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |browser, cx| browser.open(Some(path), cx));
                }
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |browser, cx| {
                    browser.error = Some(error.to_string());
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    fn begin_resize(
        &mut self,
        split: &str,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || self.save_failed {
            return;
        }
        self.pane_drag = None;
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        self.resize = self.layout.as_ref().and_then(|layout| {
            let bounds = *self.split_bounds.borrow().get(split)?;
            split_resize::Drag::begin(layout.model(), split, bounds, position)
        });
        if self.resize.is_some() {
            self.editing = None;
            self.chooser = None;
            self.tab_menu = None;
            self.workspace_menu = false;
            if let Some(focus) = &self.navigation {
                window.focus(focus, cx);
            }
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Keep a dragged panel width: save it and repaint every window.
    fn finish_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        self.sidebar_drag = None;
        let (done, saved) = futures::channel::oneshot::channel();
        settings::save_in_background(settings::Settings::current(), move |result| {
            let _ = done.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Err(error)) = saved.await {
                let _ = this.update(cx, |browser, cx| {
                    browser.save_notice = Some(format!("Panel width not saved: {error}"));
                    cx.notify();
                });
            }
        })
        .detach();
        cx.refresh_windows();
    }

    fn resize_motion(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_drag.as_ref().is_some_and(|drag| drag.dragging) {
            self.pane_drag_motion(event, window, cx);
            return;
        }
        if let Some((start, width)) = self.sidebar_drag {
            if event.dragging() {
                let width = f32::from(width + event.position.x - start).round();
                settings::set_sidebar_width(width.max(0.0) as u32);
            } else {
                self.finish_sidebar_drag(cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some(drag) = &mut self.resize else {
            return;
        };
        if !drag.dragging {
            return;
        }
        if event.dragging()
            && self
                .layout
                .as_ref()
                .is_some_and(|layout| drag.valid(layout.model()))
            && self.split_bounds.borrow().get(&drag.split) == Some(&drag.bounds)
        {
            drag.motion(event.position);
        } else {
            self.resize = None;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn finish_resize(
        &mut self,
        event: &gpui::MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != gpui::MouseButton::Left {
            return;
        }
        if self.pane_drag.as_ref().is_some_and(|drag| drag.dragging) {
            // Pane dragging ends after button presses have cleared in capture.
            return;
        }
        if self.sidebar_drag.is_some() {
            self.finish_sidebar_drag(cx);
            return;
        }
        let Some(drag) = &mut self.resize else {
            return;
        };
        if !drag.dragging {
            return;
        }
        drag.dragging = false;
        drag.motion(event.position);
        let proposal = self.layout.as_ref().and_then(|layout| {
            (self.split_bounds.borrow().get(&drag.split) == Some(&drag.bounds))
                .then(|| drag.proposal(layout))
                .flatten()
        });
        if let Some(proposal) = proposal {
            self.pending_save = Some(proposal);
            self.save(cx);
        } else {
            self.resize = None;
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.resize_activation.is_none() {
            self.resize_activation =
                Some(cx.observe_window_activation(window, |browser, window, cx| {
                    if !window.is_window_active() {
                        if browser.resize.as_ref().is_some_and(|drag| drag.dragging) {
                            browser.resize = None;
                            cx.notify();
                        }
                        if let Some(drag) = &mut browser.pane_drag
                            && drag.dragging
                        {
                            drag.cancel();
                            cx.notify();
                        }
                    }
                }));
        }
        if self.resize.as_ref().is_some_and(|drag| {
            !self
                .layout
                .as_ref()
                .is_some_and(|layout| drag.valid(layout.model()))
        }) {
            self.resize = None;
        }
        self.split_bounds.borrow_mut().clear();
        if self.pane_drag.as_ref().is_some_and(|drag| {
            !self
                .layout
                .as_ref()
                .is_some_and(|layout| drag.valid(layout.model()))
        }) {
            if let Some(drag) = &mut self.pane_drag
                && drag.dragging
            {
                drag.cancel();
            } else {
                self.pane_drag = None;
            }
        }
        self.pane_bounds.borrow_mut().clear();
        let resize_motion = cx.listener(Self::resize_motion);
        let resize_end = cx.listener(Self::finish_resize);
        let pane_drag_end = cx.listener(|browser, event: &gpui::MouseUpEvent, _, cx| {
            if event.button == gpui::MouseButton::Left
                && browser.pane_drag.as_ref().is_some_and(|drag| drag.dragging)
            {
                browser.finish_pane_drag(event, cx);
            }
        });
        if self.chooser.as_ref().is_some_and(|chooser| {
            !self
                .layout
                .as_ref()
                .is_some_and(|layout| chooser.destination.displayed(layout.model()))
        }) {
            self.chooser = None;
            self.reset_focus = true;
        }
        if std::mem::take(&mut self.reset_focus)
            && let Some(focus) = &self.navigation
        {
            window.focus(focus, cx);
        }
        // A new entity owns each connection generation. Results from a removed
        // leaf cannot install a view or accept keys in its replacement.
        let leaves = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().tab())
            .map(|tab| {
                tab.tree
                    .leaves()
                    .into_iter()
                    .filter(|leaf| leaf.runs_terminal())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Hidden tabs retain their clients. Record window choices for targets
        // that may need to reconnect after a saved arrangement changes.
        for (id, terminal) in &self.terminals {
            let terminal = terminal.read(cx);
            if let Some(selected) = terminal.selection() {
                self.window_choices
                    .insert(id.clone(), (terminal.target.clone(), selected));
            }
        }
        self.window_choices.retain(|_, (target, _)| {
            self.layout
                .as_ref()
                .is_some_and(|layout| target.exists_in(layout.model()))
        });
        self.terminals.retain(|_, terminal| {
            self.layout
                .as_ref()
                .is_some_and(|layout| terminal.read(cx).target.exists_in(layout.model()))
        });
        for leaf in &leaves {
            let target = Target {
                leaf: leaf.clone(),
                directory: self.directory.clone(),
                source: self
                    .source
                    .clone()
                    .unwrap_or_else(|| Err("Source socket unavailable".into())),
            };
            if !self
                .terminals
                .get(&leaf.id)
                .is_some_and(|view| view.read(cx).target.same_session(&target))
            {
                let selected = self
                    .window_choices
                    .get(&leaf.id)
                    .filter(|(previous, _)| previous.same_session(&target))
                    .map(|(_, selected)| selected.clone());
                let environment = self
                    .shell_environment
                    .as_ref()
                    .and_then(|environment| environment.as_ref().ok())
                    .cloned();
                let terminal = cx.new(|cx| LiveTerminal::new(target, selected, environment, cx));
                cx.subscribe_in(
                    &terminal,
                    window,
                    |browser, terminal, _: &terminal::Detach, window, cx| {
                        let target = terminal.read(cx).target.clone();
                        browser.detach_pane(&target, window, cx);
                    },
                )
                .detach();
                cx.subscribe_in(
                    &terminal,
                    window,
                    |browser, terminal, _: &terminal::OpenTerminal, window, cx| {
                        let target = terminal.read(cx).target.clone();
                        browser.open_terminal(&target.leaf.id, Some(&target), window, cx);
                    },
                )
                .detach();
                cx.subscribe(
                    &terminal,
                    |browser, terminal, _: &crate::view::Focus, cx| {
                        let target = &terminal.read(cx).target;
                        if let Some(layout) = &mut browser.layout
                            && layout.model().tab().is_some_and(|tab| {
                                tab.tree
                                    .leaves()
                                    .iter()
                                    .any(|leaf| target.matches_leaf(leaf))
                            })
                        {
                            layout.model_mut().choose_pane(&target.leaf.id);
                            for (view, _) in browser.views.values() {
                                view.read(cx).focus_native(cx);
                            }
                            cx.notify();
                        }
                    },
                )
                .detach();
                self.terminals.insert(leaf.id.clone(), terminal);
            }
            // A stopped Backbone agent's pane offers Resume and Start new.
            let agent = leaf.attachment.as_ref().and_then(|attachment| {
                let snapshot = self.agents.as_ref()?;
                if snapshot.error.is_some()
                    || snapshot.states.get(&attachment.session)? != "offline"
                    || !self.is_window_source(attachment.source_socket.as_deref())
                {
                    return None;
                }
                Some((self.backbone.clone()?, attachment.session.clone()))
            });
            if let Some(terminal) = self.terminals.get(&leaf.id) {
                terminal.update(cx, |terminal, cx| {
                    terminal.set_cwd(leaf);
                    terminal.set_agent(agent, cx)
                });
            }
        }
        let shown: Vec<(String, View)> = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().tab())
            .map(|tab| {
                tab.tree
                    .leaves()
                    .into_iter()
                    .filter_map(|leaf| Some((leaf.id.clone(), leaf.view.clone()?)))
                    .collect()
            })
            .unwrap_or_default();
        self.views.retain(|id, (pane, _)| {
            let pane = pane.read(cx);
            shown
                .iter()
                .any(|(leaf, view)| leaf == id && &pane.view == view)
                || (pane.has_conversations()
                    && self
                        .layout
                        .as_ref()
                        .is_some_and(|layout| saved_view(layout.model(), id, &pane.view)))
        });
        for (id, view) in shown.clone() {
            if !self.views.contains_key(&id) {
                let backbone = self.backbone.clone();
                let directory = self.directory.clone();
                let reports = view.kind == agent_interface_core::ViewKind::Reports;
                let pane = cx.new(|cx| views::ViewPane::new(view, backbone, directory, window, cx));
                if let Some(section) = self.opening_sections.remove(&id)
                    && reports
                {
                    pane.update(cx, |pane, cx| pane.open_section(section, window, cx));
                }
                let leaf = id.clone();
                let events = cx.subscribe_in(
                    &pane,
                    window,
                    move |browser, _, event, window, cx| match event {
                        views::ViewEvent::ShowAgent(name) => browser.go_to_agent(name, window, cx),
                        views::ViewEvent::OpenBeside(view) => {
                            browser.open_beside(&leaf, view.clone(), window, cx)
                        }
                        views::ViewEvent::ReturnFocus => {
                            if let Some(focus) = &browser.navigation {
                                window.focus(focus, cx);
                            }
                        }
                        views::ViewEvent::Focus => {
                            if let Some(layout) = &mut browser.layout {
                                layout.model_mut().choose_pane(&leaf);
                                // GPUI also handles key equivalents forwarded by
                                // child webviews. Leave any terminal's dispatch
                                // path without changing AppKit's first responder.
                                if let Some(focus) = &browser.navigation {
                                    window.focus(focus, cx);
                                }
                                cx.notify();
                            }
                        }
                    },
                );
                self.views.insert(id, (pane, events));
            }
        }
        let expanded = self.layout.as_ref().and_then(|layout| {
            let model = layout.model();
            model
                .focus_mode()
                .then(|| model.tab().map(|tab| tab.focus.as_str()))
                .flatten()
        });
        for (id, terminal) in &self.terminals {
            if !leaves.iter().any(|leaf| &leaf.id == id)
                || expanded.is_some_and(|focused| focused != id)
            {
                terminal.update(cx, |terminal, cx| terminal.hide(cx));
            }
        }
        for (id, (view, _)) in &self.views {
            view.read(cx).set_visible(
                shown.iter().any(|(shown, _)| shown == id)
                    && expanded.is_none_or(|focused| focused == id),
                cx,
            );
            // Native child views are above GPUI drawing. Hide them while a
            // popover or geometry gesture needs that space or its pointer input.
            view.read(cx).occlude_web(
                self.workspace_menu
                    || self.icon_picker
                    || self.tab_menu.is_some()
                    || self.editing.is_some()
                    || self.chooser.is_some()
                    || self.jump.is_some()
                    || self.resize.is_some()
                    || self.sidebar_drag.is_some()
                    || self.pane_drag.as_ref().is_some_and(|drag| drag.dragging),
                cx,
            );
        }
        let empty: Vec<String> = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().tab())
            .map(|tab| {
                tab.tree
                    .leaves()
                    .into_iter()
                    .filter(|leaf| leaf.empty && leaf.attachment.is_none())
                    .map(|leaf| leaf.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.new_panes.retain(|id, _| empty.contains(id));
        for id in empty {
            if !self.new_panes.contains_key(&id) {
                let socket = self.source.clone().and_then(Result::ok);
                let backbone = self.backbone.clone();
                let pane = id.clone();
                let chooser =
                    cx.new(|cx| new_pane::NewPane::new(pane, socket, backbone, window, cx));
                let events =
                    cx.subscribe_in(&chooser, window, |browser, chooser, event, window, cx| {
                        let pane = chooser.read(cx).pane().to_owned();
                        match event {
                            new_pane::NewPaneEvent::OpenTerminal => {
                                browser.open_terminal_in_empty(&pane, window, cx)
                            }
                            new_pane::NewPaneEvent::Attach(name) => {
                                browser.attach_into(&pane, name, window, cx)
                            }
                            new_pane::NewPaneEvent::OpenView(view) => {
                                browser.open_view(&pane, view.clone(), window, cx);
                            }
                            new_pane::NewPaneEvent::OpenSection(section) => {
                                if browser.open_view(
                                    &pane,
                                    View::of(agent_interface_core::ViewKind::Reports),
                                    window,
                                    cx,
                                ) {
                                    browser.opening_sections.insert(pane.clone(), *section);
                                    browser.section_saving = Some(pane.clone());
                                }
                            }
                        }
                    });
                self.new_panes.insert(id, (chooser, events));
            }
        }
        for (id, (chooser, _)) in &self.new_panes {
            chooser.update(cx, |chooser, cx| {
                chooser.set_visible(expanded.is_none_or(|focused| focused == id), cx);
            });
        }
        if let Some(request) = self.shell_to_open.take()
            && let Some(layout) = &self.layout
            && let Some(tab) = layout.model().tab()
            && (!layout.model().focus_mode() || tab.focus == request.id)
            && let Some(leaf) = tab.tree.leaves().into_iter().find(|leaf| {
                leaf.id == request.id && leaf.runs_terminal() && leaf.attachment.is_none()
            })
            && let Some(terminal) = self.terminals.get(&request.id)
            && let Some(Ok(environment)) = &self.shell_environment
        {
            let environment = environment.clone();
            let leaf = leaf.clone();
            terminal.update(cx, |terminal, cx| {
                terminal.open_shell(leaf, environment, cx)
            });
        }
        let sidebar = self.sidebar(window, cx);
        let content = self.content(window, cx);
        let status = self.status_bar(window, cx);
        let overlay = self.popover(window, cx);
        let t = design::tokens();
        let body = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |_, (), window, _| {
                        window.on_mouse_event(
                            move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                                if phase == gpui::DispatchPhase::Capture {
                                    resize_motion(event, window, cx);
                                }
                            },
                        );
                        window.on_mouse_event(
                            move |event: &gpui::MouseUpEvent, phase, window, cx| {
                                if phase == gpui::DispatchPhase::Capture {
                                    resize_end(event, window, cx);
                                }
                            },
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .bg(design::colors().ground)
            .text_color(design::colors().text)
            .font_family(t.font.ui.clone())
            .text_size(t.text.row.size())
            .when_some(self.navigation.clone(), |el, focus| el.track_focus(&focus))
            .on_action(
                cx.listener(|browser, _: &Copy, window, cx| browser.reader_edit("c", window, cx)),
            )
            .on_action(cx.listener(|browser, _: &SelectAll, window, cx| {
                browser.reader_edit("a", window, cx)
            }))
            .capture_any_mouse_down(cx.listener(|browser, _, _, cx| {
                // A GPUI focus handle alone does not change AppKit's first
                // responder. Native clicks must leave an embedded HTML field.
                for (view, _) in browser.views.values() {
                    view.read(cx).focus_native(cx);
                }
            }))
            .capture_key_down(
                cx.listener(|browser, event: &gpui::KeyDownEvent, window, cx| {
                    if let Some(drag) = &mut browser.pane_drag
                        && drag.dragging
                    {
                        drag.cancel();
                        cx.notify();
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            return;
                        }
                    }
                    if browser.resize.as_ref().is_some_and(|drag| drag.dragging) {
                        browser.resize = None;
                        cx.notify();
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            return;
                        }
                    }
                    if browser.editing.is_some() {
                        return;
                    }
                    let key = &event.keystroke;
                    // The jump field keeps typing and editing; arrows move its
                    // highlight unless an input method is composing. Only a
                    // shortcut that moves focus elsewhere closes Jump first.
                    if let Some(jump) = &browser.jump {
                        let command = shortcuts::command_for(key);
                        let numbered = if cfg!(target_os = "macos") {
                            key.modifiers.platform
                                && !key.modifiers.control
                                && key.key.parse::<u8>().is_ok()
                        } else {
                            shortcuts::numbered(key).is_some()
                        };
                        if command == Some(shortcuts::Command::Jump) {
                            // Falls through: the command closes Jump again.
                        } else if command.is_some() || numbered {
                            browser.close_jump(window, cx);
                        } else {
                            if !key.modifiers.modified() && !jump.composing(cx) {
                                let step = match key.key.as_str() {
                                    "up" => Some(-1),
                                    "down" => Some(1),
                                    _ => None,
                                };
                                if let Some(step) = step {
                                    browser.jump_move(step, cx);
                                    cx.stop_propagation();
                                }
                            }
                            return;
                        }
                    }
                    if let Some(chooser) = &browser.chooser {
                        if key.key == "escape" && !chooser.view.read(cx).composing(cx) {
                            browser.chooser = None;
                            if let Some(focus) = &browser.navigation {
                                window.focus(focus, cx);
                            }
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        if chooser.view.read(cx).composing(cx) {
                            return;
                        }
                    }
                    if (browser.tab_menu.is_some() || browser.workspace_menu || browser.icon_picker)
                        && key.key == "escape"
                    {
                        browser.close_menus();
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    // Escape restores the pane layout before the reader or terminal
                    // handles it. Open dialogs and menus retain their own Escape above.
                    if key.key == "escape"
                        && !key.modifiers.modified()
                        && (browser
                            .navigation
                            .as_ref()
                            .is_some_and(|focus| focus.is_focused(window))
                            || browser
                                .terminals
                                .values()
                                .any(|terminal| terminal.read(cx).has_focus(window, cx)))
                        && let Some(layout) = &mut browser.layout
                        && layout.model().focus_mode()
                    {
                        layout.model_mut().set_focus_mode(false);
                        browser.pane_drag = None;
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    let command = shortcuts::command_for(key);
                    let handled = match command {
                        Some(shortcuts::Command::ClosePane) => {
                            browser.close_pane(window, cx);
                            true
                        }
                        Some(shortcuts::Command::CloseTab) => {
                            browser.close_tab(window, cx);
                            true
                        }
                        Some(shortcuts::Command::Detach) => {
                            browser.detach_focused(window, cx);
                            true
                        }
                        Some(shortcuts::Command::NewWorkspace) => {
                            browser.begin_workspace(window, cx);
                            true
                        }
                        Some(shortcuts::Command::NewTab) => {
                            browser.create(Create::Tab, window, cx);
                            true
                        }
                        Some(shortcuts::Command::SplitRight) => {
                            browser.create(Create::Split(Direction::Right), window, cx);
                            true
                        }
                        Some(shortcuts::Command::SplitBelow) => {
                            browser.create(Create::Split(Direction::Below), window, cx);
                            true
                        }
                        Some(shortcuts::Command::Attach) => {
                            if let Some(pane) = browser
                                .layout
                                .as_ref()
                                .and_then(|layout| layout.model().pane())
                                .map(|pane| pane.id.clone())
                            {
                                browser.open_attach(&pane, window, cx);
                            }
                            true
                        }
                        Some(shortcuts::Command::RenameTab) => {
                            browser.begin_name(true, window, cx);
                            true
                        }
                        Some(shortcuts::Command::RenameWorkspace) => {
                            browser.begin_name(false, window, cx);
                            true
                        }
                        Some(shortcuts::Command::Jump) => {
                            // Open under the panel's jump field.
                            let t = design::tokens();
                            let below = browser.sidebar_top(window)
                                + t.size.workspace_tile
                                + t.size.workspace_block_padding * 2.0
                                + t.space.s8
                                + t.size.jump_field;
                            browser.open_jump(point(px(t.space.s12), px(below)), window, cx);
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        for (view, _) in browser.views.values() {
                            view.read(cx).focus_native(cx);
                        }
                        cx.stop_propagation();
                        return;
                    }
                    if command.is_none()
                        && browser.chooser.is_none()
                        && browser.tab_menu.is_none()
                        && !browser.workspace_menu
                        && !browser.icon_picker
                        && browser
                            .navigation
                            .as_ref()
                            .is_some_and(|focus| focus.is_focused(window))
                        && let Some(pane) = browser
                            .layout
                            .as_ref()
                            .and_then(|layout| layout.model().pane())
                            .filter(|leaf| leaf.empty && leaf.attachment.is_none())
                            .map(|leaf| leaf.id.clone())
                        && let Some((chooser, _)) = browser.new_panes.get(&pane)
                        && chooser.update(cx, |chooser, cx| chooser.key(key, true, cx))
                    {
                        cx.stop_propagation();
                        return;
                    }
                    if command.is_none()
                        && browser
                            .navigation
                            .as_ref()
                            .is_some_and(|focus| focus.is_focused(window))
                        && let Some(pane) = browser
                            .layout
                            .as_ref()
                            .and_then(|layout| layout.model().pane())
                            .map(|leaf| leaf.id.clone())
                        && let Some((view, _)) = browser.views.get(&pane)
                        && view.update(cx, |view, cx| view.key(key, cx))
                    {
                        cx.stop_propagation();
                        return;
                    }
                    let sidebar = command == Some(shortcuts::Command::FocusPanel);
                    let navigated = browser
                        .layout
                        .as_mut()
                        .is_some_and(|layout| navigate_key(layout.model_mut(), key));
                    if sidebar || navigated {
                        // Keyboard navigation needs the same native responder
                        // handoff as a click outside the HTML document.
                        for (view, _) in browser.views.values() {
                            view.read(cx).focus_native(cx);
                        }
                        if let Some(focus) = &browser.navigation {
                            window.focus(focus, cx);
                        }
                        if navigated
                            && let Some(leaf) = browser
                                .layout
                                .as_ref()
                                .and_then(|layout| layout.model().pane())
                        {
                            if let Some(chooser) = browser
                                .chooser
                                .as_ref()
                                .filter(|chooser| chooser.destination.pane == leaf.id)
                            {
                                let focus = chooser.view.read(cx).focus_handle(cx);
                                window.focus(&focus, cx);
                            } else if let Some(terminal) = browser.terminals.get(&leaf.id)
                                && terminal.read(cx).target.matches_leaf(leaf)
                            {
                                terminal
                                    .update(cx, |terminal, cx| terminal.focus_terminal(window, cx));
                            }
                        }
                        browser.close_menus();
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(content),
            )
            .child(status)
            .children(overlay)
            // Last painted means first in the bubble phase. Let every button
            // clear its press in capture before suppressing drag-generated clicks.
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |_, (), window, _| {
                        window.on_mouse_event(
                            move |event: &gpui::MouseUpEvent, phase, window, cx| {
                                if phase.bubble() {
                                    pane_drag_end(event, window, cx);
                                }
                            },
                        );
                    },
                )
                .absolute()
                .size_full(),
            );
        window_controls::frame(body, "Agent Interface", window)
    }
}

/// The workspace, tab and pane showing an agent's session from this window's
/// source: the current tab if it shows one, else the first in saved order.
fn agent_destination(
    model: &agent_interface_core::Layout,
    name: &str,
    same_source: impl Fn(Option<&str>) -> bool,
) -> Option<(String, String, String)> {
    let current = model.tab().map(|tab| tab.id.clone());
    let mut found = Vec::new();
    for space in &model.workspaces {
        for tab in &space.tabs {
            for leaf in tab.tree.leaves() {
                if leaf.attachment.as_ref().is_some_and(|attachment| {
                    attachment.session == name && same_source(attachment.source_socket.as_deref())
                }) {
                    found.push((space.id.clone(), tab.id.clone(), leaf.id.clone()));
                }
            }
        }
    }
    found.sort_by_key(|(_, tab, _)| Some(tab) != current.as_ref());
    found.into_iter().next()
}

// Navigation changes this window's selection only. An unavailable terminal keeps
// focus on navigation; no keys are queued for a future connection.
fn navigate_key(model: &mut agent_interface_core::Layout, key: &gpui::Keystroke) -> bool {
    // Numbered selection stays fixed; other navigation comes from settings.
    if let Some((number, workspace)) = shortcuts::numbered(key) {
        if workspace {
            model.choose_workspace_number(number);
        } else {
            model.choose_tab_number(number);
        }
        return true;
    }
    match shortcuts::command_for(key) {
        Some(shortcuts::Command::NextPane) => {
            model.next_pane(1);
        }
        Some(shortcuts::Command::PreviousPane) => {
            model.next_pane(-1);
        }
        Some(shortcuts::Command::NextTab) => {
            model.next_tab(1);
        }
        Some(shortcuts::Command::PreviousTab) => {
            model.next_tab(-1);
        }
        Some(shortcuts::Command::NextWorkspace) => {
            model.next_workspace(1);
        }
        Some(shortcuts::Command::PreviousWorkspace) => {
            model.next_workspace(-1);
        }
        Some(shortcuts::Command::FocusPane) => {
            if model.pane().is_some() {
                model.set_focus_mode(!model.focus_mode());
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_store::{Document, Error, Store};

    #[test]
    fn saved_reader_survives_tab_navigation_but_not_detach() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_or_create(directory.path()).unwrap();
        let mut model = store.load().unwrap().model().clone();
        model.add_tab(None, "/work".into(), true);
        let tab = model.tab().unwrap().id.clone();
        let id = model.pane().unwrap().id.clone();
        let view = View::of(agent_interface_core::ViewKind::Reports);
        assert!(model.show_view(&tab, &id, view.clone()));
        model.add_tab(None, "/work".into(), true);
        assert_ne!(model.tab().unwrap().id, tab);
        assert!(saved_view(&model, &id, &view));
        model.choose_tab(&tab);
        assert!(model.detach_pane());
        assert!(!saved_view(&model, &id, &view));
    }

    #[test]
    fn close_tab_does_not_remove_the_refresh_fallback() {
        let path = std::path::Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
        let document = Document::parse(raw.into(), path).unwrap();
        let mut model = document.model().clone();
        model.add_tab(Some("Closing".into()), "/tmp".into(), true);
        let mut peer = model.clone();
        peer.add_tab(Some("Survivor".into()), "/tmp".into(), true);
        let survivor = peer.tab().unwrap().id.clone();
        peer.choose_tab(&model.tab().unwrap().id);
        let document = document.with_model(&peer, path).unwrap();
        let mut layout = WindowLayout::new(document.clone());
        let workspace = layout.model().selected.clone();
        let tab = layout.model().tab().unwrap().id.clone();
        assert!(close_tab_proposal(&layout, &workspace, &tab).is_some());

        peer.close_tab();
        let refreshed = layout
            .refresh(document.with_model(&peer, path).unwrap(), path)
            .unwrap();
        assert!(!refreshed.conflict);
        assert!(refreshed.active_tree_changed);
        assert_eq!(layout.model().tab().unwrap().id, survivor);
        assert!(close_tab_proposal(&layout, &workspace, &tab).is_none());
        assert_eq!(layout.model().workspace().unwrap().tabs.len(), 1);
    }

    #[test]
    fn close_pane_preserves_siblings_and_rejects_a_refreshed_target() {
        let directory = tempfile::tempdir().unwrap();
        drop(Store::open_or_create(directory.path()).unwrap());
        let mut store = Store::open_existing_writable(directory.path()).unwrap();
        let mut base = store.load().unwrap();
        let mut model = base.model().clone();
        model.add_tab(None, "/work".into(), true);
        let first = model.pane().unwrap().clone();
        model.split(Direction::Right, "/work".into(), true);
        let closing = model.pane().unwrap().clone();
        assert!(!store.save(&mut base, &model).unwrap().conflict);
        let mut layout = WindowLayout::new(store.load().unwrap());
        let workspace = model.selected.clone();
        let tab = model.tab().unwrap().id.clone();
        let (mut base, proposed) =
            close_pane_proposal(&layout, &workspace, &tab, &closing).unwrap();
        assert_eq!(layout.model().tab().unwrap().tree.leaves().len(), 2);
        let result = store.save(&mut base, &proposed).unwrap();
        layout.saved(base, &result);
        assert_eq!(layout.model().tab().unwrap().id, tab);
        assert_eq!(layout.model().pane(), Some(&first));
        assert_eq!(store.load().unwrap().model().pane(), Some(&first));
        assert!(close_pane_proposal(&layout, &workspace, &tab, &closing).is_none());
        let (_, proposed) = close_pane_proposal(&layout, &workspace, &tab, &first).unwrap();
        assert!(
            proposed.tab().is_none(),
            "closing the final pane closes its tab"
        );
    }

    #[test]
    fn detach_saves_ordinary_terminals_and_views_as_the_same_empty_pane() {
        let directory = tempfile::tempdir().unwrap();
        drop(Store::open_or_create(directory.path()).unwrap());
        let mut store = Store::open_existing_writable(directory.path()).unwrap();
        let base = store.load().unwrap();
        let mut model = base.model().clone();
        model.add_tab(None, "/work".into(), true);
        model.open_terminal();
        let pane = model.pane().unwrap().clone();
        let tab = model.tab().unwrap().id.clone();
        for view in [None, Some(View::of(agent_interface_core::ViewKind::Skills))] {
            if let Some(view) = view {
                assert!(model.detach_pane());
                assert!(model.show_view(&tab, &pane.id, view));
            }
            let mut base = store.load().unwrap();
            assert!(!store.save(&mut base, &model).unwrap().conflict);
            let mut layout = WindowLayout::new(store.load().unwrap());
            let target = Target {
                leaf: layout.model().pane().unwrap().clone(),
                directory: None,
                source: Err("No source socket".into()),
            };
            let (mut base, mut proposed) = layout.save_snapshot();
            assert!(target.detach_from(&mut proposed));
            assert_eq!(layout.model().pane(), Some(&target.leaf));
            let result = store.save(&mut base, &proposed).unwrap();
            assert!(!result.conflict);
            layout.saved(base, &result);
            let saved = store.load().unwrap();
            let empty = saved.model().pane().unwrap();
            assert!(empty.empty && empty.view.is_none() && empty.attachment.is_none());
            assert_eq!(empty.id, pane.id);
            assert_eq!(empty.cwd, pane.cwd);
            assert_eq!(layout.model().pane(), Some(empty));
            assert_eq!(layout.model().tab().unwrap().id, tab);
            assert!(!target.detach_from(layout.model_mut()));
            assert!(layout.model_mut().show_view(
                &tab,
                &pane.id,
                View::of(agent_interface_core::ViewKind::Reports)
            ));
            assert!(!target.detach_from(layout.model_mut()));
        }
    }

    #[test]
    fn edited_close_shortcuts_replace_native_bindings_and_round_trip() {
        use shortcuts::{Bindings, Chord, Command};
        let key = |text| gpui::Keystroke::parse(shortcuts::platform_key(text)).unwrap();
        let mut bindings = Bindings::default();
        assert_eq!(
            bindings.command_for(&key("cmd-w")),
            Some(Command::ClosePane)
        );
        assert_eq!(
            bindings.command_for(&key("cmd-shift-w")),
            Some(Command::CloseTab)
        );
        assert_eq!(
            bindings.command_for(&key("cmd-shift-x")),
            Some(Command::Detach)
        );
        bindings
            .assign(Command::ClosePane, Chord::parse("ctrl-alt-w").unwrap())
            .unwrap();
        bindings
            .assign(
                Command::NewTab,
                Chord::parse(shortcuts::platform_key("cmd-w")).unwrap(),
            )
            .unwrap();
        let changes = bindings.changed();
        let restored =
            Bindings::from_entries(changes.iter().map(|(id, chord)| (*id, chord.as_str())));
        assert_eq!(restored, bindings);
        assert_eq!(restored.command_for(&key("cmd-w")), Some(Command::NewTab));
        let native = application_bindings(&restored);
        assert!(
            native
                .iter()
                .all(|binding| binding.match_keystrokes(&[key("cmd-w")]).is_none())
        );
        assert!(
            native
                .iter()
                .any(|binding| binding.match_keystrokes(&[key("ctrl-alt-w")]) == Some(false))
        );

        // New defaults must not take keys away from previously saved commands.
        let upgraded = Bindings::from_entries([
            ("jump", shortcuts::platform_key("cmd-shift-x")),
            ("new-tab", shortcuts::platform_key("cmd-shift-w")),
        ]);
        assert_eq!(
            upgraded.command_for(&key("cmd-shift-x")),
            Some(Command::Jump)
        );
        assert_eq!(
            upgraded.command_for(&key("cmd-shift-w")),
            Some(Command::NewTab)
        );
        assert_eq!(upgraded.chord(Command::Detach), None);
        assert_eq!(upgraded.chord(Command::CloseTab), None);
        let native = application_bindings(&upgraded);
        assert!(native.iter().all(|binding| {
            binding.match_keystrokes(&[key("cmd-shift-x")]).is_none()
                && binding.match_keystrokes(&[key("cmd-shift-w")]).is_none()
        }));
        let changes = upgraded.changed();
        assert_eq!(
            Bindings::from_entries(changes.iter().map(|(id, chord)| (*id, chord.as_str()))),
            upgraded
        );
    }

    #[test]
    fn detaching_and_closing_a_tab_apply_only_after_saving() {
        let directory = tempfile::tempdir().unwrap();
        drop(Store::open_or_create(directory.path()).unwrap());
        let mut store = Store::open_existing_writable(directory.path()).unwrap();
        let mut base = store.load().unwrap();
        let mut model = base.model().clone();
        model.add_tab(None, "/work".into(), true);
        model.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: Some("/fixture/source".into()),
        }));
        assert!(!store.save(&mut base, &model).unwrap().conflict);
        let mut layout = WindowLayout::new(store.load().unwrap());
        let target = Target {
            leaf: layout.model().pane().unwrap().clone(),
            directory: Some(directory.path().into()),
            source: Ok("/fixture/source".into()),
        };
        let (mut base, mut proposed) = layout.save_snapshot();
        assert!(target.detach_from(&mut proposed));
        assert!(layout.model().pane().unwrap().attachment.is_some());
        let result = store.save(&mut base, &proposed).unwrap();
        layout.saved(base, &result);
        assert!(layout.model().pane().unwrap().empty);
        assert!(store.load().unwrap().model().pane().unwrap().empty);
        let (mut base, mut proposed) = layout.save_snapshot();
        assert!(proposed.close_tab());
        assert!(layout.model().tab().is_some());
        let result = store.save(&mut base, &proposed).unwrap();
        layout.saved(base, &result);
        assert!(layout.model().tab().is_none());
        assert_eq!(layout.model().workspaces.len(), 1);
        assert!(store.load().unwrap().model().tab().is_none());
    }

    #[test]
    fn attachment_guards_empty_displayed_panes_and_keeps_the_original_destination() {
        let mut model = Layout::initial("/tmp".into());
        model.split(Direction::Right, "/tmp".into(), true);
        let pane = model.pane().unwrap().id.clone();
        let edit = AttachmentEdit::capture(&model).unwrap();
        let attachment = Attachment {
            session: "existing worker".into(),
            source_socket: Some("/fixture/source".into()),
        };
        model.next_pane(1);
        assert!(edit.apply(&mut model, attachment.clone()));
        assert_eq!(model.pane().unwrap().id, pane);
        assert_eq!(model.pane().unwrap().cwd.as_deref(), Some("/tmp"));
        assert!(!model.pane().unwrap().empty);

        model.split(Direction::Right, "/tmp".into(), true);
        let empty = AttachmentEdit::capture(&model).unwrap();
        model.open_terminal();
        let before = model.clone();
        assert!(!empty.apply(&mut model, attachment.clone()));
        assert_eq!(
            model, before,
            "peer-filled ordinary shell must not be overwritten"
        );

        model.split(Direction::Right, "/tmp".into(), true);
        let hidden = AttachmentEdit::capture(&model).unwrap();
        model.next_pane(1);
        model.set_focus_mode(true);
        assert!(!hidden.apply(&mut model, attachment.clone()));
        model.set_focus_mode(false);
        assert!(hidden.apply(&mut model, attachment.clone()));

        let menu = AttachmentEdit::capture(&model).unwrap();
        model.split(Direction::Below, "/tmp".into(), true);
        assert!(
            menu.apply(&mut model, attachment),
            "safe refreshed changes to another pane remain valid"
        );
    }

    #[test]
    fn ordinary_shell_proposals_keep_the_empty_pane_until_saved_and_reject_stale_targets() {
        let path = std::path::Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Tab","tree":{"id":"000000000003","agent":null,"empty":true,"cwd":"/tmp"},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let mut window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        let destination = AttachmentEdit::capture(window.model()).unwrap();
        let before = window.model().pane().unwrap().clone();
        let (_, proposed) = destination.propose_shell(&mut window).unwrap();
        assert_eq!(window.model().pane(), Some(&before));
        let ordinary = proposed.pane().unwrap();
        assert!(!ordinary.empty);
        assert!(ordinary.attachment.is_none());
        assert_eq!(ordinary.id, before.id);
        assert_eq!(ordinary.cwd, before.cwd);
        window.model_mut().open_terminal();
        assert!(destination.propose_shell(&mut window).is_none());
        let ordinary = AttachmentEdit::capture(window.model()).unwrap();
        assert!(ordinary.propose_shell(&mut window).is_some());
        window.model_mut().attach(Some(Attachment {
            session: "external".into(),
            source_socket: Some("/fixture/source".into()),
        }));
        assert!(ordinary.propose_shell(&mut window).is_none());
        let external = AttachmentEdit::capture(window.model()).unwrap();
        assert!(external.propose_shell(&mut window).is_none());
        window.model_mut().add_tab(None, "/tmp".into(), true);
        assert!(destination.propose_shell(&mut window).is_none());
    }

    #[test]
    fn an_attachment_proposal_is_displayed_only_after_a_successful_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layouts.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
            .unwrap();
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Tab","tree":{"id":"000000000003","direction":"right","first":{"id":"000000000004","agent":null,"empty":true,"cwd":"/tmp","extension":"keep"},"second":{"id":"000000000005","agent":null,"cwd":"/tmp"}},"focus":"000000000004"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        connection
            .execute("INSERT INTO terminal_layout VALUES (1, ?1)", [raw])
            .unwrap();
        let mut store = Store::open_existing_writable(directory.path()).unwrap();
        let mut window = WindowLayout::new(store.load().unwrap());
        let destination = AttachmentEdit::capture(window.model()).unwrap();
        window.model_mut().next_pane(1);
        assert_ne!(window.model().pane().unwrap().id, destination.pane);
        let (mut base, proposed) = destination
            .propose(
                &mut window,
                Attachment {
                    session: "worker".into(),
                    source_socket: Some("/fixture/source".into()),
                },
            )
            .unwrap();
        assert_eq!(window.model().pane().unwrap().id, destination.pane);
        assert!(
            window.model().pane().unwrap().empty,
            "pre-commit display retains the existing model"
        );
        // An invalid peer document makes saving fail without exposing the proposal.
        connection
            .execute("UPDATE terminal_layout SET value = '{}'", [])
            .unwrap();
        assert!(store.save(&mut base, &proposed).is_err());
        assert!(window.model().pane().unwrap().empty);
        connection
            .execute("UPDATE terminal_layout SET value = ?1", [raw])
            .unwrap();
        let result = store.save(&mut base, &proposed).unwrap();
        assert!(window.model().pane().unwrap().empty);
        assert!(window.saved(base, &result));
        assert_eq!(
            window
                .model()
                .pane()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "worker"
        );
        let saved = store.load().unwrap();
        assert!(saved.raw_json().contains("\"extension\":\"keep\""));
        assert!(!saved.raw_json().contains("\"empty\""));

        // Returning to the captured shell preserves the attachment until commit,
        // even after focus moves, and never redirects to a different pane.
        let menu = TabMenu {
            workspace: window.model().selected.clone(),
            tab: window.model().tab().unwrap().clone(),
            shell: window.model().attachment_target(),
        };
        let attached = window.model().pane().unwrap().clone();
        window.model_mut().next_pane(1);
        let (mut shell_base, shell_model) = menu.shell_proposal(&mut window).unwrap();
        assert_eq!(window.model().pane().unwrap(), &attached);
        assert_eq!(shell_model.pane().unwrap().attachment, None);
        assert_eq!(shell_model.pane().unwrap().id, attached.id);
        assert_eq!(shell_model.pane().unwrap().cwd, attached.cwd);
        let shell_result = store.save(&mut shell_base, &shell_model).unwrap();
        assert_eq!(window.model().pane().unwrap(), &attached);
        assert!(window.saved(shell_base, &shell_result));
        assert_eq!(window.model().pane().unwrap().attachment, None);
        assert!(menu.shell_proposal(&mut window).is_none());
        assert!(
            store
                .load()
                .unwrap()
                .raw_json()
                .contains("\"extension\":\"keep\"")
        );
        let (mut base, mut restored) = window.save_snapshot();
        restored.attach(attached.attachment);
        let result = store.save(&mut base, &restored).unwrap();
        window.saved(base, &result);

        // A later conflict adopts the peer attachment, never the rejected choice.
        let destination = AttachmentEdit::capture(window.model()).unwrap();
        let (mut base, mut proposed) = window.save_snapshot();
        destination.apply(
            &mut proposed,
            Attachment {
                session: "local".into(),
                source_socket: Some("/fixture/source".into()),
            },
        );
        let mut peer = store.load().unwrap();
        let mut peer_model = peer.model().clone();
        peer_model.attach(Some(Attachment {
            session: "peer".into(),
            source_socket: Some("/fixture/source".into()),
        }));
        assert!(!store.save(&mut peer, &peer_model).unwrap().conflict);
        let result = store.save(&mut base, &proposed).unwrap();
        assert!(result.conflict);
        assert_eq!(
            window
                .model()
                .pane()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "worker"
        );
        assert!(window.saved(base, &result));
        assert_eq!(
            window
                .model()
                .pane()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "peer"
        );
    }

    #[test]
    fn tab_options_reject_changed_targets_and_moves_keep_the_terminal_tree() {
        let mut model = agent_interface_core::Layout::initial("/tmp".into());
        model.split(Direction::Right, "/tmp".into(), true);
        let tab = model.tab().unwrap().clone();
        let source = model.selected.clone();
        let menu = TabMenu {
            workspace: source.clone(),
            tab: tab.clone(),
            shell: None,
        };
        assert!(menu.current(&model));
        model.rename_tab("Peer rename");
        assert!(!menu.current(&model));
        model.rename_tab(&tab.name);
        model.add_tab(Some("Other".into()), "/tmp".into(), true);
        assert!(!menu.current(&model));
        model.choose_tab(&tab.id);
        model.move_tab(1);
        assert!(menu.current(&model));
        assert_eq!(model.workspace().unwrap().tabs[1], tab);
        model.add_workspace("Destination".into());
        let destination = model.selected.clone();
        model.choose_workspace(&source);
        assert!(model.transfer_tab(&destination));
        assert_eq!(model.tab(), Some(&tab));
        assert!(!menu.current(&model));
    }

    #[test]
    fn an_agent_goes_to_the_current_tab_showing_it_else_the_first() {
        let mut model = agent_interface_core::Layout::initial("/tmp".into());
        let session = |name: &str, socket: Option<&str>| {
            Some(Attachment {
                session: name.into(),
                source_socket: socket.map(Into::into),
            })
        };
        assert!(model.attach(session("reviewer", None)));
        let first = model.tab().unwrap().id.clone();
        assert!(model.add_tab(None, "/tmp".into(), false));
        assert!(model.attach(session("reviewer", Some("/elsewhere"))));
        let other_source = model.tab().unwrap().id.clone();
        assert!(model.add_tab(None, "/tmp".into(), false));
        assert!(model.attach(session("reviewer", None)));
        let current = model.tab().unwrap().clone();
        let local = |socket: Option<&str>| socket.is_none();
        let found = agent_destination(&model, "reviewer", local).unwrap();
        assert_eq!(found.1, current.id);
        assert_eq!(found.2, current.focus);
        assert!(model.choose_tab(&other_source));
        assert_eq!(
            agent_destination(&model, "reviewer", local).unwrap().1,
            first
        );
        assert!(agent_destination(&model, "absent", local).is_none());
    }

    #[test]
    fn settings_binding_matches_the_platform_event() {
        // Linux XKB reports an unshifted comma with Control unchanged. Avoid
        // Shift-comma: punctuation loses its Shift flag and arrives as `<`.
        let event = gpui::Keystroke {
            key: ",".into(),
            key_char: None,
            modifiers: gpui::Modifiers {
                control: !cfg!(target_os = "macos"),
                platform: cfg!(target_os = "macos"),
                ..Default::default()
            },
        };
        let binding = KeyBinding::new(shortcuts::platform_key("cmd-,"), OpenSettings, None);
        assert_eq!(binding.match_keystrokes(&[event]), Some(false));
    }

    #[test]
    fn native_navigation_cycles_saved_selections_without_editing_the_tree() {
        let mut model = agent_interface_core::Layout::initial("/tmp".into());
        let first = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/tmp".into(), true);
        let second = model.pane().unwrap().id.clone();
        let tree = model.tab().unwrap().tree.clone();
        let key = |value| gpui::Keystroke::parse(value).unwrap();
        assert!(navigate_key(
            &mut model,
            &key(shortcuts::platform_key("cmd-]"))
        ));
        assert_eq!(model.pane().unwrap().id, first);
        assert!(navigate_key(
            &mut model,
            &key(shortcuts::platform_key("cmd-["))
        ));
        assert_eq!(model.pane().unwrap().id, second);
        navigate_key(&mut model, &key(shortcuts::platform_key("cmd-shift-enter")));
        assert!(model.focus_mode());
        assert_eq!(model.tab().unwrap().tree, tree);
        let tab = model.tab().unwrap().id.clone();
        model.add_tab(None, "/tmp".into(), true);
        navigate_key(&mut model, &key(shortcuts::platform_key("cmd-shift-]")));
        assert_eq!(model.tab().unwrap().id, tab);
        let workspace = model.selected.clone();
        model.add_workspace("Other".into());
        navigate_key(&mut model, &key(shortcuts::platform_key("cmd-alt-right")));
        assert_eq!(model.selected, workspace);
        let tab_number = if cfg!(target_os = "macos") {
            "cmd-2"
        } else {
            "alt-2"
        };
        assert!(navigate_key(&mut model, &key(tab_number)));
        assert_eq!(
            model.tab().unwrap().id,
            model.workspace().unwrap().tabs[1].id
        );
        let space_number = if cfg!(target_os = "macos") {
            "cmd-alt-2"
        } else {
            "ctrl-alt-2"
        };
        assert!(navigate_key(&mut model, &key(space_number)));
        assert_eq!(model.selected, model.workspaces[1].id);
        let before = model.clone();
        let missing = if cfg!(target_os = "macos") {
            "cmd-9"
        } else {
            "alt-9"
        };
        assert!(navigate_key(&mut model, &key(missing)));
        assert!(!navigate_key(&mut model, &key("ctrl-]")));
        assert!(!navigate_key(&mut model, &key("cmd-alt-]")));
        assert!(!navigate_key(&mut model, &key("cmd-ctrl-1")));
        assert_eq!(model, before);
    }

    #[test]
    fn agent_state_lookup_omits_offline_and_unavailable_states() {
        let mut browser = Browser::default();
        let mut snapshot = agent_interface_roster::Snapshot::default();
        snapshot.update(br#"{"items":[{"name":"worker","state":"busy"},{"name":"idle","state":""},{"name":"gone","state":"offline"}]}"#);
        browser.agents = Some(snapshot);
        let states = browser.agent_states().unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states["worker"], components::AgentState::Working);
        assert_eq!(
            states["idle"],
            components::AgentState::from_report("unknown")
        );
        browser.agents.as_mut().unwrap().update(b"invalid");
        assert!(browser.agent_states().is_none());
    }

    #[test]
    fn refresh_keeps_local_navigation_and_read_errors_keep_the_last_good_layout() {
        let path = std::path::Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"First","tree":{"id":"000000000003","agent":null},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let base = Document::parse(raw.into(), path).unwrap();
        let mut saved = base.model().clone();
        saved.add_tab(Some("Second".into()), "/tmp".into(), true);
        let second = saved.tab().unwrap().id.clone();
        saved.choose_tab("000000000002");
        let mut browser = Browser::default();
        let mut other = Browser::default();
        for window in [&mut browser, &mut other] {
            window.apply(Update {
                document: Some(base.with_model(&saved, path).unwrap()),
                error: None,
            });
        }
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .choose_tab(&second);
        saved.rename_workspace("Updated workspace");
        browser.apply(Update {
            document: Some(base.with_model(&saved, path).unwrap()),
            error: None,
        });
        let model = browser.layout.as_ref().unwrap().model();
        assert_eq!(model.workspace().unwrap().name, "Updated workspace");
        assert_eq!(model.tab().unwrap().id, second);
        assert_eq!(
            other.layout.as_ref().unwrap().model().tab().unwrap().id,
            "000000000002"
        );

        let before = model.clone();
        browser.apply(Update {
            document: None,
            error: Some(Error {
                path: path.into(),
                detail: "fixture read failure".into(),
            }),
        });
        assert_eq!(browser.layout.as_ref().unwrap().model(), &before);
        assert!(
            browser
                .error
                .as_ref()
                .unwrap()
                .contains("fixture read failure")
        );
        browser.apply(Update {
            document: Some(base.with_model(&saved, path).unwrap()),
            error: None,
        });
        assert!(browser.error.is_none());
        assert_eq!(
            browser.layout.as_ref().unwrap().model().tab().unwrap().id,
            second
        );
    }
}
