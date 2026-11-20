//! Recovery of original first-agent requests, independent of catalog readiness.
use super::{Back, Open, PAGE_SIZE, Ticket, ui};
use crate::{
    desktop::design::{colors, tokens},
    profiles::{
        Client, journal,
        setup_controller::{Controller, Error, Phase, Progress, Snapshot},
    },
};
use agent_interface_roster::profiles::{
    AddError, Connection, Profile,
    setup::{
        Action, Event, Prepared,
        start::{Start, Started},
    },
};
use futures::{StreamExt, channel::mpsc};
use gpui::{Context, EventEmitter, FocusHandle, SharedString, Window, div, prelude::*, px};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Read,
    Submit,
    Retry,
    Cancel,
    RetryCancel,
}
impl Command {
    fn allowed(self, snapshot: Option<&Snapshot>) -> bool {
        if self == Self::Read {
            return true;
        }
        let Some(s) = snapshot else {
            return false;
        };
        if s.closed() {
            return false;
        }
        let retry = |event: Option<&Event>| {
            event.is_none_or(|e| e.retry.as_deref() == Some("same_request"))
        };
        match self {
            Self::Read => true,
            Self::Submit => s.phase == Phase::Prepared,
            Self::Cancel => s.phase == Phase::Submitted && s.cancellation_result.is_none(),
            Self::Retry => {
                s.phase != Phase::Prepared
                    && retry(s.first_result.as_ref())
                    && (s.phase != Phase::CancelPending
                        || s.cancellation_result
                            .as_ref()
                            .is_some_and(crate::profiles::setup_controller::left_open))
            }
            Self::RetryCancel => {
                s.phase != Phase::Prepared
                    && (s.phase == Phase::CancelPending || s.cancellation_result.is_some())
                    && retry(s.cancellation_result.as_ref())
            }
        }
    }
}
fn status(s: &Snapshot) -> &'static str {
    if s.cancellation_result.as_ref().is_some_and(|e| e.cancelled) {
        "Safely cancelled"
    } else if started(s) {
        "Agent started"
    } else if s.phase == Phase::Prepared {
        "Saved, not submitted"
    } else if s.phase == Phase::CancelPending && s.cancellation_result.is_none() {
        "Cancellation outcome not confirmed"
    } else if s.first_result.is_none() && s.cancellation_result.is_none() {
        "Setup outcome not confirmed"
    } else if s.closed() {
        // Setup's answers to a request sent again may describe only that
        // send, so none says the agent wasn't added.
        "Not known whether it was added"
    } else {
        "Needs attention"
    }
}
/// The agent's CLI runs. Its terminal may still ask for a sign-in, trust or
/// terms first, so that terminal is the next step.
fn started(s: &Snapshot) -> bool {
    s.first_result
        .as_ref()
        .is_some_and(|event| event.ready_agent.is_some())
}
/// What the latest result means for the person, with its code: what came
/// of it, then what to do next.
fn outcome(s: &Snapshot) -> Option<(String, String)> {
    let cancel_view = s.phase == Phase::CancelPending;
    let event = if cancel_view {
        s.cancellation_result.as_ref()
    } else {
        s.first_result.as_ref()
    }?;
    let code = event.code.clone()?;
    Some((format!("{} {}", said(event)?, next(s, cancel_view)), code))
}
/// What came of a result: the app's own words where it can point to a
/// remedy, else setup's message; none for the agent started or the
/// cancellation confirmed.
fn said(event: &Event) -> Option<String> {
    let cancel = event.action == Action::CancelFirstAgent;
    Some(match event.code.as_deref()? {
        "agent_ready" if event.ready_agent.is_some() => return None,
        "request_cancelled" if event.cancelled => return None,
        "cancellation_unsafe" => "Setup had already begun to change things, so it wasn't cancelled: a cancellation is not a rollback. Look at the agent first.".into(),
        "agent_conflict" => "The name, its folder or its terminal isn't free, or the folder is missing or could not be made: change the name or folder.".into(),
        "runtime_missing" => "This tool isn't installed where the profile can use it, or setup doesn't start it: install it or choose another.".into(),
        "invalid_request" if cancel => "Setup refused the cancellation as it was sent.".into(),
        "invalid_request" => "Setup refused the request as it was sent: change the agent's name, folder, model or instructions.".into(),
        "service_unavailable" => "The profile's service, or the computer the agent runs on, did not answer. If the service isn't running, start it with backbone up --detach.".into(),
        _ => event
            .message
            .clone()
            .unwrap_or_else(|| "Setup said nothing more.".into()),
    })
}
/// What the person can do next, from the decisions that offer the
/// buttons and close the request, by what may follow the result.
fn next(s: &Snapshot, cancel_view: bool) -> &'static str {
    let event = if cancel_view {
        s.cancellation_result.as_ref()
    } else {
        s.first_result.as_ref()
    };
    if cancel_view && Command::RetryCancel.allowed(Some(s)) {
        "Try again: it sends the same cancellation."
    } else if cancel_view
        && Command::Retry.allowed(Some(s))
        && event.and_then(|e| e.code.as_deref()) == Some("cancellation_unsafe")
    {
        "Then send it again to learn what came of it, or forget it."
    } else if cancel_view && Command::Retry.allowed(Some(s)) {
        // Refused, or answered for another request: an earlier send of it
        // may have cancelled it.
        "The cancellation isn't confirmed: the request may still run, or may have been cancelled. Look at the agent first, then send it again to learn what came of it, or forget it."
    } else if Command::Retry.allowed(Some(s)) {
        "Try again: it sends the same request."
    } else if !s.closed() {
        "What may follow isn't one this app knows, so it is kept open and not sent again. Look at the workspace, or forget it."
    } else if event.and_then(|e| e.retry.as_deref()) == Some("new_request") {
        "Only a new request can follow: change what you need and start again."
    } else {
        "Not known what came of it: look at the workspace first."
    }
}
/// What the guide's main button does once a start has stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    /// Send the same request again.
    Replay,
    /// It is closed: start a new request.
    Fresh,
    /// It is open with nothing it may send: neither.
    Wait,
}
impl Then {
    fn label(self) -> Option<&'static str> {
        match self {
            Self::Replay => Some("Try again"),
            Self::Fresh => Some("Start a new request"),
            Self::Wait => None,
        }
    }
}
/// What may follow a guide's stopped start: the same request while it may
/// be sent again, a new one only once it is closed.
fn then(snapshot: Option<&Snapshot>) -> Then {
    match saved_open(snapshot) {
        Some(true) => Then::Replay,
        Some(false) => Then::Wait,
        None => Then::Fresh,
    }
}
/// The main button a stopped start shows, if any, and what pressing it
/// does. Add agent's is as it always was: Try again, the same start while
/// what came of it isn't known, else a new one.
fn main_button(
    served: Option<&Option<Answer>>,
    snapshot: Option<&Snapshot>,
) -> Option<(&'static str, Then)> {
    match served {
        Some(Some(Answer::Unknown(_))) => Some(("Try again", Then::Replay)),
        Some(_) => Some(("Try again", Then::Fresh)),
        None => {
            let after = then(snapshot);
            after.label().map(|label| (label, after))
        }
    }
}
/// Whether a saved request that is still open can be sent again; `None`
/// when it is closed, or none was saved.
fn saved_open(snapshot: Option<&Snapshot>) -> Option<bool> {
    snapshot
        .filter(|s| !s.closed())
        .map(|s| Command::Submit.allowed(Some(s)) || Command::Retry.allowed(Some(s)))
}
/// Whether a saved setup record can't be read, so it can only be forgotten.
fn unreadable(error: Error) -> bool {
    matches!(
        error,
        Error::Invalid | Error::Storage(journal::Error::Invalid | journal::Error::TooLarge)
    )
}

