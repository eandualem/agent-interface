//! Connect each saved leaf to its actual existing tmux session.

use crate::{
    library,
    panes::Panes,
    sessions,
    view::{Appearance, TerminalView},
};
use futures::channel::mpsc::{self, UnboundedReceiver};
use gpui::{Context, Entity, Render, Task, Window, div, prelude::*, rgb};
use std::{path::PathBuf, sync::Arc};
use tmux_workspaces_core::Leaf;

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
    let connection =
        tmux_workspaces_tmux::connection::connect(&socket, &session).map_err(|e| e.to_string())?;
    let (wake, updates) = mpsc::unbounded();
    let panes = Panes::start(connection.client, &connection.layout, 10_000, move || {
        let _ = wake.unbounded_send(());
    })
    .map_err(|e| e.to_string())?;
    Ok(Ready {
        panes: Some(Arc::new(panes)),
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
    task: Option<Task<()>>,
    closed: Option<gpui::Subscription>,
}

impl LiveTerminal {
    pub fn new(target: Target, cx: &mut Context<Self>) -> Self {
        let mut terminal = Self {
            target,
            view: None,
            error: None,
            connecting: false,
            task: None,
            closed: None,
        };
        terminal.start(cx);
        terminal
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.closed = None;
        self.view = None;
        self.error = None;
        self.connecting = true;
        let target = self.target.clone();
        let connection = cx
            .background_executor()
            .spawn(async move { connect(target) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |terminal, cx| {
                terminal.connecting = false;
                match result {
                    Ok(mut ready) => {
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(div().flex_1().min_h_0().children(self.view.clone()))
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
        let queue = crate::view::input_queue(Arc::clone(panes));
        queue.send((retired, b"discarded\r".to_vec())).unwrap();
        queue
            .send((ready.input, b"surviving-input\r".to_vec()))
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
