//! Connect each saved leaf to its actual existing tmux session.

#[cfg(test)]
mod profile_isolation_tests;

use super::{components as ui, design};
use crate::{
    library,
    panes::Panes,
    sessions,
    view::{Appearance, TerminalView},
};
use agent_interface_core::Leaf;
use agent_interface_roster::Restart;
use agent_interface_tmux::connection::SessionWindow;
use futures::channel::mpsc::{self, UnboundedReceiver};
use gpui::{Context, Entity, Render, Task, Window, div, prelude::*};
use std::{path::PathBuf, sync::Arc};

static CONNECTION_ATTEMPTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Clone)]
pub(super) struct Target {
    pub connection: Option<agent_interface_roster::profiles::Connection>,
    pub leaf: Leaf,
    pub directory: Option<PathBuf>,
    pub source: Result<PathBuf, String>,
}

impl Target {
    fn profile_matches(&self, space: &agent_interface_core::Workspace) -> bool {
        space.profile_id.as_deref()
            == self
                .connection
                .as_ref()
                .map(|c| c.identity().profile_id.as_str())
    }
    /// A provider's sign-in session. It does not come back once it ends.
    /// A session of that name on another tmux server is a person's own.
    pub fn is_sign_in(&self) -> bool {
        self.connection.as_ref().is_some_and(|connection| {
            self.leaf.attachment.as_ref().is_some_and(|attachment| {
                attachment.session.starts_with("sign-in-")
                    && attachment
                        .source_socket
                        .as_deref()
                        .is_none_or(|path| std::path::Path::new(path) == connection.tmux_socket())
            })
        })
    }

    /// Remove this pane from whichever workspace of its profile holds it,
    /// keeping the workspace and tab in view.
    pub fn close_from(&self, layout: &mut agent_interface_core::Layout) -> bool {
        let Some((space, tab)) = layout
            .workspaces
            .iter()
            .filter(|space| self.profile_matches(space))
            .find_map(|space| {
                space
                    .tabs
                    .iter()
                    .find(|tab| {
                        tab.tree
                            .leaves()
                            .into_iter()
                            .any(|leaf| self.matches_leaf(leaf))
                    })
                    .map(|tab| (space.id.clone(), tab.id.clone()))
            })
        else {
            return false;
        };
        let shown_space = layout.workspace().map(|space| space.id.clone());
        if !layout.choose_workspace(&space) {
            return false;
        }
        let shown_tab = layout.tab().map(|tab| tab.id.clone());
        let closed =
            layout.choose_tab(&tab) && layout.choose_pane(&self.leaf.id) && layout.close_pane();
        if let Some(shown) = shown_tab {
            layout.choose_tab(&shown);
        }
        if let Some(shown) = shown_space {
            layout.choose_workspace(&shown);
        }
        closed
    }

    /// Clear only the pane contents the user acted on, in the selected tab.
    pub fn detach_from(&self, layout: &mut agent_interface_core::Layout) -> bool {
        layout
            .workspace()
            .is_some_and(|space| self.profile_matches(space))
            && layout.tab().is_some_and(|tab| {
                tab.tree
                    .leaves()
                    .into_iter()
                    .any(|leaf| self.matches_leaf(leaf))
            })
            && layout.choose_pane(&self.leaf.id)
            && layout.detach_pane()
    }