fn message(error: Error) -> &'static str {
    match error {
        Error::Missing => "The original saved request is missing. No replacement was created.",
        Error::Invalid | Error::Storage(journal::Error::Invalid) => {
            "This saved setup request could not be read. Forget it to add another agent."
        }
        Error::WrongPhase | Error::Completed => {
            "The saved request changed. Reload its original state."
        }
        Error::Pending => {
            "An earlier first-agent request in this profile is still open. Open Saved setup requests to finish, cancel or forget it first."
        }
        Error::RetryRefused => "Setup's last answer does not allow sending this again.",
        Error::Storage(journal::Error::Unsafe) => "Setup storage is not private or safe to use.",
        Error::Storage(journal::Error::Busy | journal::Error::Conflict) => {
            "Another update won. Reload the original request."
        }
        Error::Storage(journal::Error::TooLarge) => {
            "This saved setup request is too large, or setup storage is full. Forget it, or other requests, to continue."
        }
        Error::Storage(_) => "Saved setup state could not be confirmed. Reload before retrying.",
        Error::Client(_) => {
            "The outcome could not be confirmed. Keep and recover this original request."
        }
    }
}

enum Page {
    Loading,
    History {
        ids: Vec<String>,
        offset: usize,
        rows: Vec<(String, Result<Snapshot, Error>)>,
    },
    Operation {
        key: String,
        snapshot: Option<Box<Snapshot>>,
        progress: Option<Box<Progress>>,
    },
}
/// Go back to the choices: this request can't be tried again as it was.
pub(super) struct Again;
/// A first agent for the Rust service to start. Nothing is saved for it:
/// its request ID makes a repeat after a lost reply get the first result.
#[derive(Clone)]
pub(super) struct Request {
    pub profile: Profile,
    /// The profile's verified connection, when the caller has one; else it is
    /// made off the UI thread.
    pub connection: Option<Connection>,
    pub start: Start,
    /// A folder made for the agent, made first: the service starts only in
    /// one that exists.
    pub create: Option<PathBuf>,
    /// 32 lowercase hexadecimal digits, a UUIDv7.
    pub request_id: String,
}
struct Served {
    request: Request,
    /// What the last attempt came to, once it answered.
    answer: Option<Answer>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Answer {
    /// Running, by the name its pane opens.
    Started(String),
    /// The service answered that it didn't start, and why: another try is
    /// a new request.
    Stopped(String),
    /// No answer came, or the service doesn't know what came of it: the
    /// same request is asked again. A later try that couldn't be sent keeps
    /// it unknown, with why.
    Unknown(Option<String>),
}
/// One attempt of `request`, off the UI thread: connect when needed, make
/// its folder, then start it under its own request ID. `unresolved`: an
/// earlier send of it has no known result yet.
fn attempt(request: &Request, unresolved: bool) -> Answer {
    // Nothing is sent this time. A first try ends; an earlier send stays
    // unresolved, for the same request to ask about later.
    let unsent = |stopped: &str, why: &str| {
        if unresolved {
            Answer::Unknown(Some(why.into()))
        } else {
            Answer::Stopped(stopped.into())
        }
    };
    let connection = match request
        .connection
        .clone()
        .map_or_else(|| request.profile.connect(), Ok)
    {
        Ok(connection) => connection,
        Err(_) => {
            return unsent(
                "The profile could not be reached. Try again.",
                "The profile could not be reached.",
            );
        }
    };
    if let Some(folder) = &request.create
        && std::fs::create_dir_all(folder).is_err()
    {
        return unsent(
            "The agent's folder could not be made. Choose another folder.",
            "The agent's folder could not be made.",
        );
    }
    answer(connection.start_first_agent(&request.start, &request.request_id))
}
/// The heading once a start is over without running: a result that isn't
/// known is not called a failure.
fn heading(name: &str, answer: Option<&Answer>) -> String {
    match answer {
        Some(Answer::Unknown(_)) => format!("Not known whether {name} started"),
        _ => format!("{name} didn't start"),
    }
}
/// What a served start came to, for the person.
fn answer(result: Result<Started, AddError>) -> Answer {
    use agent_interface_roster::profiles::Error;
    match result {
        Ok(started) if started.ok => Answer::Started(started.name),
        Ok(started) => Answer::Stopped(
            started
                .evidence
                .first()
                .map(|line| line.chars().filter(|c| !c.is_control()).take(200).collect::<String>())
                .filter(|line| !line.trim().is_empty())
                .map_or_else(
                    || "The CLI exited right after starting, so the agent is not running. Run the CLI in a terminal in the agent's folder to see why, then start it again.".to_owned(),
                    |line| format!("It stopped right after starting: {line}"),
                ),
        ),
        Err(AddError::Refused(reason)) => Answer::Stopped(reason),
        Err(AddError::Connection(Error::Refused(status))) => {
            Answer::Stopped(format!("The profile refused the start ({status})."))
        }
        Err(AddError::Connection(
            Error::BindingRequired | Error::BindingMismatch | Error::Unsupported,
        )) => Answer::Stopped(
            "This profile's service changed. Go back and open the profile again.".into(),
        ),
        Err(AddError::Connection(_)) => Answer::Unknown(None),
    }
}
enum Update {
    Saved(Box<Snapshot>),
    Progress(Box<Progress>),
    Finished(Box<Result<Snapshot, Error>>, Option<Error>),
}
pub(super) struct Pane {
    controller: Option<Arc<Controller>>,
    page: Page,
    ticket: Ticket,
    busy: bool,
    finished: bool,
    /// Started from fresh choices: say only how it's going. Everything else
    /// stays in the saved requests.
    fresh: Option<&'static str>,
    error: Option<Error>,
    /// The one profile whose setups this lists, when shown for one profile.
    profile: Option<String>,
    served: Option<Served>,
    pub focus: FocusHandle,
}
impl EventEmitter<Back> for Pane {}
impl EventEmitter<Open> for Pane {}
impl EventEmitter<Again> for Pane {}
impl Pane {
    pub fn new(
        client: Client,
        directory: PathBuf,
        profile: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            controller: None,
            profile: profile.clone(),
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: true,
            finished: false,
            fresh: None,
            error: None,
            served: None,
            focus: cx.focus_handle(),
        };
        let ticket = pane.ticket.begin();
        let worker = ticket.clone();
        let work = cx.background_executor().spawn(async move {
            let controller = Arc::new(Controller::new(
                journal::Journal::open(&directory.join("profile-setup"))?,
                &client,
            ));
            let page = history(&controller, profile.as_deref(), None, 0, &worker);
            Ok::<_, Error>((controller, page))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if !pane.accepts(&ticket) {
                    return;
                }
                pane.busy = false;
                match result {
                    Ok((controller, page)) => {
                        pane.controller = Some(controller);
                        pane.complete(page);
                    }
                    Err(error) => pane.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        pane
    }
    pub fn from_controller(
        controller: Arc<Controller>,
        profile: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            controller: Some(controller),
            profile,
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: false,
            finished: false,
            fresh: None,
            error: None,
            served: None,
            focus: cx.focus_handle(),
        };
        let ticket = pane.ticket.begin();
        pane.history(0, &ticket, cx);
        pane
    }
    /// Start `prepared`. `leave` names the way out when it doesn't start.
    pub fn from_prepared(
        controller: Arc<Controller>,
        prepared: Prepared,
        leave: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        // Keep the original key before persistence: an enqueue sync failure can
        // already have saved intent, and must never allocate a replacement.
        let mut pane = Self {
            controller: Some(controller),
            profile: None,
            page: Page::Operation {
                key: Controller::key(prepared.request_id()),
                snapshot: None,
                progress: None,
            },
            ticket: Ticket::default(),
            busy: false,
            finished: false,
            fresh: Some(leave),
            error: None,
            served: None,
            focus: cx.focus_handle(),
        };
        pane.run(Command::Submit, Some(prepared), cx);
        pane
    }
    /// Start `request` with the Rust service. `leave` names the way out when
    /// it doesn't start.
    pub fn served(request: Request, leave: &'static str, cx: &mut Context<Self>) -> Self {
        let mut pane = Self {
            controller: None,
            profile: None,
            page: Page::Loading,
            ticket: Ticket::default(),
            busy: false,
            finished: false,
            fresh: Some(leave),
            error: None,
            served: Some(Served {
                request,
                answer: None,
            }),
            focus: cx.focus_handle(),
        };
        pane.run_served(cx);
        pane
    }
    fn run_served(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.finished {
            return;
        }
        let Some(served) = &mut self.served else {
            return;
        };
        let unresolved = matches!(served.answer, Some(Answer::Unknown(_)));
        served.answer = None;
        // A retry sends this same request, under its same ID.
        let request = served.request.clone();
        self.busy = true;
        let ticket = self.ticket.begin();
        // Leaving doesn't stop the start; only its answer is no longer shown.
        let work = cx
            .background_executor()
            .spawn(async move { attempt(&request, unresolved) });
        cx.spawn(async move |this, cx| {
            let answer = work.await;
            let _ = this.update(cx, |pane, cx| {
                if !pane.accepts(&ticket) {
                    return;
                }
                pane.busy = false;
                if let Some(served) = &mut pane.served {
                    served.answer = Some(answer);
                }
                if pane.running() {
                    pane.get_started(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn accepts(&self, ticket: &Arc<AtomicBool>) -> bool {
        !self.finished && self.ticket.accepts(ticket)
    }
    fn complete(&mut self, result: Result<Page, Error>) {
        self.busy = false;
        match result {
            Ok(page) => {
                self.page = page;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ticket.cancel();
        cx.emit(Back);
    }
    fn history(&mut self, offset: usize, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let ids = match &self.page {
            Page::History { ids, .. } if offset > 0 => Some(ids.clone()),
            _ => None,
        };
        self.busy = true;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let profile = self.profile.clone();
        let work = cx
            .background_executor()
            .spawn(async move { history(&controller, profile.as_deref(), ids, offset, &worker) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if pane.accepts(&ticket) {
                    pane.complete(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn open(&mut self, key: &str, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        self.page = Page::Operation {
            key: key.into(),
            snapshot: None,
            progress: None,
        };
        self.invoke(Command::Read, expected, cx);
    }
    fn invoke(&mut self, command: Command, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        let Page::Operation { snapshot, .. } = &self.page else {
            return;
        };
        if !command.allowed(snapshot.as_deref()) {
            return;
        }
        self.run(command, None, cx);
    }
    fn run(&mut self, command: Command, prepared: Option<Prepared>, cx: &mut Context<Self>) {
        let Page::Operation { key, .. } = &self.page else {
            return;
        };
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let key = key.clone();
        self.busy = true;
        self.error = None;
        if let Page::Operation { progress, .. } = &mut self.page {
            *progress = None;
        }
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        // The protocol bounds every invocation to 1024 events. This channel
        // carries only validated progress and one final durable observation.
        let (sender, mut receiver) = mpsc::unbounded();
        cx.background_executor()
            .spawn(async move {
                let live = || worker.load(Ordering::Acquire);
                let emit = |progress| {
                    let _ = sender.unbounded_send(Update::Progress(Box::new(progress)));
                };
                let admission = match prepared {
                    Some(prepared) => controller.enqueue(prepared).map(|snapshot| {
                        let _ = sender.unbounded_send(Update::Saved(Box::new(snapshot)));
                    }),
                    None => Ok(()),
                };
                let result = admission.and_then(|_| match command {
                    Command::Read => Ok(()),
                    Command::Submit => controller.submit(&key, &live, emit).map(|_| ()),
                    Command::Retry => controller.retry(&key, &live, emit).map(|_| ()),
                    Command::Cancel => controller.cancel(&key, &live, emit).map(|_| ()),
                    Command::RetryCancel => controller.retry_cancel(&key, &live, emit).map(|_| ()),
                });
                let _ = sender.unbounded_send(Update::Finished(
                    Box::new(controller.read(&key)),
                    result.err(),
                ));
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Some(update) = receiver.next().await {
                let accepted = this.update(cx, |pane, cx| {
                    if !pane.accepts(&ticket) {
                        return false;
                    }
                    let mut finished = false;
                    let Page::Operation {
                        key,
                        snapshot,
                        progress,
                    } = &mut pane.page
                    else {
                        return false;
                    };
                    match update {
                        Update::Saved(next) => {
                            if next.key == *key && snapshot.is_none() {
                                *snapshot = Some(next);
                            }
                        }
                        Update::Progress(next) => {
                            if progress_matches(
                                snapshot.as_deref(),
                                progress.as_deref(),
                                key,
                                &next,
                            ) {
                                *progress = Some(next);
                            }
                        }
                        Update::Finished(result, error) => {
                            pane.busy = false;
                            *progress = None;
                            match *result {
                                Ok(next) if next.key == *key => {
                                    *snapshot = Some(Box::new(next));
                                    pane.error = error;
                                    finished = true;
                                }
                                Ok(_) => {
                                    *snapshot = None;
                                    pane.error = Some(Error::Invalid);
                                }
                                Err(failure) => {
                                    *snapshot = None;
                                    pane.error = Some(error.unwrap_or(failure));
                                }
                            }
                        }
                    }
                    // A fresh start has no ready page: once the agent is up,
                    // its own pane is the next step.
                    if finished && pane.fresh.is_some() && pane.running() {
                        pane.get_started(cx);
                        cx.notify();
                        return false;
                    }
                    cx.notify();
                    true
                });
                if !matches!(accepted, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }
    fn forget(&mut self, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if self.busy || !self.accepts(expected) {
            return;
        }
        let Page::Operation { key, snapshot, .. } = &self.page else {
            return;
        };
        let Some(controller) = self.controller.clone() else {
            return;
        };
        let (key, revision) = (key.clone(), snapshot.as_ref().map(|s| s.revision));
        self.busy = true;
        self.error = None;
        let ticket = self.ticket.begin();
        let worker = ticket.clone();
        let profile = self.profile.clone();
        let work = cx.background_executor().spawn(async move {
            controller.forget(&key, revision)?;
            history(&controller, profile.as_deref(), None, 0, &worker)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                if pane.accepts(&ticket) {
                    pane.complete(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn stop_observing(&mut self, expected: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        if !self.busy || !self.accepts(expected) || !matches!(self.page, Page::Operation { .. }) {
            return;
        }
        // Interrupt observation, then read the same durable key. This is not a
        // cancellation receipt and never unlocks replacement setup choices.
        let ticket = self.ticket.begin();
        self.busy = false;
        self.invoke(Command::Read, &ticket, cx);
    }
}
fn progress_matches(
    snapshot: Option<&Snapshot>,
    previous: Option<&Progress>,
    key: &str,
    next: &Progress,
) -> bool {
    let Some(snapshot) = snapshot else {
        return false;
    };
    next.key == key
        && next.event.profile_id == snapshot.profile_id
        && next.event.request_id == snapshot.request_id
        && next.revision > snapshot.revision
        && previous.is_none_or(|old| {
            next.revision > old.revision
                || (next.revision == old.revision && next.event.sequence > old.event.sequence)
        })
}
fn history(
    controller: &Controller,
    profile: Option<&str>,
    ids: Option<Vec<String>>,
    offset: usize,
    live: &AtomicBool,
) -> Result<Page, Error> {
    let ids = match ids {
        Some(ids) => ids,
        None => controller
            .ids()?
            .into_iter()
            .filter(|key| {
                // A record that can't be read may be any profile's, and only
                // forgetting it unblocks setup, so every profile lists it.
                profile.is_none_or(|id| match controller.read(key) {
                    Ok(s) => s.profile_id == id,
                    Err(_) => true,
                })
            })
            .collect(),
    };
    if offset > ids.len() {
        return Err(Error::Invalid);
    }
    let mut rows = Vec::new();
    for key in ids.iter().skip(offset).take(PAGE_SIZE) {
        if !live.load(Ordering::Acquire) {
            return Err(Error::Client(crate::profiles::Error::Cancelled));
        }
        rows.push((key.clone(), controller.read(key)));
    }
    Ok(Page::History { ids, offset, rows })
}
impl Pane {
    /// Leave for the running agent.
    fn get_started(&mut self, cx: &mut Context<Self>) {
        if let Some(Served {
            request,
            answer: Some(Answer::Started(name)),
        }) = &self.served
        {
            let profile_id = request.profile.item().profile_id.clone();
            cx.emit(Open::Agent {
                profile_id,
                name: name.clone(),
            });
        } else if let Page::Operation {
            snapshot: Some(s), ..
        } = &self.page
        {
            let (profile_id, name) = (s.profile_id.clone(), s.agent_name.clone());
            cx.emit(Open::Agent { profile_id, name });
        }
        self.back(cx);
    }

    /// The agent is up, whatever it's showing.
    pub fn running(&self) -> bool {
        if let Some(served) = &self.served {
            return matches!(served.answer, Some(Answer::Started(_)));
        }
        let Page::Operation { snapshot, .. } = &self.page else {
            return false;
        };
        snapshot.as_deref().is_some_and(started)
    }

    /// A fresh start is still under way.
    pub fn starting(&self) -> bool {
        if let Some(served) = &self.served {
            return !self.finished && !self.running() && (self.busy || served.answer.is_none());
        }
        let Page::Operation { snapshot, .. } = &self.page else {
            return false;
        };
        !self.finished
            && !self.running()
            && (self.busy || (snapshot.is_none() && self.error.is_none()))
    }

    /// The step the provider last reported, such as `hooks` or `launch`.
    pub fn step(&self) -> Option<&str> {
        let Page::Operation { progress, .. } = &self.page else {
            return None;
        };
        progress.as_deref().and_then(|p| p.event.step.as_deref())
    }

    /// Why a fresh start stopped, in a sentence, once it has.
    pub fn failure(&self) -> Option<String> {
        if self.starting() || self.running() || self.finished {
            return None;
        }
        if let Some(served) = &self.served {
            return Some(match &served.answer {
                Some(Answer::Stopped(why)) => why.clone(),
                Some(Answer::Unknown(why)) => format!(
                    "{}What came of the start isn't known yet. Try again: it asks about the same start, so {} isn't added twice.",
                    why.as_ref()
                        .map_or_else(String::new, |why| format!("{why} ")),
                    served.request.start.name()
                ),
                _ => "It stopped before it was ready. Try again.".to_owned(),
            });
        }
        let Page::Operation { snapshot, .. } = &self.page else {
            return None;
        };
        snapshot
            .as_deref()
            .and_then(outcome)
            .map(|(text, code)| format!("{text} ({code})"))
            .or(self.error.map(|error| message(error).to_owned()))
            .or_else(|| Some("It stopped before it was ready. Try again.".to_owned()))
    }

    /// The saved request this start made, while it is open: whether it can
    /// be sent again. `None` when none is open, so new choices may make a
    /// new request.
    pub fn saved_open(&self) -> Option<bool> {
        let Page::Operation { snapshot, .. } = &self.page else {
            return None;
        };
        saved_open(snapshot.as_deref())
    }

    /// What the guide's main button says once a start has stopped: none
    /// while its request is open with nothing it may send.
    pub fn again_label(&self) -> Option<&'static str> {
        let snapshot = match &self.page {
            Page::Operation { snapshot, .. } => snapshot.as_deref(),
            _ => None,
        };
        main_button(self.served.as_ref().map(|s| &s.answer), snapshot).map(|(label, _)| label)
    }

    /// Once a fresh start has stopped without its agent running, the revision
    /// of its saved request, 0 when none was saved: each result changes it.
    pub fn settled(&self) -> Option<u64> {
        self.failure()?;
        let Page::Operation { snapshot, .. } = &self.page else {
            return None;
        };
        Some(snapshot.as_ref().map_or(0, |s| s.revision))
    }

    /// Try the same request again, when the provider allows it. Returns false
    /// when only a new request can go on.
    pub fn retry(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(served) = &self.served {
            if !matches!(served.answer, Some(Answer::Unknown(_))) {
                return false;
            }
            self.run_served(cx);
            return true;
        }
        let Page::Operation { snapshot, .. } = &self.page else {
            return false;
        };
        // Saved but never sent: send it now.
        let command = [Command::Submit, Command::Retry]
            .into_iter()
            .find(|command| command.allowed(snapshot.as_deref()));
        let Some(command) = command else {
            return false;
        };
        let ticket = self.ticket.0.clone();
        self.invoke(command, &ticket, cx);
        true
    }

    /// What a person starting an agent needs: it's starting, or it didn't
    /// start and what to do next. Once it runs, its pane opens instead.
    fn progress(
        &self,
        leave: &'static str,
        snapshot: Option<&Snapshot>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (t, c) = (tokens(), colors());
        let meta = |text: String| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let name = match &self.served {
            Some(served) => served.request.start.name(),
            None => snapshot.map_or("Your agent", |s| s.agent_name.as_str()),
        };
        let starting = self.starting() || self.running();
        let (step, title, note) = if starting {
            (
                ui::Step::Now,
                format!("Starting {name}…"),
                // The service reports no steps: it is opening the agent.
                Some(stage(
                    self.served.as_ref().map_or(self.step(), |_| Some("launch")),
                    name,
                )),
            )
        } else {
            (
                ui::Step::Waits,
                heading(name, self.served.as_ref().and_then(|s| s.answer.as_ref())),
                self.failure(),
            )
        };
        let mut actions = div().flex().items_center().gap(px(t.space.s8));
        if !starting {
            // The same request while it may be sent again; a new one only
            // once it is closed, under the text that says to look first.
            if let Some((label, after)) =
                main_button(self.served.as_ref().map(|s| &s.answer), snapshot)
            {
                actions = actions.child(
                    ui::button("setup-try-again", label, ui::Button::Primary).on_click(
                        cx.listener(move |pane, _, _, cx| {
                            if after == Then::Fresh {
                                pane.finished = true;
                                cx.emit(Again);
                            } else if pane.served.is_some() {
                                pane.run_served(cx);
                            } else {
                                pane.retry(cx);
                            }
                        }),
                    ),
                );
            }
            actions = actions.child(
                ui::button("setup-leave", leave, ui::Button::Secondary)
                    .on_click(cx.listener(|pane, _, _, cx| pane.back(cx))),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(t.space.s10))
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(move |pane, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    pane.back(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(ui::step_dot(step))
                    .child(div().text_color(c.text).child(title)),
            )
            .children(note.map(meta))
            .child(actions)
            .into_any_element()
    }
}

/// What a start is doing, from the step the provider last reported.
pub(super) fn stage(step: Option<&str>, name: &str) -> String {
    match step {
        Some("agent") => format!("Creating {name}…"),
        Some("launch" | "ready") => "Opening its terminal…".to_owned(),
        _ => "Checking the tool…".to_owned(),
    }
}
impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let (Some(leave), Some(_)) = (self.fresh, &self.served) {
            return self.progress(leave, None, cx);
        }
        if let (Some(leave), Page::Operation { snapshot, .. }) = (self.fresh, &self.page) {
            let snapshot = snapshot.as_deref().cloned();
            return self.progress(leave, snapshot.as_ref(), cx);
        }
        let (t, c) = (tokens(), colors());
        let ticket = self.ticket.0.clone();
        let mut body = div()
            .id("setup-recovery-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(t.space.s12))
            .flex()
            .flex_col()
            .gap(px(t.space.s8));
        match &self.page {
            Page::Loading => body = body.child(ui::group_note("Opening saved setup requests…")),
            Page::History { ids, offset, rows } => {
                if rows.is_empty() {
                    body = body.child(ui::group_note("No saved agent setup requests."));
                }
                for (key, result) in rows {
                    let label = match result {
                        Ok(s) => format!("{} · {}", s.agent_name, status(s)),
                        Err(e) => message(*e).into(),
                    };
                    let key = key.clone();
                    let ticket = ticket.clone();
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("setup-{key}")))
                            .w_full()
                            .min_w_0()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .p(px(t.space.s12))
                            .gap(px(t.space.s8))
                            .bg(c.field)
                            .rounded(px(t.radius.row))
                            .cursor_pointer()
                            .hover(|row| row.bg(colors().hover))
                            .child(div().w_full().whitespace_normal().child(label))
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(format!("Operation {key}")),
                            )
                            .on_click(
                                cx.listener(move |pane, _, _, cx| pane.open(&key, &ticket, cx)),
                            ),
                    );
                }
                if !self.busy {
                    for (offset, label) in [
                        (offset.checked_sub(PAGE_SIZE), "Previous"),
                        (
                            (offset + PAGE_SIZE < ids.len()).then_some(offset + PAGE_SIZE),
                            "Next",
                        ),
                    ] {
                        if let Some(offset) = offset {
                            let ticket = ticket.clone();
                            body = body.child(
                                ui::button(label, label, ui::Button::Secondary).on_click(
                                    cx.listener(move |pane, _, _, cx| {
                                        pane.history(offset, &ticket, cx)
                                    }),
                                ),
                            );
                        }
                    }
                }
            }
            Page::Operation {
                snapshot, progress, ..
            } => {
                if let Some(s) = snapshot {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s8))
                            .child(if started(s) {
                                ui::step_dot(ui::Step::Done)
                            } else if s.closed() || outcome(s).is_some() {
                                ui::step_dot(ui::Step::Waits)
                            } else {
                                ui::step_dot(ui::Step::Now)
                            })
                            .child(div().text_color(c.text).child(s.agent_name.clone()))
                            .child(div().text_color(c.muted).child(status(s))),
                    );
                    if let Some((text, code)) = outcome(s) {
                        body = body
                            .child(ui::group_note(text))
                            .child(ui::text(div().text_color(c.muted), &t.text.meta).child(code));
                    }
                    if !self.busy && started(s) {
                        body = body.child(ui::group_note(
                            "Its terminal may ask you to sign in, trust the folder or accept terms first.",
                        ));
                        let (agent, sign_in) = (
                            Open::Agent {
                                profile_id: s.profile_id.clone(),
                                name: s.agent_name.clone(),
                            },
                            s.runtime.clone().map(|runtime| Open::SignIn {
                                profile_id: s.profile_id.clone(),
                                runtime,
                            }),
                        );
                        body = body.child(
                            ui::button("setup-open-agent", "Open agent", ui::Button::Primary)
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(agent.clone()))),
                        );
                        if let Some(sign_in) = sign_in {
                            body = body.child(
                                ui::button("setup-sign-in", "Sign in", ui::Button::Secondary)
                                    .on_click(
                                        cx.listener(move |_, _, _, cx| cx.emit(sign_in.clone())),
                                    ),
                            );
                        }
                    }
                }
                if progress.is_some() || self.busy {
                    body = body.child(ui::group_note(
                        "Starting… Leaving doesn't stop it; it finishes on its own.",
                    ));
                }
                if self.busy {
                    let ticket = ticket.clone();
                    body = body.child(
                        ui::button(
                            "setup-stop-observing",
                            "Stop waiting and reload",
                            ui::Button::Secondary,
                        )
                        .on_click(
                            cx.listener(move |pane, _, _, cx| pane.stop_observing(&ticket, cx)),
                        ),
                    );
                } else {
                    for (command, label) in [
                        (Command::Read, "Reload saved state"),
                        (Command::Submit, "Submit saved setup"),
                        (Command::Retry, "Retry identical setup"),
                        (Command::Cancel, "Cancel original setup"),
                        (Command::RetryCancel, "Retry identical cancellation"),
                    ] {
                        if command.allowed(snapshot.as_deref()) {
                            let ticket = ticket.clone();
                            body = body.child(
                                ui::button(
                                    label,
                                    label,
                                    if matches!(command, Command::Cancel | Command::RetryCancel) {
                                        ui::Button::Danger
                                    } else {
                                        ui::Button::Secondary
                                    },
                                )
                                .on_click(cx.listener(
                                    move |pane, _, _, cx| pane.invoke(command, &ticket, cx),
                                )),
                            );
                        }
                    }
                    if snapshot.is_some() || self.error.is_some_and(unreadable) {
                        let ticket = ticket.clone();
                        body = body
                            .child(ui::group_note("Forgetting removes this saved request from the app. It does not stop or remove an agent."))
                            .child(
                                ui::button("setup-forget", "Forget this request", ui::Button::Secondary)
                                    .on_click(cx.listener(move |pane, _, _, cx| pane.forget(&ticket, cx))),
                            );
                    }
                }
            }
        }
        if let Some(error) = self.error {
            body = body.child(ui::group_note(message(error)));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|pane, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    pane.back(cx);
                    cx.stop_propagation();
                }
            }))
            .child(body)
            .child(
                ui::view_toolbar()
                    .px(px(t.space.s12))
                    .gap(px(t.space.s8))
                    .child(
                        ui::button("setup-back", "Back", ui::Button::Secondary)
                            .on_click(cx.listener(|pane, _, _, cx| pane.back(cx))),
                    )
                    .when(self.controller.is_some() && !self.busy, |bar| {
                        bar.child(
                            ui::button(
                                "setup-history",
                                "Saved setup requests",
                                ui::Button::Secondary,
                            )
                            .on_click(
                                cx.listener(move |pane, _, _, cx| pane.history(0, &ticket, cx)),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
pub(super) mod tests;
