//! Bounded read-only message history through the private Backbone API.
//! Logical messages retain their original text; delivery evidence and discussion
//! are separate. The client never reads the claiming inbox as history.

use serde::Deserialize;
use std::{collections::HashSet, path::Path, time::Duration};

const LIMIT: usize = 4 * 1024 * 1024;
const PAGES: usize = 20;

/// Why the history cannot currently be read. No response body is exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No authenticated private provider, or transport failure.
    Unavailable,
    /// The provider does not implement interaction history.
    Unsupported,
    /// This specific retained record no longer exists.
    Missing,
    /// The current credential cannot inspect message history.
    Forbidden,
    /// Invalid or oversized provider data.
    Unreadable,
}

impl Error {
    /// Safe text for an unavailable or stale view.
    pub fn message(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "Backbone stopped answering. The last interaction history is still shown."
            }
            Self::Unsupported => {
                "Interaction history isn’t available. This Backbone does not offer it yet."
            }
            Self::Missing => "This message is no longer retained by Backbone.",
            Self::Forbidden => "Interaction history needs the owner's Backbone connection.",
            Self::Unreadable => "Backbone returned unreadable interaction history.",
        }
    }
}

/// One provider-bound read, keeping summary and ledger on the same snapshot.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The verified provider this read came from; never silently rebind discussion.
    pub provider: String,
    /// Every agent's retained message counts and activity marks.
    pub summary: Summary,
    /// The selected agent's messages; absent on the agent list.
    pub history: Option<InteractionPage>,
    /// Independently sampled state; an unavailable read never hides messages.
    pub states: Option<StateSpanPage>,
    /// Why state context is missing on this read.
    pub state_error: Option<Error>,
}

struct Api {
    provider: String,
    key: String,
}

impl Api {
    fn new(directory: &Path) -> Result<Self, Error> {
        let (provider, key) =
            crate::configuration::configured(directory, None).ok_or(Error::Unavailable)?;
        if !provider.starts_with("unix:") || key.is_empty() {
            return Err(Error::Unavailable);
        }
        Ok(Self { provider, key })
    }

    fn get<T: serde::de::DeserializeOwned>(&self, path: &str, detail: bool) -> Result<T, Error> {
        let response = crate::transport::local::response(
            &self.provider[5..],
            path,
            &self.key,
            None,
            LIMIT,
            Duration::from_secs(15),
        )
        .map_err(|_| Error::Unavailable)?;
        match response.status {
            200 => parse(&response.body),
            404 => Err(if detail {
                Error::Missing
            } else {
                Error::Unsupported
            }),
            401 | 403 => Err(Error::Forbidden),
            422 => Err(Error::Unreadable),
            _ => Err(Error::Unavailable),
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    if body.len() > LIMIT || !crate::within_depth(body) {
        return Err(Error::Unreadable);
    }
    #[derive(Deserialize)]
    struct Version {
        schema_version: Option<u32>,
    }
    let version: Version = serde_json::from_slice(body).map_err(|_| Error::Unreadable)?;
    if version.schema_version.is_some_and(|v| v != 1) {
        return Err(Error::Unreadable);
    }
    serde_json::from_slice(body).map_err(|_| Error::Unreadable)
}

fn quote(text: &str) -> Result<String, Error> {
    if text.len() > 4096 || text.chars().any(char::is_control) {
        return Err(Error::Unreadable);
    }
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    Ok(out)
}

fn id_path(id: &str) -> Result<String, Error> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::Unreadable);
    }
    Ok(format!("/api/interactions/{id}"))
}

