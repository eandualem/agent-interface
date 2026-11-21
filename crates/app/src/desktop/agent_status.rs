//! Connection-owned live rosters. Views retain their originating observations;
//! authored reports and historical timelines never write into current state.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};

use agent_interface_roster::{
    Snapshot,
    profiles::{
        Connection, ConnectionKey, Identity,
        runs::{Run, Runs},
    },
};
use futures::{StreamExt, channel::mpsc};
use gpui::{App, BorrowAppContext, Global, Task};

/// How an agent is doing, as Agent Backbone reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    NeedsYou,
    Blocked,
    Working,
    Starting,
    Idle,
    Offline,
    Unavailable,
    Unknown,
}

impl AgentState {
    /// Preserve reported states while sharing their visual categories.
    pub fn from_runtime(state: &str) -> Self {
        match state {
            "waiting_for_human" => Self::NeedsYou,
            "blocked" => Self::Blocked,
            "busy" => Self::Working,
            "starting" => Self::Starting,
            "idle" | "running" => Self::Idle,
            "offline" => Self::Offline,
            _ => Self::Unknown,
        }
    }

    pub fn online(self) -> bool {
        matches!(
            self,
            Self::Working | Self::Starting | Self::NeedsYou | Self::Blocked | Self::Idle
        )
    }

    pub fn needs_you(self) -> bool {
        matches!(self, Self::NeedsYou | Self::Blocked)
    }

    pub fn working(self) -> bool {
        matches!(self, Self::Working | Self::Starting)
    }

