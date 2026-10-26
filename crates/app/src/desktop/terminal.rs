//! Connect each saved leaf to its actual existing tmux session.

use super::{components as ui, design};
use crate::{
    library,
    panes::Panes,
    sessions,
    view::{Appearance, TerminalView},
};
use futures::channel::mpsc::{self, UnboundedReceiver};
use gpui::{Context, Entity, Render, Task, Window, div, prelude::*};
use std::{path::PathBuf, sync::Arc};
use tmux_workspaces_core::Leaf;
use tmux_workspaces_tmux::connection::SessionWindow;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub leaf: Leaf,
    pub directory: Option<PathBuf>,
    pub source: Result<PathBuf, String>,
}

impl Target {
    /// Clear only the pane contents the user acted on, in the selected tab.
    pub fn detach_from(&self, layout: &mut tmux_workspaces_core::Layout) -> bool {
        layout.tab().is_some_and(|tab| {
            tab.tree
                .leaves()
                .into_iter()
                .any(|leaf| self.matches_leaf(leaf))
        }) && layout.choose_pane(&self.leaf.id)
            && layout.detach_pane()
    }

    /// A hidden tab still owns its connection until the saved target changes.
    pub fn exists_in(&self, layout: &tmux_workspaces_core::Layout) -> bool {
        layout.workspaces.iter().any(|workspace| {
            workspace.tabs.iter().any(|tab| {
                tab.tree
                    .leaves()
                    .into_iter()
                    .any(|leaf| self.matches_leaf(leaf))
            })
        })
    }

    pub fn matches_leaf(&self, leaf: &Leaf) -> bool {
        self.leaf.id == leaf.id
            && self.leaf.attachment == leaf.attachment
            && self.leaf.empty == leaf.empty
            && self.leaf.view == leaf.view
    }

    pub fn same_session(&self, other: &Self) -> bool {
        self.matches_leaf(&other.leaf)
            && self.directory == other.directory
            && self.source == other.source
    }
}

struct Ready {
    socket: PathBuf,
    panes: Option<Arc<Panes>>,
    layout: Option<tmux_workspaces_tmux::layout::Change>,
    input: tmux_workspaces_tmux::control::PaneId,
    windows: Vec<SessionWindow>,
    window: u32,
    updates: Option<UnboundedReceiver<()>>,
}

impl Drop for Ready {
    fn drop(&mut self) {
        // A completed background task may be cancelled on the foreground
        // executor before installation. It must not join tmux workers there.
        if let Some(panes) = self.panes.take() {
            std::thread::spawn(move || drop(panes));
        }
    }
}

fn connect(target: Target) -> Result<Ready, String> {
    connect_selected(target, None, false)
}

