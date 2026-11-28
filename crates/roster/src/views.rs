//! Read-only data for the Backbone views a pane can show.
//!
//! Each [`Request`] is a few GETs of fixed paths of the profile's API, on its
//! verified connection, as the roster's reads are. Decoding keeps only the
//! fields a view draws; an item that does not decode is skipped.
//! Team-wide libraries are gathered in the private `library` module.

use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::{valid_agent, within_depth};

pub mod bound;
pub mod content;
mod documents;
mod library;
mod reports;

pub use documents::{DOCUMENT_HTML_LIMIT, DocumentMetadata, DocumentRevision};

pub use reports::{ReportContent, ReportItem, ReportPage, ReportQuery, ReportSection};

pub use library::{
    Loadout, LocalSkill, PREVIEW_LIMIT, Place, PolicyLibrary, Reason, SkillLibrary, TagCard,
    TagLibrary, tag_carriers,
};

const VIEW_LIMIT: usize = 4 * 1024 * 1024;

// Responses can fan out into multiple visible groups. Bound expanded rendering
// work as well as wire bytes, before retaining or constructing those groups.
fn bounded_groups(entries: impl Iterator<Item = (usize, usize)>) -> bool {
    let mut rows = 0usize;
    let mut bytes = 0usize;
    for (count, size) in entries {
        rows = rows.saturating_add(count);
        bytes = bytes.saturating_add(count.saturating_mul(size));
        if rows > 4096 || bytes > 16 * 1024 * 1024 {
            return false;
        }
    }
    true
}

/// Why a view has no data: Backbone could not be reached, or did not answer
/// with it. The text is fixed and contains no response content.
pub const UNAVAILABLE: &str = "Backbone could not provide this view.";
/// A response this version cannot read.
pub const UNREADABLE: &str = "Backbone answered with data this version cannot read.";

const POLICY_LIMIT: &str = "Backbone's policies exceed this view's limits.";

/// What a view asks Backbone for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Every agent and the active swarms.
    Team,
    /// Agent chooser rows from the roster independently of swarm discovery.
    Members,
    /// Token usage over a recent period.
    Usage(Range),
    /// Recent deliveries and inbound events.
    Activity,
    /// Authored progress reports or retained answers, one page at a time.
    Reports(ReportQuery, Option<String>),
    /// Refresh an open answer independently of its former list page.
    Response(u64),
    /// One explicitly requested durable HTML revision. Never polled.
    Document(u64),
    /// Every skill an agent can load, team-wide.
    Skills,
    /// The layers a brief is built from, team-wide.
    Policies,
    /// What each tag brings.
    Tags,
}

/// What an agent's view reads besides its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Nothing more.
    Card,
    /// The skills its next launch links.
    Skills,
    /// What it receives at its next start.
    Policies,
    /// Its usage over the last 24 hours.
    Usage,
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
    /// See [`ReportPage`].
    Reports(ReportPage),
    /// See [`DocumentRevision`].
    Document(DocumentRevision),
    /// See [`SkillLibrary`].
    Skills(Box<SkillLibrary>),
    /// See [`PolicyLibrary`].
    Policies(Box<PolicyLibrary>),
    /// See [`TagLibrary`].
    Tags(Box<TagLibrary>),
    /// See [`Agent`].
    Agent(Box<Agent>),
}

/// One read chain on a verified connection. It records the first transport
/// failure, so optional roster enrichment cannot hide a rejected binding.
pub(crate) struct Api<'a> {
    connection: &'a crate::profiles::Connection,
    error: std::cell::Cell<Option<crate::profiles::Error>>,
    live: &'a dyn Fn() -> bool,
}

impl<'a> Api<'a> {
    pub(crate) fn new(
        connection: &'a crate::profiles::Connection,
        live: &'a dyn Fn() -> bool,
    ) -> Self {
        Self {
            connection,
            error: Default::default(),
            live,
        }
    }

    fn live(&self) -> bool {
        (self.live)()
    }

    fn fetch(&self, path: &str, limit: usize) -> Result<Vec<u8>, Option<u16>> {
        // A read nobody is waiting for any more starts no further request,
        // and once a chain loses its binding, neither does it.
        if !self.live() || self.error.get().is_some() {
            return Err(None);
        }
        match self.connection.response(path, None, limit) {
            Ok(response) if (200..300).contains(&response.status) => Ok(response.body),
            Ok(response) => Err(Some(response.status)),
            Err(failure) => {
                self.error.set(Some(failure));
                Err(None)
            }
        }
    }