/// Read a summary and, when selected, a bounded complete ledger. Runs off the UI thread.
pub fn read(
    directory: &Path,
    since: &str,
    until: &str,
    agent: Option<&str>,
) -> Result<Snapshot, Error> {
    let api = Api::new(directory)?;
    let bounds = format!("since={}&until={}", quote(since)?, quote(until)?);
    let summary: Summary = api.get(&format!("/api/interactions/summary?{bounds}"), false)?;
    validate_coverage(&summary.coverage)?;
    for item in &summary.items {
        validate_actor(&item.agent)?;
        if item.ticks.len() > 200
            || item.latest.as_ref().is_some_and(|at| !valid_time(&at.time))
            || item
                .previous
                .as_ref()
                .is_some_and(|at| !valid_time(&at.time))
            || item.ticks.iter().any(|tick| !valid_time(&tick.time))
        {
            return Err(Error::Unreadable);
        }
    }
    let history = if let Some(agent) = agent {
        let path = format!(
            "/api/interactions?{bounds}&agent_id={}&direction=both&limit=200&snapshot={}",
            quote(agent)?,
            quote(&summary.snapshot)?
        );
        let mut all: InteractionPage = api.get(&path, false)?;
        validate_page(&all)?;
        let mut cursors = HashSet::new();
        let mut count = 1;
        while let Some(cursor) = all.next_cursor.take() {
            if count >= PAGES {
                all.coverage.truncated = true;
                all.next_cursor = Some(cursor);
                break;
            }
            if !cursors.insert(cursor.clone()) {
                return Err(Error::Unreadable);
            }
            let page: InteractionPage =
                api.get(&format!("{path}&cursor={}", quote(&cursor)?), false)?;
            validate_page(&page)?;
            if page.snapshot != all.snapshot || page.snapshot != summary.snapshot {
                return Err(Error::Unreadable);
            }
            all.items.extend(page.items);
            all.next_cursor = page.next_cursor;
            all.coverage.truncated |= page.coverage.truncated;
            count += 1;
        }
        if all.snapshot != summary.snapshot {
            return Err(Error::Unreadable);
        }
        let mut ids = HashSet::new();
        if all.items.iter().any(|item| !ids.insert(item.id.clone())) {
            return Err(Error::Unreadable);
        }
        Some(all)
    } else {
        None
    };
    let (states, state_error) = match state_spans(&api, &bounds, agent) {
        Ok(page) => (Some(page), None),
        Err(error) => (None, Some(error)),
    };
    if crate::conversations::provider(directory).as_ref() != Some(&api.provider) {
        return Err(Error::Unavailable);
    }
    Ok(Snapshot {
        provider: api.provider,
        summary,
        history,
        states,
        state_error,
    })
}

fn state_spans(api: &Api, bounds: &str, agent: Option<&str>) -> Result<StateSpanPage, Error> {
    let mut path = format!("/api/interactions/state-spans?{bounds}&limit=500");
    if let Some(agent) = agent {
        path.push_str(&format!("&agent_id={}", quote(agent)?));
    }
    let mut all: StateSpanPage = api.get(&path, false)?;
    validate_states(&all)?;
    path.push_str(&format!("&snapshot={}", quote(&all.snapshot)?));
    let mut cursors = HashSet::new();
    for _ in 1..PAGES {
        let Some(cursor) = all.next_cursor.take() else {
            break;
        };
        if !cursors.insert(cursor.clone()) {
            return Err(Error::Unreadable);
        }
        let page: StateSpanPage = api.get(&format!("{path}&cursor={}", quote(&cursor)?), false)?;
        validate_states(&page)?;
        if page.snapshot != all.snapshot {
            return Err(Error::Unreadable);
        }
        all.items.extend(page.items);
        all.next_cursor = page.next_cursor;
    }
    if all.next_cursor.is_some() {
        all.coverage.reasons.push("client_page_limit".into());
    }
    Ok(all)
}

fn validate_states(page: &StateSpanPage) -> Result<(), Error> {
    if page.items.len() > 500
        || !valid_time(&page.coverage.since)
        || !valid_time(&page.coverage.until)
        || !page.coverage.unknown_outside_returned_spans
        || !page.coverage.legacy_name_only_excluded
    {
        return Err(Error::Unreadable);
    }
    for span in &page.items {
        let start = crate::views::epoch_seconds(&span.since).ok_or(Error::Unreadable)?;
        let end = crate::views::epoch_seconds(&span.until).ok_or(Error::Unreadable)?;
        if span.agent_id.is_empty()
            || start > end
            || span.time_basis != "observed"
            || !valid_time(&span.first_observed_at)
            || !valid_time(&span.last_observed_at)
        {
            return Err(Error::Unreadable);
        }
    }
    Ok(())
}

