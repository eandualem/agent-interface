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
use std::{path::PathBuf, sync::Arc, time::Instant};

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

/// What became of a restart after it left the list. `at` is the roster read
/// it left in, or the newest one when it was answered ready.
#[derive(Clone, Debug, Default, PartialEq)]
enum Left {
    #[default]
    Nothing,
    /// The agent's inspection is being read.
    Asking { restart: Restart, at: Instant },
    /// It is ready: the pane tries its attachment. `tried` once an attempt
    /// has begun since.
    Ready {
        restart: Restart,
        at: Instant,
        tried: bool,
    },
}

/// One roster read that says whether the pane's agent is online.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Observation {
    at: Instant,
    offline: bool,
    /// Its unfinished restart, which the list carries while it is under way.
    restart: Option<Restart>,
}

impl Observation {
    /// The agent's state in the roster read at `at`; none while the roster
    /// cannot say whether it is online, which leaves a restart as it was.
    pub(super) fn of(status: &super::agent_status::Status, at: Option<Instant>) -> Option<Self> {
        use super::components::AgentState;
        let offline = match status.state {
            AgentState::Unknown | AgentState::Unavailable => return None,
            state => state == AgentState::Offline,
        };
        Some(Self {
            at: at?,
            offline,
            restart: status.restart.clone(),
        })
    }
}

/// A pane's view of its agent's own restart: from the roster's list and,
/// when the restart leaves the list, one read of the agent's inspection,
/// which says how it ended. Nothing is decided from when the terminal's
/// reader notices its session ended, and no timer assumes anything.
#[derive(Clone, Debug, Default, PartialEq)]
struct Restarts {
    /// The unfinished restart the list carries now.
    listed: Option<Restart>,
    left: Left,
    /// A failed answer, with its reason when it gave one, kept until a new
    /// session attaches.
    failed: Option<Option<String>>,
    /// Whether its session ended while a restart was under way or after it
    /// failed, so its end is no error until another session attaches.
    quiet: bool,
    /// Whether the pane saw its session end since it first saw the listed
    /// restart.
    ended: bool,
    /// Whether it attached a session after that end: the replacement.
    renewed: bool,
}

impl Restarts {
    /// Follow one roster read, which a pane reads again on every frame.
    /// `gone` is whether the pane shows a session that ended or could not
    /// be reached, rather than one attached or not yet attached.
    fn list(&mut self, seen: &Observation, gone: bool) -> RestartStep {
        let step = self.follow(seen);
        if gone && self.under_way() {
            self.quiet = true;
        }
        step
    }

    fn follow(&mut self, seen: &Observation) -> RestartStep {
        match (seen.restart.clone(), self.listed.take()) {
            (Some(listed), before) => {
                // A restart not listed before, or another one, is a new
                // sighting.
                if before.is_none_or(|before| before.id != listed.id) {
                    self.ended = false;
                    self.renewed = false;
                }
                self.listed = Some(listed);
                self.left = Left::Nothing;
                self.failed = None;
                RestartStep::Nothing
            }
            // Its own inspection says how it ended, whatever the roster says of
            // the agent and whatever the terminal has noticed. The answer only
            // decides what a pane whose session ended shows.
            (None, Some(restart)) => match restart.id {
                Some(id) => {
                    self.left = Left::Asking {
                        restart,
                        at: seen.at,
                    };
                    RestartStep::Inspect(id)
                }
                None => RestartStep::Nothing,
            },
            (None, None) => {
                // A read after the ready answer that says offline with
                // nothing pending is a stop: today's pane, with its buttons.
                if let Left::Ready { at, .. } = &self.left
                    && seen.at > *at
                    && seen.offline
                {
                    self.left = Left::Nothing;
                }
                RestartStep::Nothing
            }
        }
    }

    /// How restart `id` ended, from the agent's inspection; none when it
    /// could not be read. `newest` is when the roster the pane follows was
    /// last read, so only a later read can undo a ready answer.
    fn inspected(
        &mut self,
        id: u64,
        answer: Option<&Restart>,
        newest: Option<Instant>,
    ) -> RestartStep {
        let Left::Asking { restart, at } = &self.left else {
            return RestartStep::Nothing;
        };
        if restart.id != Some(id) {
            return RestartStep::Nothing;
        }
        let (restart, at) = (restart.clone(), newest.unwrap_or(*at).max(*at));
        self.left = Left::Nothing;
        // A later restart than the one that left says nothing of it.
        let answer = answer.filter(|answer| answer.id == Some(id));
        match answer.and_then(|answer| answer.phase.as_deref()) {
            // The pane already attached a session after the old one ended:
            // that is the replacement, so the restart is over.
            Some("ready") if self.renewed => RestartStep::Nothing,
            Some("ready") => {
                self.left = Left::Ready {
                    restart,
                    at,
                    tried: false,
                };
                RestartStep::Attach
            }
            Some("failed") => {
                self.failed = Some(
                    answer
                        .and_then(|answer| answer.result.as_ref())
                        .and_then(|result| result.reason.clone())
                        .filter(|reason| !reason.trim().is_empty()),
                );
                RestartStep::Nothing
            }
            _ => RestartStep::Nothing,
        }
    }