    fn get(&self, path: &str) -> Result<Vec<u8>, &'static str> {
        self.fetch(path, VIEW_LIMIT).map_err(|_| UNAVAILABLE)
    }
}

/// What one agent receives at its next start.
fn instructions(api: &Api<'_>, name: &str) -> Result<Instructions, &'static str> {
    if !api.live() {
        return Err(UNAVAILABLE);
    }
    let path = format!("/api/agents/{name}/instructions");
    match api.fetch(&path, VIEW_LIMIT) {
        Ok(body) => Instructions::decode(&body),
        Err(_) => Err(UNAVAILABLE),
    }
}

fn usage_path(range: Range) -> String {
    let since = iso(now().saturating_sub(range.seconds()));
    format!("/api/usage?refresh=false&limit=1000&since={since}")
}

fn read_using(api: &Api<'_>, request: &bound::Request) -> Result<Data, &'static str> {
    match request {
        bound::Request::Members => Ok(Data::Team(Team {
            members: Team::members(&api.get("/api/agents")?)?,
            swarms: Vec::new(),
        })),
        bound::Request::Team => Ok(Data::Team(Team::decode(
            &api.get("/api/agents")?,
            &api.get("/api/swarms")?,
        )?)),
        bound::Request::Usage(range) => {
            Ok(Data::Usage(Usage::decode(&api.get(&usage_path(*range))?)?))
        }
        bound::Request::Activity => {
            // History is an addition: when it cannot be read, the timeline
            // still shows deliveries and states now, and says why.
            let changes = history(api, now().saturating_sub(3_600 + HISTORY_LOOKBACK));
            if !api.live() {
                return Err(UNAVAILABLE);
            }
            Ok(Data::Activity(Activity::decode(
                &api.get("/api/deliveries?limit=200")?,
                &api.get("/api/events?limit=200")?,
                &api.get("/api/agents")?,
                changes,
            )?))
        }
        bound::Request::Skills => Ok(Data::Skills(Box::new(SkillLibrary::read(api)?))),
        bound::Request::Policies => {
            let agents = library::agents(api)?;
            Ok(Data::Policies(Box::new(PolicyLibrary::read(api, &agents)?)))
        }
        bound::Request::Tags => Ok(Data::Tags(Box::new(TagLibrary::read(api)?))),
    }
}

/// A section's own read failing leaves the card; the section says why.
fn read_part(api: &Api<'_>, name: &str, part: &Part, agent: &mut Agent) {
    match part {
        Part::Card => {}
        Part::Skills => {
            agent.skills = Some(
                api.get(&format!("/api/skills/preview/{name}"))
                    .and_then(|body| Loadout::decode(&body)),
            );
        }
        Part::Policies => agent.brief = Some(instructions(api, name)),
        Part::Usage => {
            agent.usage = Some(
                api.get(&usage_path(Range::Day))
                    .and_then(|body| Usage::decode(&body))
                    .map(|usage| usage.of(name)),
            );
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
    /// Machine reported by this profile, absent on older services.
    #[serde(default, deserialize_with = "crate::lenient_object")]
    pub machine: Option<crate::Machine>,
    /// Agent name.
    pub name: String,
    /// Durable registration from a bound list, absent for unregistered rows.
    pub registration_id: Option<String>,
    /// The CLI it runs in.
    pub runtime: String,
    /// The repository it owns, or empty.
    pub repo: String,
    /// Its registered workspace folder, or empty.
    pub dir: String,
    /// Its tags.
    pub tags: Vec<String>,
    /// Its reported state.
    pub state: String,
    /// Why it is waiting or blocked, when reported.
    pub reason: Option<String>,
    /// Where the state came from (`push`, `pull`, `stale`, etc.).
    #[serde(default, deserialize_with = "crate::lenient")]
    pub state_source: Option<String>,
    /// Observations explaining the state, including stale report age.
    #[serde(default, deserialize_with = "crate::lenient_strings")]
    pub evidence: Vec<String>,
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
    fn members(agents: &[u8]) -> Result<Vec<Member>, &'static str> {
        Ok(items::<Member>(&document(agents)?)?
            .into_iter()
            .filter(|member| valid_agent(&member.name) && member.configured != Some(false))
            .map(|mut member| {
                member.machine = member.machine.filter(crate::Machine::valid);
                member
            })
            .collect())
    }

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
        let members = Self::members(agents)?;
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
pub(crate) fn member_name(raw: &RawValue) -> Option<String> {
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
    /// When Backbone first saw it, as UTC text.
    pub first_seen: String,
    /// Estimated cost in US dollars, when priced.
    pub estimated_usd: Option<String>,
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
    /// Estimated cost of the totals in US dollars, when priced.
    pub estimated_usd: Option<String>,
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
            estimated_usd: Option<String>,
        }
        let document = document(body)?;
        let field = |name: &str| document.get(name).map(|raw| raw.get());
        let totals: Totals = field("totals")
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        Ok(Self {
            totals: totals.tokens,
            coverage: totals.coverage,
            estimated_usd: totals.estimated_usd,
            sessions: items(&document)?,
            unmeasured: list(&document, "unavailable"),
            more: field("has_more") == Some("true"),
        })
    }

    /// Only `agent`'s sessions, with totals, cost and coverage over them.
    pub fn of(self, agent: &str) -> Self {
        let sessions: Vec<Session> = self
            .sessions
            .into_iter()
            .filter(|session| session.agent == agent)
            .collect();
        let mut totals = Tokens::default();
        let mut usd: Option<f64> = None;
        for session in &sessions {
            totals += session.tokens;
            if let Some(cost) = session
                .estimated_usd
                .as_deref()
                .and_then(|c| c.parse::<f64>().ok())
            {
                *usd.get_or_insert(0.0) += cost;
            }
        }
        // The weakest coverage of its sessions is the agent's coverage.
        let coverage = ["unavailable", "partial", "catching_up", "measured"]
            .into_iter()
            .find(|level| sessions.iter().any(|session| session.coverage == *level))
            .unwrap_or("unavailable")
            .to_owned();
        Self {
            totals,
            coverage,
            estimated_usd: usd.map(|usd| format!("{usd:.2}")),
            unmeasured: self
                .unmeasured
                .into_iter()
                .filter(|unmeasured| unmeasured.agent == agent)
                .collect(),
            sessions,
            more: self.more,
        }
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
    /// The agent a delivery went to; empty for an inbound event.
    pub agent: String,
}

/// Recent deliveries and events, newest first, each agent's state now, and
/// the state changes Backbone observed in the last hour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    /// The records.
    pub entries: Vec<Entry>,
    /// The configured agents with their current states.
    pub members: Vec<Member>,
    /// The state changes Backbone observed.
    pub history: History,
}

/// The state history an activity read found.
#[derive(Clone, Debug, PartialEq)]
pub enum History {
    /// It could not be read this time.
    Failed(&'static str),
    /// Observations, oldest first, read from up to [`HISTORY_LOOKBACK`]
    /// before the hour so a state that began then is drawn from the hour's
    /// start. `complete` is false when the page limit stopped the read
    /// before its oldest observation, so earlier states may be missing.
    Observed {
        /// The observations, oldest first.
        changes: Vec<Change>,
        /// Whether the read reached the oldest observation it asked for.
        complete: bool,
    },
}

impl Default for History {
    fn default() -> Self {
        Self::Observed {
            changes: Vec::new(),
            complete: true,
        }
    }
}

/// A state Backbone observed for an agent. `at` is when it was observed, not
/// the exact moment it changed; changes between observations can be missed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    /// Backbone's order of observations: a later one has a larger id.
    pub id: i64,
    /// Agent name.
    pub agent: String,
    /// Its state, as Backbone reports it.
    pub state: String,
    /// Why it is waiting or blocked, when reported.
    pub reason: Option<String>,
    /// When it was observed, in seconds since the Unix epoch.
    pub at: Option<f64>,
}

impl Change {
    /// One page of state history: its changes, whether more exist, and the
    /// cursor for the next page.
    fn page(body: &[u8]) -> Result<(Vec<Self>, bool, Option<i64>), &'static str> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Raw {
            id: i64,
            agent: String,
            state: String,
            reason: Option<String>,
            at: String,
        }
        let document = document(body)?;
        let changes = items::<Raw>(&document)?
            .into_iter()
            .filter(|raw| valid_agent(&raw.agent))
            .map(|raw| Self {
                id: raw.id,
                at: epoch_seconds(&raw.at),
                agent: raw.agent,
                state: raw.state,
                reason: raw.reason,
            })
            .collect();
        let more = document.get("has_more").map(|raw| raw.get()) == Some("true");
        let next = document
            .get("next_before_id")
            .and_then(|raw| serde_json::from_str(raw.get()).ok());
        Ok((changes, more, next))
    }
}