/// Read an immutable message outside the current window and its bounded evidence pages.
pub fn detail(directory: &Path, provider: &str, id: &str) -> Result<Interaction, Error> {
    let api = Api::new(directory)?;
    if api.provider != provider {
        return Err(Error::Unavailable);
    }
    let path = id_path(id)?;
    let mut item: Interaction = api.get(&path, true)?;
    validate_record(&item)?;
    if item.id != id {
        return Err(Error::Unreadable);
    }
    let mut cursors = HashSet::new();
    for _ in 1..PAGES {
        let Some(cursor) = item.attempts.next_cursor.take() else {
            break;
        };
        if !cursors.insert(cursor.clone()) {
            return Err(Error::Unreadable);
        }
        let page: AttemptPage = api.get(
            &format!("{path}/attempts?limit=200&cursor={}", quote(&cursor)?),
            true,
        )?;
        if page.items.len() > 200 {
            return Err(Error::Unreadable);
        }
        item.attempts.items.extend(page.items);
        item.attempts.next_cursor = page.next_cursor;
    }
    cursors.clear();
    for _ in 1..PAGES {
        let Some(cursor) = item.lifecycle.next_cursor.take() else {
            break;
        };
        if !cursors.insert(cursor.clone()) {
            return Err(Error::Unreadable);
        }
        let page: EventPage = api.get(
            &format!("{path}/events?limit=200&cursor={}", quote(&cursor)?),
            true,
        )?;
        if page.items.len() > 200 {
            return Err(Error::Unreadable);
        }
        item.lifecycle.items.extend(page.items);
        item.lifecycle.next_cursor = page.next_cursor;
    }
    validate_record(&item)?;
    if crate::conversations::provider(directory).as_deref() != Some(provider) {
        return Err(Error::Unavailable);
    }
    Ok(item)
}

fn valid_time(text: &str) -> bool {
    crate::views::epoch_seconds(text).is_some()
}

fn validate_actor(actor: &Actor) -> Result<(), Error> {
    if !matches!(
        actor.kind.as_str(),
        "agent" | "owner" | "external" | "unknown"
    ) || (actor.kind == "agent" && actor.id.as_deref().is_none_or(str::is_empty))
        || (actor.kind == "unknown" && actor.id.is_some())
    {
        return Err(Error::Unreadable);
    }
    Ok(())
}
fn validate_coverage(coverage: &Coverage) -> Result<(), Error> {
    let since = crate::views::epoch_seconds(&coverage.since).ok_or(Error::Unreadable)?;
    let until = crate::views::epoch_seconds(&coverage.until).ok_or(Error::Unreadable)?;
    if since >= until
        || !valid_time(&coverage.as_of)
        || [
            &coverage.capture_started_at,
            &coverage.retained_from,
            &coverage.body_retained_from,
        ]
        .into_iter()
        .flatten()
        .any(|time| !valid_time(time))
    {
        return Err(Error::Unreadable);
    }
    for interval in &coverage.message_intervals {
        let a = crate::views::epoch_seconds(&interval.since).ok_or(Error::Unreadable)?;
        let b = crate::views::epoch_seconds(&interval.until).ok_or(Error::Unreadable)?;
        if a >= b || interval.completeness != "complete_within_declared_scope" {
            return Err(Error::Unreadable);
        }
    }
    Ok(())
}
fn validate_page(page: &InteractionPage) -> Result<(), Error> {
    validate_coverage(&page.coverage)?;
    if page.items.len() > 200 {
        return Err(Error::Unreadable);
    }
    for item in &page.items {
        validate_record(item)?;
    }
    Ok(())
}
fn validate_record(item: &Interaction) -> Result<(), Error> {
    id_path(&item.id)?;
    validate_actor(&item.sender)?;
    validate_actor(&item.recipient)?;
    if !valid_time(&item.accepted_at)
        || !valid_time(&item.last_event_at)
        || item
            .status
            .observed_at
            .as_ref()
            .is_some_and(|at| !valid_time(at))
        || !matches!(item.body.level.as_str(), "full" | "excerpt" | "unavailable")
        || (item.body.level == "full" && item.body.text.is_none())
        || item
            .lifecycle
            .items
            .iter()
            .any(|event| !valid_time(&event.time))
        || item.attempts.items.iter().any(|attempt| {
            !valid_time(&attempt.recorded_at)
                || attempt
                    .settled_at
                    .as_ref()
                    .is_some_and(|at| !valid_time(at))
        })
    {
        return Err(Error::Unreadable);
    }
    if let Some(entry) = &item.conversation_entry
        && (!valid_time(&entry.created_at)
            || entry.artifact_id.is_empty()
            || entry.entry_id == 0
            || (entry.kind == "interaction"
                && entry.anchored_interaction_id.as_deref() != Some(entry.artifact_id.as_str())))
    {
        return Err(Error::Unreadable);
    }
    if item.conversation.supported
        && (item.conversation.kind != "interaction" || item.conversation.artifact_id != item.id)
    {
        return Err(Error::Unreadable);
    }
    for recipient in &item.conversation.recipients {
        validate_actor(recipient)?;
        if recipient.kind != "agent"
            || (recipient.id != item.sender.id && recipient.id != item.recipient.id)
        {
            return Err(Error::Unreadable);
        }
    }
    Ok(())
}

