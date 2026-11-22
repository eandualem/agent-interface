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
    /// Durable registration from the profile, when reported.
    pub registration_id: Option<String>,
    /// Machine serving this agent, when the profile reports it.
    pub machine: Option<Machine>,
    /// Whether the provider observes an online agent, including unknown states.
    pub online: Option<bool>,
    /// The model reported by the service and where that value came from.
    pub model: Option<String>,
    /// Whether the runtime reported the model or the service used its configuration.
    pub model_source: Option<String>,
    /// Why the agent is waiting or blocked.
    pub reason: Option<String>,
    /// Where the state came from (`push`, `pull`, `stale`, etc.).
    pub state_source: Option<String>,
    /// Human-readable observations explaining the state (e.g. how old a stale report is).
    pub evidence: Vec<String>,
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
}

/// Machine identity reported by the profile. Names are for display only.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Machine {
    /// Stable machine identifier within the profile (64 hex on the Rust service).
    pub id: String,
    /// Human-readable machine name.
    pub name: String,
}

impl Machine {
    /// Whether this machine matches the connected local machine by full ID.
    pub fn is_local(&self, local: &profiles::Identity) -> bool {
        self.valid() && self.id == local.installation_id
    }

    /// Whether this machine is another machine in the profile by full ID.
    pub fn is_remote(&self, local: &profiles::Identity) -> bool {
        self.valid() && self.id != local.installation_id
    }

    /// The profile through which to open an agent hosted on another machine.
    pub fn remote_profile(&self, local: &profiles::Identity) -> Option<String> {
        self.is_remote(local).then(|| local.profile_id.clone())
    }

    pub(crate) fn valid(&self) -> bool {
        !self.id.is_empty() && self.id.len() <= 256 && !self.id.chars().any(char::is_control)
    }
}

/// Reachability of a profile machine as reported by `GET /api/status/services`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineState {
    /// The machine is reachable.
    Online,
    /// The machine is currently unreachable.
    Offline,
    /// An unfamiliar state value reported by a newer service.
    Unknown,
}

impl MachineState {
    fn from_wire(value: &str) -> Self {
        match value {
            "online" => Self::Online,
            "offline" => Self::Offline,
            _ => Self::Unknown,
        }
    }
}

/// One machine joined to the profile, as reported by `GET /api/status/services`.
/// Machine IDs are compared in full; names are for display only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileMachine {
    /// Stable machine identifier (64 lowercase hex digits on the Rust service).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Whether this machine is the profile's home.
    pub home: bool,
    /// Reachability (`online`, `offline`, or `unknown`).
    pub state: MachineState,
    /// When the machine joined the profile, in RFC 3339.
    pub joined_at: Option<String>,
    /// When the home last heard from this machine, in RFC 3339.
    pub last_seen: Option<String>,
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

/// As [`lenient`], for an array of strings: non-array or non-string entries are skipped.
pub(crate) fn lenient_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <Box<RawValue> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(decode_strings(raw.get()))
}