/// The most pages of state history one refresh reads.
const HISTORY_PAGES: usize = 4;

/// How far before the hour history is read, so a state that began earlier
/// is drawn from the hour's start.
pub const HISTORY_LOOKBACK: u64 = 6 * 3_600;

/// State changes observed since `since`, oldest first, following Backbone's
/// cursor and stopping when the read is no longer wanted.
fn history(api: &Api<'_>, since: u64) -> History {
    let mut changes = Vec::new();
    let mut before: Option<i64> = None;
    let mut complete = false;
    for _ in 0..HISTORY_PAGES {
        if !api.live() {
            break;
        }
        let cursor = before
            .map(|id| format!("&before_id={id}"))
            .unwrap_or_default();
        let path = format!("/api/agents/states?since={}&limit=500{cursor}", iso(since));
        let body = match api.fetch(&path, VIEW_LIMIT) {
            Ok(body) => body,
            Err(_) => return History::Failed(UNAVAILABLE),
        };
        let (page, more, next) = match Change::page(&body) {
            Ok(page) => page,
            Err(error) => return History::Failed(error),
        };
        changes.extend(page);
        before = next;
        if !more || before.is_none() {
            complete = true;
            break;
        }
    }
    oldest_first(&mut changes);
    History::Observed { changes, complete }
}