    /// A hidden tab still owns its connection until the saved target changes.
    pub fn exists_in(&self, layout: &agent_interface_core::Layout) -> bool {
        layout.workspaces.iter().any(|workspace| {
            self.profile_matches(workspace)
                && workspace.tabs.iter().any(|tab| {
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
            && self.connection.as_ref().map(|c| c.connection_key())
                == other.connection.as_ref().map(|c| c.connection_key())
    }
}

struct Ready {
    socket: PathBuf,
    panes: Option<Arc<Panes>>,
    layout: Option<agent_interface_tmux::layout::Change>,
    input: agent_interface_tmux::control::PaneId,
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

fn shell_refusal(error: agent_interface_roster::profiles::Error) -> String {
    use agent_interface_roster::profiles::Error;
    match error {
        Error::Refused(422) => "This folder is not available to the profile.",
        Error::Refused(503) => "The profile could not start this shell.",
        Error::Refused(_) => "The profile refused this shell.",
        _ => "The workspace profile connection is unavailable.",
    }
    .to_owned()
}

/// "/" is what a new bound pane saves as a placeholder; the provider starts that
/// shell in the profile's home.
fn bound_shell_folder(leaf: &Leaf) -> Option<&str> {
    leaf.cwd.as_deref().filter(|cwd| *cwd != "/")
}

const PROFILE_UNAVAILABLE: &str = "The workspace profile connection is unavailable.";
const PROFILE_CHANGED: &str = "The workspace profile connection changed while attaching.";

fn connect(target: Target) -> Result<Ready, String> {
    connect_selected(target, None, false)
}

fn connect_selected(
    target: Target,
    selected: Option<SessionWindow>,
    reopened: bool,
) -> Result<Ready, String> {
    let bound = target.connection.clone();
    if let Some(connection) = &bound {
        connection
            .verify()
            .map_err(|_| PROFILE_UNAVAILABLE.to_owned())?;
    }
    let (socket, session) = if let Some(connection) = &bound {
        let session = match &target.leaf.attachment {
            Some(attachment) => {
                if attachment
                    .source_socket
                    .as_deref()
                    .is_some_and(|path| std::path::Path::new(path) != connection.tmux_socket())
                {
                    return Err("The saved session belongs to a different profile socket.".into());
                }
                attachment.session.clone()
            }
            // The provider creates the shell; the app never starts a bound server.
            None => connection
                .ensure_shell(&target.leaf.id, bound_shell_folder(&target.leaf))
                .map_err(shell_refusal)?,
        };
        (connection.tmux_socket().to_owned(), session)
    } else if let Some(attachment) = &target.leaf.attachment {
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
        let socket = library::existing_shell_socket(&directory, &target.leaf.id)
            .map_err(|e| {
                tracing::debug!(event = "shell_socket_lookup_failed", error_kind = ?e.kind(), os_error = e.raw_os_error(), "Ordinary shell socket lookup failed");
                e.to_string()
            })?
            .ok_or_else(|| {
                tracing::debug!(event = "shell_socket_absent", "Ordinary shell socket is absent");
                "This saved shell is not running."
            })?;
        let session =
            library::ordinary_session(&target.leaf.id).ok_or("Invalid saved shell identity")?;
        (socket, session)
    };
    let connection = match selected {
        Some(window) if reopened => {
            agent_interface_tmux::connection::connect_reopened(&socket, &session, &window)
        }
        Some(window) => {
            agent_interface_tmux::connection::connect_window(&socket, &session, &window)
        }
        None => agent_interface_tmux::connection::connect(&socket, &session),
    }
    .map_err(|e| e.to_string())?;
    let (wake, updates) = mpsc::unbounded();
    let panes = Panes::start_in_session(
        connection.client,
        &socket,
        &connection.layout,
        connection.session,
        10_000,
        move || {
            let _ = wake.unbounded_send(());
        },
    )
    .map_err(|e| e.to_string())?;
    let ready = Ready {
        socket,
        panes: Some(Arc::new(panes)),
        window: connection.layout.window,
        windows: connection.windows,
        layout: Some(connection.layout),
        input: connection.input,
        updates: Some(updates),
    };
    if let Some(connection) = &bound {
        connection
            .verify()
            .map_err(|_| PROFILE_CHANGED.to_owned())?;
    }
    Ok(ready)
}

/// How often a lost attachment is tried again, in two-second refresh ticks:
/// at once, then half as often after each failure, up to a limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Retry {
    every: u32,
    left: u32,
}

impl Default for Retry {
    fn default() -> Self {
        Self { every: 1, left: 0 }
    }
}

impl Retry {
    /// Thirty seconds while shown, two minutes while hidden.
    const fn limit(shown: bool) -> u32 {
        if shown { 15 } else { 60 }
    }

    /// Count one tick; when an attempt is due, schedule the one after it.
    fn due(&mut self, shown: bool) -> bool {
        if self.left > 0 {
            self.left -= 1;
            return false;
        }
        self.left = self.every - 1;
        self.every = (self.every * 2).min(Self::limit(shown));
        true
    }
}

/// What a pane shows of its agent's own restart.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RestartShown {
    /// The old session is still attached: the terminal, with a small mark.
    Pending,
    /// The session ended: said in place of the stopped pane, without the
    /// launch buttons or the connection's error.
    Restarting { resume: bool },
    /// It failed: the stopped pane, with the service's reason.
    Failed(String),
}

/// What a pane does next for its agent's restart.
#[derive(Debug, PartialEq, Eq)]
enum RestartStep {
    Nothing,
    /// Try the saved attachment again at the next tick.
    Attach,
    /// Read the agent's inspection once to learn how this restart ended.
    Inspect(u64),
}

/// What became of a restart after it left the list.
#[derive(Clone, Debug, Default, PartialEq)]
enum Left {
    #[default]
    Nothing,
    /// The agent's inspection is being read.
    Asking(Restart),
    /// Its new session is up; `online` once the list says so.
    Ready { restart: Restart, online: bool },
}

/// A pane's view of its agent's own restart: from the roster's list, and
/// one inspection when the restart leaves the list while the agent is
/// offline. No timer assumes anything.
#[derive(Clone, Debug, Default, PartialEq)]
struct Restarts {
    /// The unfinished restart the list carries now.
    listed: Option<Restart>,
    left: Left,
    /// Why the last one failed, as the service gave it.
    failed: Option<String>,
}

impl Restarts {
    /// Follow the list; `offline` is whether it says the agent is offline.
    fn list(&mut self, listed: Option<Restart>, offline: bool) -> RestartStep {
        match (listed, self.listed.take()) {
            (Some(listed), _) => {
                self.listed = Some(listed);
                self.left = Left::Nothing;
                self.failed = None;
                RestartStep::Nothing
            }
            // Online again: its new session is there to attach.
            (None, Some(restart)) if !offline => {
                self.left = Left::Ready {
                    restart,
                    online: true,
                };
                RestartStep::Attach
            }
            (None, Some(restart)) => match restart.id {
                Some(id) => {
                    self.left = Left::Asking(restart);
                    RestartStep::Inspect(id)
                }
                None => RestartStep::Nothing,
            },
            (None, None) => {
                // A new session that ends before the pane reaches it is a stop.
                if let Left::Ready { online, .. } = &mut self.left {
                    if !offline {
                        *online = true;
                    } else if *online {
                        self.left = Left::Nothing;
                    }
                }
                RestartStep::Nothing
            }
        }
    }

    /// How restart `id` ended, from the agent's inspection; none when it
    /// could not be read.
    fn inspected(&mut self, id: u64, answer: Option<&Restart>) -> RestartStep {
        let Left::Asking(restart) = &self.left else {
            return RestartStep::Nothing;
        };
        if restart.id != Some(id) {
            return RestartStep::Nothing;
        }
        let restart = restart.clone();
        self.left = Left::Nothing;
        // A later restart than the one that left says nothing of it.
        let answer = answer.filter(|answer| answer.id == Some(id));
        match answer.and_then(|answer| answer.phase.as_deref()) {
            Some("ready") => {
                self.left = Left::Ready {
                    restart,
                    online: false,
                };
                RestartStep::Attach
            }
            Some("failed") => {
                self.failed = answer
                    .and_then(|answer| answer.result.as_ref())
                    .and_then(|result| result.reason.clone());
                RestartStep::Nothing
            }
            _ => RestartStep::Nothing,
        }
    }

    /// The pane attached a session.
    fn connected(&mut self) {
        self.left = Left::Nothing;
        self.failed = None;
    }

    /// What to show, given whether a session is attached and shown.
    fn shown(&self, attached: bool) -> Option<RestartShown> {
        if attached {
            return self.listed.as_ref().map(|_| RestartShown::Pending);
        }
        let left = match &self.left {
            Left::Asking(restart) | Left::Ready { restart, .. } => Some(restart),
            Left::Nothing => None,
        };
        match self.listed.as_ref().or(left) {
            Some(restart) => Some(RestartShown::Restarting {
                resume: restart.resume == Some(true),
            }),
            None => self.failed.clone().map(RestartShown::Failed),
        }
    }
}

/// The agent's latest restart, from one read of its inspection; none when it
/// cannot be read.
fn latest_restart(
    connection: agent_interface_roster::profiles::Connection,
    name: &str,
) -> Option<Restart> {
    agent_interface_roster::profiles::runtime_context::inspect(connection, name, None)
        .ok()
        .and_then(|inspection| inspection.agent.restart)
}

/// What a pane whose session is not shown says under its last output.
#[derive(Debug, PartialEq, Eq)]
struct Ended {
    /// The connection's failure, in the attention colour.
    error: Option<String>,
    message: String,
    /// Why its agent's restart failed, whole.
    reason: Option<String>,
    /// Whether it offers Resume and Start new.
    launch: bool,
}

/// `agent` names the pane's stopped agent; `saved` is whether the pane has a
/// saved attachment.
fn ended(
    error: &str,
    agent: Option<&str>,
    saved: bool,
    sign_in: bool,
    restart: Option<&RestartShown>,
) -> Ended {
    let restarting = match restart {
        Some(RestartShown::Restarting { resume }) => Some(*resume),
        _ => None,
    };
    let message = match (restarting, agent, saved) {
        (Some(false), ..) => "Restarting into a fresh session…".into(),
        (Some(true), ..) => "Restarting and resuming its conversation…".into(),
        (None, Some(name), _) => format!("{name} is not running."),
        (None, None, true) if sign_in => "This sign-in has ended.".into(),
        (None, None, true) => "Waiting for this session. Reconnecting automatically.".into(),
        (None, None, false) => "Could not start this terminal’s shell.".into(),
    };
    Ended {
        // A stopped agent is not an error, nor is a restart's old session ending.
        error: (agent.is_none() && !sign_in && restarting.is_none()).then(|| error.into()),
        message,
        reason: match restart {
            Some(RestartShown::Failed(reason)) if agent.is_some() => Some(reason.clone()),
            _ => None,
        },
        launch: agent.is_some() && restarting.is_none(),
    }
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
    /// Starts this ordinary terminal's shell when it is no longer running.
    environment: Option<agent_interface_tmux::shells::Environment>,
    restoring: bool,
    /// The name of the stopped agent this attachment shows. A bound pane
    /// starts it through its connection.
    agent: Option<String>,
    /// Its agent's own restart, as the roster and one inspection tell it.
    restarts: Restarts,
    launching: bool,
    launch_result: Option<agent_interface_roster::lifecycle::Outcome>,
    /// Whether the browser shows this terminal. A hidden one keeps its
    /// connection but does not follow its session's window list.
    shown: bool,
    /// Refresh ticks between attempts to reach a lost attachment, and those
    /// left before the next one.
    retry: Retry,
    /// Whether panes are moving: a view connected meanwhile is held too.
    hold_size: bool,
    /// Whether the session's windows may differ from `windows`.
    windows_stale: bool,
    _windows: Task<()>,
}

pub(super) struct Detach;
/// The sign-in session this pane showed is over.
pub(super) struct SignInEnded;
pub(super) struct OpenTerminal;
/// The terminal's session is attached and can take the keyboard.
pub(super) struct Connected;

impl gpui::EventEmitter<Detach> for LiveTerminal {}
impl gpui::EventEmitter<SignInEnded> for LiveTerminal {}
impl gpui::EventEmitter<OpenTerminal> for LiveTerminal {}
impl gpui::EventEmitter<Connected> for LiveTerminal {}
impl gpui::EventEmitter<crate::view::Focus> for LiveTerminal {}

impl LiveTerminal {
    /// Follow the saved directory, so an automatic shell restart uses it.
    pub fn set_cwd(&mut self, leaf: &Leaf) {
        if self.target.matches_leaf(leaf) {
            self.target.leaf.cwd = leaf.cwd.clone();
        }
    }

    /// Only the browser knows whether an attachment is a registered agent.
    pub fn set_agent(&mut self, agent: Option<String>, cx: &mut Context<Self>) {
        if self.agent != agent {
            self.agent = agent;
            self.launch_result = None;
            self.retry = Retry::default();
            cx.notify();
        }
    }

    /// The unfinished restart the roster lists for this pane's agent, in
    /// any state; call after [`Self::set_agent`].
    pub fn set_restart(&mut self, restart: Option<Restart>, cx: &mut Context<Self>) {
        let before = self.restarts.clone();
        let step = self.restarts.list(restart, self.agent.is_some());
        self.restart_step(step, cx);
        if self.restarts != before {
            cx.notify();
        }
    }

    fn restart_step(&mut self, step: RestartStep, cx: &mut Context<Self>) {
        match step {
            RestartStep::Nothing => {}
            RestartStep::Attach => self.retry = Retry::default(),
            RestartStep::Inspect(id) => {
                let (Some(connection), Some(attachment)) =
                    (self.target.connection.clone(), &self.target.leaf.attachment)
                else {
                    let step = self.restarts.inspected(id, None);
                    self.restart_step(step, cx);
                    return;
                };
                let name = attachment.session.clone();
                let work = cx
                    .background_executor()
                    .spawn(async move { latest_restart(connection, &name) });
                cx.spawn(async move |this, cx| {
                    let answer = work.await;
                    let _ = this.update(cx, |terminal, cx| {
                        let step = terminal.restarts.inspected(id, answer.as_ref());
                        terminal.restart_step(step, cx);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// Ask Backbone once to resume or start this pane's agent. The saved
    /// attachment then connects through the automatic retry.
    fn launch_agent(&mut self, resume: bool, cx: &mut Context<Self>) {
        let Some(agent) = self.agent.clone().filter(|_| !self.launching) else {
            return;
        };
        let Some(connection) = self.target.connection.clone() else {
            return;
        };
        self.launching = true;
        self.launch_result = None;
        let name = agent.clone();
        let work = cx
            .background_executor()
            .spawn(async move { connection.start_agent(&name, resume) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |terminal, cx| {
                terminal.launching = false;
                if terminal.agent == Some(agent) {
                    terminal.launch_result = Some(result);
                    terminal.start(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.shown = false;
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.hide(cx));
        }
    }

    /// Whether [`Self::show`] has anything to do.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// The browser shows this terminal again: follow the window list, and
    /// try a lost attachment at the next refresh.
    pub fn show(&mut self) {
        self.shown = true;
        self.retry = Retry::default();
    }

    /// Whether this leaf's terminal has keyboard focus.
    pub fn has_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.view
            .as_ref()
            .is_some_and(|view| view.read(cx).has_focus(window))
    }

    /// Keep the session's size while this pane moves; see
    /// [`TerminalView::hold_size`].
    pub fn hold_size(&mut self, hold: bool, cx: &mut Context<Self>) {
        self.hold_size = hold;
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.hold_size(hold, cx));
        }
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
        environment: agent_interface_tmux::shells::Environment,
        cx: &mut Context<Self>,
    ) {
        if !self.target.matches_leaf(&leaf) {
            return;
        }
        self.target.leaf = leaf;
        self.start_window(self.selected_window.clone(), Some(environment), false, cx);
    }

    pub fn selection(&self) -> Option<SessionWindow> {
        self.selected_window.clone()
    }

    pub fn new(
        target: Target,
        selected: Option<SessionWindow>,
        environment: Option<agent_interface_tmux::shells::Environment>,
        cx: &mut Context<Self>,
    ) -> Self {
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
            environment,
            restoring: false,
            agent: None,
            restarts: Restarts::default(),
            launching: false,
            launch_result: None,
            shown: true,
            retry: Retry::default(),
            hold_size: false,
            windows_stale: false,
            _windows: cx.spawn(async |this, cx| {
                // tmux shows a session's windows live. Follow additions,
                // removals and renames without a refresh control: list them
                // when the client reports a change, or has gone.
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    let Ok(query) = this.update(cx, |terminal, cx| {
                        terminal.windows_stale |= terminal.view.as_ref().is_none_or(|view| {
                            view.update(cx, |view, cx| {
                                view.refresh_connection(cx);
                                view.windows_changed()
                            })
                        });
                        (terminal.windows_stale && !terminal.connecting && terminal.shown)
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
                    let listed = if let Some((socket, window)) = query {
                        Some(
                            cx.background_executor()
                                .spawn(async move {
                                    let windows = agent_interface_tmux::connection::session_windows(
                                        &socket, &window,
                                    );
                                    (socket, window, windows)
                                })
                                .await,
                        )
                    } else {
                        None
                    };
                    let _ = this.update(cx, |terminal, cx| {
                        if !terminal.connecting {
                            // Refresh even after a disconnect, so a removed
                            // selected window cannot hide the remaining choices.
                            if let Some((socket, window, Ok(windows))) = listed
                                && terminal.socket.as_ref() == Some(&socket)
                                && terminal.selected_window.as_ref() == Some(&window)
                            {
                                terminal.windows_stale = false;
                                if terminal.windows != windows {
                                    terminal.windows = windows;
                                    cx.notify();
                                }
                            }
                            if terminal.error.is_some()
                                && terminal.target.leaf.attachment.is_some()
                                && terminal.retry.due(terminal.shown)
                            {
                                // Retry only the saved attachment. This never
                                // starts an agent, server or replacement shell.
                                terminal.start(cx);
                            }
                        }
                    });
                }
            }),
        };
        terminal.start_window(selected, None, false, cx);
        terminal
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if !self.connecting {
            self.start_window(self.selected_window.clone(), None, true, cx);
        }
    }

    fn start_window(
        &mut self,
        selected: Option<SessionWindow>,
        environment: Option<agent_interface_tmux::shells::Environment>,
        reconnect: bool,
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
        let opened_shell = environment.is_some();
        let connection = cx.background_executor().spawn(async move {
            let attempt = CONNECTION_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let span = tracing::debug_span!("terminal_connect", attempt, pane_id = %target.leaf.id, attached = target.leaf.attachment.is_some(), explicit_open = environment.is_some());
            let _entered = span.enter();
            let started = std::time::Instant::now();
            tracing::debug!(event = "terminal_connect_started", "Connecting terminal");
            let result = (|| {
                let reopened = reconnect || environment.is_some();
                if let Some(environment) = environment {
                    if !target.leaf.runs_terminal() || target.leaf.attachment.is_some() {
                        return Err("Only an ordinary terminal can be opened.".into());
                    }
                    let directory = library::library_directory(target.directory.as_deref())
                        .map_err(|error| error.to_string())?;
                    let socket = library::shell_socket_for_creation(&directory, &target.leaf.id)
                        .map_err(|error| error.to_string())?;
                    agent_interface_tmux::shells::ensure(
                        &socket,
                        &[agent_interface_tmux::shells::Request {
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
            })();
            match &result {
                Ok(ready) => tracing::debug!(event = "terminal_connected", attempt, window_id = ready.window, "Terminal connected"),
                Err(_) => tracing::debug!(event = "terminal_connect_failed", attempt, "Terminal connection failed"),
            }
            tracing::trace!(event = "terminal_connect_finished", attempt, elapsed_ms = started.elapsed().as_millis() as u64, success = result.is_ok(), "Terminal connection finished");
            result
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = connection.await;
            let _ = this.update(cx, |terminal, cx| {
                terminal.connecting = false;
                match result {
                    Ok(mut ready) => {
                        terminal.retry = Retry::default();
                        terminal.error = None;
                        terminal.restarts.connected();
                        terminal.restoring = false;
                        terminal.launch_result = None;
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
                        if terminal.hold_size {
                            view.update(cx, |view, cx| view.hold_size(true, cx));
                        }
                        terminal.closed = Some(cx.subscribe(
                            &view,
                            |terminal, _, event: &crate::view::Disconnected, cx| {
                                tracing::warn!(event = "terminal_disconnected", pane_id = %terminal.target.leaf.id, attached = terminal.target.leaf.attachment.is_some(), has_failure = event.0.is_some(), "Terminal connection ended");
                                terminal.error = Some(
                                    event
                                        .0
                                        .clone()
                                        .unwrap_or_else(|| "Session disconnected".into()),
                                );
                                // A shell lost while shown, such as when its server
                                // stops, gets the same one automatic restart.
                                if terminal.target.leaf.attachment.is_none() {
                                    terminal.start(cx);
                                }
                                if terminal.target.is_sign_in() {
                                    cx.emit(SignInEnded);
                                }
                                cx.notify();
                            },
                        ));
                        cx.subscribe(&view, |_, _, _: &crate::view::Focus, cx| {
                            cx.emit(crate::view::Focus);
                        })
                        .detach();
                        terminal.view = Some(view);
                        cx.emit(Connected);
                    }
                    Err(error) => {
                        // An ordinary shell that is no longer running, such as
                        // after a restart, starts again once in its saved folder.
                        if !terminal.restoring
                            && !opened_shell
                            && terminal.target.leaf.attachment.is_none()
                            && let Some(environment) = terminal.environment.clone()
                        {
                            terminal.restoring = true;
                            terminal.start_window(
                                terminal.selected_window.clone(),
                                Some(environment),
                                false,
                                cx,
                            );
                        } else {
                            terminal.restoring = false;
                            // Only tmux can say the session is gone; a profile
                            // connection that failed to verify may come back.
                            let session_gone =
                                error != PROFILE_UNAVAILABLE && error != PROFILE_CHANGED;
                            terminal.error = Some(error);
                            if session_gone && terminal.target.is_sign_in() {
                                cx.emit(SignInEnded);
                            }
                        }
                    }
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
            self.start_window(Some(window), None, false, cx);
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
        let restart = self
            .restarts
            .shown(self.error.is_none() && self.view.is_some());
        let stopped = self.error.as_deref().map(|error| {
            ended(
                error,
                self.agent.as_deref(),
                self.target.leaf.attachment.is_some(),
                self.target.is_sign_in(),
                restart.as_ref(),
            )
        });
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(c.surface)
            // Over the terminal, so its size does not change.
            .when(restart == Some(RestartShown::Pending), |el| {
                el.child(
                    div()
                        .absolute()
                        .top(gpui::px(t.space.s8))
                        .right(gpui::px(t.space.s12))
                        .child(ui::tag("Restart pending", ui::Tone::Plain)),
                )
            })
            .child(
                div().flex_1().min_h_0().children(
                    (!self.connecting || self.error.is_some())
                        .then(|| self.view.clone())
                        .flatten(),
                ),
            )
            .when(self.connecting && self.error.is_none(), |el| {
                el.child(line("Connecting…".into(), c.muted))
            })
            // A stopped agent is not an error; say so and offer its launch.
            .when_some(stopped, |el, ended| {
                let attached = self.target.leaf.attachment.is_some();
                let launching = self.launching;
                el.children(ended.error.map(|error| line(error, c.attention_text)))
                    .child(line(ended.message, c.muted))
                    // Shown whole, wrapping, as the service wrote it.
                    .children(ended.reason.map(|reason| line(reason, c.muted)))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(gpui::px(t.space.s8))
                            .px(gpui::px(t.space.s12))
                            .pb(gpui::px(t.space.s8))
                            .when(ended.launch, |el| {
                                el.child(
                                    ui::button("resume-agent", "Resume", ui::Button::Primary)
                                        .when(launching, |el| el.opacity(0.4))
                                        .on_click(cx.listener(|terminal, _, _, cx| {
                                            terminal.launch_agent(true, cx)
                                        })),
                                )
                                .child(
                                    ui::button("start-agent", "Start new", ui::Button::Secondary)
                                        .when(launching, |el| el.opacity(0.4))
                                        .on_click(cx.listener(|terminal, _, _, cx| {
                                            terminal.launch_agent(false, cx)
                                        })),
                                )
                            })
                            .when(!attached, |el| {
                                el.child(
                                    ui::button(
                                        "open-terminal",
                                        "Open terminal",
                                        ui::Button::Secondary,
                                    )
                                    .on_click(cx.listener(
                                        |terminal, _, _, cx| {
                                            // The provider starts a bound shell; connect asks it again.
                                            if terminal.target.connection.is_some() {
                                                terminal.start(cx);
                                            } else {
                                                cx.emit(OpenTerminal);
                                            }
                                        },
                                    )),
                                )
                            })
                            .when(attached, |el| {
                                el.child(
                                    ui::button("detach", "Detach", ui::Button::Secondary)
                                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Detach))),
                                )
                            }),
                    )
                    .when(launching, |el| {
                        el.child(line("Asking Backbone…".into(), c.muted))
                    })
                    .when_some(self.launch_result.as_ref(), |el, result| {
                        el.child(line(result.message(), c.muted))
                    })
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
        selection: terminal.selection,
        selection_text: terminal.selection_text,
        palette: terminal.palette,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_attachment_is_retried_less_often_until_it_is_shown_again() {
        let mut retry = Retry::default();
        let attempts = |retry: &mut Retry, ticks: u32, shown: bool| {
            (0..ticks).filter(|_| retry.due(shown)).count()
        };
        // Ticks 0, 1, 3, 7 and 15, then every 15 ticks.
        assert_eq!(attempts(&mut retry, 16, true), 5);
        assert_eq!(attempts(&mut retry, 30, true), 2);
        // Hidden, the gap keeps growing to 60 ticks.
        assert_eq!(attempts(&mut retry, 200, false), 5);
        assert_eq!(attempts(&mut Retry::default(), 1, false), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an installed Linux font and graphical session; run in graphical CI"]
    fn linux_terminal_font_has_uniform_ascii_cells() {
        let app = gpui_platform::application();
        let text = app.text_system();
        let appearance = appearance();
        assert!(
            text.all_font_names()
                .contains(&appearance.font_family.to_string())
        );
        let font = gpui::font(appearance.font_family.clone());
        let id = text.resolve_font(&font);
        let cell = crate::view::cell_size(&text, &appearance);
        for ch in ' '..='~' {
            let advance = text
                .advance(id, gpui::px(appearance.font_size), ch)
                .unwrap();
            assert!(
                (f32::from(advance.width - cell.width)).abs() < 0.01,
                "{ch:?}: {advance:?}, cell {cell:?}"
            );
        }
        println!(
            "Linux terminal font {}: ASCII cell {:?}",
            appearance.font_family, cell
        );
    }
    use agent_interface_core::Attachment;
    use agent_interface_term::Screen;
    use agent_interface_test_support::TmuxServer;
    use std::{
        collections::HashMap,
        time::{Duration, Instant},
    };

    #[test]
    fn bound_attachment_ignores_the_window_source_and_refuses_foreign_targets() {
        use crate::roster::content::tests::{Reply, provider_with_tmux};
        use serde_json::json;
        let a = TmuxServer::start().unwrap();
        let b = TmuxServer::start().unwrap();
        for server in [&a, &b] {
            server
                .run(&["new-session", "-d", "-s", "same-name", "cat"])
                .unwrap();
        }
        let socket: PathBuf = a
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim()
            .into();
        let other: PathBuf = b
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim()
            .into();
        let identity = json!({"protocol_version":1,"profile_id":"profile-a","instance_id":"instance-a","machine":{"id":"machine-a"}});
        let (_dir, connection, server) = provider_with_tmux(
            "a",
            (0..3)
                .map(|_| Reply::ok("/api/profile", identity.clone()))
                .collect(),
            Some(socket.clone()),
        );
        let mut model = agent_interface_core::Layout::initial("/tmp".into());
        model.workspaces[0].profile_id = Some("profile-a".into());
        model.attach(Some(Attachment {
            session: "same-name".into(),
            source_socket: None,
        }));
        let mut target = Target {
            connection: Some(connection),
            leaf: model.pane().unwrap().clone(),
            directory: None,
            source: Ok(other.clone()),
        };
        assert!(target.exists_in(&model));
        let ready = connect(target.clone()).unwrap();
        assert_eq!(ready.socket, socket);
        assert!(b.run(&["list-clients"]).unwrap().is_empty());
        drop(ready);
        target.leaf.attachment.as_mut().unwrap().source_socket =
            Some(other.to_str().unwrap().into());
        assert!(
            connect(target.clone())
                .err()
                .expect("attachment must fail")
                .contains("different profile socket")
        );
        assert!(b.run(&["list-clients"]).unwrap().is_empty());
        server.join().unwrap();
    }

    #[test]
    fn changed_binding_during_attachment_releases_the_owned_client() {
        use crate::roster::content::tests::{Reply, provider_with_tmux};
        use serde_json::json;
        let tmux = TmuxServer::start().unwrap();
        let socket: PathBuf = tmux
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap()
            .trim()
            .into();
        let identity = json!({"protocol_version":1,"profile_id":"profile-a","instance_id":"instance-a","machine":{"id":"machine-a"}});
        let (_dir, connection, server) = provider_with_tmux(
            "a",
            vec![
                Reply::ok("/api/profile", identity),
                Reply {
                    status: 409,
                    ..Reply::ok(
                        "/api/profile",
                        json!({"detail":{"code":"binding_mismatch"}}),
                    )
                },
            ],
            Some(socket),
        );
        let mut model = agent_interface_core::Layout::initial("/tmp".into());
        model.attach(Some(Attachment {
            session: "fixture".into(),
            source_socket: None,
        }));
        let target = Target {
            connection: Some(connection),
            leaf: model.pane().unwrap().clone(),
            directory: None,
            source: Err("no window source".into()),
        };
        assert!(
            connect(target)
                .err()
                .expect("attachment must fail")
                .contains("changed while attaching")
        );
        server.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !tmux.run(&["list-clients"]).unwrap().is_empty() {
            assert!(Instant::now() < deadline, "bound client was not released");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn attachment_survives_navigation_and_detach_returns_the_same_pane_to_its_chooser() {
        let mut model = agent_interface_core::Layout::initial("/work".into());
        model.attach(Some(Attachment {
            session: "worker".into(),
            source_socket: Some("/fixture/source".into()),
        }));
        let target = Target {
            connection: None,
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
            agent_interface_core::Direction::Right,
            "/other".into(),
            true,
        );
        model.set_focus_mode(true);
        assert!(target.exists_in(&model));
        model.set_focus_mode(false);
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
    fn a_finished_pane_closes_from_a_hidden_workspace_and_leaves_the_shown_one() {
        let mut layout = agent_interface_core::Layout::initial("/".into());
        layout.add_workspace("Work".into());
        layout.add_tab(Some("kept".into()), "/".into(), true);
        layout.add_tab(Some("sign-in-codex".into()), "/".into(), true);
        layout.attach(Some(Attachment {
            session: "sign-in-codex".into(),
            source_socket: None,
        }));
        let leaf = (*layout.pane().unwrap()).clone();
        let finished = layout.tab().unwrap().id.clone();
        let target = Target {
            connection: None,
            leaf,
            directory: None,
            source: Err("unused".into()),
        };
        let kept = layout.workspace().unwrap().tabs[0].id.clone();
        layout.choose_tab(&kept);
        let work = layout.workspace().unwrap().id.clone();
        layout.add_workspace("Elsewhere".into());
        let elsewhere = layout.workspace().unwrap().id.clone();
        assert_ne!(work, elsewhere);

        assert!(target.close_from(&mut layout));

        assert_eq!(layout.workspace().unwrap().id, elsewhere);
        layout.choose_workspace(&work);
        assert_eq!(layout.tab().unwrap().id, kept);
        assert!(
            layout
                .workspace()
                .unwrap()
                .tabs
                .iter()
                .all(|tab| tab.id != finished)
        );
        assert!(!target.close_from(&mut layout));
    }

    #[test]
    fn retry_waits_for_the_exact_saved_session_without_creating_it() {
        let server = TmuxServer::start().unwrap();
        let socket = server
            .run(&["display-message", "-p", "#{socket_path}"])
            .unwrap();
        let target = Target {
            connection: None,
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
        server
            .run(&["new-window", "-d", "-t", "=returning:", "cat"])
            .unwrap();
        let first = connect(target.clone()).unwrap();
        let selected = first
            .windows
            .iter()
            .find(|window| window.id != first.window)
            .unwrap()
            .clone();
        let retained = connect_selected(target.clone(), Some(selected.clone()), true).unwrap();
        assert_eq!(retained.window, selected.id);
        server
            .run(&["kill-window", "-t", &format!("@{}", selected.id)])
            .unwrap();
        assert!(connect_selected(target.clone(), Some(selected.clone()), true).is_err());
        let remaining = agent_interface_tmux::connection::session_windows(
            PathBuf::from(socket.trim()).as_path(),
            &selected,
        )
        .unwrap();
        assert_eq!(remaining.len(), 1);
        let recovered =
            connect_selected(target.clone(), Some(remaining[0].clone()), false).unwrap();
        assert_eq!(recovered.window, remaining[0].id);
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
        assert!(connect_selected(target.clone(), Some(selected.clone()), true).is_err());
        server
            .run(&["new-session", "-d", "-s", "returning", "cat"])
            .unwrap();
        let returned = connect_selected(target, Some(selected.clone()), true).unwrap();
        assert_ne!(returned.window, selected.id);
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
            connection: None,
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
        let moved = agent_interface_tmux::connection::connect_window(
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
            Panes::start_in_session(
                moved.client,
                std::path::Path::new(socket.trim()),
                &moved.layout,
                moved.session,
                100,
                || {}
            )
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
            connection: None,
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
            connection: None,
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
                agent_interface_term::input::Input::Paste("discarded".into()),
            ))
            .unwrap();
        queue
            .try_send((
                ready.input,
                agent_interface_term::input::Input::Paste("surviving-input".into()),
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

    /// The service's restart frames, in order, as its fixtures give them.
    fn frames(name: &str) -> Vec<Restart> {
        let path = format!(
            "{}/../roster/src/profiles/runs/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let frames: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        frames
            .into_iter()
            .map(|frame| serde_json::from_value(frame["data"].clone()).unwrap())
            .collect()
    }

    /// The list's item for a frame: carried only while the restart is under way.
    fn listed(frame: &Restart) -> Option<Restart> {
        let under_way = !matches!(frame.phase.as_deref(), Some("ready" | "stopped" | "failed"));
        under_way.then(|| Restart {
            result: None,
            ..frame.clone()
        })
    }

    const PIPE: &str = "writing to control client: Broken pipe (os error 32)";

    /// What a bound agent's pane says once its session ended.
    fn stopped(restarts: &Restarts, offline: bool) -> Ended {
        let agent = offline.then_some("builder");
        ended(PIPE, agent, true, false, restarts.shown(false).as_ref())
    }

    #[test]
    fn a_fresh_restart_reads_as_restarting_from_start_to_attach() {
        let frames = frames("stream-agent-restart-fresh");
        let mut restarts = Restarts::default();
        // The old session is attached until the restart stops it.
        assert_eq!(
            restarts.list(listed(&frames[0]), false),
            RestartStep::Nothing
        );
        assert_eq!(restarts.shown(true), Some(RestartShown::Pending));
        // It ended: whether the roster still says online or already offline,
        // the pane says it restarts, with no error and no launch buttons.
        for frame in &frames[1..4] {
            for offline in [false, true] {
                assert_eq!(restarts.list(listed(frame), offline), RestartStep::Nothing);
                let shown = stopped(&restarts, offline);
                assert_eq!(
                    shown,
                    Ended {
                        error: None,
                        message: "Restarting into a fresh session…".into(),
                        reason: None,
                        launch: false,
                    },
                    "{:?}",
                    frame.phase
                );
            }
        }
        // Ready: the list drops it before the new session can be attached.
        // The pane tries the attachment at once and still says it restarts.
        assert_eq!(
            restarts.list(listed(&frames[4]), false),
            RestartStep::Attach
        );
        assert_eq!(
            stopped(&restarts, false).message,
            "Restarting into a fresh session…"
        );
        assert_eq!(restarts.list(None, false), RestartStep::Nothing);
        assert!(!stopped(&restarts, false).launch);
        restarts.connected();
        assert_eq!(restarts.shown(true), None);
        assert_eq!(restarts.shown(false), None);
    }

    #[test]
    fn a_ready_restart_read_while_the_roster_says_offline_still_attaches() {
        let frames = frames("stream-agent-restart-fresh");
        let mut restarts = Restarts::default();
        restarts.list(listed(&frames[3]), true);
        assert_eq!(restarts.list(None, true), RestartStep::Inspect(12));
        assert!(!stopped(&restarts, true).launch);
        assert_eq!(
            restarts.inspected(12, Some(&frames[4])),
            RestartStep::Attach
        );
        assert_eq!(
            stopped(&restarts, true).message,
            "Restarting into a fresh session…"
        );
        // A stale offline state keeps it; going offline after online is a stop.
        restarts.list(None, true);
        assert!(!stopped(&restarts, true).launch);
        restarts.list(None, false);
        restarts.list(None, true);
        assert_eq!(stopped(&restarts, true).message, "builder is not running.");
    }

    #[test]
    fn a_failed_restart_ends_in_the_stopped_pane_with_its_reason() {
        let frames = frames("stream-agent-restart-failed");
        let mut restarts = Restarts::default();
        restarts.list(listed(&frames[0]), false);
        assert_eq!(restarts.list(None, true), RestartStep::Inspect(14));
        assert_eq!(
            restarts.inspected(14, Some(&frames[1])),
            RestartStep::Nothing
        );
        let reason = frames[1].result.as_ref().unwrap().reason.clone();
        assert_eq!(
            stopped(&restarts, true),
            Ended {
                error: None,
                message: "builder is not running.".into(),
                reason: reason.clone(),
                launch: true,
            }
        );
        // A long reason is shown whole, as given.
        let long = "the runtime's own record: ".repeat(40);
        let mut failed = frames[1].clone();
        failed.result.as_mut().unwrap().reason = Some(long.clone());
        let mut restarts = Restarts::default();
        restarts.list(listed(&frames[0]), true);
        restarts.list(None, true);
        restarts.inspected(14, Some(&failed));
        assert_eq!(stopped(&restarts, true).reason, Some(long));
        // Attaching a session again clears it.
        restarts.connected();
        assert_eq!(stopped(&restarts, true).reason, None);
    }

    #[test]
    fn a_failed_restart_is_read_from_the_agents_inspection_once() {
        use crate::roster::content::tests::{Reply, provider};
        let reason = "the session ended before the agent reached its prompt";
        let (_directory, connection, server) = provider(
            "restart",
            vec![Reply::ok(
                "/api/agents/builder/inspect",
                serde_json::json!({
                    "name": "builder", "registration_id": "1234567890abcdef1234567890abcdef",
                    "runtime": "claude", "known": true, "online": false, "state": "offline",
                    "restart": {"id": 14, "status": "failed", "phase": "failed", "resume": false,
                        "completed_at": "2026-11-19T18:00:01Z", "result": {"reason": reason}}
                }),
            )],
        );
        let frames = frames("stream-agent-restart-failed");
        let mut restarts = Restarts::default();
        restarts.list(listed(&frames[0]), false);
        let RestartStep::Inspect(id) = restarts.list(None, true) else {
            panic!("the inspection is read when the agent is offline");
        };
        let answer = latest_restart(connection, "builder");
        server.join().unwrap();
        assert_eq!(
            restarts.inspected(id, answer.as_ref()),
            RestartStep::Nothing
        );
        assert_eq!(
            stopped(&restarts, true),
            Ended {
                error: None,
                message: "builder is not running.".into(),
                reason: Some(reason.into()),
                launch: true,
            }
        );
    }

    #[test]
    fn a_restart_that_ended_otherwise_is_todays_stopped_pane() {
        let frames = frames("stream-agent-restart-failed");
        let today = Ended {
            error: None,
            message: "builder is not running.".into(),
            reason: None,
            launch: true,
        };
        // Stopped, unreadable, or a later restart than the one that left.
        for answer in [
            Some(Restart {
                phase: Some("stopped".into()),
                ..frames[1].clone()
            }),
            None,
            Some(Restart {
                id: Some(15),
                ..frames[1].clone()
            }),
        ] {
            let mut restarts = Restarts::default();
            restarts.list(listed(&frames[0]), false);
            restarts.list(None, true);
            restarts.inspected(14, answer.as_ref());
            assert_eq!(stopped(&restarts, true), today, "{answer:?}");
        }
    }

    #[test]
    fn the_broken_pipe_is_shown_only_when_no_restart_is_under_way() {
        let fresh = frames("stream-agent-restart-fresh");
        let mut restarts = Restarts::default();
        // No restart: the roster still says online and the error shows as today.
        assert_eq!(
            stopped(&restarts, false),
            Ended {
                error: Some(PIPE.into()),
                message: "Waiting for this session. Reconnecting automatically.".into(),
                reason: None,
                launch: false,
            }
        );
        restarts.list(listed(&fresh[1]), false);
        assert_eq!(stopped(&restarts, false).error, None);
        let resumed = frames("stream-agent-restart-resumed");
        let resuming = listed(
            resumed
                .iter()
                .find(|frame| frame.phase.as_deref() == Some("stopping"))
                .unwrap(),
        );
        let mut restarts = Restarts::default();
        restarts.list(resuming, false);
        assert_eq!(
            stopped(&restarts, true).message,
            "Restarting and resuming its conversation…"
        );
    }
}
