//! Optional Backbone roster and view reads, plus explicit user feedback.
//! Transport and response types stay separate from polling.
//!
//! This crate depends on no other project crate. Response decoding stays
//! separate from polling. See `docs/ARCHITECTURE.md`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::value::RawValue;

mod configuration;
pub mod conversations;
pub mod feedback;
pub mod interactions;
pub mod library_snapshots;
pub mod lifecycle;
pub mod plans;
pub mod profiles;
pub mod sheets;
mod transport;
pub mod views;

const RESPONSE_LIMIT: usize = 1024 * 1024;
const UNAVAILABLE: &str = "Backbone unavailable; states stale";

/// Presentation metadata from the same observation as an agent's state.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AgentDetails {
    /// Whether the provider observes an online agent, including unknown states.
    pub online: Option<bool>,
    /// Why the agent is waiting or blocked.
    pub reason: Option<String>,
    /// The issue currently being worked on.
    pub current_issue: Option<u64>,
    /// Provider timestamp of the state observation.
    pub state_since: Option<f64>,
}

/// Cached metadata, separate from tmux attachment availability.
///
/// Names, states and their presentation details are retained. An API `online` value
/// cannot establish that a tmux session exists. A failed response preserves
/// cached states and their timestamp, but the error marks those states stale.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Validated agent names and their reported states, including offline names.
    pub states: BTreeMap<String, String>,
    /// Details observed together with `states`.
    pub details: BTreeMap<String, AgentDetails>,
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
            Some((states, details)) => {
                self.states = states;
                self.details = details;
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

type Decoded = (BTreeMap<String, String>, BTreeMap<String, AgentDetails>);

fn decode(response: &[u8]) -> Option<Decoded> {
    if response.len() > RESPONSE_LIMIT || !within_depth(response) {
        return None;
    }
    // Typed containers and raw field values preserve literal JSON types. Value
    // would interpret serde_json's private marker keys under workspace features.
    let document: BTreeMap<String, &RawValue> = serde_json::from_slice(response).ok()?;
    let items: Vec<&RawValue> = serde_json::from_str(document.get("items")?.get()).ok()?;
    let mut states = BTreeMap::new();
    let mut details = BTreeMap::new();
    for raw in items {
        let Ok(item) = serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get()) else {
            continue;
        };
        let Some(name) = item
            .get("name")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
        else {
            continue;
        };
        if !valid_agent(&name) || item.get("configured").is_some_and(|v| !truthy(v)) {
            continue;
        }
        let state = item
            .get("state")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .unwrap_or_else(|| "unknown".into());
        // Last valid occurrence wins, matching the Python provider's map.
        let field = |key: &str| item.get(key).map(|value| value.get());
        details.insert(
            name.clone(),
            AgentDetails {
                online: field("online").and_then(|value| serde_json::from_str(value).ok()),
                reason: field("reason").and_then(|value| serde_json::from_str(value).ok()),
                current_issue: field("current_issue")
                    .and_then(|value| serde_json::from_str(value).ok()),
                state_since: field("state_since")
                    .and_then(|value| serde_json::from_str(value).ok()),
            },
        );
        states.insert(name, state);
    }
    Some((states, details))
}

// RawValue skips nested data without the normal Value recursion limit. Bound
// containers before parsing, ignoring bracket characters inside JSON strings.
fn within_depth(response: &[u8]) -> bool {
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for &byte in response {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > 128 {
                        return false;
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    true
}

/// Whether `name` can name an agent: letters, digits, `_`, `.` and `-`,
/// starting with a letter or digit.
pub fn valid_agent(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

// The Python provider treats all false-like configured values as disabled,
// while an absent configured field defaults to true.
fn truthy(value: &RawValue) -> bool {
    let text = value.get().trim_ascii();
    match text.as_bytes()[0] {
        b'n' | b'f' => false,
        b't' => true,
        b'"' => text.len() != 2,
        b'[' | b'{' => !text[1..text.len() - 1].trim_ascii().is_empty(),
        _ => text
            .parse::<serde_json::Number>()
            .ok()
            .and_then(|value| value.as_f64())
            .is_none_or(|value| value != 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_details_do_not_discard_valid_fields() {
        let mut snapshot = Snapshot::default();
        snapshot.update(
            br#"{"items":[{"name":"worker","state":"future","online":true,"reason":7,"current_issue":42,"state_since":"invalid"}]}"#,
        );
        let details = &snapshot.details["worker"];
        assert_eq!(details.online, Some(true));
        assert_eq!(details.reason, None);
        assert_eq!(details.current_issue, Some(42));
        assert_eq!(details.state_since, None);
    }

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
        snapshot.update(br#"{"items":{"$serde_json::private::RawValue":"[]"}}"#);
        assert_eq!(snapshot.states, previous.states);
        assert_eq!(snapshot.error, Some(UNAVAILABLE));
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
        snapshot.update(
            br#"{"items":[{"name":"worker",
            "configured":{"$serde_json::private::Number":"0"},
            "state":{"$serde_json::private::RawValue":"\"busy\""}}]}"#,
        );
        assert_eq!(
            snapshot.states,
            BTreeMap::from([("worker".into(), "unknown".into())])
        );
        assert!(snapshot.error.is_none());
        let nested = format!(
            "{{\"items\":[],\"extra\":{}{}}}",
            "[".repeat(128),
            "]".repeat(128)
        );
        snapshot.update(nested.as_bytes());
        assert_eq!(snapshot.states["worker"], "unknown");
        assert_eq!(snapshot.error, Some(UNAVAILABLE));
    }
}
