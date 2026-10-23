//! Read-only data for the Backbone views a pane can show.
//!
//! Each [`Request`] is one or two GETs of fixed local API paths, with the same
//! address, credential, proxy and redirect rules as the roster. Decoding keeps
//! only the fields a view draws; an item that does not decode is skipped.

use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::{configuration::configured, transport::get, valid_agent, within_depth};

const VIEW_LIMIT: usize = 4 * 1024 * 1024;

/// Why a view has no data. The text is fixed and contains no response content.
pub const UNAVAILABLE: &str = "Backbone is not answering.";
/// A response this version cannot read.
pub const UNREADABLE: &str = "Backbone answered with data this version cannot read.";

/// What a view asks Backbone for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Every agent and the active swarms.
    Team,
    /// Token usage over a recent period.
    Usage(Range),
    /// Recent deliveries and inbound events.
    Activity,
    /// The shared skills store.
    Skills,
    /// One agent's card.
    Agent(String),
}

/// A recent period ending now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// The last hour.
    Hour,
    /// The last 24 hours.
    Day,
    /// The last seven days.
    Week,
}

impl Range {
    /// Every range, shortest first.
    pub const ALL: [Self; 3] = [Self::Hour, Self::Day, Self::Week];

    /// A short label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hour => "1 h",
            Self::Day => "24 h",
            Self::Week => "7 d",
        }
    }

    fn seconds(self) -> u64 {
        match self {
            Self::Hour => 3_600,
            Self::Day => 86_400,
            Self::Week => 7 * 86_400,
        }
    }
}

/// The data for one request.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    /// See [`Team`].
    Team(Team),
    /// See [`Usage`].
    Usage(Usage),
    /// See [`Activity`].
    Activity(Activity),
    /// See [`Skills`].
    Skills(Skills),
    /// See [`Agent`].
    Agent(Box<Agent>),
}

/// Read one request from an enabled provider. Blocks for up to three seconds
/// per GET, so call it on an I/O worker. Configuration is re-read every time.
pub fn read(data_dir: &Path, url: Option<&str>, request: &Request) -> Result<Data, &'static str> {
    let (url, key) = configured(data_dir, url).ok_or(UNAVAILABLE)?;
    let fetch = |path: &str| get(&url, path, &key, VIEW_LIMIT).ok_or(UNAVAILABLE);
    match request {
        Request::Team => Ok(Data::Team(Team::decode(
            &fetch("/api/agents")?,
            &fetch("/api/swarms")?,
        )?)),
        Request::Usage(range) => {
            let since = iso(now().saturating_sub(range.seconds()));
            let path = format!("/api/usage?refresh=false&limit=1000&since={since}");
            Ok(Data::Usage(Usage::decode(&fetch(&path)?)?))
        }
        Request::Activity => Ok(Data::Activity(Activity::decode(
            &fetch("/api/deliveries?limit=200")?,
            &fetch("/api/events?limit=200")?,
        )?)),
        Request::Skills => Ok(Data::Skills(Skills::decode(&fetch("/api/skills")?)?)),
        Request::Agent(name) => {
            if !valid_agent(name) {
                return Err(UNREADABLE);
            }
            let path = format!("/api/agents/{name}/inspect");
            Ok(Data::Agent(Box::new(Agent::decode(&fetch(&path)?)?)))
        }
    }
}

/// Agents and the swarms they work in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Team {
    /// Configured agents in Backbone's order.
    pub members: Vec<Member>,
    /// Swarms that are still running.
    pub swarms: Vec<Swarm>,
}

/// One agent in the team.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Member {
    /// Agent name.
    pub name: String,
    /// The CLI it runs in.
    pub runtime: String,
    /// The repository it owns, or empty.
    pub repo: String,
    /// Its tags.
    pub tags: Vec<String>,
    /// Its reported state.
    pub state: String,
    /// Why it is waiting or blocked, when reported.
    pub reason: Option<String>,
    /// The issue it works on, when reported.
    pub current_issue: Option<u64>,
    /// When the state began, in seconds since the Unix epoch.
    pub state_since: Option<f64>,
    /// False for a tmux session Backbone lists but does not manage.
    pub configured: Option<bool>,
}