/// Oldest first; of observations at one time, the later one last.
fn oldest_first(changes: &mut [Change]) {
    changes.sort_by(|a, b| {
        a.at.unwrap_or(0.0)
            .total_cmp(&b.at.unwrap_or(0.0))
            .then(a.id.cmp(&b.id))
    });
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
            agent: delivery.session_name,
        }
    }
}

impl Activity {
    fn decode(
        deliveries: &[u8],
        events: &[u8],
        agents: &[u8],
        history: History,
    ) -> Result<Self, &'static str> {
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
                        agent: String::new(),
                    }
                }),
        );
        entries.sort_by(|a, b| b.at.unwrap_or(0.0).total_cmp(&a.at.unwrap_or(0.0)));
        let members = items::<Member>(&document(agents)?)?
            .into_iter()
            .filter(|member| valid_agent(&member.name) && member.configured != Some(false))
            .collect();
        Ok(Self {
            entries,
            members,
            history,
        })
    }
}

/// One entry of the shared skills store.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Skill {
    /// Stable shared artifact identity observed with this catalog row.
    pub artifact_id: Option<String>,
    /// Skill name.
    pub name: String,
    /// Its folder in the store, as Backbone reports it. In an agent's
    /// skills, a skill the service keeps without a folder carries its name.
    pub path: String,
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
    /// Group skills by tag in one pass, retaining source order and one entry per skill.
    pub fn by_tag(&self) -> std::collections::BTreeMap<&str, Vec<&Skill>> {
        let mut groups = std::collections::BTreeMap::<&str, Vec<&Skill>>::new();
        for skill in &self.items {
            for tag in skill
                .tags
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
            {
                groups.entry(tag).or_default().push(skill);
            }
        }
        groups
    }

    fn decode(body: &[u8]) -> Result<Self, &'static str> {
        let document = document(body)?;
        let items =
            skill_items(document.get("items").ok_or(UNREADABLE)?).map_err(|_| UNREADABLE)?;
        if !bounded_groups(items.iter().map(|skill| {
            (
                skill.tags.len().max(1),
                skill.name.len()
                    + skill.artifact_id.as_ref().map_or(0, String::len)
                    + skill.path.len()
                    + skill.description.len()
                    + skill.error.as_ref().map_or(0, String::len)
                    + skill.tags.iter().map(String::len).sum::<usize>(),
            )
        })) {
            return Err(UNREADABLE);
        }
        Ok(Self {
            store: document
                .get("store")
                .and_then(|raw| serde_json::from_str(raw.get()).ok()),
            items,
        })
    }
}

