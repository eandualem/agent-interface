//! Optional Backbone roster and view reads, plus explicit user feedback.
//! Transport and response types stay separate from polling.
//!
//! This crate depends on no other project crate. Response decoding stays
//! separate from polling. See `docs/ARCHITECTURE.md`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::value::RawValue;

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

/// Whether `value` is a lowercase UUID as Backbone writes one: 32 hex digits,
/// or the same digits hyphenated 8-4-4-4-12.
pub(crate) fn is_uuid(value: &str) -> bool {
    let hex = |part: &str| {
        part.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    match value.len() {
        32 => hex(value),
        36 => value.split('-').map(str::len).eq([8, 4, 4, 4, 12]) && value.split('-').all(hex),
        _ => false,
    }
}
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
    /// What the agent waits on outside itself; absent from an older service.
    pub waiting: Option<Waiting>,
    /// How much it runs in the background; absent from an older service.
    pub background: Option<Background>,
    /// The folder it works in, when reported.
    pub dir: Option<String>,
    /// What holds its terminal deliveries; absent when nothing does.
    pub held_queue: Option<HeldQueue>,
    /// Messages waiting for its terminal, the held one included; 0 when an
    /// older service does not say.
    pub queued: u64,
    /// Its own restart while it has not finished; absent otherwise.
    pub restart: Option<Restart>,
}

/// An agent's own restart. The list carries it only while it is under way,
/// whatever its phase; an inspection gives the latest one, finished or not.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Restart {
    /// Its number.
    #[serde(default, deserialize_with = "lenient")]
    pub id: Option<u64>,
    /// Its step, such as `stopping` or `failed`, as a code.
    #[serde(default, deserialize_with = "lenient")]
    pub phase: Option<String>,
    /// Whether the new session resumes the old one's conversation.
    #[serde(default, deserialize_with = "lenient")]
    pub resume: Option<bool>,
    /// How it ended, once it has.
    #[serde(default, deserialize_with = "lenient_object")]
    pub result: Option<RestartResult>,
}

/// How a restart ended.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct RestartResult {
    /// Why it failed, written for a person.
    #[serde(default, deserialize_with = "lenient")]
    pub reason: Option<String>,
}

/// Deliveries to an agent's terminal held behind one whose text may still
/// be at its prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct HeldQueue {
    /// Why, as a closed code: `uncertain_delivery` today. Absent or
    /// mistyped, deliveries are held for a reason not known.
    #[serde(default, deserialize_with = "lenient")]
    pub reason: Option<String>,
}

/// What an agent waits on outside itself, as the Rust service reports it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Waiting {
    /// What it waits on, such as a build slot, as the service wrote it.
    pub on: String,
    /// The agent it waits on, when it waits on an agent.
    #[serde(default, deserialize_with = "lenient")]
    pub on_agent: Option<String>,
    /// Why, as the service wrote it.
    #[serde(default, deserialize_with = "lenient")]
    pub reason: Option<String>,
    /// Since when, in RFC 3339 as the service wrote it.
    #[serde(default, deserialize_with = "lenient")]
    pub since: Option<String>,
}

/// How many runs and processes an agent has in the background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct Background {
    /// Its runs that are running.
    pub running: u64,
    /// Its runs that wait to start.
    pub queued: u64,
    /// Processes it started that still run.
    pub processes: u64,
}

impl Background {
    /// The counts in `raw`, a JSON object, each read on its own. A count
    /// that isn't a non-negative whole number isn't given and reads as 0;
    /// an object that gives none of them holds no counts at all.
    pub(crate) fn read(raw: &str) -> Option<Self> {
        let counts: BTreeMap<String, Box<RawValue>> = object(raw)?;
        let count = |name: &str| {
            counts
                .get(name)
                .and_then(|raw| serde_json::from_str::<u64>(raw.get()).ok())
        };
        let (running, queued, processes) = (count("running"), count("queued"), count("processes"));
        if running.is_none() && queued.is_none() && processes.is_none() {
            return None;
        }
        Some(Self {
            running: running.unwrap_or(0),
            queued: queued.unwrap_or(0),
            processes: processes.unwrap_or(0),
        })
    }
}

/// `raw` as `T` only when it is a JSON object that decodes. A derived struct
/// also decodes from an array, which would read `[0, 0, 0]` as real counts.
pub(crate) fn object<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    if !raw.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str(raw).ok()
}