/// A running swarm.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Swarm {
    /// Swarm name.
    pub name: String,
    /// Its repository.
    pub repo: String,
    /// The issue it works on.
    pub issue: Option<u64>,
    /// Member agent names.
    pub members: Vec<String>,
}

impl Team {
    fn decode(agents: &[u8], swarms: &[u8]) -> Result<Self, &'static str> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct RawSwarm {
            name: String,
            repo: String,
            issue_number: Option<u64>,
            status: String,
            members: Vec<Box<RawValue>>,
        }
        let members = items::<Member>(&document(agents)?)?
            .into_iter()
            .filter(|member| valid_agent(&member.name) && member.configured != Some(false))
            .collect();
        let swarms = items::<RawSwarm>(&document(swarms)?)?
            .into_iter()
            .filter(|swarm| !matches!(swarm.status.as_str(), "done" | "disbanded" | "failed"))
            .map(|swarm| Swarm {
                name: swarm.name,
                repo: swarm.repo,
                issue: swarm.issue_number,
                members: swarm
                    .members
                    .iter()
                    .filter_map(|m| member_name(m))
                    .collect(),
            })
            .collect();
        Ok(Self { members, swarms })
    }
}

// A member is a name, or an object with one.
fn member_name(raw: &RawValue) -> Option<String> {
    #[derive(Deserialize)]
    struct Named {
        name: String,
    }
    serde_json::from_str::<String>(raw.get())
        .or_else(|_| serde_json::from_str::<Named>(raw.get()).map(|named| named.name))
        .ok()
        .filter(|name| valid_agent(name))
}

/// Token counts, kept disjoint as Backbone reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Tokens {
    /// Uncached input.
    #[serde(rename = "input_tokens")]
    pub input: u64,
    /// Cache reads.
    #[serde(rename = "cache_read_tokens")]
    pub cache_read: u64,
    /// Cache writes.
    #[serde(rename = "cache_write_tokens")]
    pub cache_write: u64,
    /// Output, including reasoning.
    #[serde(rename = "output_tokens")]
    pub output: u64,
    /// All of the above.
    #[serde(rename = "total_tokens")]
    pub total: u64,
}

impl std::ops::AddAssign for Tokens {
    fn add_assign(&mut self, other: Self) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.total += other.total;
    }
}

/// One CLI session's usage.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Backbone's identity for the session record.
    pub id: String,
    /// The agent it belongs to.
    #[serde(rename = "agent_name")]
    pub agent: String,
    /// The CLI.
    pub runtime: String,
    /// The parent session, for a subagent.
    #[serde(rename = "parent_id")]
    pub parent: Option<String>,
    /// Models seen in the session.
    pub models: Vec<String>,
    /// How completely the counts were measured.
    pub coverage: String,
    /// Whether it is the agent's current session.
    pub current: bool,
    /// Its tokens.
    #[serde(flatten)]
    pub tokens: Tokens,
}

/// An agent with no readable usage record.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Unmeasured {
    /// Agent name.
    #[serde(rename = "agent_name")]
    pub agent: String,
    /// The CLI.
    pub runtime: String,
    /// Backbone's reason.
    pub reason: String,
}

/// Usage over one range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    /// Totals over every session in the range.
    pub totals: Tokens,
    /// Overall coverage of the totals.
    pub coverage: String,
    /// Sessions in the range.
    pub sessions: Vec<Session>,
    /// Agents with no readable record.
    pub unmeasured: Vec<Unmeasured>,
    /// Whether more sessions exist than were returned.
    pub more: bool,
}