// Limit input rows while decoding, before allocating the complete skill list.
fn skill_items(raw: &RawValue) -> Result<Vec<Skill>, serde_json::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<Skill>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("at most 4096 skill rows")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            let mut rows = 0;
            while let Some(raw) = sequence.next_element::<&RawValue>()? {
                if rows == 4096 {
                    return Err(serde::de::Error::custom("too many skill rows"));
                }
                rows += 1;
                // Preserve the existing behavior of skipping malformed entries.
                if let Ok(item) = serde_json::from_str(raw.get()) {
                    items.push(item);
                }
            }
            Ok(items)
        }
    }
    serde::Deserializer::deserialize_seq(&mut serde_json::Deserializer::from_str(raw.get()), Rows)
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
    /// Whether its session is running.
    pub online: bool,
    /// The CLI.
    pub runtime: String,
    /// The reported or configured model, when available.
    pub model: Option<String>,
    /// The service's source for that model.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub model_source: Option<String>,
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
    /// Where the state came from (`push`, `pull`, `stale`, etc.).
    #[serde(default, deserialize_with = "crate::lenient")]
    pub state_source: Option<String>,
    /// Observations explaining the state (e.g. how old a stale report is).
    #[serde(default, deserialize_with = "crate::lenient_strings")]
    pub evidence: Vec<String>,
    /// Machine serving this agent, when reported.
    #[serde(skip)]
    pub machine: Option<crate::Machine>,
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
    /// When it entered its state, in seconds since the Unix epoch by this
    /// machine's clock: left for a reader to work out from
    /// `state_age_seconds` and the time of its read.
    #[serde(skip)]
    pub state_began: Option<f64>,
    /// Its last reply, clipped by Backbone.
    pub last_message: Option<String>,
    /// Sources it is subscribed to.
    pub subscriptions: Vec<Subscription>,
    /// Its most recent deliveries.
    #[serde(skip)]
    pub received: Vec<Received>,
    /// What it waits on outside itself; absent from an older service.
    #[serde(skip)]
    pub waiting: Option<crate::Waiting>,
    /// How much it runs in the background; absent from an older service.
    #[serde(skip)]
    pub background: Option<crate::Background>,
    /// What holds its terminal deliveries; absent when nothing does.
    #[serde(skip)]
    pub held_queue: Option<crate::HeldQueue>,
    /// Messages waiting for its terminal, the held one included; 0 when an
    /// older service does not say.
    #[serde(skip)]
    pub queued: u64,
    /// Whether a delivery to it could go now, as a code such as `human_typing`.
    #[serde(skip)]
    pub delivery: Option<String>,
    /// Its latest restart, finished or not.
    #[serde(skip)]
    pub restart: Option<crate::Restart>,
    /// Processes it started that still run.
    #[serde(skip)]
    pub processes: Vec<Process>,
    /// The skills its next launch links, when its Skills section is shown.
    #[serde(skip)]
    pub skills: Option<Result<Loadout, &'static str>>,
    /// What it receives at its next start, when its Policies section is shown.
    #[serde(skip)]
    pub brief: Option<Result<Instructions, &'static str>>,
    /// Its usage, when its Usage section is shown.
    #[serde(skip)]
    pub usage: Option<Result<Usage, &'static str>>,
}

/// A process an agent started that still runs, as the Rust service reports it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Process {
    /// Its process ID.
    pub pid: u64,
    /// Its command line, as the service wrote it.
    pub command: String,
    /// When it started, in RFC 3339 as the service wrote it.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub started_at: Option<String>,
}

/// One source of an agent's startup instructions.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Source {
    /// Shared catalog identity attached by a bound library read, never a path lookup at open.
    #[serde(skip)]
    pub artifact_id: Option<String>,
    /// The policy's name, or `base` or `swarm`.
    pub name: std::sync::Arc<str>,
    /// Its file, which also tells one source from another. A source the
    /// service keeps without a file carries its name here.
    #[serde(deserialize_with = "null_as_empty")]
    pub path: std::sync::Arc<str>,
    /// Why the agent gets it: `base`, `global`, `tag:…` or `swarm`.
    pub scope: std::sync::Arc<str>,
    /// Whether it is applied at the next start.
    pub applied: bool,
    /// The file's text, when Backbone could read it.
    pub text: Option<std::sync::Arc<str>>,
}

/// The instructions one agent receives at its next start, in order.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Instructions {
    /// Agent name.
    pub name: String,
    /// Its CLI.
    pub runtime: String,
    /// How the brief reaches it.
    pub mode: String,
    /// Whether a brief is injected.
    pub enabled: bool,
    /// The sources, in the order the agent receives them.
    #[serde(deserialize_with = "policy_sources")]
    pub sources: Vec<Source>,
    /// Backbone's notes about this preview.
    pub notices: Vec<String>,
}

impl Instructions {
    fn decode(body: &[u8]) -> Result<Self, &'static str> {
        document(body)?;
        serde_json::from_slice(body).map_err(|error| {
            // serde_json appends a location to our literal custom data error.
            // Keep budget exhaustion distinct from malformed per-agent data.
            if error.to_string().starts_with(POLICY_LIMIT) {
                POLICY_LIMIT
            } else {
                UNREADABLE
            }
        })
    }
}

// The Rust service sends null for a source that has no file.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<std::sync::Arc<str>, D::Error> {
    Ok(Option::<std::sync::Arc<str>>::deserialize(deserializer)?.unwrap_or_default())
}