    /// Whether a restart is under way, so an ended session is no error.
    fn under_way(&self) -> bool {
        self.listed.is_some() || self.left != Left::Nothing
    }

    /// Whether the session's end is no error: it ended, or may end, as part
    /// of a restart.
    fn quiet(&self) -> bool {
        self.under_way() || self.quiet || self.failed.is_some()
    }

    /// The pane begins an attempt to attach. Showing another window of the
    /// session it shows (`switch`) is none.
    fn attempting(&mut self, switch: bool) {
        if switch {
            return;
        }
        if let Left::Ready { tried, .. } = &mut self.left {
            *tried = true;
        }
    }

    /// An attempt to attach failed. A ready restart whose new session
    /// cannot be attached is today's pane, with its buttons.
    /// Its error says nothing typed of whether the session is there, so it
    /// is no sign that the session ended.
    fn attach_failed(&mut self, switch: bool) {
        if switch {
            return;
        }
        if matches!(self.left, Left::Ready { tried: true, .. }) {
            self.left = Left::Nothing;
        }
    }

    /// The pane attached a session. Another window of the session it shows
    /// (`switch`) is no new session and changes nothing.
    fn connected(&mut self, switch: bool) {
        if switch {
            return;
        }
        // A session attached: no ended one is shown any more.
        self.quiet = false;
        // A ready restart ends at the next attach. An attach after an end is
        // a new session, which ends a read under way and a kept failure; any
        // other, such as the first one, may still be the old session, so
        // they stay.
        if matches!(self.left, Left::Ready { .. }) || self.ended {
            self.left = Left::Nothing;
        }
        if self.ended {
            self.renewed = true;
            self.failed = None;
        }
    }

    /// The person starts the agent: the reason kept from its restart is
    /// over. An ended session stays quiet until the next attach.
    fn launched(&mut self) {
        self.failed = None;
    }

    /// The pane's session ended.
    fn disconnected(&mut self) {
        self.ended = true;
        if self.under_way() || self.failed.is_some() {
            self.quiet = true;
        }
    }