/// For `deserialize_with`: a value that doesn't decode as `T`, `null`
/// included, reads as absent instead of failing the value that holds it.
pub(crate) fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = <Box<RawValue> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).ok())
}

/// As [`lenient`], for a value that must be a JSON object.
pub(crate) fn lenient_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = <Box<RawValue> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(object(raw.get()))
}

/// A running swarm: the agent that started it and the agents working in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swarm {
    /// Swarm name.
    pub name: String,
    /// The agent that started the swarm.
    pub initiator: String,
    /// Member agent names, the coordinator first, without the initiator.
    pub members: Vec<String>,
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
    /// Running swarms from the same poll; empty when their read failed.
    pub swarms: Vec<Swarm>,
    /// Whether `swarms` came from a response read in full, so a swarm missing
    /// from it has finished. False after a failed read or a skipped record.
    pub swarms_read: bool,
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

    /// Apply one response from the swarms endpoint, or its failure. Swarms only
    /// group agents, so an unreadable list clears them without marking states stale.
    pub fn update_swarms(&mut self, response: Option<&[u8]>) {
        let (swarms, complete) = response.and_then(decode_swarms).unwrap_or_default();
        self.swarms_read = complete;
        self.swarms = swarms;
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
        // The last valid occurrence of an agent wins.
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
                waiting: field("waiting").and_then(object),
                background: field("background").and_then(Background::read),
                dir: field("dir").and_then(|value| serde_json::from_str(value).ok()),
                held_queue: field("held_queue").and_then(object),
                queued: field("queued")
                    .and_then(|value| serde_json::from_str(value).ok())
                    .unwrap_or(0),
                restart: field("restart").and_then(object),
            },
        );
        states.insert(name, state);
    }
    Some((states, details))
}

/// Members shown for one swarm; more would not fit a tab or the saved layout.
const SWARM_MEMBERS: usize = 32;