/// Actor from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Actor {
    /// Kind.
    pub kind: String,
    /// Id.
    pub id: Option<String>,
    /// Name.
    pub name: String,
    /// Name at capture.
    pub name_at_capture: String,
    /// Attribution.
    pub attribution: String,
    /// Retired.
    #[serde(default)]
    pub retired: bool,
}

/// Body from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Body {
    /// Level.
    pub level: String,
    /// Text.
    pub text: Option<String>,
    /// Reason.
    pub reason: Option<String>,
    /// Format.
    #[serde(default)]
    pub format: String,
}

/// Status from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Status {
    /// Value.
    pub value: String,
    /// Observed at.
    pub observed_at: Option<String>,
    /// Evidence.
    pub evidence: String,
    /// Delivered.
    pub delivered: bool,
}

/// RelationTarget from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RelationTarget {
    /// Kind.
    pub kind: String,
    /// Id.
    pub id: String,
    /// Artifact kind.
    #[serde(default)]
    pub artifact_kind: Option<String>,
    /// Entry id.
    #[serde(default)]
    pub entry_id: Option<u64>,
}

/// Relation from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Relation {
    /// Actual recorded origin, including a reverse link projected onto its target.
    #[serde(default)]
    pub source_interaction_id: Option<String>,
    /// Type.
    #[serde(rename = "type")]
    pub relation: String,
    /// Target.
    pub target: RelationTarget,
    /// Recorded at.
    pub recorded_at: String,
    /// Provenance.
    pub provenance: String,
}

/// LifecycleEvent from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LifecycleEvent {
    /// Id.
    pub id: String,
    /// Kind.
    pub kind: String,
    /// Time.
    pub time: String,
    /// Time basis.
    pub time_basis: String,
    /// Outcome.
    pub outcome: Option<String>,
    /// Attempt id.
    pub attempt_id: Option<String>,
    /// Source.
    pub source: String,
}

/// Attempt from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Attempt {
    /// Id.
    pub id: String,
    /// Operation id.
    pub operation_id: String,
    /// Delivery id.
    pub delivery_id: Option<u64>,
    /// Queue id.
    pub queue_id: Option<u64>,
    /// Outcome.
    pub outcome: String,
    /// Source.
    pub source: String,
    /// Recorded at.
    pub recorded_at: String,
    /// Settled at.
    pub settled_at: Option<String>,
}

/// AttemptPage from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AttemptPage {
    /// Items.
    pub items: Vec<Attempt>,
    /// Next cursor.
    pub next_cursor: Option<String>,
}

/// EventPage from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct EventPage {
    /// Items.
    pub items: Vec<LifecycleEvent>,
    /// Next cursor.
    pub next_cursor: Option<String>,
}

