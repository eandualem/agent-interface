//! Connect each saved leaf to its actual existing tmux session.

use crate::{
    library,
    panes::Panes,
    sessions,
    view::{Appearance, TerminalView},
};
use futures::channel::mpsc::{self, UnboundedReceiver};
use gpui::{Context, Entity, Render, SharedString, Task, Window, div, prelude::*, rgb};
use std::{path::PathBuf, sync::Arc};
use tmux_workspaces_core::Leaf;
use tmux_workspaces_tmux::connection::SessionWindow;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub leaf: Leaf,
    pub directory: Option<PathBuf>,
    pub source: Result<PathBuf, String>,
}

struct Ready {
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
    connect_selected(target, None)
}

fn connect_selected(target: Target, selected: Option<SessionWindow>) -> Result<Ready, String> {
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
    selected_window: Option<u32>,
    task: Option<Task<()>>,
    closed: Option<gpui::Subscription>,
}

impl LiveTerminal {
    pub fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connecting
            && let Some(view) = &self.view
        {
            view.update(cx, |view, cx| view.focus_terminal(window, cx));
        }
    }

    pub fn new(target: Target, cx: &mut Context<Self>) -> Self {
        let mut terminal = Self {
            target,
            view: None,
            error: None,
            connecting: false,
            windows: Vec::new(),
            selected_window: None,
            task: None,
            closed: None,
        };
        terminal.start(cx);
        terminal
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.start_window(None, cx);
    }

    fn start_window(&mut self, selected: Option<SessionWindow>, cx: &mut Context<Self>) {
        self.closed = None;
        // Keep the old client attached until its replacement is ready. A source
        // with destroy-unattached enabled must not disappear during navigation.
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.suspend_input(cx));
        }
        self.error = None;
        self.connecting = true;
        let target = self.target.clone();
        let connection = cx.background_executor().spawn(async move {
            match selected {
                Some(window) => connect_selected(target, Some(window)),
                None => connect(target),
            }
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |terminal, cx| {
                terminal.connecting = false;
                match result {
                    Ok(mut ready) => {
                        terminal.windows = std::mem::take(&mut ready.windows);
                        terminal.selected_window = Some(ready.window);
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
                            |terminal, _, _: &crate::view::Disconnected, cx| {
                                terminal.error = Some("Session disconnected".into());
                                cx.notify();
                            },
                        ));
                        terminal.view = Some(view);
                    }
                    Err(error) => terminal.error = Some(error),
                }
                cx.notify();
            });
        }));
    }
}

impl Render for LiveTerminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut windows = div()
            .id("tmux-windows")
            .flex()
            .overflow_x_scroll()
            .text_xs();
        for item in &self.windows {
            let selected = item.clone();
            let name: String = item
                .name
                .chars()
                .map(|ch| if ch.is_control() { ' ' } else { ch })
                .collect();
            windows = windows.child(
                div()
                    .id(SharedString::from(format!(
                        "tmux-window-{}-{}",
                        item.id, item.index
                    )))
                    .px_2()
                    .py_1()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .when(self.selected_window == Some(item.id), |el| {
                        el.bg(rgb(0x2a3c58))
                    })
                    .child(format!("{}: {name}", item.index))
                    .when(!self.connecting, |el| {
                        el.on_click(cx.listener(move |terminal, _, _, cx| {
                            terminal.start_window(Some(selected.clone()), cx);
                        }))
                    }),
            );
        }
        windows = windows.child(
            div()
                .id("refresh-windows")
                .px_2()
                .py_1()
                .flex_shrink_0()
                .cursor_pointer()
                .child("Refresh")
                .when(!self.connecting, |el| {
                    el.on_click(cx.listener(|terminal, _, _, cx| {
                        let selected = terminal
                            .windows
                            .iter()
                            .find(|window| Some(window.id) == terminal.selected_window)
                            .cloned();
                        terminal.start_window(selected, cx);
                    }))
                }),
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(windows)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .children((!self.connecting).then(|| self.view.clone()).flatten()),
            )
            .when(self.connecting, |el| el.child("Connecting…"))
            .when_some(self.error.clone(), |el, error| {
                el.child(div().p_2().text_sm().child(error))
            })
            .when(!self.connecting && self.error.is_some(), |el| {
                el.child(
                    div()
                        .id("reconnect")
                        .p_2()
                        .cursor_pointer()
                        .child("Reconnect")
                        .on_click(cx.listener(|terminal, _, _, cx| terminal.start(cx))),
                )
            })
    }
}

fn appearance() -> Appearance {
    Appearance {
        font_family: "Menlo".into(),
        font_size: 14.0,
        foreground: rgb(0xe2e9f3),
        background: rgb(0x101620),
        cursor: rgb(0xe2e9f3),
        palette: [
            0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
            0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
        ]
        .map(rgb),
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

    fn text(screen: &Screen) -> String {
        (0..screen.rows())
            .flat_map(|row| (0..screen.cols()).map(move |col| (row, col)))
            .filter_map(|(row, col)| screen.cell(row, col).map(|(text, _)| text))
            .collect()
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
                attachment: Some(Attachment {
                    session: "window-source".into(),
                    source_socket: Some(socket.trim().into()),
                }),
            },
            directory: None,
            source: Err("use explicit source".into()),
        };
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
        let ready = connect_selected(target.clone(), Some(selected.clone())).unwrap();
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
        assert!(connect_selected(target, Some(selected)).is_err());
        assert_eq!(current(), original);
        assert!(
            server
                .run(&["has-session", "-t", "=window-source:"])
                .is_ok()
        );
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
        queue
            .send((
                retired,
                tmux_workspaces_term::input::Input::Paste("discarded\n".into()),
            ))
            .unwrap();
        queue
            .send((
                ready.input,
                tmux_workspaces_term::input::Input::Paste("surviving-input\n".into()),
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