impl Usage {
    fn decode(body: &[u8]) -> Result<Self, &'static str> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Totals {
            #[serde(flatten)]
            tokens: Tokens,
            coverage: String,
        }
        let document = document(body)?;
        let field = |name: &str| document.get(name).map(|raw| raw.get());
        let totals: Totals = field("totals")
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        Ok(Self {
            totals: totals.tokens,
            coverage: totals.coverage,
            sessions: items(&document)?,
            unmeasured: list(&document, "unavailable"),
            more: field("has_more") == Some("true"),
        })
    }
}

/// One recent record: a delivery attempt or an inbound event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    /// When it happened, in seconds since the Unix epoch.
    pub at: Option<f64>,
    /// Its kind, such as `direct_message` or `comment_created`.
    pub kind: String,
    /// Where it came from and went.
    pub route: String,
    /// What Backbone did with it.
    pub outcome: String,
    /// A short preview.
    pub preview: String,
    /// Whether it is an inbound event rather than a delivery.
    pub inbound: bool,
}

/// Recent deliveries and events, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    /// The records.
    pub entries: Vec<Entry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Delivery {
    kind: String,
    repo: String,
    issue_number: Option<u64>,
    session_name: String,
    outcome: String,
    preview: String,
    created_at: String,
}

impl From<Delivery> for Entry {
    fn from(delivery: Delivery) -> Self {
        let route = match delivery.issue_number {
            Some(issue) if !delivery.repo.is_empty() => {
                format!("{}#{issue} → {}", delivery.repo, delivery.session_name)
            }
            _ => format!("→ {}", delivery.session_name),
        };
        Self {
            at: epoch_seconds(&delivery.created_at),
            kind: delivery.kind,
            route,
            outcome: delivery.outcome,
            preview: delivery.preview,
            inbound: false,
        }
    }
}

impl Activity {
    fn decode(deliveries: &[u8], events: &[u8]) -> Result<Self, &'static str> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Event {
            source: String,
            repo: String,
            event_type: String,
            issue_number: Option<u64>,
            sender: String,
            summary: String,
            received_at: String,
            outcome: Option<String>,
        }
        let mut entries: Vec<Entry> = items::<Delivery>(&document(deliveries)?)?
            .into_iter()
            .map(Entry::from)
            .collect();
        entries.extend(
            items::<Event>(&document(events)?)?
                .into_iter()
                .map(|event| {
                    let place = match event.issue_number {
                        Some(issue) => format!("{}#{issue}", event.repo),
                        None => event.repo,
                    };
                    Entry {
                        at: epoch_seconds(&event.received_at),
                        kind: event.event_type,
                        route: [event.source, place, event.sender]
                            .into_iter()
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · "),
                        outcome: event.outcome.unwrap_or_default(),
                        preview: event.summary,
                        inbound: true,
                    }
                }),
        );
        entries.sort_by(|a, b| b.at.unwrap_or(0.0).total_cmp(&a.at.unwrap_or(0.0)));
        Ok(Self { entries })
    }
}

/// One entry of the shared skills store.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Skill {
    /// Skill name.
    pub name: String,
    /// When to use it.
    pub description: String,
    /// The tags it is shared by.
    pub tags: Vec<String>,
    /// The agents it reaches.
    pub reaches: Vec<String>,
    /// Why the entry is invalid, when it is.
    pub error: Option<String>,
}

/// The shared skills store.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skills {
    /// The store's directory, or none when the store is disabled.
    pub store: Option<String>,
    /// Its entries in Backbone's order.
    pub items: Vec<Skill>,
}

impl Skills {
    fn decode(body: &[u8]) -> Result<Self, &'static str> {
        let document = document(body)?;
        Ok(Self {
            store: document
                .get("store")
                .and_then(|raw| serde_json::from_str(raw.get()).ok()),
            items: items(&document)?,
        })
    }
}

/// A recent delivery to one agent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Received {
    /// Its kind.
    pub kind: String,
    /// What happened to it.
    pub outcome: String,
    /// A short preview.
    pub preview: String,
    /// When, in seconds since the Unix epoch.
    pub at: Option<f64>,
}