/// ConversationCapability from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ConversationCapability {
    /// Supported.
    pub supported: bool,
    /// Kind.
    #[serde(default)]
    pub kind: String,
    /// Artifact id.
    pub artifact_id: String,
    /// Recipients.
    pub recipients: Vec<Actor>,
    /// Reason.
    pub reason: Option<String>,
    /// Total.
    pub total: u64,
    /// Last entry at.
    pub last_entry_at: Option<String>,
}

/// Interaction from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Interaction {
    /// Id.
    pub id: String,
    /// Operation id.
    pub operation_id: String,
    /// Accepted at.
    pub accepted_at: String,
    /// Last event at.
    pub last_event_at: String,
    /// Kind.
    pub kind: String,
    /// Sender.
    pub sender: Actor,
    /// Recipient.
    pub recipient: Actor,
    /// Original target.
    pub original_target: String,
    /// Preview.
    pub preview: String,
    /// Body.
    pub body: Body,
    /// Status.
    pub status: Status,
    /// Attempt count.
    pub attempt_count: u64,
    /// Attempts.
    pub attempts: AttemptPage,
    /// Lifecycle.
    pub lifecycle: EventPage,
    /// Relations.
    pub relations: Vec<Relation>,
    /// More referring records exist than the bounded projection returned.
    #[serde(default)]
    pub relations_truncated: bool,
    /// Conversation.
    pub conversation: ConversationCapability,
    /// An actual captured discussion entry, never reconstructed from counts.
    #[serde(default)]
    pub conversation_entry: Option<ConversationEntryMetadata>,
}

/// Coverage from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Coverage {
    /// Since.
    pub since: String,
    /// Until.
    pub until: String,
    /// Capture started at.
    pub capture_started_at: Option<String>,
    /// Retained from.
    pub retained_from: Option<String>,
    /// Uniform full-body retention; never used as the history edge.
    #[serde(default)]
    pub body_retained_from: Option<String>,
    /// As of.
    pub as_of: String,
    /// Completeness.
    pub completeness: String,
    /// Reasons.
    pub reasons: Vec<String>,
    /// Declared scope.
    pub declared_scope: Vec<String>,
    /// Truncated.
    pub truncated: bool,
    /// State history.
    #[serde(default)]
    pub state_history: String,
    /// Verified message capture intervals for quiet-row assertions.
    #[serde(default)]
    pub message_intervals: Vec<MessageCoverageInterval>,
    /// Declared population whose absence may be shown as quiet.
    #[serde(default)]
    pub quiet_scope: String,
}

/// InteractionPage from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct InteractionPage {
    /// Version of the read projection.
    pub schema_version: u32,
    /// Items.
    pub items: Vec<Interaction>,
    /// Next cursor.
    pub next_cursor: Option<String>,
    /// Snapshot.
    pub snapshot: String,
    /// Coverage.
    pub coverage: Coverage,
}

/// Counts from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Counts {
    /// Sent.
    pub sent: u64,
    /// Received.
    pub received: u64,
    /// Not delivered.
    pub not_delivered: u64,
}

/// Tick from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Tick {
    /// Interaction id.
    pub interaction_id: String,
    /// Direction.
    pub direction: String,
    /// Time.
    pub time: String,
    /// Time basis.
    pub time_basis: String,
    /// Delivered.
    pub delivered: bool,
}

/// LastInteraction from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LastInteraction {
    /// Id.
    pub id: String,
    /// Time.
    pub time: String,
}

/// AgentSummary from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AgentSummary {
    /// Agent.
    pub agent: Actor,
    /// Runtime.
    pub runtime: String,
    /// State.
    pub state: String,
    /// Reason.
    pub reason: Option<String>,
    /// Counts.
    pub counts: Counts,
    /// Counterparties.
    pub counterparties: Vec<Actor>,
    /// Latest.
    pub latest: Option<LastInteraction>,
    /// Previous.
    pub previous: Option<LastInteraction>,
    /// Ticks.
    pub ticks: Vec<Tick>,
    /// Ticks truncated.
    pub ticks_truncated: bool,
    /// Conversation supported.
    pub conversation_supported: bool,
}

