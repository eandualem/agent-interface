//! Connection-owned live rosters. Views retain their originating observations;
//! authored reports and historical timelines never write into current state.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::{Rc, Weak},
};

use agent_interface_roster::{
    Snapshot,
    profiles::{Connection, Identity},
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
}

impl Status {
    fn new(state: AgentState) -> Self {
        Self {
            state,
            online: state.online(),
            label: state.word().into(),
            since: None,
            reason: None,
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

struct Current {
    snapshot: Option<Snapshot>,
    statuses: HashMap<String, Rc<Status>>,
    active: Option<Rc<HashMap<String, AgentState>>>,
    unknown: Rc<Status>,
    unavailable: Rc<Status>,
}

impl Default for Current {
    fn default() -> Self {
        Self {
            snapshot: None,
            statuses: HashMap::new(),
            active: None,
            unknown: Rc::new(Status::new(AgentState::Unknown)),
            unavailable: Rc::new(Status::new(AgentState::Unavailable)),
        }
    }
}

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

    pub fn active(&self) -> Option<Rc<HashMap<String, AgentState>>> {
        self.0.current.borrow().active.clone()
    }

    pub fn observed(&self) -> bool {
        self.0.current.borrow().snapshot.is_some()
    }

    pub fn update(&self, mut snapshot: Snapshot) -> bool {
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
                    }
                    (name.clone(), Rc::new(status))
                })
                .collect();
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

/// Keys describe origin, never the selected view or an agent's name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Origin {
    Legacy(PathBuf),
    Bound(Identity),
}

/// Windows and their child views own sources. The registry only deduplicates
/// live owners, so closing the last view releases its poller and cached state.
#[derive(Default)]
pub(super) struct Sources(HashMap<Origin, Weak<Source>>);
impl Global for Sources {}

impl Sources {
    fn get(&mut self, origin: &Origin) -> Option<LiveAgents> {
        self.0.retain(|_, source| source.strong_count() > 0);
        self.0.get(origin).and_then(Weak::upgrade).map(LiveAgents)
    }

    fn insert(&mut self, agents: &LiveAgents) {
        if let Some(origin) = &agents.0.origin {
            self.0.insert(origin.clone(), Rc::downgrade(&agents.0));
        }
    }
}

enum Worker {
    Legacy(crate::roster::RosterSource),
    Bound(crate::roster::BoundRosterSource),
}

/// Joining a bounded in-flight request must never block the UI thread.
#[derive(Default)]
struct Roster(Option<Worker>);
impl Drop for Roster {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            std::thread::spawn(move || drop(worker));
        }
    }
}

#[derive(Default)]
struct Source {
    origin: Option<Origin>,
    current: RefCell<Current>,
    roster: Roster,
    updates: RefCell<Option<Task<()>>>,
}

impl LiveAgents {
    pub fn belongs_to(&self, identity: &Identity) -> bool {
        self.0.origin.as_ref() == Some(&Origin::Bound(identity.clone()))
    }

    fn update_bound(&self, mut update: crate::roster::BoundRosterUpdate) -> bool {
        if self.0.origin.as_ref() != Some(&Origin::Bound(update.identity)) {
            return false;
        }
        if update.error.is_some() {
            update.snapshot.unavailable();
        }
        self.update(update.snapshot)
    }

    fn take_update(&self) -> Option<bool> {
        match self.0.roster.0.as_ref()? {
            Worker::Legacy(source) => source.take_update().map(|snapshot| self.update(snapshot)),
            Worker::Bound(source) => source.take_update().map(|update| self.update_bound(update)),
        }
    }
}

pub(super) fn start_legacy(directory: PathBuf, cx: &mut App) -> Option<LiveAgents> {
    let origin = Origin::Legacy(directory.clone());
    start(origin, cx, move |wake| {
        crate::roster::RosterSource::start(directory, None, wake).map(Worker::Legacy)
    })
}

/// Only an already-verified connection can start a bound source. Selection and
/// workspace persistence are not enabled by this ownership primitive.
#[allow(dead_code)] // Used when workspace profile binding is enabled.
pub(super) fn start_bound(connection: Connection, cx: &mut App) -> Option<LiveAgents> {
    let origin = Origin::Bound(connection.identity().clone());
    start(origin, cx, move |wake| {
        crate::roster::BoundRosterSource::start(connection, wake).map(Worker::Bound)
    })
}

fn start(
    origin: Origin,
    cx: &mut App,
    worker: impl FnOnce(Box<dyn Fn() + Send>) -> std::io::Result<Worker>,
) -> Option<LiveAgents> {
    if !cx.has_global::<Sources>() {
        cx.set_global(Sources::default());
    }
    if let Some(agents) = cx.update_global::<Sources, _>(|sources, _| sources.get(&origin)) {
        return Some(agents);
    }
    let (wake, mut woken) = mpsc::unbounded();
    let roster = worker(Box::new(move || {
        let _ = wake.unbounded_send(());
    }))
    .ok()?;
    let agents = LiveAgents(Rc::new(Source {
        origin: Some(origin),
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
                    // Notify discovery observers even on unchanged status.
                    // Each window rechecks matches against its own handle.
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

    fn origin(profile: &str, lifecycle: &str) -> Origin {
        Origin::Bound(Identity {
            profile_id: profile.into(),
            installation_id: "installation".into(),
            lifecycle_id: lifecycle.into(),
        })
    }

    fn acquire(sources: &mut Sources, origin: Origin) -> LiveAgents {
        if let Some(agents) = sources.get(&origin) {
            return agents;
        }
        let agents = LiveAgents(Rc::new(Source {
            origin: Some(origin),
            ..Source::default()
        }));
        sources.insert(&agents);
        agents
    }

    fn observation(origin: &Origin, state: &str) -> crate::roster::BoundRosterUpdate {
        let Origin::Bound(identity) = origin else {
            panic!("expected bound origin")
        };
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
        }
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
    fn legacy_directories_and_standalone_observations_are_explicit() {
        let mut sources = Sources::default();
        let first = acquire(&mut sources, Origin::Legacy("/fixture/a".into()));
        let same = acquire(&mut sources, Origin::Legacy("/fixture/a".into()));
        let other = acquire(&mut sources, Origin::Legacy("/fixture/b".into()));
        let standalone = LiveAgents::default();
        let mut snapshot = Snapshot::default();
        snapshot.update(br#"{"items":[{"name":"worker","state":"busy"}]}"#);
        first.update(snapshot);
        assert_eq!(same.get("worker").state, AgentState::Working);
        assert_eq!(other.get("worker").state, AgentState::Unknown);
        assert_eq!(standalone.get("worker").state, AgentState::Unknown);
    }
}