/// One agent's card.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Agent {
    /// Agent name.
    pub name: String,
    /// Whether Backbone knows it.
    pub known: bool,
    /// Whether its session is running.
    pub online: bool,
    /// The CLI.
    pub runtime: String,
    /// The configured model, when set.
    pub model: Option<String>,
    /// Its working directory.
    pub dir: String,
    /// The repository it owns.
    pub repo: String,
    /// Repositories it watches.
    pub watches: Vec<String>,
    /// Its tags.
    pub tags: Vec<String>,
    /// Its state.
    pub state: String,
    /// Why it is waiting or blocked.
    pub reason: Option<String>,
    /// What the CLI said about a blocked state.
    pub detail: Option<String>,
    /// The issue it works on.
    pub current_issue: Option<u64>,
    /// The repository of that issue.
    pub current_repo: Option<String>,
    /// How long it has been in its state.
    pub state_age_seconds: Option<f64>,
    /// Its last reply, clipped by Backbone.
    pub last_message: Option<String>,
    /// Its most recent deliveries.
    #[serde(skip)]
    pub received: Vec<Received>,
}

impl Agent {
    fn decode(body: &[u8]) -> Result<Self, &'static str> {
        let document = document(body)?;
        let raw = std::str::from_utf8(body).map_err(|_| UNREADABLE)?;
        let mut agent: Self = serde_json::from_str(raw).map_err(|_| UNREADABLE)?;
        agent.received = list::<Delivery>(&document, "recent_deliveries")
            .into_iter()
            .map(|delivery| Received {
                at: epoch_seconds(&delivery.created_at),
                kind: delivery.kind,
                outcome: delivery.outcome,
                preview: delivery.preview,
            })
            .collect();
        Ok(agent)
    }
}

type Document<'a> = BTreeMap<String, &'a RawValue>;

fn document(body: &[u8]) -> Result<Document<'_>, &'static str> {
    if body.len() > VIEW_LIMIT || !within_depth(body) {
        return Err(UNREADABLE);
    }
    serde_json::from_slice(body).map_err(|_| UNREADABLE)
}

// The required `items` array, skipping entries that do not decode.
fn items<T: DeserializeOwned>(document: &Document<'_>) -> Result<Vec<T>, &'static str> {
    let items: Vec<&RawValue> = document
        .get("items")
        .and_then(|raw| serde_json::from_str(raw.get()).ok())
        .ok_or(UNREADABLE)?;
    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_str(item.get()).ok())
        .collect())
}