/// Summary from the version-one interaction projection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Summary {
    /// Version of the read projection.
    pub schema_version: u32,
    /// Items.
    pub items: Vec<AgentSummary>,
    /// Snapshot.
    pub snapshot: String,
    /// Coverage.
    pub coverage: Coverage,
    /// Conversation supported.
    pub conversation_supported: bool,
}

/// Identity and context of a real captured conversation entry.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ConversationEntryMetadata {
    /// Artifact conversation kind.
    pub kind: String,
    /// Stable artifact identity.
    pub artifact_id: String,
    /// Stable entry identity within the provider.
    pub entry_id: u64,
    /// Message or an explicitly recorded artifact event.
    pub entry_kind: String,
    /// Entry creation time, independent of later delivery evidence.
    pub created_at: String,
    /// Owner, agent or system.
    pub author_type: String,
    /// Captured author registration, when present.
    pub author_id: String,
    /// Display name at capture.
    pub author_name: String,
    /// Actual addressed registration, when present.
    pub recipient_id: Option<String>,
    /// Captured recipient name.
    pub recipient_name: Option<String>,
    /// Explicitly recorded reply entry.
    pub in_reply_to: Option<u64>,
    /// Original structured artifact anchor; never interpreted as instructions.
    pub anchor: Option<serde_json::Value>,
    /// The immutable original when this is an interaction discussion.
    pub anchored_interaction_id: Option<String>,
}

/// A period of complete message capture within the declared paths.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MessageCoverageInterval {
    /// Inclusive UTC start.
    pub since: String,
    /// Exclusive UTC end.
    pub until: String,
    /// Must be complete within the declared scope to support a quiet row.
    pub completeness: String,
}

/// A state span grounded in matching samples for one stable registration.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ObservedStateSpan {
    /// Stable observation-span identity.
    pub id: String,
    /// Registration identity, never a name-only join.
    pub agent_id: String,
    /// Name at the time of observation.
    pub name_at_observation: String,
    /// Inclusive start; a point observation may have no duration.
    pub since: String,
    /// End of observed coverage.
    pub until: String,
    /// Observed runtime state, not inferred from communication.
    pub state: String,
    /// Recorded explanation, if any.
    pub reason: Option<String>,
    /// First sample time.
    pub first_observed_at: String,
    /// Last sample time.
    pub last_observed_at: String,
    /// Actual samples supporting the span.
    pub observation_ids: Vec<String>,
    /// Matching samples or a single point observation.
    pub basis: String,
    /// This read uses observed time only.
    pub time_basis: String,
    /// Provider-recorded provenance labels.
    pub evidence: Vec<String>,
}

/// State sampling coverage is separate from message capture coverage.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StateCoverage {
    /// Requested start.
    pub since: String,
    /// Requested end.
    pub until: String,
    /// Earliest retained stable-registration observation.
    pub retained_from: Option<String>,
    /// Sampled partial or unavailable; never continuous inferred history.
    pub completeness: String,
    /// Missing-coverage reasons.
    pub reasons: Vec<String>,
    /// Largest interval between samples the provider will bridge.
    pub max_sample_gap_seconds: u64,
    /// Legacy observations cannot be safely assigned by name.
    pub legacy_name_only_excluded: bool,
    /// Everything outside returned spans must stay unknown.
    pub unknown_outside_returned_spans: bool,
}