    /// The short word shown beside a working or needs-you agent.
    pub fn word(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs you",
            Self::Blocked => "blocked",
            Self::Working => "working",
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Offline => "offline",
            Self::Unavailable => "status unavailable",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Status {
    pub state: AgentState,
    pub online: bool,
    pub label: String,
    pub since: Option<f64>,
    pub reason: Option<String>,
    /// What it waits on and how many runs it has, when the service says.
    pub line: Option<String>,
    /// Its held or waiting deliveries, when the service says any wait.
    pub deliveries: Option<(String, Option<&'static str>)>,
    /// Its own restart while it is under way.
    pub restart: Option<agent_interface_roster::Restart>,
}

impl Status {
    pub fn new(state: AgentState) -> Self {
        Self {
            state,
            online: state.online(),
            label: state.word().into(),
            since: None,
            reason: None,
            line: None,
            deliveries: None,
            restart: None,
        }
    }

    pub fn rank(&self) -> u8 {
        match self.state {
            AgentState::NeedsYou | AgentState::Blocked => 0,
            AgentState::Working | AgentState::Starting => 1,
            AgentState::Offline => 3,
            _ => 2,
        }
    }
}

/// An agent's active runs, as its sidebar row shows them.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Activity {
    /// Each active review: who reviews, and its state as written.
    pub reviews: Vec<(String, String)>,
    /// Its other active runs in a few words, such as `build running`.
    pub runs: Option<String>,
}

struct Current {
    snapshot: Option<Snapshot>,
    statuses: HashMap<String, Rc<Status>>,
    active: Option<Rc<HashMap<String, AgentState>>>,
    unknown: Rc<Status>,
    unavailable: Rc<Status>,
    /// Background runs from the profile's book.
    runs: Runs,
    /// Active runs by the name of the agent that started them.
    activity: HashMap<String, Rc<Activity>>,
    /// The profile's running and queued runs together, when it has any.
    background: Option<gpui::SharedString>,
}

impl Current {
    /// Whether the roster is reported unavailable, as the status bar totals
    /// and every state then show it.
    fn reported_unavailable(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.error.is_some())
    }
}

impl Default for Current {
    fn default() -> Self {
        Self {
            snapshot: None,
            statuses: HashMap::new(),
            active: None,
            unknown: Rc::new(Status::new(AgentState::Unknown)),
            unavailable: Rc::new(Status::new(AgentState::Unavailable)),
            runs: Runs::default(),
            activity: HashMap::new(),
            background: None,
        }
    }
}

/// The non-zero ones of `{n} running` and `{n} queued`, joined by `separator`.
pub(super) fn counted(running: u64, queued: u64, separator: &str) -> Option<String> {
    let parts: Vec<String> = [(running, "running"), (queued, "queued")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect();
    (!parts.is_empty()).then(|| parts.join(separator))
}

/// The line under an agent's state: what it waits on, then its run counts.
fn waits_and_runs(details: &agent_interface_roster::AgentDetails) -> Option<String> {
    let waiting = details.waiting.as_ref().map(|waiting| {
        let agent = waiting.on_agent.as_ref().map(|agent| format!(" {agent}"));
        format!("waits on {}{}", waiting.on, agent.unwrap_or_default())
    });
    let runs = details
        .background
        .and_then(|counts| counted(counts.running, counts.queued, ", "));
    match (waiting, runs) {
        (Some(waiting), Some(runs)) => Some(format!("{waiting} · {runs}")),
        (waiting, runs) => waiting.or(runs),
    }
}

/// What holds an agent's terminal deliveries and how many wait, or `None`
/// when nothing waits. `delivery` is whether a delivery could go to it now,
/// as a code, which only an agent's inspection carries. The service sends changes within one of
/// its agent-monitor passes, so this can trail the queue by about a minute.
pub(super) fn deliveries(
    held: Option<&agent_interface_roster::HeldQueue>,
    queued: u64,
    delivery: Option<&str>,
) -> Option<String> {
    let waiting = (queued > 0).then(|| format!("{queued} waiting"));
    let Some(held) = held else {
        // Only codes are matched, never the service's sentences; an
        // unknown one gives no reason.
        let why = match delivery {
            Some("settling") => " · it has just gone idle",
            Some("human_typing") => " · someone is typing",
            Some("human_reading") => " · someone is reading its terminal",
            Some("unattended_copy_mode") => " · its terminal is in copy mode",
            Some("agent_working") => " · it is working",
            Some("waiting_for_human") => " · it is waiting for a person",
            Some("offline") => " · it is offline",
            _ => "",
        };
        return waiting.map(|waiting| format!("{waiting}{why}"));
    };
    // The held one waits too.
    let waiting = queued.max(1);
    Some(match held.reason.as_deref() {
        Some("uncertain_delivery") => {
            format!(
                "Deliveries held: a message may still be in the original terminal · {waiting} waiting"
            )
        }
        _ => format!("Deliveries are held · {waiting} waiting"),
    })
}

/// As [`deliveries`], for a navigation row: the count, and apart from it
/// why they are held, so a narrow row can cut the reason, never the count.
pub(super) fn row_deliveries(
    held: Option<&agent_interface_roster::HeldQueue>,
    queued: u64,
) -> Option<(String, Option<&'static str>)> {
    let Some(held) = held else {
        return (queued > 0).then(|| (format!("{queued} waiting"), None));
    };
    let why = match held.reason.as_deref() {
        Some("uncertain_delivery") => {
            "deliveries held: a message may still be in the original terminal"
        }
        _ => "deliveries are held",
    };
    // The held one waits too.
    Some((format!("{} waiting", queued.max(1)), Some(why)))
}

/// Who a review asked: its model, else its CLI, else the run's label, then
/// its effort when named.
fn reviewer(run: &Run) -> String {
    let review = run.review.as_ref();
    let who = review
        .and_then(|review| review.model.clone().or_else(|| review.runtime.clone()))
        .unwrap_or_else(|| run.label.clone());
    match review.and_then(|review| review.effort.as_ref()) {
        Some(effort) => format!("{who} {effort}"),
        None => who,
    }
}

/// Each agent's active runs: its reviews one by one, the rest in a few words.
fn activity(runs: &Runs) -> HashMap<String, Rc<Activity>> {
    let mut started: HashMap<&str, Vec<&Run>> = HashMap::new();
    // A stale list is no longer what runs.
    for run in runs.active.iter().filter(|_| !runs.stale) {
        if let Some(agent) = run.agent.as_deref() {
            started.entry(agent).or_default().push(run);
        }
    }
    started
        .into_iter()
        .map(|(agent, runs)| {
            let (reviews, others): (Vec<&Run>, Vec<&Run>) =
                runs.into_iter().partition(|run| run.kind == "review");
            let count = |state: &str| others.iter().filter(|run| run.state == state).count();
            let runs = match others.as_slice() {
                [] => None,
                [run] => Some(format!("{} {}", run.kind, run.state)),
                _ => counted(count("running") as u64, count("queued") as u64, ", "),
            };
            let reviews = reviews
                .into_iter()
                .map(|run| (reviewer(run), run.state.clone()))
                .collect();
            (agent.to_owned(), Rc::new(Activity { reviews, runs }))
        })
        .collect()
}

/// How long failed reads keep showing the last roster before it is reported
/// as unavailable.
pub(super) const TOLERANCE: std::time::Duration = std::time::Duration::from_secs(30);

/// Clones share the same current observation, including across native windows.
#[derive(Clone, Default)]
pub(super) struct LiveAgents(Rc<Source>);

impl LiveAgents {
    pub fn get(&self, name: &str) -> Rc<Status> {
        let current = self.0.current.borrow();
        if name.is_empty() {
            return current.unknown.clone();
        }
        if current
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.error.is_some())
        {
            return current.unavailable.clone();
        }
        current
            .statuses
            .get(name)
            .cloned()
            .unwrap_or_else(|| current.unknown.clone())
    }

    /// Whether `name` is missing from a roster that was read: its newest
    /// read succeeded, is still fresh, and does not list it. Not so before
    /// any read, while the roster cannot be read, or from a cached read
    /// past its tolerance.
    pub fn absent(&self, name: &str) -> bool {
        let current = self.0.current.borrow();
        current.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.error.is_none()
                && snapshot
                    .observed_at
                    .is_some_and(|read| read.elapsed() < TOLERANCE)
                && !snapshot.states.contains_key(name)
        })
    }

    /// The folders the newest observation's agents work in.
    pub fn folders(&self) -> Vec<std::path::PathBuf> {
        let current = self.0.current.borrow();
        let Some(snapshot) = &current.snapshot else {
            return Vec::new();
        };
        snapshot
            .details
            .values()
            .filter_map(|details| details.dir.as_deref())
            .filter(|dir| !dir.is_empty())
            .map(std::path::PathBuf::from)
            .collect()
    }

    pub fn active(&self) -> Option<Rc<HashMap<String, AgentState>>> {
        self.0.current.borrow().active.clone()
    }

    /// Running swarms from the newest observation.
    pub fn swarms(&self) -> Vec<agent_interface_roster::Swarm> {
        let current = self.0.current.borrow();
        current
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.swarms.clone())
            .unwrap_or_default()
    }

    /// Running swarms when the newest observation read them; `None` while
    /// that answer is unknown, so a failed read never looks like a finish.
    pub fn read_swarms(&self) -> Option<Vec<agent_interface_roster::Swarm>> {
        let current = self.0.current.borrow();
        let snapshot = current.snapshot.as_ref()?;
        (snapshot.error.is_none() && snapshot.swarms_read).then(|| snapshot.swarms.clone())
    }

    pub fn observed(&self) -> bool {
        self.0.current.borrow().snapshot.is_some()
    }

    /// When the roster shown was read, if it has been.
    pub fn observed_at(&self) -> Option<std::time::Instant> {
        self.0
            .current
            .borrow()
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.observed_at)
    }

    /// The background runs of the agent with this registration. While the
    /// roster is reported unavailable, its active runs are not shown as
    /// current; those that ended stay.
    pub fn runs_of(&self, registration: &str) -> Runs {
        let current = self.0.current.borrow();
        let mut runs = current.runs.of(registration);
        if current.reported_unavailable() {
            runs.active.clear();
        }
        runs
    }

    /// The active runs of the agent with this name, when it has any and the
    /// roster is not reported unavailable.
    pub fn activity(&self, name: &str) -> Option<Rc<Activity>> {
        let current = self.0.current.borrow();
        if current.reported_unavailable() {
            return None;
        }
        current.activity.get(name).cloned()
    }

    /// Agents of the roster that are offline but still have active runs;
    /// none while the roster is reported unavailable.
    pub fn offline_with_runs(&self) -> Vec<String> {
        let current = self.0.current.borrow();
        if current.reported_unavailable() {
            return Vec::new();
        }
        current
            .activity
            .keys()
            .filter(|name| {
                current
                    .statuses
                    .get(*name)
                    .is_some_and(|status| status.state == AgentState::Offline)
            })
            .cloned()
            .collect()
    }

    /// The profile's running and queued runs together, as `1 running · 2
    /// queued`; none when it has none or its roster is unavailable.
    pub fn background(&self) -> Option<gpui::SharedString> {
        self.0.current.borrow().background.clone()
    }

    /// A missed read is not yet an outage: the last roster stays shown while
    /// the worker retries, so a slow provider does not flicker.
    pub fn update(&self, mut snapshot: Snapshot) -> bool {
        if snapshot
            .observed_at
            .is_some_and(|read| read.elapsed() < TOLERANCE)
        {
            // A failed read carries the last good roster, which may be newer
            // than the one shown when an unread success was replaced.
            snapshot.error = None;
        }
        self.replace(snapshot)
    }

    fn replace(&self, mut snapshot: Snapshot) -> bool {
        // For online agents Backbone supplies the read time, not the time of
        // a state transition. Only offline timestamps have presentation meaning.
        // Normalizing here also preserves retained painting on unchanged polls.
        for (name, details) in &mut snapshot.details {
            if snapshot
                .states
                .get(name)
                .is_none_or(|state| state != "offline")
            {
                details.state_since = None;
            }
        }
        let mut current = self.0.current.borrow_mut();
        let changed = current.snapshot.as_ref().is_none_or(|old| {
            old.states != snapshot.states
                || old.details != snapshot.details
                || old.swarms != snapshot.swarms
                || old.error != snapshot.error
        });
        if changed {
            current.statuses = snapshot
                .states
                .iter()
                .map(|(name, value)| {
                    let mut status = Status::new(AgentState::from_runtime(value));
                    if let Some(details) = snapshot.details.get(name) {
                        status.since = details.state_since;
                        if status.state == AgentState::Unknown {
                            status.online = details.online.unwrap_or(false);
                        }
                        status.reason = details.reason.clone();
                        let suffix = if status.state.needs_you() {
                            details.reason.clone()
                        } else if status.state.working() {
                            details.current_issue.map(|issue| format!("#{issue}"))
                        } else {
                            None
                        };
                        if let Some(suffix) = suffix.filter(|s| !s.is_empty()) {
                            status.label = format!("{} · {suffix}", status.state.word());
                        }
                        status.line = waits_and_runs(details);
                        status.deliveries =
                            row_deliveries(details.held_queue.as_ref(), details.queued);
                        status.restart = details.restart.clone();
                    }
                    (name.clone(), Rc::new(status))
                })
                .collect();
            let (running, queued) = snapshot
                .details
                .values()
                .filter_map(|details| details.background)
                .fold((0u64, 0u64), |(running, queued), counts| {
                    (
                        running.saturating_add(counts.running),
                        queued.saturating_add(counts.queued),
                    )
                });
            current.background = counted(running, queued, " · ")
                .filter(|_| snapshot.error.is_none())
                .map(Into::into);
            current.active = snapshot.error.is_none().then(|| {
                Rc::new(
                    current
                        .statuses
                        .iter()
                        .filter(|(_, status)| status.state != AgentState::Offline)
                        .map(|(name, status)| (name.clone(), status.state))
                        .collect(),
                )
            });
        }
        current.snapshot = Some(snapshot);
        changed
    }
}

