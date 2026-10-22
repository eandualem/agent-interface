use std::collections::BTreeMap;
use std::time::Instant;

use tmux_workspaces_roster::Snapshot;

use super::Observation;

/// Which observation supplied a session's descriptive state.
#[derive(Debug, PartialEq, Eq)]
pub enum Origin {
    /// Ordinary tmux discovery, without metadata for this name.
    Tmux,
    /// The optional roster, which does not establish attachment availability.
    Backbone,
}

/// One attachment reference enriched with any available metadata.
#[derive(Debug, PartialEq, Eq)]
pub struct Session {
    /// The reported metadata state, or `running` for plain discovery.
    pub state: String,
    /// True only when the latest successful discovery includes this exact name.
    pub online: bool,
    /// The source of the descriptive state, not ownership of the session.
    pub origin: Origin,
}

/// Attachment choices with optional metadata. Keep the separate roster snapshot
/// for roster display: these entries also contain ordinary tmux sessions.
#[derive(Debug)]
pub struct CombinedObservation {
    /// The union of discovered sessions and metadata references, by exact name.
    pub sessions: BTreeMap<String, Session>,
    /// Nonempty means some observations are stale, even when discovery succeeded.
    pub error: String,
    /// Oldest participating observation time; none if any provider has none.
    pub observed_at: Option<Instant>,
}

/// Combine detached values without changing either provider's observation.
/// Only successful tmux discovery establishes availability. Metadata can add
/// offline references and replace states, but cannot remove ordinary sessions.
/// No roster means ordinary discovery alone; no adapter is activated here.
pub fn with_roster(discovery: &Observation, roster: Option<&Snapshot>) -> CombinedObservation {
    let mut sessions = BTreeMap::new();
    if discovery.error.is_none() {
        for name in &discovery.sessions {
            sessions.insert(
                name.clone(),
                Session {
                    state: "running".into(),
                    online: true,
                    origin: Origin::Tmux,
                },
            );
        }
    }
    let mut error = discovery.error.clone().unwrap_or_default();
    let mut observed_at = discovery.observed_at;
    if let Some(roster) = roster {
        for (name, state) in &roster.states {
            let online = sessions.get(name).is_some_and(|session| session.online);
            sessions.insert(
                name.clone(),
                Session {
                    state: state.clone(),
                    online,
                    origin: Origin::Backbone,
                },
            );
        }
        if let Some(message) = roster.error {
            if !error.is_empty() {
                error.push_str("; ");
            }
            error.push_str(message);
        }
        observed_at = observed_at.zip(roster.observed_at).map(|(a, b)| a.min(b));
    }
    CombinedObservation {
        sessions,
        error,
        observed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_never_owns_availability_or_removes_ordinary_sessions() {
        let discovery = Observation {
            sessions: vec!["ordinary shell".into(), "worker".into()],
            error: None,
            observed_at: Some(Instant::now()),
        };
        let mut roster = Snapshot::default();
        roster.update(
            br#"{"items":[
            {"name":"worker","state":"busy","online":false},
            {"name":"parked","state":"offline","online":true}
        ]}"#,
        );
        let combined = with_roster(&discovery, Some(&roster));
        assert_eq!(
            combined.sessions["worker"],
            Session {
                state: "busy".into(),
                online: true,
                origin: Origin::Backbone
            }
        );
        assert!(!combined.sessions["parked"].online);
        assert_eq!(combined.sessions["ordinary shell"].origin, Origin::Tmux);
        assert_eq!(combined.observed_at, discovery.observed_at);
        roster.unavailable();
        let stale = with_roster(&discovery, Some(&roster));
        assert_eq!(stale.sessions, combined.sessions);
        assert!(!stale.error.is_empty());
        assert_eq!(stale.observed_at, combined.observed_at);
        let failed = Observation {
            sessions: Vec::new(),
            error: Some("Session list unavailable".into()),
            observed_at: None,
        };
        let unavailable = with_roster(&failed, Some(&roster));
        assert!(unavailable.sessions.values().all(|session| !session.online));
        assert_eq!(
            unavailable.error,
            "Session list unavailable; Backbone unavailable; states stale"
        );
        assert!(unavailable.observed_at.is_none());
        roster.update(br#"{"items":[]}"#);
        let recovered = with_roster(&discovery, Some(&roster));
        assert_eq!(recovered.sessions, with_roster(&discovery, None).sessions);
        assert!(recovered.error.is_empty());
        assert_eq!(combined.sessions["worker"].state, "busy");
    }
}