/// A bounded, independently snapshot-bound page of observed states.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StateSpanPage {
    /// Wire version.
    pub schema_version: u32,
    /// Explicit spans, each carrying its stable agent identity.
    pub items: Vec<ObservedStateSpan>,
    /// Sampling coverage.
    pub coverage: StateCoverage,
    /// Token shared by this read's pages.
    pub snapshot: String,
    /// More retained observations exist when present.
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_fixtures_preserve_originals_missing_text_identities_and_logical_counts() {
        let page: InteractionPage = parse(include_bytes!(
            "../tests/fixtures/interactions/history.json"
        ))
        .unwrap();
        assert_eq!(page.items[0].body.level, "full");
        assert_eq!(page.items[1].sender.kind, "owner");
        assert_eq!(page.items[2].sender.id, None);
        assert_eq!(page.items[2].body.level, "excerpt");
        assert!(!page.items[2].status.delivered);
        assert_eq!(page.items[0].relations[0].target.kind, "owner_request");
        assert_eq!(page.coverage.completeness, "partial");
        let detail: Interaction =
            parse(include_bytes!("../tests/fixtures/interactions/detail.json")).unwrap();
        assert_eq!(detail.body, page.items[0].body);
        let summary: Summary = parse(include_bytes!(
            "../tests/fixtures/interactions/summary.json"
        ))
        .unwrap();
        assert_eq!(summary.items[0].counts.received, 1);
        assert_eq!(summary.items[1].state, "offline");
        assert!(summary.items[1].latest.is_none());
        assert!(parse::<Summary>(b"{\"schema_version\":2}").is_err());
        assert!(parse::<Interaction>(b"{}").is_err());
        assert!(quote("unsafe\ncursor").is_err());
        assert_eq!(quote("a+b/c=").unwrap(), "a%2Bb%2Fc%3D");
        assert!(id_path("../inbox").is_err());
    }

    #[test]
    fn observed_spans_and_source_entries_remain_separate_from_message_coverage() {
        let states: StateSpanPage = parse(include_bytes!(
            "../tests/fixtures/interactions/state-spans.json"
        ))
        .unwrap();
        assert_eq!(validate_states(&states), Ok(()));
        assert_eq!(states.items[0].agent_id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(states.items[0].basis, "matching_samples");
        assert_eq!(states.coverage.completeness, "sampled_partial");
        assert!(states.coverage.unknown_outside_returned_spans);
        let entry: ConversationEntryMetadata = parse(include_bytes!(
            "../tests/fixtures/interactions/conversation-entry.json"
        ))
        .unwrap();
        assert_eq!(entry.entry_id, 501);
        assert_eq!(entry.author_type, "owner");
        assert_eq!(
            entry.anchored_interaction_id.as_deref(),
            Some(entry.artifact_id.as_str())
        );
        let mut invalid = states;
        invalid.items[0].until = "not a time".into();
        assert_eq!(validate_states(&invalid), Err(Error::Unreadable));
    }

    #[test]
    fn malformed_evidence_cannot_be_rendered_as_retained_text_or_a_new_recipient() {
        let valid: Interaction =
            parse(include_bytes!("../tests/fixtures/interactions/detail.json")).unwrap();
        assert_eq!(validate_record(&valid), Ok(()));
        let mut broken = valid.clone();
        broken.body.text = None;
        assert_eq!(validate_record(&broken), Err(Error::Unreadable));
        let mut broken = valid.clone();
        broken.accepted_at = "not a time".into();
        assert_eq!(validate_record(&broken), Err(Error::Unreadable));
        let mut broken = valid.clone();
        broken.conversation.artifact_id = "another-message".into();
        assert_eq!(validate_record(&broken), Err(Error::Unreadable));
        let mut broken = valid;
        broken.conversation.recipients[0].id = Some("unrelated-registration".into());
        assert_eq!(validate_record(&broken), Err(Error::Unreadable));
    }

    #[test]
    fn absent_routes_and_absent_records_are_different_and_never_claim_inbox() {
        use std::{
            io::{Read, Write},
            os::unix::{fs::PermissionsExt, net::UnixListener},
        };
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = directory.path().join("history.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let worker = std::thread::spawn(move || {
            for expected in [
                "GET /api/interactions?limit=1 ",
                "GET /api/interactions/missing ",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = [0; 4096];
                let size = stream.read(&mut bytes).unwrap();
                assert!(String::from_utf8_lossy(&bytes[..size]).starts_with(expected));
                stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            }
        });
        let api = Api {
            provider: format!("unix:{}", socket.display()),
            key: "fixture-key".into(),
        };
        assert_eq!(
            api.get::<InteractionPage>("/api/interactions?limit=1", false),
            Err(Error::Unsupported)
        );
        assert_eq!(
            api.get::<Interaction>("/api/interactions/missing", true),
            Err(Error::Missing)
        );
        worker.join().unwrap();
    }
}