/// Windows and their child views own sources. The registry only deduplicates
/// live owners, so closing the last view releases its poller and cached state.
/// Keys are the provider's identity and the connection, never the selected view
/// or an agent's name.
#[derive(Default)]
pub(super) struct Sources(HashMap<(Identity, ConnectionKey), Weak<Source>>);
impl Global for Sources {}

impl Sources {
    fn get(&mut self, origin: &Identity, key: &ConnectionKey) -> Option<LiveAgents> {
        self.0.retain(|_, source| source.strong_count() > 0);
        self.0
            .get(&(origin.clone(), key.clone()))
            .and_then(Weak::upgrade)
            .map(LiveAgents)
    }

    fn insert(&mut self, agents: &LiveAgents) {
        if let (Some(origin), Some(key)) = (&agents.0.origin, &agents.0.connection_key) {
            self.0
                .insert((origin.clone(), key.clone()), Rc::downgrade(&agents.0));
        }
    }
}

/// Joining a bounded in-flight request must never block the UI thread.
#[derive(Default)]
struct Roster(Option<crate::roster::BoundRosterSource>);
impl Drop for Roster {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            std::thread::spawn(move || drop(worker));
        }
    }
}

#[derive(Default)]
struct Source {
    origin: Option<Identity>,
    /// Like `origin`, None only for an empty `LiveAgents::default()`, which no
    /// registry holds.
    connection_key: Option<ConnectionKey>,
    current: RefCell<Current>,
    roster: Roster,
    updates: RefCell<Option<Task<()>>>,
}