    /// What to show, given whether a session is attached and shown.
    fn shown(&self, attached: bool) -> Option<RestartShown> {
        let asking = matches!(self.left, Left::Asking { .. });
        if attached {
            // While its inspection is read, the pane keeps what it showed.
            return (self.listed.is_some() || asking).then_some(RestartShown::Pending);
        }
        let left = match &self.left {
            Left::Asking { restart, .. } | Left::Ready { restart, .. } => Some(restart),
            Left::Nothing => None,
        };
        match self.listed.as_ref().or(left) {
            Some(restart) => Some(RestartShown::Restarting {
                resume: restart.resume == Some(true),
            }),
            None => self.failed.clone().flatten().map(RestartShown::Failed),
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
/// `quiet` is whether the session ended while its agent restarted, which
/// is no error.
fn ended(
    error: &str,
    agent: Option<&str>,
    saved: bool,
    sign_in: bool,
    restart: Option<&RestartShown>,
    quiet: bool,
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
        error: (agent.is_none() && !sign_in && !quiet).then(|| error.into()),
        message,
        reason: match restart {
            Some(RestartShown::Failed(reason)) if agent.is_some() => Some(reason.clone()),
            _ => None,
        },
        launch: agent.is_some() && restarting.is_none(),
    }
}

/// What a pane draws of its session's end and its agent's restart.
#[derive(Debug, PartialEq, Eq)]
struct Drawn {
    /// Under the last output, once the session is not shown.
    ended: Option<Ended>,
    /// The "Restart pending" mark over the terminal.
    pending: bool,
    /// Whether the terminal leaves its ended session's message unpainted.
    quiet_end: bool,
}

/// `shown` is whether a terminal view is connected; `error` why it ended.
fn drawn(
    restarts: &Restarts,
    error: Option<&str>,
    shown: bool,
    agent: Option<&str>,
    saved: bool,
    sign_in: bool,
) -> Drawn {
    let restart = restarts.shown(error.is_none() && shown);
    let quiet = restarts.quiet();
    Drawn {
        ended: error.map(|error| ended(error, agent, saved, sign_in, restart.as_ref(), quiet)),
        pending: restart == Some(RestartShown::Pending),
        quiet_end: quiet,
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
    /// The roster its agent is followed in, once one has been given.
    roster: Option<super::agent_status::LiveAgents>,
    /// Whether the next connection shows another window of the session it
    /// shows, which is no new session.
    switching: bool,
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

    /// This pane's agent as its roster shows it now, in any state, and that
    /// roster. A state that cannot say whether it is online changes nothing.
    pub fn set_restart(
        &mut self,
        roster: Option<(
            std::rc::Rc<super::agent_status::Status>,
            super::agent_status::LiveAgents,
        )>,
        cx: &mut Context<Self>,
    ) {
        let Some((status, agents)) = roster else {
            return;
        };
        let seen = Observation::of(&status, agents.observed_at());
        self.roster = Some(agents);
        let Some(seen) = seen else {
            return;
        };
        let before = self.restarts.clone();
        // Only a session this pane attached can have ended; an attach that
        // never reached one says nothing.
        let gone = self
            .view
            .as_ref()
            .is_some_and(|view| self.error.is_some() || view.read(cx).ended());
        let step = self.restarts.list(&seen, gone);
        self.restart_step(step, cx);
        if self.restarts != before {
            cx.notify();
        }
    }

    /// When the roster this pane follows was last read.
    fn newest_read(&self) -> Option<Instant> {
        self.roster.as_ref().and_then(|roster| roster.observed_at())
    }

    fn restart_step(&mut self, step: RestartStep, cx: &mut Context<Self>) {
        match step {
            RestartStep::Nothing => {}
            RestartStep::Attach => self.retry = Retry::default(),
            RestartStep::Inspect(id) => {
                let (Some(connection), Some(attachment)) =
                    (self.target.connection.clone(), &self.target.leaf.attachment)
                else {
                    let step = self.restarts.inspected(id, None, self.newest_read());
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
                        let step = terminal.restarts.inspected(
                            id,
                            answer.as_ref(),
                            terminal.newest_read(),
                        );
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
        self.restarts.launched();
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
            roster: None,
            switching: false,
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
        let switching = std::mem::take(&mut self.switching);
        // A switch to another window keeps hearing whether the session it
        // shows ends; its new view replaces this once it is ready.
        if !switching {
            self.closed = None;
        }
        // Keep the old client attached until its replacement is ready. A source
        // with destroy-unattached enabled must not disappear during navigation.
        if let Some(view) = &self.view {
            view.update(cx, |view, _| view.suspend_input());
        }
        // Retain the last output and error while retrying, instead of flashing
        // an empty Connecting panel on every automatic attempt.
        self.connecting = true;
        self.restarts.attempting(switching);
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
                        terminal.restarts.connected(switching);
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
                                terminal.restarts.disconnected();
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
                            terminal.restarts.attach_failed(switching);
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
            self.switching = true;
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
        let drawn = drawn(
            &self.restarts,
            self.error.as_deref(),
            self.view.is_some(),
            self.agent.as_deref(),
            self.target.leaf.attachment.is_some(),
            self.target.is_sign_in(),
        );
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.quiet_end(drawn.quiet_end, cx));
        }
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(c.surface)
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
            .when_some(drawn.ended, |el, ended| {
                let attached = self.target.leaf.attachment.is_some();
                let launching = self.launching;
                // In a short pane the text scrolls and the buttons stay.
                el.child(
                    div()
                        .id("ended-message")
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(ended.error.map(|error| line(error, c.attention_text)))
                        .child(line(ended.message, c.muted))
                        // Shown whole, wrapping, as the service wrote it.
                        .children(ended.reason.map(|reason| line(reason, c.muted))),
                )
                .child(
                    div()
                        .flex_shrink_0()
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
                                ui::button("open-terminal", "Open terminal", ui::Button::Secondary)
                                    .on_click(cx.listener(|terminal, _, _, cx| {
                                        // The provider starts a bound shell; connect asks it again.
                                        if terminal.target.connection.is_some() {
                                            terminal.start(cx);
                                        } else {
                                            cx.emit(OpenTerminal);
                                        }
                                    })),
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
            // Last, so it paints over the terminal, whose size it leaves alone.
            .when(drawn.pending, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(gpui::px(t.space.s8))
                        .right(gpui::px(t.space.s12))
                        .child(ui::tag("Restart pending", ui::Tone::Plain)),
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

    /// The roster read `tick` reads after the first.
    fn at(tick: u64) -> Instant {
        static FIRST: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *FIRST.get_or_init(Instant::now) + Duration::from_secs(tick)
    }

    fn seen(tick: u64, offline: bool, restart: Option<Restart>) -> Observation {
        Observation {
            at: at(tick),
            offline,
            restart,
        }
    }

    const PIPE: &str = "writing to control client: Broken pipe (os error 32)";

    /// What a bound agent's pane says once its session ended.
    fn stopped(restarts: &Restarts, offline: bool) -> Ended {
        let agent = offline.then_some("builder");
        drawn(restarts, Some(PIPE), true, agent, true, false)
            .ended
            .unwrap()
    }

    fn restarting() -> Ended {
        Ended {
            error: None,
            message: "Restarting into a fresh session…".into(),
            reason: None,
            launch: false,
        }
    }

    fn not_running(reason: Option<String>) -> Ended {
        Ended {
            error: None,
            message: "builder is not running.".into(),
            reason,
            launch: true,
        }
    }

    /// What the pane draws while its session is attached and shown.
    fn live(restarts: &Restarts) -> Drawn {
        drawn(restarts, None, true, None, true, false)
    }

    /// A restart listed as `frames` while the pane's session is attached,
    /// then dropped from a read that says `offline` while a session is
    /// still `attached` or not: the step it gives.
    fn dropped(
        restarts: &mut Restarts,
        frames: &[&Restart],
        offline: bool,
        attached: bool,
    ) -> RestartStep {
        for (tick, frame) in (0..).zip(frames) {
            restarts.list(&seen(tick, false, listed(frame)), false);
        }
        restarts.list(&seen(frames.len() as u64, offline, None), !attached)
    }

    #[test]
    fn a_fresh_restart_reads_as_restarting_from_start_to_attach() {
        let frames = frames("stream-agent-restart-fresh");
        let mut restarts = Restarts::default();
        restarts.connected(false);
        // The old session is attached until the restart stops it.
        assert_eq!(
            restarts.list(&seen(0, false, listed(&frames[0])), false),
            RestartStep::Nothing
        );
        assert!(live(&restarts).pending);
        restarts.disconnected();
        // It ended: whether the roster still says online or already offline,
        // the pane says it restarts, with no error and no launch buttons.
        for (tick, frame) in (1..).zip(&frames[1..4]) {
            for offline in [false, true] {
                restarts.list(&seen(tick, offline, listed(frame)), true);
                assert_eq!(
                    stopped(&restarts, offline),
                    restarting(),
                    "{:?}",
                    frame.phase
                );
            }
        }
        // It leaves the list: the one inspection says how it ended, and the
        // pane keeps saying it restarts meanwhile.
        assert_eq!(
            restarts.list(&seen(5, false, listed(&frames[4])), true),
            RestartStep::Inspect(12)
        );
        assert_eq!(stopped(&restarts, false), restarting());
        // Ready: the pane tries its attachment at once, still restarting.
        assert_eq!(
            restarts.inspected(12, Some(&frames[4]), Some(at(5))),
            RestartStep::Attach
        );
        restarts.list(&seen(6, false, None), true);
        assert_eq!(stopped(&restarts, false), restarting());
        restarts.connected(false);
        assert_eq!(
            live(&restarts),
            Drawn {
                ended: None,
                pending: false,
                quiet_end: false,
            }
        );
    }

    #[test]
    fn a_roster_that_cannot_say_keeps_a_pending_restart() {
        let frames = frames("stream-agent-restart-fresh");
        let agents = super::super::agent_status::LiveAgents::default();
        let mut snapshot = agent_interface_roster::Snapshot::default();
        snapshot.update(br#"{"items":[{"name":"builder","state":"some future state"}]}"#);
        agents.update(snapshot);
        // Unknown, and absent from the roster, say nothing either way.
        for name in ["builder", "missing"] {
            assert_eq!(Observation::of(&agents.get(name), Some(at(0))), None);
        }
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(&frames[1])), true);
        // The pane is given no read, so the restart stays pending.
        assert_eq!(stopped(&restarts, false), restarting());
    }

    #[test]
    fn a_pending_restart_seen_late_still_reads_as_restarting() {
        let frames = frames("stream-agent-restart-fresh");
        let mut restarts = Restarts::default();
        // The session ended before any list showed the restart: today's pane.
        restarts.list(&seen(0, false, None), true);
        assert_eq!(stopped(&restarts, false).error.as_deref(), Some(PIPE));
        restarts.list(&seen(1, true, None), true);
        assert_eq!(stopped(&restarts, true), not_running(None));
        // The first list that shows it pending switches the pane.
        restarts.list(&seen(2, true, listed(&frames[2])), true);
        assert_eq!(stopped(&restarts, true), restarting());
        // Its end is read once and a ready answer reattaches.
        assert_eq!(
            restarts.list(&seen(3, false, None), true),
            RestartStep::Inspect(12)
        );
        assert_eq!(
            restarts.inspected(12, Some(&frames[4]), Some(at(3))),
            RestartStep::Attach
        );
    }

    #[test]
    fn a_ready_restart_never_leaves_the_pane_stuck() {
        let frames = frames("stream-agent-restart-fresh");
        let ready = |restarts: &mut Restarts| {
            assert_eq!(
                dropped(restarts, &[&frames[3]], true, false),
                RestartStep::Inspect(12)
            );
            restarts.disconnected();
            assert_eq!(stopped(restarts, true), restarting());
            assert_eq!(
                restarts.inspected(12, Some(&frames[4]), Some(at(1))),
                RestartStep::Attach
            );
            // The same read again, as every frame gives it, changes nothing.
            restarts.list(&seen(1, true, None), true);
            assert_eq!(stopped(restarts, true), restarting());
        };
        // A later read that says online: it keeps trying to attach.
        let mut restarts = Restarts::default();
        ready(&mut restarts);
        restarts.list(&seen(2, false, None), true);
        assert_eq!(stopped(&restarts, false), restarting());
        // A later read that says offline, as when the new session ended
        // before the pane reached it: today's stopped pane with its buttons.
        let mut restarts = Restarts::default();
        ready(&mut restarts);
        restarts.list(&seen(2, true, None), true);
        assert_eq!(stopped(&restarts, true), not_running(None));
        // An attempt to attach that begins after the answer and fails:
        // today's pane at once, without waiting for another read.
        let mut restarts = Restarts::default();
        ready(&mut restarts);
        restarts.attach_failed(false);
        assert_eq!(
            stopped(&restarts, true),
            restarting(),
            "no attempt has begun"
        );
        restarts.attempting(false);
        restarts.attach_failed(false);
        assert_eq!(stopped(&restarts, true), not_running(None));
    }

    #[test]
    fn a_read_given_while_the_inspection_is_asked_does_not_undo_its_ready_answer() {
        let frames = frames("stream-agent-restart-fresh");
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, true, listed(&frames[3])), true);
        assert_eq!(
            restarts.list(&seen(1, true, None), true),
            RestartStep::Inspect(12)
        );
        // A newer offline read lands while the inspection is read.
        restarts.list(&seen(2, true, None), true);
        // The roster the pane follows has read T2 by the time the answer comes.
        assert_eq!(
            restarts.inspected(12, Some(&frames[4]), Some(at(2))),
            RestartStep::Attach
        );
        // Every frame gives that read again: the pane still waits to attach.
        restarts.list(&seen(2, true, None), true);
        assert_eq!(stopped(&restarts, true), restarting());
        // Only a read after the ready answer says it ended: today's pane.
        restarts.list(&seen(3, true, None), true);
        assert_eq!(stopped(&restarts, true), not_running(None));
    }

    #[test]
    fn a_failed_restart_ends_in_the_stopped_pane_with_its_reason() {
        let frames = frames("stream-agent-restart-failed");
        let reason = frames[1].result.as_ref().unwrap().reason.clone();
        // The session had already ended when the answer came.
        let mut restarts = Restarts::default();
        restarts.connected(false);
        assert_eq!(
            dropped(&mut restarts, &[&frames[0]], true, false),
            RestartStep::Inspect(14)
        );
        restarts.disconnected();
        assert_eq!(
            restarts.inspected(14, Some(&frames[1]), None),
            RestartStep::Nothing
        );
        assert_eq!(stopped(&restarts, true), not_running(reason.clone()));
        // The old session's end stays quiet under the stopped pane.
        assert!(drawn(&restarts, Some(PIPE), true, Some("builder"), true, false).quiet_end);
        assert_eq!(stopped(&restarts, false).error, None);
        // Attaching a session again clears it.
        restarts.connected(false);
        assert_eq!(stopped(&restarts, true).reason, None);
        assert!(!live(&restarts).quiet_end);
        // A long reason is shown whole, as given; an empty one is none.
        for (given, shown) in [
            (Some("the runtime's own record: ".repeat(40)), true),
            (Some("  ".into()), false),
        ] {
            let mut failed = frames[1].clone();
            failed.result.as_mut().unwrap().reason = given.clone();
            let mut restarts = Restarts::default();
            dropped(&mut restarts, &[&frames[0]], true, false);
            restarts.disconnected();
            restarts.inspected(14, Some(&failed), None);
            assert_eq!(stopped(&restarts, true).reason, given.filter(|_| shown));
        }
    }

    #[test]
    fn a_failed_restart_whose_session_lives_on_is_an_ordinary_terminal() {
        let frames = frames("stream-agent-restart-failed");
        let reason = frames[1].result.as_ref().unwrap().reason.clone();
        // Nothing was stopped: the old session is still attached and shown.
        let mut restarts = Restarts::default();
        restarts.connected(false);
        assert_eq!(
            dropped(&mut restarts, &[&frames[0]], false, true),
            RestartStep::Inspect(14)
        );
        assert!(
            live(&restarts).pending,
            "the read keeps what the pane showed"
        );
        restarts.inspected(14, Some(&frames[1]), None);
        let shown = live(&restarts);
        assert!(!shown.pending && shown.ended.is_none());
        assert!(!restarts.under_way());
        // The answer came before the reader saw the session close: when it
        // then ends, the stopped pane carries the reason, with no pipe.
        restarts.disconnected();
        assert_eq!(stopped(&restarts, true), not_running(reason));
        assert_eq!(stopped(&restarts, false).error, None);
    }

    #[test]
    fn a_restart_that_left_the_list_is_read_however_far_it_got() {
        let fresh = frames("stream-agent-restart-fresh");
        // Never seen past waiting for its turn: the read decides, nothing is
        // assumed.
        let mut restarts = Restarts::default();
        restarts.connected(false);
        assert_eq!(
            dropped(&mut restarts, &[&fresh[0]], false, true),
            RestartStep::Inspect(12)
        );
        // Seen stopping, then dropped before the reader saw the session close.
        let mut restarts = Restarts::default();
        restarts.connected(false);
        assert_eq!(
            dropped(&mut restarts, &[&fresh[0], &fresh[1]], false, true),
            RestartStep::Inspect(12)
        );
        assert!(live(&restarts).quiet_end);
        restarts.disconnected();
        assert_eq!(stopped(&restarts, false), restarting());
    }

    #[test]
    fn every_restart_leaving_the_list_is_read_and_a_window_switch_changes_nothing() {
        let fresh = frames("stream-agent-restart-fresh");
        // A pane opened while the restart is listed, then its first attach.
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(&fresh[1])), false);
        restarts.connected(false);
        assert_eq!(
            restarts.list(&seen(1, false, None), false),
            RestartStep::Inspect(12)
        );
        // Another restart listed in its place is read by its own number.
        let mut restarts = Restarts::default();
        restarts.connected(false);
        restarts.list(&seen(0, false, listed(&fresh[1])), false);
        restarts.disconnected();
        restarts.connected(false);
        let next = Restart {
            id: Some(13),
            ..fresh[1].clone()
        };
        restarts.list(&seen(1, false, listed(&next)), false);
        assert_eq!(
            restarts.list(&seen(2, false, None), false),
            RestartStep::Inspect(13)
        );
        // Showing another window of the same session changes nothing, even
        // after an end was seen.
        let mut restarts = Restarts::default();
        restarts.connected(false);
        restarts.list(&seen(0, false, listed(&fresh[1])), false);
        restarts.disconnected();
        let before = restarts.clone();
        restarts.attempting(true);
        restarts.connected(true);
        restarts.attach_failed(true);
        assert_eq!(restarts, before);
        assert_eq!(
            restarts.list(&seen(1, false, None), false),
            RestartStep::Inspect(12)
        );
    }

    #[test]
    fn a_first_attach_while_the_inspection_is_read_keeps_it() {
        let failed = frames("stream-agent-restart-failed");
        let fresh = frames("stream-agent-restart-fresh");
        let reason = failed[1].result.as_ref().unwrap().reason.clone();
        // The restart leaves the list before the pane's first attach, which
        // then reaches the old session: the read and its failed answer stay.
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(&failed[0])), false);
        assert_eq!(
            restarts.list(&seen(1, false, None), false),
            RestartStep::Inspect(14)
        );
        restarts.connected(false);
        assert_eq!(
            restarts.inspected(14, Some(&failed[1]), None),
            RestartStep::Nothing
        );
        restarts.disconnected();
        assert_eq!(stopped(&restarts, true), not_running(reason));
        assert_eq!(stopped(&restarts, false).error, None);
        // A ready answer stays too, until a session attaches after an end.
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(&fresh[3])), false);
        assert_eq!(
            restarts.list(&seen(1, false, None), false),
            RestartStep::Inspect(12)
        );
        restarts.connected(false);
        assert_eq!(
            restarts.inspected(12, Some(&fresh[4]), Some(at(1))),
            RestartStep::Attach
        );
        restarts.disconnected();
        assert_eq!(stopped(&restarts, false), restarting());
        restarts.connected(false);
        assert!(!restarts.under_way());
    }

