//! The optional, read-only agents roster: its transport and response types.
//!
//! This crate depends on no other project crate. Response decoding stays
//! separate from polling. See `docs/ARCHITECTURE.md`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::value::RawValue;

mod configuration;
mod transport;

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
    /// Active `(name, state)` entries in Python's roster order. None means the
    /// roster is unavailable, distinct from a successful empty list. Cached
    /// states remain available in `states`, but must not appear as current rows.
    ///
    /// Waiting/blocked agents precede working/starting agents, then everyone
    /// else. Each group sorts by case-insensitive name, then exact name. Names
    /// are validated ASCII, so ASCII folding matches Python's case folding.
    /// Offline entries are omitted; empty states are presented as `unknown`.
    pub fn active_entries(&self) -> Option<Vec<(&str, &str)>> {
        if self.error.is_some() {
            return None;
        }
        let mut entries: Vec<_> = self
            .states
            .iter()
            .filter(|(_, state)| state.as_str() != "offline")
            .map(|(name, state)| {
                (
                    name.as_str(),
                    if state.is_empty() { "unknown" } else { state },
                )
            })
            .collect();
        entries.sort_by_cached_key(|&(name, state)| {
            let priority = match state {
                "waiting_for_human" | "blocked" => 0,
                "busy" | "starting" => 1,
                _ => 2,
            };
            (priority, name.to_ascii_lowercase(), name)
        });
        Some(entries)
    }

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
    if response.len() > RESPONSE_LIMIT || !within_depth(response) {
        return None;
    }
    // Typed containers and raw field values preserve literal JSON types. Value
    // would interpret serde_json's private marker keys under workspace features.
    let document: BTreeMap<String, &RawValue> = serde_json::from_slice(response).ok()?;
    let items: Vec<&RawValue> = serde_json::from_str(document.get("items")?.get()).ok()?;
    let mut states = BTreeMap::new();
    for item in items {
        let Ok(item) = serde_json::from_str::<BTreeMap<String, &RawValue>>(item.get()) else {
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
        states.insert(name, state);
    }
    Some(states)
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
    fn active_entries_follow_attention_order_and_hide_stale_rows_without_losing_cache() {
        let mut snapshot = Snapshot::default();
        snapshot.update(
            br#"{"items":[
            {"name":"z-worker","state":"busy"},
            {"name":"a-starting","state":"starting"},
            {"name":"z-waiting","state":"waiting_for_human"},
            {"name":"a-blocked","state":"blocked"},
            {"name":"alpha","state":"idle"},
            {"name":"Alpha","state":"unknown"},
            {"name":"empty","state":""},
            {"name":"unlisted","state":"new-state"},
            {"name":"offline","state":"offline"}
        ]}"#,
        );
        assert_eq!(
            snapshot.active_entries().unwrap(),
            [
                ("a-blocked", "blocked"),
                ("z-waiting", "waiting_for_human"),
                ("a-starting", "starting"),
                ("z-worker", "busy"),
                ("Alpha", "unknown"),
                ("alpha", "idle"),
                ("empty", "unknown"),
                ("unlisted", "new-state"),
            ]
        );
        let cached = snapshot.states.clone();
        let observed = snapshot.observed_at;
        snapshot.unavailable();
        assert!(snapshot.active_entries().is_none());
        assert_eq!(snapshot.states, cached);
        assert_eq!(snapshot.observed_at, observed);
        snapshot.update(
            br#"{"items":[{"name":"alpha","state":"blocked"},{"name":"Alpha","state":"idle"}]}"#,
        );
        assert_eq!(
            snapshot.active_entries().unwrap(),
            [("alpha", "blocked"), ("Alpha", "idle")]
        );
        snapshot.update(br#"{"items":[]}"#);
        assert_eq!(snapshot.active_entries(), Some(Vec::new()));
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