fn connect_selected(
    target: Target,
    selected: Option<SessionWindow>,
    reopened: bool,
) -> Result<Ready, String> {
    let (socket, session) = if let Some(attachment) = &target.leaf.attachment {
        let socket = match &attachment.source_socket {
            Some(path) => {
                sessions::source_socket(Some(path.as_ref())).map_err(|e| e.to_string())?
            }
            None => target.source?,
        };
        (socket, attachment.session.clone())
    } else {
        let directory =
            library::library_directory(target.directory.as_deref()).map_err(|e| e.to_string())?;
        let socket = library::existing_shell_socket(&directory)
            .map_err(|e| e.to_string())?
            .ok_or("This saved shell is not running.")?;
        let session =
            library::ordinary_session(&target.leaf.id).ok_or("Invalid saved shell identity")?;
        (socket, session)
    };
    let connection = match selected {
        Some(window) if reopened => {
            tmux_workspaces_tmux::connection::connect_reopened(&socket, &session, &window)
        }
        Some(window) => {
            tmux_workspaces_tmux::connection::connect_window(&socket, &session, &window)
        }
        None => tmux_workspaces_tmux::connection::connect(&socket, &session),
    }
    .map_err(|e| e.to_string())?;
    let (wake, updates) = mpsc::unbounded();
    let panes = Panes::start_in_session(
        connection.client,
        &connection.layout,
        connection.session,
        10_000,
        move || {
            let _ = wake.unbounded_send(());
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(Ready {
        socket,
        panes: Some(Arc::new(panes)),
        window: connection.layout.window,
        windows: connection.windows,
        layout: Some(connection.layout),
        input: connection.input,
        updates: Some(updates),
    })
}

pub(super) struct LiveTerminal {
    pub target: Target,
    view: Option<Entity<TerminalView>>,
    error: Option<String>,
    connecting: bool,
    windows: Vec<SessionWindow>,
    selected_window: Option<SessionWindow>,
    socket: Option<PathBuf>,
    task: Option<Task<()>>,
    closed: Option<gpui::Subscription>,
    _windows: Task<()>,
}

pub(super) struct Detach;
pub(super) struct OpenTerminal;

impl gpui::EventEmitter<Detach> for LiveTerminal {}
impl gpui::EventEmitter<OpenTerminal> for LiveTerminal {}
impl gpui::EventEmitter<crate::view::Focus> for LiveTerminal {}

impl LiveTerminal {
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.hide(cx));
        }
    }

    /// Whether this leaf's terminal has keyboard focus.
    pub fn has_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.view
            .as_ref()
            .is_some_and(|view| view.read(cx).has_focus(window))
    }

    pub fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connecting
            && let Some(view) = &self.view
        {
            view.update(cx, |view, cx| view.focus_terminal(window, cx));
        }
    }

    pub fn open_shell(
        &mut self,
        leaf: Leaf,
        environment: tmux_workspaces_tmux::shells::Environment,
        cx: &mut Context<Self>,
    ) {
        if !self.target.matches_leaf(&leaf) {
            return;
        }
        self.target.leaf = leaf;
        self.start_window(self.selected_window.clone(), Some(environment), cx);
    }

    pub fn selection(&self) -> Option<SessionWindow> {
        self.selected_window.clone()
    }

    pub fn new(target: Target, selected: Option<SessionWindow>, cx: &mut Context<Self>) -> Self {
        let mut terminal = Self {
            target,
            view: None,
            error: None,
            connecting: false,
            windows: Vec::new(),
            selected_window: selected.clone(),
            socket: None,
            task: None,
            closed: None,
            _windows: cx.spawn(async |this, cx| {
                // tmux shows a session's windows live. Follow additions,
                // removals and renames without a refresh control.
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    let Ok(query) = this.update(cx, |terminal, cx| {
                        if let Some(view) = &terminal.view {
                            view.update(cx, |view, cx| view.refresh_connection(cx));
                        }
                        if !terminal.connecting
                            && terminal.error.is_some()
                            && terminal.target.leaf.attachment.is_some()
                        {
                            // Retry only the saved attachment. This never starts
                            // an agent, tmux server or replacement shell.
                            terminal.start(cx);
                            return None;
                        }
                        (!terminal.connecting)
                            .then(|| {
                                terminal
                                    .socket
                                    .clone()
                                    .zip(terminal.selected_window.clone())
                            })
                            .flatten()
                    }) else {
                        break;
                    };
                    let Some((socket, window)) = query else {
                        continue;
                    };
                    let listed = cx
                        .background_executor()
                        .spawn(async move {
                            tmux_workspaces_tmux::connection::session_windows(&socket, &window)
                        })
                        .await;
                    if let Ok(windows) = listed {
                        let _ = this.update(cx, |terminal, cx| {
                            if !terminal.connecting && terminal.windows != windows {
                                terminal.windows = windows;
                                cx.notify();
                            }
                        });
                    }
                }
            }),
        };
        terminal.start_window(selected, None, cx);
        terminal
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if !self.connecting {
            self.start_window(None, None, cx);
        }
    }

    fn start_window(
        &mut self,
        selected: Option<SessionWindow>,
        environment: Option<tmux_workspaces_tmux::shells::Environment>,
        cx: &mut Context<Self>,
    ) {
        self.closed = None;
        // Keep the old client attached until its replacement is ready. A source
        // with destroy-unattached enabled must not disappear during navigation.
        if let Some(view) = &self.view {
            view.update(cx, |view, _| view.suspend_input());
        }
        // Retain the last output and error while retrying, instead of flashing
        // an empty Connecting panel on every automatic attempt.
        self.connecting = true;
        let target = self.target.clone();
        let connection = cx.background_executor().spawn(async move {
            let reopened = environment.is_some();
            if let Some(environment) = environment {
                if !target.leaf.runs_terminal() || target.leaf.attachment.is_some() {
                    return Err("Only an ordinary terminal can be opened.".into());
                }
                let directory = library::library_directory(target.directory.as_deref())
                    .map_err(|error| error.to_string())?;
                let socket = library::shell_socket_for_creation(&directory)
                    .map_err(|error| error.to_string())?;
                tmux_workspaces_tmux::shells::ensure(
                    &socket,
                    &[tmux_workspaces_tmux::shells::Request {
                        id: target.leaf.id.clone(),
                        cwd: target.leaf.cwd.as_ref().map(PathBuf::from),
                    }],
                    &environment,
                )
                .map_err(|error| error.to_string())?;
            }
            match selected {
                Some(window) => connect_selected(target, Some(window), reopened),
                None => connect(target),
            }
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |terminal, cx| {
                terminal.connecting = false;
                match result {
                    Ok(mut ready) => {
                        terminal.error = None;
                        terminal.socket = Some(ready.socket.clone());
                        terminal.windows = std::mem::take(&mut ready.windows);
                        terminal.selected_window = terminal
                            .windows
                            .iter()
                            .find(|window| window.id == ready.window)
                            .cloned();
                        let view = cx.new(|cx| {
                            TerminalView::embedded(
                                ready.panes.take().unwrap(),
                                ready.layout.take().unwrap().panes,
                                ready.input,
                                appearance(),
                                ready.updates.take().unwrap(),
                                cx,
                            )
                        });
                        terminal.closed = Some(cx.subscribe(
                            &view,
                            |terminal, _, event: &crate::view::Disconnected, cx| {
                                terminal.error = Some(
                                    event
                                        .0
                                        .clone()
                                        .unwrap_or_else(|| "Session disconnected".into()),
                                );
                                cx.notify();
                            },
                        ));
                        cx.subscribe(&view, |_, _, _: &crate::view::Focus, cx| {
                            cx.emit(crate::view::Focus);
                        })
                        .detach();
                        terminal.view = Some(view);
                    }
                    Err(error) => terminal.error = Some(error),
                }
                cx.notify();
            });
        }));
    }
}