// Both the team library and an individual agent use this decoder. Reject excess
// rows before constructing the full list, including defaultable empty objects.
fn policy_sources<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Source>, D::Error> {
    struct Sources;
    impl<'de> serde::de::Visitor<'de> for Sources {
        type Value = Vec<Source>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded list of policy sources")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut sources = Vec::new();
            let mut rows = 0usize;
            let mut bytes = 0usize;
            while sources.len() < 4096 {
                let Some(mut source) = sequence.next_element::<Source>()? else {
                    return Ok(sources);
                };
                if source.path.is_empty() {
                    source.path = source.name.clone();
                }
                let count = source.scope.split(", ").count();
                let size = source.name.len()
                    + source.path.len()
                    + source.scope.len()
                    + source.text.as_ref().map_or(0, |text| text.len());
                rows = rows.saturating_add(count);
                bytes = bytes.saturating_add(count.saturating_mul(size));
                if rows > 4096 || bytes > 16 * 1024 * 1024 {
                    return Err(serde::de::Error::custom(POLICY_LIMIT));
                }
                sources.push(source);
            }
            if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(POLICY_LIMIT));
            }
            Ok(sources)
        }
    }
    deserializer.deserialize_seq(Sources)
}

/// An outside source an agent receives messages from.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Subscription {
    /// The source, such as `gmail`.
    pub source: String,
    /// What it matches.
    pub filter: String,
    /// Its priority.
    pub priority: String,
}

impl Agent {
    pub(crate) fn decode(body: &[u8]) -> Result<Self, &'static str> {
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
        // Null or mistyped reads as absent, never as an unreadable card.
        let field = |name: &str| document.get(name).map(|raw| raw.get());
        agent.machine = field("machine")
            .and_then(crate::object::<crate::Machine>)
            .filter(crate::Machine::valid);
        agent.waiting = field("waiting").and_then(crate::object);
        agent.background = field("background").and_then(crate::Background::read);
        agent.held_queue = field("held_queue").and_then(crate::object);
        agent.queued = field("queued")
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or(0);
        agent.delivery = field("delivery").and_then(|raw| serde_json::from_str(raw).ok());
        agent.restart = field("restart").and_then(crate::object);
        agent.processes = field("processes")
            .and_then(|raw| serde_json::from_str::<Vec<&RawValue>>(raw).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|process| crate::object(process.get()))
            .collect();
        Ok(agent)
    }
}

type Document<'a> = BTreeMap<String, &'a RawValue>;