impl LiveAgents {
    pub fn belongs_to(&self, identity: &Identity) -> bool {
        self.0.origin.as_ref() == Some(identity)
    }

    fn update_bound(&self, mut update: crate::roster::BoundRosterUpdate) -> bool {
        if self.0.origin.as_ref() != Some(&update.identity) {
            return false;
        }
        let runs = {
            let mut current = self.0.current.borrow_mut();
            let changed = current.runs != update.runs;
            if changed {
                current.activity = activity(&update.runs);
                current.runs = update.runs;
            }
            changed
        };
        let roster = match update.error {
            None => self.update(update.snapshot),
            Some(error) => {
                update.snapshot.unavailable();
                // Only a read that could not be made is retried quietly. A
                // refusal is the provider's answer and is shown at once.
                if error == agent_interface_roster::profiles::Error::Unavailable {
                    self.update(update.snapshot)
                } else {
                    self.replace(update.snapshot)
                }
            }
        };
        runs || roster
    }

    fn take_update(&self) -> Option<bool> {
        let source = self.0.roster.0.as_ref()?;
        source.take_update().map(|update| self.update_bound(update))
    }
}

/// Only an already-verified connection can start a bound source. Selection and
/// workspace persistence are not enabled by this ownership primitive.
pub(super) fn start_bound(connection: Connection, cx: &mut App) -> Option<LiveAgents> {
    let origin = connection.identity().clone();
    let connection_key = connection.connection_key();
    if !cx.has_global::<Sources>() {
        cx.set_global(Sources::default());
    }
    if let Some(agents) =
        cx.update_global::<Sources, _>(|sources, _| sources.get(&origin, &connection_key))
    {
        return Some(agents);
    }
    let (wake, mut woken) = mpsc::unbounded();
    let roster = crate::roster::BoundRosterSource::start(connection, move || {
        let _ = wake.unbounded_send(());
    })
    .ok()?;
    let agents = LiveAgents(Rc::new(Source {
        origin: Some(origin),
        connection_key: Some(connection_key),
        roster: Roster(Some(roster)),
        ..Source::default()
    }));
    cx.update_global::<Sources, _>(|sources, _| sources.insert(&agents));
    // A delayed wake upgrades its original owner. It can never resolve a
    // newly selected source, or keep a closed window's source alive itself.
    let source = Rc::downgrade(&agents.0);
    let updates = cx.spawn(async move |cx| {
        while woken.next().await.is_some() {
            cx.update(|cx| {
                let Some(source) = source.upgrade() else {
                    return;
                };
                if let Some(changed) = LiveAgents(source).take_update() {
                    // Notify even on unchanged status: each window's observer
                    // tries its pending swarm-tab closures again, which may
                    // have waited for a save or edit to finish.
                    cx.update_global::<Sources, _>(|_, _| {});
                    if changed {
                        cx.refresh_windows();
                    }
                }
            });
        }
    });
    *agents.0.updates.borrow_mut() = Some(updates);
    Some(agents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumers_share_observations_through_stop_failure_and_recovery() {
        let first = LiveAgents::default();
        let second = first.clone();
        assert_eq!(first.get("worker").state, AgentState::Unknown);
        let mut snapshot = Snapshot::default();
        snapshot.update(
            br#"{"items":[{"name":"worker","state":"busy","current_issue":7,"state_since":10}]}"#,
        );
        assert!(first.update(snapshot.clone()));
        assert!(Rc::ptr_eq(&first.get("worker"), &second.get("worker")));
        assert_eq!(second.get("worker").label, "working · #7");
        let unchanged = first.get("worker");
        snapshot.update(
            br#"{"items":[{"name":"worker","state":"busy","current_issue":7,"state_since":20}]}"#,
        );
        assert!(!first.update(snapshot.clone()));
        assert!(Rc::ptr_eq(&unchanged, &second.get("worker")));

        snapshot.update(br#"{"items":[{"name":"worker","state":"idle"}]}"#);
        first.update(snapshot.clone());
        assert_eq!(second.get("worker").state, AgentState::Idle);
        snapshot.update(br#"{"items":[{"name":"worker","state":"offline","current_issue":7}]}"#);
        first.update(snapshot.clone());
        assert_eq!(second.get("worker").label, "offline");
        assert!(second.active().unwrap().is_empty());

        snapshot.update(br#"{"items":[{"name":"worker","state":"busy"}]}"#);
        first.update(snapshot.clone());
        snapshot.unavailable();
        // A failure soon after a good read keeps the last roster.
        assert!(!first.update(snapshot.clone()));
        assert_eq!(second.get("worker").state, AgentState::Working);
        // A consumer that never saw that good read takes it from the failure.
        let late = LiveAgents::default();
        late.update(snapshot.clone());
        assert_eq!(late.get("worker").state, AgentState::Working);
        snapshot.observed_at = std::time::Instant::now().checked_sub(TOLERANCE);
        assert!(snapshot.observed_at.is_some());
        first.update(snapshot.clone());
        assert_eq!(second.get("worker").state, AgentState::Unavailable);
        assert_eq!(second.get("").state, AgentState::Unknown);
        assert!(!second.get("worker").state.working());
        assert!(second.active().is_none());
        snapshot.update(br#"{"items":[]}"#);
        first.update(snapshot);
        assert_eq!(second.get("worker").state, AgentState::Unknown);
        assert!(second.active().unwrap().is_empty());
    }

    #[test]
    fn labels_and_attention_preserve_measured_runtime_states() {
        let agents = LiveAgents::default();
        let mut snapshot = Snapshot::default();
        snapshot.update(
            br#"{"items":[
            {"name":"waiter","state":"waiting_for_human","reason":"permission"},
            {"name":"blocked","state":"blocked","reason":"quota"},
            {"name":"starter","state":"starting"},
            {"name":"future","state":"new-state","online":true}
        ]}"#,
        );
        agents.update(snapshot);
        assert_eq!(agents.get("waiter").label, "needs you · permission");
        assert_eq!(agents.get("blocked").label, "blocked · quota");
        assert_eq!(agents.get("starter").state, AgentState::Starting);
        assert!(agents.get("starter").state.working());
        assert_eq!(agents.get("future").state, AgentState::Unknown);
        assert!(agents.get("future").online);
    }

    fn origin(profile: &str, lifecycle: &str) -> Identity {
        Identity {
            profile_id: profile.into(),
            installation_id: "installation".into(),
            lifecycle_id: lifecycle.into(),
        }
    }

    /// One verified connection's key, for the tests that key sources by origin
    /// and connection.
    fn key() -> ConnectionKey {
        static KEY: std::sync::LazyLock<ConnectionKey> = std::sync::LazyLock::new(|| {
            let (_directory, connection, server) =
                crate::roster::content::tests::provider("a", vec![]);
            server.join().unwrap();
            connection.connection_key()
        });
        KEY.clone()
    }

    fn acquire(sources: &mut Sources, origin: Identity) -> LiveAgents {
        acquire_key(sources, origin, key())
    }

    fn acquire_key(
        sources: &mut Sources,
        origin: Identity,
        connection_key: ConnectionKey,
    ) -> LiveAgents {
        if let Some(agents) = sources.get(&origin, &connection_key) {
            return agents;
        }
        let agents = LiveAgents(Rc::new(Source {
            origin: Some(origin),
            connection_key: Some(connection_key),
            ..Source::default()
        }));
        sources.insert(&agents);
        agents
    }

    fn observation(identity: &Identity, state: &str) -> crate::roster::BoundRosterUpdate {
        let mut snapshot = Snapshot::default();
        snapshot.update(
            &serde_json::to_vec(&serde_json::json!({
                "items": [{"name": "worker", "state": state, "reason": "permission"}]
            }))
            .unwrap(),
        );
        crate::roster::BoundRosterUpdate {
            identity: identity.clone(),
            snapshot,
            error: None,
            runs: Runs::default(),
        }
    }

    /// The service's fixture of a roster and of its active runs.
    fn fixture(name: &str) -> &'static [u8] {
        match name {
            "agents" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../roster/src/profiles/runs/fixtures/agents.json"
            )),
            _ => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../roster/src/profiles/runs/fixtures/runs.json"
            )),
        }
    }

    #[test]
    fn the_list_line_and_the_profile_counts_come_from_the_roster() {
        let agents = LiveAgents::default();
        let mut snapshot = Snapshot::default();
        snapshot.update(fixture("agents"));
        agents.update(snapshot.clone());
        assert_eq!(
            agents.get("worker").line.as_deref(),
            Some("waits on build slot · 1 running, 1 queued")
        );
        assert_eq!(agents.get("worker").label, "needs you · permission");
        assert_eq!(agents.get("reviewer").line, None);
        assert_eq!(agents.background().as_deref(), Some("1 running · 1 queued"));
        snapshot.update(
            br#"{"items":[{"name":"worker","state":"busy","background":{"running":0,"queued":2},
            "waiting":{"on":"agent","on_agent":"reviewer","reason":null,"since":null}}]}"#,
        );
        agents.update(snapshot.clone());
        assert_eq!(
            agents.get("worker").line.as_deref(),
            Some("waits on agent reviewer · 2 queued")
        );
        assert_eq!(agents.background().as_deref(), Some("2 queued"));
        // An older service: the row and the status bar are as before.
        snapshot.update(br#"{"items":[{"name":"worker","state":"busy"}]}"#);
        agents.update(snapshot.clone());
        assert_eq!(
            (agents.get("worker").line.clone(), agents.background()),
            (None, None)
        );
        // Counts are not shown from a roster that can no longer be read.
        snapshot.update(fixture("agents"));
        snapshot.unavailable();
        snapshot.observed_at = std::time::Instant::now().checked_sub(TOLERANCE);
        agents.update(snapshot);
        assert_eq!(agents.background(), None);
    }

    #[test]
    fn held_and_waiting_deliveries_show_with_their_count_until_none_wait() {
        let read = |body: &[u8]| {
            let agents = LiveAgents::default();
            let mut snapshot = Snapshot::default();
            snapshot.update(body);
            agents.update(snapshot);
            // The row's count, then its reason, as one line.
            agents
                .get("worker")
                .deliveries
                .clone()
                .map(|(count, why)| match why {
                    Some(why) => format!("{count} · {why}"),
                    None => count,
                })
        };
        let fixture = |name: &str| {
            std::fs::read(format!(
                "{}/../roster/src/profiles/runs/fixtures/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap()
        };
        assert_eq!(
            read(&fixture("agents-held-queue")).as_deref(),
            Some("8 waiting · deliveries held: a message may still be in the original terminal")
        );
        // The list does not say why a plain wait waits.
        assert_eq!(
            read(&fixture("agents-queued")).as_deref(),
            Some("3 waiting")
        );
        assert_eq!(read(&fixture("agents")), None);
        // One held with none behind it is still one waiting.
        assert_eq!(
            read(br#"{"items":[{"name":"worker","held_queue":{"reason":"later"},"queued":1}]}"#)
                .as_deref(),
            Some("1 waiting · deliveries are held")
        );
        // A held queue always waits for at least the held one.
        assert_eq!(
            read(br#"{"items":[{"name":"worker","held_queue":{},"queued":0}]}"#).as_deref(),
            Some("1 waiting · deliveries are held")
        );
        // Cleared only once nothing holds them and none wait.
        assert_eq!(
            read(br#"{"items":[{"name":"worker","held_queue":null,"queued":0}]}"#),
            None
        );
        // The agent's view adds why from its inspection's delivery code.
        assert_eq!(
            deliveries(None, 2, Some("human_typing")).as_deref(),
            Some("2 waiting · someone is typing")
        );
        for code in [Some("ready"), Some("later"), None] {
            assert_eq!(deliveries(None, 2, code).as_deref(), Some("2 waiting"));
        }
        assert_eq!(deliveries(None, 0, Some("human_typing")), None);
        // A held queue gives its own reason, never the delivery's.
        let held = agent_interface_roster::HeldQueue {
            reason: Some("uncertain_delivery".into()),
        };
        assert_eq!(
            deliveries(Some(&held), 8, Some("human_typing")).as_deref(),
            Some("Deliveries held: a message may still be in the original terminal · 8 waiting")
        );
        assert_eq!(
            deliveries(Some(&Default::default()), 0, None).as_deref(),
            Some("Deliveries are held · 1 waiting")
        );
    }

    #[test]
    fn runs_are_not_shown_as_current_while_the_roster_or_the_list_is_stale() {
        let listed: serde_json::Value = serde_json::from_slice(fixture("runs")).unwrap();
        let runs: Vec<Run> = serde_json::from_str(&listed["items"].to_string()).unwrap();
        let profile = origin("a", "1");
        let agents = LiveAgents(Rc::new(Source {
            origin: Some(profile.clone()),
            ..Source::default()
        }));
        let mut update = observation(&profile, "offline");
        update.runs.active = runs.clone();
        update.runs.finished = vec![runs[1].clone()];
        agents.update_bound(update.clone());
        let worker = "1234567890abcdef1234567890abcdef";
        assert!(agents.activity("worker").is_some());
        assert_eq!(agents.offline_with_runs(), ["worker"]);
        assert_eq!(agents.runs_of(worker).active.len(), 1);
        // A refused read reports the roster unavailable: its runs aren't
        // current any more, while those that ended stay.
        let mut refused = update.clone();
        refused.error = Some(agent_interface_roster::profiles::Error::Refused(401));
        agents.update_bound(refused);
        assert!(agents.activity("worker").is_none());
        assert!(agents.offline_with_runs().is_empty());
        let card = agents.runs_of(worker);
        assert!(card.active.is_empty());
        assert_eq!(card.finished.len(), 1);
        agents.update_bound(update.clone());
        assert!(agents.activity("worker").is_some());
        // A list that has failed for a while shows no active run either.
        update.runs.stale = true;
        agents.update_bound(update);
        assert!(agents.activity("worker").is_none());
        assert!(agents.offline_with_runs().is_empty());
        assert!(agents.runs_of(worker).active.is_empty());
        assert_eq!(agents.runs_of(worker).finished.len(), 1);
    }

    #[test]
    fn runs_ride_a_bound_update_to_the_card_and_the_sidebar() {
        let listed: serde_json::Value = serde_json::from_slice(fixture("runs")).unwrap();
        let mut runs: Vec<Run> = serde_json::from_str(&listed["items"].to_string()).unwrap();
        // A review without a model names its CLI; one without details, its label.
        let mut unnamed = runs[0].clone();
        unnamed.run_id = "0193a1b2c3d47e5f8a9b0c1d2e3f4a62".into();
        unnamed.review.as_mut().unwrap().model = None;
        let mut bare = runs[0].clone();
        bare.run_id = "0193a1b2c3d47e5f8a9b0c1d2e3f4a63".into();
        bare.review = None;
        bare.state = "queued".into();
        let mut test = runs[1].clone();
        test.run_id = "0193a1b2c3d47e5f8a9b0c1d2e3f4a64".into();
        test.kind = "test".into();
        test.state = "running".into();
        runs.extend([unnamed, bare]);
        let profile = origin("a", "1");
        let agents = LiveAgents(Rc::new(Source {
            origin: Some(profile.clone()),
            ..Source::default()
        }));
        let mut update = observation(&profile, "busy");
        update.runs.active = runs.clone();
        assert!(agents.update_bound(update.clone()));
        // An unchanged roster with changed runs still redraws.
        update.runs.active.push(test);
        assert!(agents.update_bound(update.clone()));
        assert!(!agents.update_bound(update));
        let builder = agents.activity("builder").unwrap();
        assert_eq!(
            builder.reviews,
            [
                ("gpt-example high".to_owned(), "running".to_owned()),
                ("codex high".to_owned(), "running".to_owned()),
                (
                    "Review round 2, gpt-example".to_owned(),
                    "queued".to_owned()
                ),
            ]
        );
        assert_eq!(builder.runs, None);
        let worker = agents.activity("worker").unwrap();
        assert!(worker.reviews.is_empty());
        assert_eq!(worker.runs.as_deref(), Some("1 running, 1 queued"));
        assert!(agents.activity("reviewer").is_none());
        // The card finds its agent's runs by registration.
        let card = agents.runs_of("1234567890abcdef1234567890abcdef");
        assert_eq!(card.active.len(), 2);
        assert!(card.finished.is_empty());
        let mut update = observation(&profile, "busy");
        update.runs.active = vec![runs[1].clone()];
        agents.update_bound(update);
        assert_eq!(
            agents.activity("worker").unwrap().runs.as_deref(),
            Some("build queued")
        );
        assert!(agents.activity("builder").is_none());
        assert!(agents.offline_with_runs().is_empty());
        // An offline agent with runs is named; one the roster doesn't know is not.
        let mut update = observation(&profile, "offline");
        update.runs.active = runs;
        agents.update_bound(update);
        assert_eq!(agents.offline_with_runs(), ["worker"]);
    }

    #[test]
    fn attention_and_delayed_observations_stay_with_their_origin() {
        let mut sources = Sources::default();
        let (origin_a, origin_b) = (origin("a", "1"), origin("b", "1"));
        let a = acquire(&mut sources, origin_a.clone());
        let b = acquire(&mut sources, origin_b.clone());
        let a_view = a.clone();
        let b_view = b.clone();
        a.update_bound(observation(&origin_a, "waiting_for_human"));
        b.update_bound(observation(&origin_b, "busy"));
        assert_eq!(a_view.get("worker").label, "needs you · permission");
        assert!(!b_view.get("worker").state.needs_you());
        assert_eq!(a_view.active().unwrap()["worker"], AgentState::NeedsYou);
        assert_eq!(b_view.active().unwrap()["worker"], AgentState::Working);

        // Hold A's result while a different view is current, then deliver it
        // through A's retained origin. No selected-source lookup participates.
        let delayed_a = observation(&origin_a, "idle");
        let selected = b_view;
        assert!(!selected.update_bound(delayed_a.clone()));
        a.update_bound(delayed_a);
        assert_eq!(a_view.get("worker").state, AgentState::Idle);
        assert_eq!(selected.get("worker").state, AgentState::Working);
        drop(a_view);
        let reopened = acquire(&mut sources, origin_a.clone());
        assert!(Rc::ptr_eq(&a.0, &reopened.0));
        assert_eq!(reopened.get("worker").state, AgentState::Idle);

        let mut refused = observation(&origin_a, "waiting_for_human");
        refused.error = Some(agent_interface_roster::profiles::Error::BindingMismatch);
        a.update_bound(refused);
        assert_eq!(reopened.get("worker").state, AgentState::Unavailable);
        assert!(reopened.active().is_none());
        assert_eq!(selected.get("worker").state, AgentState::Working);
        assert!(selected.active().is_some());
    }

    #[test]
    fn shared_sources_expire_with_their_last_owner_and_never_cross_lifecycles() {
        let mut sources = Sources::default();
        let identity = origin("a", "1");
        let first = acquire(&mut sources, identity.clone());
        first.update_bound(observation(&identity, "waiting_for_human"));
        let second_workspace = acquire(&mut sources, identity.clone());
        assert!(Rc::ptr_eq(&first.0, &second_workspace.0));
        let replacement = acquire(&mut sources, origin("a", "2"));
        assert!(!replacement.observed());
        assert!(!replacement.update_bound(observation(&identity, "busy")));
        assert_eq!(replacement.get("worker").state, AgentState::Unknown);
        let old_callback = Rc::downgrade(&first.0);
        drop(first);
        assert!(old_callback.upgrade().is_some());
        drop(second_workspace);
        assert!(old_callback.upgrade().is_none());
        let reopened = acquire(&mut sources, identity);
        assert!(!reopened.observed());
        assert!(old_callback.upgrade().is_none());
    }

    #[test]
    fn fresh_verified_connections_do_not_reuse_retained_connection_workers() {
        use crate::roster::content::tests::provider;

        // Two independently authenticated snapshots report the same full identity.
        let (_old_dir, old_connection, old_server) = provider("a", vec![]);
        let (_new_dir, new_connection, new_server) = provider("a", vec![]);
        old_server.join().unwrap();
        new_server.join().unwrap();
        assert_eq!(old_connection.identity(), new_connection.identity());
        let origin = old_connection.identity().clone();
        let mut sources = Sources::default();
        let old = acquire_key(
            &mut sources,
            origin.clone(),
            old_connection.connection_key(),
        );
        let retained = acquire_key(
            &mut sources,
            origin.clone(),
            old_connection.clone().connection_key(),
        );
        let new = acquire_key(
            &mut sources,
            origin.clone(),
            new_connection.connection_key(),
        );
        assert!(Rc::ptr_eq(&old.0, &retained.0));
        assert!(!Rc::ptr_eq(&old.0, &new.0));
        old.update_bound(observation(&origin, "waiting_for_human"));
        assert!(!new.observed());
        new.update_bound(observation(&origin, "busy"));
        let mut refused = observation(&origin, "idle");
        refused.error = Some(agent_interface_roster::profiles::Error::Refused(401));
        old.update_bound(refused);
        assert_eq!(retained.get("worker").state, AgentState::Unavailable);
        assert_eq!(new.get("worker").state, AgentState::Working);
        drop(old);
        drop(retained);
        assert!(
            sources
                .get(&origin, &old_connection.connection_key())
                .is_none()
        );
        assert!(
            sources
                .get(&origin, &new_connection.connection_key())
                .is_some()
        );
    }
}