impl LiveTerminal {
    /// The session's windows and which one is shown, for the pane header.
    pub fn windows_for_header(&self) -> Vec<(SessionWindow, bool)> {
        self.windows
            .iter()
            .map(|window| {
                let current = self
                    .selected_window
                    .as_ref()
                    .is_some_and(|selected| selected.id == window.id);
                (window.clone(), current)
            })
            .collect()
    }

    /// Show another of the session's windows.
    pub fn select_window(&mut self, window: SessionWindow, cx: &mut Context<Self>) {
        if !self.connecting {
            self.start_window(Some(window), None, cx);
        }
    }
}

impl Render for LiveTerminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Settings changes apply to terminals that are already connected.
        let current = appearance();
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.set_appearance(&current, cx));
        }
        let (t, c) = (design::tokens(), design::colors());
        let line = |text: String, color: gpui::Rgba| {
            ui::text(
                div()
                    .px(gpui::px(t.space.s12))
                    .py(gpui::px(t.space.s8))
                    .text_color(color),
                &t.text.body,
            )
            .font_family(t.font.ui.clone())
            .child(text)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(c.surface)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .children(
                        (!self.connecting || self.error.is_some())
                            .then(|| self.view.clone())
                            .flatten(),
                    ),
            )
            .when(self.connecting && self.error.is_none(), |el| {
                el.child(line("Connecting…".into(), c.muted))
            })
            .when_some(self.error.clone(), |el, error| {
                el.child(line(error, c.attention_text))
            })
            .when(self.error.is_some(), |el| {
                let attached = self.target.leaf.attachment.is_some();
                el.child(line(
                    if attached {
                        "Waiting for this session. Reconnecting automatically."
                    } else {
                        "Cannot reach this terminal’s saved shell. Open terminal can start it if it is no longer running."
                    }
                    .into(),
                    c.muted,
                ))
                .child(
                    div()
                        .flex()
                        .gap(gpui::px(t.space.s8))
                        .px(gpui::px(t.space.s12))
                        .pb(gpui::px(t.space.s8))
                        .child(
                            ui::button("reconnect", "Reconnect", ui::Button::Secondary)
                                .on_click(cx.listener(|terminal, _, _, cx| terminal.start(cx))),
                        )
                        .when(!attached, |el| {
                            el.child(
                                ui::button("open-terminal", "Open terminal", ui::Button::Secondary)
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenTerminal))),
                            )
                        })
                        .when(attached, |el| {
                            el.child(
                                ui::button("detach", "Detach", ui::Button::Secondary)
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Detach))),
                            )
                        }),
                )
            })
    }
}