/// The running swarms, and whether every record was understood.
fn decode_swarms(response: &[u8]) -> Option<(Vec<Swarm>, bool)> {
    // Null or mistyped fields drop that swarm, never the whole list.
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Raw {
        name: Option<String>,
        status: Option<String>,
        initiator: Option<String>,
        coordinator: Option<String>,
        members: Option<Vec<Box<RawValue>>>,
    }
    #[derive(serde::Deserialize)]
    struct Document {
        items: Vec<Box<RawValue>>,
    }
    if response.len() > RESPONSE_LIMIT || !within_depth(response) {
        return None;
    }
    let document: Document = serde_json::from_slice(response).ok()?;
    let mut swarms = Vec::new();
    let mut complete = true;
    for item in document.items {
        let Ok(swarm) = serde_json::from_str::<Raw>(item.get()) else {
            complete = false;
            continue;
        };
        if matches!(
            swarm.status.as_deref(),
            Some("done" | "disbanded" | "failed")
        ) {
            continue;
        }
        let (Some(name), Some(initiator)) = (swarm.name, swarm.initiator) else {
            complete = false;
            continue;
        };
        // The Rust service leaves the initiator empty for a swarm no agent
        // started: it lists under its coordinator.
        let initiator = match &swarm.coordinator {
            Some(coordinator) if initiator.is_empty() => coordinator.clone(),
            _ => initiator,
        };
        // The name becomes a tab name, so it follows the agent-name rule too.
        if !valid_agent(&name) || !valid_agent(&initiator) {
            complete = false;
            continue;
        }
        let mut members: Vec<String> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for member in swarm.members.iter().flatten() {
            if let Some(member) = views::member_name(member)
                && member != initiator
                && seen.insert(member.clone())
            {
                members.push(member);
            }
        }
        members.sort_by_key(|member| Some(member) != swarm.coordinator.as_ref());
        members.truncate(SWARM_MEMBERS);
        swarms.push(Swarm {
            name,
            initiator,
            members,
        });
    }
    Some((swarms, complete))
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

// Every false-like configured value means disabled, while an absent
// configured field defaults to true.
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
    fn swarms_keep_running_groups_with_the_coordinator_first() {
        let mut snapshot = Snapshot::default();
        snapshot.update_swarms(Some(
            br#"{"items":[
            {"name":"fix-7","status":"active","initiator":"lead","coordinator":"fix-7-coordinator",
             "members":["fix-7-coder",{"name":"fix-7-coordinator"},"lead","not an agent","fix-7-coder"]},
            {"name":"old","status":"done","initiator":"lead","coordinator":null,"members":["old-coder"]},
            {"name":"orphan","status":"active","initiator":null,"members":["x"]},
            {"name":"not a tab\tname","status":"active","initiator":"lead","members":["x"]},
            {"name":["mistyped"],"initiator":"lead"}, 7
        ]}"#,
        ));
        assert_eq!(
            snapshot.swarms,
            [Swarm {
                name: "fix-7".into(),
                initiator: "lead".into(),
                members: vec!["fix-7-coordinator".into(), "fix-7-coder".into()],
            }]
        );
        assert!(snapshot.error.is_none());
        let crowd: Vec<String> = (0..100).map(|index| format!("\"m{index}\"")).collect();
        let crowd = format!(
            r#"{{"items":[{{"name":"big","initiator":"lead","members":[{}]}}]}}"#,
            crowd.join(",")
        );
        snapshot.update_swarms(Some(crowd.as_bytes()));
        assert_eq!(snapshot.swarms[0].members.len(), SWARM_MEMBERS);
        snapshot.update_swarms(Some(b"not JSON"));
        assert!(snapshot.swarms.is_empty() && snapshot.error.is_none());
        assert!(!snapshot.swarms_read);
        snapshot.update_swarms(Some(br#"{"items":[{"name":"old","status":"done"}]}"#));
        assert!(snapshot.swarms_read);
        // A running swarm that cannot be listed leaves the answer unknown.
        snapshot.update_swarms(Some(br#"{"items":[{"name":"fix","status":"active"}]}"#));
        assert!(snapshot.swarms.is_empty() && !snapshot.swarms_read);
        snapshot.update_swarms(Some(
            br#"{"items":[{"name":"docs","status":"active","initiator":"",
                "coordinator":"docs-coordinator","members":["docs-coordinator","docs-coder"]}]}"#,
        ));
        assert_eq!(
            snapshot.swarms,
            [Swarm {
                name: "docs".into(),
                initiator: "docs-coordinator".into(),
                members: vec!["docs-coder".into()],
            }]
        );
        assert!(snapshot.swarms_read);
    }

    #[test]
    fn malformed_details_do_not_discard_valid_fields() {
        let mut snapshot = Snapshot::default();
        snapshot.update(
            br#"{"items":[{"name":"worker","state":"future","online":true,"reason":7,"current_issue":42,"state_since":"invalid","waiting":{"on":7},"background":{"running":"one"}},
            {"name":"array","waiting":["build slot"],"background":[0,0,0]},
            {"name":"lenient","waiting":{"on":"build slot","on_agent":[],"reason":7,"since":1}}]}"#,
        );
        let details = &snapshot.details["worker"];
        assert_eq!(details.online, Some(true));
        assert_eq!(details.reason, None);
        assert_eq!(details.current_issue, Some(42));
        assert_eq!(details.state_since, None);
        assert_eq!((&details.waiting, details.background), (&None, None));
        // Arrays are not objects, even though they would decode as one.
        let array = &snapshot.details["array"];
        assert_eq!((&array.waiting, array.background), (&None, None));
        // Each count is read on its own; an object that gives none is no counts.
        let counts = |body: &str| {
            let mut snapshot = Snapshot::default();
            snapshot.update(
                format!(r#"{{"items":[{{"name":"worker","background":{body}}}]}}"#).as_bytes(),
            );
            snapshot.details["worker"].background
        };
        let given = counts(r#"{"running":1,"queued":0,"processes":null}"#);
        assert_eq!(
            given.map(|counts| (counts.running, counts.queued, counts.processes)),
            Some((1, 0, 0))
        );
        let mixed = counts(r#"{"running":"one","queued":2,"processes":-1}"#);
        assert_eq!(
            mixed.map(|counts| (counts.running, counts.queued, counts.processes)),
            Some((0, 2, 0))
        );
        assert_eq!(counts("{}"), None);
        // A mistyped optional part leaves the rest of the wait.
        let waiting = snapshot.details["lenient"].waiting.clone().unwrap();
        assert_eq!(waiting.on, "build slot");
        assert_eq!(
            (waiting.on_agent, waiting.reason, waiting.since),
            (None, None, None)
        );
    }

    #[test]
    fn waiting_and_background_decode_from_the_services_fixture() {
        let mut snapshot = Snapshot::default();
        snapshot.update(include_bytes!("profiles/runs/fixtures/agents.json"));
        let worker = &snapshot.details["worker"];
        assert_eq!(
            worker.waiting,
            Some(Waiting {
                on: "build slot".into(),
                on_agent: None,
                reason: Some("Two builds already run on this machine.".into()),
                since: Some("2026-11-10T09:58:20Z".into()),
            })
        );
        let counts = |running, queued, processes| {
            Some(Background {
                running,
                queued,
                processes,
            })
        };
        assert_eq!(worker.background, counts(1, 1, 2));
        assert_eq!(worker.dir.as_deref(), Some("/home/user/project"));
        let reviewer = &snapshot.details["reviewer"];
        assert_eq!(reviewer.waiting, None);
        assert_eq!(reviewer.background, counts(0, 0, 0));
        // An older service has neither field.
        snapshot.update(br#"{"items":[{"name":"worker","state":"idle"}]}"#);
        assert_eq!(snapshot.details["worker"], AgentDetails::default());
    }

    #[test]
    fn held_and_queued_deliveries_decode_from_the_services_fixtures() {
        let mut snapshot = Snapshot::default();
        snapshot.update(include_bytes!(
            "profiles/runs/fixtures/agents-held-queue.json"
        ));
        let worker = &snapshot.details["worker"];
        assert_eq!(
            worker.held_queue,
            Some(HeldQueue {
                reason: Some("uncertain_delivery".into())
            })
        );
        assert_eq!(worker.queued, 8);
        assert_eq!(
            (
                &snapshot.details["reviewer"].held_queue,
                snapshot.details["reviewer"].queued
            ),
            (&None, 0)
        );
        snapshot.update(include_bytes!("profiles/runs/fixtures/agents-queued.json"));
        let worker = &snapshot.details["worker"];
        assert_eq!((&worker.held_queue, worker.queued), (&None, 3));
        // Today's service gives neither field, and a mistyped one reads as absent.
        snapshot.update(include_bytes!("profiles/runs/fixtures/agents.json"));
        assert_eq!(
            (
                &snapshot.details["worker"].held_queue,
                snapshot.details["worker"].queued
            ),
            (&None, 0)
        );
        snapshot.update(br#"{"items":[{"name":"worker","held_queue":{"reason":7},"queued":-1}]}"#);
        let worker = &snapshot.details["worker"];
        assert_eq!(
            (&worker.held_queue, worker.queued),
            (&Some(HeldQueue::default()), 0)
        );
    }

    #[test]
    fn an_unfinished_restart_decodes_from_the_services_fixture() {
        let mut snapshot = Snapshot::default();
        snapshot.update(include_bytes!(
            "profiles/runs/fixtures/agents-restarting.json"
        ));
        let worker = &snapshot.details["worker"];
        assert_eq!(
            worker.restart,
            Some(Restart {
                id: Some(12),
                phase: Some("waiting".into()),
                resume: Some(false),
                result: None,
            })
        );
        // This service's fixture has no queued count yet: it reads as none.
        assert_eq!(worker.queued, 0);
        assert_eq!(snapshot.details["reviewer"].restart, None);
        snapshot
            .update(br#"{"items":[{"name":"worker","restart":null},{"name":"x","restart":[12]}]}"#);
        assert_eq!(
            (
                &snapshot.details["worker"].restart,
                &snapshot.details["x"].restart
            ),
            (&None, &None)
        );
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

    #[test]
    fn a_uuid_is_read_with_or_without_hyphens() {
        assert!(is_uuid("019a7b2c5d107c3e9a412f6b8c0d1e23"));
        assert!(is_uuid("019a7b2c-5d10-7c3e-9a41-2f6b8c0d1e23"));
        for refused in [
            "019A7B2C-5D10-7C3E-9A41-2F6B8C0D1E23",
            "019a7b2c5d10-7c3e-9a41-2f6b-8c0d1e23abcd",
            "019a7b2c-5d10-7c3e-9a41-2f6b8c0d1e2",
            "../a7b2c-5d10-7c3e-9a41-2f6b8c0d1e23",
            "",
        ] {
            assert!(!is_uuid(refused), "{refused}");
        }
    }
}