pub(crate) fn decode_strings(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<&RawValue>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| serde_json::from_str::<String>(entry.get()).ok())
        .take(32)
        .collect()
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
    /// Profile machines from `GET /api/status/services`; empty when absent or unread.
    pub machines: Vec<ProfileMachine>,
    /// Whether `machines` came from a valid `GET /api/status/services` response
    /// carrying a `machines` array, so a machine missing from it is not in the profile.
    pub machines_read: bool,
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

    /// Apply one response from `GET /api/status/services`, or its absence/failure.
    /// Unreadable or missing machine status leaves agent states intact.
    pub fn update_machines(&mut self, response: Option<&[u8]>) {
        match response.and_then(decode_machines) {
            Some(machines) => {
                self.machines = machines;
                self.machines_read = true;
            }
            None => {
                self.machines.clear();
                self.machines_read = false;
            }
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
        // The last valid occurrence of an agent wins.
        let field = |key: &str| item.get(key).map(|value| value.get());
        details.insert(
            name.clone(),
            AgentDetails {
                registration_id: field("registration_id")
                    .and_then(|value| serde_json::from_str(value).ok()),
                machine: field("machine")
                    .and_then(object::<Machine>)
                    .filter(Machine::valid),
                online: field("online").and_then(|value| serde_json::from_str(value).ok()),
                model: field("model").and_then(|value| serde_json::from_str(value).ok()),
                model_source: field("model_source")
                    .and_then(|value| serde_json::from_str(value).ok()),
                reason: field("reason").and_then(|value| serde_json::from_str(value).ok()),
                state_source: field("state_source")
                    .and_then(|value| serde_json::from_str(value).ok()),
                evidence: field("evidence").map(decode_strings).unwrap_or_default(),
                current_issue: field("current_issue")
                    .and_then(|value| serde_json::from_str(value).ok()),
                state_since: field("state_since")
                    .and_then(|value| serde_json::from_str(value).ok()),
                waiting: field("waiting").and_then(object),
                background: field("background").and_then(Background::read),
                dir: field("dir").and_then(|value| serde_json::from_str(value).ok()),
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

/// Maximum profile machines read from one `GET /api/status/services` reply.
const PROFILE_MACHINES: usize = 128;

fn decode_machines(response: &[u8]) -> Option<Vec<ProfileMachine>> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Raw {
        id: Option<String>,
        #[serde(deserialize_with = "lenient")]
        name: Option<String>,
        #[serde(deserialize_with = "lenient")]
        home: Option<bool>,
        #[serde(deserialize_with = "lenient")]
        state: Option<String>,
        #[serde(deserialize_with = "lenient")]
        joined_at: Option<String>,
        #[serde(deserialize_with = "lenient")]
        last_seen: Option<String>,
    }
    if response.len() > RESPONSE_LIMIT || !within_depth(response) {
        return None;
    }
    let document: BTreeMap<String, &RawValue> = serde_json::from_slice(response).ok()?;
    let raw_machines: Vec<&RawValue> =
        serde_json::from_str(document.get("machines")?.get()).ok()?;
    let mut machines = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for entry in raw_machines {
        let Some(raw) = object::<Raw>(entry.get()) else {
            continue;
        };
        let Some(id) = raw.id else {
            continue;
        };
        let machine = Machine {
            id,
            name: raw.name.unwrap_or_default(),
        };
        if !machine.valid() || !seen.insert(machine.id.clone()) {
            continue;
        }
        machines.push(ProfileMachine {
            id: machine.id,
            name: machine.name,
            home: raw.home.unwrap_or(false),
            state: raw
                .state
                .as_deref()
                .map_or(MachineState::Unknown, MachineState::from_wire),
            joined_at: raw.joined_at,
            last_seen: raw.last_seen,
        });
        if machines.len() == PROFILE_MACHINES {
            break;
        }
    }
    Some(machines)
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
        assert_eq!(worker.model.as_deref(), Some("gpt-example"));
        assert_eq!(worker.model_source.as_deref(), Some("observed"));
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

    #[test]
    fn machine_identity_compares_full_64_hex_ids_not_names_or_prefixes() {
        let local_id = "a".repeat(64);
        let prefix_match = format!("{}b", "a".repeat(63));
        let local = profiles::Identity {
            profile_id: "profile-a".into(),
            installation_id: local_id.clone(),
            lifecycle_id: local_id.clone(),
        };
        let same_id_different_name = Machine {
            id: local_id,
            name: "renamed-host".into(),
        };
        assert!(same_id_different_name.is_local(&local));
        assert!(!same_id_different_name.is_remote(&local));
        assert_eq!(same_id_different_name.remote_profile(&local), None);

        let near_match_same_name = Machine {
            id: prefix_match,
            name: "renamed-host".into(),
        };
        assert!(!near_match_same_name.is_local(&local));
        assert!(near_match_same_name.is_remote(&local));
        assert_eq!(
            near_match_same_name.remote_profile(&local).as_deref(),
            Some("profile-a")
        );
    }

    #[test]
    fn machines_and_stale_evidence_decode_from_services_and_agents() {
        let mut snapshot = Snapshot::default();
        let home_id = "0".repeat(64);
        let worker_id = "1".repeat(64);
        snapshot.update(
            format!(
                r#"{{"items":[
                    {{"name":"remote-worker","registration_id":"0193a1b2c3d47e5f8a9b0c1d2e3f4a5b",
                      "state":"busy","state_source":"stale",
                      "evidence":["last machine report 4m ago", 7],
                      "machine":{{"id":"{worker_id}","name":"build-linux"}}}},
                    {{"name":"unplaced","state":"offline","machine":null,"state_source":"default","evidence":[]}}
                ]}}"#
            )
            .as_bytes(),
        );
        let remote = &snapshot.details["remote-worker"];
        assert_eq!(
            remote.registration_id.as_deref(),
            Some("0193a1b2c3d47e5f8a9b0c1d2e3f4a5b")
        );
        assert_eq!(remote.state_source.as_deref(), Some("stale"));
        assert_eq!(remote.evidence, ["last machine report 4m ago"]);
        assert_eq!(
            remote.machine.as_ref().map(|m| m.id.as_str()),
            Some(worker_id.as_str())
        );
        assert_eq!(snapshot.details["unplaced"].machine, None);

        snapshot.update_machines(Some(
            format!(
                r#"{{"machines":[
                    {{"id":"{home_id}","name":"laptop","home":true,"state":"online",
                      "joined_at":"2026-11-15T18:57:29Z","last_seen":"2026-11-22T12:00:00Z"}},
                    {{"id":"{worker_id}","name":"build-linux","home":false,"state":"offline",
                      "joined_at":"2026-11-19T04:47:54Z","last_seen":"2026-11-22T11:55:00Z"}},
                    {{"id":"{worker_id}","name":"duplicate-ignored","state":"online"}},
                    {{"id":"bad\nid","name":"ctrl","state":"online"}},
                    "not-an-object"
                ]}}"#
            )
            .as_bytes(),
        ));
        assert!(snapshot.machines_read);
        assert_eq!(snapshot.machines.len(), 2);
        assert_eq!(snapshot.machines[0].state, MachineState::Online);
        assert!(snapshot.machines[0].home);
        assert_eq!(snapshot.machines[1].state, MachineState::Offline);
        assert_eq!(
            snapshot.machines[1].last_seen.as_deref(),
            Some("2026-11-22T11:55:00Z")
        );

        // Missing or unreadable services reply clears machines without marking states stale.
        snapshot.update_machines(Some(b"not json"));
        assert!(!snapshot.machines_read);
        assert!(snapshot.machines.is_empty());
        assert!(snapshot.error.is_none());
    }
}