/// The terminal font and colours from the current settings and design tokens.
pub(super) fn appearance() -> Appearance {
    let (tokens, terminal) = (design::tokens(), design::terminal());
    Appearance {
        font_family: tokens.font.mono.clone().into(),
        font_size: super::settings::font_size(),
        foreground: terminal.text,
        background: terminal.background,
        cursor: terminal.cursor,
        palette: terminal.palette,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        time::{Duration, Instant},
    };
    use tmux_workspaces_core::Attachment;
    use tmux_workspaces_term::Screen;
    use tmux_workspaces_test_support::TmuxServer;

    #[test]
    fn attachment_survives_navigation_and_detach_returns_the_same_pane_to_its_chooser() {
        let mut model = tmux_workspaces_core::Layout::initial("/work".into());
        model.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: Some("/fixture/source".into()),
        }));
        let target = Target {
            leaf: model.pane().unwrap().clone(),
            directory: None,
            source: Ok("/fixture/source".into()),
        };
        let workspace = model.selected.clone();
        let tab = model.tab().unwrap().id.clone();
        model.add_tab(None, "/other".into(), true);
        assert!(target.exists_in(&model));
        assert!(!target.detach_from(&mut model));
        model.add_workspace("Other".into());
        assert!(target.exists_in(&model));
        model.choose_workspace(&workspace);
        model.choose_tab(&tab);
        model.split(
            tmux_workspaces_core::Direction::Right,
            "/other".into(),
            true,
        );
        model.focus = true;
        assert!(target.exists_in(&model));
        model.focus = false;
        assert!(target.detach_from(&mut model));
        let pane = model.pane().unwrap();
        assert!(pane.empty && pane.attachment.is_none());
        assert_eq!(pane.id, target.leaf.id);
        assert_eq!(pane.cwd, target.leaf.cwd);
        assert!(!target.exists_in(&model));
        assert!(model.fill_pane(
            &tab,
            &target.leaf.id,
            Some(Attachment {
                session: "replacement".into(),
                source_socket: Some("/fixture/source".into()),
            })
        ));
        assert!(!target.detach_from(&mut model));
        assert_eq!(
            model.pane().unwrap().attachment.as_ref().unwrap().session,
            "replacement"
        );
    }

    fn text(screen: &Screen) -> String {
        (0..screen.rows())
            .flat_map(|row| (0..screen.cols()).map(move |col| (row, col)))
            .filter_map(|(row, col)| screen.cell(row, col).map(|(text, _)| text))
            .collect()
    }

    #[test]
    fn retry_waits_for_the_exact_saved_session_without_creating_it() {
        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let target = Target {
            leaf: Leaf {
                id: "123456abcdef".into(),
                cwd: None,
                empty: false,
                view: None,
                attachment: Some(Attachment {
                    session: "returning".into(),
                    source_socket: Some(socket.trim().into()),
                }),
            },
            directory: None,
            source: Err("use the saved source".into()),
        };
        server
            .run(&["new-session", "-d", "-s", "returning-extra", "cat"])
            .unwrap();
        let original = server
            .run(&["list-sessions", "-F", "#{session_name}"])
            .unwrap();
        for _ in 0..2 {
            assert!(connect(target.clone()).is_err());
            assert_eq!(
                server
                    .run(&["list-sessions", "-F", "#{session_name}"])
                    .unwrap(),
                original
            );
            assert!(server.run(&["list-clients"]).unwrap().is_empty());
        }
        server
            .run(&["new-session", "-d", "-s", "returning", "cat"])
            .unwrap();
        let first = connect(target.clone()).unwrap();
        server.run(&["kill-session", "-t", "=returning"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !first.panes.as_ref().unwrap().is_finished() {
            first.panes.as_ref().unwrap().take_update();
            assert!(
                Instant::now() < deadline,
                "closed session did not disconnect"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(connect(target.clone()).is_err());
        server
            .run(&["new-session", "-d", "-s", "returning", "cat"])
            .unwrap();
        let returned = connect(target).unwrap();
        assert!(!returned.panes.as_ref().unwrap().is_finished());
    }

    #[test]
    fn local_window_selection_streams_and_types_without_switching_the_source() {
        let server = TmuxServer::start().unwrap();
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                "window-source",
                "-x",
                "80",
                "-y",
                "24",
                "cat",
            ])
            .unwrap();
        let second = server
            .run(&[
                "new-window",
                "-d",
                "-t",
                "=window-source:",
                "-n",
                "second",
                "-P",
                "-F",
                "#{window_id}",
                "sh",
                "-c",
                "printf 'second-window-ready\\n'; exec cat",
            ])
            .unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let current = || {
            server
                .run(&[
                    "display-message",
                    "-p",
                    "-t",
                    "=window-source:",
                    "#{window_id}",
                ])
                .unwrap()
        };
        let original = current();
        let target = Target {
            leaf: Leaf {
                id: "123456abcdef".into(),
                cwd: None,
                empty: false,
                view: None,
                attachment: Some(Attachment {
                    session: "window-source".into(),
                    source_socket: Some(socket.trim().into()),
                }),
            },
            directory: None,
            source: Err("use explicit source".into()),
        };
        let mut refreshed = target.clone();
        refreshed.leaf.cwd = Some("/updated/saved/directory".into());
        assert!(target.same_session(&refreshed));
        refreshed.leaf.id = "ffffffffffff".into();
        assert!(!target.same_session(&refreshed));
        let first = connect(target.clone()).unwrap();
        let selected = first
            .windows
            .iter()
            .find(|window| format!("@{}", window.id) == second.trim())
            .unwrap()
            .clone();
        server
            .run(&[
                "set-option",
                "-t",
                "=window-source:",
                "destroy-unattached",
                "on",
            ])
            .unwrap();
        let ready = connect_selected(target.clone(), Some(selected.clone()), false).unwrap();
        drop(first);
        assert_eq!(ready.window, selected.id);
        let panes = ready.panes.as_ref().unwrap();
        let wait = |marker: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(update) = panes.take_update()
                    && update
                        .screens
                        .iter()
                        .any(|(_, screen)| text(screen).contains(marker))
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "missing {marker}: {:?}",
                    panes.failure()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        wait("second-window-ready");
        panes
            .send_input(ready.input, b"selected-window-input\r")
            .unwrap();
        wait("selected-window-input");
        assert_eq!(current(), original);
        let original_text = server
            .run(&["capture-pane", "-p", "-t", "=window-source:"])
            .unwrap();
        assert!(!original_text.contains("selected-window-input"));
        // Restore the fixture's ordinary lifetime before testing removal.
        server
            .run(&[
                "set-option",
                "-t",
                "=window-source:",
                "destroy-unattached",
                "off",
            ])
            .unwrap();
        let moved = tmux_workspaces_tmux::connection::connect_window(
            std::path::Path::new(socket.trim()),
            "window-source",
            &selected,
        )
        .unwrap();
        // Move after discovery/attachment, before the attached capture starts.
        server
            .run(&["move-window", "-s", second.trim(), "-t", "=fixture:1"])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while panes.failure().is_none() {
            assert!(Instant::now() < deadline, "removed window stayed available");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(panes.send_input(ready.input, b"must-not-send").is_err());
        drop(ready);
        assert!(
            Panes::start_in_session(moved.client, &moved.layout, moved.session, 100, || {})
                .is_err()
        );
        assert!(connect_selected(target, Some(selected), false).is_err());
        assert_eq!(current(), original);
        assert!(
            server
                .run(&["has-session", "-t", "=window-source:"])
                .is_ok()
        );
    }

    #[test]
    fn a_declared_size_resizes_the_session_like_an_ordinary_client() {
        let server = TmuxServer::start().unwrap();
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                "sized",
                "-x",
                "100",
                "-y",
                "30",
                "cat",
            ])
            .unwrap();
        server
            .run(&["new-window", "-d", "-t", "=sized:", "cat"])
            .unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let sizes = || {
            server
                .run(&[
                    "list-windows",
                    "-t",
                    "=sized:",
                    "-F",
                    "#{window_width}x#{window_height}",
                ])
                .unwrap()
        };
        let target = Target {
            leaf: Leaf {
                id: "abcdef123456".into(),
                cwd: None,
                empty: false,
                view: None,
                attachment: Some(Attachment {
                    session: "sized".into(),
                    source_socket: Some(socket.trim().into()),
                }),
            },
            directory: None,
            source: Err("use explicit source".into()),
        };
        let ready = connect(target.clone()).unwrap();
        // Connecting alone leaves the session's geometry alone.
        assert_eq!(sizes().trim(), "100x30\n100x30");
        let panes = ready.panes.as_ref().unwrap();
        panes.set_size(60, 20).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(update) = panes.take_update()
                && update
                    .layout
                    .iter()
                    .any(|pane| pane.cols == 60 && pane.rows == 20)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "layout not resized: {:?}",
                panes.failure()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // As with the source's nested clients, every window of the session
        // follows the client under tmux's default window-size policy.
        assert_eq!(sizes().trim(), "60x20\n60x20");
        server
            .run(&["set-window-option", "-g", "window-size", "smallest"])
            .unwrap();
        let wide = connect(target).unwrap();
        wide.panes.as_ref().unwrap().set_size(80, 25).unwrap();
        let wait_size = |expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let actual = sizes();
                if actual.trim() == expected {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "size did not reach {expected:?}; actual={actual:?}; narrow={:?}; wide={:?}; clients={:?}",
                    panes.failure(),
                    wide.panes.as_ref().unwrap().failure(),
                    server.run(&[
                        "list-clients",
                        "-F",
                        "#{client_name}:#{client_width}x#{client_height}:#{client_flags}",
                    ])
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // Finish the wide client's resize before hiding the narrow client,
        // so that resize cannot incidentally apply the later flag change.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !server
            .run(&["list-clients", "-F", "#{client_width}"])
            .unwrap()
            .lines()
            .any(|width| width == "80")
        {
            assert!(Instant::now() < deadline, "wide client did not resize");
            std::thread::sleep(Duration::from_millis(5));
        }
        panes.ignore_size().unwrap();
        wait_size("80x25\n80x25");
        assert_eq!(server.run(&["list-clients"]).unwrap().lines().count(), 2);
        // Revealing the same-sized narrow view restores sizing participation.
        panes.set_size(60, 20).unwrap();
        wait_size("60x20\n60x20");
        assert_eq!(server.run(&["list-clients"]).unwrap().lines().count(), 2);
        drop(wide);
        let deadline = Instant::now() + Duration::from_secs(10);
        while server.run(&["list-clients"]).unwrap().lines().count() != 1 {
            assert!(Instant::now() < deadline, "wide client did not detach");
            std::thread::sleep(Duration::from_millis(5));
        }
        // With no other client, ignoring size retains the last declaration.
        panes.ignore_size().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !server
            .run(&["list-clients", "-F", "#{client_flags}"])
            .unwrap()
            .contains("ignore-size")
        {
            assert!(Instant::now() < deadline, "last client still sizes windows");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(sizes().trim(), "60x20\n60x20");
        drop(ready);
        assert!(server.run(&["has-session", "-t", "=sized:"]).is_ok());
    }

    #[test]
    fn saved_attachment_displays_existing_output_accepts_input_and_detaches_cleanly() {
        let server = TmuxServer::start().unwrap();
        let session = "=editor with spaces";
        server
            .run(&[
                "new-session",
                "-d",
                "-s",
                session,
                "-x",
                "100",
                "-y",
                "30",
                "sh",
                "-c",
                "printf 'existing-output\\n'; exec cat",
            ])
            .unwrap();
        let target_name = format!("={session}:");
        server
            .run(&[
                "split-window",
                "-h",
                "-t",
                &target_name,
                "sh",
                "-c",
                "printf 'second-pane\\n'; exec cat",
            ])
            .unwrap();
        let socket = PathBuf::from(
            server
                .run(&["display-message", "-p", "#{socket_path}"])
                .unwrap()
                .trim(),
        );
        let before = server
            .run(&[
                "list-panes",
                "-t",
                &target_name,
                "-F",
                "#{pane_id} #{pane_width} #{pane_height}",
            ])
            .unwrap();
        let environment = server
            .run(&["show-environment", "-t", &target_name])
            .unwrap();
        let mut target = Target {
            leaf: Leaf {
                id: "123456abcdef".into(),
                cwd: None,
                empty: false,
                view: None,
                attachment: Some(Attachment {
                    session: session.into(),
                    source_socket: Some(socket.to_str().unwrap().into()),
                }),
            },
            directory: None,
            source: Err("must use saved explicit socket".into()),
        };
        let ready = connect(target.clone()).unwrap();
        let panes = ready.panes.as_ref().unwrap();
        let mut screens = HashMap::new();
        let wait = |screens: &mut HashMap<_, Screen>, expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(update) = panes.take_update() {
                    screens.extend(update.screens);
                }
                if screens
                    .values()
                    .any(|screen| text(screen).contains(expected))
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "missing {expected}: {:?}",
                    panes.failure()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        wait(&mut screens, "existing-output");
        wait(&mut screens, "second-pane");
        assert_eq!(screens.len(), 2);
        panes.send_input(ready.input, b"typed-marker\r").unwrap();
        wait(&mut screens, "typed-marker");
        assert!(text(&screens[&ready.input]).contains("typed-marker"));
        assert!(
            screens
                .iter()
                .filter(|(id, _)| **id != ready.input)
                .all(|(_, screen)| !text(screen).contains("typed-marker"))
        );
        assert_eq!(
            server
                .run(&[
                    "list-panes",
                    "-t",
                    &target_name,
                    "-F",
                    "#{pane_id} #{pane_width} #{pane_height}"
                ])
                .unwrap(),
            before
        );
        assert_eq!(
            server
                .run(&["show-environment", "-t", &target_name])
                .unwrap(),
            environment
        );
        // A stale queued key for a retired pane must not disable its neighbor.
        let retired = *screens.keys().find(|id| **id != ready.input).unwrap();
        server
            .run(&["kill-pane", "-t", &format!("%{}", retired.0)])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(update) = panes.take_update()
                && !update.layout.iter().any(|pane| pane.id == retired)
            {
                break;
            }
            assert!(Instant::now() < deadline, "pane removal was not observed");
            std::thread::sleep(Duration::from_millis(5));
        }
        while panes.send_input(retired, b"discarded").is_ok() {
            assert!(Instant::now() < deadline, "retired pane was not rejected");
            std::thread::sleep(Duration::from_millis(5));
        }
        let queue = crate::view::input_queue(Arc::clone(panes), |_| {});
        // Single-line input also works when older tmux omits paste-mode state.
        queue
            .try_send((
                retired,
                tmux_workspaces_term::input::Input::Paste("discarded".into()),
            ))
            .unwrap();
        queue
            .try_send((
                ready.input,
                tmux_workspaces_term::input::Input::Paste("surviving-input".into()),
            ))
            .unwrap();
        wait(&mut screens, "surviving-input");
        drop(queue);
        drop(ready);
        assert!(server.run(&["has-session", "-t", &target_name]).is_ok());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !server.run(&["list-clients"]).unwrap().is_empty() {
            assert!(Instant::now() < deadline, "connection did not detach");
            std::thread::sleep(Duration::from_millis(5));
        }
        // No session-name prefix matching and no implicit server creation.
        target.leaf.attachment.as_mut().unwrap().session = "=editor".into();
        assert!(connect(target.clone()).is_err());
        let missing = server.address().0.join("missing.sock");
        target.leaf.attachment.as_mut().unwrap().source_socket =
            Some(missing.to_str().unwrap().into());
        assert!(connect(target).is_err());
        assert!(!missing.exists());
    }
}
