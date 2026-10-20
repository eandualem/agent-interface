//! The optional, read-only agents roster: its transport and response types.
//!
//! This crate depends on no other project crate. Response decoding stays
//! separate from polling. See `docs/decisions/0010-agents-roster-adapter.md`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::Value;

const RESPONSE_LIMIT: usize = 1024 * 1024;
const UNAVAILABLE: &str = "Backbone unavailable; states stale";

/// Cached metadata, separate from tmux attachment availability.
///
/// Only names and states are retained. In particular, an API `online` value
/// cannot establish that a tmux session exists. A failed response preserves
/// cached states and their timestamp, but the error marks those states stale.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Validated agent names and their reported states, including offline names.
    pub states: BTreeMap<String, String>,
    /// A fixed diagnostic when cached states are stale; none after success.
    pub error: Option<&'static str>,
    /// Last successful response time, or none before any successful response.
    pub observed_at: Option<Instant>,
}

impl Snapshot {
    /// Apply one response from the agents endpoint. Invalid or oversized JSON
    /// marks the previous snapshot stale without exposing response contents.
    /// A successful empty list clears old names and any earlier error.
    pub fn update(&mut self, response: &[u8]) {
        match decode(response) {
            Some(states) => {
                self.states = states;
                self.error = None;
                self.observed_at = Some(Instant::now());
            }
            None => self.unavailable(),
        }
    }

    /// Mark a transport or configuration failure without retaining its details.
    /// This does not change the last successful observation time.
    pub fn unavailable(&mut self) {
        self.error = Some(UNAVAILABLE);
    }
}

fn decode(response: &[u8]) -> Option<BTreeMap<String, String>> {
    if response.len() > RESPONSE_LIMIT {
        return None;
    }
    let document: Value = serde_json::from_slice(response).ok()?;
    let items = document.get("items")?.as_array()?;
    let mut states = BTreeMap::new();
    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !valid_agent(name) || item.get("configured").is_some_and(|v| !truthy(v)) {
            continue;
        }
        let state = item
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        // Last valid occurrence wins, matching the Python provider's map.
        states.insert(name.to_owned(), state.to_owned());
    }
    Some(states)
}

fn valid_agent(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

// The Python provider treats all false-like configured values as disabled,
// while an absent configured field defaults to true.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_filter_metadata_keep_stale_states_and_recover() {
        let mut snapshot = Snapshot::default();
        snapshot.update(
            br#"{"items":[
            {"name":"worker","state":"idle","online":false,"other":"ignored"},
            {"name":"offline","state":"offline","online":true},
            {"name":"unknown","state":{"invalid":"state"}},
            {"name":"hidden","configured":false},
            {"name":"zero","configured":0},
            {"name":"empty","configured":""},
            {"name":"not an agent","state":"busy"},
            {"name":"-invalid","state":"busy"},
            {"name":["invalid"]}, 1,
            {"name":"worker","state":"busy"}
        ]}"#,
        );
        assert_eq!(
            snapshot.states,
            BTreeMap::from([
                ("offline".into(), "offline".into()),
                ("unknown".into(), "unknown".into()),
                ("worker".into(), "busy".into()),
            ])
        );
        assert!(snapshot.error.is_none());
        assert!(snapshot.observed_at.is_some());
        let previous = snapshot.clone();
        snapshot.update(br#"{"items":"sensitive malformed body"}"#);
        assert_eq!(snapshot.states, previous.states);
        assert_eq!(snapshot.observed_at, previous.observed_at);
        assert_eq!(snapshot.error, Some(UNAVAILABLE));
        snapshot.unavailable();
        assert_eq!(snapshot.observed_at, previous.observed_at);
        snapshot.update(&vec![b' '; RESPONSE_LIMIT + 1]);
        assert_eq!(snapshot.states, previous.states);
        assert_eq!(snapshot.error, Some(UNAVAILABLE));
        snapshot.update(br#"{"items":[]}"#);
        assert!(snapshot.states.is_empty());
        assert!(snapshot.error.is_none());
        assert!(snapshot.observed_at >= previous.observed_at);
    }
}