/// Percent-encode a query value, keeping only RFC 3986 unreserved bytes.
pub(crate) fn encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

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
    if !(0..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0.0..61.0).contains(&second)
    {
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
    #[test]
    fn policy_decoding_bounds_rows_and_expanded_content_for_every_view() {
        let rows = std::iter::repeat_n("{}", 4096)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            super::Instructions::decode(format!("{{\"sources\":[{rows}]}}").as_bytes())
                .unwrap()
                .sources
                .len(),
            4096
        );
        assert_eq!(
            super::Instructions::decode(format!("{{\"sources\":[{rows},{{}}]}}").as_bytes())
                .unwrap_err(),
            super::POLICY_LIMIT
        );
        let body = serde_json::to_vec(&serde_json::json!({"sources":[{
            "text": "x".repeat(17 * 1024),
            "scope": (0..1024).map(|n| format!("tag:{n}")).collect::<Vec<_>>().join(", ")
        }]}))
        .unwrap();
        assert_eq!(
            super::Instructions::decode(&body).unwrap_err(),
            super::POLICY_LIMIT
        );
        assert_eq!(
            super::Instructions::decode(br#"{"sources":[null]}"#).unwrap_err(),
            super::UNREADABLE
        );
    }

    #[test]
    fn expanded_groups_are_bounded_before_rendering() {
        assert!(super::bounded_groups([(2, 1024)].into_iter()));
        assert!(!super::bounded_groups([(4097, 1)].into_iter()));
        assert!(!super::bounded_groups([(32, 1024 * 1024)].into_iter()));
        let skills = super::Skills::decode(
            br#"{"items":[{"name":"first","tags":["x","x"]},{"name":"second","tags":["x"]}]}"#,
        )
        .unwrap();
        let grouped = skills.by_tag();
        assert_eq!(
            grouped["x"]
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        // A small response must not fan out its error into large UI strings.
        let body = serde_json::to_vec(&serde_json::json!({"items":[{
            "name":"unreadable", "error":"x".repeat(17 * 1024),
            "tags": (0..1024).map(|n| n.to_string()).collect::<Vec<_>>()
        }]}))
        .unwrap();
        assert_eq!(super::Skills::decode(&body).unwrap_err(), super::UNREADABLE);
        let rows = std::iter::repeat_n("{}", 4096)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            super::Skills::decode(format!("{{\"items\":[{rows}]}}").as_bytes())
                .unwrap()
                .items
                .len(),
            4096
        );
        // Invalid rows still consume input work; do not decode an unlimited tail.
        assert_eq!(
            super::Skills::decode(format!("{{\"items\":[{rows},null]}}").as_bytes()).unwrap_err(),
            super::UNREADABLE
        );
    }

    use super::*;

    #[test]
    fn policies_decode() {
        let brief = Instructions::decode(
            br##"{"name":"worker","runtime":"claude","mode":"system","enabled":true,
            "sources":[{"name":"base","path":"/t/base.md","scope":"base","applied":true,"text":"# Base"},
            {"name":"review","path":"/t/review.md","scope":"tag:review","applied":true,"text":null}],
            "content":"...","notices":["note"]}"##,
        )
        .unwrap();
        assert_eq!(brief.sources[1].scope.as_ref(), "tag:review");
        assert_eq!(brief.sources[1].text, None);
    }

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
        for invalid in [
            "2026-11-10T-5:00:00Z",
            "2026-11-10T00:-9:00Z",
            "2026-11-10T00:00:NaNZ",
            "2026-11-10T00:00:-infZ",
            "99999999999999999-01-01T00:00:00Z",
            "-9223372036854775808-01-01T00:00:00Z",
        ] {
            assert_eq!(epoch_seconds(invalid), None, "{invalid}");
        }
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

        let mine = usage.clone().of("worker");
        assert_eq!(mine.totals.total, 37);
        assert_eq!(mine.coverage, "measured");
        assert!(mine.unmeasured.is_empty());
        let none = usage.of("desk");
        assert_eq!(none.totals.total, 0);
        assert_eq!(none.coverage, "unavailable");
        assert_eq!(none.unmeasured.len(), 1);
    }

    #[test]
    fn state_history_pages_decode_with_their_cursor() {
        let (changes, more, next) = Change::page(
            br#"{"items":[{"id":42,"agent":"worker","state":"busy","reason":null,
            "at":"2026-10-23T14:02:00.000000Z"},{"id":41,"agent":"bad name","state":"idle",
            "at":"2026-10-23T14:01:00Z"}],"has_more":true,"next_before_id":41}"#,
        )
        .unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].state, "busy");
        assert_eq!(changes[0].id, 42);
        assert_eq!(changes[0].at, epoch_seconds("2026-10-23T14:02:00Z"));
        assert!(more);
        assert_eq!(next, Some(41));
        assert!(Change::page(br#"{"items":{}}"#).is_err());
        // Newest first from Backbone; equal times keep the later id last.
        let (mut changes, _, _) = Change::page(
            br#"{"items":[{"id":8,"agent":"worker","state":"idle","at":"2026-10-23T14:02:00Z"},
            {"id":7,"agent":"worker","state":"busy","at":"2026-10-23T14:02:00Z"}]}"#,
        )
        .unwrap();
        oldest_first(&mut changes);
        assert_eq!(changes[1].state, "idle");
    }

    #[test]
    fn activity_merges_deliveries_and_events_newest_first() {
        let deliveries = br#"{"items":[{"kind":"direct_message","session_name":"worker",
            "outcome":"delivered","preview":"hello","created_at":"2026-10-23T07:00:00Z"}]}"#;
        let events = br#"{"items":[{"source":"webhook","repo":"example/app","issue_number":3,
            "event_type":"comment_created","sender":"someone","summary":"a comment",
            "received_at":"2026-10-23T08:00:00Z","outcome":null}]}"#;
        let agents = br#"{"items":[{"name":"worker","state":"busy"}]}"#;
        let activity =
            Activity::decode(deliveries, events, agents, History::Failed(UNAVAILABLE)).unwrap();
        assert_eq!(activity.entries.len(), 2);
        assert!(activity.entries[0].inbound);
        assert_eq!(
            activity.entries[0].route,
            "webhook · example/app#3 · someone"
        );
        assert_eq!(activity.entries[1].route, "→ worker");
        assert_eq!(activity.entries[1].agent, "worker");
        assert_eq!(activity.members[0].state, "busy");
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
        // An older service has none of the background fields.
        assert_eq!((agent.waiting, agent.background), (None, None));
        assert!(agent.processes.is_empty());
    }

    #[test]
    fn inspection_keeps_the_model_when_the_source_is_unfamiliar_or_mistyped() {
        for (body, expected) in [
            (
                br#"{"name":"worker","model":"example","model_source":"future"}"#.as_slice(),
                Some("future"),
            ),
            (
                br#"{"name":"worker","model":"example","model_source":42}"#.as_slice(),
                None,
            ),
            (br#"{"name":"worker","model":"example"}"#.as_slice(), None),
        ] {
            let agent = Agent::decode(body).unwrap();
            assert_eq!(agent.model.as_deref(), Some("example"));
            assert_eq!(agent.model_source.as_deref(), expected);
        }
    }

    #[test]
    fn an_inspected_agent_carries_its_wait_and_background_from_the_services_fixture() {
        let agent =
            Agent::decode(include_bytes!("profiles/runs/fixtures/agent-inspect.json")).unwrap();
        assert_eq!(
            agent.machine,
            Some(crate::Machine {
                id: "5f2c9e0d1b7a4e38".into(),
                name: "laptop".into(),
            })
        );
        assert_eq!(agent.state_source.as_deref(), Some("push"));
        assert_eq!(
            agent.evidence,
            [
                "runtime: claude",
                "hook state 'busy' written 4s ago (fresh)"
            ]
        );
        assert_eq!(agent.waiting, None);
        assert_eq!(
            agent.background,
            Some(crate::Background {
                running: 1,
                queued: 0,
                processes: 1
            })
        );
        assert_eq!(
            agent.processes,
            [Process {
                pid: 48213,
                command: "cargo test --workspace".into(),
                started_at: Some("2026-11-10T10:14:02Z".into()),
            }]
        );
        // The stream's agent.changed carries the same shape, with the agent waited on.
        let changed = include_bytes!("profiles/runs/fixtures/agent-changed.json");
        let waiting = Agent::decode(changed).unwrap().waiting.unwrap();
        assert_eq!(
            (waiting.on.as_str(), waiting.on_agent.as_deref()),
            ("agent", Some("reviewer"))
        );
        // Mistyped fields read as absent and leave the card.
        let agent = Agent::decode(
            br#"{"name":"worker","waiting":"soon","background":{"running":-1},
            "processes":[{"pid":"x","command":"a"},{"pid":7,"command":"b","started_at":null},
            [8,"c","2026-11-10T10:14:02Z"],{"pid":9,"command":"d","started_at":7}]}"#,
        )
        .unwrap();
        assert_eq!((agent.waiting, agent.background), (None, None));
        // An array is not a process; a mistyped start time leaves the process.
        let pids: Vec<_> = agent.processes.iter().map(|process| process.pid).collect();
        assert_eq!(pids, [7, 9]);
        assert_eq!(agent.processes[1].started_at, None);
        let agent = Agent::decode(br#"{"name":"worker","background":[1,0,0]}"#).unwrap();
        assert_eq!(agent.background, None);
    }

    #[test]
    fn an_inspected_agent_carries_its_held_deliveries_from_the_services_fixtures() {
        let decode = |body: &[u8]| Agent::decode(body).unwrap();
        let held = decode(include_bytes!(
            "profiles/runs/fixtures/agent-inspect-held-queue.json"
        ));
        assert_eq!(
            (held.held_queue, held.queued, held.delivery.as_deref()),
            (
                Some(crate::HeldQueue {
                    reason: Some("uncertain_delivery".into())
                }),
                8,
                Some("agent_working")
            )
        );
        let cleared = decode(include_bytes!(
            "profiles/runs/fixtures/agent-inspect-held-queue-cleared.json"
        ));
        assert_eq!((cleared.held_queue, cleared.queued), (None, 0));
        // Today's service gives neither field; mistyped ones leave the card.
        let today = decode(include_bytes!("profiles/runs/fixtures/agent-inspect.json"));
        assert_eq!((today.held_queue, today.queued), (None, 0));
        let restarting = decode(include_bytes!(
            "profiles/runs/fixtures/agent-inspect-restarting.json"
        ));
        let restart = restarting.restart.unwrap();
        assert_eq!(
            (restart.id, restart.phase.as_deref(), restart.result),
            (Some(12), Some("waiting_for_turn"), Some(Default::default()))
        );
        assert_eq!(today.restart, None);
        let mistyped = decode(br#"{"name":"worker","held_queue":[],"queued":"3","delivery":7}"#);
        assert_eq!(
            (mistyped.held_queue, mistyped.queued, mistyped.delivery),
            (None, 0, None)
        );
    }
}
