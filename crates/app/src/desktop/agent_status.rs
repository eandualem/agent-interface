//! One application-wide live roster. Views borrow its observations; authored
//! reports and historical timelines never write into current agent state.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

use agent_interface_roster::Snapshot;
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
pub(super) struct LiveAgents(Rc<RefCell<Current>>);

impl LiveAgents {
    pub fn shared(cx: &App) -> Self {
        cx.try_global::<Source>()
            .map(|source| source.agents.clone())
            .unwrap_or_default()
    }

    pub fn get(&self, name: &str) -> Rc<Status> {
        let current = self.0.borrow();
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
        self.0.borrow().active.clone()
    }

    pub fn observed(&self) -> bool {
        self.0.borrow().snapshot.is_some()
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
        let mut current = self.0.borrow_mut();
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

/// The app's roster worker joins off the UI thread when the source is dropped.
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

/// Owns the single poller until the app exits, independently of any window.
pub(super) struct Source {
    agents: LiveAgents,
    roster: Roster,
    _updates: Option<Task<()>>,
}
impl Global for Source {}

pub(super) fn start(directory: PathBuf, cx: &mut App) -> Option<LiveAgents> {
    if let Some(source) = cx.try_global::<Source>() {
        return Some(source.agents.clone());
    }
    let (wake, mut woken) = mpsc::unbounded();
    let roster = crate::roster::RosterSource::start(directory, None, move || {
        let _ = wake.unbounded_send(());
    })
    .ok()?;
    let agents = LiveAgents::default();
    cx.set_global(Source {
        agents: agents.clone(),
        roster: Roster(Some(roster)),
        _updates: None,
    });
    let updates = cx.spawn(async move |cx| {
        while woken.next().await.is_some() {
            cx.update(|cx| {
                let snapshot = cx.global::<Source>().roster.take_update();
                if let Some(snapshot) = snapshot {
                    let changed =
                        cx.update_global::<Source, _>(|source, _| source.agents.update(snapshot));
                    if changed {
                        cx.refresh_windows();
                    }
                }
            });
        }
    });
    cx.update_global::<Source, _>(|source, _| source._updates = Some(updates));
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
}