fn list<T: DeserializeOwned>(document: &Document<'_>, name: &str) -> Vec<T> {
    document
        .get(name)
        .and_then(|raw| serde_json::from_str::<Vec<&RawValue>>(raw.get()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| serde_json::from_str(item.get()).ok())
        .collect()
}

/// Seconds since the Unix epoch now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Seconds since the Unix epoch of a UTC `YYYY-MM-DDTHH:MM:SS[.fff]Z` time.
pub fn epoch_seconds(text: &str) -> Option<f64> {
    let text = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.splitn(3, ':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: f64 = time.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60) as f64 + second)
}

fn iso(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

// Howard Hinnant's proleptic Gregorian conversions.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_convert_both_ways() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(
            epoch_seconds("2026-10-23T07:25:51.5Z"),
            Some(1_792_740_351.5)
        );
        assert_eq!(iso(1_792_740_351), "2026-10-23T07:25:51Z");
        assert_eq!(epoch_seconds("2026-13-01T00:00:00Z"), None);
        assert_eq!(epoch_seconds("not a time"), None);
    }

    #[test]
    fn team_keeps_valid_agents_and_running_swarms() {
        let agents = br#"{"items":[
            {"name":"worker","runtime":"claude","repo":"example/app","tags":["coder"],
             "state":"busy","current_issue":7,"state_since":1792740000.5,"extra":{"x":1}},
            {"name":"bad name","state":"idle"},
            {"name":"scratch","state":"idle","configured":false},
            {"name":"reviewer","state":"waiting_for_human","reason":"question"}],"total":3}"#;
        let swarms = br#"{"items":[
            {"name":"fix","repo":"example/app","issue_number":7,"status":"running",
             "members":["worker",{"name":"scout"},{"other":1}]},
            {"name":"old","status":"done","members":[]}]}"#;
        let team = Team::decode(agents, swarms).unwrap();
        let names: Vec<_> = team.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["worker", "reviewer"]);
        assert_eq!(team.members[0].current_issue, Some(7));
        assert_eq!(team.members[1].reason.as_deref(), Some("question"));
        assert_eq!(team.swarms.len(), 1);
        assert_eq!(team.swarms[0].members, ["worker", "scout"]);
        assert_eq!(Team::decode(b"[]", swarms), Err(UNREADABLE));
        assert_eq!(Team::decode(br#"{"items":null}"#, swarms), Err(UNREADABLE));
        assert_eq!(Skills::decode(br#"{"items":{}}"#), Err(UNREADABLE));
    }

    #[test]
    fn usage_reads_sessions_totals_and_unmeasured_agents() {
        let body = br#"{"items":[{"id":"a1","agent_name":"worker","runtime":"claude",
            "parent_id":null,"models":["model-a"],"coverage":"measured","current":true,
            "input_tokens":10,"cache_read_tokens":20,"cache_write_tokens":3,
            "output_tokens":4,"total_tokens":37}],
            "totals":{"input_tokens":10,"cache_read_tokens":20,"cache_write_tokens":3,
            "output_tokens":4,"total_tokens":37,"coverage":"partial"},
            "unavailable":[{"agent_name":"desk","runtime":"shell","reason":"no source"}],
            "has_more":false}"#;
        let usage = Usage::decode(body).unwrap();
        assert_eq!(usage.totals.total, 37);
        assert_eq!(usage.coverage, "partial");
        assert_eq!(usage.sessions[0].tokens.cache_read, 20);
        assert_eq!(usage.unmeasured[0].reason, "no source");
        assert!(!usage.more);
    }

    #[test]
    fn activity_merges_deliveries_and_events_newest_first() {
        let deliveries = br#"{"items":[{"kind":"direct_message","session_name":"worker",
            "outcome":"delivered","preview":"hello","created_at":"2026-10-23T07:00:00Z"}]}"#;
        let events = br#"{"items":[{"source":"webhook","repo":"example/app","issue_number":3,
            "event_type":"comment_created","sender":"someone","summary":"a comment",
            "received_at":"2026-10-23T08:00:00Z","outcome":null}]}"#;
        let activity = Activity::decode(deliveries, events).unwrap();
        assert_eq!(activity.entries.len(), 2);
        assert!(activity.entries[0].inbound);
        assert_eq!(
            activity.entries[0].route,
            "webhook · example/app#3 · someone"
        );
        assert_eq!(activity.entries[1].route, "→ worker");
    }

    #[test]
    fn skills_and_agent_cards_decode() {
        let skills = Skills::decode(
            br#"{"store":"/skills","items":[{"name":"check","description":"d",
            "tags":["all"],"reaches":["worker"],"error":null}],"total":1}"#,
        )
        .unwrap();
        assert_eq!(skills.store.as_deref(), Some("/skills"));
        assert_eq!(skills.items[0].reaches, ["worker"]);
        let agent = Agent::decode(
            br#"{"name":"worker","known":true,"online":true,"runtime":"claude",
            "state":"busy","watches":["example/app"],"current_issue":7,
            "recent_deliveries":[{"kind":"direct_message","outcome":"queued",
            "preview":"later","created_at":"2026-10-23T07:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(agent.watches, ["example/app"]);
        assert_eq!(agent.received[0].outcome, "queued");
        assert_eq!(Agent::decode(b"{\"name\":7}"), Err(UNREADABLE));
    }
}