    #[test]
    fn an_attach_that_fails_without_reaching_its_session_is_no_end() {
        let failed = frames("stream-agent-restart-failed");
        let reason = failed[1].result.as_ref().unwrap().reason.clone();
        // The first attach fails before it reaches tmux, as when the
        // workspace profile is unavailable; the same session then attaches.
        // The read under way and its failed answer stay.
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(&failed[0])), false);
        assert_eq!(
            restarts.list(&seen(1, false, None), false),
            RestartStep::Inspect(14)
        );
        restarts.attempting(false);
        restarts.attach_failed(false);
        restarts.connected(false);
        restarts.inspected(14, Some(&failed[1]), None);
        restarts.disconnected();
        assert_eq!(stopped(&restarts, true), not_running(reason));
        // A pane that never attached: its attach error during the restart
        // latches nothing, so once the restart is over it shows as today.
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(&failed[0])), false);
        restarts.attempting(false);
        restarts.attach_failed(false);
        restarts.list(&seen(1, false, None), false);
        restarts.inspected(14, None, None);
        assert_eq!(stopped(&restarts, false).error.as_deref(), Some(PIPE));
    }

    #[test]
    fn a_ready_answers_attach_and_a_launch_end_what_the_restart_kept() {
        let fresh = frames("stream-agent-restart-fresh");
        let failed = frames("stream-agent-restart-failed");
        let today = Ended {
            error: Some(PIPE.into()),
            message: "Waiting for this session. Reconnecting automatically.".into(),
            reason: None,
            launch: false,
        };
        // Opened after the old session stopped: no end was seen, and the
        // attach begun after the ready answer reaches the replacement.
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, true, listed(&fresh[3])), false);
        assert_eq!(
            restarts.list(&seen(1, true, None), false),
            RestartStep::Inspect(12)
        );
        restarts.inspected(12, Some(&fresh[4]), Some(at(1)));
        restarts.attempting(false);
        restarts.connected(false);
        assert!(!restarts.under_way());
        restarts.disconnected();
        assert_eq!(stopped(&restarts, false), today);
        // After a failed restart the person starts the agent: its new
        // session's later end carries no old reason and shows as today.
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(&failed[0])), false);
        restarts.list(&seen(1, true, None), false);
        restarts.connected(false);
        restarts.inspected(14, Some(&failed[1]), None);
        restarts.launched();
        restarts.connected(false);
        restarts.disconnected();
        assert_eq!(stopped(&restarts, false), today);
        assert_eq!(stopped(&restarts, true), not_running(None));
        // A launch from the stopped pane of a failed restart: until a session
        // attaches, the old session's end stays quiet and its reason is gone.
        let mut restarts = Restarts::default();
        restarts.connected(false);
        restarts.list(&seen(0, false, listed(&failed[0])), false);
        restarts.list(&seen(1, true, None), true);
        restarts.disconnected();
        restarts.inspected(14, Some(&failed[1]), None);
        restarts.launched();
        assert_eq!(stopped(&restarts, true), not_running(None));
        assert_eq!(stopped(&restarts, false).error, None);
        assert!(drawn(&restarts, Some(PIPE), true, None, true, false).quiet_end);
        restarts.connected(false);
        assert!(!live(&restarts).quiet_end);
    }

    #[test]
    fn a_restart_that_leaves_the_list_after_its_replacement_attached_is_still_read() {
        let fresh = frames("stream-agent-restart-fresh");
        let failed = frames("stream-agent-restart-failed");
        // The replacement attached while the list still said starting.
        let reattached = |frames: &[Restart]| {
            let mut restarts = Restarts::default();
            restarts.connected(false);
            restarts.list(&seen(0, false, listed(&frames[0])), false);
            restarts.disconnected();
            restarts.list(&seen(1, false, listed(&frames[0])), true);
            restarts.connected(false);
            restarts
        };
        // A ready answer: the attached session is the replacement, so the
        // restart is over and a later end shows as today.
        let mut restarts = reattached(&fresh);
        assert_eq!(
            restarts.list(&seen(2, false, None), false),
            RestartStep::Inspect(12)
        );
        assert_eq!(
            restarts.inspected(12, Some(&fresh[4]), Some(at(2))),
            RestartStep::Nothing
        );
        let shown = live(&restarts);
        assert!(!shown.pending && shown.ended.is_none() && !restarts.under_way());
        restarts.disconnected();
        assert_eq!(stopped(&restarts, false).error.as_deref(), Some(PIPE));
        // A failed answer after the replacement died: the stopped pane with
        // the service's reason, and no pipe.
        let mut restarts = reattached(&failed);
        assert_eq!(
            restarts.list(&seen(2, true, None), false),
            RestartStep::Inspect(14)
        );
        restarts.disconnected();
        restarts.inspected(14, Some(&failed[1]), None);
        let reason = failed[1].result.as_ref().unwrap().reason.clone();
        assert_eq!(stopped(&restarts, true), not_running(reason));
        assert_eq!(stopped(&restarts, false).error, None);
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
        restarts.list(&seen(0, false, listed(&frames[0])), true);
        let RestartStep::Inspect(id) = restarts.list(&seen(1, true, None), true) else {
            panic!("the inspection is read when the agent is offline");
        };
        let answer = latest_restart(connection, "builder");
        server.join().unwrap();
        assert_eq!(
            restarts.inspected(id, answer.as_ref(), None),
            RestartStep::Nothing
        );
        assert_eq!(stopped(&restarts, true), not_running(Some(reason.into())));
        // An inspection the service refuses, as when it fails or times out,
        // is today's pane, as is a forgotten agent's, which is not found.
        for status in [500, 404] {
            let (_directory, connection, server) = provider(
                "refused",
                vec![Reply {
                    status,
                    ..Reply::ok("/api/agents/builder/inspect", serde_json::json!({}))
                }],
            );
            let mut restarts = Restarts::default();
            restarts.list(&seen(0, false, listed(&frames[0])), true);
            restarts.list(&seen(1, true, None), true);
            let answer = latest_restart(connection, "builder");
            server.join().unwrap();
            assert_eq!(answer, None, "{status}");
            restarts.inspected(14, answer.as_ref(), None);
            assert_eq!(stopped(&restarts, true), not_running(None), "{status}");
        }
    }

    #[test]
    fn a_restart_that_ended_otherwise_is_todays_stopped_pane() {
        let frames = frames("stream-agent-restart-failed");
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
            restarts.list(&seen(0, false, listed(&frames[0])), true);
            restarts.list(&seen(1, true, None), true);
            restarts.inspected(14, answer.as_ref(), None);
            assert_eq!(stopped(&restarts, true), not_running(None), "{answer:?}");
        }
    }

    #[test]
    fn the_broken_pipe_is_shown_only_when_no_restart_is_under_way() {
        let fresh = frames("stream-agent-restart-fresh");
        let mut restarts = Restarts::default();
        // No restart: the roster still says online and the error shows as
        // today, in the pane and in the terminal.
        assert_eq!(
            drawn(&restarts, Some(PIPE), true, None, true, false),
            Drawn {
                ended: Some(Ended {
                    error: Some(PIPE.into()),
                    message: "Waiting for this session. Reconnecting automatically.".into(),
                    reason: None,
                    launch: false,
                }),
                pending: false,
                quiet_end: false,
            }
        );
        restarts.list(&seen(0, false, listed(&fresh[1])), true);
        let restarting = drawn(&restarts, Some(PIPE), true, None, true, false);
        assert_eq!(restarting.ended.unwrap().error, None);
        assert!(restarting.quiet_end && !restarting.pending);
        let resumed = frames("stream-agent-restart-resumed");
        let resuming = resumed
            .iter()
            .find(|frame| frame.phase.as_deref() == Some("stopping"))
            .unwrap();
        let mut restarts = Restarts::default();
        restarts.list(&seen(0, false, listed(resuming)), true);
        assert_eq!(
            stopped(&restarts, true).message,
            "Restarting and resuming its conversation…"
        );
    }

    /// One list from a stand-in service, applied to the pane's restart: the
    /// agent's state and, while under way, its restart as the list item
    /// carries it. Returns the pane's step and whether the list says offline.
    fn read(
        agents: &super::super::agent_status::LiveAgents,
        restarts: &mut Restarts,
        tick: u64,
        state: &str,
        frame: &Restart,
        attached: bool,
    ) -> (RestartStep, bool) {
        let restart = listed(frame).map(|restart| {
            serde_json::json!({"id": restart.id, "phase": restart.phase, "resume": restart.resume})
        });
        let body =
            serde_json::json!({"items": [{"name": "builder", "state": state, "restart": restart}]});
        let mut snapshot = agent_interface_roster::Snapshot::default();
        snapshot.update(body.to_string().as_bytes());
        snapshot.observed_at = Some(at(tick));
        agents.update(snapshot);
        let status = agents.get("builder");
        let offline = status.state == super::super::components::AgentState::Offline;
        let seen = Observation::of(&status, agents.observed_at()).unwrap();
        (restarts.list(&seen, !attached), offline)
    }

    #[test]
    fn a_stand_in_services_lists_drive_what_the_pane_draws_through_to_attach() {
        let frames = frames("stream-agent-restart-fresh");
        let agents = super::super::agent_status::LiveAgents::default();
        let mut restarts = Restarts::default();
        restarts.connected(false);
        let drawn_now = |restarts: &Restarts, error: Option<&str>, offline: bool| {
            drawn(
                restarts,
                error,
                true,
                offline.then_some("builder"),
                true,
                false,
            )
        };
        // Asked for while it works: the attached terminal carries the mark.
        let (step, _) = read(&agents, &mut restarts, 0, "busy", &frames[0], true);
        assert_eq!(step, RestartStep::Nothing);
        let attached = drawn_now(&restarts, None, false);
        assert!(attached.pending && attached.quiet_end && attached.ended.is_none());
        // Stopping: the session ends while the list still says busy.
        read(&agents, &mut restarts, 1, "busy", &frames[1], true);
        restarts.disconnected();
        let ending = drawn_now(&restarts, Some(PIPE), false);
        assert_eq!(ending.ended, Some(restarting()));
        assert!(ending.quiet_end && !ending.pending);
        // Waiting and starting, offline: no error, no Resume or Start new.
        for tick in [2, 3] {
            let (_, offline) = read(
                &agents,
                &mut restarts,
                tick,
                "offline",
                &frames[tick as usize],
                false,
            );
            assert!(offline);
            assert_eq!(
                drawn_now(&restarts, Some(PIPE), true).ended,
                Some(restarting())
            );
        }
        // Ready: the list drops the restart and says the agent is busy again;
        // the one inspection answers ready.
        let (step, offline) = read(&agents, &mut restarts, 4, "busy", &frames[4], false);
        assert_eq!((step, offline), (RestartStep::Inspect(12), false));
        assert_eq!(
            drawn_now(&restarts, Some(PIPE), false).ended,
            Some(restarting())
        );
        let step = restarts.inspected(12, Some(&frames[4]), agents.observed_at());
        assert_eq!(step, RestartStep::Attach);
        assert_eq!(
            drawn_now(&restarts, Some(PIPE), false).ended,
            Some(restarting())
        );
        // The pane attaches its new session.
        restarts.connected(false);
        assert_eq!(
            drawn_now(&restarts, None, false),
            Drawn {
                ended: None,
                pending: false,
                quiet_end: false,
            }
        );
    }
}
